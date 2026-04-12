use helm_arch::aarch64::insn::Instruction;
use helm_arch::{aarch64_decode, DecodeError};
use helm_plugin::runtime::{BranchKind, InsnClass};
use helm_probe::{BranchKind as ProbeBranchKind, InsnClass as ProbeInsnClass};
use helm_timing::TimingInsnClass;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BranchPredictor {
    AlwaysTaken,
    BackwardConditional,
}

impl BranchPredictor {
    #[inline(always)]
    fn predict(self, pc: u64, target: u64) -> bool {
        match self {
            Self::AlwaysTaken => true,
            Self::BackwardConditional => target <= pc,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct DecodedAarch64Insn {
    pub(crate) insn: Instruction,
    pub(crate) class: InsnClass,
    pub(crate) timing_class: TimingInsnClass,
    pub(crate) probe_class: ProbeInsnClass,
    pub(crate) opcode_name: &'static str,
    pub(crate) is_stub: bool,
    pub(crate) is_branch: bool,
    pub(crate) records_mem_access: bool,
    pub(crate) timing_is_load: bool,
    pub(crate) timing_is_store: bool,
    pub(crate) timing_is_fp: bool,
    /// True when the instruction accesses FP/SIMD registers.
    pub(crate) is_fp_simd: bool,
    pub(crate) probe_branch_kind: ProbeBranchKind,
    pub(crate) plugin_branch_kind: BranchKind,
    predictor: BranchPredictor,
}

impl DecodedAarch64Insn {
    pub(crate) fn decode(raw: u32, pc: u64) -> Result<Self, DecodeError> {
        let insn = aarch64_decode(raw, pc)?;
        Ok(Self::from_instruction(insn))
    }

    fn from_instruction(insn: Instruction) -> Self {
        let (class, opcode_name, is_stub) = crate::classify_aarch64_opcode(insn.opcode);
        let timing_class = crate::to_timing_class(class);
        let probe_class = crate::to_probe_class(class);
        let is_branch = insn.is_branch();
        let predictor = if matches!(
            insn.opcode,
            helm_arch::aarch64::insn::Opcode::BCond
                | helm_arch::aarch64::insn::Opcode::Cbz
                | helm_arch::aarch64::insn::Opcode::Cbnz
                | helm_arch::aarch64::insn::Opcode::Tbz
                | helm_arch::aarch64::insn::Opcode::Tbnz
        ) {
            BranchPredictor::BackwardConditional
        } else {
            BranchPredictor::AlwaysTaken
        };

        Self {
            insn,
            class,
            timing_class,
            probe_class,
            opcode_name,
            is_stub,
            is_branch,
            records_mem_access: matches!(
                timing_class,
                TimingInsnClass::Load | TimingInsnClass::Store | TimingInsnClass::Atomic
            ),
            timing_is_load: matches!(timing_class, TimingInsnClass::Load),
            timing_is_store: matches!(timing_class, TimingInsnClass::Store),
            // FP/SIMD flag for CPTR_EL2.TFP trapping.  Compute opcodes
            // are caught by the timing-class fallback; FP/SIMD load/store
            // opcodes need explicit listing (timing class = Load/Store).
            is_fp_simd: {
                use helm_arch::aarch64::insn::Opcode::*;
                matches!(
                    insn.opcode,
                    LdrSimd | StrSimd | LdpSimd | StpSimd | LdurSimd | SturSimd
                        | SimdLd1 | SimdSt1 | SimdLd2 | SimdSt2
                        | SimdLd3 | SimdSt3 | SimdLd4 | SimdSt4
                        | SimdLd1r | FmovGpr
                ) || matches!(
                    timing_class,
                    TimingInsnClass::FpAlu | TimingInsnClass::SimdAlu
                )
            },
            timing_is_fp: matches!(
                timing_class,
                TimingInsnClass::FpAlu | TimingInsnClass::SimdAlu
            ),
            probe_branch_kind: crate::probe_branch_kind(insn.opcode),
            plugin_branch_kind: crate::classify_branch_kind(insn.opcode),
            predictor,
        }
    }

    #[inline(always)]
    pub(crate) fn predict_branch(&self, pc: u64, target: u64) -> bool {
        self.predictor.predict(pc, target)
    }
}

/// Number of sets (must be power of 2).
const DECODE_CACHE_SETS: usize = 2048;
const DECODE_CACHE_SET_MASK: u64 = (DECODE_CACHE_SETS as u64) - 1;
/// 2-way set associativity.
const DECODE_CACHE_WAYS: usize = 2;

#[derive(Clone, Copy)]
struct Aarch64DecodeCacheEntry {
    key: u64,
    raw: u32,
    decoded: DecodedAarch64Insn,
}

/// 2-way set-associative decode cache (2048 sets x 2 ways = 4096 effective entries).
///
/// On collision within a set, the entry in way 0 is evicted (simple round-robin:
/// new entries always go to way 0, existing way 0 moves to way 1).
pub(crate) struct Aarch64DecodeCache {
    entries: Box<[Option<Aarch64DecodeCacheEntry>; DECODE_CACHE_SETS * DECODE_CACHE_WAYS]>,
}

impl Aarch64DecodeCache {
    pub(crate) fn new() -> Self {
        Self {
            entries: Box::new([None; DECODE_CACHE_SETS * DECODE_CACHE_WAYS]),
        }
    }

    #[inline(always)]
    fn set_base(key: u64) -> usize {
        (((key >> 2) & DECODE_CACHE_SET_MASK) as usize) * DECODE_CACHE_WAYS
    }

    #[inline(always)]
    pub(crate) fn lookup(&self, key: u64, raw: u32) -> Option<DecodedAarch64Insn> {
        let base = Self::set_base(key);
        for way in 0..DECODE_CACHE_WAYS {
            if let Some(entry) = &self.entries[base + way] {
                if entry.key == key && entry.raw == raw {
                    return Some(entry.decoded);
                }
            }
        }
        None
    }

    #[inline(always)]
    pub(crate) fn insert(&mut self, key: u64, decoded: DecodedAarch64Insn) {
        let base = Self::set_base(key);
        // Check if already present in either way
        for way in 0..DECODE_CACHE_WAYS {
            if let Some(entry) = &self.entries[base + way] {
                if entry.key == key {
                    self.entries[base + way] = Some(Aarch64DecodeCacheEntry {
                        key,
                        raw: decoded.insn.raw,
                        decoded,
                    });
                    return;
                }
            }
        }
        // Not present: evict way 1, move way 0 → way 1, insert at way 0
        self.entries[base + 1] = self.entries[base];
        self.entries[base] = Some(Aarch64DecodeCacheEntry {
            key,
            raw: decoded.insn.raw,
            decoded,
        });
    }

    /// Flush all entries (used for cross-vCPU broadcast after code patching).
    pub(crate) fn flush(&mut self) {
        self.entries.fill(None);
    }

    #[inline(always)]
    pub(crate) fn invalidate_range(&mut self, key: u64, size: usize) {
        let start = key & !0x3;
        let end = key.saturating_add(size.saturating_sub(1) as u64);
        let mut cur = start;
        while cur <= end {
            let base = Self::set_base(cur);
            for way in 0..DECODE_CACHE_WAYS {
                if let Some(entry) = &self.entries[base + way] {
                    if entry.key == cur {
                        self.entries[base + way] = None;
                    }
                }
            }
            cur = cur.saturating_add(4);
        }
    }
}
