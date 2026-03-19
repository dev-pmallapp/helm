use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crate::trigger::Gate;

/// A closed instruction-count range [start, end) for gating observation.
pub struct Window {
    pub start: u64,
    pub end: u64,
    active: Arc<AtomicBool>,
}

impl Window {
    pub fn new(start: u64, end: u64) -> Self {
        Self {
            start,
            end,
            active: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Returns true iff insn_count is inside [start, end).
    /// Updates the cached `active` state.
    #[inline]
    pub fn is_active(&self, insn_count: u64) -> bool {
        let in_range = insn_count >= self.start && insn_count < self.end;
        self.active.store(in_range, Ordering::Relaxed);
        in_range
    }

    /// Read the cached active state without an insn_count check.
    /// Valid only after at least one `is_active()` call.
    #[inline]
    pub fn is_active_cached(&self) -> bool {
        self.active.load(Ordering::Relaxed)
    }

    /// Returns a Gate that shares the window's active flag.
    /// Stays in sync with `is_active()` / `subscribe_to_pre_step()` updates.
    pub fn gate(&self) -> Gate {
        Arc::clone(&self.active)
    }

    /// Subscribe to pre_step probe events to auto-update this window's active flag.
    /// After this call, closures using `is_active_cached()` reflect live state.
    #[cfg(debug_assertions)]
    pub fn subscribe_to_pre_step(self: &Arc<Self>, probes: &mut helm_probe::CpuProbes) {
        let w = Arc::clone(self);
        probes.pre_step.subscribe(move |_: &helm_probe::CpuStepEvent| {
            let n = helm_probe::probe_insn_count();
            w.active.store(n >= w.start && n < w.end, Ordering::Relaxed);
        });
    }
}

/// Wraps a primitive `T` and gates all recording to inside-window only.
pub struct Windowed<T> {
    pub window: Arc<Window>,
    pub inner: T,
}

impl<T> Windowed<T> {
    pub fn new(window: Arc<Window>, inner: T) -> Self {
        Self { window, inner }
    }

    /// Access the inner primitive only if inside the window.
    /// Returns None outside the window.
    #[inline]
    pub fn get_if_active(&self, insn_count: u64) -> Option<&T> {
        if self.window.is_active(insn_count) {
            Some(&self.inner)
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::Counter;

    #[test]
    fn window_basic_range() {
        let w = Window::new(100, 200);

        assert!(!w.is_active(0));
        assert!(!w.is_active(99));
        assert!(w.is_active(100));
        assert!(w.is_active(150));
        assert!(w.is_active(199));
        assert!(!w.is_active(200));
        assert!(!w.is_active(1000));
    }

    #[test]
    fn window_cached_state() {
        let w = Window::new(10, 20);

        assert!(!w.is_active_cached()); // default false

        w.is_active(15); // inside
        assert!(w.is_active_cached());

        w.is_active(25); // outside
        assert!(!w.is_active_cached());
    }

    #[test]
    fn windowed_gates_access() {
        let w = Arc::new(Window::new(100, 200));
        let counter = Counter::new("gated");
        let windowed = Windowed::new(Arc::clone(&w), counter);

        // Outside window
        assert!(windowed.get_if_active(50).is_none());

        // Inside window
        if let Some(c) = windowed.get_if_active(150) {
            c.inc();
        }
        assert_eq!(windowed.inner.value(), 1);

        // Outside window again
        assert!(windowed.get_if_active(250).is_none());
    }

    #[test]
    fn windowed_boundary_exact() {
        let w = Arc::new(Window::new(0, 10));
        let counter = Counter::new("boundary");
        let windowed = Windowed::new(Arc::clone(&w), counter);

        // start is inclusive
        assert!(windowed.get_if_active(0).is_some());
        // end is exclusive
        assert!(windowed.get_if_active(10).is_none());
    }
}
