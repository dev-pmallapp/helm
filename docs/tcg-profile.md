# TCG Performance Profile

## Benchmark

Workload: `vmlinuz-rpi` kernel boot on `virt` platform, 2M instructions.

| Backend | Best of 5 | MIPS |
|---------|----------|------|
| `interp` (step_fast) | 397 ms | 5.0 |
| `tcg` (threaded) | 417 ms | 4.8 |

TCG is **slower** than the interpretive backend despite caching decoded
blocks, because per-block overhead exceeds decode savings.

## TCG execution path (per block)

```
run_inner
 ├─ compiled_cache.contains_key(&pc)         HashMap lookup   ~50 ns
 ├─ regs_to_array(&cpu)                      42 × u64 copy    ~42 ns
 ├─ sync_sysregs_to_interp (28 inserts)      HashMap × 28    ~1400 ns  ◄ #1
 ├─ exec_threaded
 │   ├─ fold max_temp + vec![0; N]            alloc + scan     ~80 ns
 │   ├─ Vec::new() for mem_accesses           alloc            ~20 ns
 │   └─ dispatch loop (match on u8 guards)    ~30 ns/TCG-op
 ├─ array_to_regs(&mut cpu)                  42 × u64 copy    ~42 ns
 └─ sync_sysregs_from_interp (18 gets)       HashMap × 18     ~900 ns  ◄ #2
                                                         ─────────────
                                             Block overhead  ~2534 ns
```

A typical guest instruction emits 4-8 TCG ops (`ReadReg → Op → WriteReg`
plus flag handling).  At an average block size of ~10 guest instructions
the per-instruction overhead is **~250 ns** — roughly equal to the entire
cost of `step_fast`, leaving zero room for the actual execution.

## Bottleneck ranking

### 1. HashMap sysreg sync — ~2300 ns / block

`sync_sysregs_to_interp` inserts 28 entries into `HashMap<u32, u64>`
before every block; `sync_sysregs_from_interp` reads 18 back.
Each HashMap op costs ~50 ns (hash + probe + possible resize check).

**Fix:** Replace `HashMap<u32, u64>` with a flat `[u64; 256]` array
indexed by a compact sysreg ID (the existing 16-bit encoding fits in
one byte if mapped to a dense index).  Cost drops from ~2300 ns to
~50 ns (46 array writes at ~1 ns each).

### 2. Dispatch is not a jump table — ~30% slower

The threaded `dispatch()` function matches on `bop.op` using:
```rust
match bop.op {
    x if x == Op::Movi as u8 => { ... }
    x if x == Op::Mov  as u8 => { ... }
    ...
}
```
Pattern guards prevent the compiler from generating a jump table.
This compiles to a cascading if-else chain (46 branches, avg 23
comparisons per dispatch).

**Fix:** Match on the enum directly:
```rust
match unsafe { std::mem::transmute::<u8, Op>(bop.op) } {
    Op::Movi => { ... }
    Op::Mov  => { ... }
    ...
}
```
Or store `Op` (not `u8`) in `ByteOp` — the compiler can then emit a
direct indexed jump table.  Expected ~2× dispatch speedup.

### 3. Per-block heap allocations — ~100 ns / block

Every `exec_threaded` call:
- `vec![0u64; max_temp+1]` — heap alloc + zero-fill for temps
- `Vec::new()` for `mem_accesses`
- `.iter().fold()` to compute `max_temp`

**Fix:** Move `temps` and `mem_accesses` into the `TcgInterp` struct
and `.clear()` them between blocks.  Pre-compute `max_temp` at
compile time and store it in `CompiledBlock`.

### 4. regs_to_array / array_to_regs — ~84 ns / block

Copies all 42 registers into a flat array before every block and
back afterwards, even if the block only touches 2-3 registers.

**Fix (short-term):** Accept — 84 ns is small relative to sysreg
sync.  **(Long-term):** Have the TCG interpreter read/write directly
from the `Aarch64Cpu` register file via a pointer or trait, removing
the copy entirely.

### 5. Block cache is HashMap — ~50 ns / lookup

**Fix:** Use a direct-mapped cache (`[Option<CompiledBlock>; 4096]`)
indexed by `(pc >> 2) & 0xFFF` for the hot path, falling back to
HashMap on collision.  Cuts lookup from ~50 ns to ~5 ns.

### 6. mem_accesses Vec not needed in FE timing

`exec_threaded` builds a `Vec<MemAccess>` that `run_inner` ignores
in FE timing mode (`virtual_cycles += n`).

**Fix:** Add a `track_mem: bool` flag to skip `mem_accesses.push()`
when timing level is FE.

### 7. TCG op bloat — 4-8 ops per guest instruction

`ADD X1, X2, X3` emits: `ReadReg(2) → ReadReg(3) → Add(t, a, b) →
WriteReg(1, t)`.  With flag-setting variants this grows to 8+ ops.

**Fix:** Fused ops: `AddReg { dst_reg, src1_reg, src2_reg }` that
read, compute, and write in a single dispatch step.  Cuts op count
by ~50% for ALU-heavy code.

## Projected impact

| Fix | Savings/block | Cumulative MIPS |
|-----|--------------|----------------|
| Baseline (current TCG) | — | 4.8 |
| #1 Flat sysreg array | −2250 ns | ~8 |
| #2 Jump-table dispatch | −30% of dispatch | ~10 |
| #3 Reuse allocations | −100 ns | ~11 |
| #5 Direct-mapped cache | −45 ns | ~11.5 |
| #6 Skip mem tracking | −20 ns/load-store | ~12 |
| #7 Fused ops | −50% ops | ~16 |

With all fixes applied, TCG should reach **~15-20 MIPS** — a 3-4×
improvement over current interp, and a reasonable baseline before
adding a Cranelift JIT backend (~200+ MIPS).
