//! Phase 1 timing-hook integration tests for `helm-engine`.

use std::sync::{Arc, Mutex};

use helm_core::{AccessType, MemInterface};
use helm_devices::NullCharBackend;
use helm_engine::{ExecMode, HelmEngine, Isa, StopReason, TimingCacheConfig, TimingMemModelConfig};
use helm_event::{EventQueue, Tick};
use helm_hw_timer::Sp804;
use helm_memory::HelmAddressSpace;
use helm_platform::{BoardQuirk, PlatformQuirk, QuirkKey};
use helm_timing::{
    IntervalTiming, MemAccess, TimingInsnClass, TimingInsnInfo, TimingModel, TimingModelCaps,
    VirtualTiming,
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

#[derive(Clone, Default)]
struct FastTiming {
    cycles: Tick,
}

impl TimingModel for FastTiming {
    fn model_caps() -> TimingModelCaps {
        TimingModelCaps {
            idealized_fast_run: true,
            needs_operand_timing: false,
        }
    }

    fn on_insn(&mut self, info: &TimingInsnInfo) -> u64 {
        assert_eq!(
            info.src_reg_count, 0,
            "fast timing must not receive src deps"
        );
        assert_eq!(
            info.dst_reg_count, 0,
            "fast timing must not receive dst deps"
        );
        self.cycles += 1;
        1
    }

    fn on_mem_access(&mut self, _access: &MemAccess) {}

    fn on_branch(&mut self, _taken: bool, _predicted: bool) {}

    fn current_cycles(&self) -> Tick {
        self.cycles
    }

    fn advance_to(&mut self, tick: Tick) {
        if tick > self.cycles {
            self.cycles = tick;
        }
    }

    fn on_boundary(&mut self, _eq: &mut EventQueue) {
        panic!("idealized fast path should skip timing boundaries");
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

fn install_aarch64_system_words<T: TimingModel>(engine: &mut HelmEngine<T>, words: &[u32]) {
    let mut bytes = Vec::with_capacity(words.len() * 4);
    for word in words {
        bytes.extend_from_slice(&word.to_le_bytes());
    }

    let mut sys_mem = HelmAddressSpace::new(helm_engine::FlatMem::new(0, 0x2000));
    sys_mem.ram.load_bytes(0, &bytes);
    engine.install_test_aarch64_system_board(sys_mem).unwrap();
    engine.set_pc(0);
}

fn a64_with_rn(raw: u32, rn: u32) -> u32 {
    (raw & !0x3E0) | ((rn & 0x1F) << 5)
}

fn a64_with_rd(raw: u32, rd: u32) -> u32 {
    (raw & !0x1F) | (rd & 0x1F)
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
        vec![(0x4, 4, true, false, false), (0x4, 4, false, true, false),]
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
fn idealized_fast_run_skips_boundaries_and_syncs_event_tick() {
    let mut engine = HelmEngine::new(
        Isa::RiscV,
        ExecMode::Functional,
        FastTiming::default(),
        0,
        0x2000,
    );

    load_words(&mut engine, 0x100, &[0x0010_0093]); // addi x1, x0, 1

    assert_eq!(engine.run(1), StopReason::Quantum);
    assert_eq!(engine.current_cycles(), 1);
    assert_eq!(engine.events.current_tick(), 1);
}

#[test]
fn virtual_timing_honors_ipc_above_one_for_cycles_and_callbacks() {
    let mut engine = HelmEngine::new(
        Isa::RiscV,
        ExecMode::Functional,
        VirtualTiming::new(4.0),
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
            0x0000_0013, // nop
        ],
    );
    engine.post_callback_after(1, |engine| engine.load_bytes(0x81, &[0x5A]));

    assert_eq!(engine.run(3), StopReason::Quantum);
    assert_eq!(engine.current_cycles(), 0);
    assert_eq!(engine.memory.read(0x81, 1, AccessType::Load).unwrap(), 0);

    assert_eq!(engine.run(1), StopReason::Quantum);
    assert_eq!(engine.current_cycles(), 1);
    assert_eq!(engine.memory.read(0x81, 1, AccessType::Load).unwrap(), 0x5A);
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
    assert_eq!(engine.current_cycles(), 12);
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
    assert_eq!(interval_engine.current_cycles(), 12);
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
fn interval_timing_cache_config_changes_locality_outcomes() {
    let program = [
        0x0000_2023, // sw x0, 0(x0)
        0x0400_2023, // sw x0, 64(x0)
        0x0000_2083, // lw x1, 0(x0)
    ];

    let mut default_engine = HelmEngine::new(
        Isa::RiscV,
        ExecMode::Functional,
        IntervalTiming::new(2.0, 8),
        0,
        0x2000,
    );
    load_words(&mut default_engine, 0x100, &program);

    let mut tiny_cache_engine = HelmEngine::new(
        Isa::RiscV,
        ExecMode::Functional,
        IntervalTiming::new(2.0, 8),
        0,
        0x2000,
    )
    .with_timing_mem_model_config(TimingMemModelConfig {
        l1d: TimingCacheConfig::new(64, 1, 64),
        l2: TimingCacheConfig::new(64, 1, 64),
    });
    load_words(&mut tiny_cache_engine, 0x100, &program);

    assert_eq!(default_engine.run(3), StopReason::Quantum);
    assert_eq!(tiny_cache_engine.run(3), StopReason::Quantum);

    assert_eq!(default_engine.current_cycles(), 12);
    assert_eq!(tiny_cache_engine.current_cycles(), 13);
}

#[test]
fn interval_timing_overlaps_independent_riscv_load_misses() {
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
            0x0000_2083, // lw x1, 0(x0)
            0x0400_2103, // lw x2, 64(x0)
        ],
    );

    assert_eq!(engine.run(2), StopReason::Quantum);
    assert_eq!(engine.current_cycles(), 12);
}

#[test]
fn interval_timing_third_independent_riscv_load_miss_waits_for_mlp_slot() {
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
            0x0000_2083, // lw x1, 0(x0)
            0x0400_2103, // lw x2, 64(x0)
            0x0800_2183, // lw x3, 128(x0)
        ],
    );

    assert_eq!(engine.run(3), StopReason::Quantum);
    assert_eq!(engine.current_cycles(), 24);
}

#[test]
fn interval_timing_riscv_store_misses_overlap_with_following_load_miss() {
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
            0x0000_2023, // sw x0, 0(x0)
            0x0400_2023, // sw x0, 64(x0)
            0x0800_2083, // lw x1, 128(x0)
        ],
    );

    assert_eq!(engine.run(3), StopReason::Quantum);
    assert_eq!(engine.current_cycles(), 13);
}

#[test]
fn interval_timing_riscv_dependency_chain_costs_more_than_independent_work() {
    let dependent_program = [
        0x0010_0093, // addi x1, x0, 1
        0x0010_8113, // addi x2, x1, 1
        0x0011_0193, // addi x3, x2, 1
    ];
    let independent_program = [
        0x0010_0093, // addi x1, x0, 1
        0x0010_0113, // addi x2, x0, 1
        0x0010_0193, // addi x3, x0, 1
    ];

    let mut dependent_engine = HelmEngine::new(
        Isa::RiscV,
        ExecMode::Functional,
        IntervalTiming::new(2.0, 8),
        0,
        0x2000,
    );
    load_words(&mut dependent_engine, 0x100, &dependent_program);

    let mut independent_engine = HelmEngine::new(
        Isa::RiscV,
        ExecMode::Functional,
        IntervalTiming::new(2.0, 8),
        0,
        0x2000,
    );
    load_words(&mut independent_engine, 0x100, &independent_program);

    assert_eq!(dependent_engine.run(3), StopReason::Quantum);
    assert_eq!(independent_engine.run(3), StopReason::Quantum);

    assert_eq!(dependent_engine.current_cycles(), 3);
    assert_eq!(independent_engine.current_cycles(), 2);
}

#[test]
fn interval_timing_aarch64_dependency_chain_costs_more_than_independent_work() {
    let dependent_program = [
        0x9100_0401, // add x1, x0, #1
        0x9100_0422, // add x2, x1, #1
        0x9100_0443, // add x3, x2, #1
    ];
    let independent_program = [
        0x9100_0401, // add x1, x0, #1
        0x9100_0402, // add x2, x0, #1
        0x9100_0403, // add x3, x0, #1
    ];

    let mut dependent_engine = HelmEngine::new(
        Isa::AArch64,
        ExecMode::System,
        IntervalTiming::new(2.0, 8),
        0,
        0x2000,
    );
    install_aarch64_system_words(&mut dependent_engine, &dependent_program);
    dependent_engine
        .with_a64_state_mut(|a64| a64.x[0] = 5)
        .unwrap();

    let mut independent_engine = HelmEngine::new(
        Isa::AArch64,
        ExecMode::System,
        IntervalTiming::new(2.0, 8),
        0,
        0x2000,
    );
    install_aarch64_system_words(&mut independent_engine, &independent_program);
    independent_engine
        .with_a64_state_mut(|a64| a64.x[0] = 5)
        .unwrap();

    assert_eq!(dependent_engine.run(3), StopReason::Quantum);
    assert_eq!(independent_engine.run(3), StopReason::Quantum);

    assert_eq!(dependent_engine.current_cycles(), 3);
    assert_eq!(independent_engine.current_cycles(), 2);
}

#[test]
fn interval_timing_aarch64_pair_load_second_destination_costs_more_than_independent_work() {
    let dependent_program = [
        0xA940_0BE1, // ldp x1, x2, [sp]
        0x9100_0443, // add x3, x2, #1
    ];
    let independent_program = [
        0xA940_0BE1, // ldp x1, x2, [sp]
        0x9100_0403, // add x3, x0, #1
    ];

    let mut dependent_engine = HelmEngine::new(
        Isa::AArch64,
        ExecMode::System,
        IntervalTiming::new(2.0, 8),
        0,
        0x2000,
    );
    install_aarch64_system_words(&mut dependent_engine, &dependent_program);
    dependent_engine
        .with_a64_state_mut(|a64| a64.sp = 0x400)
        .unwrap();
    dependent_engine
        .with_system_memory_mut(|sys| {
            sys.write(0x400, 8, 0x1111_2222_3333_4444, AccessType::Store)
                .unwrap();
            sys.write(0x408, 8, 0x5555_6666_7777_8888, AccessType::Store)
                .unwrap();
        })
        .unwrap();

    let mut independent_engine = HelmEngine::new(
        Isa::AArch64,
        ExecMode::System,
        IntervalTiming::new(2.0, 8),
        0,
        0x2000,
    );
    install_aarch64_system_words(&mut independent_engine, &independent_program);
    independent_engine
        .with_a64_state_mut(|a64| a64.sp = 0x400)
        .unwrap();
    independent_engine
        .with_system_memory_mut(|sys| {
            sys.write(0x400, 8, 0x1111_2222_3333_4444, AccessType::Store)
                .unwrap();
            sys.write(0x408, 8, 0x5555_6666_7777_8888, AccessType::Store)
                .unwrap();
        })
        .unwrap();

    assert_eq!(dependent_engine.run(2), StopReason::Quantum);
    assert_eq!(independent_engine.run(2), StopReason::Quantum);

    assert_eq!(dependent_engine.current_cycles(), 13);
    assert_eq!(independent_engine.current_cycles(), 12);
}

#[test]
fn interval_timing_aarch64_sp_dependency_costs_more_than_independent_work() {
    let dependent_program = [
        0x9100_43FF, // add sp, sp, #16
        0xF940_03E1, // ldr x1, [sp]
    ];
    let independent_program = [
        0x9100_0402, // add x2, x0, #1
        0xF940_03E1, // ldr x1, [sp]
    ];

    let mut dependent_engine = HelmEngine::new(
        Isa::AArch64,
        ExecMode::System,
        IntervalTiming::new(2.0, 8),
        0,
        0x2000,
    );
    install_aarch64_system_words(&mut dependent_engine, &dependent_program);
    dependent_engine
        .with_a64_state_mut(|a64| a64.sp = 0x400)
        .unwrap();
    dependent_engine
        .with_system_memory_mut(|sys| {
            sys.write(0x400, 8, 0xAAAA_BBBB_CCCC_DDDD, AccessType::Store)
                .unwrap();
            sys.write(0x410, 8, 0x1111_2222_3333_4444, AccessType::Store)
                .unwrap();
        })
        .unwrap();

    let mut independent_engine = HelmEngine::new(
        Isa::AArch64,
        ExecMode::System,
        IntervalTiming::new(2.0, 8),
        0,
        0x2000,
    );
    install_aarch64_system_words(&mut independent_engine, &independent_program);
    independent_engine
        .with_a64_state_mut(|a64| a64.sp = 0x400)
        .unwrap();
    independent_engine
        .with_system_memory_mut(|sys| {
            sys.write(0x400, 8, 0xAAAA_BBBB_CCCC_DDDD, AccessType::Store)
                .unwrap();
            sys.write(0x410, 8, 0x1111_2222_3333_4444, AccessType::Store)
                .unwrap();
        })
        .unwrap();

    assert_eq!(dependent_engine.run(2), StopReason::Quantum);
    assert_eq!(independent_engine.run(2), StopReason::Quantum);

    assert_eq!(dependent_engine.current_cycles(), 13);
    assert_eq!(independent_engine.current_cycles(), 12);
}

#[test]
fn interval_timing_aarch64_reg_offset_dependency_costs_more_than_independent_work() {
    let dependent_program = [
        0x9100_0402, // add x2, x0, #1
        0xF862_7A63, // ldr x3, [x19, x2, lsl #3]
    ];
    let independent_program = [
        0x9100_0401, // add x1, x0, #1
        0xF862_7A63, // ldr x3, [x19, x2, lsl #3]
    ];

    let mut dependent_engine = HelmEngine::new(
        Isa::AArch64,
        ExecMode::System,
        IntervalTiming::new(2.0, 8),
        0,
        0x2000,
    );
    install_aarch64_system_words(&mut dependent_engine, &dependent_program);
    dependent_engine
        .with_a64_state_mut(|a64| {
            a64.x[0] = 1;
            a64.x[19] = 0x400;
            a64.x[2] = 0;
        })
        .unwrap();
    dependent_engine
        .with_system_memory_mut(|sys| {
            sys.write(0x410, 8, 0x1111_2222_3333_4444, AccessType::Store)
                .unwrap();
        })
        .unwrap();

    let mut independent_engine = HelmEngine::new(
        Isa::AArch64,
        ExecMode::System,
        IntervalTiming::new(2.0, 8),
        0,
        0x2000,
    );
    install_aarch64_system_words(&mut independent_engine, &independent_program);
    independent_engine
        .with_a64_state_mut(|a64| {
            a64.x[0] = 1;
            a64.x[19] = 0x400;
            a64.x[2] = 2;
        })
        .unwrap();
    independent_engine
        .with_system_memory_mut(|sys| {
            sys.write(0x410, 8, 0x1111_2222_3333_4444, AccessType::Store)
                .unwrap();
        })
        .unwrap();

    assert_eq!(dependent_engine.run(2), StopReason::Quantum);
    assert_eq!(independent_engine.run(2), StopReason::Quantum);

    assert_eq!(dependent_engine.current_cycles(), 13);
    assert_eq!(independent_engine.current_cycles(), 12);
}

#[test]
fn interval_timing_aarch64_simd_dependency_costs_more_than_independent_work() {
    let simd_add_v3 = 0x4EE7_8463u32; // ADD V3.2D, V3.2D, V7.2D
    let simd_add_v4 = a64_with_rd(a64_with_rn(simd_add_v3, 4), 4);
    let simd_abs_v0_v3 = a64_with_rn(0x4EA0_B820u32, 3); // ABS V0.4S, V3.4S

    let dependent_program = [simd_add_v3, simd_abs_v0_v3];
    let independent_program = [simd_add_v4, simd_abs_v0_v3];

    let mut dependent_engine = HelmEngine::new(
        Isa::AArch64,
        ExecMode::System,
        IntervalTiming::new(2.0, 8),
        0,
        0x2000,
    );
    install_aarch64_system_words(&mut dependent_engine, &dependent_program);
    dependent_engine
        .with_a64_state_mut(|a64| {
            a64.v[3] = 0x0000_0000_0000_0001_0000_0000_0000_0002u128;
            a64.v[4] = 0x0000_0000_0000_0003_0000_0000_0000_0004u128;
            a64.v[7] = 0x0000_0000_0000_0005_0000_0000_0000_0006u128;
        })
        .unwrap();

    let mut independent_engine = HelmEngine::new(
        Isa::AArch64,
        ExecMode::System,
        IntervalTiming::new(2.0, 8),
        0,
        0x2000,
    );
    install_aarch64_system_words(&mut independent_engine, &independent_program);
    independent_engine
        .with_a64_state_mut(|a64| {
            a64.v[3] = 0x0000_0000_0000_0001_0000_0000_0000_0002u128;
            a64.v[4] = 0x0000_0000_0000_0003_0000_0000_0000_0004u128;
            a64.v[7] = 0x0000_0000_0000_0005_0000_0000_0000_0006u128;
        })
        .unwrap();

    assert_eq!(dependent_engine.run(2), StopReason::Quantum);
    assert_eq!(independent_engine.run(2), StopReason::Quantum);

    assert_eq!(dependent_engine.current_cycles(), 6);
    assert_eq!(independent_engine.current_cycles(), 3);
}

#[test]
fn interval_timing_aarch64_dc_zva_dependency_costs_more_than_independent_work() {
    let dependent_program = [
        0x9104_0000, // add x0, x0, #0x100
        0xD50B_7420, // dc zva, x0
    ];
    let independent_program = [
        0x9104_0001, // add x1, x0, #0x100
        0xD50B_7420, // dc zva, x0
    ];

    let mut dependent_engine = HelmEngine::new(
        Isa::AArch64,
        ExecMode::System,
        IntervalTiming::new(2.0, 8),
        0,
        0x4000,
    );
    install_aarch64_system_words(&mut dependent_engine, &dependent_program);
    dependent_engine
        .with_a64_state_mut(|a64| a64.x[0] = 0x1000)
        .unwrap();

    let mut independent_engine = HelmEngine::new(
        Isa::AArch64,
        ExecMode::System,
        IntervalTiming::new(2.0, 8),
        0,
        0x4000,
    );
    install_aarch64_system_words(&mut independent_engine, &independent_program);
    independent_engine
        .with_a64_state_mut(|a64| a64.x[0] = 0x1100)
        .unwrap();

    assert_eq!(dependent_engine.run(2), StopReason::Quantum);
    assert_eq!(independent_engine.run(2), StopReason::Quantum);

    assert_eq!(dependent_engine.current_cycles(), 3);
    assert_eq!(independent_engine.current_cycles(), 2);
}

#[test]
fn interval_timing_aarch64_setf8_dependency_costs_more_than_independent_work() {
    let setf8_x1 = a64_with_rn(0x3A00_080D, 1);
    let dependent_program = [
        0x9100_0401, // add x1, x0, #1
        setf8_x1,    // setf8 x1
    ];
    let independent_program = [
        0x9100_0402, // add x2, x0, #1
        setf8_x1,    // setf8 x1
    ];

    let mut dependent_engine = HelmEngine::new(
        Isa::AArch64,
        ExecMode::System,
        IntervalTiming::new(2.0, 8),
        0,
        0x2000,
    );
    install_aarch64_system_words(&mut dependent_engine, &dependent_program);
    dependent_engine
        .with_a64_state_mut(|a64| {
            a64.x[0] = 0x7F;
            a64.x[1] = 0;
        })
        .unwrap();

    let mut independent_engine = HelmEngine::new(
        Isa::AArch64,
        ExecMode::System,
        IntervalTiming::new(2.0, 8),
        0,
        0x2000,
    );
    install_aarch64_system_words(&mut independent_engine, &independent_program);
    independent_engine
        .with_a64_state_mut(|a64| {
            a64.x[0] = 0x7F;
            a64.x[1] = 0x7F;
        })
        .unwrap();

    assert_eq!(dependent_engine.run(2), StopReason::Quantum);
    assert_eq!(independent_engine.run(2), StopReason::Quantum);

    assert_eq!(dependent_engine.current_cycles(), 2);
    assert_eq!(independent_engine.current_cycles(), 1);
}

#[test]
fn aarch64_system_store_and_load_emit_timing_mem_hooks() {
    let (timing, state) = RecordingTiming::new();
    let mut engine = HelmEngine::new(Isa::AArch64, ExecMode::System, timing, 0, 0x1000);

    let mut sys_mem = HelmAddressSpace::new(helm_engine::FlatMem::new(0, 0x2000));
    sys_mem.ram.load_bytes(
        0,
        &[
            0x40, 0x00, 0x00, 0xF9, // STR X0, [X2]
            0x41, 0x00, 0x40, 0xF9, // LDR X1, [X2]
        ],
    );

    engine.install_test_aarch64_system_board(sys_mem).unwrap();
    engine.set_pc(0);
    engine
        .with_a64_state_mut(|a64| {
            a64.x[2] = 0x400;
            a64.x[0] = 0x1122_3344_5566_7788;
        })
        .unwrap();

    assert_eq!(engine.run(2), StopReason::Quantum);

    let snap = snapshot(&state);
    assert_eq!(
        snap.insn_classes,
        vec![TimingInsnClass::Store, TimingInsnClass::Load]
    );
    assert_eq!(
        snap.mem_accesses,
        vec![
            (0x400, 8, true, false, false),
            (0x400, 8, false, true, false),
        ]
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
