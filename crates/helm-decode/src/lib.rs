//! # helm-decode
//!
//! A QEMU-style decode-tree engine for HELM.  Instruction specifications
//! are written once in a declarative pattern format (`.decode` files) and
//! produce two decoder backends:
//!
//! | Backend | Mode | Output | Speed |
//! |---------|------|--------|-------|
//! | **TCG** | SE / FE | `TcgOp` chains for dynamic translation | fast |
//! | **Static** | APE / CAE | `MicroOp` sequences for pipeline model | detailed |
//!
//! ## Decode-Tree Pattern Format
//!
//! ```text
//! # Comments start with #
//! # MNEMONIC  bit_pattern
//! #   - '0' and '1' are fixed bits
//! #   - 'name:N' is a field of N bits
//! #   - '.' is a don't-care bit
//!
//! ADD_imm   sf:1 0 0 10001 sh:1 imm12:12 rn:5 rd:5
//! B         0 00101 imm26:26
//! NOP       11010101 00000011 00100000 000 11111
//! ```

pub mod field;
pub mod pattern;
pub mod tree;

pub use field::BitField;
pub use pattern::{DecodeLine, DecodePattern};
pub use tree::{DecodeNode, DecodeTree};

#[cfg(test)]
mod tests;
