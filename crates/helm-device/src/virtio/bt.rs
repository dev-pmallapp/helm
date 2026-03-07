//! VirtIO Bluetooth device (type 42).

use crate::virtio::features::*;
use crate::virtio::queue::Virtqueue;
use crate::virtio::transport::VirtioDeviceBackend;

/// HCI packet types.
pub const VIRTIO_BT_CMD: u8 = 1;
pub const VIRTIO_BT_EVT: u8 = 2;
pub const VIRTIO_BT_ACL: u8 = 3;
pub const VIRTIO_BT_SCO: u8 = 4;
pub const VIRTIO_BT_ISO: u8 = 5;

/// Bluetooth vendor types.
pub const VIRTIO_BT_CONFIG_TYPE_PRIMARY: u8 = 0;
pub const VIRTIO_BT_CONFIG_TYPE_AMP: u8 = 1;

pub struct VirtioBt {
    pub vendor: u16,
    pub msft_opcode: u16,
    pub pending_irq: bool,
}

impl VirtioBt {
    pub fn new() -> Self {
        Self { vendor: 0, msft_opcode: 0, pending_irq: false }
    }
}

impl Default for VirtioBt {
    fn default() -> Self { Self::new() }
}

impl VirtioDeviceBackend for VirtioBt {
    fn device_id(&self) -> u32 { VIRTIO_DEV_BT }
    fn device_features(&self) -> u64 { VIRTIO_F_VERSION_1 }

    fn config_size(&self) -> u32 { 8 }
    fn read_config(&self, offset: u32) -> u8 {
        match offset {
            0 => VIRTIO_BT_CONFIG_TYPE_PRIMARY,
            1 => 0, // alignment
            2 => self.vendor as u8,
            3 => (self.vendor >> 8) as u8,
            4 => self.msft_opcode as u8,
            5 => (self.msft_opcode >> 8) as u8,
            _ => 0,
        }
    }
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
    fn name(&self) -> &str { "virtio-bt" }
}
