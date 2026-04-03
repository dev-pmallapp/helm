//! `helm-event` — discrete-event scheduler for future-tick callbacks.
//!
//! # Design note
//! This is **distinct** from `HelmEventBus` (in `helm-devices`):
//! - `EventQueue` schedules callbacks at future tick T (asynchronous / deferred).
//! - `HelmEventBus` fires synchronous observers at the moment of an event (inline).
//!
//! The queue is a min-heap over `(fire_at, seq)`. Events with equal `fire_at`
//! are ordered by insertion sequence (FIFO within the same tick).

#![allow(clippy::module_name_repetitions)]
#![allow(missing_docs)]

use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashSet};

/// Simulation time unit (abstract clock ticks, not wall-clock nanoseconds).
pub type Tick = u64;

/// Unique identifier for a posted event. Used for cancellation.
pub type EventId = u64;

// ── PendingEvent ──────────────────────────────────────────────────────────────

/// An event waiting to fire.
struct PendingEvent {
    fire_at: Tick,
    seq: EventId,
    class_id: u32,
    owner_id: u64,
    data: Box<dyn std::any::Any + Send>,
}

impl PartialEq for PendingEvent {
    fn eq(&self, other: &Self) -> bool {
        self.fire_at == other.fire_at && self.seq == other.seq
    }
}
impl Eq for PendingEvent {}

impl PartialOrd for PendingEvent {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for PendingEvent {
    /// Min-heap: smallest `fire_at` first; ties broken by insertion order.
    fn cmp(&self, other: &Self) -> Ordering {
        other
            .fire_at
            .cmp(&self.fire_at)
            .then(other.seq.cmp(&self.seq))
    }
}

// ── EventQueue ────────────────────────────────────────────────────────────────

/// Discrete-event scheduler.
///
/// Each crate that needs to schedule future work holds a shared `&mut EventQueue`
/// borrowed from the engine. There is exactly **one** queue per simulation.
pub struct EventQueue {
    heap: BinaryHeap<PendingEvent>,
    current_tick: Tick,
    next_seq: EventId,
    /// Lazy cancellation set — events whose `seq` is in this set are
    /// skipped during drain.
    cancelled: HashSet<EventId>,
}

impl Default for EventQueue {
    fn default() -> Self {
        Self::new()
    }
}

impl EventQueue {
    pub fn new() -> Self {
        Self {
            heap: BinaryHeap::new(),
            current_tick: 0,
            next_seq: 0,
            cancelled: HashSet::new(),
        }
    }

    // ── Queries ──

    pub fn current_tick(&self) -> Tick {
        self.current_tick
    }
    pub fn peek_next_tick(&self) -> Option<Tick> {
        self.heap.peek().map(|e| e.fire_at)
    }
    pub fn is_empty(&self) -> bool {
        self.heap.is_empty()
    }
    pub fn len(&self) -> usize {
        self.heap.len()
    }

    // ── Posting ──

    /// Schedule an event `delay` ticks from now. Returns its [`EventId`].
    pub fn post_after<D: std::any::Any + Send>(
        &mut self,
        delay: Tick,
        class_id: u32,
        owner_id: u64,
        data: D,
    ) -> EventId {
        self.post_at(self.current_tick + delay, class_id, owner_id, data)
    }

    /// Schedule an event at absolute tick `fire_at`. Returns its [`EventId`].
    pub fn post_at<D: std::any::Any + Send>(
        &mut self,
        fire_at: Tick,
        class_id: u32,
        owner_id: u64,
        data: D,
    ) -> EventId {
        let seq = self.next_seq;
        self.next_seq += 1;
        self.heap.push(PendingEvent {
            fire_at,
            seq,
            class_id,
            owner_id,
            data: Box::new(data),
        });
        seq
    }

    // ── Draining ──

    /// Advance simulation time to `until`, calling `handler` for each event that fires.
    ///
    /// `handler(class_id, owner_id, data)` — events fire in tick order, then insertion order.
    /// Cancel a previously posted event. Returns `true` if the event ID was
    /// not already cancelled. The event is lazily skipped during drain.
    pub fn cancel(&mut self, event_id: EventId) -> bool {
        self.cancelled.insert(event_id)
    }

    pub fn drain_until(
        &mut self,
        until: Tick,
        mut handler: impl FnMut(u32, u64, Box<dyn std::any::Any + Send>),
    ) {
        self.current_tick = until;
        while let Some(e) = self.heap.peek() {
            if e.fire_at > until {
                break;
            }
            let e = self.heap.pop().unwrap();
            if self.cancelled.remove(&e.seq) {
                continue; // lazily skip cancelled events
            }
            handler(e.class_id, e.owner_id, e.data);
        }
    }

    /// Advance time without processing events (e.g. fast-forward past idle periods).
    pub fn advance_to(&mut self, tick: Tick) {
        assert!(tick >= self.current_tick, "cannot go backwards in time");
        self.current_tick = tick;
    }
}

// ── SharedEventQueue ────────────────────────────────────────────────────────

use std::sync::Mutex;

/// Thread-safe wrapper around [`EventQueue`] that implements [`helm_core::TimerScheduler`].
///
/// Wraps the queue in a `Mutex` so it can be shared via `Arc<dyn TimerScheduler>`.
pub struct SharedEventQueue {
    inner: Mutex<EventQueue>,
}

impl SharedEventQueue {
    /// Wrap an existing queue.
    pub fn new(queue: EventQueue) -> Self {
        Self {
            inner: Mutex::new(queue),
        }
    }

    /// Access the inner queue for draining (engine-side, single-threaded).
    pub fn lock(&self) -> std::sync::MutexGuard<'_, EventQueue> {
        self.inner.lock().expect("EventQueue mutex poisoned")
    }
}

impl helm_core::TimerScheduler for SharedEventQueue {
    fn schedule_callback(&self, delay_ticks: u64, class_id: u32, owner_id: u64) -> u64 {
        self.inner
            .lock()
            .expect("EventQueue mutex poisoned")
            .post_after(delay_ticks, class_id, owner_id, ())
    }

    fn current_tick(&self) -> u64 {
        self.inner
            .lock()
            .expect("EventQueue mutex poisoned")
            .current_tick()
    }

    fn cancel(&self, event_id: u64) -> bool {
        self.inner
            .lock()
            .expect("EventQueue mutex poisoned")
            .cancel(event_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancel_prevents_delivery() {
        let mut q = EventQueue::new();
        let id_a = q.post_at(10, 1, 0, ());
        let _id_b = q.post_at(10, 2, 0, ());

        assert!(q.cancel(id_a));

        let mut delivered = Vec::new();
        q.drain_until(10, |class_id, _, _| {
            delivered.push(class_id);
        });

        // Only event B should be delivered
        assert_eq!(delivered, vec![2]);
    }

    #[test]
    fn cancel_nonexistent_returns_true_idempotent() {
        let mut q = EventQueue::new();
        // Cancelling an ID that was never posted still inserts into cancelled set
        assert!(q.cancel(999));
    }
}
