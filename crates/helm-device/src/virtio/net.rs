//! VirtIO network device (type 1) — spec 5.1.
//!
//! Provides a virtual NIC with configurable MAC address, link status,
//! and optional multi-queue support.

use crate::virtio::features::*;
use crate::virtio::queue::Virtqueue;
use crate::virtio::transport::VirtioDeviceBackend;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;

/// Network packet header (spec 5.1.6.1).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[repr(C)]
pub struct VirtioNetHdr {
    pub flags: u8,
    pub gso_type: u8,
    pub hdr_len: u16,
    pub gso_size: u16,
    pub csum_start: u16,
    pub csum_offset: u16,
    pub num_buffers: u16,
}

pub const VIRTIO_NET_HDR_GSO_NONE: u8 = 0;
pub const VIRTIO_NET_HDR_GSO_TCPV4: u8 = 1;
pub const VIRTIO_NET_HDR_GSO_UDP: u8 = 3;
pub const VIRTIO_NET_HDR_GSO_TCPV6: u8 = 4;
pub const VIRTIO_NET_HDR_GSO_UDP_L4: u8 = 5;
pub const VIRTIO_NET_HDR_GSO_ECN: u8 = 0x80;

pub const VIRTIO_NET_HDR_F_NEEDS_CSUM: u8 = 1;
pub const VIRTIO_NET_HDR_F_DATA_VALID: u8 = 2;
pub const VIRTIO_NET_HDR_F_RSC_INFO: u8 = 4;

/// Network link status.
pub const VIRTIO_NET_S_LINK_UP: u16 = 1;
pub const VIRTIO_NET_S_ANNOUNCE: u16 = 2;

/// Config space (spec 5.1.4).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VirtioNetConfig {
    pub mac: [u8; 6],
    pub status: u16,
    pub max_virtqueue_pairs: u16,
    pub mtu: u16,
    pub speed: u32,
    pub duplex: u8,
    pub rss_max_key_size: u8,
    pub rss_max_indirection_table_length: u16,
    pub supported_hash_types: u32,
}

impl Default for VirtioNetConfig {
    fn default() -> Self {
        Self {
            mac: [0x52, 0x54, 0x00, 0x12, 0x34, 0x56],
            status: VIRTIO_NET_S_LINK_UP,
            max_virtqueue_pairs: 1,
            mtu: 1500,
            speed: 10000, // 10 Gbps
            duplex: 1,    // full duplex
            rss_max_key_size: 0,
            rss_max_indirection_table_length: 0,
            supported_hash_types: 0,
        }
    }
}

/// VirtIO network device.
pub struct VirtioNet {
    config: VirtioNetConfig,
    config_bytes: Vec<u8>,
    /// Transmit buffer (packets sent by the guest).
    pub tx_queue: VecDeque<Vec<u8>>,
    /// Receive buffer (packets to deliver to the guest).
    pub rx_queue: VecDeque<Vec<u8>>,
    pub pending_irq: bool,
}

impl VirtioNet {
    pub fn new() -> Self {
        let config = VirtioNetConfig::default();
        let config_bytes = net_config_to_bytes(&config);
        Self {
            config,
            config_bytes,
            tx_queue: VecDeque::new(),
            rx_queue: VecDeque::new(),
            pending_irq: false,
        }
    }

    pub fn with_mac(mut self, mac: [u8; 6]) -> Self {
        self.config.mac = mac;
        self.config_bytes = net_config_to_bytes(&self.config);
        self
    }

    pub fn with_mtu(mut self, mtu: u16) -> Self {
        self.config.mtu = mtu;
        self.config_bytes = net_config_to_bytes(&self.config);
        self
    }

    /// Inject a packet for the guest to receive.
    pub fn inject_rx(&mut self, packet: Vec<u8>) {
        self.rx_queue.push_back(packet);
    }

    /// Drain transmitted packets.
    pub fn drain_tx(&mut self) -> Vec<Vec<u8>> {
        self.tx_queue.drain(..).collect()
    }

    fn process_tx(&mut self, queues: &mut [Virtqueue]) {
        let q = match queues.get_mut(1) {
            Some(Virtqueue::Split(q)) => q,
            _ => return,
        };

        while let Some(head) = q.pop_avail() {
            // Collect packet data from descriptor chain
            let chain = q.walk_chain(head);
            let mut packet = Vec::new();
            for desc in &chain {
                // In simulation, we record the descriptor lengths
                packet.extend_from_slice(&vec![0u8; desc.len as usize]);
            }
            self.tx_queue.push_back(packet);
            q.push_used(head, 0);
            self.pending_irq = true;
        }
    }

    fn process_rx(&mut self, queues: &mut [Virtqueue]) {
        let q = match queues.get_mut(0) {
            Some(Virtqueue::Split(q)) => q,
            _ => return,
        };

        while let Some(packet) = self.rx_queue.front() {
            if let Some(head) = q.pop_avail() {
                let pkt = self.rx_queue.pop_front().unwrap();
                q.push_used(head, pkt.len() as u32);
                self.pending_irq = true;
            } else {
                break;
            }
        }
    }
}

impl Default for VirtioNet {
    fn default() -> Self {
        Self::new()
    }
}

impl VirtioDeviceBackend for VirtioNet {
    fn device_id(&self) -> u32 {
        VIRTIO_DEV_NET
    }

    fn device_features(&self) -> u64 {
        VIRTIO_F_VERSION_1
            | VIRTIO_NET_F_MAC
            | VIRTIO_NET_F_STATUS
            | VIRTIO_NET_F_MTU
            | VIRTIO_NET_F_CSUM
            | VIRTIO_NET_F_GUEST_CSUM
            | VIRTIO_NET_F_MRG_RXBUF
            | VIRTIO_NET_F_SPEED_DUPLEX
            | VIRTIO_F_RING_INDIRECT_DESC
            | VIRTIO_F_RING_EVENT_IDX
    }

    fn config_size(&self) -> u32 {
        self.config_bytes.len() as u32
    }

    fn read_config(&self, offset: u32) -> u8 {
        self.config_bytes.get(offset as usize).copied().unwrap_or(0)
    }

    fn write_config(&mut self, offset: u32, value: u8) {
        if let Some(b) = self.config_bytes.get_mut(offset as usize) {
            *b = value;
        }
    }

    fn num_queues(&self) -> u16 {
        3 // receiveq, transmitq, controlq
    }

    fn queue_notify(&mut self, queue_idx: u16, queues: &mut [Virtqueue]) {
        match queue_idx {
            0 => self.process_rx(queues),
            1 => self.process_tx(queues),
            _ => {}
        }
    }

    fn reset(&mut self) {
        self.tx_queue.clear();
        self.rx_queue.clear();
        self.pending_irq = false;
    }

    fn name(&self) -> &str {
        "virtio-net"
    }
}

fn net_config_to_bytes(config: &VirtioNetConfig) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(24);
    bytes.extend_from_slice(&config.mac);
    bytes.extend_from_slice(&config.status.to_le_bytes());
    bytes.extend_from_slice(&config.max_virtqueue_pairs.to_le_bytes());
    bytes.extend_from_slice(&config.mtu.to_le_bytes());
    bytes.extend_from_slice(&config.speed.to_le_bytes());
    bytes.push(config.duplex);
    bytes.push(config.rss_max_key_size);
    bytes.extend_from_slice(&config.rss_max_indirection_table_length.to_le_bytes());
    bytes.extend_from_slice(&config.supported_hash_types.to_le_bytes());
    bytes
}
