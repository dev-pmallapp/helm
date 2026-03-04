//! # helm-syscall
//!
//! Intercepts and emulates Linux system calls in user-mode, similar to
//! `qemu-user`. This allows HELM to run unmodified user-space binaries
//! without a full OS image.

pub mod handler;
pub mod table;

pub use handler::SyscallHandler;

#[cfg(test)]
mod tests;
