//! Per-vCPU scoreboard — lock-free counters and data slots.
//!
//! Each vCPU only writes to its own slot.  The plugin aggregates
//! all slots at exit.  No synchronisation needed.

use std::cell::UnsafeCell;

/// A per-vCPU data array.  Thread-safe because each vCPU index is
/// owned by exactly one thread.
pub struct Scoreboard<T> {
    slots: Vec<UnsafeCell<T>>,
}

// SAFETY: Each vCPU thread only accesses its own index.
// The plugin reads all slots only after all vCPUs have stopped.
unsafe impl<T: Send> Sync for Scoreboard<T> {}
unsafe impl<T: Send> Send for Scoreboard<T> {}

impl<T: Default> Scoreboard<T> {
    /// Create a scoreboard with `n` slots (one per vCPU).
    pub fn new(n: usize) -> Self {
        let slots = (0..n).map(|_| UnsafeCell::new(T::default())).collect();
        Self { slots }
    }

    /// Get a shared reference to a vCPU's slot.
    ///
    /// # Safety
    /// Caller must ensure only the owning vCPU thread calls this
    /// during simulation, or that simulation has stopped.
    pub fn get(&self, vcpu_idx: usize) -> &T {
        unsafe { &*self.slots[vcpu_idx].get() }
    }

    /// Get a mutable reference to a vCPU's slot.
    ///
    /// # Safety
    /// Caller must ensure only the owning vCPU thread calls this.
    #[allow(clippy::mut_from_ref)]
    pub fn get_mut(&self, vcpu_idx: usize) -> &mut T {
        unsafe { &mut *self.slots[vcpu_idx].get() }
    }

    /// Number of slots.
    pub fn len(&self) -> usize {
        self.slots.len()
    }

    pub fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }

    /// Iterate over all slots (only safe after simulation stops).
    pub fn iter(&self) -> impl Iterator<Item = &T> {
        self.slots.iter().map(|cell| unsafe { &*cell.get() })
    }
}
