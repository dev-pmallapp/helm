# JIT debug session: L4Re boot divergence root cause

- Date: 2026-04-14
- Branch: `l4re-hello-jit`
- Tool: `examples/debug/l4re_jit_debug.py`

## Summary

Used the new JIT debug framework to identify a JIT correctness bug during
L4Re EL2 boot. The divergence occurs at **insn_count=192** (within the
first 200 guest instructions after entering the bootstrapper main init
function at `0x4100d0e0`).

## Root cause

Block chaining retirement counting is inconsistent with actual guest
instruction execution. A compiled block at `0x4100d160` (4 guest
instructions: LDR pre-index, CBZ, CMP, B.LE) is part of a chained
block cluster with `blk_insns=24`. The chain runs multiple iterations:

```
TRACE: retired=180 blk_insns=24 actual=4  (iteration 1)
TRACE: retired=184 blk_insns=24 actual=4  (iteration 2)
TRACE: retired=188 blk_insns=24 actual=4  (iteration 3: last good)
TRACE: retired=192 blk_insns=24 actual=4  (iteration 4: diverges here)
TRACE: retired=196 blk_insns=24 actual=18 (chain exit to 0x4100d198)
```

After 4 iterations of the 4-instruction loop body, x3 (advanced by
pre-index LDR `[x3, #0x10]!`) has accumulated 0x40 offset vs the
interpreter's 0x30. The extra iteration causes the JIT to load a
different x1 value from memory, which propagates the divergence into
all subsequent control flow.

## Affected block

```
0x4100d160: f8410c61  LDR x1, [x3, #0x10]!   (pre-index writeback)
0x4100d164: b4000121  CBZ x1, +0x24
0x4100d168: f100943f  CMP x1, #37
0x4100d16c: 54fffeed  B.LE -0x24 (-> 0x4100d148)
```

This is a dynamic ELF relocation processing loop. x3 walks a linked
list of relocation entries. x1 is the relocation type. The loop
processes entries until x1 is 0 (CBZ exit) or x1 > 37 (B.LE not-taken).

## Diagnosis

The block chain at `0x4100d160` accumulates retirement counts via ADD.
The `REG_JIT_RETIRED` slot reports `actual=4` per iteration (correct
for the 4-instruction body), but the block is part of a larger chain
that includes the inner loop at `0x4100d148`-`0x4100d16c`. The chain
budget guard (`MAX_CHAIN_BUDGET=4096`) does not fire because the total
is well under 4096.

The actual bug is that the chained block executes one too many
iterations of the pre-index LDR loop before comparing against the
interpreter's expected state. This points to either:

1. The LDR pre-index writeback interacting incorrectly with block
   chaining (x3 written back one extra time), or
2. The CMP/B.LE flag evaluation differing when re-entering the block
   via chaining vs a fresh dispatch.

## Verification method

```bash
target/release/helm-system-aarch64 --sim-trace=null: \
  examples/debug/l4re_jit_debug.py --mode bisect \
  --max-insns 1000 --checkpoint-interval 10
```

The aligned-insn-count comparison catches the divergence at exactly
insn_count=192 where both interpreter and JIT have retired the same
number of instructions but PCs differ.

## Next steps

1. Check if disabling block chaining eliminates the divergence
2. Examine the dynasm chained-block entry path for stale NZCV or
   register state from the previous block
3. Verify pre-index LDR writeback in chained block re-entry
