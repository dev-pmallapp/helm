//! VirtIO filesystem device (type 26) — spec 5.11.
//!
//! Provides a shared filesystem between host and guest using FUSE protocol.

use crate::virtio::features::*;
use crate::virtio::queue::Virtqueue;
use crate::virtio::transport::VirtioDeviceBackend;
use serde::{Deserialize, Serialize};

/// Config space (spec 5.11.4).
#[derive(Debug, Clone)]
pub struct VirtioFsConfig {
    /// Filesystem tag (UTF-8, up to 36 bytes).
    pub tag: [u8; 36],
    /// Number of request queues.
    pub num_request_queues: u32,
    /// Notification queue buffer size.
    pub notify_buf_size: u32,
}

impl Default for VirtioFsConfig {
    fn default() -> Self {
        let mut tag = [0u8; 36];
        let name = b"helmfs";
        tag[..name.len()].copy_from_slice(name);
        Self {
            tag,
            num_request_queues: 1,
            notify_buf_size: 0,
        }
    }
}

pub struct VirtioFs {
    config: VirtioFsConfig,
    config_bytes: Vec<u8>,
    pub pending_irq: bool,
}

impl VirtioFs {
    pub fn new(tag: &str) -> Self {
        let mut config = VirtioFsConfig::default();
        let tag_bytes = tag.as_bytes();
        let len = tag_bytes.len().min(36);
        config.tag = [0u8; 36];
        config.tag[..len].copy_from_slice(&tag_bytes[..len]);
        let config_bytes = fs_config_to_bytes(&config);
        Self {
            config,
            config_bytes,
            pending_irq: false,
        }
    }
}

impl Default for VirtioFs {
    fn default() -> Self {
        Self::new("helmfs")
    }
}

impl VirtioDeviceBackend for VirtioFs {
    fn device_id(&self) -> u32 { VIRTIO_DEV_FS }

    fn device_features(&self) -> u64 {
        VIRTIO_F_VERSION_1 | VIRTIO_FS_F_NOTIFICATION
    }

    fn config_size(&self) -> u32 { self.config_bytes.len() as u32 }
    fn read_config(&self, offset: u32) -> u8 {
        self.config_bytes.get(offset as usize).copied().unwrap_or(0)
    }
    fn write_config(&mut self, _offset: u32, _value: u8) {}

    fn num_queues(&self) -> u16 {
        1 + self.config.num_request_queues as u16 // hiprio + request queues
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

    fn reset(&mut self) { self.pending_irq = false; }
    fn name(&self) -> &str { "virtio-fs" }
}

fn fs_config_to_bytes(config: &VirtioFsConfig) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(44);
    bytes.extend_from_slice(&config.tag);
    bytes.extend_from_slice(&config.num_request_queues.to_le_bytes());
    bytes.extend_from_slice(&config.notify_buf_size.to_le_bytes());
    bytes
}
