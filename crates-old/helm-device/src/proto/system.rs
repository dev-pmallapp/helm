//! System bus — zero-latency, full address space.

use crate::bus::DeviceBus;

/// System bus: 0 crossing latency, full 64-bit address space.
///
/// This is the top-level bus in any platform. All other buses attach to it.
pub type SystemBus = DeviceBus;

/// Create a system bus.
pub fn system_bus() -> DeviceBus {
    DeviceBus::system()
}
