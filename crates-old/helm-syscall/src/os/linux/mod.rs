//! Linux syscall emulation.

pub mod aarch64;
pub mod generic;
pub mod handler;
pub mod table;

pub use generic::SyscallHandler;
pub use handler::Aarch64SyscallHandler;
