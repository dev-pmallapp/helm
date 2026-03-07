//! ARMv8.2 MMU page table walker.
//!
//! Implements the AArch64 Virtual Memory System Architecture (VMSA) with:
//! - 4K, 16K, and 64K granule support
//! - 4-level page table walk (L0→L1→L2→L3)
//! - Block and page descriptors
//! - TCR_EL1-driven TTBR0/TTBR1 VA space split
//! - Permission extraction (AP, PXN, UXN)
//! - Access Flag checking
//!
//! The walker is a pure function: given VA + translation registers + a physical
//! memory reader, it produces PA + permissions or a translation fault.

/// Page granule size.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Granule {
    K4,
    K16,
    K64,
}

impl Granule {
    /// Page size in bytes.
    pub fn size(self) -> u64 {
        match self {
            Granule::K4 => 4096,
            Granule::K16 => 16384,
            Granule::K64 => 65536,
        }
    }

    /// Number of bits for the page offset.
    pub fn page_shift(self) -> u32 {
        match self {
            Granule::K4 => 12,
            Granule::K16 => 14,
            Granule::K64 => 16,
        }
    }

    /// Number of index bits per table level.
    pub fn bits_per_level(self) -> u32 {
        match self {
            Granule::K4 => 9,   // 512 entries
            Granule::K16 => 11, // 2048 entries
            Granule::K64 => 13, // 8192 entries
        }
    }
}

/// Which TTBR to use for a VA.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TtbrSelect {
    Ttbr0,
    Ttbr1,
    Fault,
}

/// Parsed TCR fields for split VA space (EL1, or EL2 with VHE).
#[derive(Debug, Clone)]
pub struct TranslationConfig {
    pub t0sz: u32,
    pub t1sz: u32,
    pub tg0: Granule,
    pub tg1: Granule,
    pub ips: u32,
    pub epd0: bool,
    pub epd1: bool,
    pub a1: bool,
    pub asid_16bit: bool,
    pub ha: bool,
    pub hd: bool,
}

impl TranslationConfig {
    /// Parse from raw TCR_EL1 (or TCR_EL2 with VHE) — split VA space.
    pub fn parse(tcr: u64) -> Self {
        let t0sz = (tcr & 0x3F) as u32;
        let t1sz = ((tcr >> 16) & 0x3F) as u32;
        let tg0 = match (tcr >> 14) & 3 {
            0 => Granule::K4,
            1 => Granule::K64,
            2 => Granule::K16,
            _ => Granule::K4,
        };
        let tg1 = match (tcr >> 30) & 3 {
            1 => Granule::K16,
            2 => Granule::K4,  // note: TG1 encoding differs from TG0
            3 => Granule::K64,
            _ => Granule::K4,
        };
        Self {
            t0sz,
            t1sz,
            tg0,
            tg1,
            ips: ((tcr >> 32) & 7) as u32,
            epd0: (tcr >> 7) & 1 != 0,
            epd1: (tcr >> 23) & 1 != 0,
            a1: (tcr >> 22) & 1 != 0,
            asid_16bit: (tcr >> 36) & 1 != 0,
            ha: (tcr >> 39) & 1 != 0,
            hd: (tcr >> 40) & 1 != 0,
        }
    }

    /// Parse from raw TCR_EL2 (non-VHE) or TCR_EL3 — single VA space.
    ///
    /// EL2 (non-VHE) and EL3 use only TTBR0 with T0SZ. The upper VA range
    /// (TTBR1) is disabled by setting EPD1 and T1SZ to produce a zero-size range.
    pub fn parse_single(tcr: u64) -> Self {
        let t0sz = (tcr & 0x3F) as u32;
        let tg0 = match (tcr >> 14) & 3 {
            0 => Granule::K4,
            1 => Granule::K64,
            2 => Granule::K16,
            _ => Granule::K4,
        };
        Self {
            t0sz,
            t1sz: 64, // disable TTBR1 range (IA bits = 0)
            tg0,
            tg1: Granule::K4, // unused
            ips: ((tcr >> 16) & 7) as u32, // PS field at [18:16] for EL2/EL3
            epd0: false,
            epd1: true, // no TTBR1
            a1: false,
            asid_16bit: false,
            ha: (tcr >> 21) & 1 != 0, // HA at bit 21 for EL2/EL3
            hd: (tcr >> 22) & 1 != 0, // HD at bit 22 for EL2/EL3
        }
    }
}

/// Determine which TTBR to use based on VA and TCR.
pub fn select_ttbr(va: u64, tcr: &TranslationConfig) -> TtbrSelect {
    let ia_bits_0 = 64u32.saturating_sub(tcr.t0sz);
    let ia_bits_1 = 64u32.saturating_sub(tcr.t1sz);

    // Check if VA is in the TTBR0 range: top bits [63:ia_bits_0] must be all-zero
    if ia_bits_0 > 0 && ia_bits_0 < 64 {
        let top_mask = !((1u64 << ia_bits_0) - 1);
        if va & top_mask == 0 {
            return TtbrSelect::Ttbr0;
        }
    } else if ia_bits_0 >= 64 {
        // All of VA space belongs to TTBR0
        return TtbrSelect::Ttbr0;
    }

    // Check if VA is in the TTBR1 range: top bits [63:ia_bits_1] must be all-one
    if ia_bits_1 > 0 && ia_bits_1 < 64 {
        let top_mask = !((1u64 << ia_bits_1) - 1);
        if va & top_mask == top_mask {
            return TtbrSelect::Ttbr1;
        }
    }

    TtbrSelect::Fault
}

/// A parsed page table entry.
#[derive(Debug, Clone, Copy)]
pub struct Pte(pub u64);

impl Pte {
    pub fn is_valid(self) -> bool { self.0 & 1 != 0 }
    /// Table descriptor at L0-L2 (bits[1:0] = 0b11).
    pub fn is_table(self) -> bool { self.0 & 3 == 3 }
    /// Block descriptor at L0-L2 (bits[1:0] = 0b01).
    pub fn is_block(self) -> bool { self.0 & 3 == 1 }

    /// Output address — bits [47:page_shift], masked for the level's block size.
    pub fn oa(self, block_shift: u32) -> u64 {
        self.0 & oa_mask(block_shift)
    }

    /// Next-level table address (for table descriptors).
    pub fn table_addr(self, granule: Granule) -> u64 {
        self.0 & oa_mask(granule.page_shift())
    }

    // ── Permission / attribute fields ─────────────────────────────────────

    /// AP[2:1] (bits 7:6).
    pub fn ap(self) -> u32 { ((self.0 >> 6) & 3) as u32 }
    /// Access Flag (bit 10).
    pub fn af(self) -> bool { self.0 & (1 << 10) != 0 }
    /// Non-Global (bit 11).
    pub fn ng(self) -> bool { self.0 & (1 << 11) != 0 }
    /// PXN — Privileged Execute Never (bit 53).
    pub fn pxn(self) -> bool { self.0 & (1u64 << 53) != 0 }
    /// UXN/XN — User Execute Never (bit 54).
    pub fn uxn(self) -> bool { self.0 & (1u64 << 54) != 0 }
    /// AttrIndx[2:0] (bits 4:2) — index into MAIR_EL1.
    pub fn attr_indx(self) -> u32 { ((self.0 >> 2) & 7) as u32 }
    /// DBM — Dirty Bit Modifier (bit 51, ARMv8.1).
    pub fn dbm(self) -> bool { self.0 & (1u64 << 51) != 0 }
}

/// OA mask: bits [47:shift] for a given block/page shift.
fn oa_mask(shift: u32) -> u64 {
    0x0000_FFFF_FFFF_F000u64 & !((1u64 << shift) - 1)
}

/// Permissions extracted from a page table entry.
#[derive(Debug, Clone, Copy)]
pub struct Permissions {
    pub readable: bool,
    pub writable: bool,
    pub el1_executable: bool,
    pub el0_executable: bool,
}

impl Permissions {
    /// Extract permissions from a PTE for EL1 and EL0.
    pub fn from_pte(pte: Pte) -> Self {
        let ap = pte.ap();
        // AP[2:1]:
        //   00 = EL1 RW, EL0 none
        //   01 = EL1 RW, EL0 RW
        //   10 = EL1 RO, EL0 none
        //   11 = EL1 RO, EL0 RO
        let writable = ap & 2 == 0; // AP[2]=0 means writable
        Self {
            readable: true, // all valid entries are readable
            writable,
            el1_executable: !pte.pxn(),
            el0_executable: !pte.uxn(),
        }
    }

    /// Check if access is permitted for the given EL and access type.
    pub fn check(&self, el: u8, is_write: bool, is_fetch: bool) -> bool {
        if is_write && !self.writable {
            return false;
        }
        if is_fetch {
            if el == 0 && !self.el0_executable {
                return false;
            }
            if el >= 1 && !self.el1_executable {
                return false;
            }
        }
        true
    }
}

/// Result of a successful page table walk.
#[derive(Debug, Clone)]
pub struct WalkResult {
    /// Physical address.
    pub pa: u64,
    /// Permissions from the final descriptor.
    pub perms: Permissions,
    /// MAIR attribute index.
    pub attr_indx: u32,
    /// Translation level at which the walk completed.
    pub level: u8,
    /// Block/page size in bytes.
    pub block_size: u64,
    /// Non-global flag (ASID-tagged).
    pub ng: bool,
}

/// Translation fault type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TranslationFault {
    TranslationFault { level: u8 },
    AddressSizeFault { level: u8 },
    AccessFlagFault { level: u8 },
    PermissionFault { level: u8 },
}

impl TranslationFault {
    /// DFSC/IFSC encoding for the ESR_EL1.
    pub fn to_fsc(self) -> u32 {
        match self {
            TranslationFault::TranslationFault { level } => 0b000100 | (level as u32 & 3),
            TranslationFault::AddressSizeFault { level } => 0b000000 | (level as u32 & 3),
            TranslationFault::AccessFlagFault { level } => 0b001000 | (level as u32 & 3),
            TranslationFault::PermissionFault { level } => 0b001100 | (level as u32 & 3),
        }
    }

    pub fn level(self) -> u8 {
        match self {
            TranslationFault::TranslationFault { level }
            | TranslationFault::AddressSizeFault { level }
            | TranslationFault::AccessFlagFault { level }
            | TranslationFault::PermissionFault { level } => level,
        }
    }
}

/// Compute the starting level for a page table walk.
///
/// The starting level depends on the input address size (64 - TxSZ)
/// and the granule's bits per level.
fn start_level(ia_bits: u32, granule: Granule) -> u8 {
    let page_shift = granule.page_shift();
    let bpl = granule.bits_per_level();
    if ia_bits <= page_shift {
        return 3; // degenerate — only page offset, start at L3
    }
    let index_bits = ia_bits - page_shift;
    let levels = (index_bits + bpl - 1) / bpl;
    if levels > 4 {
        return 0; // clamp to L0
    }
    (4 - levels) as u8
}

/// Walk the AArch64 page tables for a given VA.
///
/// `read_phys_u64` reads an 8-byte value from physical memory.
/// Returns the walk result or a translation fault.
pub fn walk(
    va: u64,
    ttbr: u64,
    tsz: u32,
    granule: Granule,
    ha: bool,
    read_phys_u64: &mut dyn FnMut(u64) -> u64,
) -> Result<WalkResult, TranslationFault> {
    let ia_bits = 64u32.saturating_sub(tsz);
    let page_shift = granule.page_shift();
    let bpl = granule.bits_per_level();
    let sl = start_level(ia_bits, granule);

    // Table base from TTBR (bits [47:page_shift], ASID in upper bits ignored)
    let mut table_base = ttbr & oa_mask(page_shift);

    for level in sl..=3u8 {
        // Compute the VA bits that index into this level's table
        let shift = page_shift + (3 - level) as u32 * bpl;
        let index_mask = (1u64 << bpl) - 1;
        let index = (va >> shift) & index_mask;

        // Read the descriptor
        let desc_addr = table_base + index * 8;
        let raw = read_phys_u64(desc_addr);
        let pte = Pte(raw);

        if !pte.is_valid() {
            return Err(TranslationFault::TranslationFault { level });
        }

        if level < 3 && pte.is_table() {
            // Table descriptor → descend to next level
            table_base = pte.table_addr(granule);
            continue;
        }

        // Block (L0-L2) or Page (L3) descriptor
        let is_block_allowed = level < 3;
        let is_page_at_l3 = level == 3 && pte.is_table(); // At L3, bits[1:0]=11 means page

        if (is_block_allowed && pte.is_block()) || is_page_at_l3 {
            // Check Access Flag
            if !pte.af() && !ha {
                return Err(TranslationFault::AccessFlagFault { level });
            }

            let block_shift = shift;
            let block_size = 1u64 << block_shift;
            let offset_mask = block_size - 1;
            let pa = pte.oa(block_shift) | (va & offset_mask);
            let perms = Permissions::from_pte(pte);

            if va > 0xFFFF_0000_0000_0000 {
                log::trace!(
                    "walk VA={va:#x} → PA={pa:#x} L{level} pte={:#018x} block={block_size:#x}",
                    pte.0,
                );
            }
            return Ok(WalkResult {
                pa,
                perms,
                attr_indx: pte.attr_indx(),
                level,
                block_size,
                ng: pte.ng(),
            });
        }

        // Invalid combination (e.g. block at L0 for 4K — not architecturally valid)
        return Err(TranslationFault::TranslationFault { level });
    }

    // Should not reach here
    Err(TranslationFault::TranslationFault { level: 3 })
}

/// Full translation: select TTBR, then walk.
pub fn translate(
    va: u64,
    tcr: &TranslationConfig,
    ttbr0: u64,
    ttbr1: u64,
    read_phys_u64: &mut dyn FnMut(u64) -> u64,
) -> Result<(WalkResult, TtbrSelect), TranslationFault> {
    let sel = select_ttbr(va, tcr);
    match sel {
        TtbrSelect::Ttbr0 => {
            if tcr.epd0 {
                return Err(TranslationFault::TranslationFault { level: 0 });
            }
            let result = walk(va, ttbr0, tcr.t0sz, tcr.tg0, tcr.ha, read_phys_u64)?;
            Ok((result, sel))
        }
        TtbrSelect::Ttbr1 => {
            if tcr.epd1 {
                return Err(TranslationFault::TranslationFault { level: 0 });
            }
            let result = walk(va, ttbr1, tcr.t1sz, tcr.tg1, tcr.ha, read_phys_u64)?;
            Ok((result, sel))
        }
        TtbrSelect::Fault => {
            Err(TranslationFault::TranslationFault { level: 0 })
        }
    }
}

// ── Stage-2 translation (IPA → PA) ───────────────────────────────────────

/// Parsed VTCR_EL2 fields for stage-2 translation.
#[derive(Debug, Clone)]
pub struct Stage2Config {
    /// IPA size = 64 - t0sz (bits [5:0]).
    pub t0sz: u32,
    /// Starting level of walk (bits [7:6]).
    pub sl0: u32,
    /// Granule for stage-2 tables (bits [15:14]).
    pub tg0: Granule,
    /// Physical address size (bits [18:16]).
    pub ps: u32,
    /// Hardware Access flag (bit 21).
    pub ha: bool,
    /// Hardware Dirty bit (bit 22).
    pub hd: bool,
}

impl Stage2Config {
    /// Parse from raw VTCR_EL2 value.
    pub fn parse(vtcr: u64) -> Self {
        let tg0 = match (vtcr >> 14) & 3 {
            0 => Granule::K4,
            1 => Granule::K64,
            2 => Granule::K16,
            _ => Granule::K4,
        };
        Self {
            t0sz: (vtcr & 0x3F) as u32,
            sl0: ((vtcr >> 6) & 3) as u32,
            tg0,
            ps: ((vtcr >> 16) & 7) as u32,
            ha: (vtcr >> 21) & 1 != 0,
            hd: (vtcr >> 22) & 1 != 0,
        }
    }

    /// Compute the starting level from SL0 and granule.
    ///
    /// For 4K granule: SL0=0→L2, SL0=1→L1, SL0=2→L0
    /// For 16K granule: SL0=0→L3, SL0=1→L2, SL0=2→L1, SL0=3→L0
    /// For 64K granule: SL0=0→L3, SL0=1→L2, SL0=2→L1
    fn start_level(&self) -> u8 {
        match self.tg0 {
            Granule::K4 => match self.sl0 {
                0 => 2,
                1 => 1,
                2 => 0,
                _ => 2,
            },
            Granule::K16 => match self.sl0 {
                0 => 3,
                1 => 2,
                2 => 1,
                3 => 0,
                _ => 3,
            },
            Granule::K64 => match self.sl0 {
                0 => 3,
                1 => 2,
                2 => 1,
                _ => 3,
            },
        }
    }
}

/// Stage-2 permissions from S2AP field.
///
/// Stage-2 descriptors use S2AP[1:0] (bits [7:6]):
///   00 = no access
///   01 = read-only
///   10 = write-only
///   11 = read-write
///
/// XN[1:0] (bits [54:53]):
///   0x = execute permitted for EL1
///   x0 = execute permitted for EL0
fn s2_permissions(pte: Pte) -> Permissions {
    let s2ap = pte.ap();
    let xn1 = (pte.0 >> 54) & 1 != 0; // XN for EL1
    let xn0 = (pte.0 >> 53) & 1 != 0; // XN for EL0
    Permissions {
        readable: s2ap & 1 != 0,
        writable: s2ap & 2 != 0,
        el1_executable: !xn1,
        el0_executable: !xn0,
    }
}

/// Walk stage-2 page tables (IPA → PA).
///
/// Uses VTTBR_EL2 as the table base with VTCR_EL2 configuration.
pub fn walk_stage2(
    ipa: u64,
    vttbr: u64,
    vtcr: &Stage2Config,
    read_phys_u64: &mut dyn FnMut(u64) -> u64,
) -> Result<WalkResult, TranslationFault> {
    let page_shift = vtcr.tg0.page_shift();
    let bpl = vtcr.tg0.bits_per_level();
    let sl = vtcr.start_level();

    // Table base from VTTBR (VMID in upper bits ignored)
    let mut table_base = vttbr & oa_mask(page_shift);

    for level in sl..=3u8 {
        let shift = page_shift + (3 - level) as u32 * bpl;
        let index_mask = (1u64 << bpl) - 1;
        let index = (ipa >> shift) & index_mask;

        let desc_addr = table_base + index * 8;
        let raw = read_phys_u64(desc_addr);
        let pte = Pte(raw);

        if !pte.is_valid() {
            return Err(TranslationFault::TranslationFault { level });
        }

        if level < 3 && pte.is_table() {
            table_base = pte.table_addr(vtcr.tg0);
            continue;
        }

        let is_block_allowed = level < 3;
        let is_page_at_l3 = level == 3 && pte.is_table();

        if (is_block_allowed && pte.is_block()) || is_page_at_l3 {
            if !pte.af() && !vtcr.ha {
                return Err(TranslationFault::AccessFlagFault { level });
            }

            let block_shift = shift;
            let block_size = 1u64 << block_shift;
            let offset_mask = block_size - 1;
            let pa = pte.oa(block_shift) | (ipa & offset_mask);
            let perms = s2_permissions(pte);

            return Ok(WalkResult {
                pa,
                perms,
                attr_indx: pte.attr_indx(),
                level,
                block_size,
                ng: false, // stage-2 entries are not nG-tagged
            });
        }

        return Err(TranslationFault::TranslationFault { level });
    }

    Err(TranslationFault::TranslationFault { level: 3 })
}
