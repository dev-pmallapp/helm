# Decode Files

Listing of `.decode` files used by HELM.

## Generated Decoders

At build time, `.decode` files are processed by `helm-decode` to
generate Rust source. Two crates use build-time code generation:

### helm-isa (Static Path)

Name decoders that return `&'static str` mnemonics:

| Generated File | Decode Group |
|----------------|-------------|
| `decode_aarch64_dp_imm.rs` | Data processing (immediate) |
| `decode_aarch64_dp_reg.rs` | Data processing (register) |
| `decode_aarch64_branch.rs` | Branch / exception / system |
| `decode_aarch64_ldst.rs` | Load / store |
| `decode_aarch64_fp.rs` | Scalar floating-point |
| `decode_aarch64_simd.rs` | Advanced SIMD |

### helm-tcg (TCG Path)

Trait + dispatch pairs for TCG emission:

| Generated Files | Decode Group |
|----------------|-------------|
| `decode_aarch64_branch_trait.rs` / `_dispatch.rs` | Branch |
| `decode_aarch64_dp_imm_trait.rs` / `_dispatch.rs` | DP immediate |
| `decode_aarch64_dp_reg_trait.rs` / `_dispatch.rs` | DP register |
| `decode_aarch64_ldst_trait.rs` / `_dispatch.rs` | Load/store |

## Decode File Syntax

See [decode-tree.md](../internals/decode-tree.md) for the full syntax
reference.

## Adding New Decode Groups

1. Create a `.decode` file with patterns.
2. Add `helm_decode::generate_decoder()` call in `build.rs`.
3. `include!()` the generated file in the appropriate handler module.
