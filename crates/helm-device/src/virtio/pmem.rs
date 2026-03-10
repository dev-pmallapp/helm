//! VirtIO persistent memory device (type 27) — spec 5.12.

use crate::virtio::features::*;
use crate::virtio::queue::Virtqueue;
use crate::virtio::transport::VirtioDeviceBackend;
use serde::{Deserialize, Serialize};

/// Config space (spec 5.12.4).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct VirtioPmemConfig {
    pub start: u64,
    pub size: u64,
}

pub struct VirtioPmem {
    config: VirtioPmemConfig,
    config_bytes: Vec<u8>,
    pub storage: Vec<u8>,
    pub pending_irq: bool,
}

impl VirtioPmem {
    pub fn new(start: u64, size: u64) -> Self {
        let config = VirtioPmemConfig { start, size };
        let config_bytes = pmem_config_to_bytes(&config);
        Self {
            config,
            config_bytes,
            storage: vec![0u8; size as usize],
            pending_irq: false,
        }
    }

    /// Return the device configuration.
    pub fn config(&self) -> &VirtioPmemConfig {
        &self.config
    }
}

impl VirtioDeviceBackend for VirtioPmem {
    fn device_id(&self) -> u32 {
        VIRTIO_DEV_PMEM
    }

    fn device_features(&self) -> u64 {
        VIRTIO_F_VERSION_1 | VIRTIO_PMEM_F_SHMEM_REGION
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
    } // requestq

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
        "virtio-pmem"
    }
}

fn pmem_config_to_bytes(c: &VirtioPmemConfig) -> Vec<u8> {
    let mut b = Vec::with_capacity(16);
    b.extend_from_slice(&c.start.to_le_bytes());
    b.extend_from_slice(&c.size.to_le_bytes());
    b
}
