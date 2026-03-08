//! VirtIO MMIO transport — register map per spec 4.2.2.
//!
//! This is the transport layer that sits between the guest driver and
//! the VirtIO device. The MMIO transport exposes a fixed register
//! layout; device-specific config space starts at offset 0x100.

use crate::device::{Device, DeviceEvent};
use crate::region::MemRegion;
use crate::transaction::Transaction;
use crate::virtio::features::*;
use crate::virtio::queue::Virtqueue;
use helm_core::HelmResult;
// ── MMIO register offsets (spec 4.2.2) ──────────────────────────────────────

const MMIO_MAGIC: u64 = 0x000;
const MMIO_VERSION: u64 = 0x004;
const MMIO_DEVICE_ID: u64 = 0x008;
const MMIO_VENDOR_ID: u64 = 0x00C;
const MMIO_DEVICE_FEATURES: u64 = 0x010;
const MMIO_DEVICE_FEATURES_SEL: u64 = 0x014;
const MMIO_DRIVER_FEATURES: u64 = 0x020;
const MMIO_DRIVER_FEATURES_SEL: u64 = 0x024;
const MMIO_QUEUE_SEL: u64 = 0x030;
const MMIO_QUEUE_NUM_MAX: u64 = 0x034;
const MMIO_QUEUE_NUM: u64 = 0x038;
const MMIO_QUEUE_READY: u64 = 0x044;
const MMIO_QUEUE_NOTIFY: u64 = 0x050;
const MMIO_INTERRUPT_STATUS: u64 = 0x060;
const MMIO_INTERRUPT_ACK: u64 = 0x064;
const MMIO_STATUS: u64 = 0x070;
const MMIO_QUEUE_DESC_LOW: u64 = 0x080;
const MMIO_QUEUE_DESC_HIGH: u64 = 0x084;
const MMIO_QUEUE_DRIVER_LOW: u64 = 0x090;
const MMIO_QUEUE_DRIVER_HIGH: u64 = 0x094;
const MMIO_QUEUE_DEVICE_LOW: u64 = 0x0A0;
const MMIO_QUEUE_DEVICE_HIGH: u64 = 0x0A4;
const MMIO_SHM_SEL: u64 = 0x0AC;
const MMIO_SHM_LEN_LOW: u64 = 0x0B0;
const MMIO_SHM_LEN_HIGH: u64 = 0x0B4;
const MMIO_SHM_BASE_LOW: u64 = 0x0B8;
const MMIO_SHM_BASE_HIGH: u64 = 0x0BC;
const MMIO_QUEUE_RESET: u64 = 0x0C0;
const MMIO_CONFIG_GEN: u64 = 0x0FC;
const MMIO_CONFIG_BASE: u64 = 0x100;

/// Size of the MMIO region for a VirtIO device.
pub const VIRTIO_MMIO_SIZE: u64 = 0x200;

// ── VirtIO device backend trait ─────────────────────────────────────────────

/// Trait that each VirtIO device type implements.
///
/// The transport handles register layout, feature negotiation, and queue
/// management. The backend handles device-specific logic.
pub trait VirtioDeviceBackend: Send + Sync {
    /// Device type ID (e.g. 1=net, 2=blk).
    fn device_id(&self) -> u32;

    /// Device feature bits (including common transport features).
    fn device_features(&self) -> u64;

    /// Size of the device-specific config space (bytes).
    fn config_size(&self) -> u32;

    /// Read a byte from device-specific config space.
    fn read_config(&self, offset: u32) -> u8;

    /// Write a byte to device-specific config space.
    fn write_config(&mut self, offset: u32, value: u8);

    /// Number of virtqueues this device uses.
    fn num_queues(&self) -> u16;

    /// Maximum queue size for a given queue index.
    fn queue_max_size(&self, queue_idx: u16) -> u16 {
        let _ = queue_idx;
        256
    }

    /// Called when the driver writes to QUEUE_NOTIFY.
    fn queue_notify(&mut self, queue_idx: u16, queues: &mut [Virtqueue]);

    /// Called when device status transitions to DRIVER_OK.
    fn activate(&mut self, _features: u64, _queues: &mut [Virtqueue]) {}

    /// Called on device reset.
    fn reset(&mut self) {}

    /// Called periodically to advance device state.
    fn tick(&mut self, _cycles: u64, _queues: &mut [Virtqueue]) -> Vec<DeviceEvent> {
        vec![]
    }

    /// Serialize device-specific state.
    fn checkpoint(&self) -> serde_json::Value {
        serde_json::Value::Null
    }

    /// Restore device-specific state.
    fn restore(&mut self, _state: &serde_json::Value) {}

    /// Human-readable name.
    fn name(&self) -> &str;
}

// ── MMIO Transport ──────────────────────────────────────────────────────────

/// VirtIO MMIO Transport (spec 4.2).
///
/// Wraps a [`VirtioDeviceBackend`] and provides the standard MMIO
/// register interface. Implements the [`Device`] trait so it can be
/// attached to a `DeviceBus`.
pub struct VirtioMmioTransport {
    backend: Box<dyn VirtioDeviceBackend>,
    queues: Vec<Virtqueue>,
    region: MemRegion,

    // Transport state
    status: u8,
    device_features_sel: u32,
    driver_features: u64,
    driver_features_sel: u32,
    queue_sel: u16,
    interrupt_status: u32,
    config_generation: u32,
    shm_sel: u32,
}

impl VirtioMmioTransport {
    pub fn new(backend: Box<dyn VirtioDeviceBackend>) -> Self {
        let num_queues = backend.num_queues();
        let queues: Vec<Virtqueue> = (0..num_queues)
            .map(|i| {
                let max = backend.queue_max_size(i);
                Virtqueue::new_split(max)
            })
            .collect();

        let region = MemRegion {
            name: format!("virtio-{}", backend.name()),
            base: 0,
            size: VIRTIO_MMIO_SIZE,
            kind: crate::region::RegionKind::Io,
            priority: 0,
        };

        Self {
            backend,
            queues,
            region,
            status: 0,
            device_features_sel: 0,
            driver_features: 0,
            driver_features_sel: 0,
            queue_sel: 0,
            interrupt_status: 0,
            config_generation: 0,
            shm_sel: 0,
        }
    }

    /// Access the backend.
    pub fn backend(&self) -> &dyn VirtioDeviceBackend {
        self.backend.as_ref()
    }

    /// Mutably access the backend.
    pub fn backend_mut(&mut self) -> &mut dyn VirtioDeviceBackend {
        self.backend.as_mut()
    }

    /// Access queues.
    pub fn queues(&self) -> &[Virtqueue] {
        &self.queues
    }

    /// Current device status.
    pub fn status(&self) -> u8 {
        self.status
    }

    /// Negotiated driver features.
    pub fn driver_features(&self) -> u64 {
        self.driver_features
    }

    /// Interrupt status register.
    pub fn interrupt_status(&self) -> u32 {
        self.interrupt_status
    }

    /// Assert an interrupt (used buffer notification).
    pub fn raise_irq(&mut self) {
        self.interrupt_status |= VIRTIO_IRQ_VQUEUE;
    }

    /// Assert a config change interrupt.
    pub fn raise_config_irq(&mut self) {
        self.interrupt_status |= VIRTIO_IRQ_CONFIG;
        self.config_generation = self.config_generation.wrapping_add(1);
    }

    fn selected_queue(&self) -> Option<&Virtqueue> {
        self.queues.get(self.queue_sel as usize)
    }

    fn selected_queue_mut(&mut self) -> Option<&mut Virtqueue> {
        self.queues.get_mut(self.queue_sel as usize)
    }

    fn handle_read(&self, offset: u64) -> u32 {
        match offset {
            MMIO_MAGIC => VIRTIO_MMIO_MAGIC,
            MMIO_VERSION => VIRTIO_MMIO_VERSION,
            MMIO_DEVICE_ID => self.backend.device_id(),
            MMIO_VENDOR_ID => HELM_VIRTIO_VENDOR_ID,

            MMIO_DEVICE_FEATURES => {
                let features = self.backend.device_features();
                if self.device_features_sel == 0 {
                    features as u32
                } else {
                    (features >> 32) as u32
                }
            }

            MMIO_QUEUE_NUM_MAX => self.selected_queue().map_or(0, |q| q.size() as u32),

            MMIO_QUEUE_READY => self.selected_queue().map_or(0, |q| q.ready() as u32),

            MMIO_INTERRUPT_STATUS => self.interrupt_status,
            MMIO_STATUS => self.status as u32,
            MMIO_CONFIG_GEN => self.config_generation,

            // Shared memory (optional)
            MMIO_SHM_LEN_LOW | MMIO_SHM_LEN_HIGH => 0xFFFF_FFFF, // not implemented
            MMIO_SHM_BASE_LOW | MMIO_SHM_BASE_HIGH => 0xFFFF_FFFF,

            // Device-specific config space
            offset if offset >= MMIO_CONFIG_BASE => {
                let cfg_off = (offset - MMIO_CONFIG_BASE) as u32;
                let cfg_size = self.backend.config_size();
                if cfg_off < cfg_size {
                    // Read up to 4 bytes
                    let mut val: u32 = 0;
                    for i in 0..4u32 {
                        if cfg_off + i < cfg_size {
                            val |= (self.backend.read_config(cfg_off + i) as u32) << (i * 8);
                        }
                    }
                    val
                } else {
                    0
                }
            }

            _ => 0,
        }
    }

    fn handle_write(&mut self, offset: u64, value: u32) {
        match offset {
            MMIO_DEVICE_FEATURES_SEL => {
                self.device_features_sel = value;
            }

            MMIO_DRIVER_FEATURES => {
                if self.driver_features_sel == 0 {
                    self.driver_features =
                        (self.driver_features & 0xFFFF_FFFF_0000_0000) | value as u64;
                } else {
                    self.driver_features =
                        (self.driver_features & 0x0000_0000_FFFF_FFFF) | ((value as u64) << 32);
                }
            }

            MMIO_DRIVER_FEATURES_SEL => {
                self.driver_features_sel = value;
            }

            MMIO_QUEUE_SEL => {
                self.queue_sel = value as u16;
            }

            MMIO_QUEUE_NUM => {
                if let Some(q) = self.selected_queue_mut() {
                    // Resize queue (only before QUEUE_READY)
                    if !q.ready() {
                        let new_size = value as u16;
                        *q = Virtqueue::new_split(new_size);
                    }
                }
            }

            MMIO_QUEUE_READY => {
                if let Some(q) = self.selected_queue_mut() {
                    q.set_ready(value != 0);
                }
            }

            MMIO_QUEUE_NOTIFY => {
                let queue_idx = value as u16;
                self.backend.queue_notify(queue_idx, &mut self.queues);
            }

            MMIO_INTERRUPT_ACK => {
                self.interrupt_status &= !value;
            }

            MMIO_STATUS => {
                let new_status = value as u8;
                if new_status == 0 {
                    // Reset
                    self.do_reset();
                } else {
                    let old = self.status;
                    self.status = new_status;
                    // Check for activation transition
                    if new_status & VIRTIO_STATUS_DRIVER_OK != 0
                        && old & VIRTIO_STATUS_DRIVER_OK == 0
                    {
                        self.backend
                            .activate(self.driver_features, &mut self.queues);
                    }
                }
            }

            MMIO_QUEUE_DESC_LOW => {
                if let Some(Virtqueue::Split(q)) = self.selected_queue_mut() {
                    q.desc_addr = (q.desc_addr & 0xFFFF_FFFF_0000_0000) | value as u64;
                }
            }
            MMIO_QUEUE_DESC_HIGH => {
                if let Some(Virtqueue::Split(q)) = self.selected_queue_mut() {
                    q.desc_addr = (q.desc_addr & 0x0000_0000_FFFF_FFFF) | ((value as u64) << 32);
                }
            }
            MMIO_QUEUE_DRIVER_LOW => {
                if let Some(Virtqueue::Split(q)) = self.selected_queue_mut() {
                    q.avail_addr = (q.avail_addr & 0xFFFF_FFFF_0000_0000) | value as u64;
                }
            }
            MMIO_QUEUE_DRIVER_HIGH => {
                if let Some(Virtqueue::Split(q)) = self.selected_queue_mut() {
                    q.avail_addr = (q.avail_addr & 0x0000_0000_FFFF_FFFF) | ((value as u64) << 32);
                }
            }
            MMIO_QUEUE_DEVICE_LOW => {
                if let Some(Virtqueue::Split(q)) = self.selected_queue_mut() {
                    q.used_addr = (q.used_addr & 0xFFFF_FFFF_0000_0000) | value as u64;
                }
            }
            MMIO_QUEUE_DEVICE_HIGH => {
                if let Some(Virtqueue::Split(q)) = self.selected_queue_mut() {
                    q.used_addr = (q.used_addr & 0x0000_0000_FFFF_FFFF) | ((value as u64) << 32);
                }
            }

            MMIO_SHM_SEL => {
                self.shm_sel = value;
            }

            MMIO_QUEUE_RESET => {
                if value == 1 {
                    if let Some(q) = self.selected_queue_mut() {
                        q.reset();
                    }
                }
            }

            // Device-specific config space
            offset if offset >= MMIO_CONFIG_BASE => {
                let cfg_off = (offset - MMIO_CONFIG_BASE) as u32;
                let cfg_size = self.backend.config_size();
                for i in 0..4u32 {
                    if cfg_off + i < cfg_size {
                        self.backend
                            .write_config(cfg_off + i, (value >> (i * 8)) as u8);
                    }
                }
            }

            _ => {}
        }
    }

    fn do_reset(&mut self) {
        self.status = 0;
        self.driver_features = 0;
        self.driver_features_sel = 0;
        self.device_features_sel = 0;
        self.queue_sel = 0;
        self.interrupt_status = 0;
        for q in &mut self.queues {
            q.reset();
        }
        self.backend.reset();
    }
}

// ── Device trait implementation ─────────────────────────────────────────────

impl Device for VirtioMmioTransport {
    fn transact(&mut self, txn: &mut Transaction) -> HelmResult<()> {
        let offset = txn.offset;
        if txn.is_write {
            let value = txn.data_u32();
            self.handle_write(offset, value);
        } else {
            let value = self.handle_read(offset);
            txn.set_data_u32(value);
        }
        txn.stall_cycles += 1; // 1-cycle register access
        Ok(())
    }

    fn regions(&self) -> &[MemRegion] {
        std::slice::from_ref(&self.region)
    }

    fn reset(&mut self) -> HelmResult<()> {
        self.do_reset();
        Ok(())
    }

    fn checkpoint(&self) -> HelmResult<serde_json::Value> {
        self.backend.checkpoint().pipe(Ok)
    }

    fn restore(&mut self, state: &serde_json::Value) -> HelmResult<()> {
        self.backend.restore(state);
        Ok(())
    }

    fn tick(&mut self, cycles: u64) -> HelmResult<Vec<DeviceEvent>> {
        Ok(self.backend.tick(cycles, &mut self.queues))
    }

    fn name(&self) -> &str {
        self.backend.name()
    }
}

/// Pipe helper (avoid nightly).
trait Pipe: Sized {
    fn pipe<F, R>(self, f: F) -> R
    where
        F: FnOnce(Self) -> R,
    {
        f(self)
    }
}
impl<T> Pipe for T {}
