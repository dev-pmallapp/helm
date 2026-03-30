//! Per-ISA instruction → DecodedFields extraction.

#![allow(missing_docs)]

pub mod aarch64;
pub mod riscv64;

pub use aarch64::extract_fields_a64;
pub use riscv64::extract_fields_rv64;
