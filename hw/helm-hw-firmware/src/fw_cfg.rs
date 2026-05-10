//! Minimal `fw_cfg` MMIO device for `arm-virt`.

use std::collections::BTreeMap;

use helm_devices::Device;

const DATA_REG: u64 = 0x00;
const SELECTOR_REG: u64 = 0x08;
const DMA_REG: u64 = 0x10;
const DMA_REG_END: u64 = DMA_REG + 7;
const REGION_SIZE: u64 = 0x18;

/// QEMU-compatible selector for the signature entry.
pub const FW_CFG_SIGNATURE: u16 = 0x0000;
/// QEMU-compatible selector for the interface ID entry.
pub const FW_CFG_ID: u16 = 0x0001;
/// QEMU-compatible selector for the CPU-count entry.
pub const FW_CFG_NB_CPUS: u16 = 0x0005;

/// Minimal MMIO `fw_cfg` device.
pub struct FwCfgMmio {
    selector: u16,
    cursor: usize,
    entries: BTreeMap<u16, Vec<u8>>,
    /// Aggregate counters (data_reads, selector_writes,
    /// dma_writes). `Clone`-cheap PerfCounter handles, ZST when
    /// `helm-stats/stats` is off.
    pub stats: helm_stats::FwCfgStats,
}

impl FwCfgMmio {
    /// Create a new `fw_cfg` device with the standard minimal entry set.
    pub fn new(num_cpus: usize) -> Self {
        let mut entries = BTreeMap::new();
        entries.insert(FW_CFG_SIGNATURE, b"QEMU".to_vec());
        entries.insert(FW_CFG_ID, 1u32.to_be_bytes().to_vec());
        entries.insert(FW_CFG_NB_CPUS, (num_cpus as u32).to_be_bytes().to_vec());
        Self {
            selector: FW_CFG_SIGNATURE,
            cursor: 0,
            entries,
            stats: helm_stats::FwCfgStats::new(),
        }
    }

    fn selected_entry(&self) -> Option<&[u8]> {
        self.entries.get(&self.selector).map(Vec::as_slice)
    }

    fn read_data(&mut self, size: usize) -> u64 {
        let mut value = 0u64;
        for _ in 0..size {
            let byte = self
                .selected_entry()
                .and_then(|entry| entry.get(self.cursor).copied())
                .unwrap_or(0);
            self.cursor = self.cursor.saturating_add(1);
            value = (value << 8) | u64::from(byte);
        }
        value
    }
}

impl Device for FwCfgMmio {
    fn read(&mut self, offset: u64, size: usize) -> u64 {
        match offset {
            DATA_REG => {
                self.stats.data_reads.inc();
                self.read_data(size)
            }
            SELECTOR_REG => u64::from(self.selector),
            DMA_REG..=DMA_REG_END => 0,
            _ => 0,
        }
    }

    fn write(&mut self, offset: u64, _size: usize, val: u64) {
        match offset {
            SELECTOR_REG => {
                self.stats.selector_writes.inc();
                self.selector = val as u16;
                self.cursor = 0;
            }
            DATA_REG | DMA_REG..=DMA_REG_END => {
                if (DMA_REG..=DMA_REG_END).contains(&offset) {
                    self.stats.dma_writes.inc();
                }
            }
            _ => {}
        }
    }

    fn region_size(&self) -> u64 {
        REGION_SIZE
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signature_stream_reads_qemu_magic() {
        let mut fw_cfg = FwCfgMmio::new(4);
        fw_cfg.write(SELECTOR_REG, 2, u64::from(FW_CFG_SIGNATURE));
        assert_eq!(fw_cfg.read(DATA_REG, 1), u64::from(b'Q'));
        assert_eq!(fw_cfg.read(DATA_REG, 1), u64::from(b'E'));
        assert_eq!(fw_cfg.read(DATA_REG, 2), u64::from(0x4D55u16));
    }

    #[test]
    fn cpu_count_entry_resets_cursor_on_selector_write() {
        let mut fw_cfg = FwCfgMmio::new(8);
        fw_cfg.write(SELECTOR_REG, 2, u64::from(FW_CFG_NB_CPUS));
        assert_eq!(fw_cfg.read(DATA_REG, 4), 8);
        fw_cfg.write(SELECTOR_REG, 2, u64::from(FW_CFG_NB_CPUS));
        assert_eq!(fw_cfg.read(DATA_REG, 4), 8);
    }
}
