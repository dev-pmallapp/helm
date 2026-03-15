//! `HelmEventBus` — synchronous, observable, named-event pub-sub.
//!
//! # Distinction from `EventQueue`
//! - `EventQueue` (helm-event): schedule callbacks at **future** tick T (deferred).
//! - `HelmEventBus`: fire observers **now**, inline, synchronously.
//!
//! # Checkpoint note
//! The bus is NOT checkpointed. Subscribers must re-register after a restore.
//! This is intentional — subscriptions are structural, not state.

use std::collections::HashMap;

/// A subscription handle. Drop to unsubscribe (TODO: implement Drop).
pub struct SubscriptionId(u64);

type Callback = Box<dyn Fn(u64) + Send>;

/// Synchronous event bus for observable simulation events.
///
/// Events are identified by string name (e.g. `"cpu.insn"`, `"uart.tx"`).
/// All subscribers are called inline — no async, no queuing.
pub struct HelmEventBus {
    next_id: u64,
    subscribers: HashMap<String, Vec<(u64, Callback)>>,
}

impl Default for HelmEventBus {
    fn default() -> Self { Self::new() }
}

impl HelmEventBus {
    pub fn new() -> Self {
        Self { next_id: 0, subscribers: HashMap::new() }
    }

    /// Subscribe to a named event. Returns a `SubscriptionId` (for future unsubscribe).
    pub fn subscribe(
        &mut self,
        event: impl Into<String>,
        cb: impl Fn(u64) + Send + 'static,
    ) -> SubscriptionId {
        let id = self.next_id;
        self.next_id += 1;
        self.subscribers
            .entry(event.into())
            .or_default()
            .push((id, Box::new(cb)));
        SubscriptionId(id)
    }

    /// Fire all subscribers for `event` with `val`. Synchronous — returns after all callbacks.
    pub fn fire(&self, event: &str, val: u64) {
        if let Some(subs) = self.subscribers.get(event) {
            for (_, cb) in subs { cb(val); }
        }
    }

    /// Unsubscribe by id.
    pub fn unsubscribe(&mut self, id: SubscriptionId) {
        for subs in self.subscribers.values_mut() {
            subs.retain(|(sid, _)| *sid != id.0);
        }
    }
}
