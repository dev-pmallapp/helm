//! `helm-devices` -- device modeling infrastructure, versioned Device SDK, and DLD loader.
//!
//! # Module layout
//! - [`framework`] -- core device traits, interrupt model, params, registry, backends
//! - [`bus`]       -- bus abstraction (`Bus` trait, `BusAddress`) and `HelmEventBus`
//!
//! Concrete device implementations live in `hw/` crates (`helm-hw-amba`,
//! `helm-hw-pci`, `helm-hw-virtio`).
//!
//! # Design rules
//! - **Device knows no base address** -- `MemoryMap` / `AddressMap` owns placement.
//! - **Device knows no IRQ number**   -- `InterruptPin` fires a signal; the platform routes it.
//! - `HelmEventBus` is synchronous (see `bus::event_bus`); not checkpointed.

pub mod framework;
pub mod bus;

// Re-export key types at crate root for convenience
pub use framework::device::{Device, DeviceError};
pub use framework::transaction::{Transaction, TransactionAttrs};
pub use framework::interrupt::{InterruptPin, InterruptSink, WireId};
pub use framework::params::{ParamSchema, ParamField, ParamType, ParamValue, DeviceParams};
pub use framework::registry::{DeviceDescriptor, DeviceRegistry, DldError, HostCapability};
pub use framework::signal;
pub use framework::backend::{CharBackend, BlockBackend, NullCharBackend, BufferCharBackend};
pub use framework::address_map::{AddressMap, DeviceId};
pub use framework::sdk::{HELM_DEVICES_ABI_VERSION, SDK_VERSION, SDK_VERSION_MAJOR, SDK_VERSION_MINOR, SDK_VERSION_PATCH};
pub use bus::event_bus::HelmEventBus;

// Conditional re-exports for bus protocols (will be removed when extracted)
#[cfg(feature = "pci")]
pub use bus::pci::{Bdf, PciBus, PciEndpoint};
#[cfg(feature = "pci")]
pub use bus::pci::config::PciConfigSpace;
#[cfg(feature = "virtio")]
pub use bus::virtio::{VirtioBackend, transport::VirtioMmioTransport};
