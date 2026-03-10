//! BCM2837 Mailbox — BCM2835 ARM Peripherals §1.3.
//!
//! Mailbox interface for ARM↔VideoCore communication. Used for
//! framebuffer setup, clock configuration, power management, etc.

use crate::device::{Device, DeviceEvent};
use crate::region::MemRegion;
use crate::transaction::Transaction;
use helm_core::types::Addr;
use helm_core::HelmResult;
use std::collections::VecDeque;

// Mailbox 0 (ARM reads, VC writes)
const MBOX0_READ: u64 = 0x00;
const MBOX0_STATUS: u64 = 0x18;
// Mailbox 1 (ARM writes, VC reads)
const MBOX1_WRITE: u64 = 0x20;
const MBOX1_STATUS: u64 = 0x38;

const MBOX_FULL: u32 = 0x8000_0000;
const MBOX_EMPTY: u32 = 0x4000_0000;

/// Property tag IDs for common operations.
pub const TAG_GET_BOARD_REV: u32 = 0x0001_0002;
pub const TAG_GET_ARM_MEMORY: u32 = 0x0001_0005;
pub const TAG_GET_VC_MEMORY: u32 = 0x0001_0006;
pub const TAG_GET_CLOCK_RATE: u32 = 0x0003_0002;
pub const TAG_SET_CLOCK_RATE: u32 = 0x0003_8002;
pub const TAG_ALLOCATE_BUFFER: u32 = 0x0004_0001;
pub const TAG_GET_PITCH: u32 = 0x0004_0008;
pub const TAG_SET_PHYS_WH: u32 = 0x0004_8003;
pub const TAG_SET_VIRT_WH: u32 = 0x0004_8004;
pub const TAG_SET_DEPTH: u32 = 0x0004_8005;

pub struct BcmMailbox {
    dev_name: String,
    region: MemRegion,
    /// ARM→VC FIFO (mailbox 1).
    fifo_write: VecDeque<u32>,
    /// VC→ARM FIFO (mailbox 0).
    fifo_read: VecDeque<u32>,
    /// Board revision to report.
    pub board_revision: u32,
    /// ARM memory base and size.
    pub arm_mem_base: u32,
    pub arm_mem_size: u32,
}

impl BcmMailbox {
    pub fn new(name: impl Into<String>) -> Self {
        let n = name.into();
        Self {
            region: MemRegion {
                name: n.clone(),
                base: 0,
                size: 0x1000,
                kind: crate::region::RegionKind::Io,
                priority: 0,
            },
            dev_name: n,
            fifo_write: VecDeque::new(),
            fifo_read: VecDeque::new(),
            board_revision: 0x00A0_2082, // RPi 3B
            arm_mem_base: 0,
            arm_mem_size: 0x3C00_0000, // 960 MB (1GB - 64MB GPU)
        }
    }

    /// RPi 3 Model B configuration.
    pub fn rpi3() -> Self {
        Self::new("mailbox")
    }

    fn handle_read(&mut self, offset: u64) -> u32 {
        match offset {
            MBOX0_READ => self.fifo_read.pop_front().unwrap_or(0),
            MBOX0_STATUS => {
                let mut status = 0u32;
                if self.fifo_read.is_empty() {
                    status |= MBOX_EMPTY;
                }
                status
            }
            MBOX1_STATUS => {
                let mut status = 0u32;
                if self.fifo_write.len() >= 8 {
                    status |= MBOX_FULL;
                }
                status
            }
            _ => 0,
        }
    }

    fn handle_write(&mut self, offset: u64, value: u32) {
        match offset {
            MBOX1_WRITE => {
                self.fifo_write.push_back(value);
                // In a real implementation, we'd process property tags here.
                // For now, queue a simple response.
                let channel = value & 0xF;
                let _data_addr = value & !0xF;
                // Echo back with response bit set
                self.fifo_read.push_back((value & !0xF) | channel);
            }
            _ => {}
        }
    }
}

impl Device for BcmMailbox {
    fn transact(&mut self, txn: &mut Transaction) -> HelmResult<()> {
        if txn.is_write {
            self.handle_write(txn.offset, txn.data_u32());
        } else {
            txn.set_data_u32(self.handle_read(txn.offset));
        }
        txn.stall_cycles += 1;
        Ok(())
    }

    fn regions(&self) -> &[MemRegion] {
        std::slice::from_ref(&self.region)
    }

    fn reset(&mut self) -> HelmResult<()> {
        self.fifo_write.clear();
        self.fifo_read.clear();
        Ok(())
    }

    fn read_fast(&mut self, offset: Addr, _s: usize) -> HelmResult<u64> {
        Ok(self.handle_read(offset) as u64)
    }
    fn write_fast(&mut self, offset: Addr, _s: usize, v: u64) -> HelmResult<()> {
        self.handle_write(offset, v as u32);
        Ok(())
    }

    fn name(&self) -> &str {
        &self.dev_name
    }

    fn tick(&mut self, _cycles: u64) -> HelmResult<Vec<DeviceEvent>> {
        if !self.fifo_read.is_empty() {
            Ok(vec![DeviceEvent::Irq {
                line: 65,
                assert: true,
            }])
        } else {
            Ok(vec![])
        }
    }
}
