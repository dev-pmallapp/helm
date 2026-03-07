//! VirtIO GPU device (type 16) — spec 5.7.
//!
//! Virtual graphics adapter supporting 2D operations (scanout, resource
//! creation, transfer, flush). Optional 3D (virgl) support.

use crate::virtio::features::*;
use crate::virtio::queue::Virtqueue;
use crate::virtio::transport::VirtioDeviceBackend;
use serde::{Deserialize, Serialize};

/// Config space (spec 5.7.4).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VirtioGpuConfig {
    pub events_read: u32,
    pub events_clear: u32,
    pub num_scanouts: u32,
    pub num_capsets: u32,
}

impl Default for VirtioGpuConfig {
    fn default() -> Self {
        Self {
            events_read: 0,
            events_clear: 0,
            num_scanouts: 1,
            num_capsets: 0,
        }
    }
}

/// GPU command types (spec 5.7.6.7).
pub const VIRTIO_GPU_CMD_GET_DISPLAY_INFO: u32 = 0x0100;
pub const VIRTIO_GPU_CMD_RESOURCE_CREATE_2D: u32 = 0x0101;
pub const VIRTIO_GPU_CMD_RESOURCE_UNREF: u32 = 0x0102;
pub const VIRTIO_GPU_CMD_SET_SCANOUT: u32 = 0x0103;
pub const VIRTIO_GPU_CMD_RESOURCE_FLUSH: u32 = 0x0104;
pub const VIRTIO_GPU_CMD_TRANSFER_TO_HOST_2D: u32 = 0x0105;
pub const VIRTIO_GPU_CMD_RESOURCE_ATTACH_BACKING: u32 = 0x0106;
pub const VIRTIO_GPU_CMD_RESOURCE_DETACH_BACKING: u32 = 0x0107;
pub const VIRTIO_GPU_CMD_GET_CAPSET_INFO: u32 = 0x0108;
pub const VIRTIO_GPU_CMD_GET_CAPSET: u32 = 0x0109;
pub const VIRTIO_GPU_CMD_GET_EDID: u32 = 0x010a;
pub const VIRTIO_GPU_CMD_RESOURCE_ASSIGN_UUID: u32 = 0x010b;
pub const VIRTIO_GPU_CMD_RESOURCE_CREATE_BLOB: u32 = 0x010c;
pub const VIRTIO_GPU_CMD_SET_SCANOUT_BLOB: u32 = 0x010d;

/// GPU cursor commands.
pub const VIRTIO_GPU_CMD_UPDATE_CURSOR: u32 = 0x0300;
pub const VIRTIO_GPU_CMD_MOVE_CURSOR: u32 = 0x0301;

/// GPU response types.
pub const VIRTIO_GPU_RESP_OK_NODATA: u32 = 0x1100;
pub const VIRTIO_GPU_RESP_OK_DISPLAY_INFO: u32 = 0x1101;
pub const VIRTIO_GPU_RESP_OK_CAPSET_INFO: u32 = 0x1102;
pub const VIRTIO_GPU_RESP_OK_CAPSET: u32 = 0x1103;
pub const VIRTIO_GPU_RESP_OK_EDID: u32 = 0x1104;
pub const VIRTIO_GPU_RESP_OK_RESOURCE_UUID: u32 = 0x1105;
pub const VIRTIO_GPU_RESP_OK_MAP_INFO: u32 = 0x1106;
pub const VIRTIO_GPU_RESP_ERR_UNSPEC: u32 = 0x1200;
pub const VIRTIO_GPU_RESP_ERR_OUT_OF_MEMORY: u32 = 0x1201;
pub const VIRTIO_GPU_RESP_ERR_INVALID_SCANOUT_ID: u32 = 0x1202;
pub const VIRTIO_GPU_RESP_ERR_INVALID_RESOURCE_ID: u32 = 0x1203;
pub const VIRTIO_GPU_RESP_ERR_INVALID_CONTEXT_ID: u32 = 0x1204;
pub const VIRTIO_GPU_RESP_ERR_INVALID_PARAMETER: u32 = 0x1205;

/// Pixel formats.
pub const VIRTIO_GPU_FORMAT_B8G8R8A8_UNORM: u32 = 1;
pub const VIRTIO_GPU_FORMAT_B8G8R8X8_UNORM: u32 = 2;
pub const VIRTIO_GPU_FORMAT_A8R8G8B8_UNORM: u32 = 3;
pub const VIRTIO_GPU_FORMAT_X8R8G8B8_UNORM: u32 = 4;
pub const VIRTIO_GPU_FORMAT_R8G8B8A8_UNORM: u32 = 67;
pub const VIRTIO_GPU_FORMAT_X8B8G8R8_UNORM: u32 = 68;
pub const VIRTIO_GPU_FORMAT_A8B8G8R8_UNORM: u32 = 121;
pub const VIRTIO_GPU_FORMAT_R8G8B8X8_UNORM: u32 = 134;

/// A GPU resource (2D texture).
#[derive(Debug, Clone)]
pub struct GpuResource {
    pub id: u32,
    pub width: u32,
    pub height: u32,
    pub format: u32,
    pub data: Vec<u8>,
}

/// Scanout configuration.
#[derive(Debug, Clone, Default)]
pub struct Scanout {
    pub resource_id: u32,
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
    pub enabled: bool,
}

pub struct VirtioGpu {
    config: VirtioGpuConfig,
    config_bytes: Vec<u8>,
    pub resources: Vec<GpuResource>,
    pub scanouts: Vec<Scanout>,
    pub pending_irq: bool,
}

impl VirtioGpu {
    pub fn new(num_scanouts: u32) -> Self {
        let mut config = VirtioGpuConfig::default();
        config.num_scanouts = num_scanouts;
        let config_bytes = gpu_config_to_bytes(&config);
        let scanouts = (0..num_scanouts).map(|_| Scanout::default()).collect();
        Self {
            config,
            config_bytes,
            resources: Vec::new(),
            scanouts,
            pending_irq: false,
        }
    }

    fn process_control(&mut self, queues: &mut [Virtqueue]) {
        let q = match queues.get_mut(0) {
            Some(Virtqueue::Split(q)) => q,
            _ => return,
        };
        while let Some(head) = q.pop_avail() {
            // Simulate: acknowledge with RESP_OK_NODATA
            q.push_used(head, 24); // response header size
            self.pending_irq = true;
        }
    }
}

impl Default for VirtioGpu {
    fn default() -> Self {
        Self::new(1)
    }
}

impl VirtioDeviceBackend for VirtioGpu {
    fn device_id(&self) -> u32 {
        VIRTIO_DEV_GPU
    }

    fn device_features(&self) -> u64 {
        VIRTIO_F_VERSION_1 | VIRTIO_GPU_F_EDID | VIRTIO_GPU_F_RESOURCE_UUID
    }

    fn config_size(&self) -> u32 {
        self.config_bytes.len() as u32
    }

    fn read_config(&self, offset: u32) -> u8 {
        self.config_bytes.get(offset as usize).copied().unwrap_or(0)
    }

    fn write_config(&mut self, offset: u32, value: u8) {
        // events_clear at offset 4
        if offset >= 4 && offset < 8 {
            if let Some(b) = self.config_bytes.get_mut(offset as usize) {
                *b = value;
            }
        }
    }

    fn num_queues(&self) -> u16 {
        2 // controlq, cursorq
    }

    fn queue_notify(&mut self, queue_idx: u16, queues: &mut [Virtqueue]) {
        if queue_idx == 0 {
            self.process_control(queues);
        }
    }

    fn reset(&mut self) {
        self.resources.clear();
        for s in &mut self.scanouts {
            *s = Scanout::default();
        }
        self.pending_irq = false;
    }

    fn name(&self) -> &str {
        "virtio-gpu"
    }
}

fn gpu_config_to_bytes(config: &VirtioGpuConfig) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(16);
    bytes.extend_from_slice(&config.events_read.to_le_bytes());
    bytes.extend_from_slice(&config.events_clear.to_le_bytes());
    bytes.extend_from_slice(&config.num_scanouts.to_le_bytes());
    bytes.extend_from_slice(&config.num_capsets.to_le_bytes());
    bytes
}
