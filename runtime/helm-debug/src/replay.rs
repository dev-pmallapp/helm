//! Replay/rewind planning artifacts derived from checkpoints and stop state.

use crate::{
    CheckpointHeader, CheckpointManager, DebugConnectionView, DebugError, DebugIntentCheckpoint,
    InspectionResult, RuntimeStopState,
};

/// Captured execution cut point that can anchor replay/rewind workflows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplayCutPoint {
    pub runtime_id: Option<usize>,
    pub active_connection: Option<DebugConnectionView>,
    pub pc: u64,
    pub insn_count: u64,
    pub cycle_count: u64,
    pub stop: RuntimeStopState,
}

/// User-facing summary of a captured execution cut point.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplayCutPointSummary {
    pub runtime_id: Option<usize>,
    pub pc: u64,
    pub insn_count: u64,
    pub cycle_count: u64,
    pub target: String,
    pub rendered_stop: String,
}

/// Minimal execution window recorded between replay cut points.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplaySegment {
    pub runtime_id: Option<usize>,
    pub active_connection: Option<DebugConnectionView>,
    pub kind: String,
    pub requested_insns: u64,
    pub start_pc: u64,
    pub end_pc: u64,
    pub start_insn_count: u64,
    pub end_insn_count: u64,
    pub start_cycle_count: u64,
    pub end_cycle_count: u64,
    pub stop: RuntimeStopState,
}

/// User-facing summary of a recorded execution window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplaySegmentSummary {
    pub runtime_id: Option<usize>,
    pub kind: String,
    pub requested_insns: u64,
    pub start_pc: u64,
    pub end_pc: u64,
    pub insn_delta: u64,
    pub cycle_delta: u64,
    pub target: String,
    pub rendered_stop: String,
}

/// Summary of the current inspection snapshot relevant to replay planning.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplayInspectionSummary {
    pub arch: Option<String>,
    pub pc: u64,
    pub register_count: usize,
    pub symbol_count: usize,
    pub device_names: Vec<String>,
}

/// Summary of the checkpoint used as a replay/rewind cut point.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplayCheckpointSummary {
    pub version: u32,
    pub entry_count: u32,
    pub pc: Option<u64>,
    pub insn_count: u64,
    pub cycle_count: u64,
    pub breakpoint_count: usize,
    pub watchpoint_count: usize,
}

/// Stored checkpoint record that can be selected for later replay planning.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplayCheckpointRecord {
    pub runtime_id: Option<usize>,
    pub active_connection: Option<DebugConnectionView>,
    pub checkpoint: ReplayCheckpointSummary,
    pub bytes: Vec<u8>,
}

/// A first-class replay planning artifact for user-facing control surfaces.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplayPlan {
    pub runtime_id: Option<usize>,
    pub active_connection: Option<DebugConnectionView>,
    pub checkpoint: ReplayCheckpointSummary,
    pub cut_point: Option<ReplayCutPointSummary>,
    pub segment: Option<ReplaySegmentSummary>,
    pub inspection: ReplayInspectionSummary,
    pub stop: RuntimeStopState,
    pub target: String,
    pub steps: Vec<String>,
}

impl ReplayCutPoint {
    pub fn capture(
        runtime_id: Option<usize>,
        active_connection: Option<DebugConnectionView>,
        pc: u64,
        insn_count: u64,
        cycle_count: u64,
        stop: RuntimeStopState,
    ) -> Self {
        Self {
            runtime_id,
            active_connection,
            pc,
            insn_count,
            cycle_count,
            stop,
        }
    }

    pub fn target(&self) -> String {
        replay_target_label(&self.stop)
    }

    pub fn summary(&self) -> ReplayCutPointSummary {
        ReplayCutPointSummary {
            runtime_id: self.runtime_id,
            pc: self.pc,
            insn_count: self.insn_count,
            cycle_count: self.cycle_count,
            target: self.target(),
            rendered_stop: self.stop.render(),
        }
    }
}

impl ReplaySegment {
    #[allow(clippy::too_many_arguments)]
    pub fn capture(
        runtime_id: Option<usize>,
        active_connection: Option<DebugConnectionView>,
        kind: impl Into<String>,
        requested_insns: u64,
        start_pc: u64,
        end_pc: u64,
        start_insn_count: u64,
        end_insn_count: u64,
        start_cycle_count: u64,
        end_cycle_count: u64,
        stop: RuntimeStopState,
    ) -> Self {
        Self {
            runtime_id,
            active_connection,
            kind: kind.into(),
            requested_insns,
            start_pc,
            end_pc,
            start_insn_count,
            end_insn_count,
            start_cycle_count,
            end_cycle_count,
            stop,
        }
    }

    pub fn target(&self) -> String {
        replay_target_label(&self.stop)
    }

    pub fn summary(&self) -> ReplaySegmentSummary {
        ReplaySegmentSummary {
            runtime_id: self.runtime_id,
            kind: self.kind.clone(),
            requested_insns: self.requested_insns,
            start_pc: self.start_pc,
            end_pc: self.end_pc,
            insn_delta: self.end_insn_count.saturating_sub(self.start_insn_count),
            cycle_delta: self.end_cycle_count.saturating_sub(self.start_cycle_count),
            target: self.target(),
            rendered_stop: self.stop.render(),
        }
    }
}

impl ReplayCheckpointRecord {
    pub fn capture(
        runtime_id: Option<usize>,
        active_connection: Option<DebugConnectionView>,
        insn_count: u64,
        cycle_count: u64,
        checkpoint_bytes: Vec<u8>,
    ) -> Result<Self, DebugError> {
        let checkpoint = checkpoint_summary_from_bytes(&checkpoint_bytes, insn_count, cycle_count)?;
        Ok(Self {
            runtime_id,
            active_connection,
            checkpoint,
            bytes: checkpoint_bytes,
        })
    }
}

impl ReplayPlan {
    pub fn from_checkpoint_bytes(
        checkpoint_bytes: &[u8],
        runtime_id: Option<usize>,
        active_connection: Option<DebugConnectionView>,
        stop: &RuntimeStopState,
        cut_point: Option<&ReplayCutPoint>,
        segment: Option<&ReplaySegment>,
        inspection: &InspectionResult,
    ) -> Result<Self, DebugError> {
        let checkpoint = checkpoint_summary_from_bytes(checkpoint_bytes, 0, 0)?;
        let cut_point_summary = cut_point.map(ReplayCutPoint::summary);
        let segment_summary = segment.map(ReplaySegment::summary);
        let inspection = ReplayInspectionSummary {
            arch: inspection.arch.clone(),
            pc: inspection.pc,
            register_count: inspection.int_regs.len(),
            symbol_count: inspection.symbols.len(),
            device_names: inspection
                .devices
                .iter()
                .map(|device| device.name.clone())
                .collect(),
        };
        let target = replay_target_label(stop);
        let steps = replay_steps(
            runtime_id,
            active_connection.as_ref(),
            &checkpoint,
            cut_point_summary.as_ref(),
            segment_summary.as_ref(),
            &inspection,
            stop,
        );

        Ok(Self {
            runtime_id,
            active_connection,
            checkpoint,
            cut_point: cut_point_summary,
            segment: segment_summary,
            inspection,
            stop: stop.clone(),
            target,
            steps,
        })
    }
}

fn replay_target_label(stop: &RuntimeStopState) -> String {
    if let Some(hit) = &stop.last_native_hit {
        return hit.kind_label().to_string();
    }
    stop.stop.kind_label().to_string()
}

fn replay_steps(
    runtime_id: Option<usize>,
    active_connection: Option<&DebugConnectionView>,
    checkpoint: &ReplayCheckpointSummary,
    cut_point: Option<&ReplayCutPointSummary>,
    segment: Option<&ReplaySegmentSummary>,
    inspection: &ReplayInspectionSummary,
    stop: &RuntimeStopState,
) -> Vec<String> {
    let mut steps = vec![format!(
        "restore checkpoint version {} with {} saved fields",
        checkpoint.version, checkpoint.entry_count
    )];

    if let Some(runtime_id) = runtime_id {
        if let Some(connection) = active_connection {
            steps.push(format!(
                "select debug connection {} ({}) before replay",
                runtime_id, connection.label
            ));
        } else {
            steps.push(format!(
                "select debug connection {} before replay",
                runtime_id
            ));
        }
    }

    if checkpoint.breakpoint_count != 0 || checkpoint.watchpoint_count != 0 {
        steps.push(format!(
            "re-establish {} breakpoints and {} watchpoints from checkpoint intent",
            checkpoint.breakpoint_count, checkpoint.watchpoint_count
        ));
    }

    if let Some(cut_point) = cut_point {
        steps.push(format!(
            "seek the recorded cut point at pc={:#x}, insns={}, cycles={}",
            cut_point.pc, cut_point.insn_count, cut_point.cycle_count
        ));
        steps.push(format!(
            "verify restored PC against captured cut point {:#x}",
            cut_point.pc
        ));
    } else {
        steps.push(format!(
            "verify restored PC against captured cut point {:#x}",
            checkpoint.pc.unwrap_or(inspection.pc)
        ));
    }

    if let Some(segment) = segment {
        steps.push(format!(
            "re-run the recorded {} window from {:#x} to {:#x} (delta_insns={}, delta_cycles={}, budget={})",
            segment.kind,
            segment.start_pc,
            segment.end_pc,
            segment.insn_delta,
            segment.cycle_delta,
            segment.requested_insns
        ));
    }

    if let Some(hit) = &stop.last_native_hit {
        match hit {
            crate::NativeTriggerHitView::Breakpoint(bp) => steps.push(format!(
                "run until breakpoint {} near {:#x} to reproduce the captured stop",
                bp.breakpoint_id, bp.addr
            )),
            crate::NativeTriggerHitView::Watchpoint(wp) => steps.push(format!(
                "run until watchpoint {} on {} near {:#x} to reproduce the captured stop",
                wp.watchpoint_id, wp.access, wp.addr
            )),
        }
    } else {
        steps.push(format!(
            "run until the next {} stop and compare it against the captured stop state",
            stop.stop.kind_label()
        ));
    }

    if !inspection.device_names.is_empty() {
        steps.push(format!(
            "capture inspection snapshots for devices [{}] before and after replay",
            inspection.device_names.join(", ")
        ));
    }

    steps
}

fn checkpoint_summary_from_bytes(
    checkpoint_bytes: &[u8],
    insn_count: u64,
    cycle_count: u64,
) -> Result<ReplayCheckpointSummary, DebugError> {
    let header = CheckpointHeader::from_bytes(checkpoint_bytes)?;
    let restored = CheckpointManager::new().restore_values(checkpoint_bytes)?;
    let checkpoint_pc = restored
        .iter()
        .find(|(name, _)| name == "pc")
        .map(|(_, value)| *value);
    let debug_intent = DebugIntentCheckpoint::from_restored_values(&restored);
    Ok(ReplayCheckpointSummary {
        version: header.version,
        entry_count: header.entry_count,
        pc: checkpoint_pc,
        insn_count,
        cycle_count,
        breakpoint_count: debug_intent.breakpoints.as_ref().map_or(0, Vec::len),
        watchpoint_count: debug_intent.watchpoints.as_ref().map_or(0, Vec::len),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        BreakAction, BreakpointIntent, InspectionResult, NativeTriggerHitView, RuntimeStopState,
        RuntimeStopView, WatchAction, WatchKind, WatchpointIntent,
    };

    #[test]
    fn replay_plan_summarizes_checkpoint_stop_and_inspection() {
        let mut values = vec![("pc".to_string(), 0x4000)];
        let debug_intent = DebugIntentCheckpoint {
            breakpoints: Some(vec![BreakpointIntent {
                addr: 0x4000,
                action: BreakAction::Log,
                enabled: true,
                hit_count: 2,
            }]),
            watchpoints: Some(vec![WatchpointIntent {
                start: 0x2000,
                size: 8,
                kind: WatchKind::ReadWrite,
                action: WatchAction::Break,
                enabled: true,
            }]),
        };
        debug_intent.append_values(&mut values);
        let refs: Vec<(&str, u64)> = values.iter().map(|(key, value)| (key.as_str(), *value)).collect();
        let checkpoint = CheckpointManager::new().save_values(&refs);

        let mut inspection = InspectionResult::new(0x4000);
        inspection.set_arch("aarch64");
        inspection.add_reg("x0", 0x1234);
        inspection.add_symbol("_start", 0x4000, 0x40);
        inspection.add_device_field("uart", "tx_count", "0");

        let stop = RuntimeStopState {
            stop: RuntimeStopView::JitBreakpoint { pc: Some(0x4000) },
            last_native_hit: Some(NativeTriggerHitView::Breakpoint(crate::BreakpointHitView {
                breakpoint_id: 7,
                addr: 0x4000,
                action: "log".to_string(),
            })),
        };
        let cut_point = ReplayCutPoint::capture(
            Some(3),
            Some(DebugConnectionView {
                runtime_id: 3,
                label: "core3".to_string(),
                arch: "aarch64".to_string(),
                mode: Some("functional".to_string()),
                role: "cpu".to_string(),
                domain: 0,
                active: true,
            }),
            0x4000,
            12,
            24,
            stop.clone(),
        );
        let segment = ReplaySegment::capture(
            Some(3),
            Some(DebugConnectionView {
                runtime_id: 3,
                label: "core3".to_string(),
                arch: "aarch64".to_string(),
                mode: Some("functional".to_string()),
                role: "cpu".to_string(),
                domain: 0,
                active: true,
            }),
            "run",
            64,
            0x3ff0,
            0x4000,
            4,
            12,
            8,
            24,
            stop.clone(),
        );
        let plan = ReplayPlan::from_checkpoint_bytes(
            &checkpoint,
            Some(3),
            Some(DebugConnectionView {
                runtime_id: 3,
                label: "core3".to_string(),
                arch: "aarch64".to_string(),
                mode: Some("functional".to_string()),
                role: "cpu".to_string(),
                domain: 0,
                active: true,
            }),
            &stop,
            Some(&cut_point),
            Some(&segment),
            &inspection,
        )
        .expect("replay plan");

        assert_eq!(plan.runtime_id, Some(3));
        assert_eq!(plan.checkpoint.pc, Some(0x4000));
        assert_eq!(plan.checkpoint.breakpoint_count, 1);
        assert_eq!(plan.checkpoint.watchpoint_count, 1);
        assert_eq!(plan.cut_point.as_ref().map(|cut| cut.insn_count), Some(12));
        assert_eq!(plan.segment.as_ref().map(|segment| segment.insn_delta), Some(8));
        assert_eq!(plan.inspection.arch.as_deref(), Some("aarch64"));
        assert_eq!(plan.inspection.symbol_count, 1);
        assert_eq!(plan.target, "native_breakpoint");
        assert!(plan.steps.iter().any(|step| step.contains("restore checkpoint version 1")));
        assert!(plan.steps.iter().any(|step| step.contains("select debug connection 3 (core3)")));
        assert!(plan.steps.iter().any(|step| step.contains("seek the recorded cut point")));
        assert!(plan.steps.iter().any(|step| step.contains("re-run the recorded run window")));
        assert!(plan.steps.iter().any(|step| step.contains("re-establish 1 breakpoints and 1 watchpoints")));
        assert!(plan.steps.iter().any(|step| step.contains("devices [uart]")));
    }
}
