//! Core device interface and error types.
//!
//! The [`Device`] trait is the fundamental MMIO device abstraction. Devices
//! receive reads and writes at byte offsets within their mapped region, receive
//! named signal assertions, and optionally handle bus transactions with full
//! context.
//!
//! A device has **no knowledge of its base address or IRQ number** -- those are
//! platform configuration concerns handled by `MemoryMap` and interrupt wiring.

use std::any::Any;

use super::transaction::Transaction;

/// Extract a sub-word value from a naturally-aligned 32-bit word.
///
/// `word` is the full 32-bit register value. `offset` is the device byte
/// offset (low 2 bits select the byte lane). `size` is 1, 2, or 4.
///
/// Returns the extracted value in the low bits of a `u32`.
#[inline]
pub fn extract_subword(word: u32, offset: u64, size: usize) -> u32 {
    let shift = ((offset & 0x3) * 8) as u32;
    let mask = match size {
        1 => 0xFF,
        2 => 0xFFFF,
        4 => u32::MAX,
        _ => 0,
    };
    (word >> shift) & mask
}

/// Merge a sub-word write into a naturally-aligned 32-bit word.
///
/// `old` is the current register value. `offset` selects the byte lane
/// (low 2 bits). `size` is 1, 2, or 4. `val` is the value to write
/// (low bits significant).
///
/// Returns the updated 32-bit word.
#[inline]
pub fn merge_subword(old: u32, offset: u64, size: usize, val: u64) -> u32 {
    let shift = ((offset & 0x3) * 8) as u32;
    let mask = match size {
        1 => 0xFF,
        2 => 0xFFFF,
        4 => u32::MAX,
        _ => 0,
    };
    let shifted_mask = mask << shift;
    (old & !shifted_mask) | (((val as u32) & mask) << shift)
}

/// Errors that can occur during device construction or operation.
#[derive(Debug, thiserror::Error)]
pub enum DeviceError {
    /// A required parameter is missing from the provided configuration.
    #[error("missing required parameter: {0}")]
    MissingParam(String),

    /// A parameter has an invalid value.
    #[error("invalid parameter '{param}': {reason}")]
    InvalidParam {
        /// Name of the invalid parameter.
        param: &'static str,
        /// Human-readable reason the value is invalid.
        reason: &'static str,
    },

    /// Device initialization failed for a reason beyond parameter validation
    /// (e.g., OS resource allocation failed).
    #[error("device initialization failed: {0}")]
    InitFailed(String),

    /// A write to a register caused a device-detected protocol error.
    /// Used for bus controllers that validate sequences (e.g., I2C START
    /// without STOP).
    #[error("protocol error in register write at offset {offset:#x}: {reason}")]
    ProtocolError {
        /// Register offset where the error occurred.
        offset: u64,
        /// Human-readable description of the protocol violation.
        reason: String,
    },
}

/// Optional trait for devices that advance from simulated cycle ticks.
pub trait TickableDevice {
    /// Advance the device's internal time by `cycles` simulated cycles.
    fn tick(&mut self, cycles: u64);
}

/// Core MMIO device interface.
///
/// A `Device` receives MMIO reads and writes at byte offsets within its
/// mapped region, receives named signal assertions, and exposes an optional
/// transaction interface for full-system simulation with timing.
///
/// # Design rules
///
/// - **Device knows no base address** -- `MemoryMap` owns placement; the
///   device sees only offsets relative to its own base.
/// - **Device knows no IRQ number** -- `InterruptPin` fires a signal; the
///   platform routes it to the appropriate controller input.
/// - **`read()` takes `&mut self`** -- clear-on-read registers and FIFO drain
///   operations need mutation without interior mutability. The single-threaded
///   hot-loop invariant (design rule 8) makes this safe.
///
/// # Undefined offsets
///
/// Reads to undefined offsets return 0. Writes to undefined offsets are
/// silently ignored. Devices must never panic on arbitrary offset/size
/// combinations.
pub trait Device: Send + Any {
    /// Handle a read of `size` bytes at `offset` within this device's region.
    ///
    /// `offset` is the byte offset from the start of the device's mapped
    /// region, NOT the absolute address in the system address space.
    /// `size` is 1, 2, 4, or 8 bytes.
    ///
    /// Returns the value as a `u64`. For sub-word sizes, only the low bits
    /// are meaningful. Reads to write-only or undefined registers return 0.
    fn read(&mut self, offset: u64, size: usize) -> u64;

    /// Handle a write of `size` bytes of `val` at `offset` within this region.
    ///
    /// `offset` is relative to the device's mapped base, not absolute.
    /// `size` is 1, 2, 4, or 8. For sub-word sizes, only the low bits of
    /// `val` are significant. Writes to read-only or undefined registers are
    /// silently ignored.
    fn write(&mut self, offset: u64, size: usize, val: u64);

    /// Return the size in bytes of this device's MMIO region.
    ///
    /// This value must be constant for the lifetime of the device. The
    /// `MemoryMap` caches it at mapping time; returning a different value
    /// after mapping is undefined behavior.
    fn region_size(&self) -> u64;

    /// Receive a named signal assertion.
    ///
    /// Signals are named strings: `"reset"`, `"clock_enable"`, `"dma_ack"`.
    /// `val` is the signal level: 1 = asserted, 0 = deasserted. Other values
    /// are permitted for multi-level signals but are device-defined.
    ///
    /// The default implementation is a no-op -- devices that do not respond
    /// to any named signal do not need to override this method.
    fn signal(&mut self, _name: &str, _val: u64) {}

    /// Handle a bus transaction with full context (FS mode with timing).
    ///
    /// The default implementation delegates to [`read()`](Device::read) or
    /// [`write()`](Device::write) based on `txn.is_write`. Devices that need
    /// access to bus attributes (security, privilege) or contribute to stall
    /// cycle accounting should override this method.
    fn transact(&mut self, txn: &mut Transaction) -> Result<(), DeviceError> {
        if txn.is_write {
            self.write(txn.offset, txn.size, txn.data_u64());
        } else {
            let val = self.read(txn.offset, txn.size);
            txn.set_data_u64(val);
        }
        Ok(())
    }
}
