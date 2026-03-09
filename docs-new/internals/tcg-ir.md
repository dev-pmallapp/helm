# TCG IR

The `helm-tcg` crate defines a QEMU-inspired intermediate
representation for fast functional emulation.

## TcgOp Enum

`TcgOp` is a minimal RISC-like IR operating on `TcgTemp` virtual
registers:

| Category | Operations |
|----------|-----------|
| Moves | `Movi { dst, value }`, `Mov { dst, src }` |
| Arithmetic | `Add`, `Sub`, `Mul`, `Div`, `Addi` |
| Bitwise | `And`, `Or`, `Xor`, `Not`, `Shl`, `Shr`, `Sar` |
| Memory | `Load { dst, addr, size }`, `Store { addr, val, size }` |
| Control | `Br { label }`, `BrCond { cond, label }`, `Label { id }` |
| Comparison | `SetEq`, `SetNe`, `SetLt`, `SetGe` |
| Extension | `Sext { dst, src, from_bits }`, `Zext { dst, src, from_bits }` |
| Register | `ReadReg { dst, idx }`, `WriteReg { idx, src }` |
| System | `ReadSysReg`, `WriteSysReg`, `DaifSet`, `DaifClr`, `SetSpSel` |
| Terminator | `GotoTb { target }`, `Syscall`, `ExitTb`, `SvcExc`, `Eret`, `Wfi` |

## TcgTemp

`TcgTemp(u32)` is a virtual register identifier. The interpreter maps
these to a `Vec<u64>` of temporaries. The JIT maps them to Cranelift
SSA values.

## TcgBlock

A translated block:

- `guest_pc` — start address in guest space.
- `guest_size` — number of guest bytes covered.
- `insn_count` — number of guest instructions translated.
- `ops` — the `Vec<TcgOp>` sequence.

## TcgContext

Accumulates `TcgOp`s during translation. Provides temp allocation,
label management, and op emission helpers.
