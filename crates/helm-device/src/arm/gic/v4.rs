//! GICv4 / GICv4.1 — virtual interrupt support.
//!
//! `GicV4` wraps a [`GicV3`] and adds:
//!
//! - **vLPI** — virtual LPIs delivered directly to a virtual PE
//!   without a hypervisor trap.
//! - **vSGI** (v4.1) — virtual SGIs delivered directly.
//! - **vPE scheduling** — resident / non-resident tracking with
//!   doorbell support.
//!
//! The ITS is extended with `VMAPP`, `VMAPTI`, `VMOVI`, `VINVALL`,
//! `VSYNC`, and `VSGI` commands.

use std::collections::BTreeMap;

use crate::device::{Device, DeviceEvent};
use crate::irq::InterruptController;
use crate::region::MemRegion;
use crate::transaction::Transaction;
use helm_core::types::Addr;
use helm_core::HelmResult;

use super::v3::GicV3;

/// GICv4 sub-version selector.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GicV4Version {
    /// GICv4.0.
    V4,
    /// GICv4.1 (adds vSGI).
    V4_1,
}

/// Configuration for a virtual PE.
#[derive(Debug, Clone)]
pub struct VpeEntry {
    /// Virtual PE identifier.
    pub vpe_id: u32,
    /// Physical PE this vPE is mapped to.
    pub target_pe: u32,
    /// Base address of the virtual LPI pending table.
    pub vlpi_pending_base: u64,
    /// Base address of the virtual LPI configuration table.
    pub vlpi_config_base: u64,
    /// Whether the vPE is currently resident (scheduled) on its PE.
    pub resident: bool,
    /// vSGI priority/enable for SGIs 0-15 (GICv4.1 only).
    pub vsgi_config: [u8; 16],
    /// Pending doorbell INTID (used when vPE is non-resident).
    pub doorbell_intid: Option<u32>,
}

impl VpeEntry {
    /// Create a default vPE entry.
    pub fn new(vpe_id: u32, target_pe: u32) -> Self {
        Self {
            vpe_id,
            target_pe,
            vlpi_pending_base: 0,
            vlpi_config_base: 0,
            resident: false,
            vsgi_config: [0; 16],
            doorbell_intid: None,
        }
    }
}

/// Virtual LPI mapping entry (populated by `VMAPTI`).
#[derive(Debug, Clone)]
pub struct VlpiMapping {
    /// Source device ID.
    pub device_id: u32,
    /// Source event ID.
    pub event_id: u32,
    /// Virtual INTID to inject into the vPE.
    pub vintid: u32,
    /// Target vPE ID.
    pub vpe_id: u32,
}

/// GICv4 device model.
pub struct GicV4 {
    /// Inner GICv3 providing all v3 functionality.
    pub inner: GicV3,
    /// GICv4 sub-version.
    pub version: GicV4Version,
    /// Virtual PE table: `vPE_ID` → configuration.
    pub vpe_table: BTreeMap<u32, VpeEntry>,
    /// Virtual LPI mappings: `(DeviceID, EventID)` → vLPI target.
    pub vlpi_mappings: Vec<VlpiMapping>,
    /// Pending virtual interrupts: `(vpe_id, vintid)`.
    pub pending_vlpis: Vec<(u32, u32)>,
}

impl GicV4 {
    /// Create a GICv4 with the given parameters.
    pub fn new(
        name: impl Into<String>,
        num_irqs: u32,
        num_pes: u32,
        version: GicV4Version,
    ) -> Self {
        Self {
            inner: GicV3::new(name, num_irqs, num_pes),
            version,
            vpe_table: BTreeMap::new(),
            vlpi_mappings: Vec::new(),
            pending_vlpis: Vec::new(),
        }
    }

    // ── ITS v4 commands ─────────────────────────────────────────────────

    /// `VMAPP` — map a vPE to a physical PE.
    pub fn cmd_vmapp(&mut self, vpe_id: u32, target_pe: u32, valid: bool) {
        if valid {
            self.vpe_table
                .entry(vpe_id)
                .and_modify(|e| e.target_pe = target_pe)
                .or_insert_with(|| VpeEntry::new(vpe_id, target_pe));
        } else {
            self.vpe_table.remove(&vpe_id);
        }
    }

    /// `VMAPTI` — map `(DeviceID, EventID)` → `(vINTID, vPE)`.
    pub fn cmd_vmapti(&mut self, device_id: u32, event_id: u32, vintid: u32, vpe_id: u32) {
        self.vlpi_mappings
            .retain(|m| !(m.device_id == device_id && m.event_id == event_id));
        self.vlpi_mappings.push(VlpiMapping {
            device_id,
            event_id,
            vintid,
            vpe_id,
        });
    }

    /// `VMOVI` — move a virtual LPI to a different vPE.
    pub fn cmd_vmovi(&mut self, device_id: u32, event_id: u32, new_vpe_id: u32) {
        if let Some(mapping) = self
            .vlpi_mappings
            .iter_mut()
            .find(|m| m.device_id == device_id && m.event_id == event_id)
        {
            mapping.vpe_id = new_vpe_id;
        }
    }

    /// Schedule a vPE on its physical PE (mark as resident).
    pub fn schedule_vpe(&mut self, vpe_id: u32) {
        if let Some(vpe) = self.vpe_table.get_mut(&vpe_id) {
            vpe.resident = true;
        }
    }

    /// Deschedule a vPE (mark as non-resident).
    pub fn deschedule_vpe(&mut self, vpe_id: u32) {
        if let Some(vpe) = self.vpe_table.get_mut(&vpe_id) {
            vpe.resident = false;
        }
    }

    /// Inject a virtual LPI via `(DeviceID, EventID)`.
    ///
    /// If the target vPE is resident, the vLPI is injected directly.
    /// If non-resident, a doorbell interrupt is generated on the
    /// physical PE instead.
    pub fn inject_vlpi(&mut self, device_id: u32, event_id: u32) -> Option<(u32, u32)> {
        let mapping = self
            .vlpi_mappings
            .iter()
            .find(|m| m.device_id == device_id && m.event_id == event_id)?
            .clone();
        let vpe = self.vpe_table.get(&mapping.vpe_id)?;
        let result = (mapping.vpe_id, mapping.vintid);

        if vpe.resident {
            self.pending_vlpis.push(result);
        } else if let Some(doorbell) = vpe.doorbell_intid {
            self.inner.inject(doorbell, true);
        }

        Some(result)
    }

    /// `VSGI` — inject a virtual SGI (GICv4.1).
    pub fn inject_vsgi(&mut self, vpe_id: u32, intid: u32) -> bool {
        if self.version != GicV4Version::V4_1 || intid >= 16 {
            return false;
        }
        if let Some(vpe) = self.vpe_table.get(&vpe_id) {
            if vpe.resident {
                self.pending_vlpis.push((vpe_id, intid));
                return true;
            } else if let Some(doorbell) = vpe.doorbell_intid {
                self.inner.inject(doorbell, true);
                return true;
            }
        }
        false
    }

    /// Drain pending virtual LPI injections.
    pub fn drain_pending_vlpis(&mut self) -> Vec<(u32, u32)> {
        std::mem::take(&mut self.pending_vlpis)
    }
}

// ── Device / InterruptController delegation ─────────────────────────────────

impl Device for GicV4 {
    fn transact(&mut self, txn: &mut Transaction) -> HelmResult<()> {
        self.inner.transact(txn)
    }

    fn regions(&self) -> &[MemRegion] {
        self.inner.regions()
    }

    fn reset(&mut self) -> HelmResult<()> {
        self.inner.reset()?;
        self.vpe_table.clear();
        self.vlpi_mappings.clear();
        self.pending_vlpis.clear();
        Ok(())
    }

    fn read_fast(&mut self, offset: Addr, size: usize) -> HelmResult<u64> {
        self.inner.read_fast(offset, size)
    }

    fn write_fast(&mut self, offset: Addr, size: usize, val: u64) -> HelmResult<()> {
        self.inner.write_fast(offset, size, val)
    }

    fn name(&self) -> &str {
        self.inner.name()
    }

    fn tick(&mut self, cycles: u64) -> HelmResult<Vec<DeviceEvent>> {
        self.inner.tick(cycles)
    }
}

impl InterruptController for GicV4 {
    fn inject(&mut self, irq: u32, level: bool) {
        self.inner.inject(irq, level);
    }

    fn pending_for_cpu(&self, cpu_id: u32) -> bool {
        self.inner.pending_for_cpu(cpu_id)
    }

    fn ack(&mut self, cpu_id: u32) -> Option<u32> {
        self.inner.ack(cpu_id)
    }
}
