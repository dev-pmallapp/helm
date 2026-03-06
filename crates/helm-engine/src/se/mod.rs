//! Syscall-Emulation mode runners.
//!
//! ```text
//! se/
//!   backend.rs    — ExecBackend enum (Interpretive / Tcg)
//!   linux.rs      — Linux SE mode (AArch64 primary)
//!   freebsd.rs    — FreeBSD SE mode (future)
//!   classify.rs   — A64 instruction classifier for timing
//! ```

pub mod backend;
pub mod classify;
pub mod freebsd;
pub mod linux;

pub use backend::ExecBackend;
pub use classify::classify_a64;
pub use linux::run_aarch64_se_with_plugins;
pub use linux::{run_aarch64_se, run_aarch64_se_timed, SeResult, SeTimedResult};
