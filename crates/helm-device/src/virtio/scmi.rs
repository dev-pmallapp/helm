//! VirtIO SCMI device (type 32) — spec 5.19.
//!
//! System Control and Management Interface — allows the guest to
//! communicate with a platform firmware SCMI agent.

use crate::virtio::features::*;
use crate::virtio::queue::Virtqueue;
use crate::virtio::transport::VirtioDeviceBackend;

pub struct VirtioScmi {
    pub pending_irq: bool,
}

impl VirtioScmi {
    pub fn new() -> Self {
        Self { pending_irq: false }
    }
}

impl Default for VirtioScmi {
    fn default() -> Self { Self::new() }
}

impl VirtioDeviceBackend for VirtioScmi {
    fn device_id(&self) -> u32 { VIRTIO_DEV_SCMI }
    fn device_features(&self) -> u64 {
        VIRTIO_F_VERSION_1 | VIRTIO_SCMI_F_P2A_CHANNELS | VIRTIO_SCMI_F_SHARED_MEMORY
    }

    fn config_size(&self) -> u32 { 0 }
    fn read_config(&self, _offset: u32) -> u8 { 0 }
    fn write_config(&mut self, _offset: u32, _value: u8) {}

    fn num_queues(&self) -> u16 { 2 } // cmdq, eventq

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
    fn name(&self) -> &str { "virtio-scmi" }
}
