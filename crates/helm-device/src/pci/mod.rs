//! PCI device foundation — types, traits, addressing, and config space.
//!
//! This module provides the building blocks for simulating PCI endpoint
//! devices:
//!
//! - [`BarDecl`] — static BAR layout declaration (32-bit MMIO, 64-bit MMIO, I/O)
//! - [`PciCapability`] — trait for individual capability structures
//! - [`PciFunction`] — trait for a complete PCI endpoint function
//! - [`Bdf`] — Bus:Device.Function address with ECAM encoding/decoding
//! - [`PciConfigSpace`] — type-0 config space engine with BAR sizing protocol
//!
//! # Architecture
//!
//! ```text
//! PciFunction (your device impl)
//!   ├── bars()            → [BarDecl; 6]
//!   ├── capabilities()    → [Box<dyn PciCapability>]
//!   └── bar_read/write()  → device memory access
//!
//! PciConfigSpace (engine)
//!   ├── new(vendor, device, class, rev, bars, caps)
//!   ├── read(offset)  → u32   (with BAR sizing protocol)
//!   └── write(offset, value)  (mask-enforced)
//!
//! Bdf (bus address)
//!   ├── from_ecam_offset(u64) → (Bdf, reg_offset)
//!   └── ecam_offset(reg)      → u64
//! ```

pub mod bdf;
pub mod bus;
pub mod capability;
pub mod config;
pub mod host;
pub mod traits;

// ── Re-exports ───────────────────────────────────────────────────────────────

pub use bdf::Bdf;
pub use bus::PciBus;
pub use config::PciConfigSpace;
pub use host::PciHostBridge;
pub use traits::{BarDecl, PciCapability, PciFunction};
