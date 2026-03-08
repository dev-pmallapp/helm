//! VirtIO video encoder/decoder devices (types 30/31).

use crate::virtio::features::*;
use crate::virtio::queue::Virtqueue;
use crate::virtio::transport::VirtioDeviceBackend;
use serde::{Deserialize, Serialize};

/// Video command types (common to encoder and decoder).
pub const VIRTIO_VIDEO_CMD_QUERY_CAPABILITY: u32 = 0x100;
pub const VIRTIO_VIDEO_CMD_STREAM_CREATE: u32 = 0x200;
pub const VIRTIO_VIDEO_CMD_STREAM_DESTROY: u32 = 0x201;
pub const VIRTIO_VIDEO_CMD_STREAM_DRAIN: u32 = 0x202;
pub const VIRTIO_VIDEO_CMD_RESOURCE_CREATE: u32 = 0x300;
pub const VIRTIO_VIDEO_CMD_RESOURCE_QUEUE: u32 = 0x301;
pub const VIRTIO_VIDEO_CMD_RESOURCE_DESTROY_ALL: u32 = 0x302;
pub const VIRTIO_VIDEO_CMD_QUEUE_CLEAR: u32 = 0x303;
pub const VIRTIO_VIDEO_CMD_GET_PARAMS: u32 = 0x400;
pub const VIRTIO_VIDEO_CMD_SET_PARAMS: u32 = 0x401;
pub const VIRTIO_VIDEO_CMD_GET_CONTROL: u32 = 0x500;
pub const VIRTIO_VIDEO_CMD_SET_CONTROL: u32 = 0x501;

/// Video codec formats.
pub const VIRTIO_VIDEO_FORMAT_ARGB8888: u32 = 1;
pub const VIRTIO_VIDEO_FORMAT_NV12: u32 = 2;
pub const VIRTIO_VIDEO_FORMAT_YUV420: u32 = 3;
pub const VIRTIO_VIDEO_FORMAT_H264: u32 = 0x1000;
pub const VIRTIO_VIDEO_FORMAT_HEVC: u32 = 0x1001;
pub const VIRTIO_VIDEO_FORMAT_VP8: u32 = 0x1002;
pub const VIRTIO_VIDEO_FORMAT_VP9: u32 = 0x1003;
pub const VIRTIO_VIDEO_FORMAT_AV1: u32 = 0x1004;

/// Config space for video devices.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct VirtioVideoConfig {
    pub version: u32,
    pub max_caps_length: u32,
    pub max_resp_length: u32,
}

pub struct VirtioVideoEncoder {
    config: VirtioVideoConfig,
    config_bytes: Vec<u8>,
    pub pending_irq: bool,
}

impl VirtioVideoEncoder {
    pub fn new() -> Self {
        let config = VirtioVideoConfig {
            version: 0,
            max_caps_length: 4096,
            max_resp_length: 4096,
        };
        let config_bytes = video_config_to_bytes(&config);
        Self {
            config,
            config_bytes,
            pending_irq: false,
        }
    }

    /// Return the device configuration.
    pub fn config(&self) -> &VirtioVideoConfig {
        &self.config
    }
}

impl Default for VirtioVideoEncoder {
    fn default() -> Self {
        Self::new()
    }
}

impl VirtioDeviceBackend for VirtioVideoEncoder {
    fn device_id(&self) -> u32 {
        VIRTIO_DEV_VIDEO_ENC
    }
    fn device_features(&self) -> u64 {
        VIRTIO_F_VERSION_1 | VIRTIO_VIDEO_F_RESOURCE_GUEST_PAGES
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
    } // commandq, eventq

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
        "virtio-video-enc"
    }
}

pub struct VirtioVideoDecoder {
    config: VirtioVideoConfig,
    config_bytes: Vec<u8>,
    pub pending_irq: bool,
}

impl VirtioVideoDecoder {
    pub fn new() -> Self {
        let config = VirtioVideoConfig {
            version: 0,
            max_caps_length: 4096,
            max_resp_length: 4096,
        };
        let config_bytes = video_config_to_bytes(&config);
        Self {
            config,
            config_bytes,
            pending_irq: false,
        }
    }

    /// Return the device configuration.
    pub fn config(&self) -> &VirtioVideoConfig {
        &self.config
    }
}

impl Default for VirtioVideoDecoder {
    fn default() -> Self {
        Self::new()
    }
}

impl VirtioDeviceBackend for VirtioVideoDecoder {
    fn device_id(&self) -> u32 {
        VIRTIO_DEV_VIDEO_DEC
    }
    fn device_features(&self) -> u64 {
        VIRTIO_F_VERSION_1 | VIRTIO_VIDEO_F_RESOURCE_GUEST_PAGES
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
        "virtio-video-dec"
    }
}

fn video_config_to_bytes(c: &VirtioVideoConfig) -> Vec<u8> {
    let mut b = Vec::with_capacity(12);
    b.extend_from_slice(&c.version.to_le_bytes());
    b.extend_from_slice(&c.max_caps_length.to_le_bytes());
    b.extend_from_slice(&c.max_resp_length.to_le_bytes());
    b
}
