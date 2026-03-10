//! Priority event queue for event-driven simulation.

use std::cmp::Reverse;
use std::collections::BinaryHeap;

/// A simulation event scheduled for a specific cycle.
#[derive(Debug, Clone)]
pub struct ScheduledEvent {
    pub timestamp: u64,
    pub priority: u32,
    pub tag: u64,
}

impl Eq for ScheduledEvent {}
impl PartialEq for ScheduledEvent {
    fn eq(&self, other: &Self) -> bool {
        self.timestamp == other.timestamp && self.priority == other.priority
    }
}
impl Ord for ScheduledEvent {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        (Reverse(self.timestamp), Reverse(self.priority))
            .cmp(&(Reverse(other.timestamp), Reverse(other.priority)))
    }
}
impl PartialOrd for ScheduledEvent {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

/// Min-heap event queue keyed by (timestamp, priority).
pub struct EventQueue {
    heap: BinaryHeap<ScheduledEvent>,
    current_time: u64,
}

impl EventQueue {
    pub fn new() -> Self {
        Self {
            heap: BinaryHeap::new(),
            current_time: 0,
        }
    }

    /// Schedule an event at a future cycle.
    pub fn schedule(&mut self, timestamp: u64, priority: u32, tag: u64) {
        debug_assert!(
            timestamp >= self.current_time,
            "cannot schedule in the past"
        );
        self.heap.push(ScheduledEvent {
            timestamp,
            priority,
            tag,
        });
    }

    /// Pop the earliest event, advancing current time.
    pub fn pop(&mut self) -> Option<ScheduledEvent> {
        let event = self.heap.pop()?;
        self.current_time = event.timestamp;
        Some(event)
    }

    /// Peek at the next event's timestamp without consuming it.
    pub fn peek_time(&self) -> Option<u64> {
        self.heap.peek().map(|e| e.timestamp)
    }

    pub fn current_time(&self) -> u64 {
        self.current_time
    }

    pub fn is_empty(&self) -> bool {
        self.heap.is_empty()
    }

    pub fn len(&self) -> usize {
        self.heap.len()
    }
}

impl Default for EventQueue {
    fn default() -> Self {
        Self::new()
    }
}
