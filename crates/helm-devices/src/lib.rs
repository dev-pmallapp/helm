//! `helm-devices` — device trait, interrupt model, device registry, and `HelmEventBus`.
//!
//! # Key types
//! - [`Device`]         — MMIO device interface (read/write callbacks)
//! - [`InterruptPin`]   — device-side interrupt signal (knows no IRQ number)
//! - [`InterruptSink`]  — platform-side interrupt receiver (e.g. PLIC, GIC)
//! - [`DeviceRegistry`] — factory for named device types (including .so plugins)
//!
//! # Design rules
//! - **Device knows no base address** — `MemoryMap` owns placement.
//! - **Device knows no IRQ number**   — `InterruptPin` fires a signal; the platform routes it.
//! - `HelmEventBus` is synchronous (see `bus` submodule); not checkpointed.

pub mod bus;

use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Weak,
};

// ── Device ────────────────────────────────────────────────────────────────────

/// MMIO device interface.
///
/// `offset` is always relative to the device's own base (which the device does
/// not know — the `MemoryMap` subtracts the base before calling).
pub trait Device: Send {
    /// Read `size` bytes at `offset`. Returns value as little-endian `u64`.
    fn read(&self, offset: u64, size: usize) -> u64;

    /// Write `val` (`size` bytes) at `offset`.
    fn write(&mut self, offset: u64, size: usize, val: u64);

    /// Total size of this device's MMIO window in bytes.
    fn region_size(&self) -> u64;

    /// Receive a named signal from another device or platform (e.g. clock tick).
    fn signal(&mut self, _name: &str, _val: u64) {}

    /// Called during `elaborate()` — device stores refs to shared resources.
    fn elaborate(&mut self) {}

    /// Called during `startup()` — schedule initial events, assert initial signals.
    fn startup(&mut self) {}

    /// Reset to post-startup state.
    fn reset(&mut self) {}
}

// ── InterruptPin / Wire / Sink ────────────────────────────────────────────────

/// Device-side interrupt signal. The device asserts/deasserts this; the platform wires it.
///
/// A pin is created by the device and handed to the platform during `elaborate()`.
/// The platform calls `InterruptPin::connect(sink, wire_id)` to route it.
pub struct InterruptPin {
    state: Arc<AtomicBool>,
    wire: Option<(Weak<dyn InterruptSink + Send + Sync>, u32)>,
}

impl InterruptPin {
    pub fn new() -> Self {
        Self { state: Arc::new(AtomicBool::new(false)), wire: None }
    }

    /// Wire this pin to a sink (e.g. PLIC) with the given `wire_id` (IRQ line).
    pub fn connect(&mut self, sink: Weak<dyn InterruptSink + Send + Sync>, wire_id: u32) {
        self.wire = Some((sink, wire_id));
    }

    /// Assert the interrupt line.
    pub fn assert(&self) {
        if !self.state.swap(true, Ordering::SeqCst) {
            if let Some((sink, id)) = &self.wire {
                if let Some(s) = sink.upgrade() { s.on_assert(*id); }
            }
        }
    }

    /// Deassert the interrupt line.
    pub fn deassert(&self) {
        if self.state.swap(false, Ordering::SeqCst) {
            if let Some((sink, id)) = &self.wire {
                if let Some(s) = sink.upgrade() { s.on_deassert(*id); }
            }
        }
    }

    /// Current state (true = asserted).
    pub fn is_asserted(&self) -> bool { self.state.load(Ordering::Relaxed) }
}

impl Default for InterruptPin {
    fn default() -> Self { Self::new() }
}

/// Platform-side interrupt controller interface.
pub trait InterruptSink {
    fn on_assert(&self, wire_id: u32);
    fn on_deassert(&self, wire_id: u32);
}

// ── DeviceRegistry ────────────────────────────────────────────────────────────

/// Factory for creating named device types.
///
/// Built-in devices are registered at startup. Dynamic (.so) plugins are
/// loaded via `load_plugin()` (Phase 1+).
#[derive(Default)]
pub struct DeviceRegistry {
    // TODO(phase-1): Map<name → Box<dyn DeviceFactory>>
}

impl DeviceRegistry {
    pub fn new() -> Self { Self::default() }

    /// Load a device plugin from a shared library.
    pub fn load_plugin(&mut self, _path: &std::path::Path) -> Result<(), PluginError> {
        Err(PluginError::NotImplemented)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum PluginError {
    #[error("plugin loading not yet implemented")]
    NotImplemented,
    #[error("plugin not found: {name}")]
    NotFound { name: String },
}
