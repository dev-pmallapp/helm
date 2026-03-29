//! Phase 1 timing-hook integration tests for `helm-engine`.

use std::sync::{Arc, Mutex};

use helm_core::{AccessType, MemInterface};
use helm_engine::{ExecMode, HelmEngine, Isa, StopReason};
use helm_event::{EventQueue, Tick};
use helm_hw_timer::Sp804;
use helm_memory::HelmAddressSpace;
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

#[test]
fn boundary_dispatches_typed_engine_events() {
    const TEST_EVENT_CLASS: u32 = 0x1234;

    let (timing, _state) = RecordingTiming::new();
    let mut engine = HelmEngine::new(Isa::RiscV, ExecMode::Functional, timing, 0, 0x2000);

    load_words(&mut engine, 0x100, &[0x0000_0013, 0x0000_0013]); // nop; nop
    engine.register_event_handler(TEST_EVENT_CLASS, |engine, owner_id, data| {
        let payload = *data.downcast::<u8>().expect("typed event payload");
        engine.load_bytes(0x90, &[payload, owner_id as u8]);
    });
    engine.post_event_after(2, TEST_EVENT_CLASS, 7, 0xCDu8);

    assert_eq!(engine.run(1), StopReason::Quantum);
    assert_eq!(engine.memory.read(0x90, 1, AccessType::Load).unwrap(), 0);

    assert_eq!(engine.run(1), StopReason::Quantum);
    assert_eq!(engine.memory.read(0x90, 1, AccessType::Load).unwrap(), 0xCD);
    assert_eq!(engine.memory.read(0x91, 1, AccessType::Load).unwrap(), 7);
}

#[test]
fn boundary_dispatches_typed_events_to_real_sp804_device() {
    const TEST_EVENT_CLASS: u32 = 0x2345;
    const TIMER_BASE: u64 = 0x0901_0000;
    const TIMER_LOAD: u64 = 0x00;
    const TIMER_CONTROL: u64 = 0x08;
    const TIMER_RIS: u64 = 0x10;
    const CTRL_ENABLE: u64 = 1 << 7;
    const CTRL_PERIODIC: u64 = 1 << 6;
    const CTRL_INTEN: u64 = 1 << 5;

    let sys_mem = Arc::new(Mutex::new(HelmAddressSpace::new(
        helm_engine::FlatMem::new(0, 0),
    )));
    let timer_idx = {
        let mut sys = sys_mem.lock().unwrap();
        let idx = sys.add_device(TIMER_BASE, Box::new(Sp804::new()));
        sys.write(TIMER_BASE + TIMER_LOAD, 4, 5, AccessType::Store)
            .unwrap();
        sys.write(
            TIMER_BASE + TIMER_CONTROL,
            4,
            CTRL_ENABLE | CTRL_PERIODIC | CTRL_INTEN,
            AccessType::Store,
        )
        .unwrap();
        idx
    };

    let (timing, _state) = RecordingTiming::new();
    let mut engine = HelmEngine::new(Isa::RiscV, ExecMode::Functional, timing, 0, 0x2000);
    load_words(
        &mut engine,
        0x100,
        &[
            0x0000_0013,
            0x0000_0013,
            0x0000_0013,
            0x0000_0013,
            0x0000_0013,
        ],
    );

    engine.register_tickable_device_handler::<Sp804>(TEST_EVENT_CLASS, Arc::clone(&sys_mem));
    engine.post_tickable_device_after(5, TEST_EVENT_CLASS, timer_idx, 5);

    assert_eq!(engine.run(4), StopReason::Quantum);
    {
        let mut sys = sys_mem.lock().unwrap();
        assert_eq!(
            sys.read(TIMER_BASE + TIMER_RIS, 4, AccessType::Load)
                .unwrap(),
            0
        );
    }

    assert_eq!(engine.run(1), StopReason::Quantum);
    {
        let mut sys = sys_mem.lock().unwrap();
        assert_eq!(
            sys.read(TIMER_BASE + TIMER_RIS, 4, AccessType::Load)
                .unwrap(),
            1
        );
    }
}
