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

#![allow(missing_docs)]

pub mod bus;
pub mod framework;

// Re-export key types at crate root for convenience
pub use bus::amba::{AhbBus, ApbBus};
pub use bus::event_bus::{HelmEvent, HelmEventBus};
pub use bus::i2c::{I2cBus, I2cDevice};
pub use bus::mmio::MmioBus;
pub use bus::spi::{SpiBus, SpiDevice};
pub use framework::address_map::{AddressMap, DeviceId};
pub use framework::backend::{BlockBackend, BufferCharBackend, CharBackend, NullCharBackend};
pub use framework::class_descriptor::ClassDescriptor;
pub use framework::device::{extract_subword, merge_subword, Device, DeviceError, TickableDevice};
pub use framework::interrupt::{
    InterruptPin, InterruptSink, MessageInterrupt, MessageInterruptEmitter, MessageInterruptSink,
    WireId,
};
pub use framework::params::{DeviceParams, ParamField, ParamSchema, ParamType, ParamValue};
pub use framework::registry::{DeviceDescriptor, DeviceRegistry, DldError, HostCapability};
pub use framework::sdk::{
    HELM_DEVICES_ABI_VERSION, HELM_DEVICE_ABI_MAJOR, HELM_DEVICE_ABI_MINOR, SDK_VERSION,
    SDK_VERSION_MAJOR, SDK_VERSION_MINOR, SDK_VERSION_PATCH,
};
pub use framework::signal;
pub use framework::transaction::{Transaction, TransactionAttrs};
