//! GICv3 Distributor (GICD) — 64KB MMIO region managing SPI routing.

use std::sync::{Arc, Mutex};

use helm_devices::Device;
use helm_diag::sim_stub;

use super::GicV3SharedState;

pub struct Gicv3Distributor(pub Arc<Mutex<GicV3SharedState>>);

impl Gicv3Distributor {
    pub fn new(shared: Arc<Mutex<GicV3SharedState>>) -> Self {
        Self(shared)
    }

    pub fn assert_irq(&self, intid: u32) {
        self.0.lock().unwrap().assert_spi(intid);
    }
    pub fn deassert_irq(&self, intid: u32) {
        self.0.lock().unwrap().deassert_spi(intid);
    }
}

fn amba_id_read(offset: u64) -> Option<u64> {
    match offset {
        0xFD0 | 0xFFD0 => Some(0),
        0xFE0 | 0xFFE0 => Some(0x90),
        0xFE4 | 0xFFE4 => Some(0xB4),
        0xFE8 | 0xFFE8 => Some(0x3B),
        0xFEC | 0xFFEC => Some(0x00),
        0xFF0 | 0xFFF0 => Some(0x0D),
        0xFF4 | 0xFFF4 => Some(0xF0),
        0xFF8 | 0xFFF8 => Some(0x05),
        0xFFC | 0xFFFC => Some(0xB1),
        _ => None,
    }
}

impl Device for Gicv3Distributor {
    fn region_size(&self) -> u64 {
        0x1_0000
    } // 64KB

    fn read(&mut self, offset: u64, size: usize) -> u64 {
        let s = self.0.lock().unwrap();
        let d = &s.dist;
        if let Some(id) = amba_id_read(offset) {
            return id;
        }
        match offset {
            // ── Core registers ───────────────────────────────────────────────
            0x0000 => u64::from(d.ctlr),
            0x0004 => u64::from(d.typer),
            0x0008 => 0x0102_43B4, // GICD_IIDR
            0x000C => 0,           // GICD_TYPER2
            0x0010 => 0,           // GICD_STATUSR
            // ── GICD_IGROUPR (0x0080 + 4*n) ─────────────────────────────────
            // Word 0 (0x0080) covers SGIs/PPIs — RAZ in GICD (managed by GICR).
            0x0080 => 0,
            o @ 0x0084..=0x00FC => {
                let n = ((o - 0x0084) / 4) as usize;
                d.group.get(n).copied().unwrap_or(0) as u64
            }
            // ── GICD_ISENABLER / GICD_ICENABLER (same read) ──────────────────
            // Word 0 (0x0100 / 0x0180) covers SGIs/PPIs — RAZ/WI in GICD.
            0x0100 | 0x0180 => 0,
            o @ 0x0104..=0x017C | o @ 0x0184..=0x01FC => {
                let base = if offset < 0x0180 { 0x0104 } else { 0x0184 };
                let n = ((o - base) / 4) as usize;
                d.enabled.get(n).copied().unwrap_or(0) as u64
            }
            // ── GICD_ISPENDR / GICD_ICPENDR (same read) ──────────────────────
            0x0200 | 0x0280 => 0,
            o @ 0x0204..=0x027C | o @ 0x0284..=0x02FC => {
                let base = if offset < 0x0280 { 0x0204 } else { 0x0284 };
                let n = ((o - base) / 4) as usize;
                d.pending.get(n).copied().unwrap_or(0) as u64
            }
            // ── GICD_ISACTIVER / GICD_ICACTIVER (same read) ──────────────────
            0x0300 | 0x0380 => 0,
            o @ 0x0304..=0x037C | o @ 0x0384..=0x03FC => {
                let base = if offset < 0x0380 { 0x0304 } else { 0x0384 };
                let n = ((o - base) / 4) as usize;
                d.active.get(n).copied().unwrap_or(0) as u64
            }
            // ── GICD_IPRIORITYR ───────────────────────────────────────────────
            o @ 0x0400..=0x07FC => {
                let byte_base = (o - 0x0400) as usize;
                if size == 1 {
                    d.priority.get(byte_base).copied().unwrap_or(0) as u64
                } else {
                    // 4-byte read: 4 priority bytes
                    let b = byte_base;
                    let p = &d.priority;
                    let b0 = p.get(b).copied().unwrap_or(0) as u64;
                    let b1 = p.get(b + 1).copied().unwrap_or(0) as u64;
                    let b2 = p.get(b + 2).copied().unwrap_or(0) as u64;
                    let b3 = p.get(b + 3).copied().unwrap_or(0) as u64;
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
                if intid < 32 {
                    return 0;
                } // INTID 0..31 reserved
                let idx = intid - 32;
                let val = d.irouter.get(idx).copied().unwrap_or(0);
                if size == 4 {
                    if o & 4 != 0 {
                        val >> 32
                    } else {
                        val & 0xFFFF_FFFF
                    }
                } else {
                    val
                }
            }
            0x0040 | 0x0048 => 0, // SETSPI/CLRSPI: WO, reads 0
            0x0018 => 0,           // GICD_STATUSR: RAZ in sim
            0x0020 | 0x0028 | 0x0030 | 0x0038 => 0, // SETSPI/CLRSPI NS/SR: WO reads 0
            // GICD_IROUTER compact alias at 0x0800: 0x0800 + 8*(intid-32)
            o @ 0x0800..=0x0FFF => {
                let idx = ((o - 0x0800) / 8) as usize;
                let val = d.irouter.get(idx).copied().unwrap_or(0);
                if size == 4 {
                    if o & 4 != 0 { val >> 32 } else { val & 0xFFFF_FFFF }
                } else {
                    val
                }
            }
            // Reserved/unimplemented: RAZ silently.
            0x0050..=0x007F | 0x0D00..=0x5FFF | 0x8000..=0xEFFF => 0,
            _ => {
                sim_stub!(
                    component = "gicv3-gicd",
                    "read unhandled offset={offset:#x} -> 0"
                );
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
            0x0040 => {
                s.assert_spi(val32 & 0x3FF);
            }
            // ── GICD_CLRSPI_NSR ───────────────────────────────────────────────
            0x0048 => {
                s.deassert_spi(val32 & 0x3FF);
            }
            // ── GICD_IGROUPR ──────────────────────────────────────────────────
            // Word 0 (0x0080) = SGI/PPI group — managed by GICR; WI in GICD.
            0x0080 => {}
            o @ 0x0084..=0x00FC => {
                let n = ((o - 0x0084) / 4) as usize;
                if let Some(g) = s.dist.group.get_mut(n) {
                    *g = val32;
                }
            }
            // ── GICD_ISENABLER ────────────────────────────────────────────────
            // Word 0 (0x0100) = SGI/PPI enables — managed by GICR; WI in GICD.
            0x0100 => {}
            o @ 0x0104..=0x017C => {
                let n = ((o - 0x0104) / 4) as usize;
                if let Some(e) = s.dist.enabled.get_mut(n) {
                    *e |= val32;
                }
                s.update_all_irq_lines();
            }
            // ── GICD_ICENABLER ────────────────────────────────────────────────
            0x0180 => {}
            o @ 0x0184..=0x01FC => {
                let n = ((o - 0x0184) / 4) as usize;
                if let Some(e) = s.dist.enabled.get_mut(n) {
                    *e &= !val32;
                }
                s.update_all_irq_lines();
            }
            // ── GICD_ISPENDR ──────────────────────────────────────────────────
            0x0200 => {}
            o @ 0x0204..=0x027C => {
                let n = ((o - 0x0204) / 4) as usize;
                if let Some(p) = s.dist.pending.get_mut(n) {
                    *p |= val32;
                }
                s.update_all_irq_lines();
            }
            // ── GICD_ICPENDR ──────────────────────────────────────────────────
            0x0280 => {}
            o @ 0x0284..=0x02FC => {
                let n = ((o - 0x0284) / 4) as usize;
                if let Some(p) = s.dist.pending.get_mut(n) {
                    *p &= !val32;
                }
                s.update_all_irq_lines();
            }
            // ── GICD_ISACTIVER ────────────────────────────────────────────────
            0x0300 => {}
            o @ 0x0304..=0x037C => {
                let n = ((o - 0x0304) / 4) as usize;
                if let Some(a) = s.dist.active.get_mut(n) {
                    *a |= val32;
                }
            }
            // ── GICD_ICACTIVER ────────────────────────────────────────────────
            0x0380 => {}
            o @ 0x0384..=0x03FC => {
                let n = ((o - 0x0384) / 4) as usize;
                if let Some(a) = s.dist.active.get_mut(n) {
                    *a &= !val32;
                }
                s.update_all_irq_lines();
            }
            // ── GICD_IPRIORITYR ───────────────────────────────────────────────
            o @ 0x0400..=0x07FC => {
                let byte_base = (o - 0x0400) as usize;
                if size == 1 {
                    if let Some(b) = s.dist.priority.get_mut(byte_base) {
                        *b = val as u8;
                    }
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
                if let Some(c) = s.dist.config.get_mut(n) {
                    *c = val32;
                }
            }
            // ── GICD_IROUTER (64-bit) ─────────────────────────────────────────
            o @ 0x6100..=0x7FF8 => {
                let intid = ((o - 0x6000) / 8) as usize;
                if intid < 32 {
                    return;
                }
                let idx = intid - 32;
                if let Some(r) = s.dist.irouter.get_mut(idx) {
                    if size == 4 {
                        if o & 4 != 0 {
                            *r = (*r & 0xFFFF_FFFF) | (val << 32);
                        } else {
                            *r = (*r & !0xFFFF_FFFF) | (val & 0xFFFF_FFFF);
                        }
                    } else {
                        *r = val;
                    }
                }
                s.update_all_irq_lines();
            }
            // GICD_STATUSR (0x0018): W1C — ignore in sim
            0x0018 => {}
            // SETSPI_SR/CLRSPI_SR (Secure variants) — same as NS paths
            0x0030 => { s.assert_spi(val32 & 0x3FF); }
            0x0038 => { s.deassert_spi(val32 & 0x3FF); }
            // GICD_IROUTER compact alias: 0x0800 + 8*(intid-32)
            o @ 0x0800..=0x0FFF => {
                let idx = ((o - 0x0800) / 8) as usize;
                if let Some(r) = s.dist.irouter.get_mut(idx) {
                    if size == 4 {
                        if o & 4 != 0 {
                            *r = (*r & 0xFFFF_FFFF) | (val << 32);
                        } else {
                            *r = (*r & !0xFFFF_FFFF) | (val & 0xFFFF_FFFF);
                        }
                    } else {
                        *r = val;
                    }
                }
                s.update_all_irq_lines();
            }
            // Reserved/unimplemented: WI silently.
            0x0050..=0x007F | 0x0D00..=0x5FFF | 0x8000..=0xEFFF => {}
            _ => {
                sim_stub!(
                    component = "gicv3-gicd",
                    "write unhandled offset={offset:#x} val={val:#x} (ignored)"
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gicv3::{build_gicv3, build_gicv3_mp};

    // ARM GICv3 Architecture spec offset constants
    const GICD_CTLR: u64 = 0x0000;
    const GICD_TYPER: u64 = 0x0004;
    const GICD_IIDR: u64 = 0x0008;
    const GICD_SETSPI_NSR: u64 = 0x0040;
    const GICD_CLRSPI_NSR: u64 = 0x0048;
    const GICD_IGROUPR: u64 = 0x0080;
    const GICD_ISENABLER: u64 = 0x0100;
    const GICD_ICENABLER: u64 = 0x0180;
    const GICD_ISPENDR: u64 = 0x0200;
    const GICD_ICPENDR: u64 = 0x0280;
    const GICD_ISACTIVER: u64 = 0x0300;
    const GICD_ICACTIVER: u64 = 0x0380;
    const GICD_IPRIORITYR: u64 = 0x0400;
    const GICD_ICFGR: u64 = 0x0C00;
    const GICD_IROUTER: u64 = 0x6000;
    const GICD_PIDR2: u64 = 0xFFE8;
    const GICD_PIDR2_LOW: u64 = 0x0FE8;

    fn make_gicd(num_irqs: u32) -> (Gicv3Distributor, Arc<Mutex<GicV3SharedState>>) {
        let (gicd, _gicr, _line, shared) = build_gicv3(num_irqs);
        (gicd, shared)
    }

    fn enable_cpu(shared: &Arc<Mutex<GicV3SharedState>>, cpu: usize) {
        let mut s = shared.lock().unwrap();
        s.dist.ctlr |= 0x3; // EnableGrp0 + EnableGrp1NS
        s.redists[cpu].cpu_if.icc_igrpen0 = 1;
        s.redists[cpu].cpu_if.icc_igrpen1 = 1;
        s.redists[cpu].cpu_if.icc_pmr = 0xFF;
        s.redists[cpu].waker = 0;
        s.redists[cpu].sgi_ppi_enabled = 0xFFFF_FFFF;
        s.redists[cpu].sgi_ppi_group = 0xFFFF_FFFF;
        for g in s.dist.group.iter_mut() {
            *g = 0xFFFF_FFFF;
        }
    }

    // ── Core read-only registers ──────────────────────────────────────

    #[test]
    fn ctlr_defaults_to_are_set() {
        let (mut gicd, _) = make_gicd(128);
        let ctlr = gicd.read(GICD_CTLR, 4) as u32;
        assert_eq!(ctlr & 0x30, 0x30, "ARE_NS and ARE_S must be set");
    }

    #[test]
    fn ctlr_write_preserves_are_bits() {
        let (mut gicd, _) = make_gicd(128);
        gicd.write(GICD_CTLR, 4, 0x07);
        let ctlr = gicd.read(GICD_CTLR, 4) as u32;
        assert_eq!(ctlr & 0x30, 0x30, "ARE bits must remain set");
        assert_eq!(ctlr & 0x07, 0x07, "Enable bits should be writable");
    }

    #[test]
    fn typer_reflects_it_lines() {
        let (mut gicd, _) = make_gicd(128);
        let typer = gicd.read(GICD_TYPER, 4) as u32;
        assert_eq!(typer & 0x1F, 3);
    }

    #[test]
    fn iidr_returns_fixed_value() {
        let (mut gicd, _) = make_gicd(128);
        assert_eq!(gicd.read(GICD_IIDR, 4), 0x0102_43B4);
    }

    #[test]
    fn pidr2_arch_rev_3() {
        let (mut gicd, _) = make_gicd(128);
        assert_eq!(gicd.read(GICD_PIDR2, 4), 0x3B);
    }

    #[test]
    fn pidr2_low_alias_arch_rev_3() {
        let (mut gicd, _) = make_gicd(128);
        assert_eq!(gicd.read(GICD_PIDR2_LOW, 4), 0x3B);
    }

    // ── GICD_IGROUPR (0x0080 + 4*n) ──────────────────────────────────

    #[test]
    fn igroupr_word0_raz_wi() {
        let (mut gicd, _) = make_gicd(128);
        gicd.write(GICD_IGROUPR, 4, 0xFFFF_FFFF);
        assert_eq!(gicd.read(GICD_IGROUPR, 4), 0);
    }

    #[test]
    fn igroupr_word1_maps_spi_32_63() {
        let (mut gicd, shared) = make_gicd(128);
        let off = GICD_IGROUPR + 4;
        gicd.write(off, 4, 0xDEAD_BEEF);
        assert_eq!(shared.lock().unwrap().dist.group[0], 0xDEAD_BEEF);
        assert_eq!(gicd.read(off, 4), 0xDEAD_BEEF);
    }

    #[test]
    fn igroupr_word2_maps_spi_64_95() {
        let (mut gicd, shared) = make_gicd(128);
        let off = GICD_IGROUPR + 8;
        gicd.write(off, 4, 0x1234_5678);
        assert_eq!(shared.lock().unwrap().dist.group[1], 0x1234_5678);
        assert_eq!(gicd.read(off, 4), 0x1234_5678);
    }

    // ── GICD_ISENABLER / GICD_ICENABLER ──────────────────────────────

    #[test]
    fn isenabler_word0_raz_wi() {
        let (mut gicd, _) = make_gicd(128);
        gicd.write(GICD_ISENABLER, 4, 0xFFFF_FFFF);
        assert_eq!(gicd.read(GICD_ISENABLER, 4), 0);
    }

    #[test]
    fn isenabler_word1_sets_spi_32_63() {
        let (mut gicd, shared) = make_gicd(128);
        let off = GICD_ISENABLER + 4;
        gicd.write(off, 4, 0x5);
        assert_eq!(shared.lock().unwrap().dist.enabled[0] & 0x5, 0x5);
        assert_eq!(gicd.read(off, 4) & 0x5, 0x5);
    }

    #[test]
    fn icenabler_word0_raz_wi() {
        let (mut gicd, _) = make_gicd(128);
        gicd.write(GICD_ICENABLER, 4, 0xFFFF_FFFF);
        assert_eq!(gicd.read(GICD_ICENABLER, 4), 0);
    }

    #[test]
    fn icenabler_clears_enabled_bits() {
        let (mut gicd, _) = make_gicd(128);
        gicd.write(GICD_ISENABLER + 4, 4, 0xFF);
        assert_eq!(gicd.read(GICD_ISENABLER + 4, 4) & 0xFF, 0xFF);
        gicd.write(GICD_ICENABLER + 4, 4, 0x0F);
        assert_eq!(gicd.read(GICD_ISENABLER + 4, 4) & 0xFF, 0xF0);
    }

    #[test]
    fn isenabler_icenabler_read_same_state() {
        let (mut gicd, _) = make_gicd(128);
        gicd.write(GICD_ISENABLER + 4, 4, 0xAA);
        assert_eq!(
            gicd.read(GICD_ISENABLER + 4, 4),
            gicd.read(GICD_ICENABLER + 4, 4)
        );
    }

    // ── GICD_ISPENDR / GICD_ICPENDR ──────────────────────────────────

    #[test]
    fn ispendr_word0_raz_wi() {
        let (mut gicd, _) = make_gicd(128);
        gicd.write(GICD_ISPENDR, 4, 0xFFFF_FFFF);
        assert_eq!(gicd.read(GICD_ISPENDR, 4), 0);
    }

    #[test]
    fn ispendr_icpendr_roundtrip() {
        let (mut gicd, shared) = make_gicd(128);
        gicd.write(GICD_ISPENDR + 4, 4, 0x3);
        assert_eq!(shared.lock().unwrap().dist.pending[0] & 0x3, 0x3);
        assert_eq!(gicd.read(GICD_ISPENDR + 4, 4) & 0x3, 0x3);
        gicd.write(GICD_ICPENDR + 4, 4, 0x1);
        assert_eq!(gicd.read(GICD_ISPENDR + 4, 4) & 0x3, 0x2);
    }

    #[test]
    fn ispendr_icpendr_read_same_state() {
        let (mut gicd, _) = make_gicd(128);
        gicd.write(GICD_ISPENDR + 4, 4, 0xBB);
        assert_eq!(
            gicd.read(GICD_ISPENDR + 4, 4),
            gicd.read(GICD_ICPENDR + 4, 4)
        );
    }

    // ── GICD_ISACTIVER / GICD_ICACTIVER ──────────────────────────────

    #[test]
    fn isactiver_word0_raz_wi() {
        let (mut gicd, _) = make_gicd(128);
        gicd.write(GICD_ISACTIVER, 4, 0xFFFF_FFFF);
        assert_eq!(gicd.read(GICD_ISACTIVER, 4), 0);
    }

    #[test]
    fn isactiver_icactiver_roundtrip() {
        let (mut gicd, shared) = make_gicd(128);
        gicd.write(GICD_ISACTIVER + 4, 4, 0x7);
        assert_eq!(shared.lock().unwrap().dist.active[0] & 0x7, 0x7);
        assert_eq!(gicd.read(GICD_ISACTIVER + 4, 4) & 0x7, 0x7);
        gicd.write(GICD_ICACTIVER + 4, 4, 0x3);
        assert_eq!(gicd.read(GICD_ISACTIVER + 4, 4) & 0x7, 0x4);
    }

    #[test]
    fn isactiver_icactiver_read_same_state() {
        let (mut gicd, _) = make_gicd(128);
        gicd.write(GICD_ISACTIVER + 4, 4, 0xCC);
        assert_eq!(
            gicd.read(GICD_ISACTIVER + 4, 4),
            gicd.read(GICD_ICACTIVER + 4, 4)
        );
    }

    // ── GICD_IPRIORITYR (0x0400 + n) ─────────────────────────────────

    #[test]
    fn ipriorityr_intid32_word_write_read() {
        let (mut gicd, shared) = make_gicd(128);
        let off = GICD_IPRIORITYR + 32;
        gicd.write(off, 4, 0x4433_2211);
        let s = shared.lock().unwrap();
        assert_eq!(s.dist.priority[32], 0x11, "INTID 32 priority");
        assert_eq!(s.dist.priority[33], 0x22, "INTID 33 priority");
        assert_eq!(s.dist.priority[34], 0x33, "INTID 34 priority");
        assert_eq!(s.dist.priority[35], 0x44, "INTID 35 priority");
        drop(s);
        assert_eq!(gicd.read(off, 4), 0x4433_2211);
    }

    #[test]
    fn ipriorityr_byte_write_read() {
        let (mut gicd, shared) = make_gicd(128);
        let off = GICD_IPRIORITYR + 33;
        gicd.write(off, 1, 0xAB);
        assert_eq!(shared.lock().unwrap().dist.priority[33], 0xAB);
        assert_eq!(gicd.read(off, 1), 0xAB);
    }

    #[test]
    fn ipriorityr_intid64_maps_correctly() {
        let (mut gicd, shared) = make_gicd(128);
        let off = GICD_IPRIORITYR + 64;
        gicd.write(off, 4, 0xAABB_CCDD);
        let s = shared.lock().unwrap();
        assert_eq!(s.dist.priority[64], 0xDD);
        assert_eq!(s.dist.priority[65], 0xCC);
        assert_eq!(s.dist.priority[66], 0xBB);
        assert_eq!(s.dist.priority[67], 0xAA);
        drop(s);
        assert_eq!(gicd.read(off, 4), 0xAABB_CCDD);
    }

    // ── GICD_ICFGR (0x0C00 + 4*n) ───────────────────────────────────

    #[test]
    fn icfgr_spi_word_roundtrip() {
        let (mut gicd, shared) = make_gicd(128);
        let off = GICD_ICFGR + 8;
        gicd.write(off, 4, 0xAAAA_AAAA);
        assert_eq!(shared.lock().unwrap().dist.config[2], 0xAAAA_AAAA);
        assert_eq!(gicd.read(off, 4), 0xAAAA_AAAA);
    }

    // ── GICD_IROUTER (0x6000 + 8*n) ─────────────────────────────────

    #[test]
    fn irouter_intid_below_32_raz_wi() {
        let (mut gicd, _) = make_gicd(128);
        gicd.write(0x6000, 8, 0xDEAD);
        assert_eq!(gicd.read(0x6000, 8), 0);
    }

    #[test]
    fn irouter_intid32_roundtrip_64bit() {
        let (mut gicd, shared) = make_gicd(128);
        let off = GICD_IROUTER + 8 * 32;
        gicd.write(off, 8, 0x8000_0000_0000_0001);
        assert_eq!(
            shared.lock().unwrap().dist.irouter[0],
            0x8000_0000_0000_0001
        );
        assert_eq!(gicd.read(off, 8), 0x8000_0000_0000_0001);
    }

    #[test]
    fn irouter_intid32_split_32bit_writes() {
        let (mut gicd, shared) = make_gicd(128);
        let off = GICD_IROUTER + 8 * 32;
        gicd.write(off, 4, 0x0000_0002);
        gicd.write(off + 4, 4, 0x0000_0003);
        let val = shared.lock().unwrap().dist.irouter[0];
        assert_eq!(val & 0xFFFF_FFFF, 0x0000_0002);
        assert_eq!(val >> 32, 0x0000_0003);
        assert_eq!(gicd.read(off, 4), 0x0000_0002);
        assert_eq!(gicd.read(off + 4, 4), 0x0000_0003);
    }

    // ── SETSPI / CLRSPI ─────────────────────────────────────────────

    #[test]
    fn setspi_clrspi_assert_deassert() {
        let (mut gicd, shared) = make_gicd(128);
        gicd.write(GICD_SETSPI_NSR, 4, 32);
        assert_ne!(shared.lock().unwrap().dist.pending[0] & 1, 0);
        gicd.write(GICD_CLRSPI_NSR, 4, 32);
        assert_eq!(shared.lock().unwrap().dist.pending[0] & 1, 0);
    }

    #[test]
    fn setspi_clrspi_read_zero() {
        let (mut gicd, _) = make_gicd(128);
        assert_eq!(gicd.read(GICD_SETSPI_NSR, 4), 0);
        assert_eq!(gicd.read(GICD_CLRSPI_NSR, 4), 0);
    }

    // ── Cross-check: MMIO write <-> functional delivery ──────────────

    #[test]
    fn mmio_priority_matches_functional_delivery() {
        let (mut gicd, shared) = make_gicd(128);
        enable_cpu(&shared, 0);

        {
            let mut s = shared.lock().unwrap();
            s.dist.irouter[0] = 0;
            s.dist.enabled[0] = 1;
        }

        gicd.write(GICD_IPRIORITYR + 32, 1, 0x40);
        assert_eq!(
            shared.lock().unwrap().dist.priority[32],
            0x40,
            "MMIO must land in priority[32]"
        );

        shared.lock().unwrap().assert_spi(32);
        let pending = shared.lock().unwrap().highest_pending_for_cpu(0);
        assert_eq!(pending, Some((32, 0x40)));
    }

    #[test]
    fn mmio_priority_masking_via_mmio_write() {
        let (mut gicd, shared) = make_gicd(128);
        enable_cpu(&shared, 0);

        {
            let mut s = shared.lock().unwrap();
            s.dist.irouter[0] = 0;
            s.dist.enabled[0] = 1;
            s.redists[0].cpu_if.icc_pmr = 0x20;
        }

        gicd.write(GICD_IPRIORITYR + 32, 1, 0x40);
        shared.lock().unwrap().assert_spi(32);
        assert!(
            shared.lock().unwrap().highest_pending_for_cpu(0).is_none(),
            "prio 0x40 must be masked by PMR 0x20"
        );

        gicd.write(GICD_IPRIORITYR + 32, 1, 0x10);
        assert_eq!(
            shared.lock().unwrap().highest_pending_for_cpu(0),
            Some((32, 0x10))
        );
    }

    #[test]
    fn mmio_enable_matches_functional_delivery() {
        let (mut gicd, shared) = make_gicd(128);
        enable_cpu(&shared, 0);

        {
            let mut s = shared.lock().unwrap();
            s.dist.irouter[0] = 0;
            s.dist.priority[32] = 0x40;
        }

        shared.lock().unwrap().assert_spi(32);
        assert!(shared.lock().unwrap().highest_pending_for_cpu(0).is_none());

        gicd.write(GICD_ISENABLER + 4, 4, 0x1);
        assert!(shared.lock().unwrap().highest_pending_for_cpu(0).is_some());

        gicd.write(GICD_ICENABLER + 4, 4, 0x1);
        assert!(shared.lock().unwrap().highest_pending_for_cpu(0).is_none());
    }

    #[test]
    fn mmio_irouter_determines_spi_target() {
        let affinities = &[0u64, 1u64];
        let (mut gicd, _gicrs, _lines, shared) = build_gicv3_mp(128, 2, affinities);
        enable_cpu(&shared, 0);
        enable_cpu(&shared, 1);

        {
            let mut s = shared.lock().unwrap();
            s.dist.enabled[0] = 1;
            s.dist.priority[32] = 0x40;
        }

        gicd.write(GICD_IROUTER + 8 * 32, 8, 1);
        shared.lock().unwrap().assert_spi(32);
        assert!(
            shared.lock().unwrap().highest_pending_for_cpu(0).is_none(),
            "cpu 0 must not see SPI routed to cpu 1"
        );
        assert!(
            shared.lock().unwrap().highest_pending_for_cpu(1).is_some(),
            "cpu 1 must see SPI routed to it"
        );
    }

    // ── Sweep: every register group maps to correct array index ──────

    #[test]
    fn sweep_igroupr_all_spi_words() {
        let (mut gicd, shared) = make_gicd(256);
        for n in 1..8u64 {
            let off = GICD_IGROUPR + 4 * n;
            let val = 0x1000_0000 | n as u64;
            gicd.write(off, 4, val);
            assert_eq!(
                shared.lock().unwrap().dist.group[(n - 1) as usize],
                val as u32,
                "IGROUPR word {n} at {off:#x}"
            );
            assert_eq!(gicd.read(off, 4), val);
        }
    }

    #[test]
    fn sweep_isenabler_all_spi_words() {
        let (mut gicd, shared) = make_gicd(256);
        for n in 1..8u64 {
            let off = GICD_ISENABLER + 4 * n;
            gicd.write(off, 4, 1 << (n as u32));
            assert_ne!(
                shared.lock().unwrap().dist.enabled[(n - 1) as usize] & (1 << n),
                0,
                "ISENABLER word {n} at {off:#x}"
            );
        }
    }

    #[test]
    fn sweep_ipriorityr_spi_range() {
        let (mut gicd, shared) = make_gicd(256);
        for intid in (32..256u64).step_by(4) {
            let off = GICD_IPRIORITYR + intid;
            let val = (intid & 0xFF) as u64;
            gicd.write(off, 1, val);
            assert_eq!(
                shared.lock().unwrap().dist.priority[intid as usize],
                val as u8,
                "IPRIORITYR INTID {intid} at {off:#x}"
            );
            assert_eq!(gicd.read(off, 1), val);
        }
    }

    #[test]
    fn sweep_icfgr_spi_words() {
        let (mut gicd, shared) = make_gicd(256);
        for n in 2..16u64 {
            let off = GICD_ICFGR + 4 * n;
            let val = 0xAAAA_0000 | n as u64;
            gicd.write(off, 4, val);
            assert_eq!(
                shared.lock().unwrap().dist.config[n as usize],
                val as u32,
                "ICFGR word {n} at {off:#x}"
            );
            assert_eq!(gicd.read(off, 4), val);
        }
    }

    #[test]
    fn sweep_irouter_spi_range() {
        let (mut gicd, shared) = make_gicd(256);
        for intid in [32u64, 64, 128, 200] {
            let off = GICD_IROUTER + 8 * intid;
            let val = 0x8000_0000_0000_0000 | intid;
            gicd.write(off, 8, val);
            assert_eq!(
                shared.lock().unwrap().dist.irouter[(intid - 32) as usize],
                val,
                "IROUTER INTID {intid} at {off:#x}"
            );
            assert_eq!(gicd.read(off, 8), val);
        }
    }
}
