//! GICv3 Redistributor (GICR) — 128KB per-PE MMIO region.
//!
//! Layout: RD_base (0x0000–0xFFFF) + SGI_base (0x10000–0x1FFFF).

use std::sync::{Arc, Mutex};
use helm_devices::Device;
use helm_diag::sim_stub;
use super::GicV3SharedState;

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
    fn region_size(&self) -> u64 { 0x2_0000 } // 128KB

    fn read(&mut self, offset: u64, size: usize) -> u64 {
        let s = self.shared.lock().unwrap();
        let Some(redist) = s.redists.get(self.cpu_idx) else { return 0; };

        if offset < 0x10000 {
            // ── RD_base ───────────────────────────────────────────────────────
            match offset {
                0x0000 => u64::from(redist.ctlr),
                0x0004 => 0x0102_43B4,          // GICR_IIDR
                0x0008 => {                      // GICR_TYPER (64-bit)
                    if size == 4 { redist.typer & 0xFFFF_FFFF }
                    else { redist.typer }
                }
                0x000C => {                      // GICR_TYPER high word
                    redist.typer >> 32
                }
                0x0010 => 0,                     // GICR_STATUSR
                0x0014 => u64::from(redist.waker),
                0x0040 => 0,                     // GICR_SETLPIR (WO)
                0x0048 => 0,                     // GICR_CLRLPIR (WO)
                0x0070 => {                      // GICR_PROPBASER (64-bit)
                    if size == 4 { redist.propbaser & 0xFFFF_FFFF }
                    else { redist.propbaser }
                }
                0x0074 => redist.propbaser >> 32,
                0x0078 => {                      // GICR_PENDBASER (64-bit)
                    if size == 4 { redist.pendbaser & 0xFFFF_FFFF }
                    else { redist.pendbaser }
                }
                0x007C => redist.pendbaser >> 32,
                0xFFD0 => 0x3B,                  // GICR_PIDR2
                _ => {
                    sim_stub!(component="gicv3-gicr-rd",
                        "read unhandled RD_base offset={offset:#x} -> 0");
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
                        let b1 = p.get(byte_base+1).copied().unwrap_or(0) as u64;
                        let b2 = p.get(byte_base+2).copied().unwrap_or(0) as u64;
                        let b3 = p.get(byte_base+3).copied().unwrap_or(0) as u64;
                        b0 | (b1 << 8) | (b2 << 16) | (b3 << 24)
                    }
                }
                0x0C00 => u64::from(redist.sgi_ppi_config[0]),
                0x0C04 => u64::from(redist.sgi_ppi_config[1]),
                _ => {
                    sim_stub!(component="gicv3-gicr-sgi",
                        "read unhandled SGI_base offset={sgi_off:#x} -> 0");
                    0
                }
            }
        }
    }

    fn write(&mut self, offset: u64, size: usize, val: u64) {
        let val32 = val as u32;
        let mut s = self.shared.lock().unwrap();
        let Some(redist) = s.redists.get_mut(self.cpu_idx) else { return; };

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
                0x0070 => {  // GICR_PROPBASER low word
                    if size == 4 {
                        redist.propbaser = (redist.propbaser & !0xFFFF_FFFF) | (val & 0xFFFF_FFFF);
                    } else {
                        redist.propbaser = val;
                    }
                }
                0x0074 => { redist.propbaser = (redist.propbaser & 0xFFFF_FFFF) | (val << 32); }
                0x0078 => {  // GICR_PENDBASER low word
                    if size == 4 {
                        redist.pendbaser = (redist.pendbaser & !0xFFFF_FFFF) | (val & 0xFFFF_FFFF);
                    } else {
                        redist.pendbaser = val;
                    }
                }
                0x007C => { redist.pendbaser = (redist.pendbaser & 0xFFFF_FFFF) | (val << 32); }
                _ => {
                    sim_stub!(component="gicv3-gicr-rd",
                        "write unhandled RD_base offset={offset:#x} val={val:#x}");
                }
            }
        } else {
            // ── SGI_base ──────────────────────────────────────────────────────
            let sgi_off = offset - 0x10000;
            match sgi_off {
                0x0080 => { redist.sgi_ppi_group = val32; }
                0x0100 => { // ISENABLER0
                    redist.sgi_ppi_enabled |= val32;
                    let cpu_idx = self.cpu_idx;
                    let _ = redist;
                    s.update_irq_line(cpu_idx);
                    return;
                }
                0x0180 => { // ICENABLER0
                    redist.sgi_ppi_enabled &= !val32;
                    let cpu_idx = self.cpu_idx;
                    let _ = redist;
                    s.update_irq_line(cpu_idx);
                    return;
                }
                0x0200 => { // ISPENDR0
                    redist.sgi_ppi_pending |= val32;
                    let cpu_idx = self.cpu_idx;
                    let _ = redist;
                    s.update_irq_line(cpu_idx);
                    return;
                }
                0x0280 => { // ICPENDR0
                    redist.sgi_ppi_pending &= !val32;
                    let cpu_idx = self.cpu_idx;
                    let _ = redist;
                    s.update_irq_line(cpu_idx);
                    return;
                }
                0x0300 => { redist.sgi_ppi_active |= val32; }
                0x0380 => { // ICACTIVER0
                    redist.sgi_ppi_active &= !val32;
                    let cpu_idx = self.cpu_idx;
                    let _ = redist;
                    s.update_irq_line(cpu_idx);
                    return;
                }
                o @ 0x0400..=0x041C => {
                    let byte_base = (o - 0x0400) as usize;
                    if size == 1 {
                        if let Some(b) = redist.sgi_ppi_priority.get_mut(byte_base) { *b = val as u8; }
                    } else {
                        for i in 0..4usize {
                            if let Some(b) = redist.sgi_ppi_priority.get_mut(byte_base + i) {
                                *b = ((val >> (i * 8)) & 0xFF) as u8;
                            }
                        }
                    }
                }
                0x0C00 => { redist.sgi_ppi_config[0] = val32; }
                0x0C04 => { redist.sgi_ppi_config[1] = val32; }
                _ => {
                    sim_stub!(component="gicv3-gicr-sgi",
                        "write unhandled SGI_base offset={sgi_off:#x} val={val:#x}");
                }
            }
        }
    }
}
