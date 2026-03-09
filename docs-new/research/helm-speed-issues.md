# HELM Speed Issues — Why We Are Not at QEMU Speed

QEMU sources referenced throughout: `assets/qemu/`.

## Current State

HELM FE+JIT reports 10–100 MIPS (`docs-new/development/performance.md`).
QEMU on comparable workloads runs at 500–2000+ MIPS. The gap is roughly
10–50×. This document catalogues the architectural reasons for that gap,
with concrete source references to both codebases, and proposes a
prioritised remediation plan.

## How QEMU's vCPU Model Achieves Speed

### The CPUState / env Architecture

QEMU organises all guest state into a single `CPUArchState` (e.g.
`CPUARMState`) which is laid out *immediately* after `CPUState` in the
`ArchCPU` allocation. The critical layout is:

```text
┌──────────────────────────────────────────────────┐
│  ArchCPU                                         │
│  ┌──────────────────────────────────────────────┐ │
│  │  CPUState                                    │ │
│  │    ...thread, halt, interrupt_request...     │ │
│  │    tb_jmp_cache: *CPUJumpCache               │ │
│  │    neg_align: padding                        │ │
│  │    neg: CPUNegativeOffsetState  ◄─ TLB here  │ │
│  │      tlb: CPUTLB                             │ │
│  │      icount_decr: IcountDecr                 │ │
│  └──────────────────────────────────────────────┘ │
│  CPUArchState (= "env", TCG_AREG0 points here)   │
│    regs[], pc, flags, sysregs...                  │
└──────────────────────────────────────────────────┘
```

Source: `include/hw/core/cpu.h:484` (`struct CPUState`), lines 362–382
(`CPUNegativeOffsetState`), lines 333–339 (`CPUTLB`).

The `neg` field is placed at the **end** of `CPUState` so that
`CPUArchState` (`env`) immediately follows. The TLB, icount decrementer,
and plugin state live at *negative* offsets from `env` — small negative
displacements that generated code can reach with a single base register.

### Register Pinning: TCG_AREG0

QEMU pins the `env` pointer to a callee-saved host register for the
**entire lifetime** of the vCPU thread. The assignment is per-host-ISA:

| Host | TCG_AREG0 | Register |
|------|-----------|----------|
| x86-64 | `TCG_REG_EBP` | `%rbp` |
| AArch64 | `TCG_REG_X19` | `x19` |

Source: `tcg/x86_64/tcg-target.h:71` (`TCG_AREG0 = TCG_REG_EBP`),
`tcg/aarch64/tcg-target.h:50` (`TCG_AREG0 = TCG_REG_X19`).

The registration happens in `tcg/tcg.c:1813`:

```c
ts = tcg_global_reg_new_internal(s, TCG_TYPE_PTR, TCG_AREG0, "env");
tcg_env = temp_tcgv_ptr(ts);
```

This reserves the host register so the register allocator never uses
it for temporaries. Every generated instruction that touches guest state
emits a `[TCG_AREG0 + offset]` memory operand — one host instruction
per guest register read/write, with the base register already loaded.

### Inline Softmmu TLB

The TLB lives inside `CPUNegativeOffsetState` at a known negative
offset from `env`. The fast-path TLB structure is:

```c
// include/exec/tlb-common.h
typedef union CPUTLBEntry {
    struct {
        uintptr_t addr_read;
        uintptr_t addr_write;
        uintptr_t addr_code;
        uintptr_t addend;      // host_ptr - guest_va_page
    };
} CPUTLBEntry;

typedef struct CPUTLBDescFast {
    uintptr_t mask;            // (n_entries - 1) << CPU_TLB_ENTRY_BITS
    CPUTLBEntry *table;        // the TLB array
} CPUTLBDescFast;
```

Source: `include/exec/tlb-common.h` (full file).

The x86-64 backend emits the TLB lookup inline in generated code via
`prepare_host_addr()` (`tcg/x86_64/tcg-target.c.inc:1921`):

```c
// 1. Compute TLB index from VA
tcg_out_shifti(s, SHIFT_SHR + tlbrexw, TCG_REG_L0,
               TARGET_PAGE_BITS - CPU_TLB_ENTRY_BITS);
// 2. AND with mask from [env + fast_ofs + offsetof(mask)]
tcg_out_modrm_offset(s, OPC_AND_GvEv + trexw, TCG_REG_L0,
                     TCG_AREG0,
                     fast_ofs + offsetof(CPUTLBDescFast, mask));
// 3. ADD table base from [env + fast_ofs + offsetof(table)]
tcg_out_modrm_offset(s, OPC_ADD_GvEv + hrexw, TCG_REG_L0,
                     TCG_AREG0,
                     fast_ofs + offsetof(CPUTLBDescFast, table));
// 4. Compare tag: cmp [L0 + cmp_ofs], masked_va
tcg_out_modrm_offset(s, OPC_CMP_GvEv + trexw,
                     TCG_REG_L1, TCG_REG_L0, cmp_ofs);
// 5. jne slow_path
tcg_out_opc(s, OPC_JCC_long + JCC_JNE, 0, 0, 0);
// 6. TLB hit — load addend
tcg_out_ld(s, TCG_TYPE_PTR, TCG_REG_L0, TCG_REG_L0,
           offsetof(CPUTLBEntry, addend));
// Then: host_addr = guest_va + addend → direct load/store
```

All of this is ~10 host instructions, no function call. The slow path
(`tcg_out_qemu_ld_slow_path`, line 1838) calls a helper only on TLB
miss — which happens <5% of the time in steady state.

The `addend` field is the key insight: when a TLB entry is filled
(`accel/tcg/cputlb.c:1072`), the addend is computed as:

```c
addend = (uintptr_t)memory_region_get_ram_ptr(section->mr) + xlat;
// ...
tn.addend = addend - addr_page;
```

So `guest_va + addend` yields the **host virtual address** directly.
No PA→region lookup at runtime. The `AddressSpaceDispatch` / `FlatView`
(`include/system/memory.h:1204`) is only consulted during TLB refill,
never in the fast path.

### Block Chaining

QEMU's `TranslationBlock` has two exit slots (`jmp_dest[2]`,
`jmp_target_addr[2]`, `jmp_insn_offset[2]`) for the taken and
not-taken paths of a conditional branch.

Source: `include/exec/translation-block.h:130–155`.

**Code generation:** `tcg_out_goto_tb()` emits a direct jump
instruction with a placeholder offset:

```c
// tcg/x86_64/tcg-target.c.inc:2350
static void tcg_out_goto_tb(TCGContext *s, int which)
{
    // Align for atomic patching
    int gap = QEMU_ALIGN_PTR_UP(s->code_ptr + 1, 4) - s->code_ptr;
    if (gap != 1) tcg_out_nopn(s, gap - 1);
    tcg_out8(s, OPC_JMP_long);       // jmp rel32
    set_jmp_insn_offset(s, which);   // record patch point
    tcg_out32(s, 0);                 // placeholder offset
    set_jmp_reset_offset(s, which);  // reset target
}
```

**Runtime linking:** When the dispatcher discovers that TB-A exits
to TB-B, it calls `tb_add_jump()` (`accel/tcg/cpu-exec.c:616`):

```c
// Atomically claim the slot
old = qatomic_cmpxchg(&tb->jmp_dest[n], (uintptr_t)NULL,
                      (uintptr_t)tb_next);
// Patch the native jump to point directly to tb_next
tb_set_jmp_target(tb, n, (uintptr_t)tb_next->tc.ptr);
```

`tb_target_set_jmp_target` on x86-64 (`tcg/x86_64/tcg-target.c.inc:2372`)
simply writes the new relative offset into the placeholder:

```c
void tb_target_set_jmp_target(const TranslationBlock *tb, int n,
                              uintptr_t jmp_rx, uintptr_t jmp_rw)
{
    uintptr_t addr = tb->jmp_target_addr[n];
    qatomic_set((int32_t *)jmp_rw, addr - (jmp_rx + 4));
}
```

After patching, TB-A's exit jump goes **directly** to TB-B's native
code — no return to the dispatcher, no cache lookup, no state sync.

### The Main Dispatch Loop

The dispatcher (`cpu_exec_loop`, `accel/tcg/cpu-exec.c:933`) only
runs when chaining is broken:

```c
while (!cpu_handle_interrupt(cpu, &last_tb)) {
    tb = tb_lookup(cpu, s);        // hash lookup, O(1)
    if (tb == NULL) {
        tb = tb_gen_code(cpu, s);  // translate + compile
        // insert into tb_jmp_cache
    }
    if (last_tb) {
        tb_add_jump(last_tb, tb_exit, tb);  // link blocks
    }
    cpu_loop_exec_tb(cpu, tb, ...);  // enter generated code
}
```

`tb_lookup` (`cpu-exec.c:227`) checks a direct-mapped `CPUJumpCache`
(4096 entries, `accel/tcg/tb-jmp-cache.h:27`) first, then falls
back to a hash table. In a tight loop, the dispatcher never runs
because chained blocks jump directly to each other.

### Summary of QEMU's Speed Model

These are not independent optimisations — they form a coherent
execution model where the **common case (execute guest code, access
memory, branch to next block) stays entirely in generated native
code** with zero calls back to the runtime.

## HELM's Current Model and Where It Diverges

### Issue 1: Three-Way State Split and Constant Marshalling

HELM maintains guest state in three separate representations:

| Location | Owner | Used by |
|----------|-------|---------|
| `Aarch64Regs` (struct fields) | `Aarch64Cpu` in `helm-isa` | `step_fast()`, MMU walker, IRQ check, timer check |
| `[u64; NUM_REGS]` flat array | Stack variable in `run_inner` | JIT generated code (`ReadReg`/`WriteReg` ops) |
| `Vec<u64>` sysreg map (32K entries, 256 KB) | `TcgInterp::sysregs` in `helm-tcg` | JIT sysreg helpers, `sync_sysregs_*` |

Source: `crates/helm-tcg/src/interp.rs:76` (sysreg file),
`crates/helm-engine/src/fs/session.rs` (regs array and sync
functions at the bottom of the file).

The JIT function signature reflects this split — four separate
pointers are passed in:

```rust
// crates/helm-tcg/src/jit.rs:258
// fn(regs: ptr, cpu_ctx: ptr, mem_ctx: ptr, sysreg_ctx: ptr) -> i64
sig.params.push(AbiParam::new(ptr_type)); // regs
sig.params.push(AbiParam::new(ptr_type)); // cpu_ctx
sig.params.push(AbiParam::new(ptr_type)); // mem_ctx
sig.params.push(AbiParam::new(ptr_type)); // sysreg_ctx
```

Compare QEMU: `env` is the **only** pointer. TLB, registers, sysregs
are all at known offsets from `env`.

Every block boundary in HELM requires synchronisation between these
representations. The session file contains **66 calls** to
`set_sysreg`/`get_sysreg` across the sync functions:

| Function | Direction | Approx. copies |
|----------|-----------|---------------|
| `sync_sysregs_to_interp` | CPU → interp sysregs | 25+ fields |
| `sync_sysregs_from_interp` | interp sysregs → CPU | 18+ fields |
| `sync_mmu_to_cpu` | interp sysregs → CPU (MMU subset) | 7+ fields + conditional TLB flush |
| `regs_to_array` | CPU struct → flat array | 12+ fields |
| `array_to_regs` | flat array → CPU struct | 12+ fields |

**QEMU does none of this.** Its `env` is the single truth.

### Issue 2: Memory Access — Function Call Per Operation

Every guest Load/Store in HELM's JIT-generated code emits a
`call` to a helper function:

```rust
// crates/helm-tcg/src/jit.rs:456 (TcgOp::Load)
let inst = builder.ins().call(helpers.fn_mr,
    &[cpu_ctx, mem_ctx, addr_v, size_v]);
```

The helper `helm_mem_read` (`jit.rs:87`) does:

1. Call through `TRANSLATE_VA` global fn pointer → `jit_translate_va`
2. Which calls `cpu.translate_va_jit(va, ...)` →
3. `tlb.lookup(va, asid)` — **linear scan** of 256 entries
4. On miss: full `mmu::walk()` doing 4-level page walk via `read_phys()`
5. Back in the helper: `mem.read(pa, buf)` — **linear region scan**

```text
Total call chain per memory op (TLB hit):
  JIT code
    → call helm_mem_read          (call #1)
      → call TRANSLATE_VA (fn ptr) (call #2, indirect)
        → Tlb::lookup              (loop, up to 256 iterations)
      → AddressSpace::read         (loop over regions)
        → copy_from_slice
    ← return value
```

Compare QEMU: ~10 inline host instructions, zero function calls on
TLB hit, direct host memory access via `addend`.

| Aspect | QEMU | HELM |
|--------|------|------|
| Fast-path host instructions | ~10 | ~200+ (call + indirect + loops) |
| Function calls on TLB hit | 0 | 2 (helper + translate callback) |
| TLB lookup | O(1) direct-mapped | O(n) linear scan, n=256 |
| PA→host-RAM lookup | O(1) via `addend` field | O(m) linear region scan |
| Branch prediction | Highly predictable | Indirect call + loop branches |

### Issue 3: TLB — Linear Scan vs Direct-Mapped Hash

HELM `Tlb::lookup()` (`crates/helm-memory/src/tlb.rs:69`):

```rust
for e in &self.entries {       // up to 256 iterations
    if !e.valid { continue; }
    if !e.global && e.asid != asid { continue; }
    let offset = va.wrapping_sub(e.va_page);
    if offset < e.size { ... }
}
```

QEMU `tlb_index()` (`accel/tcg/cputlb.c:127`):

```c
static inline uintptr_t tlb_index(CPUState *cpu, uintptr_t mmu_idx,
                                  vaddr addr)
{
    uintptr_t size_mask =
        cpu_tlb_fast(cpu, mmu_idx)->mask >> CPU_TLB_ENTRY_BITS;
    return (addr >> TARGET_PAGE_BITS) & size_mask;
}
```

One shift, one AND, one array index. The variable-size page support in
HELM's TLB (storing `va_page` and `size` per entry) prevents this kind
of direct indexing — it requires a subtraction and comparison per entry.

### Issue 4: AddressSpace — Linear Region Search

`AddressSpace::read()` (`crates/helm-memory/src/address_space.rs:72`):

```rust
for region in &self.regions {
    if addr >= region.base
       && addr + buf.len() as u64 <= region.base + region.size
    {
        let offset = (addr - region.base) as usize;
        buf.copy_from_slice(&region.data[offset..offset + buf.len()]);
        return Ok(());
    }
}
```

This runs on **every** memory access, including the MMU page-table
walker's `read_phys()` which does 4 descriptor reads per walk. With
5–10 regions (RAM, I/O windows, DTB, initrd), each access does 1–10
comparisons.

QEMU's equivalent: the TLB `addend` already encodes the host pointer.
RAM access is a single `mov` instruction. I/O dispatch only happens
when the TLB entry has `TLB_MMIO` set — the `FlatView` dispatch table
(`include/system/memory.h:1204`) is pre-computed at machine-init time.

### Issue 5: No Block Chaining

When a HELM JIT block ends with `GotoTb { target_pc }`, it returns
`EXIT_CHAIN` to `run_inner` (`crates/helm-engine/src/fs/session.rs`),
which then:

1. Updates `regs[PC] = target_pc`
2. Loops back to top of `while self.insn_count < limit`
3. Checks WFI, timer/IRQ (every 1024 insns), PC breakpoint
4. Computes JIT cache index `(pc >> 2) & 0xFFFF`
5. Checks cache hit, calls `sync_mmu_to_cpu`
6. Updates `CNTVCT_EL0` in interp sysregs
7. Calls `exec_jit()` — which transmutes a fn pointer and calls it
8. After return: syncs ELR/SPSR/ESR/VBAR back from interp sysregs

Even on a tight two-block loop (`subs x0, x0, #1; b.ne loop`),
**every single iteration** pays this full dispatcher cost.

QEMU: `tb_add_jump()` patches the `jmp rel32` inside TB-A to point
directly at TB-B. The loop executes as a native code loop with zero
dispatcher involvement.

### Issue 6: Cranelift Compilation Cost

QEMU's TCG backend is a purpose-built code emitter:
- x86-64 backend: 4,599 lines (`tcg/x86_64/tcg-target.c.inc`)
- Core TCG: 7,033 lines (`tcg/tcg.c`)
- Simple linear-scan register allocator
- No SSA, no optimisation passes
- Compilation: ~5–15 µs per block

Cranelift is a full optimising compiler:
- SSA construction
- Multiple optimisation passes
- Graph-colouring register allocation
- Compilation: ~50–500 µs per block (10–100× slower)

During kernel boot (high TB turnover), this difference is significant.
Cranelift produces *better* code per block, but the quality advantage
is dwarfed by the architectural overheads above. A perfectly optimised
block body that returns to a slow dispatcher for every branch still
loses to a mediocre block body that chains directly to the next block.

### Issue 7: Interpreter Fallback Tax

Several instruction classes trigger fallback from JIT to `step_interp()`:

- Complex SIMD operations
- Exclusive load/store pairs (LDXR/STXR)
- Some system register accesses
- Barriers (some variants)

Each fallback requires the full sync round-trip:
`array_to_regs` → `sync_sysregs_from_interp` → `step_fast` →
`sync_sysregs_to_interp` → `regs_to_array` — that is ~75 field
copies per fallback instruction. During kernel boot, fallback rates
can be high (atomics, barriers, DC ZVA are frequent).

## Connection: vCPU Model → Register Pinning → Everything Else

The question "is the vCPU model related to register pinning?" — yes,
deeply. They are the same design decision viewed from different angles:

**QEMU's vCPU model means:**
- One thread per guest CPU (`accel/tcg/tcg-accel-ops-rr.c`)
- That thread owns a `CPUState` / `ArchCPU` for the entire session
- `env` (= `CPUArchState *`) is pinned to `TCG_AREG0` (`%rbp` on
  x86-64, `x19` on AArch64) for the thread's lifetime
- Generated code, helpers, softmmu, and the dispatcher all reach
  state via `[env + offset]`
- The TLB lives at `env->neg.tlb` (negative offset), so inline probes
  are just `[env + constant]` — see `prepare_host_addr`
- Block chaining is safe because all blocks assume the same `env`
  layout — `tb_add_jump` patches a `jmp rel32` without touching state

**HELM's model means:**
- `Aarch64Cpu` is a Rust struct with owned fields — not `#[repr(C)]`,
  offsets not stable
- The JIT receives a separate `regs: *mut u64` array, passed as a
  function argument — not pinned, not persistent across blocks
- The TLB is behind `Aarch64Cpu::translate_va_jit()` — a method call,
  not a known memory offset
- Sysregs live in `TcgInterp::sysregs` (third location, 256 KB)
- Block chaining is impossible without solving the state-location
  problem first: you cannot patch a jump between blocks if the state
  locations might be inconsistent between them

Register pinning is not just a micro-optimisation — it is the
**foundation** that enables inline TLB probes, zero-sync block
transitions, and block chaining. Without it, each of those
optimisations has to work around the mismatch, adding the sync
overhead that dominates HELM's runtime.

## Remediation Plan

### Phase 1: Unified CPU State (Foundation)

Redesign `Aarch64Cpu` (or introduce a new `VCpuState`) as a
`#[repr(C)]` struct with a fixed, known layout:

```text
┌─────────────────────────────────────────────┐
│  VCpuState (#[repr(C)])                     │
│                                             │
│  +0x000  gpr[31]: [u64; 31]   X0–X30       │
│  +0x0F8  sp: u64                            │
│  +0x100  pc: u64                            │
│  +0x108  nzcv: u64                          │
│  +0x110  daif: u64                          │
│  +0x118  current_el: u64                    │
│  +0x120  sp_sel: u64                        │
│  +0x128  elr_el1: u64                       │
│  +0x130  spsr_el1: u64                      │
│  +0x138  ...                                │
│  +0x200  sysregs[N]: [u64; N]               │
│  +0xWWW  tlb: InlineTlb                     │
│    each entry: tag(u64), addend(isize)      │
│    (mirrors QEMU CPUTLBEntry layout)        │
│  +0xXXX  mem_base: *mut u8  (RAM backing)   │
└─────────────────────────────────────────────┘
```

One pointer to this struct replaces `regs`, `cpu_ctx`, `mem_ctx`, and
`sysreg_ctx`. The JIT function signature becomes:

```text
fn(vcpu: *mut VCpuState) -> i64
```

All `regs_to_array`, `array_to_regs`, `sync_sysregs_*` functions
are **deleted**. The JIT emits direct loads/stores at compile-time-known
offsets from the `vcpu` pointer.

This mirrors QEMU's `ArchCPU` layout where `CPUArchState` (`env`) is
at a fixed offset from `CPUState`, and `CPUTLB` is at a fixed negative
offset from `env` (`include/hw/core/cpu.h:369–382`).

Cranelift does not support pinning a value to a specific host register
across the entire function, but passing `vcpu` as the first argument
guarantees it arrives in the first argument register (e.g. `rdi` on
x86-64). Since all memory accesses already go through helper calls,
the `vcpu` pointer will naturally be kept alive by Cranelift's register
allocator across the function body. The key gain is eliminating the
marshalling, not achieving `%rbp`-level pinning.

### Phase 2: Inline TLB Fast Path

Embed a direct-mapped TLB inside `VCpuState` at a known offset,
mirroring QEMU's `CPUTLBEntry` / `CPUTLBDescFast` from
`include/exec/tlb-common.h`:

```text
struct InlineTlbEntry {
    addr_read: u64,     // VA page | flags (matches QEMU addr_read)
    addr_write: u64,    // VA page | flags (matches QEMU addr_write)
    addend: isize,      // host_ptr - guest_va_page
}
```

The JIT emits the fast path inline for every Load/Store, following
the same pattern as QEMU's `prepare_host_addr`:

```text
; va in reg (Cranelift SSA value)
idx = va >> 12
idx = idx & TLB_MASK
idx = idx << TLB_ENTRY_BITS
entry_base = vcpu + TLB_OFFSET + idx
tag = load [entry_base + offsetof(addr_read)]
cmp tag, (va & PAGE_MASK)
jne slow_path
addend = load [entry_base + offsetof(addend)]
host_addr = va + addend
result = load [host_addr]               ; direct host memory access
; done — no function call on TLB hit

slow_path:
    call helper_tlb_fill       ; fills TLB entry + retries load
```

This eliminates the function-call overhead for ~95%+ of memory accesses.
The `addend` field means the `AddressSpace::read` linear search is also
bypassed for RAM accesses.

### Phase 3: Flat Page Table for AddressSpace

Replace the linear region list with a page-granularity dispatch
table built at machine-init time:

```text
page_table: Vec<Option<*mut u8>>   // indexed by PA >> 12
```

RAM pages point to the host-memory backing; I/O pages store a
sentinel that triggers the device-bus dispatch. The TLB `addend`
field can point directly into this table's backing memory, making
the fast path a single indexed load with no search at all.

### Phase 4: Block Chaining

With a unified state pointer, block chaining follows the same pattern
as QEMU (`tcg_out_goto_tb` + `tb_add_jump` + `tb_target_set_jmp_target`):

1. Each compiled block has two exit slots (`jmp_target[2]`), matching
   QEMU's `TranslationBlock::jmp_target_addr[2]`.
2. `tcg_out_goto_tb` emits a `jmp rel32` (x86-64) or `B` (AArch64)
   with a zero placeholder — QEMU does exactly this.
3. On first execution, the placeholder jumps to a "link stub" that
   returns to the dispatcher with the exit index.
4. The dispatcher calls `tb_add_jump(last_tb, exit_idx, new_tb)` which
   atomically patches the `jmp` instruction to point to `new_tb`'s
   entry — matching QEMU's `qatomic_set((int32_t *)jmp_rw, ...)`.
5. Subsequent executions jump block-to-block without returning.
6. TLB flush or self-modifying code detection unlinks blocks by
   resetting exits to the stub — matching QEMU's `jmp_reset_offset`.

This requires Cranelift-compiled blocks to be emitted into a
**writable code buffer** (not Cranelift's default immutable code
region) so that exit instructions can be patched at runtime.

### Phase 5: Expand TCG Emitter Coverage

Reduce interpreter fallback by implementing:

- Exclusive load/store (LDXR/STXR) — emit inline with a per-CPU
  exclusive monitor
- Common SIMD instructions (DUP, MOV, ADD vector)
- All barrier variants (DSB, DMB, ISB as no-ops in single-core)
- Remaining system register accesses

Each eliminated fallback removes a full sync round-trip.

### Phase 6: Compilation Speed

Consider a lightweight custom emitter (like QEMU's
`tcg/x86_64/tcg-target.c.inc`, 4599 lines) for the most common ops
(integer ALU, load/store, branch) and use Cranelift only for complex
blocks (FP, SIMD, system). QEMU's approach of a simple linear-scan
register allocator + direct machine-code emission is 10–100× faster
per block than Cranelift's full compiler pipeline.

## Expected Impact

| Phase | Change | Est. Speedup | Cumulative |
|-------|--------|-------------|------------|
| 1 | Unified VCpuState | 1.5–2× | 1.5–2× |
| 2 | Inline TLB fast path | 3–5× | 5–8× |
| 3 | Flat page table | 1.5–2× | 8–12× |
| 4 | Block chaining | 2–4× | 15–30× |
| 5 | Emitter coverage | 1.2–1.5× | 18–40× |
| 6 | Fast emitter | 1.1–1.3× (compilation) | — |

Phases 1–4 together should bring HELM into the same order of
magnitude as QEMU (hundreds of MIPS). Phase 1 is the prerequisite
for everything else — the unified state struct is the foundation on
which inline TLB, block chaining, and zero-sync dispatch are built.

## QEMU Source Reference Index

All paths relative to `assets/qemu/`.

| Topic | File | Key lines/symbols |
|-------|------|-------------------|
| CPUState struct | `include/hw/core/cpu.h` | line 484 `struct CPUState` |
| CPUNegativeOffsetState (TLB placement) | `include/hw/core/cpu.h` | line 369 `CPUNegativeOffsetState` |
| CPUTLB structure | `include/hw/core/cpu.h` | line 333 `CPUTLB` |
| TLB entry layout (fast path) | `include/exec/tlb-common.h` | `CPUTLBEntry`, `CPUTLBDescFast` |
| `TCG_AREG0` on x86-64 | `tcg/x86_64/tcg-target.h` | line 71 `TCG_AREG0 = TCG_REG_EBP` |
| `TCG_AREG0` on AArch64 | `tcg/aarch64/tcg-target.h` | line 50 `TCG_AREG0 = TCG_REG_X19` |
| env pinned as global reg | `tcg/tcg.c` | line 1813 `tcg_global_reg_new_internal` |
| Inline TLB code gen (x86-64) | `tcg/x86_64/tcg-target.c.inc` | line 1921 `prepare_host_addr` |
| TLB slow path | `tcg/x86_64/tcg-target.c.inc` | line 1838 `tcg_out_qemu_ld_slow_path` |
| TLB index/entry helpers | `accel/tcg/cputlb.c` | line 127 `tlb_index`, line 136 `tlb_entry` |
| TLB fill + addend computation | `accel/tcg/cputlb.c` | line 1072 `addend = memory_region_get_ram_ptr(...)` |
| Block chaining: goto_tb (x86-64) | `tcg/x86_64/tcg-target.c.inc` | line 2350 `tcg_out_goto_tb` |
| Block chaining: goto_tb (AArch64) | `tcg/aarch64/tcg-target.c.inc` | line 2000 `tcg_out_goto_tb` |
| Block chaining: patch target (x86-64) | `tcg/x86_64/tcg-target.c.inc` | line 2372 `tb_target_set_jmp_target` |
| Block linking at runtime | `accel/tcg/cpu-exec.c` | line 616 `tb_add_jump` |
| Main dispatch loop | `accel/tcg/cpu-exec.c` | line 933 `cpu_exec_loop` |
| TB lookup (fast cache) | `accel/tcg/cpu-exec.c` | line 227 `tb_lookup` |
| TB lookup helper from JIT | `accel/tcg/cpu-exec.c` | line 374 `HELPER(lookup_tb_ptr)` |
| Jump cache structure | `accel/tcg/tb-jmp-cache.h` | `CPUJumpCache`, 4096 entries |
| TranslationBlock (exit slots) | `include/exec/translation-block.h` | line 46, `jmp_dest[2]`, `jmp_target_addr[2]` |
| FlatView / memory dispatch | `include/system/memory.h` | line 1204 `struct FlatView` |

## Summary

QEMU's speed comes from a **system-level design decision**: the vCPU
model places all guest state behind a single pinned pointer (`env` =
`TCG_AREG0`), which enables inline TLB probes (via `CPUTLBEntry` at a
negative offset from `env`), zero-cost block chaining (via `goto_tb` +
`tb_add_jump`), and no marshalling between representations. HELM's
current architecture splits state across three locations and
synchronises them at every block boundary. Closing the gap requires
not just adding individual optimisations, but restructuring the state
model so those optimisations become possible. The vCPU model *is*
register pinning, and register pinning *is* the prerequisite for
everything else.
