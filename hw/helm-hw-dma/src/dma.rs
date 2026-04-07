//! DMA engine -- multi-channel MMIO-controlled DMA controller.
//!
//! Models a simplified DMA controller with up to 8 channels. Each channel
//! has source address, destination address, transfer length, and control
//! registers. The controller transfers data tick-by-tick through a
//! [`DmaPort`] callback rather than directly accessing memory, ensuring
//! all transfers flow through the memory subsystem for SMMU/trace
//! visibility.
//!
//! # Per-channel register map (each channel occupies 0x20 bytes)
//!
//! | Offset | Name     | R/W | Description                          |
//! |--------|----------|-----|--------------------------------------|
//! | 0x00   | SRC_ADDR | RW  | Source address (32-bit)               |
//! | 0x04   | DST_ADDR | RW  | Destination address (32-bit)         |
//! | 0x08   | LENGTH   | RW  | Transfer length in bytes             |
//! | 0x0C   | CONTROL  | RW  | bit0=START, bit1=IRQ_EN              |
//! | 0x10   | STATUS   | R   | bit0=BUSY, bit1=DONE, bit2=ERROR     |
//!
//! # Global registers (after channel block)
//!
//! | Offset | Name     | R/W | Description                          |
//! |--------|----------|-----|--------------------------------------|
//! | 0x100  | INT_STATUS| R  | Per-channel interrupt status bits    |
//! | 0x104  | INT_CLR  | W   | Per-channel interrupt clear          |

use helm_core::DmaPort;
use helm_devices::{Device, InterruptPin};

// ── Constants ───────────────────────────────────────────────────────────────

/// Maximum number of DMA channels.
const MAX_CHANNELS: usize = 8;

/// Bytes per channel in the register map.
const CHANNEL_STRIDE: u64 = 0x20;

/// Global register base offset.
const GLOBAL_BASE: u64 = 0x100;

// ── Per-channel register offsets ────────────────────────────────────────────

const CH_SRC_ADDR: u64 = 0x00;
const CH_DST_ADDR: u64 = 0x04;
const CH_LENGTH: u64 = 0x08;
const CH_CONTROL: u64 = 0x0C;
const CH_STATUS: u64 = 0x10;

// ── Control bits ────────────────────────────────────────────────────────────

const CTRL_START: u32 = 1 << 0;
const CTRL_IRQ_EN: u32 = 1 << 1;

// ── Status bits ─────────────────────────────────────────────────────────────

const STATUS_BUSY: u32 = 1 << 0;
const STATUS_DONE: u32 = 1 << 1;
#[allow(dead_code)]
const STATUS_ERROR: u32 = 1 << 2;

// ── Channel state ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default)]
struct DmaChannel {
    src_addr: u32,
    dst_addr: u32,
    length: u32,
    control: u32,
    status: u32,
    /// Bytes remaining to transfer.
    remaining: u32,
}

impl DmaChannel {
    fn is_busy(&self) -> bool {
        self.status & STATUS_BUSY != 0
    }

    fn irq_enabled(&self) -> bool {
        self.control & CTRL_IRQ_EN != 0
    }
}

// ── DmaEngine ───────────────────────────────────────────────────────────────

/// Multi-channel DMA controller.
///
/// Transfers happen incrementally via [`tick()`](DmaEngine::tick). Each
/// tick transfers up to `bytes_per_tick` bytes per active channel.
pub struct DmaEngine {
    channels: [DmaChannel; MAX_CHANNELS],
    /// Per-channel interrupt status (bit N = channel N done).
    int_status: u32,
    /// Bytes transferred per tick per channel.
    pub bytes_per_tick: u32,
    /// Combined interrupt output pin.
    pub irq_out: InterruptPin,
    /// Reusable transfer buffer (avoids per-tick heap allocation).
    transfer_buf: Vec<u8>,
}

impl DmaEngine {
    /// Create a new DMA engine with the given transfer rate.
    pub fn new(bytes_per_tick: u32) -> Self {
        Self {
            channels: Default::default(),
            int_status: 0,
            bytes_per_tick,
            irq_out: InterruptPin::new(),
            transfer_buf: vec![0u8; bytes_per_tick as usize],
        }
    }

    /// Advance all active DMA channels by one tick.
    ///
    /// Each active channel transfers up to `bytes_per_tick` bytes through
    /// the provided [`DmaPort`]. When a channel completes, its DONE bit
    /// is set and an interrupt is raised if enabled.
    pub fn tick(&mut self, port: &dyn DmaPort) {
        for i in 0..MAX_CHANNELS {
            if !self.channels[i].is_busy() {
                continue;
            }

            let ch = &mut self.channels[i];
            let transfer = ch.remaining.min(self.bytes_per_tick);

            if transfer > 0 {
                self.transfer_buf.resize(transfer as usize, 0);
                let src = ch.src_addr as u64 + (ch.length - ch.remaining) as u64;
                let dst = ch.dst_addr as u64 + (ch.length - ch.remaining) as u64;

                let Ok(()) = port.dma_read(src, &mut self.transfer_buf) else {
                    ch.status = STATUS_ERROR;
                    continue;
                };
                let Ok(()) = port.dma_write(dst, &self.transfer_buf) else {
                    ch.status = STATUS_ERROR;
                    continue;
                };

                ch.remaining -= transfer;
            }

            if ch.remaining == 0 {
                ch.status = STATUS_DONE;
                if ch.irq_enabled() {
                    self.int_status |= 1 << i;
                    self.irq_out.assert();
                }
            }
        }
    }
}

impl Device for DmaEngine {
    fn read(&mut self, offset: u64, _size: usize) -> u64 {
        // Global registers
        if offset >= GLOBAL_BASE {
            return match offset - GLOBAL_BASE {
                0x00 => self.int_status as u64,
                _ => 0,
            };
        }

        // Per-channel registers
        let ch_idx = (offset / CHANNEL_STRIDE) as usize;
        let reg_offset = offset % CHANNEL_STRIDE;

        if ch_idx >= MAX_CHANNELS {
            return 0;
        }

        let ch = &self.channels[ch_idx];
        let val = match reg_offset {
            CH_SRC_ADDR => ch.src_addr,
            CH_DST_ADDR => ch.dst_addr,
            CH_LENGTH => ch.length,
            CH_CONTROL => ch.control,
            CH_STATUS => ch.status,
            _ => 0,
        };
        val as u64
    }

    fn write(&mut self, offset: u64, _size: usize, val: u64) {
        let val32 = val as u32;

        // Global registers
        if offset >= GLOBAL_BASE {
            if offset - GLOBAL_BASE == 0x04 {
                // INT_CLR: write-1-to-clear
                self.int_status &= !val32;
                if self.int_status == 0 {
                    self.irq_out.deassert();
                }
            }
            return;
        }

        // Per-channel registers
        let ch_idx = (offset / CHANNEL_STRIDE) as usize;
        let reg_offset = offset % CHANNEL_STRIDE;

        if ch_idx >= MAX_CHANNELS {
            return;
        }

        let ch = &mut self.channels[ch_idx];
        match reg_offset {
            CH_SRC_ADDR => ch.src_addr = val32,
            CH_DST_ADDR => ch.dst_addr = val32,
            CH_LENGTH => ch.length = val32,
            CH_CONTROL => {
                ch.control = val32;
                if val32 & CTRL_START != 0 && !ch.is_busy() {
                    // Start transfer
                    ch.remaining = ch.length;
                    ch.status = STATUS_BUSY;
                }
            }
            // STATUS is read-only
            _ => {}
        }
    }

    fn region_size(&self) -> u64 {
        0x200 // 8 channels * 0x20 + global registers
    }
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::Mutex;

    use helm_core::MemFault;

    /// Test DMA port backed by a flat byte array.
    struct TestMemory {
        data: Mutex<Vec<u8>>,
    }

    impl TestMemory {
        fn new(size: usize) -> Self {
            Self {
                data: Mutex::new(vec![0; size]),
            }
        }

        fn write_seed(&self, addr: usize, value: u8) {
            self.data.lock().unwrap()[addr] = value;
        }

        fn snapshot(&self, start: usize, end: usize) -> Vec<u8> {
            self.data.lock().unwrap()[start..end].to_vec()
        }
    }

    impl DmaPort for TestMemory {
        fn dma_read(&self, addr: u64, buf: &mut [u8]) -> Result<(), MemFault> {
            let start = addr as usize;
            let end = start + buf.len();
            let data = self.data.lock().unwrap();
            if end <= data.len() {
                buf.copy_from_slice(&data[start..end]);
                Ok(())
            } else {
                Err(MemFault::AccessFault { addr })
            }
        }

        fn dma_write(&self, addr: u64, buf: &[u8]) -> Result<(), MemFault> {
            let start = addr as usize;
            let end = start + buf.len();
            let mut data = self.data.lock().unwrap();
            if end <= data.len() {
                data[start..end].copy_from_slice(buf);
                Ok(())
            } else {
                Err(MemFault::AccessFault { addr })
            }
        }
    }

    #[test]
    fn dma_basic_transfer() {
        let mut dma = DmaEngine::new(64);
        let mem = TestMemory::new(1024);

        // Write source data
        for i in 0..16u8 {
            mem.write_seed(i as usize, i);
        }

        // Configure channel 0: copy 16 bytes from addr 0 to addr 256
        dma.write(CH_SRC_ADDR, 4, 0);
        dma.write(CH_DST_ADDR, 4, 256);
        dma.write(CH_LENGTH, 4, 16);
        dma.write(CH_CONTROL, 4, (CTRL_START | CTRL_IRQ_EN) as u64);

        // Should be busy
        assert_eq!(dma.read(CH_STATUS, 4) as u32, STATUS_BUSY);

        // Tick to complete
        dma.tick(&mem);

        // Should be done
        assert_eq!(dma.read(CH_STATUS, 4) as u32, STATUS_DONE);

        // Verify data copied
        assert_eq!(mem.snapshot(256, 272), mem.snapshot(0, 16));
    }

    #[test]
    fn dma_multi_tick_transfer() {
        let mut dma = DmaEngine::new(4); // 4 bytes per tick
        let mem = TestMemory::new(256);

        for i in 0..16u8 {
            mem.write_seed(i as usize, 0xAA);
        }

        dma.write(CH_SRC_ADDR, 4, 0);
        dma.write(CH_DST_ADDR, 4, 128);
        dma.write(CH_LENGTH, 4, 16);
        dma.write(CH_CONTROL, 4, CTRL_START as u64);

        // 4 bytes per tick, 16 bytes total = 4 ticks
        dma.tick(&mem);
        assert_eq!(dma.read(CH_STATUS, 4) as u32, STATUS_BUSY);

        dma.tick(&mem);
        assert_eq!(dma.read(CH_STATUS, 4) as u32, STATUS_BUSY);

        dma.tick(&mem);
        assert_eq!(dma.read(CH_STATUS, 4) as u32, STATUS_BUSY);

        dma.tick(&mem);
        assert_eq!(dma.read(CH_STATUS, 4) as u32, STATUS_DONE);

        assert_eq!(mem.snapshot(128, 144), vec![0xAA; 16]);
    }

    #[test]
    fn dma_interrupt_on_completion() {
        let mut dma = DmaEngine::new(1024);
        let mem = TestMemory::new(256);

        dma.write(CH_SRC_ADDR, 4, 0);
        dma.write(CH_DST_ADDR, 4, 128);
        dma.write(CH_LENGTH, 4, 8);
        dma.write(CH_CONTROL, 4, (CTRL_START | CTRL_IRQ_EN) as u64);

        dma.tick(&mem);

        // Interrupt status should be set for channel 0
        let int_status = dma.read(GLOBAL_BASE, 4);
        assert_ne!(int_status & 1, 0);

        // Clear interrupt
        dma.write(GLOBAL_BASE + 0x04, 4, 1);
        assert_eq!(dma.read(GLOBAL_BASE, 4), 0);
    }

    #[test]
    fn dma_no_interrupt_when_disabled() {
        let mut dma = DmaEngine::new(1024);
        let mem = TestMemory::new(256);

        dma.write(CH_SRC_ADDR, 4, 0);
        dma.write(CH_DST_ADDR, 4, 128);
        dma.write(CH_LENGTH, 4, 8);
        dma.write(CH_CONTROL, 4, CTRL_START as u64); // No IRQ_EN

        dma.tick(&mem);

        // Should be done but no interrupt
        assert_eq!(dma.read(CH_STATUS, 4) as u32, STATUS_DONE);
        assert_eq!(dma.read(GLOBAL_BASE, 4), 0);
    }

    #[test]
    fn dma_region_size() {
        let dma = DmaEngine::new(64);
        assert_eq!(dma.region_size(), 0x200);
    }

    #[test]
    fn dma_fault_sets_error_status() {
        let mut dma = DmaEngine::new(16);
        let mem = TestMemory::new(32);

        dma.write(CH_SRC_ADDR, 4, 0);
        dma.write(CH_DST_ADDR, 4, 64);
        dma.write(CH_LENGTH, 4, 8);
        dma.write(CH_CONTROL, 4, CTRL_START as u64);

        dma.tick(&mem);

        assert_eq!(dma.read(CH_STATUS, 4) as u32, STATUS_ERROR);
        assert_eq!(dma.read(GLOBAL_BASE, 4), 0);
    }
}
