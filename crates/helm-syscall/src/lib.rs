//! # helm-syscall
//!
//! Intercepts and emulates Linux system calls in user-mode.

pub mod aarch64;
pub mod aarch64_handler;
pub mod fd_table;
pub mod handler;
pub mod table;

pub use aarch64_handler::Aarch64SyscallHandler;
pub use handler::SyscallHandler;

#[cfg(test)]
mod tests;
