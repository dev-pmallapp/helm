//! VirtIO block device (type 2) — spec 5.2.
//!
//! Provides a virtual block device backed by an in-memory buffer or
//! file. Supports read, write, flush, get-ID, and discard operations.

use crate::virtio::features::*;
use crate::virtio::queue::Virtqueue;
use crate::virtio::transport::VirtioDeviceBackend;
use serde::{Deserialize, Serialize};

// ── Block request types (spec 5.2.6) ───────────────────────────────────────

pub const VIRTIO_BLK_T_IN: u32 = 0;
pub const VIRTIO_BLK_T_OUT: u32 = 1;
pub const VIRTIO_BLK_T_FLUSH: u32 = 4;
pub const VIRTIO_BLK_T_GET_ID: u32 = 8;
pub const VIRTIO_BLK_T_DISCARD: u32 = 11;
pub const VIRTIO_BLK_T_WRITE_ZEROES: u32 = 13;

pub const VIRTIO_BLK_S_OK: u8 = 0;
pub const VIRTIO_BLK_S_IOERR: u8 = 1;
pub const VIRTIO_BLK_S_UNSUPP: u8 = 2;

/// Config space layout (spec 5.2.4).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[repr(C)]
pub struct VirtioBlkConfig {
    /// Capacity in 512-byte sectors.
    pub capacity: u64,
    pub size_max: u32,
    pub seg_max: u32,
    pub geometry_cylinders: u16,
    pub geometry_heads: u8,
    pub geometry_sectors: u8,
    pub blk_size: u32,
    pub topology_physical_block_exp: u8,
    pub topology_alignment_offset: u8,
    pub topology_min_io_size: u16,
    pub topology_opt_io_size: u32,
    pub writeback: u8,
    pub _unused0: u8,
    pub num_queues: u16,
    pub max_discard_sectors: u32,
    pub max_discard_seg: u32,
    pub discard_sector_alignment: u32,
    pub max_write_zeroes_sectors: u32,
    pub max_write_zeroes_seg: u32,
    pub write_zeroes_may_unmap: u8,
    pub _unused1: [u8; 3],
    pub max_secure_erase_sectors: u32,
    pub max_secure_erase_seg: u32,
    pub secure_erase_sector_alignment: u32,
    pub zoned_characteristics: u32,
}

impl Default for VirtioBlkConfig {
    fn default() -> Self {
        Self {
            capacity: 0,
            size_max: 0,
            seg_max: 128,
            geometry_cylinders: 0,
            geometry_heads: 0,
            geometry_sectors: 0,
            blk_size: 512,
            topology_physical_block_exp: 0,
            topology_alignment_offset: 0,
            topology_min_io_size: 1,
            topology_opt_io_size: 1,
            writeback: 0,
            _unused0: 0,
            num_queues: 1,
            max_discard_sectors: 0,
            max_discard_seg: 0,
            discard_sector_alignment: 0,
            max_write_zeroes_sectors: 0,
            max_write_zeroes_seg: 0,
            write_zeroes_may_unmap: 0,
            _unused1: [0; 3],
            max_secure_erase_sectors: 0,
            max_secure_erase_seg: 0,
            secure_erase_sector_alignment: 0,
            zoned_characteristics: 0,
        }
    }
}

/// VirtIO block device.
pub struct VirtioBlk {
    config: VirtioBlkConfig,
    config_bytes: Vec<u8>,
    /// Backing storage.
    storage: Vec<u8>,
    /// Read-only flag.
    read_only: bool,
    /// Serial number / device ID (20 bytes).
    serial: [u8; 20],
    /// Pending interrupt.
    pub pending_irq: bool,
}

impl VirtioBlk {
    /// Create a block device with the given capacity in bytes.
    pub fn new(capacity_bytes: u64) -> Self {
        let sectors = capacity_bytes / 512;
        let mut config = VirtioBlkConfig::default();
        config.capacity = sectors;
        config.blk_size = 512;
        config.seg_max = 128;

        let config_bytes = config_to_bytes(&config);

        Self {
            config,
            config_bytes,
            storage: vec![0u8; capacity_bytes as usize],
            read_only: false,
            serial: *b"helm-virtio-blk\0\0\0\0\0",
            pending_irq: false,
        }
    }

    /// Create a read-only block device.
    pub fn new_readonly(data: Vec<u8>) -> Self {
        let capacity = data.len() as u64;
        let sectors = capacity / 512;
        let mut config = VirtioBlkConfig::default();
        config.capacity = sectors;

        let config_bytes = config_to_bytes(&config);

        Self {
            config,
            config_bytes,
            storage: data,
            read_only: true,
            serial: *b"helm-virtio-blk-ro\0\0",
            pending_irq: false,
        }
    }

    /// Read sectors from the backing storage.
    pub fn read_sectors(&self, sector: u64, count: u64) -> Option<&[u8]> {
        let start = (sector * 512) as usize;
        let end = start + (count * 512) as usize;
        if end <= self.storage.len() {
            Some(&self.storage[start..end])
        } else {
            None
        }
    }

    /// Write sectors to the backing storage.
    pub fn write_sectors(&mut self, sector: u64, data: &[u8]) -> bool {
        if self.read_only {
            return false;
        }
        let start = (sector * 512) as usize;
        let end = start + data.len();
        if end <= self.storage.len() {
            self.storage[start..end].copy_from_slice(data);
            true
        } else {
            false
        }
    }

    fn process_request(&mut self, queues: &mut [Virtqueue], queue_idx: u16) {
        let q = match queues.get_mut(queue_idx as usize) {
            Some(Virtqueue::Split(q)) => q,
            _ => return,
        };

        while let Some(head) = q.pop_avail() {
            let chain = q.walk_chain(head);
            if chain.len() < 2 {
                q.push_used(head, 0);
                continue;
            }

            // First descriptor: request header (type + sector)
            let req_type = chain[0].addr as u32; // simplified: addr encodes type
            let _sector = chain[0].len as u64; // simplified: len encodes sector

            // For simulation: just acknowledge the request
            let status = match req_type {
                VIRTIO_BLK_T_IN | VIRTIO_BLK_T_OUT | VIRTIO_BLK_T_FLUSH => VIRTIO_BLK_S_OK,
                VIRTIO_BLK_T_GET_ID => VIRTIO_BLK_S_OK,
                VIRTIO_BLK_T_DISCARD | VIRTIO_BLK_T_WRITE_ZEROES => {
                    if self.read_only {
                        VIRTIO_BLK_S_IOERR
                    } else {
                        VIRTIO_BLK_S_OK
                    }
                }
                _ => VIRTIO_BLK_S_UNSUPP,
            };

            let written = if req_type == VIRTIO_BLK_T_IN {
                // Data was "read" — report bytes written
                chain.iter().skip(1).map(|d| d.len).sum::<u32>()
            } else {
                1 // status byte
            };

            q.push_used(head, written);
            self.pending_irq = true;
        }
    }
}

impl VirtioDeviceBackend for VirtioBlk {
    fn device_id(&self) -> u32 {
        VIRTIO_DEV_BLK
    }

    fn device_features(&self) -> u64 {
        let mut f = VIRTIO_F_VERSION_1 | VIRTIO_BLK_F_BLK_SIZE | VIRTIO_BLK_F_SEG_MAX
            | VIRTIO_BLK_F_FLUSH | VIRTIO_F_RING_INDIRECT_DESC | VIRTIO_F_RING_EVENT_IDX;
        if self.read_only {
            f |= VIRTIO_BLK_F_RO;
        }
        f
    }

    fn config_size(&self) -> u32 {
        self.config_bytes.len() as u32
    }

    fn read_config(&self, offset: u32) -> u8 {
        self.config_bytes.get(offset as usize).copied().unwrap_or(0)
    }

    fn write_config(&mut self, offset: u32, value: u8) {
        if let Some(b) = self.config_bytes.get_mut(offset as usize) {
            *b = value;
        }
    }

    fn num_queues(&self) -> u16 {
        1 // requestq
    }

    fn queue_notify(&mut self, queue_idx: u16, queues: &mut [Virtqueue]) {
        self.process_request(queues, queue_idx);
    }

    fn reset(&mut self) {
        self.pending_irq = false;
    }

    fn name(&self) -> &str {
        "virtio-blk"
    }
}

fn config_to_bytes(config: &VirtioBlkConfig) -> Vec<u8> {
    // Serialize config struct fields to little-endian bytes.
    let mut bytes = Vec::with_capacity(64);
    bytes.extend_from_slice(&config.capacity.to_le_bytes());
    bytes.extend_from_slice(&config.size_max.to_le_bytes());
    bytes.extend_from_slice(&config.seg_max.to_le_bytes());
    bytes.extend_from_slice(&config.geometry_cylinders.to_le_bytes());
    bytes.push(config.geometry_heads);
    bytes.push(config.geometry_sectors);
    bytes.extend_from_slice(&config.blk_size.to_le_bytes());
    bytes.push(config.topology_physical_block_exp);
    bytes.push(config.topology_alignment_offset);
    bytes.extend_from_slice(&config.topology_min_io_size.to_le_bytes());
    bytes.extend_from_slice(&config.topology_opt_io_size.to_le_bytes());
    bytes.push(config.writeback);
    bytes.push(config._unused0);
    bytes.extend_from_slice(&config.num_queues.to_le_bytes());
    bytes
}
