//! Enhanced device trait — the core abstraction for all simulated devices.
//!
//! Every device (UART, timer, DMA engine, bus bridge) implements [`Device`].
//! The trait supports:
//! - Unified read/write via [`Transaction`]
//! - Lifecycle: reset, checkpoint, restore
//! - Time-driven progress via `tick()`
//! - Region declaration for address-space placement

use crate::region::MemRegion;
use crate::transaction::Transaction;
use helm_core::types::Addr;
use helm_core::HelmResult;

/// Events emitted by devices during `tick()` or `transact()`.
///
/// The engine collects these and routes them (e.g. IRQ events go to the
/// IRQ router, DMA completions go to the DMA engine).
#[derive(Debug, Clone)]
pub enum DeviceEvent {
    /// Assert or de-assert an interrupt line.
    Irq { line: u32, assert: bool },
    /// A DMA channel completed its transfer.
    DmaComplete { channel: u32 },
    /// Device log message.
    Log { level: LogLevel, message: String },
}

/// Log severity for device messages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

/// Core trait for all simulated devices.
///
/// # Lifecycle
/// 1. Device is created and attached to a bus
/// 2. `reset()` is called before simulation starts
/// 3. `transact()` handles CPU/DMA-initiated reads and writes
/// 4. `tick()` is called periodically for timer/FIFO progress
/// 5. `checkpoint()` / `restore()` for save/load
pub trait Device: Send + Sync {
    /// Handle a read or write transaction.
    ///
    /// For reads, the device fills `txn.data` with the register value.
    /// For writes, the device consumes `txn.data`.
    /// The device adds its access latency to `txn.stall_cycles`.
    fn transact(&mut self, txn: &mut Transaction) -> HelmResult<()>;

    /// The MMIO region(s) this device occupies.
    fn regions(&self) -> &[MemRegion];

    /// Reset to power-on state.
    fn reset(&mut self) -> HelmResult<()> {
        Ok(())
    }

    /// Serialize device state for checkpointing.
    fn checkpoint(&self) -> HelmResult<serde_json::Value> {
        Ok(serde_json::Value::Null)
    }

    /// Restore device state from a checkpoint.
    fn restore(&mut self, _state: &serde_json::Value) -> HelmResult<()> {
        Ok(())
    }

    /// Called every N cycles for time-driven devices (timers, DMA, FIFOs).
    ///
    /// Returns events that the engine should route (IRQ assertions, etc.).
    fn tick(&mut self, _cycles: u64) -> HelmResult<Vec<DeviceEvent>> {
        Ok(vec![])
    }

    /// Human-readable device name.
    fn name(&self) -> &str;

    // ── Fast functional path (FE mode) ──────────────────────────────────
    //
    // These bypass Transaction allocation for maximum throughput in
    // functional emulation mode. Default impls delegate to transact()
    // so existing devices work unchanged. Devices that care can override
    // to skip timing-only logic.

    /// Fast-path register read — no Transaction overhead.
    ///
    /// Used by the engine in FE mode (TCG/LLVM-IR). Default delegates
    /// to `transact()`. Override in performance-critical devices.
    fn read_fast(&mut self, offset: Addr, size: usize) -> HelmResult<u64> {
        let mut txn = Transaction::read(offset, size);
        txn.offset = offset;
        self.transact(&mut txn)?;
        Ok(txn.data_u64())
    }

    /// Fast-path register write — no Transaction overhead.
    fn write_fast(&mut self, offset: Addr, size: usize, value: u64) -> HelmResult<()> {
        let mut txn = Transaction::write(offset, size, value);
        txn.offset = offset;
        self.transact(&mut txn)?;
        Ok(())
    }
}

/// Unique identifier for a device within a platform.
pub type DeviceId = u32;

// ── Legacy compatibility ────────────────────────────────────────────────────

use crate::mmio::{DeviceAccess, MemoryMappedDevice};

/// Wraps a [`MemoryMappedDevice`] so it can be used as a [`Device`].
///
/// This allows existing devices (AcceleratorDevice, etc.) to work with
/// the new Transaction-based bus without modification.
pub struct LegacyWrapper<T: MemoryMappedDevice> {
    inner: T,
    region: MemRegion,
}

impl<T: MemoryMappedDevice> LegacyWrapper<T> {
    pub fn new(inner: T) -> Self {
        let size = inner.region_size();
        let name = inner.device_name().to_string();
        let region = MemRegion {
            name,
            base: 0,
            size,
            kind: crate::region::RegionKind::Io,
            priority: 0,
        };
        Self { inner, region }
    }

    /// Access the wrapped device.
    pub fn inner(&self) -> &T {
        &self.inner
    }

    /// Mutably access the wrapped device.
    pub fn inner_mut(&mut self) -> &mut T {
        &mut self.inner
    }
}

impl<T: MemoryMappedDevice> Device for LegacyWrapper<T> {
    fn transact(&mut self, txn: &mut Transaction) -> HelmResult<()> {
        if txn.is_write {
            let value = txn.data_u64();
            let stall = self.inner.write(txn.offset, txn.size, value)?;
            txn.stall_cycles += stall;
        } else {
            let access: DeviceAccess = self.inner.read(txn.offset, txn.size)?;
            txn.set_data_u64(access.data);
            txn.stall_cycles += access.stall_cycles;
        }
        Ok(())
    }

    fn regions(&self) -> &[MemRegion] {
        std::slice::from_ref(&self.region)
    }

    fn reset(&mut self) -> HelmResult<()> {
        self.inner.reset()
    }

    fn name(&self) -> &str {
        self.inner.device_name()
    }
}
