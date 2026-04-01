//! Thread-local probe context for helm-arch execute functions.
//!
//! This module provides a thread-local mechanism for the engine to pass
//! CpuProbes into execute functions without changing their signatures.
//! The engine calls `set_current_probes()` before each step and
//! `clear_current_probes()` after.
//!
//! This is the "faster to ship" approach from the TODO: thread-local
//! `CURRENT_PROBES: RefCell<Option<*mut CpuProbes>>` set by the engine
//! before each step.

#[cfg(feature = "probe")]
use std::cell::RefCell;

#[cfg(feature = "probe")]
use helm_probe::CpuProbes;

#[cfg(feature = "probe")]
thread_local! {
    static CURRENT_PROBES: RefCell<Option<*mut CpuProbes>> = const { RefCell::new(None) };
}

/// Set the current thread's probe context.
///
/// # Safety
/// The caller must ensure that the `CpuProbes` reference remains valid
/// for the duration of the step (until `clear_current_probes()` is called).
/// This is guaranteed by the engine's single-threaded step loop.
#[cfg(feature = "probe")]
pub fn set_current_probes(probes: &mut CpuProbes) {
    CURRENT_PROBES.with(|cell| {
        *cell.borrow_mut() = Some(probes as *mut CpuProbes);
    });
}

/// Clear the current thread's probe context.
#[cfg(feature = "probe")]
pub fn clear_current_probes() {
    CURRENT_PROBES.with(|cell| {
        *cell.borrow_mut() = None;
    });
}

/// Access the current thread's probes for firing events.
///
/// Returns `None` if no probes are set (release builds, or between steps).
/// The returned reference is valid until `clear_current_probes()` is called.
#[cfg(feature = "probe")]
pub fn with_current_probes<F, R>(f: F) -> Option<R>
where
    F: FnOnce(&mut CpuProbes) -> R,
{
    CURRENT_PROBES.with(|cell| {
        let ptr = *cell.borrow();
        ptr.map(|p| {
            // SAFETY: The engine guarantees the pointer is valid between
            // set_current_probes() and clear_current_probes(), which
            // bracket each step() call. The step is single-threaded.
            let probes = unsafe { &mut *p };
            f(probes)
        })
    })
}

/// No-op versions for when the probe feature is disabled.
#[cfg(not(feature = "probe"))]
pub fn set_current_probes(_probes: &mut ()) {}

#[cfg(not(feature = "probe"))]
pub fn clear_current_probes() {}

#[cfg(test)]
#[cfg(feature = "probe")]
mod tests {
    use super::*;

    #[test]
    fn set_and_access_probes() {
        let mut probes = CpuProbes::default();
        set_current_probes(&mut probes);

        let found = with_current_probes(|p| p.any_active());
        assert_eq!(found, Some(false));

        clear_current_probes();
        let found = with_current_probes(|_| true);
        assert_eq!(found, None);
    }
}
