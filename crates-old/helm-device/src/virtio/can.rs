//! VirtIO CAN device (type 36).

use crate::virtio::features::*;
use crate::virtio::queue::Virtqueue;
use crate::virtio::transport::VirtioDeviceBackend;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;

/// CAN frame types.
pub const VIRTIO_CAN_TX: u16 = 0x0001;
pub const VIRTIO_CAN_RX: u16 = 0x0101;

/// CAN status codes.
pub const VIRTIO_CAN_RESULT_OK: u8 = 0;
pub const VIRTIO_CAN_RESULT_NOT_OK: u8 = 1;

/// A CAN frame.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CanFrame {
    pub msg_type: u16,
    pub can_id: u32,
    pub length: u16,
    pub flags: u32,
    pub data: Vec<u8>, // CAN FD supports up to 64 bytes
}

impl Default for CanFrame {
    fn default() -> Self {
        Self {
            msg_type: 0,
            can_id: 0,
            length: 0,
            flags: 0,
            data: vec![0u8; 64],
        }
    }
}

pub struct VirtioCan {
    pub tx_queue: VecDeque<CanFrame>,
    pub rx_queue: VecDeque<CanFrame>,
    pub pending_irq: bool,
}

impl VirtioCan {
    pub fn new() -> Self {
        Self {
            tx_queue: VecDeque::new(),
            rx_queue: VecDeque::new(),
            pending_irq: false,
        }
    }

    pub fn inject_rx(&mut self, frame: CanFrame) {
        self.rx_queue.push_back(frame);
    }
}

impl Default for VirtioCan {
    fn default() -> Self {
        Self::new()
    }
}

impl VirtioDeviceBackend for VirtioCan {
    fn device_id(&self) -> u32 {
        VIRTIO_DEV_CAN
    }
    fn device_features(&self) -> u64 {
        VIRTIO_F_VERSION_1 | VIRTIO_CAN_F_CAN_CLASSIC | VIRTIO_CAN_F_CAN_FD
    }

    fn config_size(&self) -> u32 {
        0
    }
    fn read_config(&self, _offset: u32) -> u8 {
        0
    }
    fn write_config(&mut self, _offset: u32, _value: u8) {}

    fn num_queues(&self) -> u16 {
        3
    } // txq, rxq, controlq

    fn queue_notify(&mut self, queue_idx: u16, queues: &mut [Virtqueue]) {
        let q = match queues.get_mut(queue_idx as usize) {
            Some(Virtqueue::Split(q)) => q,
            _ => return,
        };
        while let Some(head) = q.pop_avail() {
            if queue_idx == 0 {
                self.tx_queue.push_back(CanFrame::default());
            }
            q.push_used(head, 0);
            self.pending_irq = true;
        }
    }

    fn reset(&mut self) {
        self.tx_queue.clear();
        self.rx_queue.clear();
        self.pending_irq = false;
    }

    fn name(&self) -> &str {
        "virtio-can"
    }
}
