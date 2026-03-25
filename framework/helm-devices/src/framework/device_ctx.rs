//! Device context -- minimal runtime identity provided during lifecycle calls.
//!
//! `DeviceContext` is passed to devices during realize/unrealize to provide a
//! stable identity without requiring the full `AddressMap` and `IrqRouter`
//! infrastructure (which live in separate modules and are wired later).

/// Minimal device context for lifecycle operations.
///
/// In the current implementation, this carries only a `device_id`. The full
/// context (with address map access, event scheduling, etc.) will be built
/// out when the engine integrates timing and full-system simulation.
pub struct DeviceContext {
    /// Unique identifier for this device instance within the simulation.
    ///
    /// Assigned by the platform during device registration and remains
    /// stable for the lifetime of the device.
    pub device_id: u64,
}

impl DeviceContext {
    /// Create a new device context with the given device ID.
    pub fn new(device_id: u64) -> Self {
        Self { device_id }
    }
}
