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

pub mod arm;
pub mod backend;
pub mod bus;
pub mod device;
pub mod dma;
pub mod irq;
pub mod loader;
pub mod mmio;
pub mod platform;
pub mod proto;
pub mod region;
pub mod scheduler;
pub mod transaction;
pub mod virtio;

// ── Primary exports ─────────────────────────────────────────────────────────

pub use bus::{DeviceBus, DeviceSlot};
pub use device::{Device, DeviceEvent, DeviceId, LegacyWrapper, LogLevel};
pub use dma::{DmaChannel, DmaDirection, DmaEngine, DmaStatus};
pub use irq::{
    InterruptController, IrqController, IrqLine, IrqRoute, IrqRouter, IrqState,
};
pub use mmio::{DeviceAccess, MemoryMappedDevice};
pub use region::{FlatEntry, MemRegion, MemRegionTree, RegionKind};
pub use scheduler::{DeviceScheduler, DeviceThread, TickableDevice};
pub use transaction::{Transaction, TransactionAttrs};

pub use backend::{
    BlockBackend, BufferCharBackend, BufferNetBackend, CharBackend, MemoryBlockBackend,
    NetBackend, NullCharBackend, NullNetBackend, StdioCharBackend,
};
pub use platform::{Platform, arm_virt_platform, realview_pb_platform, rpi3_platform};
pub use proto::amba::{AhbBus, ApbBus};

#[cfg(test)]
mod tests;
