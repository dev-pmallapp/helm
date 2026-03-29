//! Phase 1 timing-hook integration tests for `helm-engine`.

use std::sync::{Arc, Mutex};

use helm_core::{AccessType, MemInterface};
use helm_engine::{ExecMode, HelmEngine, Isa, StopReason};
use helm_event::{EventQueue, Tick};
use helm_timing::{MemAccess, TimingInsnClass, TimingInsnInfo, TimingModel};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct RecordingSnapshot {
    insn_classes: Vec<TimingInsnClass>,
    mem_accesses: Vec<(u64, usize, bool, bool, bool)>,
    branches: Vec<(bool, bool)>,
    boundary_count: usize,
}

#[derive(Debug, Default)]
struct RecordingState {
    snapshot: RecordingSnapshot,
}

#[derive(Clone)]
struct RecordingTiming {
    state: Arc<Mutex<RecordingState>>,
    cycles: Tick,
}

impl RecordingTiming {
    fn new() -> (Self, Arc<Mutex<RecordingState>>) {
        let state = Arc::new(Mutex::new(RecordingState::default()));
        (
            Self {
                state: Arc::clone(&state),
                cycles: 0,
            },
            state,
        )
    }
}

impl TimingModel for RecordingTiming {
    fn on_insn(&mut self, info: &TimingInsnInfo) -> u64 {
        self.state
            .lock()
            .unwrap()
            .snapshot
            .insn_classes
            .push(info.class);
        self.cycles += 1;
        1
    }

    fn on_mem_access(&mut self, access: &MemAccess) {
        self.state.lock().unwrap().snapshot.mem_accesses.push((
            access.addr,
            access.size,
            access.is_store,
            access.hit_l1,
            access.hit_l2,
        ));
    }

    fn on_branch(&mut self, taken: bool, predicted: bool) {
        self.state
            .lock()
            .unwrap()
            .snapshot
            .branches
            .push((taken, predicted));
    }

    fn current_cycles(&self) -> Tick {
        self.cycles
    }

    fn on_boundary(&mut self, _eq: &mut EventQueue) {
        self.state.lock().unwrap().snapshot.boundary_count += 1;
    }
}

fn load_words(engine: &mut HelmEngine<RecordingTiming>, base: u64, words: &[u32]) {
    let mut bytes = Vec::with_capacity(words.len() * 4);
    for word in words {
        bytes.extend_from_slice(&word.to_le_bytes());
    }
    engine.load_bytes(base, &bytes);
    engine.set_pc(base);
}

fn snapshot(state: &Arc<Mutex<RecordingState>>) -> RecordingSnapshot {
    state.lock().unwrap().snapshot.clone()
}

#[test]
fn riscv_int_alu_step_emits_insn_and_boundary_hooks() {
    let (timing, state) = RecordingTiming::new();
    let mut engine = HelmEngine::new(Isa::RiscV, ExecMode::Functional, timing, 0, 0x2000);

    load_words(&mut engine, 0x100, &[0x0010_0093]); // addi x1, x0, 1

    assert_eq!(engine.run(1), StopReason::Quantum);

    let snap = snapshot(&state);
    assert_eq!(snap.insn_classes, vec![TimingInsnClass::IntAlu]);
    assert!(snap.mem_accesses.is_empty());
    assert!(snap.branches.is_empty());
    assert_eq!(snap.boundary_count, 1);
}

#[test]
fn riscv_store_and_load_emit_mem_hooks_with_direction() {
    let (timing, state) = RecordingTiming::new();
    let mut engine = HelmEngine::new(Isa::RiscV, ExecMode::Functional, timing, 0, 0x2000);

    load_words(
        &mut engine,
        0x100,
        &[
            0x0000_2223, // sw x0, 4(x0)
            0x0040_2083, // lw x1, 4(x0)
        ],
    );

    assert_eq!(engine.run(2), StopReason::Quantum);

    let snap = snapshot(&state);
    assert_eq!(
        snap.insn_classes,
        vec![TimingInsnClass::Store, TimingInsnClass::Load]
    );
    assert_eq!(
        snap.mem_accesses,
        vec![(0x4, 4, true, false, true), (0x4, 4, false, false, true),]
    );
    assert_eq!(snap.boundary_count, 2);
}

#[test]
fn riscv_taken_forward_branch_emits_branch_hook() {
    let (timing, state) = RecordingTiming::new();
    let mut engine = HelmEngine::new(Isa::RiscV, ExecMode::Functional, timing, 0, 0x2000);

    load_words(&mut engine, 0x100, &[0x0000_0463]); // beq x0, x0, +8

    assert_eq!(engine.run(1), StopReason::Quantum);

    let snap = snapshot(&state);
    assert_eq!(snap.insn_classes, vec![TimingInsnClass::Branch]);
    assert_eq!(snap.branches, vec![(true, false)]);
    assert_eq!(snap.boundary_count, 1);
}

#[test]
fn boundary_drives_event_queue_from_simulated_cycles() {
    let (timing, _state) = RecordingTiming::new();
    let mut engine = HelmEngine::new(Isa::RiscV, ExecMode::Functional, timing, 0, 0x2000);

    load_words(&mut engine, 0x100, &[0x0000_0013, 0x0000_0013]); // nop; nop
    engine.post_callback_after(2, |engine| engine.load_bytes(0x80, &[0xAB]));

    assert_eq!(engine.events.current_tick(), 0);
    assert_eq!(engine.run(1), StopReason::Quantum);
    assert_eq!(engine.events.current_tick(), 1);
    assert_eq!(engine.memory.read(0x80, 1, AccessType::Load).unwrap(), 0);

    assert_eq!(engine.run(1), StopReason::Quantum);
    assert_eq!(engine.events.current_tick(), 2);
    assert_eq!(engine.memory.read(0x80, 1, AccessType::Load).unwrap(), 0xAB);
}
