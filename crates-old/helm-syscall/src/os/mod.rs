//! OS-specific syscall emulation.
//!
//! Each supported OS has a sub-module with per-ISA syscall tables
//! and a handler that dispatches by syscall number.
//!
//! ```text
//! os/
//!   linux/
//!     aarch64.rs    syscall number constants
//!     handler.rs    Aarch64SyscallHandler
//!     generic.rs    generic SyscallHandler (old, for riscv/x86 stubs)
//!     table.rs      lookup(IsaKind, nr) -> Syscall enum
//!   freebsd/        (future)
//! ```

pub mod freebsd;
pub mod linux;
