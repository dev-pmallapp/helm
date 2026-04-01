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
#[non_exhaustive]
#[derive(Debug, Clone)]
pub enum HelmEvent {
    /// Simulation run started.
    SimStart,
    /// Simulation run stopped.
    SimStop { reason: String },
    /// CPU exception taken.
    Exception { cpu: u64, vector: u32, tval: u64, pc: u64 },
    /// CSR/system register written.
    CsrWrite { cpu: u64, csr: u16, old: u64, new: u64 },
    /// External interrupt delivered.
    ExternalIrq { cpu: u64, irq_num: u32 },
    /// Software breakpoint hit.
    Breakpoint { cpu: u64, addr: u64, bp_id: u32 },
    /// Magic instruction executed (e.g., gem5-style work markers).
    MagicInsn { cpu: u64, pc: u64, value: u64 },
    /// Memory write completed.
    MemWrite { addr: u64, size: usize, val: u64, cycle: u64 },
    /// Memory read completed.
    MemRead { addr: u64, size: usize, val: u64, cycle: u64 },
    /// Syscall entry.
    SyscallEnter { nr: u64, args: [u64; 6] },
    /// Syscall return.
    SyscallReturn { nr: u64, ret: u64 },
    /// Device signal assertion/deassertion.
    DeviceSignal { device: String, port: String, asserted: bool },
    /// Custom event from DLD plugins.
    Custom { name: String, data: Arc<dyn std::any::Any + Send + Sync> },
}

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

    /// Unsubscribe by id.
    pub fn unsubscribe(&mut self, id: SubscriptionId) {
        for subs in self.subscribers.values_mut() {
            subs.retain(|(sid, _)| *sid != id.0);
        }
    }
}
