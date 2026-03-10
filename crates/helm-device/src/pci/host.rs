//! PCIe host bridge with ECAM config access.
//!
//! [`PciHostBridge`] implements [`Device`] and is placed on the platform's
//! system bus as a single contiguous region. The region is split into two
//! logical windows:
//!
//! ```text
//! [ecam_base .. ecam_base + ecam_size)           — ECAM config access
//! [ecam_base + ecam_size .. + mmio32_size)       — BAR MMIO window
//! ```
//!
//! Both are exposed as one `MemRegion` of size `ecam_size + mmio32_size`.
//! The [`Device::transact`] method routes accesses to the right window based
//! on `txn.offset`.
//!
//! # BAR allocation
//!
//! [`attach`](PciHostBridge::attach) probes each BAR declared by the function,
//! allocates space from the 32-bit MMIO pool (bump allocator, power-of-two
//! aligned), and programs the resolved address back into the function's
//! config space.

use crate::device::{Device, DeviceEvent};
use crate::pci::{BarDecl, Bdf, PciBus, PciFunction};
use crate::region::{MemRegion, RegionKind};
use crate::transaction::Transaction;
use helm_core::HelmResult;

// ── PciHostBridge ─────────────────────────────────────────────────────────────

/// PCIe host bridge: ECAM config space + 32-bit BAR MMIO window.
///
/// Implements [`Device`] and is placed directly on the platform system bus.
/// Attach PCI functions via [`attach`](Self::attach); the bridge auto-allocates
/// BAR addresses from the 32-bit MMIO pool.
///
/// # Examples
///
/// ```
/// use helm_device::pci::PciHostBridge;
/// use helm_device::Device;
///
/// let bridge = PciHostBridge::new(
///     0x3000_0000,   // ECAM base
///     0x0100_0000,   // ECAM size (1 MB — 256 buses × 32 dev × 8 fn × 4 KB)
///     0x1000_0000,   // MMIO32 base
///     0x2000_0000,   // MMIO32 size
/// );
/// assert_eq!(bridge.name(), "pci-host-bridge");
/// assert_eq!(bridge.regions().len(), 1);
/// ```
pub struct PciHostBridge {
    bus: PciBus,
    /// Base address of the ECAM window; stored for reference/introspection.
    #[allow(dead_code)]
    ecam_base: u64,
    ecam_size: u64,
    mmio32_base: u64,
    mmio32_size: u64,
    /// Bump-allocator cursor: next free byte in the MMIO32 window.
    mmio32_next: u64,
    regions: Vec<MemRegion>,
}

impl PciHostBridge {
    /// Create a new host bridge.
    ///
    /// # Arguments
    ///
    /// * `ecam_base`    — base address of the ECAM window in platform space
    /// * `ecam_size`    — size of the ECAM window (typically 256 × 32 × 8 × 4 KB)
    /// * `mmio32_base`  — base address of the 32-bit BAR MMIO pool
    /// * `mmio32_size`  — size of the BAR pool
    ///
    /// # Examples
    ///
    /// ```
    /// use helm_device::pci::PciHostBridge;
    /// use helm_device::Device;
    ///
    /// let bridge = PciHostBridge::new(0x3000_0000, 0x100_0000, 0x1000_0000, 0x2000_0000);
    /// assert_eq!(bridge.regions()[0].size, 0x100_0000 + 0x2000_0000);
    /// ```
    #[must_use]
    pub fn new(ecam_base: u64, ecam_size: u64, mmio32_base: u64, mmio32_size: u64) -> Self {
        let combined_size = ecam_size.saturating_add(mmio32_size);
        let region = MemRegion {
            name: "pci-host-bridge".to_string(),
            base: ecam_base,
            size: combined_size,
            kind: RegionKind::Io,
            priority: 0,
        };
        Self {
            bus: PciBus::new(0),
            ecam_base,
            ecam_size,
            mmio32_base,
            mmio32_size,
            mmio32_next: 0,
            regions: vec![region],
        }
    }

    /// Attach a PCI function and auto-allocate its BAR addresses from the
    /// 32-bit MMIO pool.
    ///
    /// Each BAR is aligned to its declared size (next power of two). The
    /// resolved address is written into the function's config-space BAR
    /// register so that software reads the correct base address.
    ///
    /// # Panics
    ///
    /// Does not panic; BAR allocations that exceed the pool are silently
    /// skipped (the BAR address stays at 0).
    pub fn attach(&mut self, device: u8, function: u8, func: Box<dyn PciFunction>) {
        // Collect BAR declarations before moving `func` into the bus.
        let bars: [BarDecl; 6] = *func.bars();
        let bdf = Bdf::new(0, device, function);

        // Attach function to bus — builds the config space.
        self.bus.attach(device, function, func);

        // Allocate and program each declared BAR.
        let mut i = 0usize;
        while i < 6 {
            let decl = bars[i];
            let size = decl.size();
            if size == 0 || decl.is_unused() {
                i += 1;
                continue;
            }

            // Power-of-two alignment.
            let align = size.next_power_of_two();
            let cursor_aligned = (self.mmio32_next.saturating_add(align - 1)) & !(align - 1);

            if cursor_aligned.saturating_add(size) <= self.mmio32_size {
                let bar_phys = self.mmio32_base + cursor_aligned;
                self.mmio32_next = cursor_aligned + size;

                // Tell the bus about the resolved mapping.
                self.bus.set_bar_mapping(bdf, i as u8, bar_phys, size);

                // Program the BAR address into config space.
                // Config-space BAR offset = 0x10 + i*4.
                let bar_reg_off = 0x10u16 + (i as u16) * 4;
                // For 32-bit BARs write the low 32 bits.
                // For 64-bit BARs write low and high.
                #[allow(clippy::cast_possible_truncation)]
                match decl {
                    BarDecl::Mmio32 { .. } => {
                        self.bus.config_write(bdf, bar_reg_off, bar_phys as u32);
                    }
                    BarDecl::Mmio64 { .. } => {
                        self.bus.config_write(bdf, bar_reg_off, bar_phys as u32);
                        if i + 1 < 6 {
                            self.bus
                                .config_write(bdf, bar_reg_off + 4, (bar_phys >> 32) as u32);
                        }
                        i += 2; // 64-bit BAR occupies two slots
                        continue;
                    }
                    BarDecl::Io { .. } => {
                        self.bus.config_write(bdf, bar_reg_off, bar_phys as u32);
                    }
                    BarDecl::Unused => {}
                }
            }

            i += 1;
        }
    }

    /// Return the resolved base address of a BAR, or `None` if not mapped.
    ///
    /// # Examples
    ///
    /// ```
    /// use helm_device::pci::{PciHostBridge, Bdf};
    ///
    /// let bridge = PciHostBridge::new(0x3000_0000, 0x100_0000, 0x1000_0000, 0x2000_0000);
    /// let bdf = Bdf::new(0, 0, 0);
    /// assert!(bridge.bar_address(bdf, 0).is_none());
    /// ```
    #[must_use]
    pub fn bar_address(&self, bdf: Bdf, bar_index: u8) -> Option<u64> {
        self.bus.bar_address(bdf, bar_index)
    }

    /// MMIO read from the BAR window.
    ///
    /// `offset` is relative to `mmio32_base` (i.e. the absolute address is
    /// `mmio32_base + offset`).
    pub fn mmio_read(&mut self, offset: u64, size: usize) -> Option<u64> {
        let addr = self.mmio32_base.wrapping_add(offset);
        self.bus.bar_read(addr, size)
    }

    /// MMIO write to the BAR window.
    ///
    /// Returns `true` if a mapped BAR was found and the write was dispatched.
    pub fn mmio_write(&mut self, offset: u64, size: usize, value: u64) -> bool {
        let addr = self.mmio32_base.wrapping_add(offset);
        self.bus.bar_write(addr, size, value)
    }

    /// Return all populated BDFs, sorted by `(device, function)`.
    #[must_use]
    pub fn enumerate(&self) -> Vec<Bdf> {
        self.bus.enumerate()
    }
}

// ── Device impl ───────────────────────────────────────────────────────────────

impl Device for PciHostBridge {
    /// Handle a transaction to the host bridge region.
    ///
    /// The host bridge exposes one contiguous region of size
    /// `ecam_size + mmio32_size`. Accesses are split by offset:
    ///
    /// - `offset < ecam_size`  → ECAM config space (32-bit reads/writes)
    /// - `offset >= ecam_size` → BAR MMIO at `mmio32_base + (offset - ecam_size)`
    fn transact(&mut self, txn: &mut Transaction) -> HelmResult<()> {
        txn.stall_cycles += 1;

        let offset = txn.offset;

        if offset < self.ecam_size {
            // ECAM config access: decode BDF from offset.
            let (bdf, reg) = Bdf::from_ecam_offset(offset);
            if txn.is_write {
                self.bus.config_write(bdf, reg, txn.data_u32());
            } else {
                let val = self.bus.config_read(bdf, reg);
                txn.set_data_u32(val);
            }
        } else {
            // BAR MMIO access.
            let mmio_offset = offset - self.ecam_size;
            let addr = self.mmio32_base.wrapping_add(mmio_offset);
            if txn.is_write {
                self.bus.bar_write(addr, txn.size, txn.data_u64());
            } else {
                let val = self.bus.bar_read(addr, txn.size).unwrap_or(u64::MAX);
                txn.set_data_u64(val);
            }
        }

        Ok(())
    }

    fn regions(&self) -> &[MemRegion] {
        &self.regions
    }

    fn reset(&mut self) -> HelmResult<()> {
        self.bus.reset_all();
        self.mmio32_next = 0;
        Ok(())
    }

    fn tick(&mut self, cycles: u64) -> HelmResult<Vec<DeviceEvent>> {
        Ok(self.bus.tick_all(cycles))
    }

    fn name(&self) -> &str {
        "pci-host-bridge"
    }
}
