//! # helm-device
//!
//! Framework for building simulated devices.  Developers implement
//! [`Device`] (or the legacy [`MemoryMappedDevice`]) to create custom
//! peripherals (UARTs, timers, GPUs, etc.) that plug into HELM's
//! address space and interrupt system.
//!
//! ## Key abstractions
//!
//! - [`Transaction`] — unified read/write bus transaction
//! - [`Device`](device::Device) — enhanced device trait with lifecycle
//! - [`DeviceBus`] — hierarchical bus with bridge latency
//! - [`MemRegion`] / [`MemRegionTree`] — QEMU-style address space management
//! - [`IrqRouter`] — routable IRQ delivery to interrupt controllers
//! - [`DmaEngine`] — scatter-gather DMA with bus-beat fragmentation
//! - [`proto`] — bus protocol implementations (PCI, I2C, SPI, USB, AXI)
//! - [`DeviceScheduler`] — cooperative multi-clock scheduling for FS mode

pub mod address_map;
pub mod arm;
pub mod backend;
pub mod bus;
pub mod connection;
pub mod coop_scheduler;
pub mod device;
pub mod device_ctx;
pub mod dma;
pub mod fdt;
pub mod irq;
pub mod irq_wire;
pub mod loader;
pub mod mmio;
pub mod pci;
pub mod platform;
pub mod platform_v2;
pub mod proto;
pub mod region;
pub mod scheduler;
pub mod transaction;
pub mod virtio;

// ── Primary exports ─────────────────────────────────────────────────────────

pub use bus::{DeviceBus, DeviceSlot};
pub use device::{Device, DeviceEvent, DeviceId, LegacyWrapper, LogLevel};
pub use dma::{DmaChannel, DmaDirection, DmaEngine, DmaStatus};
pub use irq::{InterruptController, IrqController, IrqLine, IrqRoute, IrqRouter, IrqState};
pub use mmio::{DeviceAccess, MemoryMappedDevice};
pub use region::{FlatEntry, MemRegion, MemRegionTree, RegionKind};
pub use scheduler::{DeviceScheduler, DeviceThread, TickableDevice};
pub use transaction::{Transaction, TransactionAttrs};

pub use backend::{
    BlockBackend, BufferCharBackend, BufferNetBackend, CharBackend, MemoryBlockBackend, NetBackend,
    NullCharBackend, NullNetBackend, StdioCharBackend,
};
pub use fdt::{
    generate_virt_dtb, parse_dtb, parse_ram_size, patch_dtb, resolve_dtb, DeviceSpec, DtbConfig,
    DtbPolicy, FdtBuilder, FdtDescriptor, FdtNode, FdtValue, InferCtx, ResolvedDtb, RuntimeDtb,
};
pub use platform::{arm_virt_platform, realview_pb_platform, rpi3_platform, Platform};
pub use platform_v2::PlatformV2;
pub use proto::amba::{AhbBus, ApbBus};

pub use address_map::{AddressMap, AddressMapListener, FlatViewEntry, RegionHandle};
pub use connection::{Connection, ConnectionError, DeviceInterface};
pub use device_ctx::DeviceCtx;
pub use irq_wire::{IrqSink, IrqWire};
pub use coop_scheduler::{CoopScheduler, DeviceClock};
pub use loader::{DeviceConfig, PropertySpec, PropertyType};

#[cfg(test)]
mod tests;
