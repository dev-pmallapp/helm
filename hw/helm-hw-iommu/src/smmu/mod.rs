//! ARM `SMMUv3` — System MMU for DMA isolation.
//!
//! Provides an IOMMU model that translates I/O Virtual Addresses (IOVAs)
//! to Physical Addresses (PAs) based on per-stream configuration.
//!
//! # Quick start
//!
//! ```ignore
//! use helm_hw_iommu::smmu::SmmuState;
//! use helm_hw_iommu::ByteMem;
//!
//! let mem = MyByteMem::new(ram_size);
//! let mut smmu = SmmuState::new(mem);
//! // Configure stream table, enable SMMU, then:
//! let result = smmu.translate(stream_id, iova, is_write);
//! ```

use helm_devices::{Device, InterruptPin};

use crate::common::fault::{IommuFault, IommuTranslateResult};
use crate::common::mem::ByteMem;
use crate::common::tlb::IommuTlb;

// ── Register offsets (from SMMUv3 spec) ─────────────────────────────────────

const SMMU_IDR0: u64 = 0x0000;
const SMMU_IDR1: u64 = 0x0004;
const SMMU_IDR2: u64 = 0x0008;
const SMMU_IDR3: u64 = 0x000C;
const SMMU_IDR5: u64 = 0x0014;
const SMMU_IIDR: u64 = 0x0018;
const SMMU_AIDR: u64 = 0x001C;
const SMMU_CR0: u64 = 0x0020;
const SMMU_CR0ACK: u64 = 0x0024;
const SMMU_CR1: u64 = 0x0028;
const SMMU_CR2: u64 = 0x002C;
const SMMU_STATUSR: u64 = 0x0040;
const SMMU_GBPA: u64 = 0x0044;
const SMMU_IRQ_CTRL: u64 = 0x0060;
const SMMU_IRQ_CTRLACK: u64 = 0x0064;
const SMMU_GERROR: u64 = 0x0068;
const SMMU_GERRORN: u64 = 0x006C;

const SMMU_STRTAB_BASE: u64 = 0x0080;
const SMMU_STRTAB_BASE_HI: u64 = 0x0084;
const SMMU_STRTAB_BASE_CFG: u64 = 0x0088;

const SMMU_CMDQ_BASE: u64 = 0x0090;
const SMMU_CMDQ_BASE_HI: u64 = 0x0094;
const SMMU_CMDQ_PROD: u64 = 0x0098;
const SMMU_CMDQ_CONS: u64 = 0x009C;

const SMMU_EVTQ_BASE: u64 = 0x00A0;
const SMMU_EVTQ_BASE_HI: u64 = 0x00A4;
const SMMU_EVTQ_PROD: u64 = 0x00A8;
const SMMU_EVTQ_CONS: u64 = 0x00AC;
const SMMU_EVTQ_IRQ_CFG0: u64 = 0x00B0;
const SMMU_EVTQ_IRQ_CFG1: u64 = 0x00B8;
const SMMU_EVTQ_IRQ_CFG2: u64 = 0x00BC;

// CR0 bits
const CR0_SMMUEN: u32 = 1 << 0;
const CR0_CMDQEN: u32 = 1 << 1;
const CR0_EVTQEN: u32 = 1 << 2;

// GBPA bits
const GBPA_ABORT: u32 = 1 << 20;

// IRQ_CTRL bits
const IRQ_CTRL_EVTQ_IRQEN: u32 = 1 << 0;
const IRQ_CTRL_GERROR_IRQEN: u32 = 1 << 2;

// GERROR bits
const GERROR_CMDQ_ERR: u32 = 1 << 4;

// Command opcodes
const CMD_CFGI_STE_RANGE: u8 = 0x04;
const CMD_CFGI_ALL: u8 = 0x05;
const CMD_TLBI_NH_ALL: u8 = 0x20;
const CMD_TLBI_NH_ASID: u8 = 0x21;
const CMD_TLBI_NH_VA: u8 = 0x22;
const CMD_SYNC: u8 = 0x46;

// ── ARM-specific fault codes ────────────────────────────────────────────────

/// SMMU fault type codes (from `SMMUv3` spec section 6.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum SmmuFaultCode {
    /// SID exceeds stream table size.
    BadStreamId = 0x01,
    /// Error reading STE from memory.
    SteFetch = 0x02,
    /// STE valid=0 or Config=Abort.
    BadSte = 0x03,
    /// SMMU disabled and GBPA=Abort.
    StreamDisabled = 0x05,
    /// `SubstreamID` exceeds CD table size.
    BadSubstreamId = 0x07,
    /// Error reading CD from memory.
    CdFetch = 0x08,
    /// CD valid=0.
    BadCd = 0x09,
    /// External abort during page table walk.
    WalkEabt = 0x0A,
    /// Page table entry not valid.
    Translation = 0x10,
    /// Output PA exceeds physical address size.
    AddrSize = 0x11,
    /// Access flag fault (AF=0).
    Access = 0x12,
    /// Permission fault.
    Permission = 0x13,
}

/// Full fault context for ARM SMMU event recording.
#[derive(Debug)]
pub struct SmmuFault {
    /// Fault type code.
    pub code: SmmuFaultCode,
    /// Stream ID that caused the fault.
    pub stream_id: u32,
    /// Faulting input address (IOVA or IPA).
    pub input_addr: u64,
    /// Was the transaction a write?
    pub is_write: bool,
}

/// ARM SMMU translation result (wraps the generic [`IommuTranslateResult`]
/// while keeping the ARM-specific [`SmmuFault`] type for event queue writing).
#[derive(Debug)]
pub enum SmmuTranslateResult {
    /// Translation succeeded — output physical address.
    Ok(u64),
    /// Bypass — no translation, pass IOVA through as PA.
    Bypass,
    /// Translation fault — must record event.
    Fault(SmmuFault),
}

/// Build a 32-byte event record (4 doublewords) for the event queue.
///
/// Format (from `SMMUv3` spec section 6.2):
/// ```text
/// DW0: [7:0] type, [15:8] stall=0, [63:32] StreamID
/// DW1: [63:0] input address
/// DW2: [63:0] flags (bit 1 = WRITE)
/// DW3: [63:0] reserved
/// ```
#[must_use]
pub fn build_event_record(fault: &SmmuFault) -> [u8; 32] {
    let mut record = [0u8; 32];

    let dw0 = u64::from(fault.code as u8) | (u64::from(fault.stream_id) << 32);
    record[0..8].copy_from_slice(&dw0.to_le_bytes());

    record[8..16].copy_from_slice(&fault.input_addr.to_le_bytes());

    let flags: u64 = if fault.is_write { 0x2 } else { 0x0 };
    record[16..24].copy_from_slice(&flags.to_le_bytes());

    record
}

// ── Stream table format ─────────────────────────────────────────────────────

/// Stream table format: linear (flat) or 2-level.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum StrtabFmt {
    /// Linear: flat array of STEs indexed by SID.
    #[default]
    Linear,
    /// 2-level: L1 descriptor array + L2 STE pages.
    TwoLevel,
}

// ── STE Config field ────────────────────────────────────────────────────────

/// Decoded STE configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SteConfig {
    Abort,
    Bypass,
    S1Only,
    S2Only,
    S1S2,
}

impl SteConfig {
    fn from_bits(bits: u8) -> Self {
        match bits & 0x7 {
            0b100 => Self::Bypass,
            0b101 => Self::S1Only,
            0b110 => Self::S2Only,
            0b111 => Self::S1S2,
            _ => Self::Abort,
        }
    }
}

// ── Parsed STE ──────────────────────────────────────────────────────────────

/// Parsed Stream Table Entry (subset of fields needed for translation).
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub(crate) struct ParsedSte {
    pub valid: bool,
    pub config: SteConfig,
    pub s1_context_ptr: u64,
    pub s1_cd_max: u8,
    pub vmid: u16,
    pub s2_ttb: u64,
    pub s2_t0sz: u8,
    pub s2_tg: u8,
    pub s2_ps: u8,
}

// ── Parsed CD ───────────────────────────────────────────────────────────────

/// Parsed Context Descriptor (subset for S1 translation).
#[derive(Debug, Clone)]
pub(crate) struct ParsedCd {
    pub valid: bool,
    pub t0sz: u8,
    pub tg0: u8,
    pub ttb0: u64,
    pub asid: u16,
}

// ── SmmuState ───────────────────────────────────────────────────────────────

/// ARM `SMMUv3` state.
///
/// Contains all control registers, queues, and TLB state. The `mem` field
/// provides access to guest physical memory for stream table / page table
/// walks. In production this wraps [`FlatMem`]; in tests, a `Vec<u8>`.
pub struct SmmuState<M: ByteMem> {
    // ── SMMU identification (RO) ─────────────────────────────────────────
    /// `SMMU_IDR0`: feature flags.
    pub idr0: u32,
    /// `SMMU_IDR1`: queue capacities.
    pub idr1: u32,
    /// `SMMU_IDR2`: address sizes.
    pub idr2: u32,
    /// `SMMU_IDR3`.
    pub idr3: u32,
    /// `SMMU_IDR5`: OAS, granule support.
    pub idr5: u32,
    /// `SMMU_IIDR`: implementer ID.
    pub iidr: u32,
    /// `SMMU_AIDR`: architecture revision.
    pub aidr: u32,

    // ── Global control ───────────────────────────────────────────────────
    /// `SMMU_CR0`: `SMMUEN`, `CMDQEN`, `EVTQEN`, `PRIQEN`.
    pub cr0: u32,
    /// `SMMU_CR0ACK`: mirrors `CR0` after ack.
    pub cr0ack: u32,
    /// `SMMU_CR1`: cache/shareability for table walks.
    pub cr1: u32,
    /// `SMMU_CR2`.
    pub cr2: u32,
    /// `SMMU_STATUSR`: IDLE, DORMANT.
    pub statusr: u32,
    /// `SMMU_GBPA`: global bypass/abort.
    pub gbpa: u32,

    // ── IRQ control ──────────────────────────────────────────────────────
    /// `SMMU_IRQ_CTRL`: per-queue IRQ enables.
    pub irq_ctrl: u32,
    /// `SMMU_IRQ_CTRLACK`.
    pub irq_ctrlack: u32,
    /// `SMMU_GERROR`: sticky global errors.
    pub gerror: u32,
    /// `SMMU_GERRORN`: written to acknowledge `GERROR` bits.
    pub gerrorn: u32,

    // ── Stream table ─────────────────────────────────────────────────────
    /// `SMMU_STRTAB_BASE` (64-bit).
    pub strtab_base: u64,
    /// `SMMU_STRTAB_BASE_CFG`.
    pub strtab_base_cfg: u32,
    /// Decoded format.
    pub strtab_fmt: StrtabFmt,
    /// log2 of stream table size.
    pub strtab_log2size: u8,
    /// L1/L2 split for 2-level.
    pub strtab_split: u8,

    // ── Command queue ────────────────────────────────────────────────────
    /// `SMMU_CMDQ_BASE` (64-bit).
    pub cmdq_base: u64,
    /// `SMMU_CMDQ_PROD`.
    pub cmdq_prod: u32,
    /// `SMMU_CMDQ_CONS`.
    pub cmdq_cons: u32,
    /// Decoded: log2 of command queue depth (entries).
    pub cmdq_log2size: u8,

    // ── Event queue ──────────────────────────────────────────────────────
    /// `SMMU_EVTQ_BASE` (64-bit).
    pub evtq_base: u64,
    /// `SMMU_EVTQ_PROD`.
    pub evtq_prod: u32,
    /// `SMMU_EVTQ_CONS`.
    pub evtq_cons: u32,
    /// Decoded: log2 of event queue depth.
    pub evtq_log2size: u8,
    /// Event queue IRQ config (MSI address, data, control).
    pub evtq_irq_cfg: [u64; 3],

    // ── TLB ──────────────────────────────────────────────────────────────
    /// Software TLB cache.
    pub tlb: IommuTlb,

    // ── IRQ output pins ──────────────────────────────────────────────────
    /// GERROR interrupt output → `GICv3` SPI.
    pub gerror_irq: InterruptPin,
    /// Event queue interrupt output → `GICv3` SPI.
    pub evtq_irq: InterruptPin,

    // ── Guest memory ─────────────────────────────────────────────────────
    /// Access to guest physical memory for table walks.
    pub mem: M,
}

impl<M: ByteMem> SmmuState<M> {
    /// Create a new SMMU with default identification registers.
    pub fn new(mem: M) -> Self {
        Self {
            idr0: 0x0000_0001,
            idr1: (15 << 21) | (15 << 16),
            idr2: (4 << 4) | 4,
            idr3: 0,
            idr5: 4 | (1 << 4),
            iidr: 0x4845_4C4D, // "HELM"
            aidr: 0x01,

            cr0: 0,
            cr0ack: 0,
            cr1: 0,
            cr2: 0,
            statusr: 0,
            gbpa: 0,

            irq_ctrl: 0,
            irq_ctrlack: 0,
            gerror: 0,
            gerrorn: 0,

            strtab_base: 0,
            strtab_base_cfg: 0,
            strtab_fmt: StrtabFmt::default(),
            strtab_log2size: 0,
            strtab_split: 0,

            cmdq_base: 0,
            cmdq_prod: 0,
            cmdq_cons: 0,
            cmdq_log2size: 0,

            evtq_base: 0,
            evtq_prod: 0,
            evtq_cons: 0,
            evtq_log2size: 0,
            evtq_irq_cfg: [0; 3],

            tlb: IommuTlb::new(),

            gerror_irq: InterruptPin::new(),
            evtq_irq: InterruptPin::new(),

            mem,
        }
    }

    /// Return true if the SMMU is globally enabled.
    pub fn is_enabled(&self) -> bool {
        (self.cr0 & CR0_SMMUEN) != 0
    }

    // ── Stream table lookup ──────────────────────────────────────────────

    fn granule_params(granule: u8) -> Option<(u32, u32, u32)> {
        match granule {
            0 => Some((12, 9, 4)),  // 4K
            1 => Some((16, 13, 3)), // 64K
            _ => None,
        }
    }

    /// Look up a Stream Table Entry by stream ID.
    ///
    /// # Two-level stream table (`StrtabFmt::TwoLevel`)
    ///
    /// Two-level STE lookup (L1 descriptor array + L2 STE pages) is **not yet
    /// implemented**. If the guest programs `STRTAB_BASE_CFG.FMT = 1`, this
    /// method returns an `SteFetch` fault for every stream ID.  This is an
    /// intentional stub: the linear format covers all current platform needs
    /// (arm-virt, virtio requesters).  When two-level support is added, the
    /// `StrtabFmt::TwoLevel` arm below should perform the L1-descriptor fetch,
    /// extract the L2 base, and index into the L2 page to locate the STE.
    #[allow(clippy::cast_possible_truncation)]
    pub(crate) fn lookup_ste(&mut self, stream_id: u32) -> Result<ParsedSte, SmmuFault> {
        let max_sids = 1u32 << self.strtab_log2size;
        if stream_id >= max_sids {
            return Err(SmmuFault {
                code: SmmuFaultCode::BadStreamId,
                stream_id,
                input_addr: 0,
                is_write: false,
            });
        }

        let table_base = self.strtab_base & !0x3F;
        if self.strtab_fmt == StrtabFmt::TwoLevel {
            log::warn!(
                "SMMU: two-level stream table not implemented; \
                 rejecting SID {stream_id} with SteFetch fault"
            );
            return Err(SmmuFault {
                code: SmmuFaultCode::SteFetch,
                stream_id,
                input_addr: table_base,
                is_write: false,
            });
        }
        let ste_addr = table_base + u64::from(stream_id) * 64;

        let dw0 = self.mem.read_le_u64(ste_addr, 8).map_err(|_| SmmuFault {
            code: SmmuFaultCode::SteFetch,
            stream_id,
            input_addr: ste_addr,
            is_write: false,
        })?;
        let dw1 = self
            .mem
            .read_le_u64(ste_addr + 8, 8)
            .map_err(|_| SmmuFault {
                code: SmmuFaultCode::SteFetch,
                stream_id,
                input_addr: ste_addr + 8,
                is_write: false,
            })?;
        let dw2 = self
            .mem
            .read_le_u64(ste_addr + 16, 8)
            .map_err(|_| SmmuFault {
                code: SmmuFaultCode::SteFetch,
                stream_id,
                input_addr: ste_addr + 16,
                is_write: false,
            })?;

        let valid = (dw0 & 0x1) != 0;
        let config_bits = ((dw0 >> 1) & 0x7) as u8;
        let config = SteConfig::from_bits(config_bits);

        let s1_context_ptr = dw0 & 0x000F_FFFF_FFFF_FFC0;
        let s1_cd_max = ((dw0 >> 59) & 0x1F) as u8;

        let vmid = (dw1 & 0xFFFF) as u16;

        let s2_ttb = dw2 & 0x000F_FFFF_FFFF_FFF0;
        let s2_t0sz = (dw2 & 0x3F) as u8;
        let s2_tg = ((dw2 >> 56) & 0x3) as u8;
        let s2_ps = ((dw2 >> 32) & 0x7) as u8;

        Ok(ParsedSte {
            valid,
            config,
            s1_context_ptr,
            s1_cd_max,
            vmid,
            s2_ttb,
            s2_t0sz,
            s2_tg,
            s2_ps,
        })
    }

    /// Look up a Context Descriptor from the CD table.
    ///
    /// # Sub-stream IDs (intentionally out of scope)
    ///
    /// The `_sub_stream_id` parameter is accepted for API completeness but is
    /// currently **ignored**.  Sub-stream IDs (also called SubstreamIDs or
    /// PASIDs) allow a single stream to carry multiple address-space contexts.
    /// This feature requires multi-entry CD table indexing and is not needed by
    /// any current platform requester.  Until sub-stream support is added, this
    /// method always reads the base CD entry at `ste.s1_context_ptr` regardless
    /// of the sub-stream ID value.
    #[allow(clippy::cast_possible_truncation, clippy::unnecessary_wraps)]
    pub(crate) fn lookup_cd(
        &mut self,
        ste: &ParsedSte,
        _sub_stream_id: u32,
    ) -> Result<ParsedCd, SmmuFault> {
        let cd_addr = ste.s1_context_ptr;

        let dw0 = self.mem.read_le_u64(cd_addr, 8).map_err(|_| SmmuFault {
            code: SmmuFaultCode::CdFetch,
            stream_id: 0,
            input_addr: cd_addr,
            is_write: false,
        })?;
        let dw1 = self
            .mem
            .read_le_u64(cd_addr + 8, 8)
            .map_err(|_| SmmuFault {
                code: SmmuFaultCode::CdFetch,
                stream_id: 0,
                input_addr: cd_addr + 8,
                is_write: false,
            })?;

        let valid = (dw0 & (1 << 31)) != 0;
        let t0sz = ((dw0 >> 32) & 0x3F) as u8;
        let tg0 = ((dw0 >> 46) & 0x3) as u8;
        let ttb0 = dw1 & 0x0000_FFFF_FFFF_F000;
        let asid = ((dw1 >> 48) & 0xFFFF) as u16;

        Ok(ParsedCd {
            valid,
            t0sz,
            tg0,
            ttb0,
            asid,
        })
    }

    fn walk_aarch64(
        &mut self,
        table_base: u64,
        t0sz: u8,
        granule: u8,
        va: u64,
        is_write: bool,
        stream_id: u32,
    ) -> Result<(u64, u64, u32), SmmuFault> {
        let make_fault = |code: SmmuFaultCode| SmmuFault {
            code,
            stream_id,
            input_addr: va,
            is_write,
        };

        let Some((page_shift, bits_per_level, max_levels)) = Self::granule_params(granule) else {
            return Err(make_fault(SmmuFaultCode::Translation));
        };

        let input_bits = 64_u32.saturating_sub(u32::from(t0sz));
        if input_bits <= page_shift {
            return Err(make_fault(SmmuFaultCode::Translation));
        }

        let walk_bits = input_bits - page_shift;
        let num_levels = walk_bits.div_ceil(bits_per_level).clamp(1, max_levels);
        let start_level = max_levels - num_levels;
        let index_mask = (1u64 << bits_per_level) - 1;
        let page_mask = !((1u64 << page_shift) - 1);

        let mut table_addr = table_base & page_mask;

        for level in start_level..max_levels {
            let shift = page_shift + (max_levels - 1 - level) * bits_per_level;
            let index = (va >> shift) & index_mask;
            let desc_addr = table_addr + index * 8;
            let desc = self
                .mem
                .read_le_u64(desc_addr, 8)
                .map_err(|_| make_fault(SmmuFaultCode::WalkEabt))?;

            if (desc & 0x1) == 0 {
                return Err(make_fault(SmmuFaultCode::Translation));
            }

            let is_table = (desc & 0x2) != 0;
            if is_table && level != max_levels - 1 {
                table_addr = desc & page_mask;
                continue;
            }

            let output_mask = !((1u64 << shift) - 1);
            let pa_base = (desc & page_mask) & output_mask;
            let page_offset = va & ((1u64 << shift) - 1);
            let page_size = 1u64 << shift;

            if pa_base >> 48 != 0 {
                return Err(make_fault(SmmuFaultCode::AddrSize));
            }

            if (desc & (1 << 10)) == 0 {
                return Err(make_fault(SmmuFaultCode::Access));
            }

            let ap = (desc >> 6) & 0x3;
            let read_only = (ap & 0x2) != 0;
            if is_write && read_only {
                return Err(make_fault(SmmuFaultCode::Permission));
            }

            let mut prot = 0x1u32;
            if !read_only {
                prot |= 0x2;
            }
            if (desc & (1u64 << 54)) == 0 {
                prot |= 0x4;
            }

            return Ok((pa_base | page_offset, page_size, prot));
        }

        Err(make_fault(SmmuFaultCode::Translation))
    }

    // ── Page table walk (Stage 1) ────────────────────────────────────────

    /// Walk a Stage 1 page table (4KB granule, `AArch64` format).
    pub(crate) fn walk_s1(
        &mut self,
        cd: &ParsedCd,
        va: u64,
        is_write: bool,
        stream_id: u32,
    ) -> Result<(u64, u64, u32), SmmuFault> {
        self.walk_aarch64(cd.ttb0, cd.t0sz, cd.tg0, va, is_write, stream_id)
    }

    /// Walk a Stage 2 page table (`AArch64` format) for the given IPA.
    pub(crate) fn walk_s2(
        &mut self,
        ste: &ParsedSte,
        ipa: u64,
        is_write: bool,
        stream_id: u32,
    ) -> Result<(u64, u64, u32), SmmuFault> {
        self.walk_aarch64(ste.s2_ttb, ste.s2_t0sz, ste.s2_tg, ipa, is_write, stream_id)
    }

    // ── Top-level translate ──────────────────────────────────────────────

    /// Translate an IOVA to a PA for a given stream ID.
    pub fn translate(&mut self, stream_id: u32, iova: u64, is_write: bool) -> SmmuTranslateResult {
        fn record_fault<M: ByteMem>(
            smmu: &mut SmmuState<M>,
            code: SmmuFaultCode,
            stream_id: u32,
            iova: u64,
            is_write: bool,
        ) -> SmmuTranslateResult {
            let fault = SmmuFault {
                code,
                stream_id,
                input_addr: iova,
                is_write,
            };
            smmu.write_event_record(&fault);
            SmmuTranslateResult::Fault(fault)
        }

        if !self.is_enabled() {
            if (self.gbpa & GBPA_ABORT) != 0 {
                return record_fault(
                    self,
                    SmmuFaultCode::StreamDisabled,
                    stream_id,
                    iova,
                    is_write,
                );
            }
            return SmmuTranslateResult::Bypass;
        }

        let ste = match self.lookup_ste(stream_id) {
            Ok(ste) => ste,
            Err(fault) => {
                self.write_event_record(&fault);
                return SmmuTranslateResult::Fault(fault);
            }
        };

        if !ste.valid {
            return record_fault(self, SmmuFaultCode::BadSte, stream_id, iova, is_write);
        }

        let cached_asid = match ste.config {
            SteConfig::S1Only | SteConfig::S1S2 => match self.lookup_cd(&ste, 0) {
                Ok(cd) if cd.valid => cd.asid,
                Ok(_) => {
                    return record_fault(self, SmmuFaultCode::BadCd, stream_id, iova, is_write)
                }
                Err(fault) => {
                    self.write_event_record(&fault);
                    return SmmuTranslateResult::Fault(fault);
                }
            },
            SteConfig::S2Only | SteConfig::Bypass | SteConfig::Abort => 0,
        };

        if let Some(entry) = self.tlb.lookup(stream_id, cached_asid, iova) {
            let page_mask = !(entry.size - 1);
            let pa = entry.pa | (iova & !page_mask);
            if is_write && (entry.prot & 0x2) == 0 {
                return record_fault(self, SmmuFaultCode::Permission, stream_id, iova, is_write);
            }
            return SmmuTranslateResult::Ok(pa);
        }

        match ste.config {
            SteConfig::Bypass => SmmuTranslateResult::Bypass,

            SteConfig::Abort => {
                record_fault(self, SmmuFaultCode::BadSte, stream_id, iova, is_write)
            }

            SteConfig::S1Only => {
                let cd = match self.lookup_cd(&ste, 0) {
                    Ok(cd) => cd,
                    Err(fault) => {
                        self.write_event_record(&fault);
                        return SmmuTranslateResult::Fault(fault);
                    }
                };

                if !cd.valid {
                    return record_fault(self, SmmuFaultCode::BadCd, stream_id, iova, is_write);
                }

                match self.walk_s1(&cd, iova, is_write, stream_id) {
                    Ok((pa, size, prot)) => {
                        self.tlb.fill(stream_id, cd.asid, iova, pa, size, prot);
                        SmmuTranslateResult::Ok(pa)
                    }
                    Err(fault) => {
                        self.write_event_record(&fault);
                        SmmuTranslateResult::Fault(fault)
                    }
                }
            }

            SteConfig::S2Only => match self.walk_s2(&ste, iova, is_write, stream_id) {
                Ok((pa, size, prot)) => {
                    self.tlb.fill(stream_id, 0, iova, pa, size, prot);
                    SmmuTranslateResult::Ok(pa)
                }
                Err(fault) => {
                    self.write_event_record(&fault);
                    SmmuTranslateResult::Fault(fault)
                }
            },

            SteConfig::S1S2 => {
                let cd = match self.lookup_cd(&ste, 0) {
                    Ok(cd) => cd,
                    Err(fault) => {
                        self.write_event_record(&fault);
                        return SmmuTranslateResult::Fault(fault);
                    }
                };

                if !cd.valid {
                    return record_fault(self, SmmuFaultCode::BadCd, stream_id, iova, is_write);
                }

                match self.walk_s1(&cd, iova, is_write, stream_id) {
                    Ok((ipa, size1, prot1)) => match self.walk_s2(&ste, ipa, is_write, stream_id) {
                        Ok((pa, size2, prot2)) => {
                            self.tlb.fill(
                                stream_id,
                                cd.asid,
                                iova,
                                pa,
                                size1.min(size2),
                                prot1 & prot2,
                            );
                            SmmuTranslateResult::Ok(pa)
                        }
                        Err(fault) => {
                            self.write_event_record(&fault);
                            SmmuTranslateResult::Fault(fault)
                        }
                    },
                    Err(fault) => {
                        self.write_event_record(&fault);
                        SmmuTranslateResult::Fault(fault)
                    }
                }
            }
        }
    }

    /// DMA read through the SMMU for one requester stream.
    pub fn dma_read(&mut self, stream_id: u32, iova: u64, buf: &mut [u8]) -> Result<(), SmmuFault> {
        let pa = match self.translate(stream_id, iova, false) {
            SmmuTranslateResult::Ok(pa) => pa,
            SmmuTranslateResult::Bypass => iova,
            SmmuTranslateResult::Fault(fault) => return Err(fault),
        };
        self.mem.read_bytes(pa, buf).map_err(|_| SmmuFault {
            code: SmmuFaultCode::WalkEabt,
            stream_id,
            input_addr: iova,
            is_write: false,
        })
    }

    /// DMA write through the SMMU for one requester stream.
    pub fn dma_write(&mut self, stream_id: u32, iova: u64, buf: &[u8]) -> Result<(), SmmuFault> {
        let pa = match self.translate(stream_id, iova, true) {
            SmmuTranslateResult::Ok(pa) => pa,
            SmmuTranslateResult::Bypass => iova,
            SmmuTranslateResult::Fault(fault) => return Err(fault),
        };
        self.mem.write_bytes(pa, buf).map_err(|_| SmmuFault {
            code: SmmuFaultCode::WalkEabt,
            stream_id,
            input_addr: iova,
            is_write: true,
        })
    }

    /// Copy bytes between two IOVA ranges through the SMMU for one requester stream.
    pub fn dma_copy(
        &mut self,
        stream_id: u32,
        src_iova: u64,
        dst_iova: u64,
        len: usize,
    ) -> Result<(), SmmuFault> {
        let mut buf = vec![0u8; len];
        self.dma_read(stream_id, src_iova, &mut buf)?;
        self.dma_write(stream_id, dst_iova, &buf)
    }

    /// Convert an [`SmmuTranslateResult`] to the generic [`IommuTranslateResult`].
    #[allow(clippy::cast_possible_truncation)]
    pub fn translate_generic(
        &mut self,
        stream_id: u32,
        iova: u64,
        is_write: bool,
    ) -> IommuTranslateResult {
        match self.translate(stream_id, iova, is_write) {
            SmmuTranslateResult::Ok(pa) => IommuTranslateResult::Ok(pa),
            SmmuTranslateResult::Bypass => IommuTranslateResult::Bypass,
            SmmuTranslateResult::Fault(f) => IommuTranslateResult::Fault(IommuFault {
                code: f.code as u8,
                device_id: f.stream_id,
                input_addr: f.input_addr,
                is_write: f.is_write,
            }),
        }
    }

    // ── Event queue ──────────────────────────────────────────────────────

    /// Write a fault record to the event queue and update IRQ state.
    pub fn write_event_record(&mut self, fault: &SmmuFault) {
        if (self.cr0 & CR0_EVTQEN) == 0 {
            return;
        }

        let depth = 1u32 << self.evtq_log2size;
        let index_mask = depth - 1;
        let prod_idx = self.evtq_prod & index_mask;
        let base_addr = self.evtq_base & !0x1F;

        let addr = base_addr + u64::from(prod_idx) * 32;
        let _ = self.mem.write_le_u64(
            addr,
            8,
            u64::from(fault.code as u8) | (u64::from(fault.stream_id) << 32),
        );
        let _ = self.mem.write_le_u64(addr + 8, 8, fault.input_addr);
        let _ = self
            .mem
            .write_le_u64(addr + 16, 8, if fault.is_write { 2 } else { 0 });
        let _ = self.mem.write_le_u64(addr + 24, 8, 0);

        self.evtq_prod = (self.evtq_prod + 1) & ((2 * depth) - 1);

        self.update_irq_lines();
    }

    // ── Command queue processing ─────────────────────────────────────────

    /// Process all pending commands in the command queue.
    #[allow(clippy::cast_possible_truncation)]
    pub fn process_cmdq(&mut self) {
        if (self.cr0 & CR0_CMDQEN) == 0 {
            return;
        }

        let depth = 1u32 << self.cmdq_log2size;
        let index_mask = depth - 1;
        let wrap_bit = depth;
        let base_addr = self.cmdq_base & !0x1F;

        while self.cmdq_cons != self.cmdq_prod {
            let cons_idx = self.cmdq_cons & index_mask;
            let cmd_addr = base_addr + u64::from(cons_idx) * 16;

            let dw0 = if let Ok(dw0) = self.mem.read_le_u64(cmd_addr, 8) {
                dw0
            } else {
                self.gerror |= GERROR_CMDQ_ERR;
                self.update_irq_lines();
                break;
            };
            let dw1 = if let Ok(dw1) = self.mem.read_le_u64(cmd_addr + 8, 8) {
                dw1
            } else {
                self.gerror |= GERROR_CMDQ_ERR;
                self.update_irq_lines();
                break;
            };

            let opcode = (dw0 & 0xFF) as u8;
            self.process_command(opcode, dw0, dw1);

            let next = (self.cmdq_cons & index_mask) + 1;
            if next >= depth {
                self.cmdq_cons = (self.cmdq_cons ^ wrap_bit) & wrap_bit;
            } else {
                self.cmdq_cons = (self.cmdq_cons & wrap_bit) | next;
            }
        }
    }

    /// Dispatch a single command.
    #[allow(clippy::cast_possible_truncation)]
    fn process_command(&mut self, opcode: u8, dw0: u64, dw1: u64) {
        match opcode {
            CMD_CFGI_STE_RANGE => {
                let sid = ((dw0 >> 32) & 0xFFFF) as u32;
                let range_bits = ((dw0 >> 48) & 0x1F) as u32;
                let count = 1u32 << range_bits;
                self.tlb.flush_by_sid_range(sid, count);
                log::trace!("SMMU: CFGI_STE_RANGE sid={sid} count={count}");
            }
            CMD_CFGI_ALL => {
                self.tlb.flush_all();
                log::trace!("SMMU: CFGI_ALL — flushed all TLB entries");
            }
            CMD_TLBI_NH_ALL => {
                self.tlb.flush_all();
                log::trace!("SMMU: TLBI_NH_ALL");
            }
            CMD_TLBI_NH_ASID => {
                let asid = ((dw0 >> 48) & 0xFFFF) as u16;
                self.tlb.flush_by_asid(asid);
                log::trace!("SMMU: TLBI_NH_ASID asid={asid}");
            }
            CMD_TLBI_NH_VA => {
                let asid = ((dw0 >> 48) & 0xFFFF) as u16;
                let va = dw1 & 0xFFFF_FFFF_FFFF_F000;
                self.tlb.flush_by_va_asid(asid, va);
                log::trace!("SMMU: TLBI_NH_VA asid={asid} va={va:#x}");
            }
            CMD_SYNC => {
                log::trace!("SMMU: CMD_SYNC");
            }
            _ => {
                log::warn!("SMMU: unknown command opcode {opcode:#x}");
                self.gerror |= GERROR_CMDQ_ERR;
                self.update_irq_lines();
            }
        }
    }

    // ── IRQ management ───────────────────────────────────────────────────

    /// Update interrupt output lines based on queue state and `IRQ_CTRL`.
    pub fn update_irq_lines(&self) {
        let evtq_pending = self.evtq_prod != self.evtq_cons;
        if evtq_pending && (self.irq_ctrl & IRQ_CTRL_EVTQ_IRQEN) != 0 {
            self.evtq_irq.assert();
        } else {
            self.evtq_irq.deassert();
        }

        let gerror_pending = self.gerror != self.gerrorn;
        if gerror_pending && (self.irq_ctrl & IRQ_CTRL_GERROR_IRQEN) != 0 {
            self.gerror_irq.assert();
        } else {
            self.gerror_irq.deassert();
        }
    }
}

// ── Device trait ────────────────────────────────────────────────────────────

impl<M: ByteMem + Send + 'static> Device for SmmuState<M> {
    fn read(&mut self, offset: u64, size: usize) -> u64 {
        let _ = size;
        match offset {
            SMMU_IDR0 => u64::from(self.idr0),
            SMMU_IDR1 => u64::from(self.idr1),
            SMMU_IDR2 => u64::from(self.idr2),
            SMMU_IDR3 => u64::from(self.idr3),
            SMMU_IDR5 => u64::from(self.idr5),
            SMMU_IIDR => u64::from(self.iidr),
            SMMU_AIDR => u64::from(self.aidr),

            SMMU_CR0 => u64::from(self.cr0),
            SMMU_CR0ACK => u64::from(self.cr0ack),
            SMMU_CR1 => u64::from(self.cr1),
            SMMU_CR2 => u64::from(self.cr2),
            SMMU_STATUSR => u64::from(self.statusr),
            SMMU_GBPA => u64::from(self.gbpa),

            SMMU_IRQ_CTRL => u64::from(self.irq_ctrl),
            SMMU_IRQ_CTRLACK => u64::from(self.irq_ctrlack),
            SMMU_GERROR => u64::from(self.gerror),
            SMMU_GERRORN => u64::from(self.gerrorn),

            SMMU_STRTAB_BASE => self.strtab_base & 0xFFFF_FFFF,
            SMMU_STRTAB_BASE_HI => self.strtab_base >> 32,
            SMMU_STRTAB_BASE_CFG => u64::from(self.strtab_base_cfg),

            SMMU_CMDQ_BASE => self.cmdq_base & 0xFFFF_FFFF,
            SMMU_CMDQ_BASE_HI => self.cmdq_base >> 32,
            SMMU_CMDQ_PROD => u64::from(self.cmdq_prod),
            SMMU_CMDQ_CONS => u64::from(self.cmdq_cons),

            SMMU_EVTQ_BASE => self.evtq_base & 0xFFFF_FFFF,
            SMMU_EVTQ_BASE_HI => self.evtq_base >> 32,
            SMMU_EVTQ_PROD => u64::from(self.evtq_prod),
            SMMU_EVTQ_CONS => u64::from(self.evtq_cons),
            SMMU_EVTQ_IRQ_CFG0 => self.evtq_irq_cfg[0],
            SMMU_EVTQ_IRQ_CFG1 => self.evtq_irq_cfg[1],
            SMMU_EVTQ_IRQ_CFG2 => self.evtq_irq_cfg[2],

            _ => {
                log::trace!("SMMU: read from undefined offset {offset:#x}");
                0
            }
        }
    }

    #[allow(clippy::cast_possible_truncation)]
    fn write(&mut self, offset: u64, size: usize, val: u64) {
        let _ = size;
        match offset {
            SMMU_CR0 => {
                self.cr0 = val as u32;
                self.cr0ack = self.cr0;
                log::trace!("SMMU: CR0 = {:#x}, CR0ACK mirrored", self.cr0);
            }
            SMMU_CR1 => self.cr1 = val as u32,
            SMMU_CR2 => self.cr2 = val as u32,
            SMMU_GBPA => self.gbpa = val as u32,

            SMMU_IRQ_CTRL => {
                self.irq_ctrl = val as u32;
                self.irq_ctrlack = self.irq_ctrl;
                self.update_irq_lines();
            }
            SMMU_GERRORN => {
                self.gerrorn = val as u32;
                self.update_irq_lines();
            }

            SMMU_STRTAB_BASE => {
                self.strtab_base = (self.strtab_base & 0xFFFF_FFFF_0000_0000) | (val & 0xFFFF_FFFF);
            }
            SMMU_STRTAB_BASE_HI => {
                self.strtab_base =
                    (self.strtab_base & 0x0000_0000_FFFF_FFFF) | ((val & 0xFFFF_FFFF) << 32);
            }
            SMMU_STRTAB_BASE_CFG => {
                self.strtab_base_cfg = val as u32;
                let fmt = (val >> 16) & 0x3;
                self.strtab_fmt = if fmt == 1 {
                    StrtabFmt::TwoLevel
                } else {
                    StrtabFmt::Linear
                };
                self.strtab_log2size = (val & 0x3F) as u8;
                self.strtab_split = ((val >> 6) & 0x1F) as u8;
                log::trace!(
                    "SMMU: STRTAB_BASE_CFG fmt={:?} log2size={} split={}",
                    self.strtab_fmt,
                    self.strtab_log2size,
                    self.strtab_split
                );
            }

            SMMU_CMDQ_BASE => {
                self.cmdq_base = (self.cmdq_base & 0xFFFF_FFFF_0000_0000) | (val & 0xFFFF_FFFF);
                self.cmdq_log2size = (val & 0x1F) as u8;
            }
            SMMU_CMDQ_BASE_HI => {
                self.cmdq_base =
                    (self.cmdq_base & 0x0000_0000_FFFF_FFFF) | ((val & 0xFFFF_FFFF) << 32);
            }
            SMMU_CMDQ_PROD => {
                self.cmdq_prod = val as u32;
                self.process_cmdq();
            }
            SMMU_CMDQ_CONS => {
                self.cmdq_cons = val as u32;
            }

            SMMU_EVTQ_BASE => {
                self.evtq_base = (self.evtq_base & 0xFFFF_FFFF_0000_0000) | (val & 0xFFFF_FFFF);
                self.evtq_log2size = (val & 0x1F) as u8;
            }
            SMMU_EVTQ_BASE_HI => {
                self.evtq_base =
                    (self.evtq_base & 0x0000_0000_FFFF_FFFF) | ((val & 0xFFFF_FFFF) << 32);
            }
            SMMU_EVTQ_PROD => {
                self.evtq_prod = val as u32;
            }
            SMMU_EVTQ_CONS => {
                self.evtq_cons = val as u32;
                self.update_irq_lines();
            }
            SMMU_EVTQ_IRQ_CFG0 => self.evtq_irq_cfg[0] = val,
            SMMU_EVTQ_IRQ_CFG1 => self.evtq_irq_cfg[1] = val,
            SMMU_EVTQ_IRQ_CFG2 => self.evtq_irq_cfg[2] = val,

            _ => {
                log::trace!("SMMU: write to undefined offset {offset:#x} val={val:#x}");
            }
        }
    }

    fn region_size(&self) -> u64 {
        0x1_0000 // 64KB
    }
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::mem::TestMem;
    use helm_devices::{InterruptSink, WireId};
    use std::sync::Arc;

    struct NullSink;

    impl InterruptSink for NullSink {
        fn on_assert(&self, _wire_id: WireId) {}
        fn on_deassert(&self, _wire_id: WireId) {}
    }

    fn wire_irq_pins(smmu: &mut SmmuState<TestMem>) {
        let sink: Arc<dyn InterruptSink> = Arc::new(NullSink);
        smmu.gerror_irq.wire(0u64, Arc::clone(&sink));
        smmu.evtq_irq.wire(1u64, sink);
    }

    // ── Helper: build a minimal SMMU with stream table + page tables ────

    const STRTAB_BASE: u64 = 0x10000;
    const CD_BASE: u64 = 0x20000;
    const L1_TABLE: u64 = 0x31000;
    const L2_TABLE: u64 = 0x32000;
    const L3_TABLE: u64 = 0x33000;
    const OUTPUT_PAGE: u64 = 0x40000;
    const OUTPUT_IPA: u64 = 0x50000;
    const OUTPUT_IPA_DST: u64 = 0x60000;
    const OUTPUT_S2_PAGE: u64 = 0x90000;
    const OUTPUT_S2_DST_PAGE: u64 = 0xA0000;
    const L2_64K_TABLE: u64 = 0x50000;
    const L3_64K_TABLE: u64 = 0x60000;
    const S2_L2_64K_TABLE: u64 = 0x70000;
    const S2_L3_64K_TABLE: u64 = 0x80000;
    const CMDQ_BASE_ADDR: u64 = 0x50000;
    const EVTQ_BASE_ADDR: u64 = 0x60000;

    fn build_test_smmu() -> SmmuState<TestMem> {
        let mut mem = TestMem::new(0x0010_0000);

        let ste_dw0: u64 = 0x1 | (0b101 << 1) | (CD_BASE & 0x000F_FFFF_FFFF_FFC0);
        mem.write_u64(STRTAB_BASE, ste_dw0);
        mem.write_u64(STRTAB_BASE + 8, 0);
        mem.write_u64(STRTAB_BASE + 16, 0);
        mem.write_u64(STRTAB_BASE + 24, 0);

        let cd_dw0: u64 = (1u64 << 31) | (25u64 << 32);
        let cd_dw1: u64 = (42u64 << 48) | (L1_TABLE & 0x000F_FFFF_FFFF_F000);
        mem.write_u64(CD_BASE, cd_dw0);
        mem.write_u64(CD_BASE + 8, cd_dw1);

        mem.write_u64(L1_TABLE, L2_TABLE | 0x3);
        mem.write_u64(L2_TABLE, L3_TABLE | 0x3);
        mem.write_u64(L3_TABLE + 8, OUTPUT_PAGE | 0x3 | (0b01 << 6) | (1 << 10));

        let mut smmu = SmmuState::new(mem);
        smmu.strtab_base = STRTAB_BASE;
        smmu.strtab_log2size = 1;
        smmu.strtab_fmt = StrtabFmt::Linear;
        smmu.cmdq_base = CMDQ_BASE_ADDR | 2;
        smmu.cmdq_log2size = 2;
        smmu.evtq_base = EVTQ_BASE_ADDR | 2;
        smmu.evtq_log2size = 2;
        smmu.cr0 = 0x7;
        smmu.cr0ack = smmu.cr0;
        smmu
    }

    fn build_test_smmu_s2_4k() -> SmmuState<TestMem> {
        let mut mem = TestMem::new(0x0010_0000);

        let ste_dw0: u64 = 0x1 | (0b110 << 1);
        let ste_dw2: u64 = (0x19_u64 & 0x3F) | (L1_TABLE & 0x000F_FFFF_FFFF_FFF0);
        mem.write_u64(STRTAB_BASE, ste_dw0);
        mem.write_u64(STRTAB_BASE + 8, 0);
        mem.write_u64(STRTAB_BASE + 16, ste_dw2);
        mem.write_u64(STRTAB_BASE + 24, 0);

        mem.write_u64(L1_TABLE, L2_TABLE | 0x3);
        mem.write_u64(L2_TABLE, L3_TABLE | 0x3);
        mem.write_u64(L3_TABLE + 8, OUTPUT_PAGE | 0x3 | (0b01 << 6) | (1 << 10));

        let mut smmu = SmmuState::new(mem);
        smmu.strtab_base = STRTAB_BASE;
        smmu.strtab_log2size = 1;
        smmu.strtab_fmt = StrtabFmt::Linear;
        smmu.cmdq_base = CMDQ_BASE_ADDR | 2;
        smmu.cmdq_log2size = 2;
        smmu.evtq_base = EVTQ_BASE_ADDR | 2;
        smmu.evtq_log2size = 2;
        smmu.cr0 = 0x7;
        smmu.cr0ack = smmu.cr0;
        smmu
    }

    fn build_test_smmu_s1s2_64k() -> SmmuState<TestMem> {
        let mut mem = TestMem::new(0x0020_0000);

        let ste_dw0: u64 = 0x1 | (0b111 << 1) | (CD_BASE & 0x000F_FFFF_FFFF_FFC0);
        let ste_dw2: u64 =
            (0x1c_u64 & 0x3F) | ((0x1_u64 & 0x3) << 56) | (S2_L2_64K_TABLE & 0x000F_FFFF_FFFF_FFF0);
        mem.write_u64(STRTAB_BASE, ste_dw0);
        mem.write_u64(STRTAB_BASE + 8, 0);
        mem.write_u64(STRTAB_BASE + 16, ste_dw2);
        mem.write_u64(STRTAB_BASE + 24, 0);

        let cd_dw0: u64 = (1u64 << 31) | (28u64 << 32) | (1u64 << 46);
        let cd_dw1: u64 = (42u64 << 48) | (L2_64K_TABLE & 0x0000_FFFF_FFFF_0000);
        mem.write_u64(CD_BASE, cd_dw0);
        mem.write_u64(CD_BASE + 8, cd_dw1);

        mem.write_u64(L2_64K_TABLE, L3_64K_TABLE | 0x3);
        mem.write_u64(
            L3_64K_TABLE + 5 * 8,
            OUTPUT_IPA | 0x3 | (0b01 << 6) | (1 << 10),
        );
        mem.write_u64(
            L3_64K_TABLE + 6 * 8,
            OUTPUT_IPA_DST | 0x3 | (0b01 << 6) | (1 << 10),
        );

        mem.write_u64(S2_L2_64K_TABLE, S2_L3_64K_TABLE | 0x3);
        mem.write_u64(
            S2_L3_64K_TABLE + 5 * 8,
            OUTPUT_S2_PAGE | 0x3 | (0b01 << 6) | (1 << 10),
        );
        mem.write_u64(
            S2_L3_64K_TABLE + 6 * 8,
            OUTPUT_S2_DST_PAGE | 0x3 | (0b01 << 6) | (1 << 10),
        );

        let mut smmu = SmmuState::new(mem);
        smmu.strtab_base = STRTAB_BASE;
        smmu.strtab_log2size = 1;
        smmu.strtab_fmt = StrtabFmt::Linear;
        smmu.cmdq_base = CMDQ_BASE_ADDR | 2;
        smmu.cmdq_log2size = 2;
        smmu.evtq_base = EVTQ_BASE_ADDR | 2;
        smmu.evtq_log2size = 2;
        smmu.cr0 = 0x7;
        smmu.cr0ack = smmu.cr0;
        smmu
    }

    // ── Device trait tests ───────────────────────────────────────────────

    #[test]
    fn device_region_size_is_64kb() {
        let smmu = SmmuState::new(TestMem::new(4096));
        assert_eq!(smmu.region_size(), 0x1_0000);
    }

    #[test]
    fn device_read_idr0() {
        let mut smmu = SmmuState::new(TestMem::new(4096));
        assert_eq!(smmu.read(0x0000, 4), 0x0000_0001);
    }

    #[test]
    fn device_read_aidr() {
        let mut smmu = SmmuState::new(TestMem::new(4096));
        assert_eq!(smmu.read(0x001C, 4), 0x01);
    }

    #[test]
    fn device_read_iidr() {
        let mut smmu = SmmuState::new(TestMem::new(4096));
        assert_eq!(smmu.read(0x0018, 4), u64::from(0x4845_4C4Du32));
    }

    #[test]
    fn cr0_mirrors_to_cr0ack() {
        let mut smmu = SmmuState::new(TestMem::new(4096));
        smmu.write(0x0020, 4, 0x7);
        assert_eq!(smmu.read(0x0024, 4), 0x7);
    }

    #[test]
    fn irq_ctrl_mirrors_to_ack() {
        let mut smmu = SmmuState::new(TestMem::new(4096));
        smmu.write(0x0060, 4, 0x5);
        assert_eq!(smmu.read(0x0064, 4), 0x5);
    }

    #[test]
    fn undefined_offset_returns_zero() {
        let mut smmu = SmmuState::new(TestMem::new(4096));
        assert_eq!(smmu.read(0xFFFC, 4), 0);
    }

    #[test]
    fn strtab_base_cfg_parses_fields() {
        let mut smmu = SmmuState::new(TestMem::new(4096));
        smmu.write(0x0088, 4, 8 | (6 << 6) | (1 << 16));
        assert_eq!(smmu.strtab_log2size, 8);
        assert_eq!(smmu.strtab_split, 6);
        assert_eq!(smmu.strtab_fmt, StrtabFmt::TwoLevel);
    }

    // ── STE lookup ───────────────────────────────────────────────────────

    #[test]
    fn lookup_ste_valid_s1only() {
        let mut smmu = build_test_smmu();
        let ste = smmu.lookup_ste(0).unwrap();
        assert!(ste.valid);
        assert_eq!(ste.config, SteConfig::S1Only);
        assert_eq!(ste.s1_context_ptr, CD_BASE);
    }

    #[test]
    fn lookup_ste_bad_stream_id() {
        let mut smmu = build_test_smmu();
        let err = smmu.lookup_ste(99).unwrap_err();
        assert_eq!(err.code, SmmuFaultCode::BadStreamId);
    }

    #[test]
    fn lookup_ste_two_level_rejected_until_implemented() {
        let mut smmu = build_test_smmu();
        smmu.strtab_fmt = StrtabFmt::TwoLevel;
        let err = smmu.lookup_ste(0).unwrap_err();
        assert_eq!(err.code, SmmuFaultCode::SteFetch);
        assert_eq!(err.input_addr, STRTAB_BASE);
    }

    // ── CD lookup ────────────────────────────────────────────────────────

    #[test]
    fn lookup_cd_valid() {
        let mut smmu = build_test_smmu();
        let ste = smmu.lookup_ste(0).unwrap();
        let cd = smmu.lookup_cd(&ste, 0).unwrap();
        assert!(cd.valid);
        assert_eq!(cd.t0sz, 25);
        assert_eq!(cd.tg0, 0);
        assert_eq!(cd.asid, 42);
        assert_eq!(cd.ttb0, L1_TABLE);
    }

    // ── S1 walk ──────────────────────────────────────────────────────────

    #[test]
    fn walk_s1_valid_mapping() {
        let mut smmu = build_test_smmu();
        let ste = smmu.lookup_ste(0).unwrap();
        let cd = smmu.lookup_cd(&ste, 0).unwrap();
        let (pa, size, prot) = smmu.walk_s1(&cd, 0x1000, false, 0).unwrap();
        assert_eq!(pa, OUTPUT_PAGE);
        assert_eq!(size, 0x1000);
        assert!(prot & 0x1 != 0);
        assert!(prot & 0x2 != 0);
    }

    #[test]
    fn walk_s1_unmapped_faults() {
        let mut smmu = build_test_smmu();
        let ste = smmu.lookup_ste(0).unwrap();
        let cd = smmu.lookup_cd(&ste, 0).unwrap();
        let err = smmu.walk_s1(&cd, 0x2000, false, 0).unwrap_err();
        assert_eq!(err.code, SmmuFaultCode::Translation);
    }

    // ── translate() ──────────────────────────────────────────────────────

    #[test]
    fn translate_s1_read() {
        let mut smmu = build_test_smmu();
        match smmu.translate(0, 0x1000, false) {
            SmmuTranslateResult::Ok(pa) => assert_eq!(pa, OUTPUT_PAGE),
            other => panic!("expected Ok, got {other:?}"),
        }
    }

    #[test]
    fn translate_s1_write() {
        let mut smmu = build_test_smmu();
        match smmu.translate(0, 0x1000, true) {
            SmmuTranslateResult::Ok(pa) => assert_eq!(pa, OUTPUT_PAGE),
            other => panic!("expected Ok, got {other:?}"),
        }
    }

    #[test]
    fn translate_tlb_hit() {
        let mut smmu = build_test_smmu();
        let _ = smmu.translate(0, 0x1000, false);
        match smmu.translate(0, 0x1000, false) {
            SmmuTranslateResult::Ok(pa) => assert_eq!(pa, OUTPUT_PAGE),
            other => panic!("expected TLB hit, got {other:?}"),
        }
    }

    #[test]
    fn translate_tlb_miss_when_asid_changes() {
        let mut smmu = build_test_smmu();
        match smmu.translate(0, 0x1000, false) {
            SmmuTranslateResult::Ok(pa) => assert_eq!(pa, OUTPUT_PAGE),
            other => panic!("expected initial Ok, got {other:?}"),
        }
        assert!(smmu.tlb.lookup(0, 42, 0x1000).is_some());

        // Change the CD's ASID from 42 to 77 and point at a different output page.
        let new_cd_dw1: u64 = (77u64 << 48) | (L1_TABLE & 0x000F_FFFF_FFFF_F000);
        smmu.mem.write_u64(CD_BASE + 8, new_cd_dw1);
        smmu.mem
            .write_u64(L3_TABLE + 8, OUTPUT_S2_PAGE | 0x3 | (0b01 << 6) | (1 << 10));

        // translate() reads the new ASID=77 from the CD, misses the TLB
        // (no entry for ASID 77), and performs a fresh page-table walk.
        match smmu.translate(0, 0x1000, false) {
            SmmuTranslateResult::Ok(pa) => assert_eq!(pa, OUTPUT_S2_PAGE),
            other => panic!("expected ASID-driven miss and rewalk, got {other:?}"),
        }
        // The stale ASID=42 entry may or may not still be present (it
        // lives in a different index slot now that ASID is part of the
        // hash). What matters is that translate() used the new ASID.
        assert!(smmu.tlb.lookup(0, 77, 0x1000).is_some());
    }

    #[test]
    fn translate_bypass_when_disabled() {
        let mut smmu = build_test_smmu();
        smmu.cr0 = 0;
        assert!(matches!(
            smmu.translate(0, 0x1000, false),
            SmmuTranslateResult::Bypass
        ));
    }

    #[test]
    fn translate_abort_disabled_gbpa() {
        let mut smmu = build_test_smmu();
        smmu.cr0 = 0;
        smmu.gbpa = 1 << 20;
        match smmu.translate(0, 0x1234, false) {
            SmmuTranslateResult::Fault(f) => assert_eq!(f.code, SmmuFaultCode::StreamDisabled),
            other => panic!("expected StreamDisabled, got {other:?}"),
        }
    }

    #[test]
    fn translate_bypass_ste() {
        let mut smmu = build_test_smmu();
        smmu.mem.write_u64(STRTAB_BASE, 0x1 | (0b100 << 1));
        smmu.tlb.flush_all();
        assert!(matches!(
            smmu.translate(0, 0x9999, false),
            SmmuTranslateResult::Bypass
        ));
    }

    #[test]
    fn translate_abort_ste() {
        let mut smmu = build_test_smmu();
        smmu.mem.write_u64(STRTAB_BASE, 0x1);
        smmu.tlb.flush_all();
        match smmu.translate(0, 0x1000, false) {
            SmmuTranslateResult::Fault(f) => assert_eq!(f.code, SmmuFaultCode::BadSte),
            other => panic!("expected BadSte, got {other:?}"),
        }
    }

    #[test]
    fn translate_invalid_ste() {
        let mut smmu = build_test_smmu();
        smmu.mem.write_u64(STRTAB_BASE, 0);
        smmu.tlb.flush_all();
        match smmu.translate(0, 0x1000, false) {
            SmmuTranslateResult::Fault(f) => assert_eq!(f.code, SmmuFaultCode::BadSte),
            other => panic!("expected BadSte, got {other:?}"),
        }
    }

    #[test]
    fn translate_s2_only_read() {
        let mut smmu = build_test_smmu_s2_4k();
        match smmu.translate(0, 0x1000, false) {
            SmmuTranslateResult::Ok(pa) => assert_eq!(pa, OUTPUT_PAGE),
            other => panic!("expected S2 Ok, got {other:?}"),
        }
    }

    #[test]
    fn translate_s1s2_64k_read() {
        let mut smmu = build_test_smmu_s1s2_64k();
        match smmu.translate(0, 0x50000, false) {
            SmmuTranslateResult::Ok(pa) => assert_eq!(pa, OUTPUT_S2_PAGE),
            other => panic!("expected S1S2 Ok, got {other:?}"),
        }
    }

    #[test]
    fn dma_copy_s1s2_64k_round_trip() {
        let mut smmu = build_test_smmu_s1s2_64k();
        smmu.mem
            .write_bytes(OUTPUT_S2_PAGE, b"payload")
            .expect("write source payload");
        smmu.dma_copy(0, 0x50000, 0x60000, 7)
            .expect("S1S2 DMA read/write should succeed");
        let mut buf = [0u8; 7];
        smmu.mem
            .read_bytes(OUTPUT_S2_DST_PAGE, &mut buf)
            .expect("read destination payload");
        assert_eq!(&buf, b"payload");
    }

    #[test]
    fn dma_write_unmapped_faults() {
        let mut smmu = build_test_smmu_s2_4k();
        let err = smmu.dma_write(0, 0x2000, b"x").unwrap_err();
        assert_eq!(err.code, SmmuFaultCode::Translation);
    }

    // ── Command queue ────────────────────────────────────────────────────

    #[test]
    fn cmdq_cmd_sync() {
        let mut smmu = build_test_smmu();
        smmu.mem.write_u64(CMDQ_BASE_ADDR, 0x46);
        smmu.mem.write_u64(CMDQ_BASE_ADDR + 8, 0);
        smmu.cmdq_prod = 1;
        smmu.process_cmdq();
        assert_eq!(smmu.cmdq_cons, 1);
    }

    #[test]
    fn cmdq_tlbi_nh_all() {
        let mut smmu = build_test_smmu();
        let _ = smmu.translate(0, 0x1000, false);
        assert!(smmu.tlb.lookup(0, 42, 0x1000).is_some());
        smmu.mem.write_u64(CMDQ_BASE_ADDR, 0x20);
        smmu.mem.write_u64(CMDQ_BASE_ADDR + 8, 0);
        smmu.cmdq_prod = 1;
        smmu.process_cmdq();
        assert!(smmu.tlb.lookup(0, 42, 0x1000).is_none());
    }

    #[test]
    fn cmdq_multiple() {
        let mut smmu = build_test_smmu();
        smmu.mem.write_u64(CMDQ_BASE_ADDR, 0x20);
        smmu.mem.write_u64(CMDQ_BASE_ADDR + 8, 0);
        smmu.mem.write_u64(CMDQ_BASE_ADDR + 16, 0x46);
        smmu.mem.write_u64(CMDQ_BASE_ADDR + 24, 0);
        smmu.cmdq_prod = 2;
        smmu.process_cmdq();
        assert_eq!(smmu.cmdq_cons, 2);
    }

    #[test]
    fn cmdq_wrap() {
        let mut smmu = build_test_smmu();
        for i in 0..4u64 {
            smmu.mem.write_u64(CMDQ_BASE_ADDR + i * 16, 0x46);
            smmu.mem.write_u64(CMDQ_BASE_ADDR + i * 16 + 8, 0);
        }
        smmu.cmdq_prod = 4;
        smmu.process_cmdq();
        assert_eq!(smmu.cmdq_cons, smmu.cmdq_prod);
    }

    #[test]
    fn cmdq_disabled() {
        let mut smmu = build_test_smmu();
        smmu.cr0 &= !0x2;
        smmu.cmdq_prod = 1;
        smmu.process_cmdq();
        assert_eq!(smmu.cmdq_cons, 0);
    }

    #[test]
    fn cmdq_fetch_fault_sets_gerror_and_stops_drain() {
        let mut smmu = build_test_smmu();
        // Point the command queue at memory beyond the TestMem backing store.
        smmu.cmdq_base = 0x0010_0000 | 2;
        smmu.cmdq_prod = 1;
        smmu.process_cmdq();
        // Consumer must NOT advance — the command was never decoded.
        assert_eq!(smmu.cmdq_cons, 0);
        // GERROR.CMDQ_ERR must be set.
        assert_eq!(smmu.gerror & GERROR_CMDQ_ERR, GERROR_CMDQ_ERR);
        assert_eq!(smmu.read(SMMU_GERROR, 4), u64::from(GERROR_CMDQ_ERR));
    }

    #[test]
    fn cmdq_fetch_fault_asserts_gerror_irq_when_enabled() {
        let mut smmu = build_test_smmu();
        wire_irq_pins(&mut smmu);
        smmu.irq_ctrl = IRQ_CTRL_GERROR_IRQEN;
        smmu.cmdq_base = 0x0010_0000 | 2;
        smmu.cmdq_prod = 1;
        smmu.process_cmdq();
        assert_eq!(smmu.gerror & GERROR_CMDQ_ERR, GERROR_CMDQ_ERR);
        assert!(
            smmu.gerror_irq.is_asserted(),
            "GERROR IRQ pin must be asserted when GERROR_IRQEN is set"
        );
        smmu.gerrorn = smmu.gerror;
        smmu.update_irq_lines();
        assert!(
            !smmu.gerror_irq.is_asserted(),
            "GERROR IRQ pin must deassert after acknowledgment"
        );
    }

    #[test]
    fn cmdq_fetch_fault_no_irq_when_gerror_irq_disabled() {
        let mut smmu = build_test_smmu();
        wire_irq_pins(&mut smmu);
        smmu.irq_ctrl = 0;
        smmu.cmdq_base = 0x0010_0000 | 2;
        smmu.cmdq_prod = 1;
        smmu.process_cmdq();
        assert_eq!(smmu.gerror & GERROR_CMDQ_ERR, GERROR_CMDQ_ERR);
        assert!(
            !smmu.gerror_irq.is_asserted(),
            "GERROR IRQ pin must stay deasserted when GERROR_IRQEN is clear"
        );
    }

    // ── Two-level stream table rejection ─────────────────────────────────

    #[test]
    fn two_level_strtab_translate_records_event_and_faults() {
        let mut smmu = build_test_smmu();
        smmu.strtab_fmt = StrtabFmt::TwoLevel;
        smmu.tlb.flush_all();
        match smmu.translate(0, 0x1000, false) {
            SmmuTranslateResult::Fault(f) => {
                assert_eq!(f.code, SmmuFaultCode::SteFetch);
                assert_eq!(f.stream_id, 0);
            }
            other => panic!("expected SteFetch fault for two-level strtab, got {other:?}"),
        }
        assert_ne!(
            smmu.evtq_prod, 0,
            "event queue should record the SteFetch fault"
        );
        let dw0 = smmu.mem.read_le_u64(EVTQ_BASE_ADDR, 8).unwrap();
        assert_eq!(
            (dw0 & 0xFF) as u8,
            SmmuFaultCode::SteFetch as u8,
            "event record must carry SteFetch code"
        );
    }

    // ── Event queue ──────────────────────────────────────────────────────

    #[test]
    fn evtq_records_fault() {
        let mut smmu = build_test_smmu();
        smmu.mem.write_u64(STRTAB_BASE, 0);
        smmu.tlb.flush_all();
        let _ = smmu.translate(0, 0xBEEF, true);
        assert_ne!(smmu.evtq_prod, 0);
        let dw0 = smmu.mem.read_le_u64(EVTQ_BASE_ADDR, 8).unwrap();
        assert_eq!((dw0 & 0xFF) as u8, SmmuFaultCode::BadSte as u8);
    }

    #[test]
    fn evtq_disabled_no_record() {
        let mut smmu = build_test_smmu();
        smmu.cr0 &= !0x4;
        smmu.mem.write_u64(STRTAB_BASE, 0);
        smmu.tlb.flush_all();
        let _ = smmu.translate(0, 0xBEEF, false);
        assert_eq!(smmu.evtq_prod, 0);
    }

    // ── Fault record format ──────────────────────────────────────────────

    #[test]
    fn event_record_format() {
        let fault = SmmuFault {
            code: SmmuFaultCode::Translation,
            stream_id: 7,
            input_addr: 0xDEAD_BEEF,
            is_write: true,
        };
        let record = build_event_record(&fault);
        let dw0 = u64::from_le_bytes(record[0..8].try_into().unwrap());
        assert_eq!(dw0 & 0xFF, 0x10);
        assert_eq!((dw0 >> 32) & 0xFFFF_FFFF, 7);
        let dw1 = u64::from_le_bytes(record[8..16].try_into().unwrap());
        assert_eq!(dw1, 0xDEAD_BEEF);
        let dw2 = u64::from_le_bytes(record[16..24].try_into().unwrap());
        assert_eq!(dw2 & 0x2, 0x2);
    }

    // ── Device::write CMDQ_PROD triggers drain ──────────────────────────

    #[test]
    fn device_cmdq_prod_write_drains() {
        let mut smmu = build_test_smmu();
        smmu.mem.write_u64(CMDQ_BASE_ADDR, 0x46);
        smmu.mem.write_u64(CMDQ_BASE_ADDR + 8, 0);
        smmu.write(0x0098, 4, 1);
        assert_eq!(smmu.cmdq_cons, 1);
    }

    // ── TransactionAttrs ─────────────────────────────────────────────────

    #[test]
    fn transaction_attrs_stream_id_default_none() {
        let attrs = helm_devices::TransactionAttrs::default();
        assert!(attrs.stream_id.is_none());
        assert!(attrs.sub_stream_id.is_none());
    }
}
