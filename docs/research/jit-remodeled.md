# JIT Remodel: Path to 20–50x Over Interpreter

> Root-cause analysis of why the current JIT achieves only 1.3–1.7x (3→4–5
> MIPS), and a concrete restructuring plan to reach 60–150 MIPS (20–50x).
>
> Based on line-by-line review of `helm-jit/` (cache, regs, helpers, dynasm
> emitters) and `helm-engine/` (jit.rs, hot loop integration).

---

## Resolution Status (April 2026)

### JIT Root Causes (RC-1..7) — All Open

These are the major JIT restructuring items that require multi-week implementation
effort. None have been started yet. They form the JIT Phase 1–4 roadmap below.

| ID | Title | Status |
|----|-------|--------|
| RC-1 | No register pinning | Open — Phase 1 |
| RC-2 | Memory access via call to C helper | Open — Phase 1 |
| RC-3 | No block chaining | Open — Phase 2 |
| RC-4 | arch_to_flat/flat_to_arch copies 31 regs | Open — subsumed by RC-1 + RC-3 |
| RC-5 | Conditional branches emit two exit paths | Open — Phase 2 |
| RC-6 | Direct-mapped cache with 4096 entries | **Partially fixed** — `60b6f6b` upgraded to 2-way set-associative (DI-09) |
| RC-7 | No FP/SIMD JIT coverage | Open — Phase 3 |

### Interpreter Bottlenecks (IB-1..6) — 3 Fixed, 2 Deferred, 1 N/A

| ID | Title | Status | Commit |
|----|-------|--------|--------|
| IB-1 | InstrumentedMem used for all loads/stores | **Fixed** | `9f5cf08` Phase A — extra pre-read in write() removed; remaining records_mem_access gate is correct (timing needs on_mem_access) |
| IB-2 | DecodedAarch64Insn copied by value on cache hit | **Fixed** | `e89de0c` — removed redundant with_pc() copy; lookup simplified (key, raw) |
| IB-3 | Session accessor chain called 3x per instruction | Deferred | Requires restructuring step_aarch64() borrow pattern |
| IB-4 | EXIT_CHAIN defined but never emitted | N/A | Confirmed; will be addressed when block chaining (RC-3) is implemented |
| IB-5 | Stencil NZCV uses pushfq/popfq | Deferred | Requires stencil C codegen rebuild pipeline |
| IB-6 | Vec allocation per JIT cache miss | **Fixed** | `e89de0c` — pre-allocated reusable Vec<Instruction> in HelmEngine |

### Related Design-Issues Fixes

Several items from `design-issues.md` also improve interpreter/JIT performance:

| DI | Title | Impact |
|----|-------|--------|
| DI-09 | 2-way decode cache | Reduces collision evictions (RC-6 partial fix) |
| DI-11 | Pin<Box> for CompiledBlock | Lifetime safety for JIT blocks |
| DI-26 | InstrumentedMem limit 8→16 | Supports SIMD multi-register instrumentation |
| DI-30 | Callback dispatch bitmask | Faster has_any_callbacks() on hot path |

---

## 1. Current Performance Profile

| Mode | MIPS | Speedup vs. Interp |
|------|------|--------------------|
| Interpreter (VirtualTiming) | ~3 | 1.0x |
| Stencil JIT | ~4 | 1.3x |
| Tiered (stencil + dynasm hot) | ~5 | 1.7x |
| **Target** | **60–150** | **20–50x** |

**Comparable systems:** QEMU TCG (AArch64→x86-64) achieves 100–500 MIPS on the
same class of workloads. The gap is not a fundamental limit — it's architectural.

---

## 2. Root Causes: Why Only 1.7x

The JIT is slow because it eliminates only **decode overhead** (~15–20% of
interpreter time) while preserving or worsening every other bottleneck. Seven
structural issues prevent the expected 20–50x speedup:

### RC-1: No Register Pinning — All Registers In Memory

**Current:** All 48 guest register slots live in a `[u64; 48]` array in memory,
addressed via `[rdi + offset]`. Every guest register read is a memory load;
every write is a memory store.

**Example — ADD X0, X1, X2** emits:
```x86
mov rax, [rdi + 8]      ; load X1 from memory
mov rcx, [rdi + 16]     ; load X2 from memory
add rax, rcx
mov [rdi + 0], rax      ; store X0 to memory
```

**What QEMU does:** Pins 8–12 guest registers to x86-64 registers (`r8`–`r15`,
`rbx`, `rbp`). Common registers (X0–X7, SP, LR) are accessed as register-to-
register operations — zero memory traffic.

**Impact:** Every instruction pays 2–3 memory loads + 1 store for register
access. On modern x86-64 (L1 hit = 4 cycles), this adds **8–16 cycles per
instruction** that QEMU doesn't pay.

**Estimated speedup from fixing:** 3–5x alone.

### RC-2: Memory Access Via `call` to C Helper — Not Inlined

**Current:** Every load/store emits a full function call to `jit_mem_read` /
`jit_mem_write`:

```x86
; For a single LDR X0, [X1]:
push rdi              ; save regs ptr
push rsi              ; save mem ptr
sub rsp, 16           ; output buffer + alignment
mov rdi, rsi          ; arg1: mem pointer
mov rsi, r8           ; arg2: address
mov edx, 8            ; arg3: size
lea rcx, [rsp]        ; arg4: output pointer
mov rax, <jit_mem_read>
call rax              ; CALL OVERHEAD: 15+ cycles
mov rcx, [rsp]        ; read result
add rsp, 16
pop rsi
pop rdi
test rax, rax
jnz >fault
mov rax, rcx
```

That's **~20 x86 instructions** (including 2 pushes, 1 call, 2 pops, stack
alignment) for a single guest memory access.

**What QEMU does:** Inline TLB lookup + direct host pointer dereference:
```x86
; QEMU softmmu inline fast path (~6 instructions):
mov rax, guest_addr
shr rax, 12
and rax, TLB_MASK
cmp [tlb + rax*16], guest_page   ; TLB tag check
jne >slow_path
mov result, [host_ptr + offset]  ; DIRECT HOST POINTER ACCESS
```

**Impact:** Load-heavy workloads (50%+ of instructions) pay ~20 extra
instructions per memory access. This alone is **4–8x** slower than QEMU's
inline TLB.

**Estimated speedup from fixing:** 4–8x for memory-intensive workloads.

### RC-3: No Block Chaining — Every Block Exits to Interpreter

**Current:** Every compiled block ends with:
```x86
mov rax, EXIT_END_OF_BLOCK
ret                    ; return to Rust dispatch loop
```

The Rust dispatch loop then:
1. Calls `flat_to_arch()` — copies 31 registers from flat array → ArchState
2. Looks up next PC in JIT cache (hash + tag compare)
3. If hit: calls `arch_to_flat()` — copies 31 registers back
4. Calls block entry via `unsafe { (block.entry)(regs, mem) }`

**Per-block overhead:** ~100–200 cycles for register sync + cache lookup +
function call setup + return.

**What QEMU does:** Direct block chaining — the epilogue of block A patches
its exit jump to point directly to block B's entry:
```x86
; Block A epilogue (after chaining):
jmp block_B_entry     ; 0 cycles of dispatch overhead
```

**Impact:** Average basic block is 5–8 instructions. With 100–200 cycles of
dispatch overhead per block, that's **15–40 cycles overhead per guest
instruction** — more than the instruction itself costs to execute.

**Estimated speedup from fixing:** 3–5x.

### RC-4: `arch_to_flat` / `flat_to_arch` Copies 31 Registers Per Block

**Current:** Every block entry/exit copies all 31 integer registers between
`Aarch64ArchState` and the flat `[u64; 48]` array. Two full copies per block:
- `arch_to_flat`: 31 loads + 31 stores + 6 misc = ~70 memory ops
- `flat_to_arch`: 31 loads + 31 stores + 1 zeroing = ~63 memory ops

**Total:** ~133 memory operations per block boundary. At L1 latency, that's
~500 cycles per block.

**What QEMU does:** Doesn't copy — pinned registers are always live. Only
spills to memory on JIT→interpreter transitions (rare).

**Estimated speedup from fixing:** Subsumed by RC-1 and RC-3.

### RC-5: Conditional Branches Emit TWO Exit Paths — No Branch Continuation

**Current:** Every conditional branch (B.cond, CBZ, CBNZ, TBZ, TBNZ) emits
TWO complete block exits — one for taken, one for not-taken:

```x86
; B.cond:
bt r8d, 30            ; check Z flag
jnc >not_taken

; Taken path:
mov rax, <target>
mov [rdi + pc_off], rax
mov rax, EXIT_END_OF_BLOCK
ret

; Not-taken path:
not_taken:
mov rax, <fallthrough>
mov [rdi + pc_off], rax
mov rax, EXIT_END_OF_BLOCK
ret
```

Every conditional branch terminates the block, returning to the interpreter.
A tight loop of 5 instructions with a backward branch generates blocks of
only 1–5 instructions each.

**What QEMU does:** The not-taken path falls through to the next block
(inlined). Only the taken path exits (and even that gets chained). Result:
conditional branches are nearly free for predicted directions.

**Estimated speedup from fixing:** 2–3x for branch-heavy workloads.

### RC-6: Direct-Mapped Cache With 4096 Entries — High Collision Rate

**Current:** `JitCache` uses `(pc >> 2) & 0xFFF` as index. Two functions
separated by 16 KB alias to the same slot. Silent eviction means hot blocks
get evicted by cold blocks — triggering recompilation.

**What QEMU does:** Hash table with chaining — no silent eviction. Uses
per-thread translation buffers with ~256K entry capacity.

### RC-7: No FP/SIMD JIT Coverage — Falls Back to Interpreter

**Current:** The emitter supports:
- Data processing: ADD/SUB/AND/ORR/EOR/ANDS (imm + reg), MOV variants
- Load/store: LDR/STR/LDP/STP (immediate offset only — no register offset)
- Branches: B/BL/BR/BLR/RET/CBZ/CBNZ/B.cond/TBZ/TBNZ
- System: SVC/NOP/BRK/WFI (exit codes only)

**Not covered:** All FP scalar, SIMD, MUL/DIV, CSEL, BFM, EXTR, CLZ, REV,
MADD, SMULL, UMULL, atomic LSE, register-offset loads, ADRP, LDR literal,
PRFM, DC ZVA, LDXR/STXR.

Any block containing an unsupported opcode terminates compilation at that
point. The remaining instructions fall back to interpreter. For typical
workloads, 30–50% of instructions hit unsupported opcodes.

---

## 3. Theoretical Speedup Budget

| Fix | Estimated Speedup | Cumulative |
|-----|-------------------|------------|
| Register pinning (RC-1) | 3–5x | 3–5x |
| Inline TLB memory access (RC-2) | 4–8x | 6–15x |
| Block chaining (RC-3) | 3–5x | 10–30x |
| Branch continuation (RC-5) | 2–3x | 15–50x |
| Opcode coverage (RC-7) | 1.5–2x | 20–80x |
| Cache improvements (RC-6) | 1.1–1.3x | 22–100x |

Not all factors are independent (they interact), but the compound effect
clearly reaches the 20–50x range. The three biggest wins — register pinning,
inline TLB, block chaining — together account for 10–30x.

---

## 4. Restructured JIT Architecture

### 4.1 Register Allocation: Pin Guest Registers to x86-64

**Proposed mapping:**

| x86-64 | Guest | Role |
|--------|-------|------|
| `r8` | X0 | arg/return |
| `r9` | X1 | arg |
| `r10` | X2 | arg |
| `r11` | X3 | arg |
| `r12` | X19 | callee-saved |
| `r13` | X29 (FP) | frame pointer |
| `r14` | X30 (LR) | link register |
| `r15` | SP | stack pointer |
| `rbx` | X4 | arg |
| `rbp` | NZCV (packed) | flags |
| `rdi` | reg array ptr | spill base (for X5–X28) |
| `rsi` | mem/TLB ptr | memory context |

**Spill protocol:** X5–X18, X20–X28 remain in the flat array (`[rdi + off]`).
Only accessed when actually used by a block — lazy load/store.

**Impact:** The 8 most-used registers (X0–X4, FP, LR, SP) become register-to-
register operations. An ADD X0, X1, X2 compiles to:

```x86
; WITH register pinning:
lea r8, [r9 + r10]    ; 1 instruction, 1 cycle
; vs. WITHOUT (current):
mov rax, [rdi+8]      ; 4 instructions, 8+ cycles
mov rcx, [rdi+16]
add rax, rcx
mov [rdi+0], rax
```

### 4.2 Inline Softmmu TLB: Eliminate Helper Calls

**Proposed inline fast path:**

```x86
; LDR X0, [X1] with inline TLB:
mov rax, r9                ; guest VA = X1 (pinned)
mov rcx, rax
shr rcx, 12               ; page number
and ecx, TLB_MASK         ; TLB index (1023 entries)
; TLB entry: [tag (8 bytes), host_ptr (8 bytes)]
lea rdx, [tlb_base + rcx*16]
cmp [rdx], rax             ; tag == VA page?
jne >slow_path
mov r8, [rdx + 8]          ; host pointer
mov r8, [r8 + rax & 0xFFF] ; DIRECT HOST MEMORY ACCESS
jmp >done
slow_path:
; call helper only on TLB miss (~1% of accesses)
<call jit_mem_read>
done:
```

**Total:** 8 instructions on TLB hit (vs. 20+ current).
**TLB hit rate:** 95–99% for typical workloads.

**Implementation:**
- Pass TLB base address in a callee-saved register or at fixed offset in
  the register array
- TLB entries: `struct { va_page: u64, host_ptr: *mut u8 }` — 16 bytes each
- TLB size: 1024 entries = 16 KB (fits L1 cache)

### 4.3 Block Chaining: Direct Jump Between Blocks

**Proposed mechanism:**

```
Block A (ends with B <target>):
  ...
  jmp <block_B_entry>        ; patched at link time

Block B:
  ...
  b.eq <block_A_entry>       ; back-edge (loop)
  jmp <block_C_entry>        ; fall-through
```

**Implementation:**
1. Each block has a **patch list**: `Vec<(offset, target_pc)>` of exit points
   that need linking.
2. After compilation, the engine **links** block exits to their targets:
   - If target is already compiled: patch `jmp` to target's entry address
   - If not compiled: leave as `ret` (falls back to interpreter dispatch)
3. On JIT cache eviction: **unlink** all blocks that jump to the evicted block
   (replace with `ret`).

**No `flat_to_arch` / `arch_to_flat` needed:** Registers stay pinned across
block transitions. Only spill on JIT→interpreter fallback.

### 4.4 Branch Continuation: Don't Exit on Conditional Branches

**Proposed approach — trace-through-branches:**

Instead of terminating blocks at conditional branches, emit both paths
inline (up to a depth limit):

```x86
; B.EQ <target> — conditional:
test ebp, (1 << 30)        ; check Z in pinned NZCV
jz >fallthrough

; Taken path: chain to target block
jmp <target_block>

fallthrough:
; Continue with next instruction in this block
; (no exit, no dispatch, no register sync)
<next_instruction>
<next_instruction>
...
```

**For backward branches (loops):** Emit the not-taken (fall-through) path
inline. The taken (backward) path jumps to the block start (self-chaining
loop body).

**Result:** A tight loop like:
```aarch64
loop:
  ldr x0, [x1], #8
  add x2, x2, x0
  subs x3, x3, #1
  b.ne loop
```
compiles to a **single x86 basic block** with an internal backward jump.
No dispatch overhead per iteration.

### 4.5 Expanded Opcode Coverage

**Priority order (by frequency in typical workloads):**

| Tier | Opcodes | Coverage Impact |
|------|---------|-----------------|
| **T1** | CSEL/CSINC/CSINV/CSNEG | +8% coverage |
| **T1** | MUL/MADD/MSUB | +5% |
| **T1** | ADRP/ADR | +5% |
| **T1** | Register-offset LDR/STR | +10% |
| **T2** | BFM/UBFM/SBFM (BFI/BFX/LSL/LSR/ASR) | +8% |
| **T2** | CLZ/RBIT/REV | +3% |
| **T2** | EXTR | +1% |
| **T2** | LDXR/STXR/LDAXR/STLXR | +3% |
| **T3** | FP scalar (FADD/FMUL/FCMP/FCVT) | +5% |
| **T3** | SIMD basic (DUP/INS/UMOV/MOV) | +3% |

**Expected coverage:** T1 alone brings coverage from ~50% to ~78%. T1+T2
reaches ~92%. T1+T2+T3 reaches ~97%.

### 4.6 Trace Compilation (Phase 2)

Once block chaining and branch continuation work, the next step is **trace
recording:**

1. **Profile:** Count executions per edge (taken/not-taken) via inline
   counters in JIT'd code.
2. **Record:** When an edge exceeds threshold (e.g., 1000 executions), record
   a trace from that point: follow the hot path through multiple blocks.
3. **Compile:** Compile the entire trace as one x86 function with all
   pinned registers and inlined memory access. Side exits go to interpreter
   or deoptimize to block-level JIT.

**Expected speedup over basic block JIT:** 1.5–3x for loop-heavy workloads
(reduces block exit/entry overhead to zero).

---

## 5. Restructuring Plan

### Phase 1: Register Pinning + Inline TLB (Target: 15–25 MIPS)

**Duration:** 2–3 weeks

| Step | Work |
|------|------|
| 1.1 | Define x86-64 register mapping (table in §4.1) |
| 1.2 | Rewrite `regs.rs`: replace flat array access with register-pinned macros |
| 1.3 | Rewrite `dp.rs`: emit register-to-register ops for pinned regs |
| 1.4 | Rewrite `ldst.rs`: inline TLB fast path, call helper only on miss |
| 1.5 | Rewrite `branch.rs`: use pinned NZCV register for condition checks |
| 1.6 | Add TLB sync: flush JIT TLB on TLBI / SCTLR write |
| 1.7 | Update `arch_to_flat` / `flat_to_arch`: only sync non-pinned regs |
| 1.8 | Benchmark: measure MIPS on busybox, fish, Alpine boot workloads |

**Key files changed:**
- `framework/helm-jit/src/regs.rs` — register mapping constants
- `framework/helm-jit/src/dynasm/emit/dp.rs` — data processing emitters
- `framework/helm-jit/src/dynasm/emit/ldst.rs` — load/store emitters
- `framework/helm-jit/src/dynasm/emit/branch.rs` — branch emitters
- `framework/helm-jit/src/helpers.rs` — TLB slow path helper
- `runtime/helm-engine/src/jit.rs` — TLB context setup

### Phase 2: Block Chaining + Branch Continuation (Target: 40–80 MIPS)

**Duration:** 2–3 weeks

| Step | Work |
|------|------|
| 2.1 | Add `PatchSite` struct to `CompiledBlock` (offset, target_pc, type) |
| 2.2 | Add `link_blocks()` method to `JitCache` — patch exits to targets |
| 2.3 | Add `unlink_block()` — revert patches on eviction |
| 2.4 | Implement fall-through continuation for conditional branches |
| 2.5 | Implement backward-branch self-loop (single x86 block for tight loops) |
| 2.6 | Remove `flat_to_arch`/`arch_to_flat` from block-to-block transitions |
| 2.7 | Only sync registers on JIT→interpreter transitions |
| 2.8 | Upgrade cache to hash table with chaining (replace direct-mapped) |
| 2.9 | Benchmark: measure MIPS, block chain hit rate |

**Key files changed:**
- `framework/helm-jit/src/block.rs` — add PatchSite, link/unlink
- `framework/helm-jit/src/cache.rs` — hash table with chaining
- `framework/helm-jit/src/dynasm/emit/branch.rs` — chainable exits
- `framework/helm-jit/src/dynasm/mod.rs` — patch list generation
- `runtime/helm-engine/src/jit.rs` — chained dispatch loop

### Phase 3: Opcode Coverage Expansion (Target: 60–120 MIPS)

**Duration:** 2–4 weeks

| Step | Work |
|------|------|
| 3.1 | T1 opcodes: CSEL, MUL/MADD, ADRP, register-offset LDR/STR |
| 3.2 | T2 opcodes: BFM family, CLZ/REV, EXTR, exclusives |
| 3.3 | T3 opcodes: FP scalar (using SSE2/AVX), basic SIMD |
| 3.4 | Add opcode coverage tracking (% of instructions compiled) |
| 3.5 | Add JIT-vs-interpreter differential test suite per opcode |
| 3.6 | Benchmark: per-opcode-class compilation rate |

### Phase 4: Trace Compilation (Target: 100–150 MIPS)

**Duration:** 3–4 weeks

| Step | Work |
|------|------|
| 4.1 | Add edge counters to conditional branch exits |
| 4.2 | Implement trace recorder (follow hot path through blocks) |
| 4.3 | Implement trace compiler (compile full trace as single function) |
| 4.4 | Add side-exit to interpreter on trace deoptimization |
| 4.5 | Benchmark: compare trace vs. block-level on SPECint-like workloads |

---

## 6. Per-Host-Instruction Cost Analysis

### Current: ADD X0, X1, X2

| x86 insn | Cycles | Purpose |
|----------|--------|---------|
| `mov rax, [rdi+8]` | 4 | Load X1 from memory |
| `mov rcx, [rdi+16]` | 4 | Load X2 from memory |
| `add rax, rcx` | 1 | The actual addition |
| `mov [rdi+0], rax` | 4 | Store X0 to memory |
| **Total** | **~13** | **4 host insns / 1 guest insn** |

### After Phase 1: ADD X0, X1, X2 (with register pinning)

| x86 insn | Cycles | Purpose |
|----------|--------|---------|
| `lea r8, [r9+r10]` | 1 | ADD using pinned X0=r8, X1=r9, X2=r10 |
| **Total** | **~1** | **1 host insn / 1 guest insn** |

### Current: LDR X0, [X1, #8]

| x86 insn | Cycles | Purpose |
|----------|--------|---------|
| `mov r8, [rdi+8]` | 4 | Load X1 (base) |
| `add r8, 8` | 1 | Add offset |
| `push rdi` | 1 | Save regs ptr |
| `push rsi` | 1 | Save mem ptr |
| `sub rsp, 16` | 1 | Stack alignment |
| `mov rdi, rsi` | 1 | arg1: mem |
| `mov rsi, r8` | 1 | arg2: addr |
| `mov edx, 8` | 1 | arg3: size |
| `lea rcx, [rsp]` | 1 | arg4: out ptr |
| `mov rax, <helper>` | 1 | Load function ptr |
| `call rax` | ~15 | CALL + helper body + RET |
| `mov rcx, [rsp]` | 4 | Read result |
| `add rsp, 16` | 1 | Restore stack |
| `pop rsi` | 1 | Restore mem |
| `pop rdi` | 1 | Restore regs |
| `test rax, rax` | 1 | Check fault |
| `jnz >fault` | 0 | (predicted not-taken) |
| `mov rax, rcx` | 1 | Move to result reg |
| `mov [rdi+0], rax` | 4 | Store X0 |
| **Total** | **~40+** | **~20 host insns / 1 guest insn** |

### After Phase 1: LDR X0, [X1, #8] (with inline TLB)

| x86 insn | Cycles | Purpose |
|----------|--------|---------|
| `lea rax, [r9+8]` | 1 | Effective address (X1 pinned in r9) |
| `mov rcx, rax` | 1 | Copy for TLB index |
| `shr rcx, 12` | 1 | Page number |
| `and ecx, 0x3FF` | 1 | TLB index (1024 entries) |
| `cmp [tlb+rcx*16], rax` | 4 | TLB tag match (L1 hit) |
| `jne >slow` | 0 | (predicted not-taken; 99% hit) |
| `mov rdx, [tlb+rcx*16+8]` | 4 | Host pointer |
| `mov r8, [rdx+rax&0xFFF]` | 4 | DIRECT HOST LOAD |
| **Total** | **~16** | **8 host insns / 1 guest insn** |

**Speedup per load:** 40 cycles → 16 cycles = **2.5x** on TLB hit.

### After Phase 2 + Block Chaining: Tight Loop

```aarch64
loop:
  ldr x0, [x1], #8         ; post-index load
  add x2, x2, x0           ; accumulate
  subs x3, x3, #1          ; decrement counter
  b.ne loop                 ; branch back
```

**Current:** 4 blocks × (avg 40 cycles/block + 150 cycles dispatch) = **760
cycles per iteration**.

**After all phases:** Single x86 block with self-loop:
```x86
loop:
  mov rax, [<tlb_host_ptr> + r9 & 0xFFF]  ; inline TLB load
  add r9, 8                                ; post-index
  add r10, rax                             ; accumulate (pinned)
  sub r11, 1                               ; decrement (pinned)
  jnz loop                                 ; backward branch
```
**~5 instructions, ~8 cycles per iteration.** Speedup: 760 / 8 = **~95x** for
this specific pattern. Real workloads average 20–50x.

---

## 7. Risk Analysis

| Risk | Likelihood | Impact | Mitigation |
|------|-----------|--------|------------|
| Register pressure: x86-64 only has 16 GPRs | Certain | Medium | Pin only 8–10 most-used regs; spill rest to array |
| Block chaining invalidation complexity | Medium | High | Conservative unlinking: flush chain on TLB flush |
| Inline TLB correctness (race with device MMIO) | Medium | High | TLB only covers RAM regions; MMIO always goes through helper |
| Trace compilation side-exit complexity | High | Medium | Defer to Phase 4; blocks+chaining sufficient for 40–80 MIPS |
| dynasm-rs API limitations for patching | Low | Low | dynasm-rs supports label patching; backup: raw mmap+mprotect |
| FP/SIMD register pinning (SSE regs) | Medium | Medium | Use xmm0–xmm15 for FP; defer SIMD vectors to Phase 3 |

---

## 8. Verification Plan

| Milestone | Metric | Method |
|-----------|--------|--------|
| Phase 1 complete | 15–25 MIPS | `helm-aarch64 --jit busybox ls` timing |
| Phase 2 complete | 40–80 MIPS | Same + block chain hit rate counter |
| Phase 3 complete | 60–120 MIPS | Same + opcode coverage % |
| Phase 4 complete | 100–150 MIPS | SPECint subset + trace compile rate |
| Correctness | 0 mismatches | JIT-vs-interpreter differential tests (per-opcode sweep, existing `assert_jit_matches_interpreter` pattern) |
| No regression | Interpreter ≥3 MIPS | Ensure JIT-off path unchanged |

---

## 9. Comparison to Reference Systems

| System | Architecture | Technique | MIPS |
|--------|-------------|-----------|------|
| helm-ng interpreter | AArch64→interpret | Decode cache + monomorphized timing | ~3 |
| helm-ng stencil JIT | AArch64→x86-64 | Copy-and-patch, no pinning | ~4 |
| helm-ng tiered JIT | AArch64→x86-64 | Stencil + dynasm hot | ~5 |
| **helm-ng target** | **AArch64→x86-64** | **Pinned regs + inline TLB + chaining** | **60–150** |
| QEMU TCG | AArch64→x86-64 | Pinned regs, chaining, softmmu inline | 100–500 |
| Rosetta 2 | AArch64→x86-64 | AOT + runtime patching, memory mapping | ~80% native |

The target of 60–150 MIPS is conservative relative to QEMU. The key
differences:
- QEMU has 20+ years of optimization; helm-ng JIT is new
- QEMU has no timing model; helm-ng may keep lightweight timing hooks
- QEMU's IR (TCGOp) enables more optimization passes; helm-ng translates
  directly from decoded AArch64 instructions

---

## 10. Interpreter Bottlenecks (From Agent Deep-Dive)

The background agents' line-by-line analysis of `lib.rs` (2998 lines),
`fs.rs` (684 lines), and `jit.rs` (456 lines) revealed additional
bottlenecks in the interpreter path that compound the JIT gap:

### IB-1: InstrumentedMem Used For ALL Loads/Stores — ✅ Fixed (`9f5cf08`)

> **Resolution:** The extra pre-read in `InstrumentedMem::write()` was removed
> in Phase A. `value_before` is now set to `None` for stores. The remaining
> `records_mem_access` gate is correct — the timing model requires
> `on_mem_access()` calls which need the access records (addr, size, is_store).

`records_mem_access` in `DecodedAarch64Insn` (line 73-76 of decode cache)
is `true` for every Load/Store/Atomic instruction. This means **even without
any plugins or probes active**, the interpreter wraps memory in
`InstrumentedMem` for every load/store instruction.

~~Worse: `InstrumentedMem::write()` (line 426 of lib.rs) does an **extra read
before every write** to capture `value_before`:~~
```rust
// REMOVED in Phase A — value_before is now None for stores.
// let old = self.inner.read(addr, size, AccessType::Load).unwrap_or(0);
```
~~This **doubles store memory traffic** in the interpreter.~~ The JIT path
skips instrumentation entirely, which is part of why it gets any speedup
at all.

**Fix:** Only use InstrumentedMem when `has_mem_callbacks || has_mem_probe`
is true. The `records_mem_access` flag should only gate timing cache
simulation, not full instrumentation.

### IB-2: DecodedAarch64Insn Copied By Value On Every Cache Hit — ✅ Fixed (`e89de0c`)

`Aarch64DecodeCache::lookup()` returns `Option<DecodedAarch64Insn>` by
value. The struct contains `Instruction` (42 bytes) + classification fields
(~40 bytes) = **~80+ bytes copied per instruction** on every decode cache
hit.

**Fix:** Return `&DecodedAarch64Insn` reference instead of copying. The
cache entry lifetime is stable (direct-mapped, single-threaded).

### IB-3: Session Accessor Chain Called 3x Per Instruction — Deferred

`self.session.aarch64().and_then(Aarch64Core::state)` appears at least 3
times per instruction in `step_aarch64()`:
- Line 1328: PC read
- Line 1457: Branch target
- Line 1494: Plugin context

Each call is two `Option` pattern matches through `HelmMachine` →
`HelmCoreSet` → `Aarch64Core`.

**Fix:** Cache the `&mut Aarch64ArchState` reference at the top of
`step_aarch64()` and reuse.

### IB-4: EXIT_CHAIN Defined But Never Emitted — Open (RC-3 prerequisite)

`block.rs` defines `EXIT_CHAIN = 1` but neither the dynasm nor stencil
backend ever emits it. The block chaining infrastructure was planned in
the original design but never implemented. This confirms RC-3.

### IB-5: Stencil NZCV Uses pushfq/popfq — Deferred

The stencil C backend captures flags via:
```c
uint64_t flags;
asm volatile("pushfq; pop %0" : "=r"(flags));
```
`pushfq` is extremely expensive on modern x86-64 (10+ cycles, causes
partial flag stalls). The dynasm backend's `setCC` approach (12
instructions but no stalls) is 2–3x faster for flag capture.

**Fix for stencil:** Replace `pushfq/popfq` with inline `setCC`-based
flag extraction matching the dynasm pattern.

### IB-6: Vec Allocation Per JIT Cache Miss — ✅ Fixed (`e89de0c`)

`jit.rs` lines 146 and 201 allocate `Vec::new()` to collect decoded
instructions on every JIT cache miss. For cold workloads with many unique
PCs, this creates allocation pressure.

**Fix:** Pre-allocate a reusable `Vec<Instruction>` in `HelmEngine` and
clear+reuse it.

---

## 11. Quick-Start: Smallest Changes With Largest Impact

Ranked by effort/impact ratio:

### Quick Win 1: Fix InstrumentedMem Overuse (IB-1) — ✅ Done

**Status:** Fixed in `9f5cf08` — extra pre-read removed. The `records_mem_access`
gate remains correct because timing needs `on_mem_access()` calls. No further
change needed.

### Quick Win 2: Return Decode Cache Entry By Reference (IB-2) — ✅ Done

**Status:** Fixed in `e89de0c` — `with_pc()` copy eliminated, lookup simplified
to `(key, raw)`. Full by-reference return blocked by borrow checker (decoded ref
conflicts with later `&mut self` borrows in execute path).

### Quick Win 3: Inline TLB for JIT Memory Access (RC-2) — Open

**Effort:** ~200 lines in `ldst.rs` + `helpers.rs`
**Impact:** 2–3x JIT speedup (5 MIPS → 10–15 MIPS)

Add a TLB pointer to the register array (slot 46 already reserved).
Rewrite `emit_mem_read` to inline the TLB fast path. Keep the helper
call as the TLB miss slow path.

### Quick Win 4: Fix Stencil pushfq/popfq (IB-5) — Deferred

**Effort:** ~50 lines in `stencil_gen/aarch64.c`
**Impact:** 1.2–1.5x stencil speedup for flag-setting instructions
**Deferred:** Requires stencil C codegen rebuild pipeline.
