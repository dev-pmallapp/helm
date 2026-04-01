//! Device SDK version constants and prelude re-exports.
//!
//! These constants are used for ABI compatibility checks when loading
//! DLD (Dynamic Loadable Device) plugins. The ABI version is the sole
//! gate for binary compatibility between independently compiled devices.

/// Semantic version of the Device SDK (string form).
pub const SDK_VERSION: &str = "1.0.0";

/// SDK semantic version -- major component.
pub const SDK_VERSION_MAJOR: u32 = 1;

/// SDK semantic version -- minor component.
pub const SDK_VERSION_MINOR: u32 = 0;

/// SDK semantic version -- patch component.
pub const SDK_VERSION_PATCH: u32 = 0;

/// ABI major version — incremented on breaking changes.
///
/// Bump when:
/// - `Device` trait method signature changes
/// - `DeviceDescriptor` struct layout changes
/// - `Transaction` struct layout changes
/// - `DeviceRegistry::register()` ABI changes
pub const HELM_DEVICE_ABI_MAJOR: u32 = 1;

/// ABI minor version — incremented on backward-compatible additions.
///
/// Do NOT bump when:
/// - New optional `Device` methods with default impls are added
/// - New `DldError` variants are added
/// - Built-in device implementations change
pub const HELM_DEVICE_ABI_MINOR: u32 = 0;

/// Combined ABI version for backward compat (major * 1000 + minor).
pub const HELM_DEVICES_ABI_VERSION: u32 = HELM_DEVICE_ABI_MAJOR * 1000 + HELM_DEVICE_ABI_MINOR;

/// Convenience re-exports for out-of-tree DLD authors.
///
/// Usage:
/// ```ignore
/// use helm_devices::prelude::*;
/// ```
///
/// This brings in every SDK type needed to write a DLD.
/// It does NOT bring in bus protocols, built-in devices, or platform types.
pub mod prelude {
    // Core device trait and errors.
    pub use super::super::device::{Device, DeviceError};

    // Transaction types.
    pub use super::super::transaction::{Transaction, TransactionAttrs};

    // Interrupt model.
    pub use super::super::interrupt::{InterruptPin, InterruptSink, WireId};

    // Parameters.
    pub use super::super::params::{DeviceParams, ParamField, ParamSchema, ParamType, ParamValue};

    // Registry.
    pub use super::super::registry::{DeviceDescriptor, DeviceRegistry, DldError, HostCapability};

    // Backends.
    pub use super::super::backend::{BlockBackend, CharBackend};

    // Signal constants.
    pub use super::super::signal::{SIGNAL_CLOCK_ENABLE, SIGNAL_DMA_ACK, SIGNAL_NMI, SIGNAL_RESET};

    // Version constants.
    pub use super::{
        HELM_DEVICES_ABI_VERSION, HELM_DEVICE_ABI_MAJOR, HELM_DEVICE_ABI_MINOR,
        SDK_VERSION, SDK_VERSION_MAJOR, SDK_VERSION_MINOR, SDK_VERSION_PATCH,
    };
}
