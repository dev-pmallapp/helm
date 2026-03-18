//! VirtIO MMIO transport -- spec 4.2.2 register layout.
//!
//! Implements the VirtIO over MMIO transport v2 register interface. The
//! transport wraps a [`VirtioBackend`](super::VirtioBackend) and exposes
//! the standard MMIO register set that the guest driver uses to discover,
//! negotiate features, and operate virtqueues.
//!
//! # Register map (spec 4.2.2)
//!
//! | Offset | Name               | R/W | Description                    |
//! |--------|--------------------|-----|--------------------------------|
//! | 0x000  | MagicValue         | R   | 0x74726976 ("virt")            |
//! | 0x004  | Version            | R   | 2                              |
//! | 0x008  | DeviceID           | R   | VirtIO device type             |
//! | 0x00C  | VendorID           | R   | VirtIO vendor ID               |
//! | 0x010  | DeviceFeatures     | R   | Flags (selected page)          |
//! | 0x014  | DeviceFeaturesSel  | W   | Feature page selector          |
//! | 0x020  | DriverFeatures     | W   | Driver-accepted features       |
//! | 0x024  | DriverFeaturesSel  | W   | Feature page selector          |
//! | 0x030  | QueueSel           | W   | Virtqueue index selector       |
//! | 0x034  | QueueNumMax        | R   | Max queue size                 |
//! | 0x038  | QueueNum           | W   | Queue size (driver-set)        |
//! | 0x044  | QueueReady         | RW  | Queue ready flag               |
//! | 0x050  | QueueNotify        | W   | Queue notification (doorbell)  |
//! | 0x060  | InterruptStatus    | R   | Interrupt flags                |
//! | 0x064  | InterruptACK       | W   | Interrupt acknowledge          |
//! | 0x070  | Status             | RW  | Device status                  |
//! | 0x080  | QueueDescLow       | W   | Descriptor table addr (low)    |
//! | 0x084  | QueueDescHigh      | W   | Descriptor table addr (high)   |
//! | 0x090  | QueueDriverLow     | W   | Available ring addr (low)      |
//! | 0x094  | QueueDriverHigh    | W   | Available ring addr (high)     |
//! | 0x0A0  | QueueDeviceLow     | W   | Used ring addr (low)           |
//! | 0x0A4  | QueueDeviceHigh    | W   | Used ring addr (high)          |
//! | 0x0FC  | ConfigGeneration   | R   | Config space change counter    |
//! | 0x100+ | Config             | RW  | Device-specific config         |

use helm_devices::Device;
use helm_devices::InterruptPin;

use super::VirtioBackend;

// ── MMIO register offsets (spec 4.2.2) ──────────────────────────────────────

const MAGIC_VALUE: u64 = 0x000;
const VERSION: u64 = 0x004;
const DEVICE_ID: u64 = 0x008;
const VENDOR_ID: u64 = 0x00C;
const DEVICE_FEATURES: u64 = 0x010;
const DEVICE_FEATURES_SEL: u64 = 0x014;
const DRIVER_FEATURES: u64 = 0x020;
const DRIVER_FEATURES_SEL: u64 = 0x024;
const QUEUE_SEL: u64 = 0x030;
const QUEUE_NUM_MAX: u64 = 0x034;
const QUEUE_NUM: u64 = 0x038;
const QUEUE_READY: u64 = 0x044;
const QUEUE_NOTIFY: u64 = 0x050;
const INTERRUPT_STATUS: u64 = 0x060;
const INTERRUPT_ACK: u64 = 0x064;
const STATUS: u64 = 0x070;
const QUEUE_DESC_LOW: u64 = 0x080;
const QUEUE_DESC_HIGH: u64 = 0x084;
const QUEUE_DRIVER_LOW: u64 = 0x090;
const QUEUE_DRIVER_HIGH: u64 = 0x094;
const QUEUE_DEVICE_LOW: u64 = 0x0A0;
const QUEUE_DEVICE_HIGH: u64 = 0x0A4;
const CONFIG_GENERATION: u64 = 0x0FC;
const CONFIG_SPACE_START: u64 = 0x100;

/// Magic value that identifies a VirtIO MMIO device ("virt" in LE).
const VIRTIO_MAGIC: u32 = 0x74726976;

/// MMIO transport version (v2).
const VIRTIO_VERSION: u32 = 2;

// ── Device status bits ──────────────────────────────────────────────────────

/// Guest OS has found the device and recognized it.
pub const STATUS_ACKNOWLEDGE: u32 = 1;
/// Guest OS knows how to drive the device.
pub const STATUS_DRIVER: u32 = 2;
/// Driver is set up and ready to drive the device.
pub const STATUS_DRIVER_OK: u32 = 4;
/// Driver has acknowledged all features it understands.
pub const STATUS_FEATURES_OK: u32 = 8;
/// Something went wrong; device has given up on the driver.
pub const STATUS_DEVICE_NEEDS_RESET: u32 = 64;
/// Something went wrong in the guest; guest has given up on the device.
pub const STATUS_FAILED: u32 = 128;

// ── Queue state ─────────────────────────────────────────────────────────────

/// State for a single virtqueue.
#[derive(Debug, Default)]
struct QueueState {
    /// Queue size (number of descriptors).
    num: u32,
    /// True when the queue is ready for operation.
    ready: bool,
    /// Descriptor table physical address.
    desc_addr: u64,
    /// Available ring physical address.
    driver_addr: u64,
    /// Used ring physical address.
    device_addr: u64,
}

/// Maximum number of virtqueues supported.
const MAX_QUEUES: usize = 8;

// ── VirtioMmioTransport ─────────────────────────────────────────────────────

/// VirtIO MMIO transport device.
///
/// Wraps a [`VirtioBackend`] and exposes the standard VirtIO MMIO register
/// set. The transport handles feature negotiation, queue setup, and
/// interrupt management. Actual I/O processing is delegated to the backend.
pub struct VirtioMmioTransport {
    backend: Box<dyn VirtioBackend>,

    // ── Feature negotiation ──
    device_features_sel: u32,
    driver_features: [u32; 2], // Two pages of 32 bits each
    driver_features_sel: u32,

    // ── Queue management ──
    queue_sel: u32,
    queues: [QueueState; MAX_QUEUES],

    // ── Interrupt and status ──
    interrupt_status: u32,
    status: u32,
    config_generation: u32,

    /// Interrupt output pin.
    pub irq_out: InterruptPin,
}

impl VirtioMmioTransport {
    /// Create a new MMIO transport wrapping the given backend.
    pub fn new(backend: Box<dyn VirtioBackend>) -> Self {
        Self {
            backend,
            device_features_sel: 0,
            driver_features: [0; 2],
            driver_features_sel: 0,
            queue_sel: 0,
            queues: Default::default(),
            interrupt_status: 0,
            status: 0,
            config_generation: 0,
            irq_out: InterruptPin::new(),
        }
    }

    fn selected_queue(&self) -> usize {
        (self.queue_sel as usize).min(MAX_QUEUES - 1)
    }
}

impl Device for VirtioMmioTransport {
    fn read(&mut self, offset: u64, _size: usize) -> u64 {
        match offset {
            MAGIC_VALUE => VIRTIO_MAGIC as u64,
            VERSION => VIRTIO_VERSION as u64,
            DEVICE_ID => self.backend.device_type() as u64,
            VENDOR_ID => self.backend.vendor_id() as u64,
            DEVICE_FEATURES => {
                let features = self.backend.device_features();
                let page = self.device_features_sel;
                ((features >> (page * 32)) & 0xFFFF_FFFF) as u64
            }
            QUEUE_NUM_MAX => self.backend.queue_max_size(self.selected_queue()) as u64,
            QUEUE_READY => {
                let qi = self.selected_queue();
                self.queues[qi].ready as u64
            }
            INTERRUPT_STATUS => self.interrupt_status as u64,
            STATUS => self.status as u64,
            CONFIG_GENERATION => self.config_generation as u64,
            off if off >= CONFIG_SPACE_START => {
                let config_offset = (off - CONFIG_SPACE_START) as u32;
                self.backend.read_config(config_offset) as u64
            }
            _ => 0,
        }
    }

    fn write(&mut self, offset: u64, _size: usize, val: u64) {
        let val32 = val as u32;
        match offset {
            DEVICE_FEATURES_SEL => {
                self.device_features_sel = val32;
            }
            DRIVER_FEATURES => {
                let page = self.driver_features_sel as usize;
                if page < 2 {
                    self.driver_features[page] = val32;
                }
            }
            DRIVER_FEATURES_SEL => {
                self.driver_features_sel = val32;
            }
            QUEUE_SEL => {
                self.queue_sel = val32;
            }
            QUEUE_NUM => {
                let qi = self.selected_queue();
                self.queues[qi].num = val32;
            }
            QUEUE_READY => {
                let qi = self.selected_queue();
                self.queues[qi].ready = val32 != 0;
            }
            QUEUE_NOTIFY => {
                // Doorbell: notify the backend that queue `val32` has new buffers.
                self.backend.queue_notify(val32 as usize);
            }
            INTERRUPT_ACK => {
                self.interrupt_status &= !val32;
                if self.interrupt_status == 0 {
                    self.irq_out.deassert();
                }
            }
            STATUS => {
                if val32 == 0 {
                    // Reset
                    self.status = 0;
                    self.interrupt_status = 0;
                    for q in &mut self.queues {
                        *q = QueueState::default();
                    }
                    self.backend.reset();
                    self.irq_out.deassert();
                } else {
                    self.status = val32;
                }
            }
            QUEUE_DESC_LOW => {
                let qi = self.selected_queue();
                self.queues[qi].desc_addr =
                    (self.queues[qi].desc_addr & 0xFFFF_FFFF_0000_0000) | val32 as u64;
            }
            QUEUE_DESC_HIGH => {
                let qi = self.selected_queue();
                self.queues[qi].desc_addr =
                    (self.queues[qi].desc_addr & 0x0000_0000_FFFF_FFFF) | ((val32 as u64) << 32);
            }
            QUEUE_DRIVER_LOW => {
                let qi = self.selected_queue();
                self.queues[qi].driver_addr =
                    (self.queues[qi].driver_addr & 0xFFFF_FFFF_0000_0000) | val32 as u64;
            }
            QUEUE_DRIVER_HIGH => {
                let qi = self.selected_queue();
                self.queues[qi].driver_addr =
                    (self.queues[qi].driver_addr & 0x0000_0000_FFFF_FFFF) | ((val32 as u64) << 32);
            }
            QUEUE_DEVICE_LOW => {
                let qi = self.selected_queue();
                self.queues[qi].device_addr =
                    (self.queues[qi].device_addr & 0xFFFF_FFFF_0000_0000) | val32 as u64;
            }
            QUEUE_DEVICE_HIGH => {
                let qi = self.selected_queue();
                self.queues[qi].device_addr =
                    (self.queues[qi].device_addr & 0x0000_0000_FFFF_FFFF) | ((val32 as u64) << 32);
            }
            off if off >= CONFIG_SPACE_START => {
                let config_offset = (off - CONFIG_SPACE_START) as u32;
                self.backend.write_config(config_offset, val32);
                self.config_generation = self.config_generation.wrapping_add(1);
            }
            _ => {}
        }
    }

    fn region_size(&self) -> u64 {
        0x200 // 512 bytes: registers + config space
    }
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// Null VirtIO backend for testing the transport.
    struct NullBackend;

    impl VirtioBackend for NullBackend {
        fn device_type(&self) -> u32 {
            2 // block device
        }

        fn vendor_id(&self) -> u32 {
            0x554D4551 // "QEMU"
        }

        fn device_features(&self) -> u64 {
            0x0000_0001_0000_0001 // bit 0 and bit 32
        }

        fn queue_max_size(&self, _queue: usize) -> u32 {
            256
        }

        fn queue_notify(&mut self, _queue: usize) {}

        fn read_config(&self, _offset: u32) -> u32 {
            0
        }

        fn write_config(&mut self, _offset: u32, _val: u32) {}

        fn reset(&mut self) {}
    }

    #[test]
    fn magic_value() {
        let mut transport = VirtioMmioTransport::new(Box::new(NullBackend));
        assert_eq!(transport.read(MAGIC_VALUE, 4), 0x74726976);
    }

    #[test]
    fn version() {
        let mut transport = VirtioMmioTransport::new(Box::new(NullBackend));
        assert_eq!(transport.read(VERSION, 4), 2);
    }

    #[test]
    fn device_id() {
        let mut transport = VirtioMmioTransport::new(Box::new(NullBackend));
        assert_eq!(transport.read(DEVICE_ID, 4), 2);
    }

    #[test]
    fn feature_negotiation() {
        let mut transport = VirtioMmioTransport::new(Box::new(NullBackend));

        // Read features page 0
        transport.write(DEVICE_FEATURES_SEL, 4, 0);
        let f0 = transport.read(DEVICE_FEATURES, 4);
        assert_eq!(f0 & 1, 1); // bit 0 set

        // Read features page 1
        transport.write(DEVICE_FEATURES_SEL, 4, 1);
        let f1 = transport.read(DEVICE_FEATURES, 4);
        assert_eq!(f1 & 1, 1); // bit 32 set (page 1, bit 0)
    }

    #[test]
    fn status_reset() {
        let mut transport = VirtioMmioTransport::new(Box::new(NullBackend));

        // Set status
        transport.write(STATUS, 4, STATUS_ACKNOWLEDGE as u64);
        assert_eq!(transport.read(STATUS, 4), STATUS_ACKNOWLEDGE as u64);

        // Reset by writing 0
        transport.write(STATUS, 4, 0);
        assert_eq!(transport.read(STATUS, 4), 0);
    }

    #[test]
    fn queue_setup() {
        let mut transport = VirtioMmioTransport::new(Box::new(NullBackend));

        // Select queue 0
        transport.write(QUEUE_SEL, 4, 0);

        // Check max size
        let max = transport.read(QUEUE_NUM_MAX, 4);
        assert_eq!(max, 256);

        // Set queue size
        transport.write(QUEUE_NUM, 4, 128);

        // Set descriptor table address
        transport.write(QUEUE_DESC_LOW, 4, 0x1000);
        transport.write(QUEUE_DESC_HIGH, 4, 0);

        // Mark ready
        transport.write(QUEUE_READY, 4, 1);
        assert_eq!(transport.read(QUEUE_READY, 4), 1);
    }

    #[test]
    fn region_size() {
        let transport = VirtioMmioTransport::new(Box::new(NullBackend));
        assert_eq!(transport.region_size(), 0x200);
    }
}
