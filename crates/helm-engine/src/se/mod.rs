//! Syscall-Emulation mode runners.
//!
//! ```text
//! se/
//!   linux.rs     — Linux SE mode (AArch64 primary)
//!   freebsd.rs   — FreeBSD SE mode (future)
//! ```

pub mod freebsd;
pub mod linux;

pub use linux::run_aarch64_se_with_plugins;
pub use linux::{run_aarch64_se, SeResult};
