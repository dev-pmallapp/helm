# Adding Instructions

How to add a new AArch64 instruction to HELM.

## Steps

### 1. Add to .decode File

Create or update a `.decode` file with the instruction's bit pattern:

```
MY_INSN  1101_0101 .... .... .... .... .... ....  %rd %rn %imm
```

### 2. Implement in the Executor

In `crates/helm-isa/src/arm/aarch64/exec.rs`, add handling for the
new instruction in the appropriate dispatch block (DP-imm, DP-reg,
branch, load/store, etc.).

### 3. Implement in the TCG Emitter

In `crates/helm-tcg/src/a64_emitter.rs`, implement the generated
handler trait method to emit `TcgOp` sequences.

### 4. Add MicroOp Classification

In `crates/helm-isa/src/arm/aarch64/decode.rs`, add the instruction
to the appropriate `decode_*_to_opcode()` function.

### 5. Write Tests

- **Decode test** in `helm-isa/src/arm/aarch64/tests/decode.rs`.
- **Execution test** in `helm-isa/src/arm/aarch64/tests/exec_*.rs`.
- **TCG parity test** if the instruction is in the TCG path.

### 6. Run Pre-Commit

```bash
make pre-commit
```

## Example

Adding a hypothetical `MYADD` instruction:

1. `.decode`: `MYADD  0001_1010 00.. .... .... .... .... ....  %rd %rn %rm`
2. `exec.rs`: implement `myadd()` method with register reads/writes.
3. `a64_emitter.rs`: emit `ReadReg` + `Add` + `WriteReg` TcgOps.
4. `decode.rs`: map `"MYADD"` → `Opcode::IntAlu`.
5. Tests: verify encoding, execution, and flag behaviour.
