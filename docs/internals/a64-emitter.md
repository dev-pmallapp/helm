# A64 Emitter

The `A64TcgEmitter` translates AArch64 instructions into `TcgOp`
sequences for the TCG path.

## Architecture

```text
insn (u32)
    │
    ▼
A64TcgEmitter::translate_insn(insn)
    │
    ├── op0 bits [28:25] select dispatch function
    │   ├── 10xx → decode_aarch64_dp_imm_dispatch
    │   ├── 101x → decode_aarch64_branch_dispatch
    │   ├── x1x0 → decode_aarch64_ldst_dispatch
    │   └── x101 → decode_aarch64_dp_reg_dispatch
    │
    ├── Generated trait methods (from .decode files) emit TcgOps
    │   into the TcgContext
    │
    └── Returns TranslateAction:
        ├── Continue    — more instructions in this block
        ├── EndBlock    — block-ending instruction (branch, etc.)
        └── Unhandled   — fall back to interpreter
```

## Generated Code

At build time, `helm-decode` generates:
- **Trait files** (e.g. `decode_aarch64_branch_trait.rs`) — one method
  per mnemonic (e.g. `handle_B`, `handle_BL`).
- **Dispatch files** (e.g. `decode_aarch64_branch_dispatch.rs`) —
  match-based dispatch calling the trait methods.

The emitter `include!`s these files and implements the handler methods
to emit appropriate `TcgOp` sequences.

## Key Patterns

- **Immediate operations** — `Movi` + `Add`/`Sub`/`And`/`Or`.
- **Register-register** — `ReadReg` + arithmetic + `WriteReg`.
- **Loads/Stores** — address computation + `Load`/`Store` ops.
- **Branches** — `GotoTb` for unconditional, `BrCond` + `GotoTb`
  for conditional; `end_block = true`.
- **SVC** — `SvcExc` in FS mode, `Syscall` in SE mode.
- **Flag computation** — `Add` with `SetEq`/`SetLt` for NZCV.

## Bitmask Immediate Decoding

`decode_bitmask(n, imms, immr, is64)` implements the AArch64 logical
immediate decoding algorithm (shared by AND/ORR/EOR immediate).

## Sign Extension Helper

`sext(val, bits)` sign-extends a `bits`-wide value to `i64`.
