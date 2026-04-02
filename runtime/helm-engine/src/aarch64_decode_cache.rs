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
    pub(crate) fn with_pc(mut self, pc: u64) -> Self {
        self.insn.pc = pc;
        self
    }

    #[inline(always)]
    pub(crate) fn predict_branch(&self, pc: u64, target: u64) -> bool {
        self.predictor.predict(pc, target)
    }
}

const DECODE_CACHE_ENTRIES: usize = 4096;
const DECODE_CACHE_MASK: u64 = (DECODE_CACHE_ENTRIES as u64) - 1;

#[derive(Clone, Copy)]
struct Aarch64DecodeCacheEntry {
    key: u64,
    raw: u32,
    decoded: DecodedAarch64Insn,
}

pub(crate) struct Aarch64DecodeCache {
    entries: Box<[Option<Aarch64DecodeCacheEntry>; DECODE_CACHE_ENTRIES]>,
}

impl Aarch64DecodeCache {
    pub(crate) fn new() -> Self {
        Self {
            entries: Box::new([None; DECODE_CACHE_ENTRIES]),
        }
    }

    #[inline(always)]
    fn idx(key: u64) -> usize {
        ((key >> 2) & DECODE_CACHE_MASK) as usize
    }

    #[inline(always)]
    pub(crate) fn lookup(&self, key: u64, pc: u64, raw: u32) -> Option<DecodedAarch64Insn> {
        self.entries[Self::idx(key)].and_then(|entry| {
            (entry.key == key && entry.raw == raw).then_some(entry.decoded.with_pc(pc))
        })
    }

    #[inline(always)]
    pub(crate) fn insert(&mut self, key: u64, decoded: DecodedAarch64Insn) {
        self.entries[Self::idx(key)] = Some(Aarch64DecodeCacheEntry {
            key,
            raw: decoded.insn.raw,
            decoded,
        });
    }

    #[inline(always)]
    pub(crate) fn invalidate_range(&mut self, key: u64, size: usize) {
        let start = key & !0x3;
        let end = key.saturating_add(size.saturating_sub(1) as u64);
        let mut cur = start;
        while cur <= end {
            self.entries[Self::idx(cur)] = None;
            cur = cur.saturating_add(4);
        }
    }
}
