//! Phase 1 timing-hook integration tests for `helm-engine`.

use std::sync::{Arc, Mutex};

use helm_core::{AccessType, MemInterface};
use helm_devices::NullCharBackend;
use helm_engine::{ExecMode, HelmEngine, Isa, StopReason};
use helm_event::{EventQueue, Tick};
use helm_hw_timer::Sp804;
use helm_memory::HelmAddressSpace;
use helm_platform::{BoardQuirk, PlatformQuirk, QuirkKey};
use helm_timing::{
    IntervalTiming, MemAccess, TimingInsnClass, TimingInsnInfo, TimingModel, VirtualTiming,
};

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

    fn advance_to(&mut self, tick: Tick) {
        if tick > self.cycles {
            self.cycles = tick;
        }
    }

    fn on_boundary(&mut self, _eq: &mut EventQueue) {
        self.state.lock().unwrap().snapshot.boundary_count += 1;
    }
}

fn load_words<T: TimingModel>(engine: &mut HelmEngine<T>, base: u64, words: &[u32]) {
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
    let (timing, _state) = RecordingTiming::new();
    let mut engine = HelmEngine::new(Isa::RiscV, ExecMode::Functional, timing, 0, 0x2000);

    load_words(&mut engine, 0x100, &[0x0000_0013, 0x0000_0013]); // nop; nop
    let class_id = engine.register_event_handler_auto(|engine, owner_id, data| {
        let payload = *data.downcast::<u8>().expect("typed event payload");
        engine.load_bytes(0x90, &[payload, owner_id as u8]);
    });
    engine.post_event_after(2, class_id, 7, 0xCDu8);

    assert_eq!(engine.run(1), StopReason::Quantum);
    assert_eq!(engine.memory.read(0x90, 1, AccessType::Load).unwrap(), 0);

    assert_eq!(engine.run(1), StopReason::Quantum);
    assert_eq!(engine.memory.read(0x90, 1, AccessType::Load).unwrap(), 0xCD);
    assert_eq!(engine.memory.read(0x91, 1, AccessType::Load).unwrap(), 7);
}

#[test]
fn virtual_timing_current_cycles_match_retired_work() {
    let mut engine = HelmEngine::new(
        Isa::RiscV,
        ExecMode::Functional,
        VirtualTiming::new(1.0),
        0,
        0x2000,
    );

    load_words(
        &mut engine,
        0x100,
        &[
            0x0000_0013, // nop
            0x0000_0013, // nop
            0x0000_0013, // nop
        ],
    );

    assert_eq!(engine.run(3), StopReason::Quantum);
    assert_eq!(engine.current_cycles(), 3);
}

#[test]
fn interval_timing_cycles_include_mem_and_branch_penalties() {
    let mut engine = HelmEngine::new(
        Isa::RiscV,
        ExecMode::Functional,
        IntervalTiming::new(2.0, 8),
        0,
        0x2000,
    );

    load_words(
        &mut engine,
        0x100,
        &[
            0x0000_2223, // sw x0, 4(x0)
            0x0040_2083, // lw x1, 4(x0)
            0x0000_0463, // beq x0, x0, +8
        ],
    );

    assert_eq!(engine.run(3), StopReason::Quantum);
    assert_eq!(engine.current_cycles(), 16);
}

#[test]
fn interval_timing_drives_callbacks_earlier_than_virtual_for_same_workload() {
    let program = [
        0x0000_2223, // sw x0, 4(x0)
        0x0040_2083, // lw x1, 4(x0)
    ];

    let mut virtual_engine = HelmEngine::new(
        Isa::RiscV,
        ExecMode::Functional,
        VirtualTiming::new(1.0),
        0,
        0x2000,
    );
    load_words(&mut virtual_engine, 0x100, &program);
    virtual_engine.post_callback_after(6, |engine| engine.load_bytes(0x80, &[0x11]));

    let mut interval_engine = HelmEngine::new(
        Isa::RiscV,
        ExecMode::Functional,
        IntervalTiming::new(2.0, 8),
        0,
        0x2000,
    );
    load_words(&mut interval_engine, 0x100, &program);
    interval_engine.post_callback_after(6, |engine| engine.load_bytes(0x80, &[0x22]));

    assert_eq!(virtual_engine.run(2), StopReason::Quantum);
    assert_eq!(interval_engine.run(2), StopReason::Quantum);

    assert_eq!(virtual_engine.current_cycles(), 2);
    assert_eq!(interval_engine.current_cycles(), 10);
    assert_eq!(
        virtual_engine
            .memory
            .read(0x80, 1, AccessType::Load)
            .unwrap(),
        0
    );
    assert_eq!(
        interval_engine
            .memory
            .read(0x80, 1, AccessType::Load)
            .unwrap(),
        0x22
    );
}

#[test]
fn boundary_dispatches_typed_events_to_real_sp804_device() {
    const TIMER_BASE: u64 = 0x0901_0000;
    const TIMER_LOAD: u64 = 0x00;
    const TIMER_CONTROL: u64 = 0x08;
    const TIMER_RIS: u64 = 0x10;
    const CTRL_ENABLE: u64 = 1 << 7;
    const CTRL_PERIODIC: u64 = 1 << 6;
    const CTRL_INTEN: u64 = 1 << 5;

    let mut sys_mem = HelmAddressSpace::new(helm_engine::FlatMem::new(0, 0x1000));
    sys_mem.ram.load_bytes(
        0,
        &[
            0x1F, 0x20, 0x03, 0xD5, // nop
            0x1F, 0x20, 0x03, 0xD5, // nop
            0x1F, 0x20, 0x03, 0xD5, // nop
            0x1F, 0x20, 0x03, 0xD5, // nop
            0x1F, 0x20, 0x03, 0xD5, // nop
        ],
    );
    let timer_idx = {
        let idx = sys_mem.add_device(TIMER_BASE, Box::new(Sp804::new()));
        sys_mem
            .write(TIMER_BASE + TIMER_LOAD, 4, 5, AccessType::Store)
            .unwrap();
        sys_mem
            .write(
                TIMER_BASE + TIMER_CONTROL,
                4,
                CTRL_ENABLE | CTRL_PERIODIC | CTRL_INTEN,
                AccessType::Store,
            )
            .unwrap();
        idx
    };

    let (timing, _state) = RecordingTiming::new();
    let mut engine = HelmEngine::new(Isa::AArch64, ExecMode::System, timing, 0, 0x1000);
    engine.install_test_aarch64_system_board(sys_mem).unwrap();
    engine.set_pc(0);

    let class_id = engine.register_system_tickable_device_handler::<Sp804>();
    engine.post_tickable_device_after(5, class_id, timer_idx, 5);

    assert_eq!(engine.run(4), StopReason::Quantum);
    assert_eq!(
        engine
            .with_system_memory_mut(|sys| sys
                .read(TIMER_BASE + TIMER_RIS, 4, AccessType::Load)
                .unwrap())
            .unwrap(),
        0
    );

    assert_eq!(engine.run(1), StopReason::Quantum);
    assert_eq!(
        engine
            .with_system_memory_mut(|sys| sys
                .read(TIMER_BASE + TIMER_RIS, 4, AccessType::Load)
                .unwrap())
            .unwrap(),
        1
    );
}

#[test]
fn auto_allocated_event_classes_skip_reserved_callback_class() {
    let (timing, _state) = RecordingTiming::new();
    let mut engine = HelmEngine::new(Isa::RiscV, ExecMode::Functional, timing, 0, 0x2000);

    let class_a = engine.register_event_handler_auto(|_, _, _| {});
    let class_b = engine.register_event_handler_auto(|_, _, _| {});

    assert_eq!(class_a, 1);
    assert_eq!(class_b, 2);
    assert_ne!(class_a, u32::MAX);
    assert_ne!(class_b, u32::MAX);
}

#[test]
fn arm_virt_rtc_ticks_through_runtime_owned_registration() {
    const RTC_BASE: u64 = 0x0901_0000;

    let (timing, _state) = RecordingTiming::new();
    let mut engine = HelmEngine::new(Isa::AArch64, ExecMode::System, timing, 0, 0x1000);
    engine
        .install_arm_virt_board(
            256,
            1,
            helm_engine::platform::arm_virt::ArmVirtGicVersion::V2,
            Box::new(NullCharBackend),
        )
        .unwrap();
    engine
        .with_system_memory_mut(|sys| {
            sys.ram.load_bytes(
                0,
                &[
                    0x1F, 0x20, 0x03, 0xD5, // nop
                ],
            );
        })
        .unwrap();
    engine.set_pc(0);

    let tickables = engine.register_arm_virt_tickable_devices().unwrap();
    engine.post_tickable_device_after(1, tickables.rtc.class_id, tickables.rtc.device_idx, 1);

    assert_eq!(
        engine
            .with_system_memory_mut(|sys| sys.read(RTC_BASE, 4, AccessType::Load).unwrap())
            .unwrap(),
        0
    );
    assert_eq!(engine.run(1), StopReason::Quantum);
    assert_eq!(
        engine
            .with_system_memory_mut(|sys| sys.read(RTC_BASE, 4, AccessType::Load).unwrap())
            .unwrap(),
        1
    );
}

#[test]
fn arm_virt_board_exposes_default_quirk_selection() {
    let (timing, _state) = RecordingTiming::new();
    let mut engine = HelmEngine::new(Isa::AArch64, ExecMode::System, timing, 0, 0x1000);
    engine
        .install_arm_virt_board(
            256,
            1,
            helm_engine::platform::arm_virt::ArmVirtGicVersion::V2,
            Box::new(NullCharBackend),
        )
        .unwrap();

    assert_eq!(
        engine.system_board_has_quirk(QuirkKey::Platform(PlatformQuirk::ArmVirtPl031Rtc)),
        Some(true)
    );
    assert_eq!(
        engine.system_board_has_quirk(QuirkKey::Board(BoardQuirk::PsciViaEngine)),
        Some(true)
    );
}

#[test]
fn wfi_fast_forward_advances_timed_events() {
    const WFI: u32 = 0xD503_207F;

    let (timing, _state) = RecordingTiming::new();
    let mut engine = HelmEngine::new(Isa::AArch64, ExecMode::System, timing, 0, 0x1000);

    let mut sys_mem = HelmAddressSpace::new(helm_engine::FlatMem::new(0, 0x1000));
    sys_mem.ram.load_bytes(0, &WFI.to_le_bytes());
    engine.install_test_aarch64_system_board(sys_mem).unwrap();
    engine.set_pc(0);
    engine
        .with_a64_state_mut(|a64| {
            a64.cntv_ctl_el0 = 1;
            a64.cntv_cval_el0 = 10;
        })
        .unwrap();

    engine.post_callback_after(10, |engine| engine.load_bytes(0x80, &[0x5A]));

    assert_eq!(engine.events.current_tick(), 0);
    assert_eq!(engine.run(1), StopReason::Quantum);
    assert_eq!(engine.events.current_tick(), 10);
    assert_eq!(engine.memory.read(0x80, 1, AccessType::Load).unwrap(), 0x5A);
}
