# Interpreter

The match-based TCG interpreter in `helm-tcg`.

## Design

`TcgInterp` walks a `TcgBlock`'s op list and executes each `TcgOp`
via a Rust `match`. It maintains:

- `temps: Vec<u64>` — temporary register file for `TcgTemp` values.
- `sysregs: Vec<u64>` — flat system register array (32K entries)
  indexed by the 15-bit sysreg encoding minus `SYSREG_BASE` (0x8000).

## Register Layout

Guest architectural state is stored in a flat array passed to the
interpreter. Key indices (defined in `target::aarch64::regs`):

| Index | Register |
|-------|----------|
| 0–30 | X0–X30 |
| 31 | SP |
| 32 | PC |
| 33 | NZCV |
| 34 | DAIF |
| 35+ | ELR_EL1, SPSR_EL1, ESR_EL1, VBAR_EL1, CURRENT_EL, SPSEL, SP_EL1, ... |

Total: `NUM_REGS` entries.

## Execution Loop

```rust
for op in &block.ops {
    match op {
        TcgOp::Movi { dst, value } => temps[dst] = *value,
        TcgOp::Add { dst, a, b }   => temps[dst] = temps[a] + temps[b],
        TcgOp::Load { dst, addr, size } => {
            temps[dst] = mem.read(temps[addr], *size)?;
        }
        TcgOp::BrCond { cond, label } => {
            if temps[cond] != 0 { jump to label; }
        }
        TcgOp::GotoTb { target } => return Chain { target },
        TcgOp::Syscall { .. }    => return Syscall { nr },
        ...
    }
}
```

## InterpExit

The interpreter returns an `InterpExit` variant indicating how the
block ended: `EndOfBlock`, `Chain`, `Syscall`, `Exit`, `Wfi`,
`Exception`, or `ExceptionReturn`.

## Threaded Interpreter

`helm-tcg::threaded` provides a faster variant that converts
`Vec<TcgOp>` into a compact bytecode representation and executes via a
function-pointer dispatch table, avoiding per-op `match` overhead.
Each instruction is packed into a fixed-size `[u64; 2]` slot.
