use crate::address_space::HelmAddressSpace;
use crate::fs::FsState;
use crate::platform::arm_virt::ArmVirtDevices;
use crate::session::{HelmBoard, HelmCore, HelmMachine, HelmVcpu};
use crate::{
    classify_aarch64_opcode, Aarch64Core, ExecMode, FlatMem, HelmEngine, Isa, VirtualTiming,
};
use helm_arch::aarch64::insn::Opcode;
use helm_arch::Aarch64ArchState;
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
        },
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
        },
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
