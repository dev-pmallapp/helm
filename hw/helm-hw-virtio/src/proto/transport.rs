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

use crate::proto::virtqueue::VirtQueue;
use crate::VirtioBackend;
use crate::VirtioPendingEvents;

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
    /// Shadow avail ring index already consumed by the device.
    last_avail_idx: u16,
    /// Used ring index already published by the device.
    used_idx: u16,
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

    /// Process any backend-latched queue work against guest memory.
    ///
    /// MMIO queue notifications cannot supply guest memory at write time, so
    /// backends typically latch a notify-pending bit in [`queue_notify`] and
    /// rely on the caller to invoke this helper once it has access to the live
    /// guest memory surface.
    pub fn process_pending(&mut self, mem: &mut dyn helm_core::ByteMem) -> VirtioPendingEvents {
        let mut queues = Vec::with_capacity(MAX_QUEUES);

        for queue in &self.queues {
            let size = if queue.ready && queue.num != 0 {
                queue.num as u16
            } else {
                1
            };
            queues.push(VirtQueue::new_with_progress(
                size,
                queue.desc_addr,
                queue.driver_addr,
                queue.device_addr,
                queue.last_avail_idx,
                queue.used_idx,
            ));
        }

        let events = self.backend.process_pending(mem, &mut queues);

        for (idx, queue) in queues.into_iter().enumerate() {
            let (last_avail_idx, used_idx) = queue.progress();
            self.queues[idx].last_avail_idx = last_avail_idx;
            self.queues[idx].used_idx = used_idx;
        }

        if events.queue_irq {
            self.interrupt_status |= 0x1;
        }
        if events.failed {
            // Guest-memory or queue-structure faults are surfaced as a transport
            // failure so the guest can observe that the device stopped making
            // forward progress on the queue.
            self.status |= STATUS_FAILED;
        }
        if events.config_irq || events.failed {
            self.interrupt_status |= 0x2;
            self.config_generation = self.config_generation.wrapping_add(1);
        }
        if self.interrupt_status != 0 {
            self.irq_out.assert();
        }

        events
    }

    /// Try to downcast the wrapped backend to a concrete type.
    pub fn backend_as_mut<T: crate::VirtioBackend + 'static>(&mut self) -> Option<&mut T> {
        self.backend.as_any_mut().downcast_mut::<T>()
    }
}

impl Device for VirtioMmioTransport {
    fn read(&mut self, offset: u64, size: usize) -> u64 {
        let Some(size) = supported_width(size) else {
            return 0;
        };
        if crosses_word_boundary(offset, size) {
            return 0;
        }
        let word = match offset & !0x3 {
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
                let config_offset = ((off - CONFIG_SPACE_START) & !0x3) as u32;
                self.backend.read_config(config_offset) as u64
            }
            _ => 0,
        };
        u64::from(extract_subword(word as u32, offset, size))
    }

    fn write(&mut self, offset: u64, size: usize, val: u64) {
        let Some(size) = supported_width(size) else {
            return;
        };
        if crosses_word_boundary(offset, size) {
            return;
        }
        let word_offset = offset & !0x3;
        let val32 = merge_subword(self.read(word_offset, 4) as u32, offset, size, val);
        match word_offset {
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
                // Transport does not have guest memory access — pass None.
                // Callers with memory access should use queue_notify directly.
                self.backend.queue_notify(val32 as usize, None);
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
                let config_offset = ((off - CONFIG_SPACE_START) & !0x3) as u32;
                self.backend.write_config(config_offset, val32);
                self.config_generation = self.config_generation.wrapping_add(1);
            }
            _ => {}
        }
    }

    fn region_size(&self) -> u64 {
        // Fixed at 512 bytes: 256 bytes of transport registers (0x000-0x0FC)
        // plus 256 bytes of device-specific config space (0x100-0x1FF).
        // This upper bound covers all current backends (blk: 12 B, net: 12 B,
        // console: 4 B, rng: 0 B). Backends with larger config spaces would
        // need a dynamic region_size or a higher fixed bound.
        0x200
    }
}

fn supported_width(size: usize) -> Option<usize> {
    match size {
        1 | 2 | 4 => Some(size),
        _ => None,
    }
}

fn crosses_word_boundary(offset: u64, size: usize) -> bool {
    (offset & 0x3) + size as u64 > 4
}

use helm_devices::{extract_subword, merge_subword};

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

        fn queue_notify(&mut self, _queue: usize, _mem: Option<&mut dyn helm_core::ByteMem>) {}

        fn read_config(&self, _offset: u32) -> u32 {
            0
        }

        fn write_config(&mut self, _offset: u32, _val: u32) {}

        fn reset(&mut self) {}

        fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
            self
        }
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

    struct ConfigBackend {
        config0: u32,
        writes: Vec<(u32, u32)>,
    }

    impl VirtioBackend for ConfigBackend {
        fn device_type(&self) -> u32 {
            2
        }

        fn vendor_id(&self) -> u32 {
            0x554D4551
        }

        fn device_features(&self) -> u64 {
            0
        }

        fn queue_max_size(&self, _queue: usize) -> u32 {
            64
        }

        fn queue_notify(&mut self, _queue: usize, _mem: Option<&mut dyn helm_core::ByteMem>) {}

        fn read_config(&self, offset: u32) -> u32 {
            match offset {
                0 => self.config0,
                _ => 0,
            }
        }

        fn write_config(&mut self, offset: u32, val: u32) {
            if offset == 0 {
                self.config0 = val;
            }
            self.writes.push((offset, val));
        }

        fn reset(&mut self) {}

        fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
            self
        }
    }

    #[test]
    fn config_space_subword_reads_use_little_endian_layout() {
        let mut transport = VirtioMmioTransport::new(Box::new(ConfigBackend {
            config0: 0x4433_2211,
            writes: Vec::new(),
        }));

        assert_eq!(transport.read(CONFIG_SPACE_START + 1, 1), 0x22);
        assert_eq!(transport.read(CONFIG_SPACE_START + 2, 2), 0x4433);
    }

    #[test]
    fn config_space_subword_writes_merge_into_backend_word() {
        let mut transport = VirtioMmioTransport::new(Box::new(ConfigBackend {
            config0: 0x4433_2211,
            writes: Vec::new(),
        }));

        transport.write(CONFIG_SPACE_START + 1, 2, 0xAABB);
        let backend = transport.backend_as_mut::<ConfigBackend>().unwrap();
        assert_eq!(backend.config0, 0x44AA_BB11);
        assert_eq!(backend.writes, vec![(0, 0x44AA_BB11)]);
    }

    #[test]
    fn queue_ready_byte_access_works() {
        let mut transport = VirtioMmioTransport::new(Box::new(NullBackend));
        transport.write(QUEUE_READY, 1, 1);
        assert_eq!(transport.read(QUEUE_READY, 1), 1);
    }

    #[test]
    fn identity_register_byte_reads() {
        let mut transport = VirtioMmioTransport::new(Box::new(NullBackend));

        // MagicValue = 0x74726976 ("virt" in LE)
        // byte 0 = 0x76, byte 1 = 0x69, byte 2 = 0x72, byte 3 = 0x74
        assert_eq!(transport.read(MAGIC_VALUE, 1), 0x76);
        assert_eq!(transport.read(MAGIC_VALUE + 1, 1), 0x69);
        assert_eq!(transport.read(MAGIC_VALUE + 2, 1), 0x72);
        assert_eq!(transport.read(MAGIC_VALUE + 3, 1), 0x74);

        // half-word reads
        assert_eq!(transport.read(MAGIC_VALUE, 2), 0x6976);
        assert_eq!(transport.read(MAGIC_VALUE + 2, 2), 0x7472);

        // DeviceID = 2 (block)
        assert_eq!(transport.read(DEVICE_ID, 1), 2);
        assert_eq!(transport.read(DEVICE_ID + 1, 1), 0);
    }

    #[test]
    fn interrupt_status_byte_read() {
        let mut transport = VirtioMmioTransport::new(Box::new(FailingBackend));
        let mut mem = helm_memory::FlatMem::new(0, 0);

        // Set up a queue so process_pending has something to work with
        transport.write(STATUS, 4, STATUS_ACKNOWLEDGE as u64);
        transport.write(QUEUE_SEL, 4, 0);
        transport.write(QUEUE_NUM, 4, 8);
        transport.write(QUEUE_READY, 4, 1);

        // FailingBackend sets failed=true which latches interrupt_status |= 0x2
        let events = transport.process_pending(&mut mem);
        assert!(events.failed);

        // Byte read should extract the low byte (0x2) correctly
        assert_eq!(transport.read(INTERRUPT_STATUS, 1), 0x2);
        // Half-word read should also work
        assert_eq!(transport.read(INTERRUPT_STATUS, 2), 0x2);
    }

    #[test]
    fn status_register_byte_write_and_read() {
        let mut transport = VirtioMmioTransport::new(Box::new(NullBackend));
        transport.write(STATUS, 1, STATUS_ACKNOWLEDGE as u64);
        assert_eq!(transport.read(STATUS, 1), STATUS_ACKNOWLEDGE as u64);
        assert_eq!(transport.read(STATUS, 4), STATUS_ACKNOWLEDGE as u64);
    }

    #[test]
    fn cross_word_boundary_read_returns_zero() {
        let mut transport = VirtioMmioTransport::new(Box::new(NullBackend));
        // Attempt a 2-byte read at offset 3 (crosses into next word)
        assert_eq!(transport.read(MAGIC_VALUE + 3, 2), 0);
        // 4-byte read at offset 1 also crosses
        assert_eq!(transport.read(MAGIC_VALUE + 1, 4), 0);
    }

    #[test]
    fn cross_word_boundary_write_is_ignored() {
        let mut transport = VirtioMmioTransport::new(Box::new(NullBackend));
        transport.write(STATUS, 4, STATUS_ACKNOWLEDGE as u64);
        // Attempt cross-boundary write; status should not change
        transport.write(STATUS + 3, 2, 0);
        assert_eq!(transport.read(STATUS, 4), STATUS_ACKNOWLEDGE as u64);
    }

    #[test]
    fn unsupported_width_read_returns_zero() {
        let mut transport = VirtioMmioTransport::new(Box::new(NullBackend));
        // size=3 is not supported
        assert_eq!(transport.read(MAGIC_VALUE, 3), 0);
        // size=8 is not supported
        assert_eq!(transport.read(MAGIC_VALUE, 8), 0);
    }

    #[test]
    fn unsupported_width_write_is_ignored() {
        let mut transport = VirtioMmioTransport::new(Box::new(NullBackend));
        transport.write(STATUS, 4, STATUS_ACKNOWLEDGE as u64);
        // size=3 write should be ignored
        transport.write(STATUS, 3, 0);
        assert_eq!(transport.read(STATUS, 4), STATUS_ACKNOWLEDGE as u64);
    }

    struct FailingBackend;

    impl VirtioBackend for FailingBackend {
        fn device_type(&self) -> u32 {
            2
        }

        fn vendor_id(&self) -> u32 {
            0x554D4551
        }

        fn device_features(&self) -> u64 {
            0
        }

        fn queue_max_size(&self, _queue: usize) -> u32 {
            8
        }

        fn queue_notify(&mut self, _queue: usize, _mem: Option<&mut dyn helm_core::ByteMem>) {}

        fn read_config(&self, _offset: u32) -> u32 {
            0
        }

        fn write_config(&mut self, _offset: u32, _val: u32) {}

        fn reset(&mut self) {}

        fn process_pending(
            &mut self,
            _mem: &mut dyn helm_core::ByteMem,
            _queues: &mut [VirtQueue],
        ) -> VirtioPendingEvents {
            VirtioPendingEvents {
                queue_irq: false,
                config_irq: false,
                failed: true,
            }
        }

        fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
            self
        }
    }

    #[test]
    fn process_pending_latches_failed_status_and_interrupt() {
        let mut transport = VirtioMmioTransport::new(Box::new(FailingBackend));
        let mut mem = helm_memory::FlatMem::new(0, 0);

        transport.write(STATUS, 4, STATUS_ACKNOWLEDGE as u64);
        transport.write(QUEUE_SEL, 4, 0);
        transport.write(QUEUE_NUM, 4, 8);
        transport.write(QUEUE_READY, 4, 1);

        let events = transport.process_pending(&mut mem);

        assert!(events.failed);
        assert_eq!(
            transport.read(STATUS, 4),
            (STATUS_ACKNOWLEDGE | STATUS_FAILED) as u64
        );
        assert_eq!(transport.read(INTERRUPT_STATUS, 4), 0x2);
    }
}
