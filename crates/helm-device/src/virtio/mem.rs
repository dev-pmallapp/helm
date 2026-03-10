//! VirtIO memory device (type 24) — spec 5.15.
//!
//! Provides hot-plug/unplug of memory to the guest.

use crate::virtio::features::*;
use crate::virtio::queue::Virtqueue;
use crate::virtio::transport::VirtioDeviceBackend;
use serde::{Deserialize, Serialize};

/// Memory device request types (spec 5.15.6.1).
pub const VIRTIO_MEM_REQ_PLUG: u16 = 0;
pub const VIRTIO_MEM_REQ_UNPLUG: u16 = 1;
pub const VIRTIO_MEM_REQ_UNPLUG_ALL: u16 = 2;
pub const VIRTIO_MEM_REQ_STATE: u16 = 3;

/// Config space (spec 5.15.4).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct VirtioMemConfig {
    pub block_size: u64,
    pub node_id: u16,
    pub _padding: [u8; 6],
    pub addr: u64,
    pub region_size: u64,
    pub usable_region_size: u64,
    pub plugged_size: u64,
    pub requested_size: u64,
}

pub struct VirtioMem {
    config: VirtioMemConfig,
    config_bytes: Vec<u8>,
    pub pending_irq: bool,
}

impl VirtioMem {
    pub fn new(addr: u64, region_size: u64, block_size: u64) -> Self {
        let config = VirtioMemConfig {
            block_size,
            node_id: 0,
            _padding: [0; 6],
            addr,
            region_size,
            usable_region_size: region_size,
            plugged_size: 0,
            requested_size: 0,
        };
        let config_bytes = mem_config_to_bytes(&config);
        Self {
            config,
            config_bytes,
            pending_irq: false,
        }
    }

    pub fn set_requested_size(&mut self, size: u64) {
        self.config.requested_size = size;
        self.config_bytes = mem_config_to_bytes(&self.config);
    }
}

impl VirtioDeviceBackend for VirtioMem {
    fn device_id(&self) -> u32 {
        VIRTIO_DEV_MEM
    }
    fn device_features(&self) -> u64 {
        VIRTIO_F_VERSION_1 | VIRTIO_MEM_F_UNPLUGGED_INACCESSIBLE
    }

    fn config_size(&self) -> u32 {
        self.config_bytes.len() as u32
    }
    fn read_config(&self, offset: u32) -> u8 {
        self.config_bytes.get(offset as usize).copied().unwrap_or(0)
    }
    fn write_config(&mut self, _offset: u32, _value: u8) {}

    fn num_queues(&self) -> u16 {
        1
    }

    fn queue_notify(&mut self, queue_idx: u16, queues: &mut [Virtqueue]) {
        let q = match queues.get_mut(queue_idx as usize) {
            Some(Virtqueue::Split(q)) => q,
            _ => return,
        };
        while let Some(head) = q.pop_avail() {
            q.push_used(head, 0);
            self.pending_irq = true;
        }
    }

    fn reset(&mut self) {
        self.pending_irq = false;
    }
    fn name(&self) -> &str {
        "virtio-mem"
    }
}

fn mem_config_to_bytes(c: &VirtioMemConfig) -> Vec<u8> {
    let mut b = Vec::with_capacity(56);
    b.extend_from_slice(&c.block_size.to_le_bytes());
    b.extend_from_slice(&c.node_id.to_le_bytes());
    b.extend_from_slice(&c._padding);
    b.extend_from_slice(&c.addr.to_le_bytes());
    b.extend_from_slice(&c.region_size.to_le_bytes());
    b.extend_from_slice(&c.usable_region_size.to_le_bytes());
    b.extend_from_slice(&c.plugged_size.to_le_bytes());
    b.extend_from_slice(&c.requested_size.to_le_bytes());
    b
}
