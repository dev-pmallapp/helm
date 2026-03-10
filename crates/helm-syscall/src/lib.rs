//! # helm-syscall
//!
//! Syscall emulation for SE mode. Supports multiple OSes and ISAs.
//!
//! ```text
//! os/linux/aarch64   — AArch64 Linux syscalls (primary target)
//! os/linux/table     — generic Linux syscall lookup
//! os/freebsd/        — FreeBSD syscalls (future)
//! ```

pub mod adapter;
pub mod fd_table;
pub mod os;

// Re-exports for convenience
pub use adapter::TraitSyscallHandler;
pub use os::linux::handler::{SyscallAction, ThreadBlockReason};
pub use os::linux::Aarch64SyscallHandler;
pub use os::linux::SyscallHandler;

#[cfg(test)]
mod tests;
