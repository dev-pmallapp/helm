# Adding an ISA

How to add a new ISA (e.g. RISC-V, x86) to HELM.

## Architecture

New ISAs plug in via the `IsaFrontend` trait:

```rust
pub trait IsaFrontend: Send + Sync {
    fn name(&self) -> &str;
    fn decode(&self, pc: Addr, bytes: &[u8]) -> HelmResult<(Vec<MicroOp>, usize)>;
    fn min_insn_align(&self) -> usize;
}
```

## Steps

### 1. Create the Frontend Module

In `crates/helm-isa/src/`, create a new ISA directory:

```
crates/helm-isa/src/my_isa/
  mod.rs        — IsaFrontend impl
  decode.rs     — instruction decoding
  exec.rs       — instruction execution (optional)
  regs.rs       — register file definition
```

### 2. Implement IsaFrontend

The `decode()` method must:
- Read instruction bytes at the given PC.
- Return a `Vec<MicroOp>` with the correct `Opcode` classification.
- Return the number of bytes consumed.

### 3. Add Register File

Define the architectural register set in `regs.rs`.

### 4. Add .decode Files (Optional)

Create `.decode` files and add code generation to `build.rs`.

### 5. Add TCG Target (Optional)

In `crates/helm-tcg/src/target/`, add a register mapping module for
the new ISA.

### 6. Register in helm-isa

Add `pub mod my_isa;` to `crates/helm-isa/src/lib.rs`.

### 7. Write Tests

Add tests in `crates/helm-isa/src/tests/my_isa.rs` for decode and
execution coverage.

## Existing Stubs

- `crates/helm-isa/src/riscv/mod.rs` — RISC-V 64-bit stub.
- `crates/helm-isa/src/x86/mod.rs` — x86-64 stub.
- `crates/helm-tcg/src/target/riscv64/` — RISC-V register mapping.
- `crates/helm-tcg/src/target/x86_64/` — x86-64 register mapping.
