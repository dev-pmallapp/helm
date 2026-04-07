//! Hot-trace recorder: detects hot backward branches and accumulates instruction
//! sequences for trace compilation (Phase 2-D).

use std::collections::HashMap;

use helm_arch::aarch64::insn::{Instruction, Opcode};

use super::{RecordState, TRACE_MAX_DEPTH, TRACE_MAX_INSNS, TRACE_THRESHOLD};

/// Detects hot backward branches and records the path they traverse.
#[derive(Default)]
pub struct TraceRecorder {
    /// Per-PC backward-branch execution counters.
    counters: HashMap<u64, u32>,
    /// Active recording state.
    pub state: RecordState,
}

impl TraceRecorder {
    /// Returns `true` when a trace recording is currently active.
    pub fn is_recording(&self) -> bool {
        matches!(self.state, RecordState::Recording { .. })
    }

    /// Called each time a backward branch lands at `target_pc`.
    ///
    /// Returns `true` if this call triggered the start of a new recording.
    pub fn on_backward_branch(&mut self, target_pc: u64) -> bool {
        // Don't start a new recording while one is active.
        if !matches!(self.state, RecordState::Idle) {
            return false;
        }
        let cnt = self.counters.entry(target_pc).or_insert(0);
        *cnt += 1;
        if *cnt == TRACE_THRESHOLD {
            self.state = RecordState::Recording {
                start_pc: target_pc,
                insns: Vec::with_capacity(64),
                pcs: Vec::new(),
                depth: 0,
            };
            true
        } else {
            false
        }
    }

    /// Feed a decoded block into the active recording.
    ///
    /// Returns the completed `(start_pc, insns)` pair when the recording closes
    /// on a backward branch to the trace header.
    pub fn record_block(&mut self, insns: &[Instruction]) -> Option<(u64, Vec<Instruction>)> {
        for insn in insns {
            if let Some(trace) = self.record(insn.pc, insn) {
                return Some(trace);
            }
        }
        None
    }

    /// Feed a decoded instruction into the active recording.
    ///
    /// Returns the completed `(start_pc, insns)` pair when the recording closes
    /// (i.e., a backward branch back to `start_pc` is detected). Returns `None`
    /// while recording is still in progress or if not recording.
    ///
    /// If the recording exceeds `TRACE_MAX_INSNS` or `TRACE_MAX_DEPTH`, it is
    /// aborted silently and `self.state` is reset to `Idle`.
    pub fn record(&mut self, _pc: u64, insn: &Instruction) -> Option<(u64, Vec<Instruction>)> {
        let closed = match &mut self.state {
            RecordState::Recording {
                start_pc,
                insns,
                pcs,
                depth,
            } => {
                pcs.push(insn.pc);
                insns.push(*insn);

                // Abort if limits exceeded.
                if insns.len() > TRACE_MAX_INSNS {
                    return self.abort();
                }

                // Detect branch instructions.
                if is_branch(insn) {
                    let target = branch_target(insn);
                    if target == *start_pc {
                        // Loop closure — trace is complete.
                        true
                    } else {
                        *depth += 1;
                        if *depth >= TRACE_MAX_DEPTH {
                            return self.abort();
                        }
                        false
                    }
                } else {
                    false
                }
            }
            _ => return None,
        };

        if closed {
            Some(self.finish_recording())
        } else {
            None
        }
    }

    fn finish_recording(&mut self) -> (u64, Vec<Instruction>) {
        let (start_pc, insns) = match std::mem::take(&mut self.state) {
            RecordState::Recording {
                start_pc, insns, ..
            } => (start_pc, insns),
            _ => unreachable!(),
        };
        self.state = RecordState::Idle;
        (start_pc, insns)
    }

    fn abort(&mut self) -> Option<(u64, Vec<Instruction>)> {
        self.state = RecordState::Idle;
        None
    }
}

/// Returns `true` if `insn` is any kind of branch.
fn is_branch(insn: &Instruction) -> bool {
    matches!(
        insn.opcode,
        Opcode::B
            | Opcode::BCond
            | Opcode::Bl
            | Opcode::Blr
            | Opcode::Br
            | Opcode::Ret
            | Opcode::Cbz
            | Opcode::Cbnz
            | Opcode::Tbz
            | Opcode::Tbnz
    )
}

/// Compute the static branch target PC.
///
/// For indirect branches (BLR, BR, RET) where the target is not statically
/// known, returns 0 (which will never equal `start_pc` for a real program).
fn branch_target(insn: &Instruction) -> u64 {
    match insn.opcode {
        Opcode::B | Opcode::Bl | Opcode::BCond | Opcode::Cbz | Opcode::Cbnz => {
            insn.pc.wrapping_add(insn.imm as u64)
        }
        Opcode::Tbz | Opcode::Tbnz => insn.pc.wrapping_add(insn.imm as u64),
        _ => 0, // indirect — no static target
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_bcond(pc: u64, target_pc: u64) -> Instruction {
        let mut i = Instruction::zeroed();
        i.opcode = Opcode::BCond;
        i.pc = pc;
        i.imm = target_pc.wrapping_sub(pc) as i64;
        i
    }

    fn make_b(pc: u64, target_pc: u64) -> Instruction {
        let mut i = Instruction::zeroed();
        i.opcode = Opcode::B;
        i.pc = pc;
        i.imm = target_pc.wrapping_sub(pc) as i64;
        i
    }

    fn make_add(pc: u64) -> Instruction {
        let mut i = Instruction::zeroed();
        i.opcode = Opcode::AddImm;
        i.pc = pc;
        i
    }

    #[test]
    fn threshold_triggers_recording() {
        let mut rec = TraceRecorder::default();
        for _ in 0..TRACE_THRESHOLD - 1 {
            assert!(!rec.on_backward_branch(0x1000));
        }
        assert!(rec.on_backward_branch(0x1000));
        assert!(matches!(
            rec.state,
            RecordState::Recording {
                start_pc: 0x1000,
                ..
            }
        ));
    }

    #[test]
    fn loop_closure_completes_trace() {
        let mut rec = TraceRecorder::default();
        for _ in 0..TRACE_THRESHOLD {
            rec.on_backward_branch(0x1000);
        }
        // Emit some body instructions
        assert!(rec.record(0x1000, &make_add(0x1000)).is_none());
        assert!(rec.record(0x1004, &make_add(0x1004)).is_none());
        // Backward branch back to start → completes
        let branch = make_bcond(0x1008, 0x1000);
        let result = rec.record(0x1008, &branch);
        assert!(result.is_some());
        let (start_pc, insns) = result.unwrap();
        assert_eq!(start_pc, 0x1000);
        assert_eq!(insns.len(), 3);
    }

    #[test]
    fn aborts_on_max_insns() {
        let mut rec = TraceRecorder::default();
        for _ in 0..TRACE_THRESHOLD {
            rec.on_backward_branch(0x1000);
        }
        // Feed TRACE_MAX_INSNS + 1 non-branch instructions
        for j in 0..=TRACE_MAX_INSNS {
            let result = rec.record(0x1000 + j as u64 * 4, &make_add(0x1000 + j as u64 * 4));
            if j == TRACE_MAX_INSNS {
                // Aborted — returns None, state back to Idle
                assert!(result.is_none());
                assert!(matches!(rec.state, RecordState::Idle));
            }
        }
    }

    #[test]
    fn multi_block_recording_completes_across_successive_blocks() {
        let mut rec = TraceRecorder::default();
        for _ in 0..TRACE_THRESHOLD {
            rec.on_backward_branch(0x1000);
        }

        let block0 = [make_add(0x1000), make_b(0x1004, 0x1008)];
        assert!(rec.record_block(&block0).is_none());
        assert!(rec.is_recording());

        let block1 = [make_add(0x1008), make_b(0x100c, 0x1000)];
        let (start_pc, insns) = rec.record_block(&block1).expect("trace should close");
        assert_eq!(start_pc, 0x1000);
        assert_eq!(insns.len(), 4);
        assert!(matches!(rec.state, RecordState::Idle));
    }
}
