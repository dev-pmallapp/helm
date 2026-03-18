//! Device modeling framework — traits, types, and infrastructure for device plugins.

pub mod device;
pub mod transaction;
pub mod interrupt;
pub mod irq_router;
pub mod signal;
pub mod port;
pub mod params;
pub mod registry;
pub mod device_ctx;
pub mod address_map;
pub mod backend;
pub mod sdk;
