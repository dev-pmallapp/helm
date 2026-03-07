//! VirtIO device framework — spec 1.4 compliant.
//!
//! Provides the complete VirtIO stack for simulation:
//! - **Transport**: MMIO register interface (spec 4.2)
//! - **Queues**: Split and packed virtqueue layouts (spec 2.7–2.8)
//! - **Features**: All feature bit definitions
//! - **Devices**: All device types defined in the VirtIO 1.4 spec
//!
//! # Architecture
//!
//! Each device type implements [`VirtioDeviceBackend`], which handles
//! device-specific config space and virtqueue processing. The
//! [`VirtioMmioTransport`] wraps any backend and provides the standard
//! MMIO register interface. The transport implements the [`Device`](crate::device::Device)
//! trait, so it can be attached to any `DeviceBus`.
//!
//! ```text
//! DeviceBus
//!   └── VirtioMmioTransport (MMIO registers @ 0x100)
//!         └── VirtioBlk (backend)
//!               └── queues: [requestq]
//! ```

pub mod features;
pub mod queue;
pub mod transport;

// Device types
pub mod balloon;
pub mod blk;
pub mod bt;
pub mod can;
pub mod console;
pub mod crypto;
pub mod fs;
pub mod gpio;
pub mod gpu;
pub mod i2c;
pub mod input;
pub mod iommu;
pub mod mem;
pub mod net;
pub mod pmem;
pub mod rng;
pub mod scmi;
pub mod scsi;
pub mod sound;
pub mod video;
pub mod vsock;
pub mod watchdog;

// Re-exports
pub use features::*;
pub use queue::{PackedVirtqueue, SplitVirtqueue, Virtqueue, VringDesc, VringUsedElem};
pub use transport::{VirtioDeviceBackend, VirtioMmioTransport};

pub use balloon::VirtioBalloon;
pub use blk::VirtioBlk;
pub use bt::VirtioBt;
pub use can::VirtioCan;
pub use console::VirtioConsole;
pub use crypto::VirtioCrypto;
pub use fs::VirtioFs;
pub use gpio::VirtioGpio;
pub use gpu::VirtioGpu;
pub use i2c::VirtioI2c;
pub use input::VirtioInput;
pub use iommu::VirtioIommu;
pub use mem::VirtioMem;
pub use net::VirtioNet;
pub use pmem::VirtioPmem;
pub use rng::VirtioRng;
pub use scmi::VirtioScmi;
pub use scsi::VirtioScsi;
pub use sound::VirtioSound;
pub use video::{VirtioVideoDecoder, VirtioVideoEncoder};
pub use vsock::VirtioVsock;
pub use watchdog::VirtioWatchdog;
