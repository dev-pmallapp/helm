//! VirtIO IOMMU device (type 23) — spec 5.13.

use crate::virtio::features::*;
use crate::virtio::queue::Virtqueue;
use crate::virtio::transport::VirtioDeviceBackend;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// IOMMU request types (spec 5.13.6.1).
pub const VIRTIO_IOMMU_T_ATTACH: u8 = 1;
pub const VIRTIO_IOMMU_T_DETACH: u8 = 2;
pub const VIRTIO_IOMMU_T_MAP: u8 = 3;
pub const VIRTIO_IOMMU_T_UNMAP: u8 = 4;
pub const VIRTIO_IOMMU_T_PROBE: u8 = 5;

/// Config space (spec 5.13.4).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VirtioIommuConfig {
    pub page_size_mask: u64,
    pub input_range_start: u64,
    pub input_range_end: u64,
    pub domain_range_start: u32,
    pub domain_range_end: u32,
    pub probe_size: u32,
    pub bypass: u8,
    pub _padding: [u8; 7],
}

impl Default for VirtioIommuConfig {
    fn default() -> Self {
        Self {
            page_size_mask: 0xFFFF_FFFF_FFFF_F000, // 4K pages
            input_range_start: 0,
            input_range_end: u64::MAX,
            domain_range_start: 0,
            domain_range_end: u32::MAX,
            probe_size: 64,
            bypass: 1,
            _padding: [0; 7],
        }
    }
}

/// An IOMMU mapping entry.
#[derive(Debug, Clone)]
pub struct IommuMapping {
    pub domain: u32,
    pub virt_start: u64,
    pub virt_end: u64,
    pub phys_start: u64,
    pub flags: u32,
}

pub struct VirtioIommu {
    config: VirtioIommuConfig,
    config_bytes: Vec<u8>,
    /// Domain → endpoint attachments.
    pub attachments: HashMap<u32, Vec<u32>>,
    /// Address mappings per domain.
    pub mappings: HashMap<u32, Vec<IommuMapping>>,
    pub pending_irq: bool,
}

impl VirtioIommu {
    pub fn new() -> Self {
        let config = VirtioIommuConfig::default();
        let config_bytes = iommu_config_to_bytes(&config);
        Self {
            config,
            config_bytes,
            attachments: HashMap::new(),
            mappings: HashMap::new(),
            pending_irq: false,
        }
    }

    /// Return the device configuration.
    pub fn config(&self) -> &VirtioIommuConfig {
        &self.config
    }
}

impl Default for VirtioIommu {
    fn default() -> Self {
        Self::new()
    }
}

impl VirtioDeviceBackend for VirtioIommu {
    fn device_id(&self) -> u32 {
        VIRTIO_DEV_IOMMU
    }

    fn device_features(&self) -> u64 {
        VIRTIO_F_VERSION_1
            | VIRTIO_IOMMU_F_INPUT_RANGE
            | VIRTIO_IOMMU_F_DOMAIN_RANGE
            | VIRTIO_IOMMU_F_MAP_UNMAP
            | VIRTIO_IOMMU_F_BYPASS
            | VIRTIO_IOMMU_F_PROBE
            | VIRTIO_IOMMU_F_BYPASS_CONFIG
    }

    fn config_size(&self) -> u32 {
        self.config_bytes.len() as u32
    }
    fn read_config(&self, offset: u32) -> u8 {
        self.config_bytes.get(offset as usize).copied().unwrap_or(0)
    }
    fn write_config(&mut self, _offset: u32, _value: u8) {}

    fn num_queues(&self) -> u16 {
        2
    } // requestq, eventq

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
        self.attachments.clear();
        self.mappings.clear();
        self.pending_irq = false;
    }

    fn name(&self) -> &str {
        "virtio-iommu"
    }
}

fn iommu_config_to_bytes(c: &VirtioIommuConfig) -> Vec<u8> {
    let mut b = Vec::with_capacity(44);
    b.extend_from_slice(&c.page_size_mask.to_le_bytes());
    b.extend_from_slice(&c.input_range_start.to_le_bytes());
    b.extend_from_slice(&c.input_range_end.to_le_bytes());
    b.extend_from_slice(&c.domain_range_start.to_le_bytes());
    b.extend_from_slice(&c.domain_range_end.to_le_bytes());
    b.extend_from_slice(&c.probe_size.to_le_bytes());
    b.push(c.bypass);
    b.extend_from_slice(&c._padding);
    b
}
