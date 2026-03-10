//! VirtIO SCSI host device (type 8) — spec 5.6.
//!
//! Presents a SCSI host bus adapter. The guest submits SCSI CDBs through
//! virtqueues; the device executes them against backing storage.

use crate::virtio::features::*;
use crate::virtio::queue::Virtqueue;
use crate::virtio::transport::VirtioDeviceBackend;
use serde::{Deserialize, Serialize};

/// Config space (spec 5.6.4).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VirtioScsiConfig {
    pub num_queues: u32,
    pub seg_max: u32,
    pub max_sectors: u32,
    pub cmd_per_lun: u32,
    pub event_info_size: u32,
    pub sense_size: u32,
    pub cdb_size: u32,
    pub max_channel: u16,
    pub max_target: u16,
    pub max_lun: u32,
}

impl Default for VirtioScsiConfig {
    fn default() -> Self {
        Self {
            num_queues: 1,
            seg_max: 128,
            max_sectors: 65535,
            cmd_per_lun: 128,
            event_info_size: 0,
            sense_size: 96,
            cdb_size: 32,
            max_channel: 0,
            max_target: 255,
            max_lun: 16383,
        }
    }
}

/// SCSI request response codes.
pub const VIRTIO_SCSI_S_OK: u8 = 0;
pub const VIRTIO_SCSI_S_OVERRUN: u8 = 1;
pub const VIRTIO_SCSI_S_ABORTED: u8 = 2;
pub const VIRTIO_SCSI_S_BAD_TARGET: u8 = 3;
pub const VIRTIO_SCSI_S_RESET: u8 = 4;
pub const VIRTIO_SCSI_S_BUSY: u8 = 5;
pub const VIRTIO_SCSI_S_TRANSPORT_FAILURE: u8 = 6;
pub const VIRTIO_SCSI_S_TARGET_FAILURE: u8 = 7;
pub const VIRTIO_SCSI_S_NEXUS_FAILURE: u8 = 8;
pub const VIRTIO_SCSI_S_FAILURE: u8 = 9;

pub struct VirtioScsi {
    config: VirtioScsiConfig,
    config_bytes: Vec<u8>,
    pub pending_irq: bool,
}

impl VirtioScsi {
    pub fn new() -> Self {
        let config = VirtioScsiConfig::default();
        let config_bytes = scsi_config_to_bytes(&config);
        Self {
            config,
            config_bytes,
            pending_irq: false,
        }
    }

    pub fn with_num_queues(mut self, n: u32) -> Self {
        self.config.num_queues = n;
        self.config_bytes = scsi_config_to_bytes(&self.config);
        self
    }

    fn process_request(&mut self, queues: &mut [Virtqueue], queue_idx: u16) {
        // Queue 0 = control, queue 1 = event, queues 2+ = request
        let q = match queues.get_mut(queue_idx as usize) {
            Some(Virtqueue::Split(q)) => q,
            _ => return,
        };
        while let Some(head) = q.pop_avail() {
            // Simulate: acknowledge request
            q.push_used(head, 0);
            self.pending_irq = true;
        }
    }
}

impl Default for VirtioScsi {
    fn default() -> Self {
        Self::new()
    }
}

impl VirtioDeviceBackend for VirtioScsi {
    fn device_id(&self) -> u32 {
        VIRTIO_DEV_SCSI
    }

    fn device_features(&self) -> u64 {
        VIRTIO_F_VERSION_1 | VIRTIO_SCSI_F_INOUT | VIRTIO_SCSI_F_HOTPLUG | VIRTIO_SCSI_F_CHANGE
    }

    fn config_size(&self) -> u32 {
        self.config_bytes.len() as u32
    }

    fn read_config(&self, offset: u32) -> u8 {
        self.config_bytes.get(offset as usize).copied().unwrap_or(0)
    }

    fn write_config(&mut self, _offset: u32, _value: u8) {}

    fn num_queues(&self) -> u16 {
        2 + self.config.num_queues as u16 // controlq + eventq + request queues
    }

    fn queue_notify(&mut self, queue_idx: u16, queues: &mut [Virtqueue]) {
        self.process_request(queues, queue_idx);
    }

    fn reset(&mut self) {
        self.pending_irq = false;
    }

    fn name(&self) -> &str {
        "virtio-scsi"
    }
}

fn scsi_config_to_bytes(config: &VirtioScsiConfig) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(36);
    bytes.extend_from_slice(&config.num_queues.to_le_bytes());
    bytes.extend_from_slice(&config.seg_max.to_le_bytes());
    bytes.extend_from_slice(&config.max_sectors.to_le_bytes());
    bytes.extend_from_slice(&config.cmd_per_lun.to_le_bytes());
    bytes.extend_from_slice(&config.event_info_size.to_le_bytes());
    bytes.extend_from_slice(&config.sense_size.to_le_bytes());
    bytes.extend_from_slice(&config.cdb_size.to_le_bytes());
    bytes.extend_from_slice(&config.max_channel.to_le_bytes());
    bytes.extend_from_slice(&config.max_target.to_le_bytes());
    bytes.extend_from_slice(&config.max_lun.to_le_bytes());
    bytes
}
