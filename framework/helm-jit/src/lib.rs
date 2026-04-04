//! `helm-jit` — pluggable JIT backend framework for AArch64 → x86-64 translation.
//!
//! Compiles basic blocks of decoded AArch64 `Instruction`s into native x86-64
//! machine code. The framework provides backend-agnostic infrastructure
//! (cache, register layout, memory helpers, compiled block types) while
//! concrete backends live behind feature gates.
//!
//! # Architecture
//!
//! - **Backend trait** (`backend.rs`): `JitBackend` trait that backends implement.
//!
//! - **Compiled block** (`block.rs`): Backend-agnostic `CompiledBlock` wrapper
//!   that holds executable memory and an entry-point function pointer.
//!
//! - **Flat register array** (`regs.rs`): JIT code accesses guest registers via
//!   a `[u64; 48]` array passed in `rdi`. Sync functions copy to/from
//!   `Aarch64ArchState`.
//!
//! - **JIT cache** (`cache.rs`): 4096-entry direct-mapped cache keyed by guest
//!   PC. Stores function pointers to compiled blocks.
//!
//! - **Memory helpers** (`helpers.rs`): `extern "C"` functions callable from
//!   JIT'd code for guest memory access.
//!
//! - **Dynasm backend** (`dynasm/`, feature `backend-dynasm`): Per-category
//!   x86-64 code generators using dynasm-rs.

#![allow(clippy::cast_possible_truncation)]
#![allow(clippy::cast_sign_loss)]

pub mod backend;
pub mod block;
pub mod cache;
pub mod helpers;
pub mod regs;

#[cfg(feature = "backend-dynasm")]
pub mod arena;

#[cfg(feature = "backend-dynasm")]
pub mod dynasm;

#[cfg(feature = "backend-dynasm")]
pub mod trace;

#[cfg(feature = "backend-stencil")]
pub mod stencil;
