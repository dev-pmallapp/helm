//! `helm-jit` — dynasm-rs JIT backend for AArch64 → x86-64 translation.
//!
//! Compiles basic blocks of decoded AArch64 `Instruction`s into native x86-64
//! machine code using dynasm-rs. Unsupported opcodes cause block compilation
//! to stop; the caller falls back to the interpreter for those instructions.
//!
//! # Architecture
//!
//! - **Flat register array** (`regs.rs`): JIT code accesses guest registers via
//!   a `[u64; 48]` array passed in `rdi`. Sync functions copy to/from
//!   `Aarch64ArchState`.
//!
//! - **Block compiler** (`compiler.rs`): Iterates decoded instructions, emits
//!   x86-64 via dynasm macros, produces `CompiledBlock`.
//!
//! - **JIT cache** (`cache.rs`): 4096-entry direct-mapped cache keyed by guest
//!   PC. Stores function pointers to compiled blocks.
//!
//! - **Memory helpers** (`helpers.rs`): `extern "C"` functions callable from
//!   JIT'd code for guest memory access.
//!
//! - **Emitters** (`emit/`): Per-category x86-64 code generators for
//!   data-processing, load/store, branch, and system instructions.

#![allow(clippy::cast_possible_truncation)]
#![allow(clippy::cast_sign_loss)]

pub mod regs;
pub mod block;
pub mod cache;
pub mod helpers;
pub mod compiler;
pub mod emit;
