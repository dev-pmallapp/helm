//! AArch64 MMU page table walker (4KB granule, EL0/1/2/3 stage-1).
//!
//! Implements a 4-level page table walk for VA->PA translation.
//! Table walks use physical addresses (bypass the MMU) via `mem.read()`.
//!
//! # TLB
//! `Tlb` is a direct-mapped 256-entry software TLB indexed by VA bits [19:12].
//! Each entry stores the VA page base, PA page base, and a TTBR tag to detect
//! stale entries after context switches (TTBR write → `tlb.flush()`).

use helm_core::{AccessType, MemInterface};

use super::arch_state::Aarch64ArchState;

// ---------------------------------------------------------------------------
// TLB
// ---------------------------------------------------------------------------

const TLB_ENTRIES: usize = 1024;
const TLB_INDEX_MASK: u64 = (TLB_ENTRIES as u64) - 1;
const TTBR_ASID_SHIFT: u64 = 48;
const TTBR_ASID_MASK: u64 = 0xFFFFu64 << TTBR_ASID_SHIFT;
const TTBR_BASE_MASK: u64 = 0x0000_FFFF_FFFF_F000;
const HCR_VM: u64 = 1u64 << 0;
const HCR_TGE: u64 = 1u64 << 27;

/// Direct-mapped software TLB — 1024 entries, indexed by VA bits [21:12].
///
/// Tag = TTBR (upper or lower) used at translation time.  A tag mismatch
/// (e.g. after a context switch that writes TTBR0/1) acts as an implicit
/// flush for that entry.  Call `flush()` explicitly on SCTLR/TCR writes.
pub struct Tlb {
    entries: Box<[TlbEntry; TLB_ENTRIES]>,
    /// TTBR tag per entry — invalidates stale entries on context switch.
    tags: Box<[u64; TLB_ENTRIES]>,
}

#[derive(Clone, Copy)]
struct TlbEntry {
    va_page: u64,
    pa_page: u64,
    attr_idx: u8,
    ap: u8,
    pxn: bool,
    uxn: bool,
}

impl Default for TlbEntry {
    fn default() -> Self {
        Self {
            va_page: 0,
            pa_page: 0,
            attr_idx: 0,
            ap: 0,
            pxn: false,
            uxn: false,
        }
    }
}

impl Tlb {
    pub fn new() -> Self {
        Self {
            entries: Box::new([TlbEntry::default(); TLB_ENTRIES]),
            tags: Box::new([u64::MAX; TLB_ENTRIES]),
        }
    }

    #[inline]
    fn idx(va: u64) -> usize {
        ((va >> 12) & TLB_INDEX_MASK) as usize
    }

    /// Look up a VA. Returns the PA if cached and tag matches.
    #[inline]
    pub fn lookup(&self, va: u64, tag: u64) -> Option<TranslateResult> {
        let i = Self::idx(va);
        let va_page = va & !0xFFF;
        let entry = self.entries[i];
        if entry.va_page == va_page && self.tags[i] == tag {
            Some(TranslateResult {
                pa: entry.pa_page | (va & 0xFFF),
                attr_idx: entry.attr_idx,
                ap: entry.ap,
                pxn: entry.pxn,
                uxn: entry.uxn,
            })
        } else {
            None
        }
    }

    /// Insert a (VA page, PA page) pair with the given normalized TLB tag.
    #[inline]
    pub fn insert(&mut self, va: u64, result: TranslateResult, tag: u64) {
        let i = Self::idx(va);
        self.entries[i] = TlbEntry {
            va_page: va & !0xFFF,
            pa_page: result.pa & !0xFFF,
            attr_idx: result.attr_idx,
            ap: result.ap,
            pxn: result.pxn,
            uxn: result.uxn,
        };
        self.tags[i] = tag;
    }

    /// Invalidate all TLB entries.
    #[inline]
    pub fn flush(&mut self) {
        for tag in self.tags.iter_mut() {
            *tag = u64::MAX;
        }
    }

    /// Invalidate the TLB entry matching a specific VA (page-aligned).
    ///
    /// Used by TLBI VAE1/VALE1 instructions for per-address invalidation
    /// instead of a full flush.
    #[inline]
    pub fn invalidate_va(&mut self, va: u64) {
        let i = Self::idx(va);
        let va_page = va & !0xFFF;
        if self.entries[i].va_page == va_page {
            self.tags[i] = u64::MAX;
        }
    }

    /// Invalidate entries matching a specific EL1 ASID.
    #[inline]
    pub fn flush_asid(&mut self, asid: u16) {
        let asid_bits = (asid as u64) << TTBR_ASID_SHIFT;
        for tag in self.tags.iter_mut() {
            if *tag != u64::MAX && (*tag & TTBR_ASID_MASK) == asid_bits {
                *tag = u64::MAX;
            }
        }
    }
}

impl Default for Tlb {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// MmuConfig (used by translate_cfg to avoid dummy ArchState allocation)
// ---------------------------------------------------------------------------

/// Minimal snapshot of MMU configuration registers needed for VA→PA translation.
/// Used by `translate_cfg()` to avoid passing a full `Aarch64ArchState`.
#[derive(Clone, Copy)]
pub struct MmuConfig {
    pub sctlr_el1: u64,
    pub sctlr_el2: u64,
    pub sctlr_el3: u64,
    pub tcr_el1: u64,
    pub tcr_el2: u64,
    pub tcr_el3: u64,
    pub ttbr0_el1: u64,
    pub ttbr0_el2: u64,
    pub ttbr0_el3: u64,
    pub ttbr1_el1: u64,
    pub ttbr1_el2: u64,
    pub vttbr_el2: u64,
    pub vtcr_el2: u64,
    pub current_el: u8,
    pub hcr_el2: u64,
}

impl MmuConfig {
    pub fn from_arch(a: &Aarch64ArchState) -> Self {
        Self {
            sctlr_el1: a.sctlr_el1,
            sctlr_el2: a.sctlr_el2,
            sctlr_el3: a.sctlr_el3,
            tcr_el1: a.tcr_el1,
            tcr_el2: a.tcr_el2,
            tcr_el3: a.tcr_el3,
            ttbr0_el1: a.ttbr0_el1,
            ttbr0_el2: a.ttbr0_el2,
            ttbr0_el3: a.ttbr0_el3,
            ttbr1_el1: a.ttbr1_el1,
            ttbr1_el2: a.ttbr1_el2,
            vttbr_el2: a.vttbr_el2,
            vtcr_el2: a.vtcr_el2,
            current_el: a.current_el,
            hcr_el2: a.hcr_el2,
        }
    }

    #[inline]
    pub fn mmu_enabled(&self) -> bool {
        match self.current_el {
            0 | 1 => (self.sctlr_el1 & 1 != 0) || (self.hcr_el2 & HCR_VM != 0),
            2 => self.sctlr_el2 & 1 != 0,
            3 => self.sctlr_el3 & 1 != 0,
            _ => false,
        }
    }
}

#[inline]
fn el1_asid_mask(tcr_el1: u64) -> u16 {
    if (tcr_el1 >> 36) & 1 != 0 {
        0xFFFF
    } else {
        0x00FF
    }
}

#[inline]
fn el1_effective_asid(tcr_el1: u64, ttbr0_el1: u64, ttbr1_el1: u64) -> u16 {
    let a1 = (tcr_el1 >> 22) & 1 != 0;
    let ttbr = if a1 { ttbr1_el1 } else { ttbr0_el1 };
    ((ttbr >> TTBR_ASID_SHIFT) as u16) & el1_asid_mask(tcr_el1)
}

#[inline]
fn tlb_tag(ttbr: u64, current_el: u8, tcr_el1: u64, ttbr0_el1: u64, ttbr1_el1: u64) -> u64 {
    let table_base = ttbr & TTBR_BASE_MASK;
    if matches!(current_el, 0 | 1) {
        table_base | ((el1_effective_asid(tcr_el1, ttbr0_el1, ttbr1_el1) as u64) << TTBR_ASID_SHIFT)
    } else {
        table_base
    }
}

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Access type for MMU permission checks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MmuAccess {
    Read,
    Write,
    Execute,
}

/// Result of a successful address translation.
#[derive(Debug, Clone, Copy)]
pub struct TranslateResult {
    /// Physical address.
    pub pa: u64,
    /// Memory attributes index (from descriptor AttrIndx[2:0]).
    pub attr_idx: u8,
    /// Access permission bits AP[2:1].
    pub ap: u8,
    /// Privileged execute-never.
    pub pxn: bool,
    /// User execute-never.
    pub uxn: bool,
}

/// MMU translation fault.
#[derive(Debug, Clone, Copy)]
pub struct MmuFault {
    /// Faulting virtual address.
    pub va: u64,
    /// Fault address to report via FAR_ELx when taking the exception.
    pub far: u64,
    /// Page table walk level where the fault occurred (0-3).
    pub level: u8,
    /// Fault type.
    pub kind: FaultKind,
    /// Override target EL for faults that must be delivered to the hypervisor.
    pub target_el: Option<u8>,
    /// Intermediate physical address to report via HPFAR_EL2 when present.
    pub ipa: Option<u64>,
}

/// Classification of MMU faults.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FaultKind {
    /// Translation fault -- invalid descriptor at this level.
    Translation,
    /// Access flag fault -- AF bit not set.
    AccessFlag,
    /// Permission fault -- AP bits deny the access.
    Permission,
    /// Address size fault -- address outside supported range.
    AddressSize,
}

#[inline]
fn stage1_fault(va: u64, level: u8, kind: FaultKind) -> MmuFault {
    MmuFault {
        va,
        far: va,
        level,
        kind,
        target_el: None,
        ipa: None,
    }
}

#[inline]
fn stage2_fault(va: u64, ipa: u64, level: u8, kind: FaultKind) -> MmuFault {
    MmuFault {
        va,
        far: ipa,
        level,
        kind,
        target_el: Some(2),
        ipa: Some(ipa),
    }
}

// ---------------------------------------------------------------------------
// Syndrome encoding
// ---------------------------------------------------------------------------

impl MmuFault {
    /// Encode as ISS for ESR_EL1 Data Abort syndrome.
    ///
    /// ISS[5:0] = DFSC (Data Fault Status Code) encoding:
    /// - Translation fault = `0b0001xx` where `xx` = level
    /// - Access flag fault = `0b0010xx`
    /// - Permission fault  = `0b0011xx`
    /// - Address size fault = `0b0000xx`
    ///
    /// ISS[6] = WnR (1 for write, 0 for read).
    pub fn iss_data(&self, is_write: bool) -> u32 {
        let dfsc = self.fault_status_code();
        let wnr = if is_write { 1 << 6 } else { 0 };
        dfsc | wnr
    }

    /// Encode as ISS for ESR_EL1 Instruction Abort syndrome.
    ///
    /// ISS[5:0] = IFSC (same encoding as DFSC).
    pub fn iss_insn(&self) -> u32 {
        self.fault_status_code()
    }

    /// Common DFSC/IFSC encoding: fault-class bits [5:2] | level [1:0].
    /// Public for AT instruction (PAR_EL1.FST field).
    pub fn fault_status_code_pub(&self) -> u32 {
        self.fault_status_code()
    }

    /// Common DFSC/IFSC encoding: fault-class bits [5:2] | level [1:0].
    fn fault_status_code(&self) -> u32 {
        let class = match self.kind {
            FaultKind::AddressSize => 0b000000,
            FaultKind::Translation => 0b000100,
            FaultKind::AccessFlag => 0b001000,
            FaultKind::Permission => 0b001100,
        };
        class | (self.level as u32 & 3)
    }
}

// ---------------------------------------------------------------------------
// Top-level translate entry point
// ---------------------------------------------------------------------------

/// Translate a virtual address to a physical address.
///
/// If SCTLR_EL1.M = 0 (MMU disabled), returns identity mapping (PA = VA).
/// Otherwise performs a 4-level page table walk using the appropriate TTBR.
/// On a successful walk the result is inserted into `tlb` (if provided).
///
/// `mem` is used for physical memory reads (table walk accesses).
pub fn translate(
    a: &Aarch64ArchState,
    va: u64,
    access: MmuAccess,
    mem: &mut impl MemInterface,
    tlb: Option<&mut Tlb>,
) -> Result<TranslateResult, MmuFault> {
    translate_inner(
        a.sctlr_el1,
        a.sctlr_el2,
        a.sctlr_el3,
        a.tcr_el1,
        a.tcr_el2,
        a.tcr_el3,
        a.ttbr0_el1,
        a.ttbr0_el2,
        a.ttbr0_el3,
        a.ttbr1_el1,
        a.ttbr1_el2,
        a.vttbr_el2,
        a.vtcr_el2,
        a.current_el,
        a.hcr_el2,
        va,
        access,
        mem,
        tlb,
    )
}

/// Like `translate()` but takes a `MmuConfig` snapshot instead of a full
/// `Aarch64ArchState`.  Returns just the PA (sufficient for FS-mode dispatch).
/// Avoids allocating a dummy `Aarch64ArchState` in `TranslatingMem`.
/// Like `translate()` but takes a `MmuConfig` snapshot instead of a full
/// `Aarch64ArchState`.  Returns `(PA, MmuFault)` so callers can propagate
/// the exact fault level and kind into ESR ISS fields.
pub fn translate_cfg(
    cfg: &MmuConfig,
    va: u64,
    access: MmuAccess,
    mem: &mut impl MemInterface,
    tlb: Option<&mut Tlb>,
) -> Result<u64, MmuFault> {
    translate_inner(
        cfg.sctlr_el1,
        cfg.sctlr_el2,
        cfg.sctlr_el3,
        cfg.tcr_el1,
        cfg.tcr_el2,
        cfg.tcr_el3,
        cfg.ttbr0_el1,
        cfg.ttbr0_el2,
        cfg.ttbr0_el3,
        cfg.ttbr1_el1,
        cfg.ttbr1_el2,
        cfg.vttbr_el2,
        cfg.vtcr_el2,
        cfg.current_el,
        cfg.hcr_el2,
        va,
        access,
        mem,
        tlb,
    )
    .map(|r| r.pa)
}

/// Shared core of `translate()` and `translate_cfg()`.
#[inline]
fn translate_inner(
    sctlr_el1: u64,
    sctlr_el2: u64,
    sctlr_el3: u64,
    tcr_el1: u64,
    tcr_el2: u64,
    tcr_el3: u64,
    ttbr0_el1: u64,
    ttbr0_el2: u64,
    ttbr0_el3: u64,
    ttbr1_el1: u64,
    ttbr1_el2: u64,
    vttbr_el2: u64,
    vtcr_el2: u64,
    current_el: u8,
    hcr_el2: u64,
    va: u64,
    access: MmuAccess,
    mem: &mut impl MemInterface,
    tlb: Option<&mut Tlb>,
) -> Result<TranslateResult, MmuFault> {
    if matches!(current_el, 0 | 1) && (hcr_el2 & HCR_VM) != 0 && (hcr_el2 & HCR_TGE) == 0 {
        let ipa = if sctlr_el1 & 1 == 0 {
            va
        } else {
            translate_stage1_el1_no_tlb(
                va,
                tcr_el1,
                ttbr0_el1,
                ttbr1_el1,
                current_el,
                access,
                mem,
            )?
            .pa
        };
        return walk_stage2_and_check(va, ipa, vtcr_el2, vttbr_el2, current_el, access, mem);
    }

    // Fast path: MMU disabled => identity translation.
    let regime = active_regime(
        current_el, sctlr_el1, sctlr_el2, sctlr_el3, tcr_el1, tcr_el2, tcr_el3, ttbr0_el1,
        ttbr0_el2, ttbr0_el3, ttbr1_el1, ttbr1_el2, hcr_el2,
    );

    if !regime.mmu_enabled {
        return Ok(TranslateResult {
            pa: va,
            attr_idx: 0,
            ap: 0,
            pxn: false,
            uxn: false,
        });
    }

    let (ttbr, tsz, ha) = if regime.split {
        select_ttbr(va, regime.tcr, regime.ttbr0, regime.ttbr1)?
    } else {
        select_ttbr_single(va, regime.tcr, regime.ttbr0, regime.ha)?
    };

    let tag = tlb_tag(ttbr, current_el, tcr_el1, ttbr0_el1, ttbr1_el1);

    // TLB fast path.
    if let Some(ref tlb_ref) = tlb {
        if let Some(result) = tlb_ref.lookup(va, tag) {
            check_permissions(va, 3, result.ap, result.pxn, result.uxn, access, current_el)?;
            return Ok(result);
        }
    }

    // Table base address from TTBR: bits [47:12] for 4KB granule.
    let table_base = ttbr & 0x0000_FFFF_FFFF_F000;

    // Starting level: 4KB granule, each level resolves 9 VA bits.
    let va_bits = 64 - tsz;
    let start_level = if va_bits > 39 {
        0u8
    } else if va_bits > 30 {
        1
    } else if va_bits > 21 {
        2
    } else {
        3
    };

    let result = walk(va, table_base, start_level, access, mem, current_el, ha)?;

    // TLB fill on success.
    if let Some(tlb_mut) = tlb {
        tlb_mut.insert(va, result, tag);
    }

    Ok(result)
}

fn translate_stage1_el1_no_tlb(
    va: u64,
    tcr_el1: u64,
    ttbr0_el1: u64,
    ttbr1_el1: u64,
    current_el: u8,
    access: MmuAccess,
    mem: &mut impl MemInterface,
) -> Result<TranslateResult, MmuFault> {
    let (ttbr, tsz, ha) = select_ttbr(va, tcr_el1, ttbr0_el1, ttbr1_el1)?;
    let table_base = ttbr & 0x0000_FFFF_FFFF_F000;
    let va_bits = 64 - tsz;
    let start_level = if va_bits > 39 {
        0u8
    } else if va_bits > 30 {
        1
    } else if va_bits > 21 {
        2
    } else {
        3
    };
    walk(va, table_base, start_level, access, mem, current_el, ha)
}

fn walk_stage2_and_check(
    va: u64,
    ipa: u64,
    vtcr_el2: u64,
    vttbr_el2: u64,
    current_el: u8,
    access: MmuAccess,
    mem: &mut impl MemInterface,
) -> Result<TranslateResult, MmuFault> {
    let cfg = Stage2Config::parse(vtcr_el2);
    let walk = walk_stage2(ipa, vttbr_el2, &cfg, mem).map_err(|fault| match fault.kind {
        FaultKind::Translation => stage2_fault(va, ipa, fault.level, FaultKind::Translation),
        FaultKind::AccessFlag => stage2_fault(va, ipa, fault.level, FaultKind::AccessFlag),
        FaultKind::Permission => stage2_fault(va, ipa, fault.level, FaultKind::Permission),
        FaultKind::AddressSize => stage2_fault(va, ipa, fault.level, FaultKind::AddressSize),
    })?;

    if !walk.perms.check(current_el, access == MmuAccess::Write, access == MmuAccess::Execute) {
        return Err(stage2_fault(va, ipa, walk.level, FaultKind::Permission));
    }

    Ok(TranslateResult {
        pa: walk.pa,
        attr_idx: walk.attr_idx,
        ap: 0,
        pxn: false,
        uxn: false,
    })
}

fn select_ttbr(
    va: u64,
    tcr_el1: u64,
    ttbr0_el1: u64,
    ttbr1_el1: u64,
) -> Result<(u64, u32, bool), MmuFault> {
    let t0sz = (tcr_el1 & 0x3F) as u32;
    let t1sz = ((tcr_el1 >> 16) & 0x3F) as u32;
    let epd0 = (tcr_el1 >> 7) & 1 != 0;
    let epd1 = (tcr_el1 >> 23) & 1 != 0;
    let ha = (tcr_el1 >> 39) & 1 != 0;

    if va_in_lower_range(va, t0sz) {
        if epd0 {
            return Err(stage1_fault(va, 0, FaultKind::Translation));
        }
        return Ok((ttbr0_el1, t0sz, ha));
    }

    if va_in_upper_range(va, t1sz) {
        if epd1 {
            return Err(stage1_fault(va, 0, FaultKind::Translation));
        }
        return Ok((ttbr1_el1, t1sz, ha));
    }

    Err(stage1_fault(va, 0, FaultKind::AddressSize))
}

fn select_ttbr_single(
    va: u64,
    tcr: u64,
    ttbr0: u64,
    ha: bool,
) -> Result<(u64, u32, bool), MmuFault> {
    let t0sz = (tcr & 0x3F) as u32;
    if va_in_lower_range(va, t0sz) {
        Ok((ttbr0, t0sz, ha))
    } else {
        Err(stage1_fault(va, 0, FaultKind::AddressSize))
    }
}

const HCR_E2H: u64 = 1u64 << 34;

struct ActiveRegime {
    mmu_enabled: bool,
    split: bool,
    tcr: u64,
    ttbr0: u64,
    ttbr1: u64,
    ha: bool,
}

fn active_regime(
    current_el: u8,
    sctlr_el1: u64,
    sctlr_el2: u64,
    sctlr_el3: u64,
    tcr_el1: u64,
    tcr_el2: u64,
    tcr_el3: u64,
    ttbr0_el1: u64,
    ttbr0_el2: u64,
    ttbr0_el3: u64,
    ttbr1_el1: u64,
    ttbr1_el2: u64,
    hcr_el2: u64,
) -> ActiveRegime {
    match current_el {
        2 => {
            let split = (hcr_el2 & HCR_E2H) != 0;
            ActiveRegime {
                mmu_enabled: sctlr_el2 & 1 != 0,
                split,
                tcr: tcr_el2,
                ttbr0: ttbr0_el2,
                ttbr1: if split { ttbr1_el2 } else { 0 },
                ha: if split {
                    (tcr_el2 >> 39) & 1 != 0
                } else {
                    (tcr_el2 >> 21) & 1 != 0
                },
            }
        }
        3 => ActiveRegime {
            mmu_enabled: sctlr_el3 & 1 != 0,
            split: false,
            tcr: tcr_el3,
            ttbr0: ttbr0_el3,
            ttbr1: 0,
            ha: (tcr_el3 >> 21) & 1 != 0,
        },
        _ => ActiveRegime {
            mmu_enabled: sctlr_el1 & 1 != 0,
            split: true,
            tcr: tcr_el1,
            ttbr0: ttbr0_el1,
            ttbr1: ttbr1_el1,
            ha: (tcr_el1 >> 39) & 1 != 0,
        },
    }
}

#[inline]
fn va_in_lower_range(va: u64, tsz: u32) -> bool {
    let ia_bits = 64u32.saturating_sub(tsz);
    if ia_bits >= 64 {
        return true;
    }
    if ia_bits == 0 {
        return false;
    }
    let top_mask = !((1u64 << ia_bits) - 1);
    va & top_mask == 0
}

#[inline]
fn va_in_upper_range(va: u64, tsz: u32) -> bool {
    let ia_bits = 64u32.saturating_sub(tsz);
    if ia_bits == 0 {
        return false;
    }
    if ia_bits >= 64 {
        return va == u64::MAX;
    }
    let top_mask = !((1u64 << ia_bits) - 1);
    va & top_mask == top_mask
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Granule {
    K4,
    K16,
    K64,
}

impl Granule {
    fn page_shift(self) -> u32 {
        match self {
            Self::K4 => 12,
            Self::K16 => 14,
            Self::K64 => 16,
        }
    }

    fn bits_per_level(self) -> u32 {
        match self {
            Self::K4 => 9,
            Self::K16 => 11,
            Self::K64 => 13,
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct Stage2Config {
    t0sz: u32,
    sl0: u32,
    tg0: Granule,
    ha: bool,
}

impl Stage2Config {
    fn parse(vtcr: u64) -> Self {
        let tg0 = match (vtcr >> 14) & 0x3 {
            0 => Granule::K4,
            1 => Granule::K64,
            2 => Granule::K16,
            _ => Granule::K4,
        };
        Self {
            t0sz: (vtcr & 0x3F) as u32,
            sl0: ((vtcr >> 6) & 0x3) as u32,
            tg0,
            ha: (vtcr >> 21) & 1 != 0,
        }
    }

    fn start_level(self) -> u8 {
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

#[derive(Debug, Clone, Copy)]
struct Stage2Permissions {
    readable: bool,
    writable: bool,
    el1_executable: bool,
    el0_executable: bool,
}

impl Stage2Permissions {
    fn check(self, el: u8, is_write: bool, is_fetch: bool) -> bool {
        if is_write && !self.writable {
            return false;
        }
        if !is_write && !is_fetch && !self.readable {
            return false;
        }
        if is_fetch {
            if el == 0 {
                return self.el0_executable;
            }
            return self.el1_executable;
        }
        true
    }
}

#[derive(Debug, Clone, Copy)]
struct Stage2WalkResult {
    pa: u64,
    perms: Stage2Permissions,
    attr_idx: u8,
    level: u8,
}

fn stage2_oa_mask(shift: u32) -> u64 {
    0x0000_FFFF_FFFF_F000u64 & !((1u64 << shift) - 1)
}

fn stage2_table_addr(desc: u64, granule: Granule) -> u64 {
    desc & stage2_oa_mask(granule.page_shift())
}

fn stage2_permissions(desc: u64) -> Stage2Permissions {
    let s2ap = ((desc >> 6) & 0x3) as u8;
    let xn1 = desc & (1u64 << 54) != 0;
    let xn0 = desc & (1u64 << 53) != 0;
    Stage2Permissions {
        readable: s2ap & 0x1 != 0,
        writable: s2ap & 0x2 != 0,
        el1_executable: !xn1,
        el0_executable: !xn0,
    }
}

fn walk_stage2(
    ipa: u64,
    vttbr_el2: u64,
    cfg: &Stage2Config,
    mem: &mut impl MemInterface,
) -> Result<Stage2WalkResult, MmuFault> {
    let page_shift = cfg.tg0.page_shift();
    let bits_per_level = cfg.tg0.bits_per_level();
    let start_level = cfg.start_level();
    let mut table_base = vttbr_el2 & stage2_oa_mask(page_shift);
    let ipa_bits = 64u32.saturating_sub(cfg.t0sz);
    if ipa_bits < page_shift {
        return Err(stage1_fault(ipa, 0, FaultKind::AddressSize));
    }

    for level in start_level..=3u8 {
        let shift = page_shift + (3 - level) as u32 * bits_per_level;
        let index_mask = (1u64 << bits_per_level) - 1;
        let index = (ipa >> shift) & index_mask;
        let desc_addr = table_base + index * 8;
        let desc = mem
            .read(desc_addr, 8, AccessType::Load)
            .map_err(|_| stage1_fault(ipa, level, FaultKind::Translation))?;

        if desc & 1 == 0 {
            return Err(stage1_fault(ipa, level, FaultKind::Translation));
        }

        if level < 3 && desc & 0x3 == 0x3 {
            table_base = stage2_table_addr(desc, cfg.tg0);
            continue;
        }

        let is_block = level < 3 && desc & 0x2 == 0;
        let is_page = level == 3 && desc & 0x3 == 0x3;
        if !is_block && !is_page {
            return Err(stage1_fault(ipa, level, FaultKind::Translation));
        }

        if desc & (1u64 << 10) == 0 && !cfg.ha {
            return Err(stage1_fault(ipa, level, FaultKind::AccessFlag));
        }

        let block_shift = shift;
        let block_mask = (1u64 << block_shift) - 1;
        let pa = (desc & stage2_oa_mask(block_shift)) | (ipa & block_mask);
        return Ok(Stage2WalkResult {
            pa,
            perms: stage2_permissions(desc),
            attr_idx: ((desc >> 2) & 0x7) as u8,
            level,
        });
    }

    Err(stage1_fault(ipa, 3, FaultKind::Translation))
}

// ---------------------------------------------------------------------------
// Internal page table walk
// ---------------------------------------------------------------------------

/// Perform the page table walk from `start_level` to level 3.
///
/// Each level indexes into a 512-entry table (9 bits of VA) using 8-byte
/// descriptors read from physical memory via `mem.read()`.
fn walk(
    va: u64,
    table_base: u64,
    start_level: u8,
    access: MmuAccess,
    mem: &mut impl MemInterface,
    current_el: u8,
    ha: bool,
) -> Result<TranslateResult, MmuFault> {
    let mut table_addr = table_base;

    for level in start_level..=3 {
        // VA index bits for this level:
        //   Level 0: VA[47:39]  (shift = 39)
        //   Level 1: VA[38:30]  (shift = 30)
        //   Level 2: VA[29:21]  (shift = 21)
        //   Level 3: VA[20:12]  (shift = 12)
        let shift = ((3 - level) as u64) * 9 + 12;
        let index = (va >> shift) & 0x1FF;

        // Read the 8-byte descriptor at physical address.
        let desc_addr = table_addr + index * 8;
        let desc = mem
            .read(desc_addr, 8, AccessType::Load)
            .map_err(|_| stage1_fault(va, level, FaultKind::Translation))?;

        // Bit 0 = valid.
        if desc & 1 == 0 {
            return Err(stage1_fault(va, level, FaultKind::Translation));
        }

        if level == 3 {
            // Level 3: must be a page descriptor (bits[1:0] = 0b11).
            if desc & 0x3 != 0x3 {
                return Err(stage1_fault(va, level, FaultKind::Translation));
            }
            return finish_page(va, desc, desc_addr, level, access, mem, current_el, ha, 12);
        }

        // Levels 0-2: bit 1 distinguishes table (1) vs block (0).
        if desc & 0x2 == 0 {
            // Block descriptor (valid at level 1 and level 2 only).
            return match level {
                1 => finish_page(va, desc, desc_addr, level, access, mem, current_el, ha, 30),
                2 => finish_page(va, desc, desc_addr, level, access, mem, current_el, ha, 21),
                _ => Err(stage1_fault(va, level, FaultKind::Translation)),
            };
        }

        // Table descriptor: next-level table address = desc[47:12].
        table_addr = desc & 0x0000_FFFF_FFFF_F000;
    }

    // Should never reach here -- the loop covers levels start..=3.
    Err(stage1_fault(va, 3, FaultKind::Translation))
}

/// Extract the physical address and attributes from a page/block descriptor,
/// check permissions, and return the `TranslateResult`.
///
/// `page_shift` is 12 for 4KB pages, 21 for 2MB blocks, 30 for 1GB blocks.
///
/// Hardware Access Flag (HA): when `ha=true` and `AF=0` in the descriptor,
/// the hardware sets `AF=1` by writing the updated descriptor back to memory
/// (atomically on real hardware; plain write here since we're single-threaded
/// per-vCPU). This avoids software AF fault handling.
fn finish_page(
    va: u64,
    desc: u64,
    desc_addr: u64,
    level: u8,
    access: MmuAccess,
    mem: &mut impl MemInterface,
    current_el: u8,
    ha: bool,
    page_shift: u32,
) -> Result<TranslateResult, MmuFault> {
    // Physical base address from descriptor bits [47:page_shift].
    let pa_mask = 0x0000_FFFF_FFFF_F000u64 & !((1u64 << page_shift) - 1);
    let pa_base = desc & pa_mask;

    // Offset within the page/block.
    let offset_mask = (1u64 << page_shift) - 1;
    let pa = pa_base | (va & offset_mask);

    // Descriptor attribute fields.
    let attr_idx = ((desc >> 2) & 0x7) as u8; // AttrIndx[2:0]
    let ap = ((desc >> 6) & 0x3) as u8; // AP[2:1]
    let pxn = desc & (1u64 << 53) != 0;
    let uxn = desc & (1u64 << 54) != 0;
    let af = desc & (1u64 << 10) != 0;

    if !af {
        if ha {
            // Hardware Access Flag management (TCR.HA=1): set AF in the
            // page table entry in memory, avoiding a software AF fault.
            let updated = desc | (1u64 << 10);
            let _ = mem.write(desc_addr, 8, updated, AccessType::Store);
        } else {
            return Err(stage1_fault(va, level, FaultKind::AccessFlag));
        }
    }

    // Permission check.
    check_permissions(va, level, ap, pxn, uxn, access, current_el)?;

    Ok(TranslateResult {
        pa,
        attr_idx,
        ap,
        pxn,
        uxn,
    })
}

// ---------------------------------------------------------------------------
// Permission checks
// ---------------------------------------------------------------------------

/// Check access permissions from AP[2:1] bits.
///
/// AP[2:1] encoding (EL1 accesses, 4KB granule):
///
/// | AP[2:1] | EL1       | EL0       |
/// |---------|-----------|-----------|
/// | `00`    | RW        | No access |
/// | `01`    | RW        | RW        |
/// | `10`    | RO        | No access |
/// | `11`    | RO        | RO        |
///
/// Enforces AP read/write rules plus PXN/UXN execute permissions for EL0/EL1/EL2/EL3.
fn check_permissions(
    va: u64,
    level: u8,
    ap: u8,
    pxn: bool,
    uxn: bool,
    access: MmuAccess,
    current_el: u8,
) -> Result<(), MmuFault> {
    match current_el {
        0 => match access {
            MmuAccess::Read => {
                if ap & 0x1 == 0 {
                    Err(stage1_fault(va, level, FaultKind::Permission))
                } else {
                    Ok(())
                }
            }
            MmuAccess::Write => {
                if ap != 0b01 {
                    Err(stage1_fault(va, level, FaultKind::Permission))
                } else {
                    Ok(())
                }
            }
            MmuAccess::Execute => {
                if ap & 0x1 == 0 || uxn {
                    Err(stage1_fault(va, level, FaultKind::Permission))
                } else {
                    Ok(())
                }
            }
        },
        _ => match access {
            MmuAccess::Read => Ok(()),
            MmuAccess::Write => {
                if ap & 0x2 != 0 {
                    Err(stage1_fault(va, level, FaultKind::Permission))
                } else {
                    Ok(())
                }
            }
            MmuAccess::Execute => {
                if pxn {
                    Err(stage1_fault(va, level, FaultKind::Permission))
                } else {
                    Ok(())
                }
            }
        },
    }
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use helm_core::{AccessType, MemFault, MemInterface};

    // -----------------------------------------------------------------------
    // Test memory: sparse physical memory backed by a HashMap.
    // -----------------------------------------------------------------------

    struct TestMem {
        data: std::collections::HashMap<u64, u64>,
    }

    impl TestMem {
        fn new() -> Self {
            Self {
                data: std::collections::HashMap::new(),
            }
        }

        /// Store a u64 at the given physical address (8-byte aligned).
        fn store_u64(&mut self, addr: u64, val: u64) {
            self.data.insert(addr, val);
        }
    }

    impl MemInterface for TestMem {
        fn read(&mut self, addr: u64, size: usize, _ty: AccessType) -> Result<u64, MemFault> {
            if size == 8 {
                Ok(*self.data.get(&addr).unwrap_or(&0))
            } else {
                Ok(0)
            }
        }

        fn write(
            &mut self,
            _addr: u64,
            _size: usize,
            _val: u64,
            _ty: AccessType,
        ) -> Result<(), MemFault> {
            Ok(())
        }
    }

    // -----------------------------------------------------------------------
    // Helpers to construct arch state with MMU on / off.
    // -----------------------------------------------------------------------

    fn make_state_mmu_off() -> Aarch64ArchState {
        let mut a = Aarch64ArchState::new();
        a.sctlr_el1 = 0; // MMU off
        a
    }

    fn make_state_mmu_on() -> Aarch64ArchState {
        let mut a = Aarch64ArchState::new();
        a.sctlr_el1 = 1; // MMU on (bit 0 = M)
        a.current_el = 1;
        a
    }

    fn map_lower_l3_page(
        a: &mut Aarch64ArchState,
        mem: &mut TestMem,
        va: u64,
        page_pa: u64,
        leaf_extra: u64,
    ) {
        a.tcr_el1 = (a.tcr_el1 & !((0x3Fu64 << 16) | 0x3F)) | 25 | (25 << 16);
        let l1_base: u64 = 0x1_0000;
        let l2_table: u64 = 0x2_0000;
        let l3_table: u64 = 0x3_0000;
        a.ttbr0_el1 = l1_base;

        let l1_index = ((va >> 30) & 0x1FF) * 8;
        let l2_index = ((va >> 21) & 0x1FF) * 8;
        let l3_index = ((va >> 12) & 0x1FF) * 8;

        mem.store_u64(l1_base + l1_index, l2_table | 0x3);
        mem.store_u64(l2_table + l2_index, l3_table | 0x3);
        mem.store_u64(l3_table + l3_index, (page_pa & !0xFFF) | leaf_extra | 0x3);
    }

    fn map_stage2_l3_page(mem: &mut TestMem, vttbr: u64, l3_table: u64, ipa: u64, pa: u64, leaf_extra: u64) {
        let l2_index = ((ipa >> 21) & 0x1FF) * 8;
        let l3_index = ((ipa >> 12) & 0x1FF) * 8;
        mem.store_u64(vttbr + l2_index, l3_table | 0x3);
        mem.store_u64(l3_table + l3_index, (pa & !0xFFF) | leaf_extra | 0x3);
    }

    #[test]
    fn tlb_flush_asid_only_removes_matching_entries() {
        let mut tlb = Tlb::new();
        let result = TranslateResult {
            pa: 0x8000_0000,
            attr_idx: 0,
            ap: 0,
            pxn: false,
            uxn: false,
        };

        tlb.insert(0x1000, result, 0x0012_0000_0000_0000);
        tlb.insert(0x2000, result, 0x0034_0000_0000_0000);

        assert!(tlb.lookup(0x1000, 0x0012_0000_0000_0000).is_some());
        assert!(tlb.lookup(0x2000, 0x0034_0000_0000_0000).is_some());

        tlb.flush_asid(0x12);

        assert!(tlb.lookup(0x1000, 0x0012_0000_0000_0000).is_none());
        assert!(tlb.lookup(0x2000, 0x0034_0000_0000_0000).is_some());
    }

    #[test]
    fn tlb_tag_uses_ttbr1_asid_when_a1_set() {
        let tcr_el1 = (1u64 << 22) | (1u64 << 36);
        let ttbr0_el1 = 0x0011_0000_0001_0000;
        let ttbr1_el1 = 0x00aa_0000_0002_0000;
        let selected_ttbr = ttbr0_el1;

        let tag = tlb_tag(selected_ttbr, 1, tcr_el1, ttbr0_el1, ttbr1_el1);

        assert_eq!(tag & TTBR_BASE_MASK, selected_ttbr & TTBR_BASE_MASK);
        assert_eq!((tag >> TTBR_ASID_SHIFT) as u16, 0x00aa);
    }

    // -----------------------------------------------------------------------
    // Test: MMU disabled => identity translation.
    // -----------------------------------------------------------------------

    #[test]
    fn mmu_off_identity_translation() {
        let a = make_state_mmu_off();
        let mut mem = TestMem::new();
        let result = translate(&a, 0x4000_0000, MmuAccess::Read, &mut mem, None).unwrap();
        assert_eq!(result.pa, 0x4000_0000);
    }

    // -----------------------------------------------------------------------
    // Test: Level 2 block descriptor (2 MB).
    // -----------------------------------------------------------------------

    #[test]
    fn level2_block_2mb() {
        let mut a = make_state_mmu_on();
        a.tcr_el1 = 16; // T0SZ=16 => 48-bit VA, start at level 0
        let mut mem = TestMem::new();

        let pgd_base: u64 = 0x1_0000;
        a.ttbr0_el1 = pgd_base;

        // L0[0] -> table at 0x2_0000
        let l1_table: u64 = 0x2_0000;
        mem.store_u64(pgd_base, l1_table | 0x3); // valid table

        // L1[0] -> table at 0x3_0000
        let l2_table: u64 = 0x3_0000;
        mem.store_u64(l1_table, l2_table | 0x3); // valid table

        // L2[0] -> 2 MB block at PA 0x8000_0000
        let block_pa: u64 = 0x8000_0000;
        mem.store_u64(l2_table, block_pa | (1 << 10) | 0x1); // valid block (bit1=0, bit0=1)

        // VA 0x10_0000: L0 idx=0, L1 idx=0, L2 idx=0, offset=0x10_0000
        let va = 0x0000_0000_0010_0000;
        let result = translate(&a, va, MmuAccess::Read, &mut mem, None).unwrap();
        assert_eq!(result.pa, block_pa | (va & 0x1F_FFFF));
    }

    // -----------------------------------------------------------------------
    // Test: Level 3 page descriptor (4 KB).
    // -----------------------------------------------------------------------

    #[test]
    fn level3_page_4kb() {
        let mut a = make_state_mmu_on();
        a.tcr_el1 = 25; // T0SZ=25 => 39-bit VA, start at level 1
        let mut mem = TestMem::new();

        let l1_base: u64 = 0x1_0000;
        a.ttbr0_el1 = l1_base;

        // L1[0] -> table at 0x2_0000
        let l2_table: u64 = 0x2_0000;
        mem.store_u64(l1_base, l2_table | 0x3);

        // L2[0] -> table at 0x3_0000
        let l3_table: u64 = 0x3_0000;
        mem.store_u64(l2_table, l3_table | 0x3);

        // L3[0] -> 4KB page at PA 0x5_0000
        let page_pa: u64 = 0x5_0000;
        mem.store_u64(l3_table, page_pa | (1 << 10) | 0x3); // valid page

        let va = 0x0000_0000_0000_0042;
        let result = translate(&a, va, MmuAccess::Read, &mut mem, None).unwrap();
        assert_eq!(result.pa, page_pa | 0x42);
    }

    // -----------------------------------------------------------------------
    // Test: Translation fault from invalid descriptor.
    // -----------------------------------------------------------------------

    #[test]
    fn translation_fault_invalid_descriptor() {
        let mut a = make_state_mmu_on();
        a.tcr_el1 = 25;
        let mut mem = TestMem::new();

        let l1_base: u64 = 0x1_0000;
        a.ttbr0_el1 = l1_base;

        // L1[0] = 0 (invalid).
        mem.store_u64(l1_base, 0);

        let result = translate(&a, 0x0, MmuAccess::Read, &mut mem, None);
        assert!(result.is_err());
        let fault = result.unwrap_err();
        assert_eq!(fault.kind, FaultKind::Translation);
        assert_eq!(fault.level, 1);
    }

    // -----------------------------------------------------------------------
    // Test: Permission fault -- write to read-only mapping.
    // -----------------------------------------------------------------------

    #[test]
    fn permission_fault_write_to_ro() {
        let mut a = make_state_mmu_on();
        a.tcr_el1 = 25;
        let mut mem = TestMem::new();

        let l1_base: u64 = 0x1_0000;
        a.ttbr0_el1 = l1_base;

        // L1[0] -> table
        let l2_table: u64 = 0x2_0000;
        mem.store_u64(l1_base, l2_table | 0x3);

        // L2[0] -> table
        let l3_table: u64 = 0x3_0000;
        mem.store_u64(l2_table, l3_table | 0x3);

        // L3[0] -> page with AP=0b10 (RO at EL1)
        let page_pa: u64 = 0x5_0000;
        let ap_ro: u64 = 0b10 << 6;
        mem.store_u64(l3_table, page_pa | (1 << 10) | ap_ro | 0x3);

        // Read should succeed.
        let result = translate(&a, 0, MmuAccess::Read, &mut mem, None);
        assert!(result.is_ok());

        // Write should fail with Permission fault.
        let result = translate(&a, 0, MmuAccess::Write, &mut mem, None);
        assert!(result.is_err());
        let fault = result.unwrap_err();
        assert_eq!(fault.kind, FaultKind::Permission);
    }

    // -----------------------------------------------------------------------
    // Test: TTBR1 selected for upper-half (kernel) addresses.
    // -----------------------------------------------------------------------

    #[test]
    fn ttbr1_for_upper_addresses() {
        let mut a = make_state_mmu_on();
        a.tcr_el1 = 25 | (25 << 16); // T0SZ=25, T1SZ=25
        let mut mem = TestMem::new();

        // TTBR0 for lower half.
        let ttbr0_base: u64 = 0x1_0000;
        a.ttbr0_el1 = ttbr0_base;

        // TTBR1 for upper half.
        let ttbr1_base: u64 = 0xA_0000;
        a.ttbr1_el1 = ttbr1_base;

        // Use a kernel VA whose L1/L2/L3 indices are all 0 within the
        // TTBR1 address space.  With T1SZ=25 the VA range starts at
        // 0xFFFF_FF80_0000_0000.  L1[0], L2[0], L3[0].
        let va = 0xFFFF_FF80_0000_0000u64;

        // Set up TTBR1 page tables for kernel VA.
        let l2_table: u64 = 0xB_0000;
        mem.store_u64(ttbr1_base, l2_table | 0x3); // L1[0]
        let l3_table: u64 = 0xC_0000;
        mem.store_u64(l2_table, l3_table | 0x3); // L2[0]
        let page_pa: u64 = 0xD_0000;
        mem.store_u64(l3_table, page_pa | (1 << 10) | 0x3); // L3[0]

        // Upper-half VA (bit63=1): should use TTBR1.
        let result = translate(&a, va, MmuAccess::Read, &mut mem, None).unwrap();
        assert_eq!(result.pa, page_pa | (va & 0xFFF));
    }

    // -----------------------------------------------------------------------
    // Test: Level 1 block descriptor (1 GB).
    // -----------------------------------------------------------------------

    #[test]
    fn level1_block_1gb() {
        let mut a = make_state_mmu_on();
        a.tcr_el1 = 16; // T0SZ=16 => 48-bit VA, start at level 0
        let mut mem = TestMem::new();

        let pgd_base: u64 = 0x1_0000;
        a.ttbr0_el1 = pgd_base;

        // L0[0] -> table at 0x2_0000
        let l1_table: u64 = 0x2_0000;
        mem.store_u64(pgd_base, l1_table | 0x3);

        // L1[0] -> 1 GB block at PA 0x4000_0000
        let block_pa: u64 = 0x4000_0000;
        mem.store_u64(l1_table, block_pa | (1 << 10) | 0x1); // valid block

        let va = 0x1234_5678u64;
        let result = translate(&a, va, MmuAccess::Read, &mut mem, None).unwrap();
        assert_eq!(result.pa, block_pa | (va & 0x3FFF_FFFF));
    }

    // -----------------------------------------------------------------------
    // Test: Fault syndrome encoding carries the correct level.
    // -----------------------------------------------------------------------

    #[test]
    fn mmu_fault_encodes_level() {
        let fault = stage1_fault(0x1000, 2, FaultKind::Translation);
        let iss = fault.iss_data(false);
        // Translation fault level 2: 0b000110 = 0x06
        assert_eq!(iss & 0x3F, 0b000110);
    }

    // -----------------------------------------------------------------------
    // Test: Data-abort WnR bit set for write faults.
    // -----------------------------------------------------------------------

    #[test]
    fn data_abort_wnr_bit() {
        let fault = stage1_fault(0x2000, 3, FaultKind::Permission);

        // Read fault: WnR = 0.
        let iss_read = fault.iss_data(false);
        assert_eq!(iss_read & (1 << 6), 0);

        // Write fault: WnR = 1.
        let iss_write = fault.iss_data(true);
        assert_ne!(iss_write & (1 << 6), 0);
    }

    // -----------------------------------------------------------------------
    // Test: Instruction-abort syndrome encoding.
    // -----------------------------------------------------------------------

    #[test]
    fn insn_abort_syndrome() {
        let fault = stage1_fault(0x3000, 1, FaultKind::Permission);
        let iss = fault.iss_insn();
        // Permission fault level 1: 0b001101 = 0x0D
        assert_eq!(iss & 0x3F, 0b001101);
    }

    // -----------------------------------------------------------------------
    // Test: Address size fault encoding.
    // -----------------------------------------------------------------------

    #[test]
    fn address_size_fault_encoding() {
        let fault = stage1_fault(0, 0, FaultKind::AddressSize);
        let iss = fault.iss_data(false);
        // Address size fault level 0: 0b000000
        assert_eq!(iss & 0x3F, 0b000000);
    }

    // -----------------------------------------------------------------------
    // Test: Access flag fault encoding.
    // -----------------------------------------------------------------------

    #[test]
    fn access_flag_fault_encoding() {
        let fault = stage1_fault(0, 2, FaultKind::AccessFlag);
        let iss = fault.iss_data(false);
        // Access flag fault level 2: 0b001010
        assert_eq!(iss & 0x3F, 0b001010);
    }

    // -----------------------------------------------------------------------
    // Test: Non-zero page offset preserved through 4KB translation.
    // -----------------------------------------------------------------------

    #[test]
    fn page_offset_preserved() {
        let mut a = make_state_mmu_on();
        a.tcr_el1 = 25; // T0SZ=25 => 39-bit VA
        let mut mem = TestMem::new();

        let l1_base: u64 = 0x1_0000;
        a.ttbr0_el1 = l1_base;

        let l2_table: u64 = 0x2_0000;
        mem.store_u64(l1_base, l2_table | 0x3);
        let l3_table: u64 = 0x3_0000;
        mem.store_u64(l2_table, l3_table | 0x3);
        let page_pa: u64 = 0x7_0000;
        mem.store_u64(l3_table, page_pa | (1 << 10) | 0x3);

        // Translate with various offsets within the page.
        for offset in [0x000u64, 0x100, 0x7FF, 0xFFF] {
            let va = offset;
            let result = translate(&a, va, MmuAccess::Read, &mut mem, None).unwrap();
            assert_eq!(
                result.pa,
                page_pa | offset,
                "offset {offset:#x} not preserved"
            );
        }
    }

    // -----------------------------------------------------------------------
    // Test: EL1 execute access allowed when PXN clear.
    // -----------------------------------------------------------------------

    #[test]
    fn execute_access_allowed() {
        let mut a = make_state_mmu_on();
        let mut mem = TestMem::new();
        let page_pa: u64 = 0x5_0000;
        map_lower_l3_page(&mut a, &mut mem, 0, page_pa, (0b10 << 6) | (1 << 10));

        let result = translate(&a, 0, MmuAccess::Execute, &mut mem, None);
        assert!(result.is_ok());
    }

    #[test]
    fn access_flag_fault_when_af_clear_and_ha_disabled() {
        let mut a = make_state_mmu_on();
        let mut mem = TestMem::new();
        map_lower_l3_page(&mut a, &mut mem, 0, 0x5_0000, 0);

        let fault = translate(&a, 0, MmuAccess::Read, &mut mem, None).unwrap_err();
        assert_eq!(fault.kind, FaultKind::AccessFlag);
        assert_eq!(fault.level, 3);
    }

    #[test]
    fn access_flag_clear_allowed_when_ha_enabled() {
        let mut a = make_state_mmu_on();
        let mut mem = TestMem::new();
        a.tcr_el1 |= 1u64 << 39; // HA
        map_lower_l3_page(&mut a, &mut mem, 0, 0x5_0000, 0);

        let result = translate(&a, 0, MmuAccess::Read, &mut mem, None).unwrap();
        assert_eq!(result.pa, 0x5_0000);
    }

    #[test]
    fn el0_read_faults_without_user_access() {
        let mut a = make_state_mmu_on();
        a.current_el = 0;
        let mut mem = TestMem::new();
        map_lower_l3_page(&mut a, &mut mem, 0, 0x5_0000, 1 << 10);

        let fault = translate(&a, 0, MmuAccess::Read, &mut mem, None).unwrap_err();
        assert_eq!(fault.kind, FaultKind::Permission);
    }

    #[test]
    fn el0_write_allowed_for_ap01() {
        let mut a = make_state_mmu_on();
        a.current_el = 0;
        let mut mem = TestMem::new();
        map_lower_l3_page(&mut a, &mut mem, 0, 0x5_0000, (1 << 10) | (0b01 << 6));

        let result = translate(&a, 0, MmuAccess::Write, &mut mem, None).unwrap();
        assert_eq!(result.pa, 0x5_0000);
    }

    #[test]
    fn el0_write_faults_for_user_ro_page() {
        let mut a = make_state_mmu_on();
        a.current_el = 0;
        let mut mem = TestMem::new();
        map_lower_l3_page(&mut a, &mut mem, 0, 0x5_0000, (1 << 10) | (0b11 << 6));

        let fault = translate(&a, 0, MmuAccess::Write, &mut mem, None).unwrap_err();
        assert_eq!(fault.kind, FaultKind::Permission);
    }

    #[test]
    fn el0_execute_faults_when_uxn_set() {
        let mut a = make_state_mmu_on();
        a.current_el = 0;
        let mut mem = TestMem::new();
        map_lower_l3_page(
            &mut a,
            &mut mem,
            0,
            0x5_0000,
            (1 << 10) | (0b01 << 6) | (1u64 << 54),
        );

        let fault = translate(&a, 0, MmuAccess::Execute, &mut mem, None).unwrap_err();
        assert_eq!(fault.kind, FaultKind::Permission);
    }

    #[test]
    fn el1_stage2_translation_works_with_stage1_disabled() {
        let mut a = Aarch64ArchState::new();
        a.current_el = 1;
        a.hcr_el2 = HCR_VM;
        a.vttbr_el2 = 0x8_0000;
        a.vtcr_el2 = 0;
        let mut mem = TestMem::new();
        map_stage2_l3_page(
            &mut mem,
            a.vttbr_el2,
            0x9_0000,
            0x4000,
            0x1234_5000,
            (1 << 10) | (0b11 << 6),
        );

        let result = translate(&a, 0x4123, MmuAccess::Read, &mut mem, None).unwrap();
        assert_eq!(result.pa, 0x1234_5123);
    }

    #[test]
    fn el1_stage1_and_stage2_translation_compose() {
        let mut a = make_state_mmu_on();
        a.hcr_el2 = HCR_VM;
        a.vttbr_el2 = 0x8_0000;
        a.vtcr_el2 = 0;
        let mut mem = TestMem::new();
        let ipa_page: u64 = 0x6000_0000;
        let pa_page: u64 = 0x7000_0000;
        map_lower_l3_page(&mut a, &mut mem, 0x4000, ipa_page, 1 << 10);
        map_stage2_l3_page(
            &mut mem,
            a.vttbr_el2,
            0x9_0000,
            ipa_page,
            pa_page,
            (1 << 10) | (0b11 << 6),
        );

        let result = translate(&a, 0x4123, MmuAccess::Read, &mut mem, None).unwrap();
        assert_eq!(result.pa, pa_page | 0x123);
    }

    #[test]
    fn el1_stage2_fault_targets_el2() {
        let mut a = Aarch64ArchState::new();
        a.current_el = 1;
        a.hcr_el2 = HCR_VM;
        a.vttbr_el2 = 0x8_0000;
        a.vtcr_el2 = 0;
        let mut mem = TestMem::new();

        let fault = translate(&a, 0x4000, MmuAccess::Read, &mut mem, None).unwrap_err();
        assert_eq!(fault.kind, FaultKind::Translation);
        assert_eq!(fault.target_el, Some(2));
        assert_eq!(fault.ipa, Some(0x4000));
        assert_eq!(fault.far, 0x4000);
    }

    #[test]
    fn el1_execute_faults_when_pxn_set() {
        let mut a = make_state_mmu_on();
        let mut mem = TestMem::new();
        map_lower_l3_page(&mut a, &mut mem, 0, 0x5_0000, (1 << 10) | (1u64 << 53));

        let fault = translate(&a, 0, MmuAccess::Execute, &mut mem, None).unwrap_err();
        assert_eq!(fault.kind, FaultKind::Permission);
    }

    #[test]
    fn tlb_fast_path_rechecks_permissions_for_el0() {
        let mut a = make_state_mmu_on();
        let mut mem = TestMem::new();
        let mut tlb = Tlb::new();
        map_lower_l3_page(&mut a, &mut mem, 0, 0x5_0000, 1 << 10);

        let warm = translate(&a, 0, MmuAccess::Read, &mut mem, Some(&mut tlb)).unwrap();
        assert_eq!(warm.pa, 0x5_0000);

        a.current_el = 0;
        let fault = translate(&a, 0, MmuAccess::Read, &mut mem, Some(&mut tlb)).unwrap_err();
        assert_eq!(fault.kind, FaultKind::Permission);
    }

    #[test]
    fn epd1_blocks_upper_half_walk() {
        let mut a = make_state_mmu_on();
        a.tcr_el1 = 25 | (25 << 16) | (1 << 23);
        a.ttbr1_el1 = 0xA_0000;
        let mut mem = TestMem::new();

        let va = 0xFFFF_FF80_0000_0000u64;
        let fault = translate(&a, va, MmuAccess::Read, &mut mem, None).unwrap_err();
        assert_eq!(fault.kind, FaultKind::Translation);
        assert_eq!(fault.level, 0);
    }

    #[test]
    fn gap_address_faults_before_walk() {
        let mut a = make_state_mmu_on();
        a.tcr_el1 = 25 | (25 << 16);
        let mut mem = TestMem::new();

        let va = 0x0001_0000_0000_0000u64;
        let fault = translate(&a, va, MmuAccess::Read, &mut mem, None).unwrap_err();
        assert_eq!(fault.kind, FaultKind::AddressSize);
        assert_eq!(fault.level, 0);
    }

    #[test]
    fn el2_non_vhe_uses_ttbr0_el2_single_space() {
        let mut a = make_state_mmu_on();
        a.current_el = 2;
        a.sctlr_el2 = 1;
        a.sctlr_el1 = 0;
        a.tcr_el2 = 25;
        a.ttbr0_el2 = 0x8_0000;
        let mut mem = TestMem::new();

        let l2_table = 0x8_1000;
        let l3_table = 0x8_2000;
        let page_pa = 0x9_0000;
        mem.store_u64(a.ttbr0_el2, l2_table | 0x3);
        mem.store_u64(l2_table, l3_table | 0x3);
        mem.store_u64(l3_table, page_pa | (1 << 10) | 0x3);

        let result = translate(&a, 0, MmuAccess::Read, &mut mem, None).unwrap();
        assert_eq!(result.pa, page_pa);
    }

    #[test]
    fn el2_non_vhe_upper_half_faults() {
        let mut a = make_state_mmu_on();
        a.current_el = 2;
        a.sctlr_el2 = 1;
        a.sctlr_el1 = 0;
        a.tcr_el2 = 25;
        let mut mem = TestMem::new();

        let va = 0xFFFF_FF80_0000_0000u64;
        let fault = translate(&a, va, MmuAccess::Read, &mut mem, None).unwrap_err();
        assert_eq!(fault.kind, FaultKind::AddressSize);
        assert_eq!(fault.level, 0);
    }

    #[test]
    fn el2_vhe_uses_ttbr1_el2_for_upper_half() {
        let mut a = make_state_mmu_on();
        a.current_el = 2;
        a.sctlr_el2 = 1;
        a.sctlr_el1 = 0;
        a.hcr_el2 = HCR_E2H;
        a.tcr_el2 = 25 | (25 << 16);
        a.ttbr1_el2 = 0xA_0000;
        let mut mem = TestMem::new();

        let va = 0xFFFF_FF80_0000_0000u64;
        let l2_table = 0xA_1000;
        let l3_table = 0xA_2000;
        let page_pa = 0xB_0000;
        mem.store_u64(a.ttbr1_el2, l2_table | 0x3);
        mem.store_u64(l2_table, l3_table | 0x3);
        mem.store_u64(l3_table, page_pa | (1 << 10) | 0x3);

        let result = translate(&a, va, MmuAccess::Read, &mut mem, None).unwrap();
        assert_eq!(result.pa, page_pa | (va & 0xFFF));
    }

    #[test]
    fn el3_uses_tcr_el3_and_ttbr0_el3() {
        let mut a = make_state_mmu_on();
        a.current_el = 3;
        a.sctlr_el3 = 1;
        a.sctlr_el1 = 0;
        a.tcr_el3 = 25;
        a.ttbr0_el3 = 0xC_0000;
        let mut mem = TestMem::new();

        let l2_table = 0xC_1000;
        let l3_table = 0xC_2000;
        let page_pa = 0xD_0000;
        mem.store_u64(a.ttbr0_el3, l2_table | 0x3);
        mem.store_u64(l2_table, l3_table | 0x3);
        mem.store_u64(l3_table, page_pa | (1 << 10) | (1u64 << 53) | 0x3);

        let result = translate(&a, 0, MmuAccess::Read, &mut mem, None).unwrap();
        assert_eq!(result.pa, page_pa);

        let fault = translate(&a, 0, MmuAccess::Execute, &mut mem, None).unwrap_err();
        assert_eq!(fault.kind, FaultKind::Permission);
    }

    // -----------------------------------------------------------------------
    // Test: Attribute index extracted correctly.
    // -----------------------------------------------------------------------

    #[test]
    fn attr_index_extracted() {
        let mut a = make_state_mmu_on();
        a.tcr_el1 = 25;
        let mut mem = TestMem::new();

        let l1_base: u64 = 0x1_0000;
        a.ttbr0_el1 = l1_base;

        let l2_table: u64 = 0x2_0000;
        mem.store_u64(l1_base, l2_table | 0x3);
        let l3_table: u64 = 0x3_0000;
        mem.store_u64(l2_table, l3_table | 0x3);

        // Page with AttrIndx = 5 (bits[4:2] = 0b101).
        let page_pa: u64 = 0x5_0000;
        let attr_idx_val: u64 = 5 << 2; // AttrIndx[2:0] in bits[4:2]
        mem.store_u64(l3_table, page_pa | (1 << 10) | attr_idx_val | 0x3);

        let result = translate(&a, 0, MmuAccess::Read, &mut mem, None).unwrap();
        assert_eq!(result.attr_idx, 5);
    }
}
