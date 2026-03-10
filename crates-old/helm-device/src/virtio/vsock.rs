//! VirtIO socket device (type 19) — spec 5.10.
//!
//! Provides host-guest socket communication (AF_VSOCK). Supports
//! stream and seqpacket transport.

use crate::virtio::features::*;
use crate::virtio::queue::Virtqueue;
use crate::virtio::transport::VirtioDeviceBackend;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;

/// Vsock operation types (spec 5.10.6.6).
pub const VIRTIO_VSOCK_OP_INVALID: u16 = 0;
pub const VIRTIO_VSOCK_OP_REQUEST: u16 = 1;
pub const VIRTIO_VSOCK_OP_RESPONSE: u16 = 2;
pub const VIRTIO_VSOCK_OP_RST: u16 = 3;
pub const VIRTIO_VSOCK_OP_SHUTDOWN: u16 = 4;
pub const VIRTIO_VSOCK_OP_RW: u16 = 5;
pub const VIRTIO_VSOCK_OP_CREDIT_UPDATE: u16 = 6;
pub const VIRTIO_VSOCK_OP_CREDIT_REQUEST: u16 = 7;

/// Vsock transport types.
pub const VIRTIO_VSOCK_TYPE_STREAM: u16 = 1;
pub const VIRTIO_VSOCK_TYPE_SEQPACKET: u16 = 2;

/// Config space (spec 5.10.4).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct VirtioVsockConfig {
    pub guest_cid: u64,
}

pub struct VirtioVsock {
    config: VirtioVsockConfig,
    config_bytes: Vec<u8>,
    pub tx_queue: VecDeque<Vec<u8>>,
    pub pending_irq: bool,
}

impl VirtioVsock {
    pub fn new(guest_cid: u64) -> Self {
        let config = VirtioVsockConfig { guest_cid };
        let config_bytes = config.guest_cid.to_le_bytes().to_vec();
        Self {
            config,
            config_bytes,
            tx_queue: VecDeque::new(),
            pending_irq: false,
        }
    }

    /// Return the device configuration.
    pub fn config(&self) -> &VirtioVsockConfig {
        &self.config
    }
}

impl VirtioDeviceBackend for VirtioVsock {
    fn device_id(&self) -> u32 {
        VIRTIO_DEV_VSOCK
    }

    fn device_features(&self) -> u64 {
        VIRTIO_F_VERSION_1 | VIRTIO_VSOCK_F_STREAM | VIRTIO_VSOCK_F_SEQPACKET
    }

    fn config_size(&self) -> u32 {
        self.config_bytes.len() as u32
    }
    fn read_config(&self, offset: u32) -> u8 {
        self.config_bytes.get(offset as usize).copied().unwrap_or(0)
    }
    fn write_config(&mut self, _offset: u32, _value: u8) {}

    fn num_queues(&self) -> u16 {
        3
    } // rx, tx, event

    fn queue_notify(&mut self, queue_idx: u16, queues: &mut [Virtqueue]) {
        if queue_idx == 1 {
            let q = match queues.get_mut(1) {
                Some(Virtqueue::Split(q)) => q,
                _ => return,
            };
            while let Some(head) = q.pop_avail() {
                q.push_used(head, 0);
                self.pending_irq = true;
            }
        }
    }

    fn reset(&mut self) {
        self.tx_queue.clear();
        self.pending_irq = false;
    }

    fn name(&self) -> &str {
        "virtio-vsock"
    }
}
