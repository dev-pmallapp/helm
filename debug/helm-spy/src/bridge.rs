//! ProbePluginBridge — connects helm-probe events to HelmSpy analysis primitives.
//!
//! This module provides the wiring layer between Layer 1 (helm-probe) raw events
//! and Layer 2 (helm-spy) analysis primitives. The bridge subscribes to CpuProbes
//! and translates raw probe events into enriched HelmSpy events (InsnInfo,
//! BranchInfo, MemInfo).

use std::sync::Arc;

use helm_probe::{BranchEvent, CpuProbes, CpuStepEvent, MemAccessEvent};

use crate::events::{BranchInfo, BranchKind, InsnInfo, MemInfo};
use crate::session::HelmSpy;

/// Bridge that connects CpuProbes to a HelmSpy session.
///
/// Call [`ProbePluginBridge::wire`] to subscribe HelmSpy primitives to probe
/// events. This replaces the deprecated `HelmPluginRegistry` callback system
/// with the zero-cost probe architecture.
pub struct ProbePluginBridge;

impl ProbePluginBridge {
    /// Wire a HelmSpy session to CpuProbes.
    ///
    /// This subscribes all active primitives in the session to the appropriate
    /// probe events:
    /// - `pre_step` → InsnInfo events → insn_count, insn_mix, hot_pcs
    /// - `mem` → MemInfo events → cache_l1d
    /// - `branch` → BranchInfo events → branch_heatmap, branch_pred
    #[cfg(debug_assertions)]
    pub fn wire(session: &HelmSpy, probes: &mut CpuProbes) {
        // Wire instruction tracking (pre_step probe → insn_count, insn_mix, hot_pcs)
        Self::wire_insn_tracking(session, probes);

        // Wire memory tracking (mem probe → cache_l1d)
        Self::wire_mem_tracking(session, probes);

        // Wire branch tracking (branch probe → branch_heatmap, branch_pred)
        Self::wire_branch_tracking(session, probes);
    }

    /// Wire instruction tracking: pre_step → insn_count, insn_mix, hot_pcs.
    #[cfg(debug_assertions)]
    fn wire_insn_tracking(session: &HelmSpy, probes: &mut CpuProbes) {
        session.insn_count.subscribe_to_steps(probes);
        session.insn_mix.subscribe_to_steps(probes);
        session.hot_pcs.subscribe_to_steps(probes);
    }

    /// Wire memory tracking: mem probe → cache model.
    #[cfg(debug_assertions)]
    fn wire_mem_tracking(session: &HelmSpy, probes: &mut CpuProbes) {
        if let Some(ref cache) = session.cache_l1d {
            cache.subscribe_to_mem(probes);
        }
    }

    /// Wire branch tracking: branch probe → branch_heatmap, branch predictor.
    #[cfg(debug_assertions)]
    fn wire_branch_tracking(session: &HelmSpy, probes: &mut CpuProbes) {
        session.branch_heatmap.subscribe_to_branches(probes);
        if let Some(ref pred) = session.branch_pred {
            crate::analysis::BranchPredictor::subscribe_shared(pred, probes);
        }
    }

    /// Convert a raw probe CpuStepEvent to an enriched InsnInfo.
    pub fn step_to_insn_info(event: &CpuStepEvent, insn_count: u64) -> InsnInfo {
        InsnInfo {
            vcpu_idx: 0,
            pc: event.pc,
            raw: event.raw,
            size: 4,
            class: event.insn_class,
            opcode_name: "",
            is_stub: event.is_stub,
            context: crate::events::ArchContext::None,
            insn_count,
        }
    }

    /// Convert a raw probe MemAccessEvent to a MemInfo.
    pub fn mem_to_mem_info(event: &MemAccessEvent) -> MemInfo {
        MemInfo {
            vaddr: event.addr,
            size: event.size as u8,
            is_store: event.is_store,
            is_atomic: false,
            pc: event.pc,
        }
    }

    /// Convert a raw probe BranchEvent to a BranchInfo.
    pub fn branch_to_branch_info(event: &BranchEvent, insn_count: u64) -> BranchInfo {
        BranchInfo {
            pc: event.pc,
            target: event.target,
            taken: event.taken,
            kind: Self::convert_branch_kind(event.kind),
            insn_count,
        }
    }

    /// Convert helm-probe BranchKind to helm-spy BranchKind.
    fn convert_branch_kind(kind: helm_probe::BranchKind) -> BranchKind {
        match kind {
            helm_probe::BranchKind::DirectCond => BranchKind::DirectCond,
            helm_probe::BranchKind::DirectUncond => BranchKind::DirectUncond,
            helm_probe::BranchKind::Call => BranchKind::Call,
            helm_probe::BranchKind::Return => BranchKind::Return,
            helm_probe::BranchKind::IndirectJump => BranchKind::IndirectJump,
            helm_probe::BranchKind::IndirectCall => BranchKind::IndirectCall,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use helm_probe::InsnClass;

    #[test]
    fn step_to_insn_info_conversion() {
        let event = CpuStepEvent {
            pc: 0x8000_0000,
            raw: 0xD503_201F, // NOP
            insn_class: InsnClass::Nop,
            is_stub: false,
        };
        let info = ProbePluginBridge::step_to_insn_info(&event, 42);
        assert_eq!(info.pc, 0x8000_0000);
        assert_eq!(info.insn_count, 42);
        assert_eq!(info.class, InsnClass::Nop);
    }

    #[test]
    fn mem_to_mem_info_conversion() {
        let event = MemAccessEvent {
            addr: 0x4000_0000,
            size: 8,
            is_store: true,
            pc: 0x1000,
        };
        let info = ProbePluginBridge::mem_to_mem_info(&event);
        assert_eq!(info.vaddr, 0x4000_0000);
        assert!(info.is_store);
        assert_eq!(info.size, 8);
    }

    #[test]
    fn branch_kind_conversion() {
        assert_eq!(
            ProbePluginBridge::convert_branch_kind(helm_probe::BranchKind::Call),
            BranchKind::Call
        );
        assert_eq!(
            ProbePluginBridge::convert_branch_kind(helm_probe::BranchKind::Return),
            BranchKind::Return
        );
    }

    #[cfg(debug_assertions)]
    #[test]
    fn wire_creates_subscriptions() {
        let session = HelmSpy::new();
        let mut probes = CpuProbes::default();
        ProbePluginBridge::wire(&session, &mut probes);
        // After wiring, probes should have listeners
        assert!(probes.any_active());
    }
}
