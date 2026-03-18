//! GICv2 interrupt controller -- distributor and CPU interface.

pub mod distributor;
pub mod cpu_interface;

pub use distributor::Gicv2Distributor;
pub use cpu_interface::Gicv2CpuInterface;
