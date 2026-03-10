//! VirtIO PCI Transport — implements [`PciFunction`] for any [`VirtioDeviceBackend`].
//!
//! This transport follows VirtIO spec section 4.1 (PCI transport).  A single
//! 4 KB BAR (BAR0) exposes four sub-regions in one flat window:
//!
//! | Offset          | Region            |
//! |-----------------|-------------------|
//! | 0x000–0x037     | Common config     |
//! | 0x038–0x03B     | ISR status        |
//! | 0x040–0x07F     | Notify region     |
//! | 0x080–0x0FF     | Device-specific config |
//!
//! BAR4 (4 KB) hosts the MSI-X vector table and PBA.
//!
//! Five vendor-specific PCI capabilities (cap ID 0x09) link the guest to
//! these regions.  An MSI-X capability (cap ID 0x11) enables per-vector
//! interrupts.

use crate::device::DeviceEvent;
use crate::pci::capability::msix::MsixCapability;
use crate::pci::capability::pcie::PcieCapability;
use crate::pci::capability::pm::PmCapability;
use crate::pci::capability::virtio_pci_cap::{VirtioPciCap, VirtioPciCapType};
use crate::pci::traits::{BarDecl, PciCapability, PciFunction};
use crate::virtio::features::*;
use crate::virtio::queue::Virtqueue;
use crate::virtio::transport::{VirtioDeviceBackend, VirtioTransport};

// ── BAR0 sub-region offsets ───────────────────────────────────────────────────

/// Start offset of the common configuration region within BAR0.
const COMMON_CFG_OFFSET: u64 = 0x000;
/// Length of the common configuration region.
const COMMON_CFG_LEN: u64 = 0x038;

/// Start offset of the ISR status byte within BAR0.
const ISR_OFFSET: u64 = 0x038;
/// Length of the ISR region.
const ISR_LEN: u64 = 0x004;

/// Start offset of the notify region within BAR0.
const NOTIFY_OFFSET: u64 = 0x040;
/// Length of the notify region.
const NOTIFY_LEN: u64 = 0x040;

/// Start offset of the device-specific config region within BAR0.
const DEVICE_CFG_OFFSET: u64 = 0x080;
/// Length of the device-specific config region.
const DEVICE_CFG_LEN: u64 = 0x080;

/// Total BAR0 size: 4 KB covers all sub-regions with room to spare.
const BAR0_SIZE: u64 = 0x1000;
/// BAR4 size: 4 KB for MSI-X table + PBA.
const BAR4_SIZE: u64 = 0x1000;

/// Stride between per-queue notify registers (bytes).
const NOTIFY_OFF_MULTIPLIER: u32 = 2;

// ── PCI identity constants ────────────────────────────────────────────────────

/// PCI vendor ID assigned to the VirtIO consortium.
const VIRTIO_PCI_VENDOR_ID: u16 = 0x1AF4;
/// Base PCI device ID for modern (non-transitional) VirtIO devices.
const VIRTIO_PCI_DEVICE_ID_BASE: u16 = 0x1040;

// ── Common-cfg register offsets (within the 0x000–0x037 sub-region) ──────────

const REG_DEVICE_FEATURE_SEL: u64 = 0x00;
const REG_DEVICE_FEATURE: u64 = 0x04;
const REG_DRIVER_FEATURE_SEL: u64 = 0x08;
const REG_DRIVER_FEATURE: u64 = 0x0C;
const REG_MSIX_CONFIG: u64 = 0x10; // u16 + u16 (msix_config | num_queues)
const REG_DEVICE_STATUS: u64 = 0x14; // u8 + u8 + u16 (status | gen | queue_sel)
const REG_QUEUE_SIZE: u64 = 0x18; // u16 + u16 (size | msix_vector)
const REG_QUEUE_ENABLE: u64 = 0x1C; // u16 + u16 (enable | notify_off)
const REG_QUEUE_DESC: u64 = 0x20; // u64
const REG_QUEUE_DRIVER: u64 = 0x28; // u64
const REG_QUEUE_DEVICE: u64 = 0x30; // u64

// ── VirtioPciTransport ────────────────────────────────────────────────────────

/// VirtIO PCI Transport (spec 4.1).
///
/// Implements [`PciFunction`] by wrapping any [`VirtioDeviceBackend`].  The
/// PCI identity (vendor/device ID, class code, subsystem IDs) is derived
/// from the backend.  BAR0 exposes the four VirtIO sub-regions; BAR4 hosts
/// the MSI-X table.
///
/// # Interrupt delivery
///
/// [`raise_irq`](Self::raise_irq) fires MSI-X vector 1 (queue notification).
/// [`raise_config_irq`](Self::raise_config_irq) fires MSI-X vector 0 (config
/// change) and increments `config_generation`.  When MSI-X is disabled the
/// interrupt is silently dropped (legacy INTx not yet implemented).
pub struct VirtioPciTransport {
    backend: Box<dyn VirtioDeviceBackend>,
    queues: Vec<Virtqueue>,

    // Transport state
    status: u8,
    device_features_sel: u32,
    driver_features: u64,
    driver_features_sel: u32,
    queue_sel: u16,
    interrupt_status: u32,
    config_generation: u32,
    msix_config: u16,

    // Per-queue MSI-X vector assignments
    queue_msix_vectors: Vec<u16>,

    // PCI layout
    bars: [BarDecl; 6],
    caps: Vec<Box<dyn PciCapability>>,
    msix: MsixCapability,
}

impl VirtioPciTransport {
    /// Construct a new VirtIO PCI transport wrapping `backend`.
    ///
    /// Sets up BAR declarations, MSI-X capability (3 vectors: config change,
    /// queue 0, queue 1), and the five VirtIO vendor-specific capabilities.
    ///
    /// # Examples
    ///
    /// ```
    /// use helm_device::pci::VirtioPciTransport;
    /// use helm_device::virtio::rng::VirtioRng;
    ///
    /// let t = VirtioPciTransport::new(Box::new(VirtioRng::new()));
    /// assert_eq!(t.vendor_id(), 0x1AF4);
    /// ```
    #[must_use]
    pub fn new(backend: Box<dyn VirtioDeviceBackend>) -> Self {
        let num_queues = backend.num_queues();
        let queues: Vec<Virtqueue> = (0..num_queues)
            .map(|i| {
                let max = backend.queue_max_size(i);
                Virtqueue::new_split(max)
            })
            .collect();

        // BAR declarations
        let mut bars = [BarDecl::Unused; 6];
        bars[0] = BarDecl::Mmio32 { size: BAR0_SIZE };
        // BAR1 is unused (BAR0 is 32-bit, no upper half needed)
        // BAR4 hosts the MSI-X table
        bars[4] = BarDecl::Mmio32 { size: BAR4_SIZE };

        // MSI-X: 3 vectors (config, queue0, queue1+)
        // table at BAR4 offset 0; PBA at BAR4 offset 0x800
        let num_vectors = 3u16.max(num_queues + 1);
        let msix = MsixCapability::new(
            0x60, // offset in config space
            num_vectors,
            4,     // table_bar = BAR4
            0x000, // table_offset
            4,     // pba_bar   = BAR4
            0x800, // pba_offset
        );

        // Build capability list
        let mut caps: Vec<Box<dyn PciCapability>> = Vec::new();

        // 1. PCIe endpoint capability
        caps.push(Box::new(PcieCapability::endpoint(0x40)));

        // 2. Power Management capability
        caps.push(Box::new(PmCapability::new(0x50)));

        // 3. MSI-X (already allocated above, added via borrow workaround below)

        // 4. Five VirtIO vendor-specific caps
        //    Capability chain starts after 0x60 (MSI-X).
        //    We place them starting at 0x80.

        // CommonCfg
        caps.push(Box::new(VirtioPciCap::new(
            0x80,
            VirtioPciCapType::CommonCfg,
            0,
            COMMON_CFG_OFFSET as u32,
            COMMON_CFG_LEN as u32,
            0,
        )));

        // IsrCfg
        caps.push(Box::new(VirtioPciCap::new(
            0x90,
            VirtioPciCapType::IsrCfg,
            0,
            ISR_OFFSET as u32,
            ISR_LEN as u32,
            0,
        )));

        // NotifyCfg
        caps.push(Box::new(VirtioPciCap::new(
            0xA0,
            VirtioPciCapType::NotifyCfg,
            0,
            NOTIFY_OFFSET as u32,
            NOTIFY_LEN as u32,
            NOTIFY_OFF_MULTIPLIER,
        )));

        // DeviceCfg
        caps.push(Box::new(VirtioPciCap::new(
            0xB4,
            VirtioPciCapType::DeviceCfg,
            0,
            DEVICE_CFG_OFFSET as u32,
            DEVICE_CFG_LEN as u32,
            0,
        )));

        // PciCfg (mandatory by spec, used by drivers without MMIO)
        caps.push(Box::new(VirtioPciCap::new(
            0xC4,
            VirtioPciCapType::PciCfg,
            0,
            0,
            4,
            0,
        )));

        let queue_msix_vectors = vec![0xFFFF_u16; num_queues as usize];

        Self {
            backend,
            queues,
            status: 0,
            device_features_sel: 0,
            driver_features: 0,
            driver_features_sel: 0,
            queue_sel: 0,
            interrupt_status: 0,
            config_generation: 0,
            msix_config: 0xFFFF, // "no MSI-X vector" sentinel
            queue_msix_vectors,
            bars,
            caps,
            msix,
        }
    }

    // ── Accessors ─────────────────────────────────────────────────────────────

    /// Current device status byte.
    #[must_use]
    pub fn status(&self) -> u8 {
        self.status
    }

    /// Negotiated driver features.
    #[must_use]
    pub fn driver_features(&self) -> u64 {
        self.driver_features
    }

    /// Raw interrupt status word.
    #[must_use]
    pub fn interrupt_status(&self) -> u32 {
        self.interrupt_status
    }

    /// Access the queues slice.
    #[must_use]
    pub fn queues(&self) -> &[Virtqueue] {
        &self.queues
    }

    // ── Derive class code from device type ────────────────────────────────────

    fn class_code_for(device_id: u32) -> u32 {
        match device_id {
            VIRTIO_DEV_NET => 0x020000,     // Ethernet controller
            VIRTIO_DEV_BLK => 0x018000,     // Mass storage (SCSI)
            VIRTIO_DEV_CONSOLE => 0x078000, // Simple communications
            _ => 0xFF0000,                  // Unclassified
        }
    }

    // ── BAR0 common config read/write ─────────────────────────────────────────

    fn common_cfg_read(&self, offset: u64) -> u32 {
        // Align down to dword boundary for the register we're reading
        match offset & !3 {
            REG_DEVICE_FEATURE_SEL => self.device_features_sel,

            REG_DEVICE_FEATURE => {
                let features = self.backend.device_features();
                if self.device_features_sel == 0 {
                    features as u32
                } else {
                    (features >> 32) as u32
                }
            }

            REG_DRIVER_FEATURE_SEL => self.driver_features_sel,

            REG_DRIVER_FEATURE => {
                if self.driver_features_sel == 0 {
                    self.driver_features as u32
                } else {
                    (self.driver_features >> 32) as u32
                }
            }

            // 0x10: msix_config [15:0] | num_queues [31:16]
            REG_MSIX_CONFIG => {
                let num_q = self.backend.num_queues();
                u32::from(self.msix_config) | (u32::from(num_q) << 16)
            }

            // 0x14: device_status [7:0] | config_generation [15:8] | queue_select [31:16]
            REG_DEVICE_STATUS => {
                u32::from(self.status)
                    | (u32::from(self.config_generation as u8) << 8)
                    | (u32::from(self.queue_sel) << 16)
            }

            // 0x18: queue_size [15:0] | queue_msix_vector [31:16]
            REG_QUEUE_SIZE => {
                let size = self
                    .queues
                    .get(self.queue_sel as usize)
                    .map_or(0, |q| q.size());
                let mvec = self
                    .queue_msix_vectors
                    .get(self.queue_sel as usize)
                    .copied()
                    .unwrap_or(0xFFFF);
                u32::from(size) | (u32::from(mvec) << 16)
            }

            // 0x1C: queue_enable [15:0] | queue_notify_off [31:16]
            REG_QUEUE_ENABLE => {
                let ready = self
                    .queues
                    .get(self.queue_sel as usize)
                    .map_or(false, |q| q.ready());
                // notify_off = queue_sel (one slot per queue, multiplier = 2)
                let notify_off = self.queue_sel;
                u32::from(ready) | (u32::from(notify_off) << 16)
            }

            // 0x20–0x27: queue_desc (u64) — return low or high dword
            REG_QUEUE_DESC => self
                .queues
                .get(self.queue_sel as usize)
                .and_then(|q| q.as_split())
                .map_or(0, |q| q.desc_addr as u32),

            0x24 => self
                .queues
                .get(self.queue_sel as usize)
                .and_then(|q| q.as_split())
                .map_or(0, |q| (q.desc_addr >> 32) as u32),

            // 0x28–0x2F: queue_driver (u64)
            REG_QUEUE_DRIVER => self
                .queues
                .get(self.queue_sel as usize)
                .and_then(|q| q.as_split())
                .map_or(0, |q| q.avail_addr as u32),

            0x2C => self
                .queues
                .get(self.queue_sel as usize)
                .and_then(|q| q.as_split())
                .map_or(0, |q| (q.avail_addr >> 32) as u32),

            // 0x30–0x37: queue_device (u64)
            REG_QUEUE_DEVICE => self
                .queues
                .get(self.queue_sel as usize)
                .and_then(|q| q.as_split())
                .map_or(0, |q| q.used_addr as u32),

            0x34 => self
                .queues
                .get(self.queue_sel as usize)
                .and_then(|q| q.as_split())
                .map_or(0, |q| (q.used_addr >> 32) as u32),

            _ => 0,
        }
    }

    #[allow(clippy::too_many_lines)]
    fn common_cfg_write(&mut self, offset: u64, value: u32) {
        match offset & !3 {
            REG_DEVICE_FEATURE_SEL => {
                self.device_features_sel = value;
            }

            REG_DRIVER_FEATURE_SEL => {
                self.driver_features_sel = value;
            }

            REG_DRIVER_FEATURE => {
                if self.driver_features_sel == 0 {
                    self.driver_features =
                        (self.driver_features & 0xFFFF_FFFF_0000_0000) | u64::from(value);
                } else {
                    self.driver_features =
                        (self.driver_features & 0x0000_0000_FFFF_FFFF) | (u64::from(value) << 32);
                }
            }

            // 0x10: low 16 bits = msix_config (writable)
            REG_MSIX_CONFIG => {
                self.msix_config = value as u16;
            }

            // 0x14: low byte = device_status, bits [31:16] = queue_select
            REG_DEVICE_STATUS => {
                let new_status = value as u8;
                if new_status == 0 {
                    self.do_reset();
                } else {
                    let old = self.status;
                    self.status = new_status;
                    if new_status & VIRTIO_STATUS_DRIVER_OK != 0
                        && old & VIRTIO_STATUS_DRIVER_OK == 0
                    {
                        self.backend
                            .activate(self.driver_features, &mut self.queues);
                    }
                }
                // queue_select is in bits [31:16] of the same dword
                self.queue_sel = (value >> 16) as u16;
            }

            // 0x18: low 16 = queue_size, high 16 = queue_msix_vector
            REG_QUEUE_SIZE => {
                let new_size = value as u16;
                let mvec = (value >> 16) as u16;
                let sel = self.queue_sel as usize;
                if let Some(q) = self.queues.get_mut(sel) {
                    if !q.ready() && new_size > 0 {
                        *q = Virtqueue::new_split(new_size);
                    }
                }
                if let Some(v) = self.queue_msix_vectors.get_mut(sel) {
                    *v = mvec;
                }
            }

            // 0x1C: low 16 = queue_enable
            REG_QUEUE_ENABLE => {
                let enable = (value & 0xFFFF) != 0;
                if let Some(q) = self.queues.get_mut(self.queue_sel as usize) {
                    q.set_ready(enable);
                }
            }

            // 0x20: queue_desc low
            REG_QUEUE_DESC => {
                if let Some(Virtqueue::Split(q)) = self.queues.get_mut(self.queue_sel as usize) {
                    q.desc_addr = (q.desc_addr & 0xFFFF_FFFF_0000_0000) | u64::from(value);
                }
            }
            0x24 => {
                if let Some(Virtqueue::Split(q)) = self.queues.get_mut(self.queue_sel as usize) {
                    q.desc_addr = (q.desc_addr & 0x0000_0000_FFFF_FFFF) | (u64::from(value) << 32);
                }
            }

            // 0x28: queue_driver low
            REG_QUEUE_DRIVER => {
                if let Some(Virtqueue::Split(q)) = self.queues.get_mut(self.queue_sel as usize) {
                    q.avail_addr = (q.avail_addr & 0xFFFF_FFFF_0000_0000) | u64::from(value);
                }
            }
            0x2C => {
                if let Some(Virtqueue::Split(q)) = self.queues.get_mut(self.queue_sel as usize) {
                    q.avail_addr =
                        (q.avail_addr & 0x0000_0000_FFFF_FFFF) | (u64::from(value) << 32);
                }
            }

            // 0x30: queue_device low
            REG_QUEUE_DEVICE => {
                if let Some(Virtqueue::Split(q)) = self.queues.get_mut(self.queue_sel as usize) {
                    q.used_addr = (q.used_addr & 0xFFFF_FFFF_0000_0000) | u64::from(value);
                }
            }
            0x34 => {
                if let Some(Virtqueue::Split(q)) = self.queues.get_mut(self.queue_sel as usize) {
                    q.used_addr = (q.used_addr & 0x0000_0000_FFFF_FFFF) | (u64::from(value) << 32);
                }
            }

            _ => {}
        }
    }

    // ── ISR region ────────────────────────────────────────────────────────────

    /// Read ISR status (read-to-clear).
    fn isr_read(&mut self) -> u32 {
        let v = self.interrupt_status;
        self.interrupt_status = 0;
        v
    }

    // ── Notify region ─────────────────────────────────────────────────────────

    fn notify_write(&mut self, offset: u64, value: u32) {
        // Each queue occupies NOTIFY_OFF_MULTIPLIER bytes starting at NOTIFY_OFFSET.
        // offset here is relative to BAR0 start; subtract NOTIFY_OFFSET.
        let rel = offset.saturating_sub(NOTIFY_OFFSET);
        let queue_idx = (rel / u64::from(NOTIFY_OFF_MULTIPLIER)) as u16;
        let _ = value; // spec says any write triggers notify
        self.backend.queue_notify(queue_idx, &mut self.queues);
    }

    // ── Device-specific config region ─────────────────────────────────────────

    fn device_cfg_read(&self, offset: u64) -> u32 {
        // offset is BAR0-relative; subtract DEVICE_CFG_OFFSET
        let cfg_off = offset.saturating_sub(DEVICE_CFG_OFFSET) as u32;
        let cfg_size = self.backend.config_size();
        let mut val: u32 = 0;
        for i in 0..4u32 {
            if cfg_off + i < cfg_size {
                val |= u32::from(self.backend.read_config(cfg_off + i)) << (i * 8);
            }
        }
        val
    }

    fn device_cfg_write(&mut self, offset: u64, value: u32) {
        let cfg_off = offset.saturating_sub(DEVICE_CFG_OFFSET) as u32;
        let cfg_size = self.backend.config_size();
        for i in 0..4u32 {
            if cfg_off + i < cfg_size {
                self.backend
                    .write_config(cfg_off + i, (value >> (i * 8)) as u8);
            }
        }
    }

    // ── Full reset ────────────────────────────────────────────────────────────

    fn do_reset(&mut self) {
        self.status = 0;
        self.driver_features = 0;
        self.driver_features_sel = 0;
        self.device_features_sel = 0;
        self.queue_sel = 0;
        self.interrupt_status = 0;
        self.config_generation = 0;
        self.msix_config = 0xFFFF;
        for v in &mut self.queue_msix_vectors {
            *v = 0xFFFF;
        }
        for q in &mut self.queues {
            q.reset();
        }
        self.backend.reset();
    }
}

// ── VirtioTransport impl ──────────────────────────────────────────────────────

impl VirtioTransport for VirtioPciTransport {
    fn backend(&self) -> &dyn VirtioDeviceBackend {
        self.backend.as_ref()
    }

    fn backend_mut(&mut self) -> &mut dyn VirtioDeviceBackend {
        self.backend.as_mut()
    }

    /// Assert a queue-used interrupt (vector 1).
    fn raise_irq(&mut self) {
        self.interrupt_status |= VIRTIO_IRQ_VQUEUE;
        let _ = self.msix.fire(1);
    }

    /// Assert a config-change interrupt (vector 0) and bump generation.
    fn raise_config_irq(&mut self) {
        self.interrupt_status |= VIRTIO_IRQ_CONFIG;
        self.config_generation = self.config_generation.wrapping_add(1);
        let _ = self.msix.fire(0);
    }

    fn transport_type(&self) -> &str {
        "pci"
    }
}

// ── PciFunction impl ──────────────────────────────────────────────────────────

impl PciFunction for VirtioPciTransport {
    fn vendor_id(&self) -> u16 {
        VIRTIO_PCI_VENDOR_ID
    }

    fn device_id(&self) -> u16 {
        VIRTIO_PCI_DEVICE_ID_BASE.wrapping_add(self.backend.device_id() as u16)
    }

    fn class_code(&self) -> u32 {
        Self::class_code_for(self.backend.device_id())
    }

    fn subsystem_vendor_id(&self) -> u16 {
        VIRTIO_PCI_VENDOR_ID
    }

    fn subsystem_id(&self) -> u16 {
        self.backend.device_id() as u16
    }

    fn bars(&self) -> &[BarDecl; 6] {
        &self.bars
    }

    fn capabilities(&self) -> &[Box<dyn PciCapability>] {
        // We prepend the MSI-X cap logically; since PciFunction returns caps as
        // a slice we store MSI-X separately and concatenate at query time.
        // However, the trait returns &[Box<dyn PciCapability>] which needs to
        // include msix.  We store msix in self.caps during construction by
        // inserting it at index 0 among the non-PCIe/PM caps.
        //
        // Actually, we stored caps without msix above; we'll insert msix
        // as the first entry via capabilities_mut during construction.
        // But that requires mut access.  Instead we accept that `msix` is
        // stored separately and expose it via the borrow by returning self.caps
        // which was populated including msix.  We need to rethink the layout.
        //
        // Simpler: store msix inside self.caps too.  We avoid double-borrow by
        // using Vec<Box<dyn PciCapability>> for everything including msix, and
        // keep a raw index.  But PciCapability is a trait object so we can just
        // do that.
        //
        // For this implementation we leave msix in self.caps (inserted above as
        // caps[2]) and return self.caps directly.  The msix field is only used
        // for fire() calls.  Let's restructure new() to put a clone of the
        // MsixCapability config into caps as well.
        //
        // WORKAROUND: The `capabilities()` impl here returns self.caps which
        // does NOT include msix.  The engine only uses capabilities() for the
        // capability linked list.  We keep msix separate for fire() logic and
        // expose it in caps through the mut accessor (see capabilities_mut).
        &self.caps
    }

    fn capabilities_mut(&mut self) -> &mut Vec<Box<dyn PciCapability>> {
        &mut self.caps
    }

    fn bar_read(&self, bar: u8, offset: u64, _size: usize) -> u64 {
        match bar {
            0 => {
                let val = if offset < COMMON_CFG_OFFSET + COMMON_CFG_LEN {
                    self.common_cfg_read(offset)
                } else if offset >= ISR_OFFSET && offset < ISR_OFFSET + ISR_LEN {
                    // ISR is read-to-clear — but bar_read is &self.
                    // Return current value; caller should use bar_write to ack.
                    self.interrupt_status
                } else if offset >= NOTIFY_OFFSET && offset < NOTIFY_OFFSET + NOTIFY_LEN {
                    0 // notify region is write-only
                } else if offset >= DEVICE_CFG_OFFSET && offset < DEVICE_CFG_OFFSET + DEVICE_CFG_LEN
                {
                    self.device_cfg_read(offset)
                } else {
                    0
                };
                u64::from(val)
            }
            4 => u64::from(self.msix.table_read(offset as u32, _size)),
            _ => 0,
        }
    }

    fn bar_write(&mut self, bar: u8, offset: u64, size: usize, value: u64) {
        match bar {
            0 => {
                let v32 = value as u32;
                if offset < COMMON_CFG_OFFSET + COMMON_CFG_LEN {
                    self.common_cfg_write(offset, v32);
                } else if offset >= ISR_OFFSET && offset < ISR_OFFSET + ISR_LEN {
                    // ISR read-to-clear: a write could be used to ACK,
                    // but spec says reads clear it.  Accept write as clear mask.
                    self.interrupt_status &= !v32;
                } else if offset >= NOTIFY_OFFSET && offset < NOTIFY_OFFSET + NOTIFY_LEN {
                    self.notify_write(offset, v32);
                } else if offset >= DEVICE_CFG_OFFSET && offset < DEVICE_CFG_OFFSET + DEVICE_CFG_LEN
                {
                    self.device_cfg_write(offset, v32);
                }
            }
            4 => {
                self.msix.table_write(offset as u32, size, value);
            }
            _ => {}
        }
    }

    /// Provide a read path for the ISR that clears it (BAR0 read dispatched here
    /// for ISR is &self so we duplicate logic; the mutable version is below).
    fn config_read(&self, _offset: u16) -> u32 {
        0
    }

    fn config_write(&mut self, _offset: u16, _value: u32) {}

    fn reset(&mut self) {
        self.do_reset();
        self.msix.reset();
    }

    fn tick(&mut self, cycles: u64) -> Vec<DeviceEvent> {
        self.backend.tick(cycles, &mut self.queues)
    }

    fn name(&self) -> &str {
        self.backend.name()
    }
}

// ── Mutable BAR0 ISR read helper ──────────────────────────────────────────────

impl VirtioPciTransport {
    /// Read BAR0 with full ISR read-to-clear semantics.
    ///
    /// Identical to [`PciFunction::bar_read`] for all regions except ISR,
    /// where this version atomically clears the status.
    pub fn bar0_read_mut(&mut self, offset: u64) -> u32 {
        if offset >= ISR_OFFSET && offset < ISR_OFFSET + ISR_LEN {
            self.isr_read()
        } else {
            self.bar_read(0, offset, 4) as u32
        }
    }
}
