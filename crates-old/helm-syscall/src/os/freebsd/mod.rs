//! FreeBSD syscall emulation (future).
//!
//! FreeBSD uses different syscall numbers and conventions:
//! - AArch64: syscall number in X8 (same as Linux)
//! - Struct layouts differ (stat, termios, etc.)
//! - ioctl numbers differ
//! - kqueue instead of epoll
//! - No /proc/self/exe — use sysctl KERN_PROC_PATHNAME
