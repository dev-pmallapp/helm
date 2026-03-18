//! Interrupt model: `WireId`, `InterruptSink`, `InterruptWire`, `InterruptPin`.
//!
//! Implements the platform-wired, device-agnostic interrupt model described in
//! `docs/design/helm-devices/LLD-interrupt-model.md`.
//!
//! # Design principles
//!
//! - **Device knows no IRQ number.** A device asserts/deasserts its
//!   [`InterruptPin`]. That is the complete device-side API.
//! - **Wiring is a platform concern.** `World::wire_interrupt(pin, sink,
//!   wire_id)` connects a pin to a sink during `elaborate()`.
//! - **One pin, one wire, one sink.** [`InterruptPin`] is not `Clone`.
//!   Fan-out requires an explicit fan-out sink.

use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

// ── WireId ───────────────────────────────────────────────────────────────────

/// Opaque wire identifier.
///
/// Chosen by the platform at `World::wire_interrupt()` time. The value is
/// meaningful to the sink; the pin and wire infrastructure treat it as an
/// opaque `u64`.
///
/// For a PLIC, the platform passes `WireId::from(source_number)`.
/// For a GIC, the platform passes `WireId::from(spi_number)`.
/// For a test sink, any value works -- the test checks it by value.
///
/// `WireId` is `Copy` because sinks store it in collections (e.g. a `HashMap`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct WireId(u64);

impl WireId {
    /// Create a new `WireId` from a raw `u64` value.
    pub fn new(val: u64) -> Self {
        Self(val)
    }

    /// Return the underlying `u64` value.
    pub fn as_u64(self) -> u64 {
        self.0
    }
}

impl From<u64> for WireId {
    fn from(v: u64) -> Self {
        Self(v)
    }
}

impl From<u32> for WireId {
    fn from(v: u32) -> Self {
        Self(v as u64)
    }
}

impl From<usize> for WireId {
    fn from(v: usize) -> Self {
        Self(v as u64)
    }
}

// ── InterruptSink ────────────────────────────────────────────────────────────

/// Implemented by interrupt controllers that receive interrupt signals.
///
/// Common implementors: PLIC, ARM GIC, Intel 8259 PIC, test sinks.
///
/// Both methods are called synchronously from [`InterruptPin::assert()`] /
/// [`InterruptPin::deassert()`]. They must not block, must not acquire locks
/// that the calling device might hold, and must not re-enter the device that
/// triggered the interrupt.
///
/// `InterruptSink` must be `Send + Sync` because it is stored in an `Arc` and
/// may be called from any thread that owns a device (in future multi-threaded
/// simulations).
pub trait InterruptSink: Send + Sync {
    /// Called when a wire transitions from deasserted to asserted (0 -> 1).
    ///
    /// `wire_id` is the identifier chosen by the platform at wiring time.
    /// For a PLIC, `wire_id` carries the source number (e.g. 10 for UART).
    /// The sink uses `wire_id` to identify which of its inputs changed.
    fn on_assert(&self, wire_id: WireId);

    /// Called when a wire transitions from asserted to deasserted (1 -> 0).
    ///
    /// Same `wire_id` semantics as [`on_assert`](InterruptSink::on_assert).
    fn on_deassert(&self, wire_id: WireId);
}

// ── InterruptWire ────────────────────────────────────────────────────────────

/// Internal connection between an [`InterruptPin`] and an [`InterruptSink`].
///
/// Created by `World::wire_interrupt()`. Not part of the public API. Shared
/// via `Arc` between the owning `InterruptPin` and the `World`'s wire
/// registry (for checkpoint/restore access to assertion state).
pub(crate) struct InterruptWire {
    /// Opaque wire identifier -- meaningful to the sink (e.g. PLIC source number).
    pub(crate) wire_id: WireId,

    /// The interrupt controller (or test sink) that receives level changes.
    pub(crate) sink: Arc<dyn InterruptSink>,

    /// Current assertion state. Canonical source of truth.
    /// `true` = asserted. Stored atomically to allow lock-free `is_asserted()`.
    pub(crate) asserted: AtomicBool,
}

impl InterruptWire {
    /// Create a new wire connecting the given `wire_id` to the given `sink`.
    ///
    /// Returns an `Arc` because both the pin and the world wire registry hold
    /// references to it.
    #[allow(dead_code)]
    pub(crate) fn new(wire_id: WireId, sink: Arc<dyn InterruptSink>) -> Arc<Self> {
        Arc::new(Self {
            wire_id,
            sink,
            asserted: AtomicBool::new(false),
        })
    }
}

// ── InterruptPin ─────────────────────────────────────────────────────────────

/// A device's interrupt output pin.
///
/// The device owns this struct. The device calls [`assert()`](Self::assert) or
/// [`deassert()`](Self::deassert) to raise or lower the interrupt signal. The
/// device has no knowledge of where the signal goes, what interrupt number it
/// represents, or which controller receives it.
///
/// `InterruptPin` is **not** `Clone` and **not** `Copy` (Q70). One pin = one
/// wire = one sink. Platform fan-out requires an explicit fan-out sink
/// implementation.
///
/// Before `World::wire_interrupt()` is called, the pin is unconnected.
/// [`assert()`](Self::assert) on an unconnected pin is a no-op with a
/// `log::warn!()` (Q71).
pub struct InterruptPin {
    /// The active wire, set by `World::wire_interrupt()` at elaborate time.
    /// `None` until wired. After `startup()`, this is immutable.
    wire: Option<Arc<InterruptWire>>,
}

impl InterruptPin {
    /// Create a new, unconnected pin.
    pub fn new() -> Self {
        Self { wire: None }
    }

    /// Assert the interrupt -- propagate to the connected sink.
    ///
    /// If already asserted, this is a no-op (edge-triggered behavior: the sink
    /// is only called on level transitions, not on repeated assertions of the
    /// same level).
    ///
    /// If not connected: emits `log::warn!()`, returns without calling any
    /// sink. Does not panic (Q71).
    pub fn assert(&self) {
        match &self.wire {
            None => {
                log::warn!("InterruptPin::assert() called on unconnected pin -- no-op");
            }
            Some(wire) => {
                // Only propagate on 0 -> 1 transition.
                let was_asserted = wire.asserted.swap(true, Ordering::SeqCst);
                if !was_asserted {
                    wire.sink.on_assert(wire.wire_id);
                }
            }
        }
    }

    /// Deassert the interrupt -- propagate to the connected sink.
    ///
    /// If already deasserted, this is a no-op. If not connected: emits
    /// `log::warn!()`, returns without panicking.
    pub fn deassert(&self) {
        match &self.wire {
            None => {
                log::warn!("InterruptPin::deassert() called on unconnected pin -- no-op");
            }
            Some(wire) => {
                // Only propagate on 1 -> 0 transition.
                let was_asserted = wire.asserted.swap(false, Ordering::SeqCst);
                if was_asserted {
                    wire.sink.on_deassert(wire.wire_id);
                }
            }
        }
    }

    /// Query current assertion state.
    ///
    /// Returns `false` if unconnected (no wire = no assertion state).
    pub fn is_asserted(&self) -> bool {
        self.wire
            .as_ref()
            .map(|w| w.asserted.load(Ordering::SeqCst))
            .unwrap_or(false)
    }

    /// Connect this pin to a wire. Called only by `World::wire_interrupt()`.
    ///
    /// # Panics
    ///
    /// Panics if the pin is already connected (double-wiring is a
    /// configuration error).
    #[allow(dead_code)]
    pub(crate) fn connect(&mut self, wire: Arc<InterruptWire>) {
        assert!(
            self.wire.is_none(),
            "InterruptPin::connect() called on already-connected pin -- \
             double-wiring is a configuration error"
        );
        self.wire = Some(wire);
    }

    /// Set the assertion state directly, without triggering sink callbacks.
    ///
    /// Used by checkpoint restore to restore wire state without re-triggering
    /// `on_assert()` / `on_deassert()` on the sink (which may not yet be
    /// fully restored).
    #[allow(dead_code)]
    pub(crate) fn set_asserted_state(&self, asserted: bool) {
        if let Some(wire) = &self.wire {
            wire.asserted.store(asserted, Ordering::SeqCst);
        }
    }
}

impl Default for InterruptPin {
    fn default() -> Self {
        Self::new()
    }
}

// InterruptPin is intentionally NOT Clone and NOT Copy.
// This enforces the one-pin-one-wire-one-sink invariant (Q70).

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU32;

    /// A simple test sink that counts assert/deassert calls and records the
    /// last wire ID seen.
    struct TestSink {
        assert_count: AtomicU32,
        deassert_count: AtomicU32,
        last_wire_id: std::sync::Mutex<Option<WireId>>,
    }

    impl TestSink {
        fn new() -> Self {
            Self {
                assert_count: AtomicU32::new(0),
                deassert_count: AtomicU32::new(0),
                last_wire_id: std::sync::Mutex::new(None),
            }
        }

        fn assert_count(&self) -> u32 {
            self.assert_count.load(Ordering::SeqCst)
        }

        fn deassert_count(&self) -> u32 {
            self.deassert_count.load(Ordering::SeqCst)
        }

        fn last_wire_id(&self) -> Option<WireId> {
            *self.last_wire_id.lock().unwrap()
        }
    }

    impl InterruptSink for TestSink {
        fn on_assert(&self, wire_id: WireId) {
            self.assert_count.fetch_add(1, Ordering::SeqCst);
            *self.last_wire_id.lock().unwrap() = Some(wire_id);
        }

        fn on_deassert(&self, wire_id: WireId) {
            self.deassert_count.fetch_add(1, Ordering::SeqCst);
            *self.last_wire_id.lock().unwrap() = Some(wire_id);
        }
    }

    // ── WireId tests ─────────────────────────────────────────────────────

    #[test]
    fn wire_id_new_and_as_u64() {
        let id = WireId::new(42);
        assert_eq!(id.as_u64(), 42);
    }

    #[test]
    fn wire_id_from_u64() {
        let id = WireId::from(100u64);
        assert_eq!(id.as_u64(), 100);
    }

    #[test]
    fn wire_id_from_u32() {
        let id = WireId::from(10u32);
        assert_eq!(id.as_u64(), 10);
    }

    #[test]
    fn wire_id_from_usize() {
        let id = WireId::from(7usize);
        assert_eq!(id.as_u64(), 7);
    }

    #[test]
    fn wire_id_copy_and_eq() {
        let a = WireId::new(5);
        let b = a; // Copy
        assert_eq!(a, b);
    }

    #[test]
    fn wire_id_hash() {
        use std::collections::HashSet;
        let mut set = HashSet::new();
        set.insert(WireId::new(1));
        set.insert(WireId::new(2));
        set.insert(WireId::new(1));
        assert_eq!(set.len(), 2);
    }

    // ── InterruptPin basic tests ─────────────────────────────────────────

    #[test]
    fn unconnected_pin_defaults_to_not_asserted() {
        let pin = InterruptPin::new();
        assert!(!pin.is_asserted());
    }

    #[test]
    fn default_creates_unconnected_pin() {
        let pin = InterruptPin::default();
        assert!(!pin.is_asserted());
    }

    #[test]
    fn assert_on_unconnected_pin_does_not_panic() {
        let pin = InterruptPin::new();
        // Should warn but not panic.
        pin.assert();
        assert!(!pin.is_asserted());
    }

    #[test]
    fn deassert_on_unconnected_pin_does_not_panic() {
        let pin = InterruptPin::new();
        pin.deassert();
        assert!(!pin.is_asserted());
    }

    // ── Wired pin tests ──────────────────────────────────────────────────

    fn wired_pin(wire_id: u64) -> (InterruptPin, Arc<TestSink>) {
        let sink = Arc::new(TestSink::new());
        let wire = InterruptWire::new(WireId::new(wire_id), Arc::clone(&sink) as _);
        let mut pin = InterruptPin::new();
        pin.connect(wire);
        (pin, sink)
    }

    #[test]
    fn assert_calls_sink_on_assert() {
        let (pin, sink) = wired_pin(10);
        pin.assert();
        assert!(pin.is_asserted());
        assert_eq!(sink.assert_count(), 1);
        assert_eq!(sink.last_wire_id(), Some(WireId::new(10)));
    }

    #[test]
    fn deassert_calls_sink_on_deassert() {
        let (pin, sink) = wired_pin(10);
        pin.assert();
        pin.deassert();
        assert!(!pin.is_asserted());
        assert_eq!(sink.deassert_count(), 1);
    }

    #[test]
    fn repeated_assert_is_noop() {
        let (pin, sink) = wired_pin(10);
        pin.assert();
        pin.assert();
        pin.assert();
        assert_eq!(sink.assert_count(), 1, "sink should only be called once");
    }

    #[test]
    fn repeated_deassert_is_noop() {
        let (pin, sink) = wired_pin(10);
        // Never asserted, so deassert should be a no-op.
        pin.deassert();
        assert_eq!(sink.deassert_count(), 0);
    }

    #[test]
    fn assert_deassert_cycle() {
        let (pin, sink) = wired_pin(5);
        pin.assert();
        pin.deassert();
        pin.assert();
        pin.deassert();
        assert_eq!(sink.assert_count(), 2);
        assert_eq!(sink.deassert_count(), 2);
    }

    #[test]
    #[should_panic(expected = "double-wiring")]
    fn double_connect_panics() {
        let sink = Arc::new(TestSink::new());
        let wire1 = InterruptWire::new(WireId::new(1), Arc::clone(&sink) as _);
        let wire2 = InterruptWire::new(WireId::new(2), Arc::clone(&sink) as _);
        let mut pin = InterruptPin::new();
        pin.connect(wire1);
        pin.connect(wire2); // should panic
    }

    // ── Checkpoint restore tests ─────────────────────────────────────────

    #[test]
    fn set_asserted_state_does_not_call_sink() {
        let (pin, sink) = wired_pin(10);
        pin.set_asserted_state(true);
        assert!(pin.is_asserted());
        assert_eq!(sink.assert_count(), 0, "sink must not be called during restore");
    }

    #[test]
    fn set_asserted_state_false_restores_deasserted() {
        let (pin, sink) = wired_pin(10);
        pin.assert(); // sink called once
        pin.set_asserted_state(false);
        assert!(!pin.is_asserted());
        assert_eq!(sink.deassert_count(), 0, "sink must not be called during restore");
    }

    #[test]
    fn set_asserted_state_on_unconnected_pin_is_noop() {
        let pin = InterruptPin::new();
        pin.set_asserted_state(true); // should not panic
        assert!(!pin.is_asserted());
    }
}
