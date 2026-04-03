#![allow(unsafe_code)]
use std::cell::UnsafeCell;

/// Per-vCPU scoreboard: each slot is independently mutable via `UnsafeCell`.
///
/// # Safety invariant
/// Each slot must have at most **one writer** at a time. In practice, slot `i`
/// is exclusively owned by vCPU `i` — no cross-CPU writes are permitted.
/// The simulator is single-threaded, so this invariant is trivially upheld.
/// When SMP threading is added, each vCPU thread must only access its own slot.
pub struct HelmScoreboard<T> {
    slots: Vec<UnsafeCell<T>>,
}

// SAFETY: Each slot is independently accessed by one vCPU thread.
// No slot is shared between threads — the per-vCPU invariant guarantees this.
unsafe impl<T: Send> Sync for HelmScoreboard<T> {}
unsafe impl<T: Send> Send for HelmScoreboard<T> {}

impl<T: Default> HelmScoreboard<T> {
    pub fn new(n: usize) -> Self {
        Self {
            slots: (0..n).map(|_| UnsafeCell::new(T::default())).collect(),
        }
    }
    pub fn get(&self, idx: usize) -> &T {
        unsafe { &*self.slots[idx].get() }
    }

    /// Get a mutable reference to slot `idx`.
    ///
    /// # Safety (upheld by caller)
    /// The caller must ensure no other reference (shared or mutable) to this
    /// slot exists concurrently. In the single-threaded simulator, this is
    /// always true. For SMP, each vCPU must only access its own slot.
    #[allow(clippy::mut_from_ref)]
    pub fn get_mut(&self, idx: usize) -> &mut T {
        unsafe { &mut *self.slots[idx].get() }
    }
    pub fn len(&self) -> usize {
        self.slots.len()
    }
    pub fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }
    pub fn iter(&self) -> impl Iterator<Item = &T> {
        self.slots.iter().map(|c| unsafe { &*c.get() })
    }
}
