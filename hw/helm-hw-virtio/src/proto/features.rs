//! VirtIO feature bit constants.
//!
//! Defined per the VirtIO specification v1.2, section 6.
//! Feature bits 0..23 are device-specific; bits 24..37 are reserved for
//! the transport; bits 38+ are reserved for future use.

// ── Transport features (bits 24..37) ────────────────────────────────────────

/// Negotiating this feature indicates that the driver can use indirect
/// descriptors (virtq_desc with VIRTQ_DESC_F_INDIRECT flag).
pub const VIRTIO_F_INDIRECT_DESC: u64 = 1 << 28;

/// Negotiating this feature indicates that the driver can use
/// VIRTQ_AVAIL_F_NO_INTERRUPT and VIRTQ_USED_F_NO_NOTIFY flags
/// for event suppression.
pub const VIRTIO_F_EVENT_IDX: u64 = 1 << 29;

/// VirtIO version 1.0+ compliance.
pub const VIRTIO_F_VERSION_1: u64 = 1 << 32;

/// The device can be used on a platform where accesses to memory regions
/// have limited atomicity.
pub const VIRTIO_F_ACCESS_PLATFORM: u64 = 1 << 33;

/// The device supports packed virtqueue layout.
pub const VIRTIO_F_RING_PACKED: u64 = 1 << 34;

/// The device can report in-order buffer use.
pub const VIRTIO_F_IN_ORDER: u64 = 1 << 35;

/// The device supports MMIO notification without writing a full 32-bit value.
pub const VIRTIO_F_NOTIFICATION_DATA: u64 = 1 << 38;

// ── Device-specific features (bits 0..23) ───────────────────────────────────

// Network device (device type 1)
/// Device has a MAC address feature.
pub const VIRTIO_NET_F_MAC: u64 = 1 << 5;
/// Device provides link status in config space.
pub const VIRTIO_NET_F_STATUS: u64 = 1 << 16;
/// Device supports multi-queue.
pub const VIRTIO_NET_F_MQ: u64 = 1 << 22;

// Block device (device type 2)
/// Maximum size of any single segment is in `size_max`.
pub const VIRTIO_BLK_F_SIZE_MAX: u64 = 1 << 1;
/// Maximum number of segments in a request is in `seg_max`.
pub const VIRTIO_BLK_F_SEG_MAX: u64 = 1 << 2;
/// Disk-style geometry specified in geometry.
pub const VIRTIO_BLK_F_GEOMETRY: u64 = 1 << 4;
/// Device is read-only.
pub const VIRTIO_BLK_F_RO: u64 = 1 << 5;
/// Block size of the disk is available.
pub const VIRTIO_BLK_F_BLK_SIZE: u64 = 1 << 6;
/// Device supports multi-queue.
pub const VIRTIO_BLK_F_MQ: u64 = 1 << 12;

// Console device (device type 3)
/// Console size (cols, rows) is available.
pub const VIRTIO_CONSOLE_F_SIZE: u64 = 1 << 0;
/// Device supports multiple ports.
pub const VIRTIO_CONSOLE_F_MULTIPORT: u64 = 1 << 1;

// ── Device type IDs ─────────────────────────────────────────────────────────

/// Network card.
pub const VIRTIO_DEVICE_NET: u32 = 1;
/// Block device.
pub const VIRTIO_DEVICE_BLK: u32 = 2;
/// Console.
pub const VIRTIO_DEVICE_CONSOLE: u32 = 3;
/// Entropy source.
pub const VIRTIO_DEVICE_RNG: u32 = 4;
/// Memory balloon.
pub const VIRTIO_DEVICE_BALLOON: u32 = 5;
/// SCSI host.
pub const VIRTIO_DEVICE_SCSI: u32 = 8;
/// GPU device.
pub const VIRTIO_DEVICE_GPU: u32 = 16;
/// Input device.
pub const VIRTIO_DEVICE_INPUT: u32 = 18;
/// Socket device.
pub const VIRTIO_DEVICE_VSOCK: u32 = 19;

// ── VirtIO vendor ID ────────────────────────────────────────────────────────

/// Standard VirtIO vendor ID used in PCI config space.
pub const VIRTIO_VENDOR_ID: u16 = 0x1AF4;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn feature_bits_non_overlapping() {
        // Transport features should not overlap device features
        assert_eq!(VIRTIO_F_INDIRECT_DESC & 0x00FF_FFFF, 0);
        assert_eq!(VIRTIO_F_EVENT_IDX & 0x00FF_FFFF, 0);
        assert_eq!(VIRTIO_F_VERSION_1 & 0x00FF_FFFF, 0);
    }

    #[test]
    fn device_type_ids() {
        assert_eq!(VIRTIO_DEVICE_NET, 1);
        assert_eq!(VIRTIO_DEVICE_BLK, 2);
        assert_eq!(VIRTIO_DEVICE_CONSOLE, 3);
    }
}
