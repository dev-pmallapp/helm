//! PCI capability structures.
//!
//! Each sub-module implements a concrete [`PciCapability`](super::super::traits::PciCapability)
//! for a specific capability type.  The capability list for a device is
//! built by instantiating the structs exported here and placing them in the
//! `capabilities` field of the [`PciFunction`](super::super::traits::PciFunction).
//!
//! # Capability map
//!
//! | Module  | Cap ID | Ext? | Description                    |
//! |---------|--------|------|--------------------------------|
//! | `pcie`  | 0x10   | No   | PCIe device/link capabilities  |
//! | `msix`  | 0x11   | No   | MSI-X interrupt table          |
//! | `pm`    | 0x01   | No   | Power Management               |
//! | `aer`   | 0x0001 | Yes  | Advanced Error Reporting       |
//! | `acs`   | 0x000D | Yes  | Access Control Services        |

pub mod acs;
pub mod aer;
pub mod msix;
pub mod pcie;
pub mod pm;

pub use acs::AcsCapability;
pub use aer::AerCapability;
pub use msix::{MsixCapability, MsixVector};
pub use pcie::PcieCapability;
pub use pm::PmCapability;
