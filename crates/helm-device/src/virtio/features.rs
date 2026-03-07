//! VirtIO feature bit definitions per spec 1.4.
//!
//! Feature bits are negotiated between device and driver during
//! initialization. Bits 0–23 are device-specific; bits 24–37 are
//! reserved for the transport/queue; bits 38+ are reserved.

// ── Transport / common features (bits 24–41) ────────────────────────────────

/// Ring indirect descriptors.
pub const VIRTIO_F_RING_INDIRECT_DESC: u64 = 1 << 28;
/// Ring event index (suppressed notifications).
pub const VIRTIO_F_RING_EVENT_IDX: u64 = 1 << 29;
/// VirtIO 1.0+ (non-legacy) device.
pub const VIRTIO_F_VERSION_1: u64 = 1 << 32;
/// Device can be used on platforms with limited/translated DMA.
pub const VIRTIO_F_ACCESS_PLATFORM: u64 = 1 << 33;
/// Packed virtqueue layout.
pub const VIRTIO_F_RING_PACKED: u64 = 1 << 34;
/// Used/avail buffers are processed in order.
pub const VIRTIO_F_IN_ORDER: u64 = 1 << 35;
/// Memory ordering guarantees provided by platform.
pub const VIRTIO_F_ORDER_PLATFORM: u64 = 1 << 36;
/// SR-IOV VF support.
pub const VIRTIO_F_SR_IOV: u64 = 1 << 37;
/// Notification data field.
pub const VIRTIO_F_NOTIFICATION_DATA: u64 = 1 << 38;
/// Notification config data.
pub const VIRTIO_F_NOTIF_CONFIG_DATA: u64 = 1 << 39;
/// Virtqueue reset support.
pub const VIRTIO_F_RING_RESET: u64 = 1 << 40;
/// Admin virtqueue support.
pub const VIRTIO_F_ADMIN_VQ: u64 = 1 << 41;

// ── Device status bits ──────────────────────────────────────────────────────

/// Guest OS has found the device.
pub const VIRTIO_STATUS_ACKNOWLEDGE: u8 = 1;
/// Guest OS knows how to drive the device.
pub const VIRTIO_STATUS_DRIVER: u8 = 2;
/// Driver is set up and ready to drive the device.
pub const VIRTIO_STATUS_DRIVER_OK: u8 = 4;
/// Feature negotiation complete.
pub const VIRTIO_STATUS_FEATURES_OK: u8 = 8;
/// Device needs reset (unrecoverable error).
pub const VIRTIO_STATUS_DEVICE_NEEDS_RESET: u8 = 64;
/// Something went wrong in the guest; device gave up.
pub const VIRTIO_STATUS_FAILED: u8 = 128;

// ── Device type IDs (spec 5.*) ──────────────────────────────────────────────

pub const VIRTIO_DEV_NET: u32 = 1;
pub const VIRTIO_DEV_BLK: u32 = 2;
pub const VIRTIO_DEV_CONSOLE: u32 = 3;
pub const VIRTIO_DEV_RNG: u32 = 4;
pub const VIRTIO_DEV_BALLOON: u32 = 5;
pub const VIRTIO_DEV_IOMEM: u32 = 6;
pub const VIRTIO_DEV_RPMSG: u32 = 7;
pub const VIRTIO_DEV_SCSI: u32 = 8;
pub const VIRTIO_DEV_9P: u32 = 9;
pub const VIRTIO_DEV_WLAN: u32 = 10;
pub const VIRTIO_DEV_RPROC_SERIAL: u32 = 11;
pub const VIRTIO_DEV_CAIF: u32 = 12;
pub const VIRTIO_DEV_BALLOON_NEW: u32 = 13;
pub const VIRTIO_DEV_GPU: u32 = 16;
pub const VIRTIO_DEV_TIMER: u32 = 17;
pub const VIRTIO_DEV_INPUT: u32 = 18;
pub const VIRTIO_DEV_VSOCK: u32 = 19;
pub const VIRTIO_DEV_CRYPTO: u32 = 20;
pub const VIRTIO_DEV_SIGNAL: u32 = 21;
pub const VIRTIO_DEV_PSTORE: u32 = 22;
pub const VIRTIO_DEV_IOMMU: u32 = 23;
pub const VIRTIO_DEV_MEM: u32 = 24;
pub const VIRTIO_DEV_SOUND: u32 = 25;
pub const VIRTIO_DEV_FS: u32 = 26;
pub const VIRTIO_DEV_PMEM: u32 = 27;
pub const VIRTIO_DEV_RPMB: u32 = 28;
pub const VIRTIO_DEV_MAC80211_HWSIM: u32 = 29;
pub const VIRTIO_DEV_VIDEO_ENC: u32 = 30;
pub const VIRTIO_DEV_VIDEO_DEC: u32 = 31;
pub const VIRTIO_DEV_SCMI: u32 = 32;
pub const VIRTIO_DEV_NITRO: u32 = 33;
pub const VIRTIO_DEV_I2C: u32 = 34;
pub const VIRTIO_DEV_WATCHDOG: u32 = 35;
pub const VIRTIO_DEV_CAN: u32 = 36;
pub const VIRTIO_DEV_PARAM_SERV: u32 = 40;
pub const VIRTIO_DEV_AUDIO_POLICY: u32 = 41;
pub const VIRTIO_DEV_BT: u32 = 42;
pub const VIRTIO_DEV_GPIO: u32 = 43;
pub const VIRTIO_DEV_RDMA: u32 = 44;

// ── MMIO magic and version ──────────────────────────────────────────────────

/// Magic value at offset 0x000 ("virt" in little-endian).
pub const VIRTIO_MMIO_MAGIC: u32 = 0x7472_6976;
/// MMIO transport version for VirtIO 1.0+ (non-legacy).
pub const VIRTIO_MMIO_VERSION: u32 = 2;
/// Default vendor ID for HELM-simulated devices.
pub const HELM_VIRTIO_VENDOR_ID: u32 = 0x484C_4D00; // "HLM\0"

// ── Virtqueue descriptor flags ──────────────────────────────────────────────

/// Buffer continues via the `next` field.
pub const VRING_DESC_F_NEXT: u16 = 1;
/// Buffer is device-writable (read by device for writes, written for reads).
pub const VRING_DESC_F_WRITE: u16 = 2;
/// Buffer contains a table of indirect descriptors.
pub const VRING_DESC_F_INDIRECT: u16 = 4;

// ── Packed virtqueue flags ──────────────────────────────────────────────────

/// Descriptor available flag (packed vq).
pub const VRING_PACKED_DESC_F_AVAIL: u16 = 1 << 7;
/// Descriptor used flag (packed vq).
pub const VRING_PACKED_DESC_F_USED: u16 = 1 << 15;

// ── Interrupt status bits ───────────────────────────────────────────────────

/// Used buffer notification.
pub const VIRTIO_IRQ_VQUEUE: u32 = 1;
/// Configuration change notification.
pub const VIRTIO_IRQ_CONFIG: u32 = 2;

// ═══════════════════════════════════════════════════════════════════════════
// Per-device feature bits
// ═══════════════════════════════════════════════════════════════════════════

// ── Network device (type 1) ─────────────────────────────────────────────────

pub const VIRTIO_NET_F_CSUM: u64 = 1 << 0;
pub const VIRTIO_NET_F_GUEST_CSUM: u64 = 1 << 1;
pub const VIRTIO_NET_F_CTRL_GUEST_OFFLOADS: u64 = 1 << 2;
pub const VIRTIO_NET_F_MTU: u64 = 1 << 3;
pub const VIRTIO_NET_F_MAC: u64 = 1 << 5;
pub const VIRTIO_NET_F_GSO: u64 = 1 << 6;
pub const VIRTIO_NET_F_GUEST_TSO4: u64 = 1 << 7;
pub const VIRTIO_NET_F_GUEST_TSO6: u64 = 1 << 8;
pub const VIRTIO_NET_F_GUEST_ECN: u64 = 1 << 9;
pub const VIRTIO_NET_F_GUEST_UFO: u64 = 1 << 10;
pub const VIRTIO_NET_F_HOST_TSO4: u64 = 1 << 11;
pub const VIRTIO_NET_F_HOST_TSO6: u64 = 1 << 12;
pub const VIRTIO_NET_F_HOST_ECN: u64 = 1 << 13;
pub const VIRTIO_NET_F_HOST_UFO: u64 = 1 << 14;
pub const VIRTIO_NET_F_MRG_RXBUF: u64 = 1 << 15;
pub const VIRTIO_NET_F_STATUS: u64 = 1 << 16;
pub const VIRTIO_NET_F_CTRL_VQ: u64 = 1 << 17;
pub const VIRTIO_NET_F_CTRL_RX: u64 = 1 << 18;
pub const VIRTIO_NET_F_CTRL_VLAN: u64 = 1 << 19;
pub const VIRTIO_NET_F_GUEST_ANNOUNCE: u64 = 1 << 21;
pub const VIRTIO_NET_F_MQ: u64 = 1 << 22;
pub const VIRTIO_NET_F_CTRL_MAC_ADDR: u64 = 1 << 23;
pub const VIRTIO_NET_F_HASH_REPORT: u64 = 1 << 57;
pub const VIRTIO_NET_F_RSS: u64 = 1 << 60;
pub const VIRTIO_NET_F_RSC_EXT: u64 = 1 << 61;
pub const VIRTIO_NET_F_STANDBY: u64 = 1 << 62;
pub const VIRTIO_NET_F_SPEED_DUPLEX: u64 = 1 << 63;

// ── Block device (type 2) ───────────────────────────────────────────────────

pub const VIRTIO_BLK_F_SIZE_MAX: u64 = 1 << 1;
pub const VIRTIO_BLK_F_SEG_MAX: u64 = 1 << 2;
pub const VIRTIO_BLK_F_GEOMETRY: u64 = 1 << 4;
pub const VIRTIO_BLK_F_RO: u64 = 1 << 5;
pub const VIRTIO_BLK_F_BLK_SIZE: u64 = 1 << 6;
pub const VIRTIO_BLK_F_FLUSH: u64 = 1 << 9;
pub const VIRTIO_BLK_F_TOPOLOGY: u64 = 1 << 10;
pub const VIRTIO_BLK_F_CONFIG_WCE: u64 = 1 << 11;
pub const VIRTIO_BLK_F_MQ: u64 = 1 << 12;
pub const VIRTIO_BLK_F_DISCARD: u64 = 1 << 13;
pub const VIRTIO_BLK_F_WRITE_ZEROES: u64 = 1 << 14;
pub const VIRTIO_BLK_F_LIFETIME: u64 = 1 << 15;
pub const VIRTIO_BLK_F_SECURE_ERASE: u64 = 1 << 16;
pub const VIRTIO_BLK_F_ZONED: u64 = 1 << 17;

// ── Console device (type 3) ─────────────────────────────────────────────────

pub const VIRTIO_CONSOLE_F_SIZE: u64 = 1 << 0;
pub const VIRTIO_CONSOLE_F_MULTIPORT: u64 = 1 << 1;
pub const VIRTIO_CONSOLE_F_EMERG_WRITE: u64 = 1 << 2;

// ── Balloon device (type 5/13) ──────────────────────────────────────────────

pub const VIRTIO_BALLOON_F_MUST_TELL_HOST: u64 = 1 << 0;
pub const VIRTIO_BALLOON_F_STATS_VQ: u64 = 1 << 1;
pub const VIRTIO_BALLOON_F_DEFLATE_ON_OOM: u64 = 1 << 2;
pub const VIRTIO_BALLOON_F_FREE_PAGE_HINT: u64 = 1 << 3;
pub const VIRTIO_BALLOON_F_PAGE_POISON: u64 = 1 << 4;
pub const VIRTIO_BALLOON_F_REPORTING: u64 = 1 << 5;

// ── SCSI host (type 8) ─────────────────────────────────────────────────────

pub const VIRTIO_SCSI_F_INOUT: u64 = 1 << 0;
pub const VIRTIO_SCSI_F_HOTPLUG: u64 = 1 << 1;
pub const VIRTIO_SCSI_F_CHANGE: u64 = 1 << 2;
pub const VIRTIO_SCSI_F_T10_PI: u64 = 1 << 3;

// ── GPU device (type 16) ───────────────────────────────────────────────────

pub const VIRTIO_GPU_F_VIRGL: u64 = 1 << 0;
pub const VIRTIO_GPU_F_EDID: u64 = 1 << 1;
pub const VIRTIO_GPU_F_RESOURCE_UUID: u64 = 1 << 2;
pub const VIRTIO_GPU_F_RESOURCE_BLOB: u64 = 1 << 3;
pub const VIRTIO_GPU_F_CONTEXT_INIT: u64 = 1 << 4;

// ── Input device (type 18) ─────────────────────────────────────────────────
// No device-specific feature bits in spec 1.4.

// ── Vsock device (type 19) ─────────────────────────────────────────────────

pub const VIRTIO_VSOCK_F_STREAM: u64 = 1 << 0;
pub const VIRTIO_VSOCK_F_SEQPACKET: u64 = 1 << 1;

// ── Crypto device (type 20) ────────────────────────────────────────────────

pub const VIRTIO_CRYPTO_F_REVISION_1: u64 = 1 << 0;
pub const VIRTIO_CRYPTO_F_CIPHER_STATELESS_MODE: u64 = 1 << 1;
pub const VIRTIO_CRYPTO_F_HASH_STATELESS_MODE: u64 = 1 << 2;
pub const VIRTIO_CRYPTO_F_MAC_STATELESS_MODE: u64 = 1 << 3;
pub const VIRTIO_CRYPTO_F_AEAD_STATELESS_MODE: u64 = 1 << 4;

// ── IOMMU device (type 23) ─────────────────────────────────────────────────

pub const VIRTIO_IOMMU_F_INPUT_RANGE: u64 = 1 << 0;
pub const VIRTIO_IOMMU_F_DOMAIN_RANGE: u64 = 1 << 1;
pub const VIRTIO_IOMMU_F_MAP_UNMAP: u64 = 1 << 2;
pub const VIRTIO_IOMMU_F_BYPASS: u64 = 1 << 3;
pub const VIRTIO_IOMMU_F_PROBE: u64 = 1 << 4;
pub const VIRTIO_IOMMU_F_MMIO: u64 = 1 << 5;
pub const VIRTIO_IOMMU_F_BYPASS_CONFIG: u64 = 1 << 6;

// ── Memory device (type 24) ────────────────────────────────────────────────

pub const VIRTIO_MEM_F_ACPI_PXM: u64 = 1 << 0;
pub const VIRTIO_MEM_F_UNPLUGGED_INACCESSIBLE: u64 = 1 << 1;

// ── Sound device (type 25) ─────────────────────────────────────────────────
// No device-specific feature bits in spec 1.4.

// ── Filesystem device (type 26) ────────────────────────────────────────────

pub const VIRTIO_FS_F_NOTIFICATION: u64 = 1 << 0;

// ── PMEM device (type 27) ──────────────────────────────────────────────────

pub const VIRTIO_PMEM_F_SHMEM_REGION: u64 = 1 << 0;

// ── SCMI device (type 32) ──────────────────────────────────────────────────

pub const VIRTIO_SCMI_F_P2A_CHANNELS: u64 = 1 << 0;
pub const VIRTIO_SCMI_F_SHARED_MEMORY: u64 = 1 << 1;

// ── I2C adapter (type 34) ──────────────────────────────────────────────────

pub const VIRTIO_I2C_F_ZERO_LENGTH_REQUEST: u64 = 1 << 0;

// ── GPIO device (type 43) ──────────────────────────────────────────────────

pub const VIRTIO_GPIO_F_IRQ: u64 = 1 << 0;

// ── CAN device (type 36) ───────────────────────────────────────────────────

pub const VIRTIO_CAN_F_CAN_CLASSIC: u64 = 1 << 0;
pub const VIRTIO_CAN_F_CAN_FD: u64 = 1 << 1;
pub const VIRTIO_CAN_F_LATE_TX_ACK: u64 = 1 << 2;
pub const VIRTIO_CAN_F_RTR: u64 = 1 << 3;

// ── Watchdog device (type 35) ──────────────────────────────────────────────
// No device-specific feature bits in spec 1.4.

// ── Video encoder/decoder (types 30/31) ─────────────────────────────────────

pub const VIRTIO_VIDEO_F_RESOURCE_GUEST_PAGES: u64 = 1 << 0;
pub const VIRTIO_VIDEO_F_HOST_RESOURCE_GUEST_PAGES: u64 = 1 << 1;
pub const VIRTIO_VIDEO_F_RESOURCE_NON_CONTIG: u64 = 1 << 2;
pub const VIRTIO_VIDEO_F_RESOURCE_VIRTIO_OBJECT: u64 = 1 << 3;
