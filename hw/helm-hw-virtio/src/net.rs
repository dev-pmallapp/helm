//! VirtIO network device backend (device type 1, VirtIO spec §5.1).
//!
//! Implements a minimal network device with:
//! - **Queue 0** (receiveq): device→driver packet delivery.
//! - **Queue 1** (transmitq): driver→device packet transmission.
//!
//! # Frame format
//!
//! Each virtqueue buffer begins with a [`VirtioNetHdr`] (12 bytes), followed
//! by the raw Ethernet frame. The header carries checksum and GSO metadata;
//! this implementation stubs those fields as zero for functional simulation.
//!
//! # Limitations
//!
//! - No actual network backend (TAP, socket); all transmitted packets are
//!   dropped. Received packets must be injected via
//!   [`VirtioNet::inject_packet`].
//! - Multi-queue (`VIRTIO_NET_F_MQ`) is not implemented.
//! - MAC address is fixed at construction time.
//! - GSO offload (`VIRTIO_NET_F_HOST_TSO4/6`, `CSUM`) is not negotiated.

use crate::proto::features::{VIRTIO_DEVICE_NET, VIRTIO_F_VERSION_1, VIRTIO_NET_F_MAC};
use crate::VirtioBackend;

// ── VirtioNetHdr (VirtIO spec §5.1.6) ────────────────────────────────────────

/// Network packet header prepended to every frame in both queues.
///
/// All unused fields are zero for functional simulation.
#[derive(Debug, Clone, Copy, Default)]
#[repr(C)]
pub struct VirtioNetHdr {
    /// Flags: NEEDS_CSUM (1), DATA_VALID (2), RSC_INFO (4).
    pub flags: u8,
    /// GSO type: NONE (0), TCPV4 (1), UDP (3), TCPV6 (4), ECN (0x80).
    pub gso_type: u8,
    /// Header length for GSO.
    pub hdr_len: u16,
    /// Maximum segment size for GSO.
    pub gso_size: u16,
    /// Offset of the start of the checksum.
    pub csum_start: u16,
    /// Offset after the last byte of the checksum.
    pub csum_offset: u16,
    /// Number of RSC coalesced TCP segments.
    pub num_buffers: u16,
}

/// Size of the VirtioNetHdr in bytes.
pub const VIRTIO_NET_HDR_SIZE: usize = std::mem::size_of::<VirtioNetHdr>();

// ── Config space (VirtIO spec §5.1.4) ────────────────────────────────────────

/// VirtIO network config space.
#[derive(Debug, Clone, Copy)]
struct NetConfig {
    mac: [u8; 6],
    status: u16,
    max_virtqueue_pairs: u16,
}

impl NetConfig {
    fn new(mac: [u8; 6]) -> Self {
        Self { mac, status: 1 /* LINK_UP */, max_virtqueue_pairs: 1 }
    }
}

// ── VirtioNet ────────────────────────────────────────────────────────────────

/// VirtIO network device backend.
///
/// Packets injected via [`inject_packet`](Self::inject_packet) are buffered
/// and delivered to the guest on the next receiveq processing cycle.
/// Transmitted packets are discarded (no TAP/socket backend).
pub struct VirtioNet {
    config: NetConfig,
    /// Packets waiting to be delivered to the guest via the receiveq.
    rx_queue: std::collections::VecDeque<Vec<u8>>,
    /// Pending notification on queue 0 (receiveq).
    rx_notify_pending: bool,
    /// Pending notification on queue 1 (transmitq).
    tx_notify_pending: bool,
}

impl VirtioNet {
    /// Create a new network device with the given MAC address.
    ///
    /// A common locally-administered MAC is `[0x52, 0x54, 0x00, 0x12, 0x34, 0x56]`.
    pub fn new(mac: [u8; 6]) -> Self {
        Self {
            config: NetConfig::new(mac),
            rx_queue: std::collections::VecDeque::new(),
            rx_notify_pending: false,
            tx_notify_pending: false,
        }
    }

    /// Inject a raw Ethernet frame for delivery to the guest.
    ///
    /// The frame should NOT include the [`VirtioNetHdr`]; the device prepends
    /// one with all fields zeroed. The guest driver will read the header then
    /// the frame payload.
    pub fn inject_packet(&mut self, frame: Vec<u8>) {
        self.rx_queue.push_back(frame);
    }

    /// Pop the next pending frame from the RX queue.
    ///
    /// Returns the raw Ethernet frame (without the VirtIO header). The caller
    /// is responsible for prepending a zeroed [`VirtioNetHdr`] when writing
    /// into the guest receiveq buffer.
    pub fn pop_rx_frame(&mut self) -> Option<Vec<u8>> {
        self.rx_queue.pop_front()
    }

    /// Return `true` if there are frames waiting to be delivered to the guest.
    pub fn has_rx_data(&self) -> bool {
        !self.rx_queue.is_empty()
    }

    /// Return the number of pending RX frames.
    pub fn rx_pending_count(&self) -> usize {
        self.rx_queue.len()
    }

    /// Take (and clear) the TX notify pending flag.
    pub fn take_tx_pending(&mut self) -> bool {
        let v = self.tx_notify_pending;
        self.tx_notify_pending = false;
        v
    }

    /// Take (and clear) the RX notify pending flag.
    pub fn take_rx_pending(&mut self) -> bool {
        let v = self.rx_notify_pending;
        self.rx_notify_pending = false;
        v
    }
}

impl VirtioBackend for VirtioNet {
    fn device_type(&self) -> u32 {
        VIRTIO_DEVICE_NET
    }

    fn vendor_id(&self) -> u32 {
        crate::proto::features::VIRTIO_VENDOR_ID as u32
    }

    fn device_features(&self) -> u64 {
        VIRTIO_F_VERSION_1 | VIRTIO_NET_F_MAC
    }

    fn queue_max_size(&self, queue: usize) -> u32 {
        match queue {
            0 | 1 => 256, // receiveq and transmitq
            _ => 0,
        }
    }

    fn queue_notify(&mut self, queue: usize) {
        match queue {
            0 => self.rx_notify_pending = true,
            1 => self.tx_notify_pending = true,
            _ => {}
        }
    }

    fn read_config(&self, offset: u32) -> u32 {
        match offset {
            // MAC bytes 0..3 at offset 0
            0 => u32::from_le_bytes(self.config.mac[0..4].try_into().unwrap()),
            // MAC bytes 4..5 + status at offset 4
            4 => {
                let lo = u16::from_le_bytes(self.config.mac[4..6].try_into().unwrap());
                let hi = self.config.status;
                (hi as u32) << 16 | lo as u32
            }
            // max_virtqueue_pairs at offset 8
            8 => self.config.max_virtqueue_pairs as u32,
            _ => 0,
        }
    }

    fn write_config(&mut self, _offset: u32, _val: u32) {
        // Network config is read-only from the driver side
    }

    fn reset(&mut self) {
        self.rx_queue.clear();
        self.rx_notify_pending = false;
        self.tx_notify_pending = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn default_mac() -> [u8; 6] {
        [0x52, 0x54, 0x00, 0x12, 0x34, 0x56]
    }

    #[test]
    fn net_device_type() {
        let n = VirtioNet::new(default_mac());
        assert_eq!(n.device_type(), VIRTIO_DEVICE_NET);
    }

    #[test]
    fn net_queue_max_size() {
        let n = VirtioNet::new(default_mac());
        assert_eq!(n.queue_max_size(0), 256);
        assert_eq!(n.queue_max_size(1), 256);
        assert_eq!(n.queue_max_size(2), 0);
    }

    #[test]
    fn net_mac_config() {
        let mac = [0x52, 0x54, 0x00, 0xAB, 0xCD, 0xEF];
        let n = VirtioNet::new(mac);
        // Offset 0: bytes 0..3 as LE u32
        assert_eq!(n.read_config(0), u32::from_le_bytes([0x52, 0x54, 0x00, 0xAB]));
        // Offset 4: bytes 4..5 in low 16 bits + status in high 16 bits
        let val = n.read_config(4);
        assert_eq!((val & 0xFFFF) as u16, u16::from_le_bytes([0xCD, 0xEF]));
    }

    #[test]
    fn net_features_include_mac() {
        let n = VirtioNet::new(default_mac());
        assert!(n.device_features() & VIRTIO_NET_F_MAC != 0);
    }

    #[test]
    fn net_inject_and_pop() {
        let mut n = VirtioNet::new(default_mac());
        assert!(!n.has_rx_data());

        let frame = vec![0xFFu8; 64];
        n.inject_packet(frame.clone());
        assert!(n.has_rx_data());
        assert_eq!(n.rx_pending_count(), 1);

        let popped = n.pop_rx_frame().unwrap();
        assert_eq!(popped, frame);
        assert!(!n.has_rx_data());
    }

    #[test]
    fn net_reset_clears_state() {
        let mut n = VirtioNet::new(default_mac());
        n.inject_packet(vec![0u8; 32]);
        n.queue_notify(0);
        n.queue_notify(1);
        n.reset();
        assert!(!n.has_rx_data());
        assert!(!n.take_rx_pending());
        assert!(!n.take_tx_pending());
    }

    #[test]
    fn net_notify_pending_flags() {
        let mut n = VirtioNet::new(default_mac());
        n.queue_notify(0);
        assert!(n.take_rx_pending());
        assert!(!n.take_rx_pending());

        n.queue_notify(1);
        assert!(n.take_tx_pending());
        assert!(!n.take_tx_pending());
    }

    #[test]
    fn virtio_net_hdr_size() {
        assert_eq!(VIRTIO_NET_HDR_SIZE, 12);
    }
}
