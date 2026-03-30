//! GICv3 Redistributor (GICR) — 128KB per-PE MMIO region.
//!
//! Layout: RD_base (0x0000–0xFFFF) + SGI_base (0x10000–0x1FFFF).

use super::GicV3SharedState;
use helm_devices::Device;
use helm_diag::sim_stub;
use std::sync::{Arc, Mutex};

pub struct Gicv3Redistributor {
    pub shared: Arc<Mutex<GicV3SharedState>>,
    pub cpu_idx: usize,
}

impl Gicv3Redistributor {
    pub fn new(shared: Arc<Mutex<GicV3SharedState>>, cpu_idx: usize) -> Self {
        Self { shared, cpu_idx }
    }
}

impl Device for Gicv3Redistributor {
    fn region_size(&self) -> u64 {
        0x2_0000
    } // 128KB

    fn read(&mut self, offset: u64, size: usize) -> u64 {
        let s = self.shared.lock().unwrap();
        let Some(redist) = s.redists.get(self.cpu_idx) else {
            return 0;
        };

        if offset < 0x10000 {
            // ── RD_base ───────────────────────────────────────────────────────
            match offset {
                0x0000 => u64::from(redist.ctlr),
                0x0004 => 0x0102_43B4, // GICR_IIDR
                0x0008 => {
                    // GICR_TYPER (64-bit)
                    if size == 4 {
                        redist.typer & 0xFFFF_FFFF
                    } else {
                        redist.typer
                    }
                }
                0x000C => {
                    // GICR_TYPER high word
                    redist.typer >> 32
                }
                0x0010 => 0, // GICR_STATUSR
                0x0014 => u64::from(redist.waker),
                0x0040 => 0, // GICR_SETLPIR (WO)
                0x0048 => 0, // GICR_CLRLPIR (WO)
                0x0070 => {
                    // GICR_PROPBASER (64-bit)
                    if size == 4 {
                        redist.propbaser & 0xFFFF_FFFF
                    } else {
                        redist.propbaser
                    }
                }
                0x0074 => redist.propbaser >> 32,
                0x0078 => {
                    // GICR_PENDBASER (64-bit)
                    if size == 4 {
                        redist.pendbaser & 0xFFFF_FFFF
                    } else {
                        redist.pendbaser
                    }
                }
                0x007C => redist.pendbaser >> 32,
                0xFFE8 => 0x3B, // GICR_PIDR2: ArchRev=3
                0xFFD0 => 0,    // GICR_PIDR4
                _ => {
                    sim_stub!(
                        component = "gicv3-gicr-rd",
                        "read unhandled RD_base offset={offset:#x} -> 0"
                    );
                    0
                }
            }
        } else {
            // ── SGI_base (subtract 0x10000) ───────────────────────────────────
            let sgi_off = offset - 0x10000;
            match sgi_off {
                0x0080 => u64::from(redist.sgi_ppi_group),
                // ISENABLER0 and ICENABLER0 read the same state
                0x0100 | 0x0180 => u64::from(redist.sgi_ppi_enabled),
                // ISPENDR0 and ICPENDR0
                0x0200 | 0x0280 => u64::from(redist.sgi_ppi_pending),
                // ISACTIVER0 and ICACTIVER0
                0x0300 | 0x0380 => u64::from(redist.sgi_ppi_active),
                // GICR_IPRIORITYR[0..7]: 4 bytes per word
                o @ 0x0400..=0x041C => {
                    let byte_base = (o - 0x0400) as usize;
                    if size == 1 {
                        redist.sgi_ppi_priority.get(byte_base).copied().unwrap_or(0) as u64
                    } else {
                        let p = &redist.sgi_ppi_priority;
                        let b0 = p.get(byte_base).copied().unwrap_or(0) as u64;
                        let b1 = p.get(byte_base + 1).copied().unwrap_or(0) as u64;
                        let b2 = p.get(byte_base + 2).copied().unwrap_or(0) as u64;
                        let b3 = p.get(byte_base + 3).copied().unwrap_or(0) as u64;
                        b0 | (b1 << 8) | (b2 << 16) | (b3 << 24)
                    }
                }
                0x0C00 => u64::from(redist.sgi_ppi_config[0]),
                0x0C04 => u64::from(redist.sgi_ppi_config[1]),
                _ => {
                    sim_stub!(
                        component = "gicv3-gicr-sgi",
                        "read unhandled SGI_base offset={sgi_off:#x} -> 0"
                    );
                    0
                }
            }
        }
    }

    fn write(&mut self, offset: u64, size: usize, val: u64) {
        let val32 = val as u32;
        let mut s = self.shared.lock().unwrap();
        let Some(redist) = s.redists.get_mut(self.cpu_idx) else {
            return;
        };

        if offset < 0x10000 {
            // ── RD_base ───────────────────────────────────────────────────────
            match offset {
                0x0000 => {
                    // EnableLPIs is sticky once set
                    if redist.ctlr & 1 == 0 {
                        redist.ctlr = val32 & 0x1;
                    }
                }
                0x0010 => {} // STATUSR: W1C, ignore
                0x0014 => {
                    // GICR_WAKER: only ProcessorSleep[1] is RW
                    redist.waker = (redist.waker & !0x2) | (val32 & 0x2);
                    // ChildrenAsleep[2] = RO (GIC-driven, 0 in sim)
                }
                0x0040 => {} // GICR_SETLPIR: Phase 2
                0x0048 => {} // GICR_CLRLPIR: Phase 2
                0x0070 => {
                    // GICR_PROPBASER low word
                    if size == 4 {
                        redist.propbaser = (redist.propbaser & !0xFFFF_FFFF) | (val & 0xFFFF_FFFF);
                    } else {
                        redist.propbaser = val;
                    }
                }
                0x0074 => {
                    redist.propbaser = (redist.propbaser & 0xFFFF_FFFF) | (val << 32);
                }
                0x0078 => {
                    // GICR_PENDBASER low word
                    if size == 4 {
                        redist.pendbaser = (redist.pendbaser & !0xFFFF_FFFF) | (val & 0xFFFF_FFFF);
                    } else {
                        redist.pendbaser = val;
                    }
                }
                0x007C => {
                    redist.pendbaser = (redist.pendbaser & 0xFFFF_FFFF) | (val << 32);
                }
                _ => {
                    sim_stub!(
                        component = "gicv3-gicr-rd",
                        "write unhandled RD_base offset={offset:#x} val={val:#x}"
                    );
                }
            }
        } else {
            // ── SGI_base ──────────────────────────────────────────────────────
            let sgi_off = offset - 0x10000;
            match sgi_off {
                0x0080 => {
                    redist.sgi_ppi_group = val32;
                }
                0x0100 => {
                    // ISENABLER0
                    redist.sgi_ppi_enabled |= val32;
                    let cpu_idx = self.cpu_idx;
                    let _ = redist;
                    s.update_irq_line(cpu_idx);
                    return;
                }
                0x0180 => {
                    // ICENABLER0
                    redist.sgi_ppi_enabled &= !val32;
                    let cpu_idx = self.cpu_idx;
                    let _ = redist;
                    s.update_irq_line(cpu_idx);
                    return;
                }
                0x0200 => {
                    // ISPENDR0
                    redist.sgi_ppi_pending |= val32;
                    let cpu_idx = self.cpu_idx;
                    let _ = redist;
                    s.update_irq_line(cpu_idx);
                    return;
                }
                0x0280 => {
                    // ICPENDR0
                    redist.sgi_ppi_pending &= !val32;
                    let cpu_idx = self.cpu_idx;
                    let _ = redist;
                    s.update_irq_line(cpu_idx);
                    return;
                }
                0x0300 => {
                    redist.sgi_ppi_active |= val32;
                }
                0x0380 => {
                    // ICACTIVER0
                    redist.sgi_ppi_active &= !val32;
                    let cpu_idx = self.cpu_idx;
                    let _ = redist;
                    s.update_irq_line(cpu_idx);
                    return;
                }
                o @ 0x0400..=0x041C => {
                    let byte_base = (o - 0x0400) as usize;
                    if size == 1 {
                        if let Some(b) = redist.sgi_ppi_priority.get_mut(byte_base) {
                            *b = val as u8;
                        }
                    } else {
                        for i in 0..4usize {
                            if let Some(b) = redist.sgi_ppi_priority.get_mut(byte_base + i) {
                                *b = ((val >> (i * 8)) & 0xFF) as u8;
                            }
                        }
                    }
                }
                0x0C00 => {
                    redist.sgi_ppi_config[0] = val32;
                }
                0x0C04 => {
                    redist.sgi_ppi_config[1] = val32;
                }
                _ => {
                    sim_stub!(
                        component = "gicv3-gicr-sgi",
                        "write unhandled SGI_base offset={sgi_off:#x} val={val:#x}"
                    );
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gicv3::build_gicv3;

    // ARM spec GICR SGI_base offsets (relative to SGI_base = GICR_base + 0x10000)
    const SGI_IGROUPR0: u64 = 0x10080;
    const SGI_ISENABLER0: u64 = 0x10100;
    const SGI_ICENABLER0: u64 = 0x10180;
    const SGI_ISPENDR0: u64 = 0x10200;
    const SGI_ICPENDR0: u64 = 0x10280;
    const SGI_ISACTIVER0: u64 = 0x10300;
    const SGI_ICACTIVER0: u64 = 0x10380;
    const SGI_IPRIORITYR0: u64 = 0x10400;
    const SGI_ICFGR0: u64 = 0x10C00;
    const SGI_ICFGR1: u64 = 0x10C04;

    // RD_base offsets
    #[allow(dead_code)]
    const GICR_CTLR: u64 = 0x0000;
    const GICR_TYPER: u64 = 0x0008;
    const GICR_WAKER: u64 = 0x0014;
    const GICR_PIDR2: u64 = 0xFFE8;

    fn make_gicr() -> (Gicv3Redistributor, Arc<Mutex<GicV3SharedState>>) {
        let (_gicd, gicr, _line, shared) = build_gicv3(128);
        (gicr, shared)
    }

    // ── RD_base registers ────────────────────────────────────────────

    #[test]
    fn gicr_typer_returns_affinity_and_flags() {
        let (mut gicr, _) = make_gicr();
        let typer_lo = gicr.read(GICR_TYPER, 4);
        // Last bit [4] should be set (single CPU), DirectLPI [3], PLPIS [0]
        assert_ne!(typer_lo & (1 << 4), 0, "Last bit");
        assert_ne!(typer_lo & (1 << 3), 0, "DirectLPI");
        assert_ne!(typer_lo & 1, 0, "PLPIS");
    }

    #[test]
    fn gicr_waker_processor_sleep_rw() {
        let (mut gicr, _) = make_gicr();
        assert_eq!(gicr.read(GICR_WAKER, 4) & 0x2, 0);
        gicr.write(GICR_WAKER, 4, 0x2);
        assert_eq!(gicr.read(GICR_WAKER, 4) & 0x2, 0x2);
        gicr.write(GICR_WAKER, 4, 0x0);
        assert_eq!(gicr.read(GICR_WAKER, 4) & 0x2, 0);
    }

    #[test]
    fn gicr_pidr2_arch_rev_3() {
        let (mut gicr, _) = make_gicr();
        assert_eq!(gicr.read(GICR_PIDR2, 4), 0x3B);
    }

    // ── SGI_base registers ───────────────────────────────────────────

    #[test]
    fn sgi_igroupr0_roundtrip() {
        let (mut gicr, shared) = make_gicr();
        gicr.write(SGI_IGROUPR0, 4, 0xFFFF_0000);
        assert_eq!(shared.lock().unwrap().redists[0].sgi_ppi_group, 0xFFFF_0000);
        assert_eq!(gicr.read(SGI_IGROUPR0, 4), 0xFFFF_0000);
    }

    #[test]
    fn sgi_isenabler_icenabler_roundtrip() {
        let (mut gicr, shared) = make_gicr();
        gicr.write(SGI_ISENABLER0, 4, 0xFF);
        assert_eq!(
            shared.lock().unwrap().redists[0].sgi_ppi_enabled & 0xFF,
            0xFF
        );
        assert_eq!(gicr.read(SGI_ISENABLER0, 4) & 0xFF, 0xFF);
        gicr.write(SGI_ICENABLER0, 4, 0x0F);
        assert_eq!(gicr.read(SGI_ISENABLER0, 4) & 0xFF, 0xF0);
    }

    #[test]
    fn sgi_isenabler_icenabler_read_same() {
        let (mut gicr, _) = make_gicr();
        gicr.write(SGI_ISENABLER0, 4, 0xAA);
        assert_eq!(gicr.read(SGI_ISENABLER0, 4), gicr.read(SGI_ICENABLER0, 4));
    }

    #[test]
    fn sgi_ispendr_icpendr_roundtrip() {
        let (mut gicr, shared) = make_gicr();
        gicr.write(SGI_ISPENDR0, 4, 0x5);
        assert_eq!(shared.lock().unwrap().redists[0].sgi_ppi_pending & 0x5, 0x5);
        assert_eq!(gicr.read(SGI_ISPENDR0, 4) & 0x5, 0x5);
        gicr.write(SGI_ICPENDR0, 4, 0x1);
        assert_eq!(gicr.read(SGI_ISPENDR0, 4) & 0x5, 0x4);
    }

    #[test]
    fn sgi_ispendr_icpendr_read_same() {
        let (mut gicr, _) = make_gicr();
        gicr.write(SGI_ISPENDR0, 4, 0xBB);
        assert_eq!(gicr.read(SGI_ISPENDR0, 4), gicr.read(SGI_ICPENDR0, 4));
    }

    #[test]
    fn sgi_isactiver_icactiver_roundtrip() {
        let (mut gicr, shared) = make_gicr();
        gicr.write(SGI_ISACTIVER0, 4, 0x7);
        assert_eq!(shared.lock().unwrap().redists[0].sgi_ppi_active & 0x7, 0x7);
        assert_eq!(gicr.read(SGI_ISACTIVER0, 4) & 0x7, 0x7);
        gicr.write(SGI_ICACTIVER0, 4, 0x3);
        assert_eq!(gicr.read(SGI_ISACTIVER0, 4) & 0x7, 0x4);
    }

    #[test]
    fn sgi_isactiver_icactiver_read_same() {
        let (mut gicr, _) = make_gicr();
        gicr.write(SGI_ISACTIVER0, 4, 0xCC);
        assert_eq!(gicr.read(SGI_ISACTIVER0, 4), gicr.read(SGI_ICACTIVER0, 4));
    }

    #[test]
    fn sgi_ipriorityr_word_write_read() {
        let (mut gicr, shared) = make_gicr();
        gicr.write(SGI_IPRIORITYR0, 4, 0x4433_2211);
        let s = shared.lock().unwrap();
        assert_eq!(s.redists[0].sgi_ppi_priority[0], 0x11);
        assert_eq!(s.redists[0].sgi_ppi_priority[1], 0x22);
        assert_eq!(s.redists[0].sgi_ppi_priority[2], 0x33);
        assert_eq!(s.redists[0].sgi_ppi_priority[3], 0x44);
        drop(s);
        assert_eq!(gicr.read(SGI_IPRIORITYR0, 4), 0x4433_2211);
    }

    #[test]
    fn sgi_ipriorityr_byte_write_read() {
        let (mut gicr, shared) = make_gicr();
        gicr.write(SGI_IPRIORITYR0 + 5, 1, 0xEE);
        assert_eq!(shared.lock().unwrap().redists[0].sgi_ppi_priority[5], 0xEE);
        assert_eq!(gicr.read(SGI_IPRIORITYR0 + 5, 1), 0xEE);
    }

    #[test]
    fn sgi_icfgr_roundtrip() {
        let (mut gicr, shared) = make_gicr();
        gicr.write(SGI_ICFGR0, 4, 0x5555_5555);
        assert_eq!(
            shared.lock().unwrap().redists[0].sgi_ppi_config[0],
            0x5555_5555
        );
        assert_eq!(gicr.read(SGI_ICFGR0, 4), 0x5555_5555);
        gicr.write(SGI_ICFGR1, 4, 0xAAAA_AAAA);
        assert_eq!(
            shared.lock().unwrap().redists[0].sgi_ppi_config[1],
            0xAAAA_AAAA
        );
        assert_eq!(gicr.read(SGI_ICFGR1, 4), 0xAAAA_AAAA);
    }
}
