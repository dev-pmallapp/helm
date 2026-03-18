//! VirtIO device backend trait and MMIO transport.
//!
//! The VirtIO subsystem is split into two layers:
//!
//! - **Protocol** ([`proto`]): MMIO transport, split-ring virtqueue processor,
//!   and feature/device-type constants shared by all backends.
//! - **Backends**: concrete device implementations, one module per device class.
//!
//! | Module        | Device          | VirtIO type |
//! |---------------|-----------------|-------------|
//! | [`blk`]       | Block storage   | 2           |
//! | [`net`]       | Network card    | 1           |
//! | [`console`]   | Serial console  | 3           |
//! | [`rng`]       | Entropy source  | 4           |

// ── Protocol layer ──────────────────────────────────────────────────────────

pub mod proto;

// ── Device backends ─────────────────────────────────────────────────────────

pub mod blk;
pub mod console;
pub mod net;
pub mod rng;

// ── VirtioBackend trait ─────────────────────────────────────────────────────

/// Backend trait for VirtIO devices.
///
/// Each VirtIO device type (block, network, console, etc.) implements
/// this trait. The [`proto::transport::VirtioMmioTransport`] calls these
/// methods during device operation.
pub trait VirtioBackend: Send {
    /// Return the VirtIO device type ID (e.g., 1 = net, 2 = block).
    fn device_type(&self) -> u32;

    /// Return the vendor ID (typically 0x554D4551 for QEMU-style devices).
    fn vendor_id(&self) -> u32;

    /// Return the full 64-bit device feature flags.
    fn device_features(&self) -> u64;

    /// Return the maximum queue size for the given queue index.
    ///
    /// Returns 0 if the queue index is not supported by this device.
    fn queue_max_size(&self, queue: usize) -> u32;

    /// Called when the driver writes to the queue notification register.
    ///
    /// The backend should process available buffers in the given queue.
    fn queue_notify(&mut self, queue: usize);

    /// Read a 32-bit value from device-specific config space at `offset`.
    fn read_config(&self, offset: u32) -> u32;

    /// Write a 32-bit value to device-specific config space at `offset`.
    fn write_config(&mut self, offset: u32, val: u32);

    /// Reset the device to its initial state.
    fn reset(&mut self);
}

#[cfg(test)]
mod tests {
    use super::proto::features::*;
    use super::proto::transport::VirtioMmioTransport;
    use super::VirtioBackend;

    struct TestBackend {
        reset_count: u32,
        notify_count: u32,
    }

    impl TestBackend {
        fn new() -> Self {
            Self { reset_count: 0, notify_count: 0 }
        }
    }

    impl VirtioBackend for TestBackend {
        fn device_type(&self) -> u32 { VIRTIO_DEVICE_BLK }
        fn vendor_id(&self) -> u32 { VIRTIO_VENDOR_ID as u32 }
        fn device_features(&self) -> u64 { VIRTIO_F_VERSION_1 | VIRTIO_BLK_F_SIZE_MAX }
        fn queue_max_size(&self, _queue: usize) -> u32 { 128 }
        fn queue_notify(&mut self, _queue: usize) { self.notify_count += 1; }
        fn read_config(&self, _offset: u32) -> u32 { 0 }
        fn write_config(&mut self, _offset: u32, _val: u32) {}
        fn reset(&mut self) { self.reset_count += 1; }
    }

    #[test]
    fn backend_creates_transport() {
        use helm_devices::Device;

        let backend = TestBackend::new();
        let mut transport = VirtioMmioTransport::new(Box::new(backend));

        // Should read magic
        assert_eq!(transport.read(0x000, 4), 0x74726976);
        // Device type should be block (2)
        assert_eq!(transport.read(0x008, 4), VIRTIO_DEVICE_BLK as u64);
    }
}
