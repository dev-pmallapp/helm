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
use std::sync::Arc;

// ── HelmEvent (typed, non-exhaustive) ────────────────────────────────────────

/// Typed simulation event for the HelmEventBus.
///
/// Marked `#[non_exhaustive]` so DLD plugins compiled against older SDK
/// versions are not broken by new variants. DLD authors should use
/// `Custom { name, data }` for plugin-defined events.
// Discriminant assignment (stable for DLD binary compatibility):
//   SimStart = 0, SimStop = 1, Exception = 2, CsrWrite = 3,
//   ExternalIrq = 4, Breakpoint = 5, MagicInsn = 6,
//   MemWrite = 7, MemRead = 8, SyscallEnter = 9, SyscallReturn = 10,
//   DeviceSignal = 11, Custom = 12.
// New variants MUST be appended at the end with the next sequential number.
// DLD plugins should use the `Custom { name, data }` variant for plugin-defined events.
#[non_exhaustive]
#[derive(Debug, Clone)]
pub enum HelmEvent {
    /// Simulation run started.
    SimStart,
    /// Simulation run stopped.
    SimStop { reason: String },
    /// CPU exception taken.
    Exception {
        cpu: u64,
        vector: u32,
        tval: u64,
        pc: u64,
    },
    /// CSR/system register written.
    CsrWrite {
        cpu: u64,
        csr: u16,
        old: u64,
        new: u64,
    },
    /// External interrupt delivered.
    ExternalIrq { cpu: u64, irq_num: u32 },
    /// Software breakpoint hit.
    Breakpoint { cpu: u64, addr: u64, bp_id: u32 },
    /// Magic instruction executed (e.g., gem5-style work markers).
    MagicInsn { cpu: u64, pc: u64, value: u64 },
    /// Memory write completed.
    MemWrite {
        addr: u64,
        size: usize,
        val: u64,
        cycle: u64,
    },
    /// Memory read completed.
    MemRead {
        addr: u64,
        size: usize,
        val: u64,
        cycle: u64,
    },
    /// Syscall entry.
    SyscallEnter { nr: u64, args: [u64; 6] },
    /// Syscall return.
    SyscallReturn { nr: u64, ret: u64 },
    /// Device signal assertion/deassertion.
    DeviceSignal {
        device: String,
        port: String,
        asserted: bool,
    },
    /// Custom event from DLD plugins.
    Custom {
        name: String,
        data: Arc<dyn std::any::Any + Send + Sync>,
    },
}

/// A subscription handle. Drop to unsubscribe (TODO: implement Drop).
pub struct SubscriptionId(u64);

/// Interned event name ID for hot-path `fire_id()` calls.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct EventNameId(u64);

type Callback = Box<dyn Fn(u64) + Send>;

/// Synchronous event bus for observable simulation events.
///
/// Events are identified by string name (e.g. `"cpu.insn"`, `"uart.tx"`).
/// All subscribers are called inline — no async, no queuing.
///
/// For hot-path use, call [`intern()`](Self::intern) at setup time to get an
/// [`EventNameId`], then use [`fire_id()`](Self::fire_id) to avoid per-call
/// string hashing.
pub struct HelmEventBus {
    next_id: u64,
    subscribers: HashMap<String, Vec<(u64, Callback)>>,
    /// Interned event name → index into `indexed_subscribers`.
    name_to_id: HashMap<String, EventNameId>,
    /// Flat lookup by interned ID → subscriber list.
    indexed_subscribers: Vec<Vec<(u64, Callback)>>,
}

impl Default for HelmEventBus {
    fn default() -> Self {
        Self::new()
    }
}

impl HelmEventBus {
    /// Create a new, empty event bus.
    pub fn new() -> Self {
        Self {
            next_id: 0,
            subscribers: HashMap::new(),
            name_to_id: HashMap::new(),
            indexed_subscribers: Vec::new(),
        }
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
            for (_, cb) in subs {
                cb(val);
            }
        }
    }

    /// Intern an event name for fast `fire_id()` lookups.
    ///
    /// Returns the same [`EventNameId`] for the same name. Call at setup time.
    pub fn intern(&mut self, event: impl Into<String>) -> EventNameId {
        let name = event.into();
        if let Some(&id) = self.name_to_id.get(&name) {
            return id;
        }
        let id = EventNameId(self.indexed_subscribers.len() as u64);
        self.indexed_subscribers.push(Vec::new());
        self.name_to_id.insert(name, id);
        id
    }

    /// Fire all subscribers by interned ID. No string hashing on this path.
    #[inline]
    pub fn fire_id(&self, id: EventNameId, val: u64) {
        if let Some(subs) = self.indexed_subscribers.get(id.0 as usize) {
            for (_, cb) in subs {
                cb(val);
            }
        }
    }

    /// Unsubscribe by id.
    pub fn unsubscribe(&mut self, id: SubscriptionId) {
        for subs in self.subscribers.values_mut() {
            subs.retain(|(sid, _)| *sid != id.0);
        }
        for subs in &mut self.indexed_subscribers {
            subs.retain(|(sid, _)| *sid != id.0);
        }
    }
}
