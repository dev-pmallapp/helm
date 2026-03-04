//! # helm-decode
//!
//! QEMU-compatible decode-tree engine for HELM.
//!
//! Supports the same `.decode` file syntax used by QEMU's
//! `scripts/decodetree.py`, so QEMU's upstream ARM `.decode` files
//! (e.g. `target/arm/tcg/a64.decode`) can be used directly.
//!
//! ## Supported Syntax
//!
//! | Element | Syntax | Purpose |
//! |---------|--------|---------|
//! | Field | `%name pos:len` | Named bit extraction |
//! | Argument set | `&name field1 field2 ...` | Group of fields for translate fn |
//! | Format | `@name pattern &argset` | Reusable bit pattern template |
//! | Pattern | `MNEMONIC bits @format` | Instruction encoding |
//! | Group | `{ pat1 \n pat2 }` | Overlapping patterns (first match) |
//! | Constraint | `field=value` | Fixed field value requirement |
//! | Comment | `# ...` | Ignored |
//!
//! ## Dual Backend
//!
//! The same `.decode` file drives two code paths:
//!
//! - **TCG path** (SE/FE): emits `TcgOp` chains via `helm-tcg`
//! - **Static path** (APE/CAE): emits `MicroOp` vecs via `helm-core::ir`

pub mod field;
pub mod format;
pub mod pattern;
pub mod tree;

pub use field::{BitField, FieldDef};
pub use format::FormatDef;
pub use pattern::{ArgSet, DecodeLine, DecodePattern};
pub use tree::{DecodeNode, DecodeTree};

#[cfg(test)]
mod tests;
