# HELM Speed — Current State and Next Improvements

## Where We Are

HELM went from **12 MIPS → 70–88 MIPS** (6–7× improvement). Several
items from the original remediation plan have been implemented. This
document audits what was done, what remains, and identifies the
specific bottlenecks still separating us from QEMU-class throughput
(500+ MIPS).

## What Was Implemented

### ✅ Flat Page Table in AddressSpace (Phase 3)

`crates/helm-memory/src/address_space.rs` now has a flat page table
(`page_table: Vec<*mut u8>`) indexed by `(PA - base) >> 12`. The
`read()`, `write()`, and `read_phys()` methods all check this O(1)
fast path before falling through to the linear region scan. The table
is rebuilt on `map()`. Covers up to 4GB (1M pages, 8MB table).

**Impact:** Eliminated the per-access linear region scan for RAM.
Page-table walker descriptor reads (`read_phys`) are now O(1).

### ✅ Two-Level TLB with Direct-Mapped Fast Path (Phase 2, partial)

`crates/helm-memory/src/tlb.rs` now has a two-level structure:

- **Fast TLB:** 1024-entry direct-mapped hash, O(1) lookup via
  `(va >> 12) & FAST_TLB_MASK`. Stores pre-computed `addend`
  (host_ptr − va_page) so VA → host address is a single `iadd`.
  Entries are `#[repr(C)]` `FastTlbEntry` with `va_tag`, `pa_page`,
  `addend`, ASID, permission bits, and `has_addend`.
- **Slow TLB:** 256-entry fully-associative (unchanged), handles
  2M/1G block mappings.

Fast TLB entries are populated on every MMU walk miss
(`crates/helm-isa/src/arm/aarch64/exec.rs:403`), including the
`addend` from `AddressSpace::host_ptr_for_pa()`.

### ✅ Fast TLB Used in JIT Memory Helpers

`helm_mem_read` and `helm_mem_write` (`crates/helm-tcg/src/jit.rs:111`)
now have an inline fast path that checks the fast TLB + addend before
falling back to the slow translate + AddressSpace path:

```rust
let entry = cpu.tlb.fast_entries[idx];
if entry.va_tag == va_tag && entry.perm_read && entry.has_addend
    && (entry.global || entry.asid == cpu.current_asid())
{
    let host = (addr as isize).wrapping_add(entry.addend) as *const u8;
    return read_host(host, size);
}
```

**Impact:** TLB-hit memory accesses skip the full translate path
and the AddressSpace region scan entirely.

### ✅ Reduced Per-Block Sync Overhead

- Timer check is now countdown-based (`timer_countdown`, every 1024
  blocks) instead of modulo on `insn_count`.
- IRQ check fast-skips when `DAIF.I` is set (IRQs masked), avoiding
  the expensive `array_to_regs`/`sync_sysregs` round-trip during
  interrupt handlers.
- `sync_mmu_to_cpu` only flushes TLB when SCTLR/TCR/TTBR actually
  changed.
- Post-JIT sync reads only 4 sysregs (ELR/SPSR/ESR/VBAR) instead
  of the full 25+.
- `Chain` exit (the most common) just writes `regs[PC]` and
  continues — no `array_to_regs` unless IRQ/exception.

### ✅ Large Emitter Coverage

253 instruction handlers in `a64_emitter.rs` (3567 lines). Only two
`Unhandled` exit points remain: unrecognised top-level `op0` groups
and decode errors. Most integer, branch, load/store, and system
instructions are covered.

## What Was NOT Implemented

### ❌ Unified VCpuState (Phase 1)

The three-way state split remains:

1. `Aarch64Regs` struct fields (CPU struct)
2. `[u64; NUM_REGS]` flat array (stack variable in `run_inner`)
3. `TcgInterp::sysregs` (32K-entry Vec, 256 KB)

The JIT function still takes 4 pointers:
`fn(regs, cpu_ctx, mem_ctx, sysreg_ctx) -> i64`.

`sync_mmu_to_cpu` still runs on **every JIT cache hit** (line 507),
doing 7 sysreg reads + comparison + conditional TLB flush.

### ❌ Inline TLB in Generated Code (Phase 2, incomplete)

The fast TLB lookup is in the **C helper function** (`helm_mem_read`),
not inlined into Cranelift-generated code. Every guest Load/Store
still emits a `call` instruction to `helm_mem_read`/`helm_mem_write`.

The helper fast path is good (~15 instructions: null check, tag
extract, index, unchecked load, 4 comparisons, addend add, unaligned
read) — but the `call` instruction itself costs:
- Register spills around the call (Cranelift must save live values)
- Branch predictor pollution (call + return)
- ~5–8 cycle overhead per call on modern x86

For a block of 10 instructions with 3 loads, that's 3 extra function
calls that QEMU would handle with inline code.

### ❌ Block Chaining (Phase 4)

No block chaining at all. Every block exit (`Chain`, `EndOfBlock`)
returns to the Rust dispatcher in `run_inner`, which does:

1. `sync_mmu_to_cpu` (7 sysreg reads + conditional TLB flush)
2. `set_sysreg(CNTVCT_EL0, ...)` (timer counter update)
3. JIT cache index computation + hit check
4. `exec_jit()` call (fn pointer transmute + call)
5. Post-JIT 4-sysreg read-back

On a tight loop this overhead runs **every ~10 guest instructions**
(average block size). This is likely the single largest remaining
bottleneck.

## Remaining Bottlenecks — Prioritised

### 1. Per-Block Dispatcher Overhead (No Block Chaining)

**Estimated cost:** 30–50% of total runtime

Every block boundary pays: `sync_mmu_to_cpu` + timer update + cache
lookup + `exec_jit` call + post-JIT sysreg sync. For a block of 10
guest instructions, that's ~50 host instructions of overhead per ~50
host instructions of useful work. Halving the per-block overhead
would give ~1.3–1.5× speedup; block chaining would eliminate it
almost entirely for hot loops (~2–3× speedup).

**Near-term fix (no block chaining):** Skip `sync_mmu_to_cpu` on
`Chain` exits. The MMU registers (SCTLR/TCR/TTBR) almost never
change between blocks — only on kernel context switch or MMU setup.
Track a "dirty" flag: set it when `WriteSysReg` writes any MMU
register (SCTLR_EL1/TCR_EL1/TTBR0_EL1/TTBR1_EL1), clear it after
`sync_mmu_to_cpu`. Only call `sync_mmu_to_cpu` when the flag is set.
This would eliminate 7 sysreg reads from the hot path.

**Near-term fix:** Move `CNTVCT_EL0` update out of the per-block
path. Only update it when actually read (lazy evaluation) or at
timer-check intervals.

**Near-term fix:** Hoist the post-JIT ELR/SPSR/ESR/VBAR read-back
outside the `Chain` case. These only matter for ERET/exception paths.
On `Chain` exit (the common case), skip them entirely.

**Medium-term:** Implement block chaining. Given Cranelift's
compilation model, the simplest approach: after `exec_jit` returns
`Chain { target_pc }`, check if the target is already in the JIT
cache and call it directly without going through the full dispatcher.
A tight inner loop: `exec_jit → Chain → exec_jit → Chain ...` with
no sync in between.

### 2. Memory Access Helper Call Overhead

**Estimated cost:** 15–25% of total runtime

Every guest Load/Store is a `call` to an extern "C" function. Even
with the fast TLB hit path in the helper, the call/return overhead
is ~8–12 cycles per memory op. A typical block has 2–4 memory ops.

**Near-term fix:** Not easily fixable without changing the Cranelift
IR emission. Cranelift does not support emitting inline assembly or
custom instruction sequences.

**Medium-term:** Emit the fast TLB check as Cranelift IR directly.
The fast path is simple enough to express in Cranelift's IR:

```text
va_tag = ushr(addr, 12)
idx = band(va_tag, FAST_TLB_MASK)
entry_ptr = iadd(cpu_ctx, TLB_OFFSET + idx * sizeof(FastTlbEntry))
tag = load [entry_ptr + offsetof(va_tag)]
cmp tag, va_tag
brif ne, slow_path
addend = load [entry_ptr + offsetof(addend)]
host_addr = iadd(addr_as_isize, addend)
result = load [host_addr]     ; direct host memory access
jump done
slow_path:
  result = call helm_mem_read_slow(cpu_ctx, mem_ctx, addr, size)
done:
```

This is ~10 Cranelift IR ops. The slow path is the existing helper
without the fast-path prefix. Permission and ASID checks can be
simplified: embed them in the tag (QEMU packs flags into `addr_read`
/ `addr_write` so a single compare covers both address and access
type).

**Impact:** ~2× speedup on memory-heavy code (eliminates call
overhead for 95%+ of memory accesses).

### 3. `sync_mmu_to_cpu` on Every Block

**Estimated cost:** 5–10% of total runtime

Called on every JIT cache hit (`fs/session.rs:507`). Reads 7 sysregs
from the interp array, compares 4 of them, writes 7 to cpu.regs,
and conditionally flushes the entire TLB. In steady state the values
rarely change, but the reads and comparisons still run.

**Fix:** Dirty-flag approach described above. The JIT `WriteSysReg`
op for MMU-related registers should set a flag in a known location
(e.g., a byte in the sysreg array or a field in the regs array).
`sync_mmu_to_cpu` only needs to run when the flag is set. Cost: one
byte-load check per block instead of 7 qword reads + 4 comparisons.

### 4. Post-JIT Sysreg Read-Back

**Estimated cost:** 3–5% of total runtime

After every `exec_jit`, 4 sysregs (ELR/SPSR/ESR/VBAR) are read from
the interp array into the flat regs array. This is only needed for
ERET and exception handling — not for `Chain` or `EndOfBlock` exits.

**Fix:** Move inside the `match result.exit` arms. Only read them for
`ExceptionReturn` and `Exception` cases.

### 5. JIT Cache Hash Collisions

The JIT cache is 64K entries indexed by `(pc >> 2) & 0xFFFF`. Kernel
code with multiple hot PCs that alias to the same index will thrash.

**Fix:** Increase to 128K or 256K entries, or use a 2-way
set-associative cache (check two entries per lookup). Memory cost is
small (each entry is a PC + pointer, ~16 bytes × 256K = 4MB).

### 6. Cranelift Compilation Cost

Cranelift compilation is 10–100× slower per block than QEMU's TCG
backend. This matters during kernel boot and context switches.

**Near-term fix:** Cache compiled blocks more aggressively. The
current `HashMap<u64, TcgBlock>` for IR + 64K direct-mapped JIT
cache means IR blocks are translated once but may be compiled
multiple times if evicted from the JIT cache. Keep a secondary LRU
of compiled `JitBlock`s keyed by guest PC to avoid recompilation.

**Medium-term:** Consider a tiered approach: interpret the first
N executions of a block (threaded interpreter is already implemented),
then JIT-compile hot blocks. This amortises Cranelift's cost over
many executions.

### 7. Block Size

Maximum block size is 64 guest instructions. Average block size in
typical ARM code is 5–15 instructions (branches are frequent).
Larger blocks amortise dispatcher overhead better.

**Fix:** Increase max to 256 or 512 (QEMU uses 512). More
importantly, implement **trace formation**: when a `Chain` exit
always goes to the same target, merge the blocks into a single
larger compiled block (superblock / trace). This is orthogonal to
block chaining and can be done at the Cranelift IR level.

## Quantified Improvement Estimates

Current: **70–88 MIPS**. Target: **300+ MIPS** (within 2× of QEMU).

| # | Change | Est. Speedup | New MIPS |
|---|--------|-------------|----------|
| 1 | Skip sync_mmu_to_cpu + post-JIT readback on Chain | 1.3–1.5× | 90–130 |
| 2 | Inline TLB fast path in Cranelift IR | 1.5–2× | 135–260 |
| 3 | Simple block chaining (direct re-call) | 1.5–2× | 200–400 |
| 4 | Increase JIT cache + tiered compilation | 1.1–1.2× | 220–480 |
| 5 | Trace formation / larger blocks | 1.1–1.3× | 240–500 |

Items 1–3 are the high-impact changes. Item 1 is the easiest (a few
hours of work), item 2 is moderate (rewrite Load/Store emission in
jit.rs), item 3 is the most architectural but has the highest ceiling.

## Recommended Implementation Order

### ✅ Implemented

1. **MMU dirty flag + conditional `sync_mmu_to_cpu`** —
   `crates/helm-tcg/src/interp.rs`: added `MMU_DIRTY_IDX = SYSREG_FILE_SIZE - 1`.
   `crates/helm-tcg/src/jit.rs`: `helm_sysreg_write` sets `sysregs[MMU_DIRTY_IDX] = 1`
   when writing SCTLR_EL1/TTBR0_EL1/TTBR1_EL1/TCR_EL1. The chain loop reads
   the flag before each block and only calls `sync_mmu_to_cpu` when set, then
   clears it.

2. **Inner block chain loop** — `crates/helm-engine/src/fs/session.rs`: replaced
   the single-block JIT dispatch with a `'chain: loop` that stays in the JIT
   for consecutive `Chain` exits without returning to the outer dispatcher.
   Breaks on: timer expiry, pending unmasked IRQ, JIT cache miss, or any
   non-trivial exit (ERET, exception, WFI, Syscall).

3. **ELR/SPSR/ESR/VBAR readback moved out of hot path** — New helper
   `sync_exception_regs_from_sysregs` is called only inside the
   `ExceptionReturn` and `Exception` match arms, not after every block.
   `sync_mmu_to_cpu` refactored to take `&[u64]` instead of `&TcgInterp`.

### Remaining

4. **Inline TLB in Cranelift IR** — Moderate effort, high impact.
   Replace the `call helpers.fn_mr` in `emit_ops` for `TcgOp::Load`
   with ~10 Cranelift IR instructions that check the fast TLB inline,
   falling back to `call helm_mem_read_slow` on miss.

5. **Larger JIT cache + tiered compilation** — Increase cache to
   256K entries, add a hot-count threshold before Cranelift
   compilation.
