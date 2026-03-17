//! Generic SE runtime threading helpers.
//!
//! This module holds cross-ISA clone classification logic shared by SE runtimes.

use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

/// Linux clone flag mask for signal selection.
pub const CSIGNAL: u64 = 0x0000_00ff;

pub const CLONE_VM: u64 = 0x0000_0100;
pub const CLONE_FS: u64 = 0x0000_0200;
pub const CLONE_FILES: u64 = 0x0000_0400;
pub const CLONE_SIGHAND: u64 = 0x0000_0800;
pub const CLONE_SYSVSEM: u64 = 0x0004_0000;
pub const CLONE_THREAD: u64 = 0x0001_0000;
pub const CLONE_SETTLS: u64 = 0x0008_0000;
pub const CLONE_PARENT_SETTID: u64 = 0x0010_0000;
pub const CLONE_CHILD_CLEARTID: u64 = 0x0020_0000;
pub const CLONE_DETACHED: u64 = 0x0040_0000;
pub const CLONE_CHILD_SETTID: u64 = 0x0100_0000;
pub const CLONE_VFORK: u64 = 0x0000_4000;
pub const CLONE_IO: u64 = 0x8000_0000;
pub const CLONE_PARENT: u64 = 0x0000_8000;
pub const CLONE_PIDFD: u64 = 0x0000_1000;

/// Required flags for thread-style clone, matching QEMU linux-user.
pub const CLONE_THREAD_FLAGS: u64 =
    CLONE_VM | CLONE_FS | CLONE_FILES | CLONE_SIGHAND | CLONE_THREAD | CLONE_SYSVSEM;

/// Optional flags tolerated on thread-style clone.
pub const CLONE_OPTIONAL_THREAD_FLAGS: u64 =
    CLONE_SETTLS | CLONE_PARENT_SETTID | CLONE_CHILD_CLEARTID | CLONE_CHILD_SETTID | CLONE_PARENT;

/// Optional flags tolerated on fork-style clone.
pub const CLONE_OPTIONAL_FORK_FLAGS: u64 =
    CLONE_SETTLS | CLONE_PARENT_SETTID | CLONE_PIDFD | CLONE_CHILD_CLEARTID | CLONE_CHILD_SETTID;

/// Ignored flags, matching QEMU linux-user.
pub const CLONE_IGNORED_FLAGS: u64 = CLONE_DETACHED | CLONE_IO;

/// Supported clone style in the SE runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloneStyle {
    Thread,
    Fork,
}

/// Classification error for clone flags.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloneFlagsError {
    InvalidThreadFlags,
    InvalidForkFlags,
}

/// Error returned when creating a host thread for a guest clone request fails.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostThreadSpawnError {
    InvalidThreadFlags,
    InvalidForkFlags,
    UnsupportedCloneStyle,
    SpawnFailed,
}

/// Minimal host-thread runtime used by the SE runtime for guest thread-style clone.
pub struct HostThreadRuntime {
    next_tid: AtomicU64,
    handles: Mutex<Vec<std::thread::JoinHandle<()>>>,
}

/// Classify guest clone flags into thread-style or fork-style, using the same
/// high-level policy as QEMU linux-user.
pub fn classify_clone_flags(flags: u64) -> Result<CloneStyle, CloneFlagsError> {
    let mut flags = flags & !CLONE_IGNORED_FLAGS;

    // QEMU treats vfork as fork by clearing both VFORK and VM early.
    if flags & CLONE_VFORK != 0 {
        flags &= !(CLONE_VFORK | CLONE_VM);
    }

    if flags & CLONE_VM != 0 {
        let invalid_thread_flags = !(CSIGNAL | CLONE_THREAD_FLAGS | CLONE_OPTIONAL_THREAD_FLAGS);
        if (flags & CLONE_THREAD_FLAGS) != CLONE_THREAD_FLAGS || (flags & invalid_thread_flags) != 0 {
            return Err(CloneFlagsError::InvalidThreadFlags);
        }
        Ok(CloneStyle::Thread)
    } else {
        let invalid_fork_flags = !(CSIGNAL | CLONE_OPTIONAL_FORK_FLAGS);
        if (flags & invalid_fork_flags) != 0 || (flags & CSIGNAL) != libc::SIGCHLD as u64 {
            return Err(CloneFlagsError::InvalidForkFlags);
        }
        Ok(CloneStyle::Fork)
    }
}

/// Compute the child thread-pointer value after clone.
///
/// If `CLONE_SETTLS` is present, the guest-requested TLS value is used.
/// Otherwise the child inherits the parent thread-pointer value.
pub fn child_thread_pointer(parent_tp: u64, flags: u64, requested_tls: u64) -> u64 {
    if flags & CLONE_SETTLS != 0 {
        requested_tls
    } else {
        parent_tp
    }
}

impl HostThreadRuntime {
    pub fn new(main_tid: u64) -> Self {
        Self {
            next_tid: AtomicU64::new(main_tid + 1),
            handles: Mutex::new(Vec::new()),
        }
    }

    pub fn spawn_thread_for_clone<F>(&self, flags: u64, f: F) -> Result<u64, HostThreadSpawnError>
    where
        F: FnOnce() + Send + 'static,
    {
        match classify_clone_flags(flags) {
            Ok(CloneStyle::Thread) => {}
            Ok(CloneStyle::Fork) => return Err(HostThreadSpawnError::UnsupportedCloneStyle),
            Err(CloneFlagsError::InvalidThreadFlags) => {
                return Err(HostThreadSpawnError::InvalidThreadFlags);
            }
            Err(CloneFlagsError::InvalidForkFlags) => {
                return Err(HostThreadSpawnError::InvalidForkFlags);
            }
        }

        let tid = self.next_tid.fetch_add(1, Ordering::Relaxed);
        let handle = std::thread::Builder::new()
            .name(format!("helm-se-thread-{tid}"))
            .spawn(f)
            .map_err(|_| HostThreadSpawnError::SpawnFailed)?;
        self.handles.lock().unwrap().push(handle);
        Ok(tid)
    }

    pub fn join_all(&self) -> Result<(), HostThreadSpawnError> {
        let mut handles = self.handles.lock().unwrap();
        let to_join: Vec<_> = handles.drain(..).collect();
        drop(handles);
        for handle in to_join {
            handle.join().map_err(|_| HostThreadSpawnError::SpawnFailed)?;
        }
        Ok(())
    }
}
