//! Device modeling framework — traits, types, and infrastructure for device plugins.

pub mod address_map;
pub mod backend;
pub mod class_descriptor;
pub mod device;
pub mod device_ctx;
pub mod interrupt;
pub mod irq_router;
pub mod params;
pub mod port;
pub mod register_bank;
pub mod registry;
pub mod sdk;
pub mod signal;
pub mod transaction;

pub use class_descriptor::ClassDescriptor;
