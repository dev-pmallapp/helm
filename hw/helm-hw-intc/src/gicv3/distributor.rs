//! GICv3 Distributor (GICD) — 64KB MMIO region managing SPI routing.

use std::sync::{Arc, Mutex};

use helm_devices::Device;
use helm_diag::sim_stub;

use super::GicV3SharedState;

pub struct Gicv3Distributor(pub Arc<Mutex<GicV3SharedState>>);

impl Gicv3Distributor {
    pub fn new(shared: Arc<Mutex<GicV3SharedState>>) -> Self { Self(shared) }

    pub fn assert_irq(&self, intid: u32) {
        self.0.lock().unwrap().assert_spi(intid);
    }
    pub fn deassert_irq(&self, intid: u32) {
        self.0.lock().unwrap().deassert_spi(intid);
    }
}

impl Device for Gicv3Distributor {
    fn region_size(&self) -> u64 { 0x1_0000 } // 64KB

    fn read(&mut self, offset: u64, size: usize) -> u64 {
        let s = self.0.lock().unwrap();
        let d = &s.dist;
        match offset {
            // ── Core registers ───────────────────────────────────────────────
            0x0000 => u64::from(d.ctlr),
            0x0004 => u64::from(d.typer),
            0x0008 => 0x0102_43B4,          // GICD_IIDR
            0x000C => 0,                     // GICD_TYPER2
            0x0010 => 0,                     // GICD_STATUSR
            0xFFE8 => 0x3B,                  // GICD_PIDR2 (GICv3): ArchRev=3
            0xFFD0 => 0,                     // GICD_PIDR4
            // ── GICD_IGROUPR: SPI group bits ─────────────────────────────────
            o @ 0x0100..=0x017C => {
                let n = ((o - 0x0100) / 4) as usize;
                d.group.get(n).copied().unwrap_or(0) as u64
            }
            // ── GICD_ISENABLER / GICD_ICENABLER (same read) ──────────────────
            o @ 0x0180..=0x01FC |
            o @ 0x0200..=0x027C => {
                let base = if offset < 0x0200 { 0x0180 } else { 0x0200 };
                let n = ((o - base) / 4) as usize;
                d.enabled.get(n).copied().unwrap_or(0) as u64
            }
            // ── GICD_ISPENDR / GICD_ICPENDR (same read) ──────────────────────
            o @ 0x0280..=0x02FC |
            o @ 0x0300..=0x037C => {
                let base = if offset < 0x0300 { 0x0280 } else { 0x0300 };
                let n = ((o - base) / 4) as usize;
                d.pending.get(n).copied().unwrap_or(0) as u64
            }
            // ── GICD_ISACTIVER / GICD_ICACTIVER (same read) ──────────────────
            o @ 0x0380..=0x03FC |
            o @ 0x0400..=0x047C => {
                let base = if offset < 0x0400 { 0x0380 } else { 0x0400 };
                let n = ((o - base) / 4) as usize;
                d.active.get(n).copied().unwrap_or(0) as u64
            }
            // ── GICD_IPRIORITYR ───────────────────────────────────────────────
            o @ 0x0480..=0x07F8 => {
                let byte_base = (o - 0x0480) as usize;
                if size == 1 {
                    d.priority.get(byte_base + 32).copied().unwrap_or(0) as u64
                } else {
                    // 4-byte read: 4 priority bytes
                    let b = byte_base + 32;
                    let p = &d.priority;
                    let b0 = p.get(b).copied().unwrap_or(0) as u64;
                    let b1 = p.get(b+1).copied().unwrap_or(0) as u64;
                    let b2 = p.get(b+2).copied().unwrap_or(0) as u64;
                    let b3 = p.get(b+3).copied().unwrap_or(0) as u64;
                    b0 | (b1 << 8) | (b2 << 16) | (b3 << 24)
                }
            }
            // ── GICD_ICFGR ────────────────────────────────────────────────────
            o @ 0x0C00..=0x0CFC => {
                let n = ((o - 0x0C00) / 4) as usize;
                d.config.get(n).copied().unwrap_or(0) as u64
            }
            // ── GICD_IROUTER (64-bit) ─────────────────────────────────────────
            o @ 0x6100..=0x7FF8 => {
                let intid = ((o - 0x6000) / 8) as usize;
                if intid < 32 { return 0; } // INTID 0..31 reserved
                let idx = intid - 32;
                let val = d.irouter.get(idx).copied().unwrap_or(0);
                if size == 4 {
                    if o & 4 != 0 { val >> 32 } else { val & 0xFFFF_FFFF }
                } else {
                    val
                }
            }
            0x0040 | 0x0048 => 0, // SETSPI/CLRSPI: WO, reads 0
            _ => {
                sim_stub!(component="gicv3-gicd", "read unhandled offset={offset:#x} -> 0");
                0
            }
        }
    }

    fn write(&mut self, offset: u64, size: usize, val: u64) {
        let val32 = val as u32;
        let mut s = self.0.lock().unwrap();
        match offset {
            // ── GICD_CTLR ─────────────────────────────────────────────────────
            0x0000 => {
                // Preserve ARE_NS/ARE_S bits; enforce ARE=1 in GICv3-only mode
                s.dist.ctlr = (val32 & 0x37) | 0x30; // keep ARE_NS|ARE_S set
                s.update_all_irq_lines();
            }
            // ── GICD_STATUSR ──────────────────────────────────────────────────
            0x0010 => {} // W1C — ignore in sim
            // ── GICD_SETSPI_NSR ───────────────────────────────────────────────
            0x0040 => { s.assert_spi(val32 & 0x3FF); }
            // ── GICD_CLRSPI_NSR ───────────────────────────────────────────────
            0x0048 => { s.deassert_spi(val32 & 0x3FF); }
            // ── GICD_IGROUPR ──────────────────────────────────────────────────
            o @ 0x0100..=0x017C => {
                let n = ((o - 0x0100) / 4) as usize;
                if n > 0 { // word 0 = INTID 0..31 → belongs to GICR, RAZ/WI
                    if let Some(g) = s.dist.group.get_mut(n) { *g = val32; }
                }
            }
            // ── GICD_ISENABLER ────────────────────────────────────────────────
            o @ 0x0180..=0x01FC => {
                let n = ((o - 0x0180) / 4) as usize;
                if let Some(e) = s.dist.enabled.get_mut(n) { *e |= val32; }
                s.update_all_irq_lines();
            }
            // ── GICD_ICENABLER ────────────────────────────────────────────────
            o @ 0x0200..=0x027C => {
                let n = ((o - 0x0200) / 4) as usize;
                if let Some(e) = s.dist.enabled.get_mut(n) { *e &= !val32; }
                s.update_all_irq_lines();
            }
            // ── GICD_ISPENDR ──────────────────────────────────────────────────
            o @ 0x0280..=0x02FC => {
                let n = ((o - 0x0280) / 4) as usize;
                if let Some(p) = s.dist.pending.get_mut(n) { *p |= val32; }
                s.update_all_irq_lines();
            }
            // ── GICD_ICPENDR ──────────────────────────────────────────────────
            o @ 0x0300..=0x037C => {
                let n = ((o - 0x0300) / 4) as usize;
                if let Some(p) = s.dist.pending.get_mut(n) { *p &= !val32; }
                s.update_all_irq_lines();
            }
            // ── GICD_ISACTIVER ────────────────────────────────────────────────
            o @ 0x0380..=0x03FC => {
                let n = ((o - 0x0380) / 4) as usize;
                if let Some(a) = s.dist.active.get_mut(n) { *a |= val32; }
            }
            // ── GICD_ICACTIVER ────────────────────────────────────────────────
            o @ 0x0400..=0x047C => {
                let n = ((o - 0x0400) / 4) as usize;
                if let Some(a) = s.dist.active.get_mut(n) { *a &= !val32; }
                s.update_all_irq_lines();
            }
            // ── GICD_IPRIORITYR ───────────────────────────────────────────────
            o @ 0x0480..=0x07F8 => {
                let byte_base = (o - 0x0480) as usize + 32; // offset into priority[]
                if size == 1 {
                    if let Some(b) = s.dist.priority.get_mut(byte_base) { *b = val as u8; }
                } else {
                    for i in 0..4usize {
                        if let Some(b) = s.dist.priority.get_mut(byte_base + i) {
                            *b = ((val >> (i * 8)) & 0xFF) as u8;
                        }
                    }
                }
            }
            // ── GICD_ICFGR ────────────────────────────────────────────────────
            o @ 0x0C00..=0x0CFC => {
                let n = ((o - 0x0C00) / 4) as usize;
                if let Some(c) = s.dist.config.get_mut(n) { *c = val32; }
            }
            // ── GICD_IROUTER (64-bit) ─────────────────────────────────────────
            o @ 0x6100..=0x7FF8 => {
                let intid = ((o - 0x6000) / 8) as usize;
                if intid < 32 { return; }
                let idx = intid - 32;
                if let Some(r) = s.dist.irouter.get_mut(idx) {
                    if size == 4 {
                        if o & 4 != 0 { *r = (*r & 0xFFFF_FFFF) | (val << 32); }
                        else          { *r = (*r & !0xFFFF_FFFF) | (val & 0xFFFF_FFFF); }
                    } else {
                        *r = val;
                    }
                }
                s.update_all_irq_lines();
            }
            _ => {
                sim_stub!(component="gicv3-gicd",
                    "write unhandled offset={offset:#x} val={val:#x} (ignored)");
            }
        }
    }
}
