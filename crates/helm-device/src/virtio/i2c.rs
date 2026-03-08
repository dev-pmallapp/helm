//! VirtIO I2C adapter device (type 34) — spec 5.18.

use crate::virtio::features::*;
use crate::virtio::queue::Virtqueue;
use crate::virtio::transport::VirtioDeviceBackend;

/// I2C request flags (spec 5.18.6.1).
pub const VIRTIO_I2C_FLAGS_FAIL_NEXT: u32 = 1 << 0;
pub const VIRTIO_I2C_FLAGS_M_RD: u32 = 1 << 1;

/// I2C response status.
pub const VIRTIO_I2C_MSG_OK: u8 = 0;
pub const VIRTIO_I2C_MSG_ERR: u8 = 1;

pub struct VirtioI2c {
    pub pending_irq: bool,
}

impl VirtioI2c {
    pub fn new() -> Self {
        Self { pending_irq: false }
    }
}

impl Default for VirtioI2c {
    fn default() -> Self {
        Self::new()
    }
}

impl VirtioDeviceBackend for VirtioI2c {
    fn device_id(&self) -> u32 {
        VIRTIO_DEV_I2C
    }
    fn device_features(&self) -> u64 {
        VIRTIO_F_VERSION_1 | VIRTIO_I2C_F_ZERO_LENGTH_REQUEST
    }

    fn config_size(&self) -> u32 {
        0
    }
    fn read_config(&self, _offset: u32) -> u8 {
        0
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
            q.push_used(head, 1); // 1 byte status
            self.pending_irq = true;
        }
    }

    fn reset(&mut self) {
        self.pending_irq = false;
    }
    fn name(&self) -> &str {
        "virtio-i2c"
    }
}
