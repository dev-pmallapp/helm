//! LLVM-IR accelerator as a new-style [`Device`].
//!
//! This is the gem5-SALAM-inspired accelerator device from the IO redesign
//! doc (section 3.9). It implements the full Device trait with
//! `realize`/`unrealize` lifecycle, `IrqWire` for completion interrupts,
//! and `clock_hz()` for cooperative scheduler integration.
//!
//! The existing [`AcceleratorDevice`](crate::device_bridge::AcceleratorDevice)
//! (MemoryMappedDevice) remains for backward compatibility.

use crate::accelerator::{Accelerator, AcceleratorBuilder, AcceleratorConfig};
use helm_core::types::Addr;
use helm_core::HelmResult;
use helm_device::device::{Device, DeviceEvent};
use helm_device::irq_wire::IrqWire;
use helm_device::region::{MemRegion, RegionKind};
use helm_device::transaction::Transaction;

/// MMIO register offsets for the accelerator.
const REG_STATUS: Addr = 0x00;
const REG_CONTROL: Addr = 0x04;
const REG_CYCLES: Addr = 0x08;
const REG_LOADS: Addr = 0x10;
const REG_STORES: Addr = 0x18;

/// Accelerator execution status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccelStatus {
    /// Idle and ready.
    Idle = 0,
    /// Currently executing.
    Running = 1,
    /// Completed successfully.
    Complete = 2,
    /// Build or runtime error.
    Error = 3,
}

/// LLVM-IR hardware accelerator exposed as a Device.
///
/// Implements the full lifecycle: construct → realize → run → unrealize.
/// Hot-pluggable — can be attached/detached at runtime.
///
/// # MMIO Register Map
///
/// | Offset | Name    | R/W | Description |
/// |--------|---------|-----|-------------|
/// | 0x00   | STATUS  | R   | AccelStatus value |
/// | 0x04   | CONTROL | W   | 1=start |
/// | 0x08   | CYCLES  | R   | Total cycles elapsed |
/// | 0x10   | LOADS   | R   | Total memory loads |
/// | 0x18   | STORES  | R   | Total memory stores |
pub struct LlvmAcceleratorDevice {
    name: String,
    accel: Option<Accelerator>,
    #[allow(dead_code)]
    config: AcceleratorConfig,
    status: AccelStatus,

    /// Cached stats from last run.
    total_cycles: u64,
    memory_loads: u64,
    memory_stores: u64,

    /// MMIO region descriptor.
    region: MemRegion,

    /// Completion interrupt wire.
    pub irq: IrqWire,

    /// Clock frequency (Hz). 0 = untimed (runs to completion synchronously).
    clock: u64,
}

impl LlvmAcceleratorDevice {
    /// Create from an LLVM IR string.
    pub fn from_string(name: impl Into<String>, ir: &str) -> Self {
        let n = name.into();
        let config = AcceleratorConfig::default();
        let accel = AcceleratorBuilder::new()
            .with_ir_string(ir)
            .build()
            .ok();
        let status = if accel.is_some() {
            AccelStatus::Idle
        } else {
            AccelStatus::Error
        };

        Self {
            region: MemRegion {
                name: n.clone(),
                base: 0,
                size: 0x100,
                kind: RegionKind::Io,
                priority: 0,
            },
            name: n,
            accel,
            config,
            status,
            total_cycles: 0,
            memory_loads: 0,
            memory_stores: 0,
            irq: IrqWire::new(0),
            clock: 0,
        }
    }

    /// Create from an LLVM IR file.
    pub fn from_file(name: impl Into<String>, path: &str) -> Self {
        let n = name.into();
        let config = AcceleratorConfig::default();
        let accel = AcceleratorBuilder::new()
            .with_ir_file(path)
            .build()
            .ok();
        let status = if accel.is_some() {
            AccelStatus::Idle
        } else {
            AccelStatus::Error
        };

        Self {
            region: MemRegion {
                name: n.clone(),
                base: 0,
                size: 0x100,
                kind: RegionKind::Io,
                priority: 0,
            },
            name: n,
            accel,
            config,
            status,
            total_cycles: 0,
            memory_loads: 0,
            memory_stores: 0,
            irq: IrqWire::new(0),
            clock: 0,
        }
    }

    /// Set the clock frequency for cooperative scheduling.
    pub fn with_clock_hz(mut self, hz: u64) -> Self {
        self.clock = hz;
        self
    }

    /// Set the IRQ line number for the completion interrupt.
    pub fn with_irq_line(mut self, line: u32) -> Self {
        self.irq = IrqWire::new(line);
        self
    }

    /// Current status.
    pub fn status(&self) -> AccelStatus {
        self.status
    }

    /// Run the accelerator to completion.
    fn run_accelerator(&mut self) {
        let Some(accel) = self.accel.as_mut() else {
            return;
        };

        self.status = AccelStatus::Running;
        match accel.run() {
            Ok(()) => {
                let stats = accel.stats();
                self.total_cycles = stats.total_cycles;
                self.memory_loads = stats.memory_loads;
                self.memory_stores = stats.memory_stores;
                self.status = AccelStatus::Complete;
                // Fire completion interrupt
                self.irq.set_level(true);
            }
            Err(_) => {
                self.status = AccelStatus::Error;
            }
        }
    }
}

impl Device for LlvmAcceleratorDevice {
    fn transact(&mut self, txn: &mut Transaction) -> HelmResult<()> {
        if txn.is_write {
            match txn.offset {
                REG_CONTROL if txn.data_u64() == 1 => {
                    self.run_accelerator();
                    txn.stall_cycles += self.total_cycles;
                }
                _ => {}
            }
        } else {
            let val = match txn.offset {
                REG_STATUS => self.status as u64,
                REG_CYCLES => self.total_cycles,
                REG_LOADS => self.memory_loads,
                REG_STORES => self.memory_stores,
                _ => 0,
            };
            txn.set_data_u64(val);
            txn.stall_cycles += 1;
        }
        Ok(())
    }

    fn regions(&self) -> &[MemRegion] {
        std::slice::from_ref(&self.region)
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn reset(&mut self) -> HelmResult<()> {
        if self.accel.is_some() {
            self.status = AccelStatus::Idle;
        }
        self.total_cycles = 0;
        self.memory_loads = 0;
        self.memory_stores = 0;
        self.irq.set_level(false);
        Ok(())
    }

    fn is_hotpluggable(&self) -> bool {
        true
    }

    fn clock_hz(&self) -> u64 {
        self.clock
    }

    fn tick(&mut self, _cycles: u64) -> HelmResult<Vec<DeviceEvent>> {
        // Synchronous model: accelerator runs to completion on start.
        // For cycle-accurate ticking, a future CDFG-based engine would
        // advance the three-queue scheduler here.
        Ok(vec![])
    }
}
