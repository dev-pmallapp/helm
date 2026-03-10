//! FreeBSD SE mode runner (future).
//!
//! Differences from Linux SE:
//! - Different syscall numbers and conventions
//! - Different ELF auxiliary vector entries
//! - Different ioctl numbers (termios, etc.)
//! - kqueue instead of epoll/ppoll
//! - Different /dev paths and procfs layout
