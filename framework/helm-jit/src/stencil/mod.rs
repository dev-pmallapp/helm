//! Stencil (copy-and-patch) JIT backend.
//!
//! Compiles guest instructions by copying pre-compiled x86-64 code templates
//! and patching relocation holes with instruction-specific values. This is
//! 10-20x faster than runtime instruction encoding (dynasm) at the cost of
//! less optimization opportunity.
//!
//! # Architecture
//!
//! - **`types`**: Core type definitions (`HoleKind`, `Stencil`, `DecodedFields`, etc.)
//! - **`compiler`**: Block compilation (`compile_block`, `MmapBuffer`, `resolve_hole`)
//! - **`data`**: Per-ISA stencil byte arrays and relocation tables (build-time generated)
//! - **`fields`**: Per-ISA instruction → `DecodedFields` extraction
//! - **`regs_rv64`**: Flat register array layout for RISC-V64

pub mod types;
pub mod compiler;
pub mod data;
pub mod fields;
pub mod regs_rv64;

mod backend;
pub use backend::StencilBackend;

pub use types::{
    DecodedFields, HelperFn, HoleKind, RegField, Stencil, StencilReloc,
};
pub use compiler::{compile_block, resolve_hole, MmapBuffer};
