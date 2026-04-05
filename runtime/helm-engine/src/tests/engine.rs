use crate::address_space::HelmAddressSpace;
use crate::fs::FsState;
use crate::platform::arm_virt::ArmVirtDevices;
use crate::session::{HelmBoard, HelmCore, HelmMachine, HelmVcpu};
use crate::{
    classify_aarch64_opcode, Aarch64Core, ExecMode, FlatMem, HelmEngine, Isa, VirtualTiming,
};
use helm_arch::aarch64::insn::Opcode;
use helm_arch::Aarch64ArchState;
#[cfg(all(feature = "jit-dynasm", not(feature = "jit-stencil")))]
use helm_jit::runtime::DEFAULT_RUNTIME_CONFIG;
use helm_platform::QuirkSet;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

#[test]
fn classify_implemented_simd_ops_as_non_stub() {
    for opcode in [
        Opcode::SimdAdd,
        Opcode::SimdSub,
        Opcode::SimdMul,
        Opcode::SimdAnd,
        Opcode::SimdOrr,
        Opcode::SimdEor,
        Opcode::SimdBic,
        Opcode::SimdNot,
        Opcode::SimdNeg,
        Opcode::SimdAbs,
        Opcode::SimdCmeq,
        Opcode::SimdCmgt,
        Opcode::SimdCmge,
        Opcode::SimdCmhi,
        Opcode::SimdCmhs,
        Opcode::SimdUmaxv,
        Opcode::SimdUminv,
    ] {
        let (_, _, is_stub) = classify_aarch64_opcode(opcode);
        assert!(!is_stub, "{opcode:?} should not be classified as a stub");
    }
}

#[test]
fn unimplemented_instruction_tracking_deduplicates_by_site() {
    let mut engine = HelmEngine::new(
        Isa::AArch64,
        ExecMode::Syscall,
        VirtualTiming::new(1.0),
        0,
        1 << 20,
    );

    assert!(engine.note_unimplemented_instruction(0x1000, 0xDEADBEEF, "SimdOther"));
    assert!(!engine.note_unimplemented_instruction(0x1000, 0xDEADBEEF, "SimdOther"));
    assert!(engine.note_unimplemented_instruction(0x1004, 0xDEADBEEF, "SimdOther"));
    assert_eq!(engine.unimplemented_instruction_count(), 2);
    assert!(engine.has_unimplemented_instructions());
}

#[test]
fn riscv_constructor_syncs_session_mode() {
    let engine = HelmEngine::new(
        Isa::RiscV,
        ExecMode::Syscall,
        VirtualTiming::new(1.0),
        0,
        0x1000,
    );
    assert_eq!(engine.active_mode(), ExecMode::Syscall);
}

#[test]
fn psci_cpu_on_powers_secondary_vcpu() {
    let mut cpu0 = Aarch64ArchState::new();
    cpu0.mpidr_el1 = 0x8000_0000;
    cpu0.sp_el1 = 0x8000_0000;
    cpu0.psci_via_engine = true;

    let mut cpu1 = Aarch64ArchState::new();
    cpu1.mpidr_el1 = 0x8000_0001;
    cpu1.psci_via_engine = true;

    let mut machine = HelmBoard {
        sys_mem: HelmAddressSpace::new(FlatMem::new(0, 0)),
        vcpus: vec![
            HelmVcpu {
                arch: cpu0,
                fs: FsState::new(),
                powered_on: true,
            },
            HelmVcpu {
                arch: cpu1,
                fs: FsState::new(),
                powered_on: false,
            },
        ],
        next_vcpu: 0,
        devs: ArmVirtDevices {
            gicd_idx: 0,
            gicc_idx: 0,
            uart_idx: 0,
            rtc_idx: None,
        },
        quirks: QuirkSet::default(),
        irq_lines: Vec::new(),
        gic: None,
    };

    HelmEngine::<VirtualTiming>::handle_fs_psci_call(
        &mut machine,
        0,
        "smc",
        0x8400_0003,
        0x8000_0001,
        0x1234_0000,
        0x55AA,
    )
    .unwrap();

    assert!(machine.vcpus[1].powered_on);
    assert_eq!(machine.vcpus[1].arch.pc, 0x1234_0000);
    assert_eq!(machine.vcpus[1].arch.x[0], 0x55AA);
    assert_eq!(machine.vcpus[0].arch.x[0], 0);
}

#[test]
fn fs_irq_polling_uses_selected_vcpu_irq_line() {
    let mut cpu0 = Aarch64ArchState::new();
    cpu0.pc = 0;
    cpu0.current_el = 1;
    cpu0.spsel = true;

    let mut cpu1 = Aarch64ArchState::new();
    cpu1.pc = 0;
    cpu1.current_el = 1;
    cpu1.spsel = true;

    let mut sys_mem = HelmAddressSpace::new(FlatMem::new(0, 0x1000));
    sys_mem.ram.load_bytes(0, &0xD503_201Fu32.to_le_bytes());

    let machine = HelmBoard {
        sys_mem,
        vcpus: vec![
            HelmVcpu {
                arch: cpu0,
                fs: FsState::new(),
                powered_on: false,
            },
            HelmVcpu {
                arch: cpu1,
                fs: FsState::new(),
                powered_on: true,
            },
        ],
        next_vcpu: 0,
        devs: ArmVirtDevices {
            gicd_idx: 0,
            gicc_idx: 0,
            uart_idx: 0,
            rtc_idx: None,
        },
        quirks: QuirkSet::default(),
        irq_lines: vec![
            Arc::new(AtomicBool::new(true)),
            Arc::new(AtomicBool::new(false)),
        ],
        gic: None,
    };

    let mut engine = HelmEngine::new(
        Isa::AArch64,
        ExecMode::System,
        VirtualTiming::new(1.0),
        0,
        0x1000,
    );
    engine.session = HelmMachine::new_primary(HelmCore::Aarch64(Aarch64Core::System(machine)));
    engine.irq_poll_countdown = 1;

    engine
        .step_aarch64_system()
        .expect("secondary vCPU should execute a NOP");

    let machine = engine
        .session
        .aarch64()
        .and_then(Aarch64Core::machine)
        .expect("machine should remain present");
    assert!(
        !machine.vcpus[1].fs.irq_pending,
        "CPU1 must not inherit CPU0's IRQ line state"
    );
}

#[test]
fn aarch64_se_decode_cache_rechecks_raw_after_code_change() {
    let mut engine = HelmEngine::new(
        Isa::AArch64,
        ExecMode::Functional,
        VirtualTiming::new(1.0),
        0,
        0x2000,
    );

    engine.load_bytes(0x100, &0xD503_201Fu32.to_le_bytes()); // nop
    engine.set_pc(0x100);
    assert_eq!(engine.run(1), crate::StopReason::Quantum);

    {
        let a64 = engine
            .session
            .aarch64()
            .and_then(Aarch64Core::state)
            .expect("functional AArch64 state");
        assert_eq!(a64.pc, 0x104);
    }

    engine.load_bytes(0x100, &0x9100_1400u32.to_le_bytes()); // add x0, x0, #5
    {
        let a64 = engine
            .session
            .aarch64_mut()
            .and_then(Aarch64Core::state_mut)
            .expect("functional AArch64 state");
        a64.pc = 0x100;
        a64.write_x(0, 2);
    }

    assert_eq!(engine.run(1), crate::StopReason::Quantum);

    let a64 = engine
        .session
        .aarch64()
        .and_then(Aarch64Core::state)
        .expect("functional AArch64 state");
    assert_eq!(a64.read_x(0), 7);
}

#[cfg(feature = "jit")]
fn encode_add_imm(sf: u32, sh: u32, imm12: u32, rn: u32, rd: u32) -> u32 {
    (sf << 31) | (0b10001 << 24) | (sh << 22) | (imm12 << 10) | (rn << 5) | rd
}

#[cfg(feature = "jit")]
fn encode_adrp(immhi: u32, immlo: u32, rd: u32) -> u32 {
    (1 << 31) | (immlo << 29) | (0b10000 << 24) | (immhi << 5) | rd
}

#[cfg(feature = "jit")]
fn encode_b(imm26: i32) -> u32 {
    (0b00101 << 26) | ((imm26 as u32) & 0x03FF_FFFF)
}

#[cfg(feature = "jit-stencil")]
fn encode_rv64_addi(rd: u32, rs1: u32, imm12: i32) -> u32 {
    (((imm12 as u32) & 0x0FFF) << 20) | (rs1 << 15) | (rd << 7) | 0b0010011
}

#[cfg(feature = "jit-stencil")]
fn encode_rv64_jal(rd: u32, offset: i32) -> u32 {
    let imm = (offset as u32) & 0x001F_FFFF;
    let bit20 = ((imm >> 20) & 0x1) << 31;
    let bits10_1 = ((imm >> 1) & 0x03FF) << 21;
    let bit11 = ((imm >> 11) & 0x1) << 20;
    let bits19_12 = ((imm >> 12) & 0xFF) << 12;
    bit20 | bits10_1 | bit11 | bits19_12 | (rd << 7) | 0b1101111
}

#[cfg(all(feature = "jit-dynasm", not(feature = "jit-stencil")))]
#[test]
fn jit_se_fallback_uses_bounded_interpreter_batch() {
    let mut engine = HelmEngine::new(
        Isa::AArch64,
        ExecMode::Functional,
        VirtualTiming::new(1.0),
        0,
        0x2000,
    );
    engine.set_jit(true);

    let code = [
        encode_add_imm(1, 0, 1, 0, 0), // supported by dynasm JIT
        encode_adrp(1, 0, 1),          // interpreter-only for dynasm backend
        encode_b(-2),                  // loop back to the ADRP
    ];
    let bytes: Vec<u8> = code.into_iter().flat_map(u32::to_le_bytes).collect();
    engine.load_bytes(0x1000, &bytes);
    engine.set_pc(0x1000);

    let stop = engine.run_jit(DEFAULT_RUNTIME_CONFIG.interp_fallback_batch_insns + 1);
    assert_eq!(stop, crate::StopReason::Quantum);
    assert_eq!(
        engine.insns_retired,
        DEFAULT_RUNTIME_CONFIG.interp_fallback_batch_insns + 1,
        "one JIT insn plus one bounded interpreter batch should retire"
    );

    let stats = engine.jit_perf_stats();
    assert!(stats.blocks_compiled >= 1);
    assert!(stats.blocks_executed >= 1);
    assert_eq!(stats.fallback_count, 1);
    assert_eq!(
        stats.fallback_insns,
        DEFAULT_RUNTIME_CONFIG.interp_fallback_batch_insns
    );
    assert_eq!(stats.unsupported_block_starts, 1);
    assert!(stats.block_cache_hits >= 1);
    assert!(stats.block_cache_misses >= 1);
    assert_eq!(stats.unsupported_opcodes.values().copied().sum::<u64>(), 1);
}

#[cfg(feature = "jit")]
#[test]
fn jit_perf_stats_report_cache_metadata() {
    let mut engine = HelmEngine::new(
        Isa::AArch64,
        ExecMode::Functional,
        VirtualTiming::new(1.0),
        0,
        0x2000,
    );
    engine.set_jit(true);

    let bytes: Vec<u8> = [
        encode_add_imm(1, 0, 1, 0, 0),
        encode_b(-1), // self-loop on the branch instruction
    ]
    .into_iter()
    .flat_map(u32::to_le_bytes)
    .collect();
    engine.load_bytes(0x1000, &bytes);
    engine.set_pc(0x1000);

    let stop = engine.run_jit(8);
    assert_eq!(stop, crate::StopReason::Quantum);

    let stats = engine.jit_perf_stats();
    assert!(stats.blocks_compiled >= 1);
    assert!(stats.blocks_executed >= 1);
    assert!(stats.block_cache_hits >= 1);
    assert!(stats.cache_entries >= 1);
}

#[cfg(feature = "jit-stencil")]
#[test]
fn jit_rv64_perf_stats_report_cache_activity() {
    let mut engine = HelmEngine::new(
        Isa::RiscV,
        ExecMode::Functional,
        VirtualTiming::new(1.0),
        0,
        0x2000,
    );
    engine.set_jit(true);

    let bytes: Vec<u8> = [encode_rv64_addi(1, 1, 1), encode_rv64_jal(0, -4)]
        .into_iter()
        .flat_map(u32::to_le_bytes)
        .collect();
    engine.load_bytes(0x1000, &bytes);
    engine.set_pc(0x1000);

    let stop = engine.run_jit(8);
    assert_eq!(stop, crate::StopReason::Quantum);
    assert_eq!(engine.insns_retired, 8);

    let stats = engine.jit_perf_stats();
    assert!(stats.blocks_compiled >= 1);
    assert!(stats.block_cache_hits >= 1);
    assert!(stats.block_cache_misses >= 1);
    assert!(stats.blocks_executed >= 1);
    assert!(stats.cache_entries >= 1);
}
