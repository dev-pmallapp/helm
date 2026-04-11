use crate::address_space::{drain_pci_bus_remaps, HelmAddressSpace};
use crate::fs::FsState;
use crate::platform::arm_virt::ArmVirtDevices;
use crate::session::{HelmBoard, HelmCore, HelmMachine, HelmVcpu};
use crate::{
    build_simulator_from_request, classify_aarch64_opcode, Aarch64Core, ExecMode, FlatMem,
    HelmEngine, HelmSim, Isa, SimulatorBuildRequest, TimingChoice, VirtualTiming,
};
use helm_arch::aarch64::insn::Opcode;
use helm_arch::Aarch64ArchState;
use helm_core::{AccessType, HartException, MemInterface};
use helm_hw_pci::{config::PciConfigSpace, Bdf, PciBus, PciEndpoint};
#[cfg(feature = "jit-tiered")]
use helm_jit::cache::PROMOTE_THRESHOLD;
#[cfg(all(feature = "jit-dynasm", not(feature = "jit-stencil")))]
use helm_jit::runtime::{JitRuntimeConfig, DEFAULT_RUNTIME_CONFIG};
#[cfg(any(feature = "jit-dynasm", feature = "jit-tiered"))]
use helm_jit::trace::compiler::CompiledTrace;
#[cfg(any(feature = "jit-dynasm", feature = "jit-tiered"))]
use helm_jit::trace::exit::TraceInvalidationEvent;
#[cfg(all(feature = "jit-dynasm", not(feature = "jit-stencil")))]
use helm_jit::trace::GUARD_MISS_THRESHOLD;
use helm_platform::{BuiltInPlatform, QuirkSet};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

struct TestPciEndpoint {
    config: PciConfigSpace,
    vendor: u16,
    device: u16,
    class: u32,
}

impl TestPciEndpoint {
    fn new(vendor_id: u16, device_id: u16, class_code: u32) -> Self {
        Self {
            config: PciConfigSpace::new(vendor_id, device_id, class_code, 0x00),
            vendor: vendor_id,
            device: device_id,
            class: class_code,
        }
    }

    fn with_bar0(mut self, base: u32, size: u32) -> Self {
        self.config.set_bar_size(0, size);
        self.config.write(0x10, 4, base);
        self
    }
}

impl PciEndpoint for TestPciEndpoint {
    fn config_read(&self, offset: u16, size: usize) -> u32 {
        let off = offset as usize;
        match size {
            1 => self.config.data_ref().get(off).copied().unwrap_or(0) as u32,
            2 => self
                .config
                .data_ref()
                .get(off..off + 2)
                .map(|bytes| u16::from_le_bytes([bytes[0], bytes[1]]) as u32)
                .unwrap_or(0),
            4 => self
                .config
                .data_ref()
                .get(off..off + 4)
                .map(|bytes| u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
                .unwrap_or(0),
            _ => 0,
        }
    }

    fn config_write(&mut self, offset: u16, size: usize, val: u32) {
        self.config.write(offset, size, val);
    }

    fn vendor_id(&self) -> u16 {
        self.vendor
    }

    fn device_id(&self) -> u16 {
        self.device
    }

    fn class_code(&self) -> u32 {
        self.class
    }

    fn bar_base(&self, bar_index: u8) -> Option<u64> {
        self.config.bar_address(bar_index as usize)
    }

    fn bar_size(&self, bar_index: u8) -> Option<u64> {
        self.config.bar_size(bar_index as usize)
    }
}

struct MockBarDevice {
    last_write_offset: u64,
    last_write_val: u64,
}

impl MockBarDevice {
    fn new() -> Self {
        Self {
            last_write_offset: u64::MAX,
            last_write_val: 0,
        }
    }
}

impl helm_devices::Device for MockBarDevice {
    fn read(&mut self, _offset: u64, _size: usize) -> u64 {
        0
    }

    fn write(&mut self, offset: u64, _size: usize, val: u64) {
        self.last_write_offset = offset;
        self.last_write_val = val;
    }

    fn region_size(&self) -> u64 {
        0x1000
    }
}

#[cfg(feature = "jit")]
const DEFAULT_INFLATE_TEST: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../assets/aarch64/binaries/inflate_test"
);

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
fn simulator_build_request_constructs_expected_engine() {
    let sim = build_simulator_from_request(SimulatorBuildRequest::new(
        Isa::RiscV,
        ExecMode::Functional,
        TimingChoice::VirtualTiming { ipc: 2.0 },
        0x4000,
        0x8000,
    ));

    match sim {
        HelmSim::VirtualTiming(engine) => {
            assert_eq!(engine.isa, Isa::RiscV);
            assert_eq!(engine.mode, ExecMode::Functional);
        }
        _ => panic!("unexpected simulator variant"),
    }

    let system_request = SimulatorBuildRequest::new(
        Isa::AArch64,
        ExecMode::System,
        TimingChoice::VirtualTiming { ipc: 1.0 },
        BuiltInPlatform::ArmVirt.default_ram_base(),
        0x20_0000,
    )
    .with_platform(BuiltInPlatform::ArmVirt)
    .with_arm_virt_defaults(2, crate::platform::arm_virt::ArmVirtGicVersion::V2);
    assert_eq!(system_request.platform, Some(BuiltInPlatform::ArmVirt));

    let sim = build_simulator_from_request(system_request);
    match sim {
        HelmSim::VirtualTiming(engine) => {
            assert_eq!(engine.isa, Isa::AArch64);
            assert_eq!(engine.active_mode(), ExecMode::System);
            let machine = engine
                .session
                .aarch64()
                .and_then(Aarch64Core::machine)
                .expect("arm-virt system board should be realized");
            assert_eq!(machine.vcpus.len(), 2);
            assert!(matches!(machine.gic, Some(crate::session::HelmGic::V2(_))));
            assert!(machine.sys_mem.address_map.lookup(0x3000_0000).is_some());
        }
        _ => panic!("unexpected simulator variant"),
    }
}

#[test]
fn system_mode_fault_callbacks_do_not_require_riscv_runtime() {
    let mut engine = HelmEngine::new(
        Isa::AArch64,
        ExecMode::System,
        VirtualTiming::new(1.0),
        0,
        0x2000,
    );
    let seen = Arc::new(Mutex::new(None));
    let seen_fault = Arc::clone(&seen);
    engine.plugins.on_fault(Box::new(move |info| {
        *seen_fault.lock().unwrap() = Some(info.context.clone());
    }));

    let stop = engine.handle_exception(HartException::Breakpoint { pc: 0x1000 });

    assert!(matches!(
        stop,
        crate::StopReason::Exception(HartException::Breakpoint { pc: 0x1000 })
    ));
    assert!(matches!(
        seen.lock().unwrap().clone(),
        Some(helm_plugin::runtime::ArchContext::None)
    ));
}

#[test]
fn drain_pci_bus_remaps_projects_ecam_bar_write_onto_live_machine_memory() {
    const ECAM_BASE: u64 = 0x3000_0000;
    const BAR0_BASE: u64 = 0x0A00_0000;
    const NEW_BAR0_BASE: u64 = 0x0B00_0000;

    let mut sys_mem = HelmAddressSpace::new(FlatMem::new(0, 0));

    let mut pci_bus = PciBus::new("pci0");
    let endpoint =
        TestPciEndpoint::new(0x1AF4, 0x1001, 0x010000).with_bar0(BAR0_BASE as u32, 0x1000);
    pci_bus
        .attach_endpoint(Bdf::new(0, 1, 0), Box::new(endpoint))
        .unwrap();
    let pci_idx = sys_mem.add_device(ECAM_BASE, Box::new(pci_bus));

    let bar_dev_idx = sys_mem.add_device(BAR0_BASE, Box::new(MockBarDevice::new()));
    assert!(sys_mem.register_pci_bar_region(0, 1, 0, 0, bar_dev_idx, BAR0_BASE, 0x1000, 0));

    let ecam_bar0 = ECAM_BASE + (1u64 << 15) + 0x10;
    sys_mem
        .write(ecam_bar0, 4, NEW_BAR0_BASE, AccessType::Store)
        .unwrap();

    let result = drain_pci_bus_remaps(&mut sys_mem, pci_idx);
    assert_eq!(result.drained, 1);
    assert_eq!(result.applied, 1);

    assert!(sys_mem.address_map.lookup(BAR0_BASE).is_none());
    assert!(sys_mem.address_map.lookup(NEW_BAR0_BASE).is_some());

    sys_mem
        .write(NEW_BAR0_BASE + 0x18, 4, 0xCD, AccessType::Store)
        .unwrap();
    let bar_dev = sys_mem.device_as_mut::<MockBarDevice>(bar_dev_idx).unwrap();
    assert_eq!(bar_dev.last_write_offset, 0x18);
    assert_eq!(bar_dev.last_write_val, 0xCD);
}

#[test]
fn psci_cpu_on_powers_secondary_vcpu() {
    let mut cpu0 = Aarch64ArchState::new();
    cpu0.mpidr_el1 = 0x8000_0000;
    cpu0.sp_el1 = 0x8000_0000;
    cpu0.current_el = 1;
    cpu0.spsel = true;
    cpu0.psci_via_engine = true;

    let mut cpu1 = Aarch64ArchState::new();
    cpu1.mpidr_el1 = 0x8000_0001;
    cpu1.current_el = 1;
    cpu1.psci_via_engine = true;

    let mut machine = HelmBoard {
        sys_mem: Box::new(HelmAddressSpace::new(FlatMem::new(0, 0))),
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
            smmu_idx: None,
        },
        quirks: QuirkSet::default(),
        irq_lines: Vec::new(),
        gic: None,
        pci_msi: None,
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
    assert_eq!(machine.vcpus[1].arch.current_el, 1);
    assert_eq!(machine.vcpus[0].arch.x[0], 0);
}

#[test]
fn psci_cpu_on_preserves_el2_for_secondary_vcpu() {
    let mut cpu0 = Aarch64ArchState::new();
    cpu0.mpidr_el1 = 0x8000_0000;
    cpu0.sp_el2 = 0x9000_0000;
    cpu0.current_el = 2;
    cpu0.spsel = true;
    cpu0.psci_via_engine = true;

    let mut cpu1 = Aarch64ArchState::new();
    cpu1.mpidr_el1 = 0x8000_0001;
    cpu1.psci_via_engine = true;

    let mut machine = HelmBoard {
        sys_mem: Box::new(HelmAddressSpace::new(FlatMem::new(0, 0))),
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
            smmu_idx: None,
        },
        quirks: QuirkSet::default(),
        irq_lines: Vec::new(),
        gic: None,
        pci_msi: None,
    };

    HelmEngine::<VirtualTiming>::handle_fs_psci_call(
        &mut machine,
        0,
        "smc",
        0x8400_0003,
        0x8000_0001,
        0x2345_0000,
        0xAA55,
    )
    .unwrap();

    assert!(machine.vcpus[1].powered_on);
    assert_eq!(machine.vcpus[1].arch.current_el, 2);
    assert_eq!(machine.vcpus[1].arch.sp_el2, 0x8FFE_0000);
    assert_eq!((machine.vcpus[1].arch.id_aa64pfr0_el1 >> 8) & 0xF, 1);
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
        sys_mem: Box::new(sys_mem),
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
            smmu_idx: None,
        },
        quirks: QuirkSet::default(),
        irq_lines: vec![
            Arc::new(AtomicBool::new(true)),
            Arc::new(AtomicBool::new(false)),
        ],
        gic: None,
        pci_msi: None,
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
fn system_mode_accessors_follow_last_selected_vcpu() {
    let mut cpu0 = Aarch64ArchState::new();
    cpu0.pc = 0x0;
    cpu0.current_el = 1;
    cpu0.spsel = true;

    let mut cpu1 = Aarch64ArchState::new();
    cpu1.pc = 0x1000;
    cpu1.current_el = 1;
    cpu1.spsel = true;

    let mut sys_mem = HelmAddressSpace::new(FlatMem::new(0, 0x2000));
    sys_mem.ram.load_bytes(0x1000, &0xD503_201Fu32.to_le_bytes());

    let machine = HelmBoard {
        sys_mem: Box::new(sys_mem),
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
            smmu_idx: None,
        },
        quirks: QuirkSet::default(),
        irq_lines: vec![Arc::new(AtomicBool::new(false)), Arc::new(AtomicBool::new(false))],
        gic: None,
        pci_msi: None,
    };

    let mut engine = HelmEngine::new(
        Isa::AArch64,
        ExecMode::System,
        VirtualTiming::new(1.0),
        0,
        0x2000,
    );
    engine.session = HelmMachine::new_primary(HelmCore::Aarch64(Aarch64Core::System(machine)));

    engine
        .step_aarch64_system()
        .expect("secondary vCPU should execute a NOP");

    assert_eq!(engine.active_fs_vcpu, 1);
    assert_eq!(
        engine.aarch64_state_for_current_context().map(|state| state.pc),
        Some(0x1004)
    );
    assert_eq!(engine.with_a64_state_mut(|state| state.pc), Some(0x1004));
}

#[test]
fn next_timer_countdown_saturates_large_deadlines() {
    let mut a64 = Aarch64ArchState::new();
    let fs = FsState::new();

    a64.cntp_ctl_el0 = 1;
    a64.cntp_cval_el0 = u64::from(u32::MAX) + 1;

    assert_eq!(crate::next_timer_countdown(&a64, &fs), 4096);
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
fn encode_mrs(rt: u32, o0: u32, op1: u32, crn: u32, crm: u32, op2: u32) -> u32 {
    0xD500_0000
        | (1 << 21)
        | (1 << 20)
        | (o0 << 19)
        | (op1 << 16)
        | (crn << 12)
        | (crm << 8)
        | (op2 << 5)
        | rt
}

#[cfg(feature = "jit")]
fn encode_b(imm26: i32) -> u32 {
    (0b00101 << 26) | ((imm26 as u32) & 0x03FF_FFFF)
}

#[cfg(feature = "jit")]
fn encode_b_cond(cond: u32, imm19: i32) -> u32 {
    (0b01010100 << 24) | (((imm19 as u32) & 0x7ffff) << 5) | (cond & 0xf)
}

#[cfg(feature = "jit")]
fn encode_cbnz(sf: u32, imm19: i32, rt: u32) -> u32 {
    (sf << 31) | (0b011010_1 << 24) | (((imm19 as u32) & 0x7ffff) << 5) | rt
}

#[cfg(feature = "jit")]
fn encode_subs_imm(sf: u32, sh: u32, imm12: u32, rn: u32, rd: u32) -> u32 {
    (sf << 31)
        | (1 << 30)
        | (1 << 29)
        | (0b10001 << 24)
        | (sh << 22)
        | (imm12 << 10)
        | (rn << 5)
        | rd
}

#[cfg(feature = "jit")]
fn encode_orr_reg(sf: u32, shift_type: u32, rm: u32, shift_amt: u32, rn: u32, rd: u32) -> u32 {
    (sf << 31)
        | (0b01 << 29)
        | (0b01010 << 24)
        | (shift_type << 22)
        | (rm << 16)
        | (shift_amt << 10)
        | (rn << 5)
        | rd
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

#[cfg(any(feature = "jit-dynasm", feature = "jit-tiered"))]
fn make_test_trace(start_pc: u64) -> CompiledTrace {
    make_guard_test_trace(start_pc, start_pc + 0x10)
}

#[cfg(any(feature = "jit-dynasm", feature = "jit-tiered"))]
fn make_guard_test_trace(start_pc: u64, target_pc: u64) -> CompiledTrace {
    use helm_arch::aarch64::insn::{Instruction, Opcode};
    use helm_jit::trace::compiler::compile_trace;

    let mut add = Instruction::zeroed();
    add.opcode = Opcode::AddImm;
    add.pc = start_pc;
    add.rd = 0;
    add.rn = 0;
    add.imm = 1;
    add.sf = true;

    let mut branch = Instruction::zeroed();
    branch.opcode = Opcode::Cbnz;
    branch.pc = start_pc + 4;
    branch.rd = 0;
    branch.imm = target_pc.wrapping_sub(branch.pc) as i64;
    branch.sf = true;

    compile_trace(&[add, branch], start_pc).unwrap()
}

#[cfg(any(feature = "jit-dynasm", feature = "jit-tiered"))]
fn make_linear_test_trace(start_pc: u64) -> CompiledTrace {
    use helm_arch::aarch64::insn::{Instruction, Opcode};
    use helm_jit::trace::compiler::compile_trace;

    let mut add0 = Instruction::zeroed();
    add0.opcode = Opcode::AddImm;
    add0.pc = start_pc;
    add0.rd = 1;
    add0.rn = 1;
    add0.imm = 1;
    add0.sf = true;

    let mut add1 = Instruction::zeroed();
    add1.opcode = Opcode::AddImm;
    add1.pc = start_pc + 4;
    add1.rd = 1;
    add1.rn = 1;
    add1.imm = 1;
    add1.sf = true;

    compile_trace(&[add0, add1], start_pc).unwrap()
}

#[cfg(any(feature = "jit-dynasm", feature = "jit-tiered"))]
fn make_fused_subs_bne_trace(start_pc: u64, target_pc: u64) -> CompiledTrace {
    use helm_arch::aarch64::insn::{Instruction, Opcode};
    use helm_jit::trace::compiler::compile_trace;

    let mut subs = Instruction::zeroed();
    subs.opcode = Opcode::SubsImm;
    subs.pc = start_pc;
    subs.rd = 0;
    subs.rn = 0;
    subs.imm = 1;
    subs.sf = true;

    let mut bne = Instruction::zeroed();
    bne.opcode = Opcode::BCond;
    bne.pc = start_pc + 4;
    bne.imm = target_pc.wrapping_sub(bne.pc) as i64;
    bne.cond = 1;

    let mut add = Instruction::zeroed();
    add.opcode = Opcode::AddImm;
    add.pc = start_pc + 8;
    add.rd = 1;
    add.rn = 1;
    add.imm = 1;
    add.sf = true;

    compile_trace(&[subs, bne, add], start_pc).unwrap()
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
        encode_mrs(1, 3, 3, 4, 2, 0),  // MRS X1, NZCV -> interpreter-only for dynasm
        encode_b(-2),                  // loop back to the MRS
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

#[cfg(all(feature = "jit-dynasm", not(feature = "jit-stencil")))]
#[test]
fn jit_adrp_compiles_without_fallback() {
    let mut engine = HelmEngine::new(
        Isa::AArch64,
        ExecMode::Functional,
        VirtualTiming::new(1.0),
        0,
        0x4000,
    );
    engine.set_jit(true);

    engine.load_bytes(0x1234, &encode_adrp(0, 0, 0).to_le_bytes());
    engine.set_pc(0x1234);

    let stop = engine.run_jit(1);
    assert_eq!(stop, crate::StopReason::Quantum);

    let a64 = engine
        .session
        .aarch64()
        .and_then(Aarch64Core::state)
        .expect("functional AArch64 state");
    assert_eq!(a64.read_x(0), 0x1000);
    assert_eq!(a64.pc, 0x1238);

    let stats = engine.jit_perf_stats();
    assert!(stats.blocks_compiled >= 1);
    assert!(stats.blocks_executed >= 1);
    assert_eq!(stats.fallback_count, 0);
    assert!(stats.unsupported_opcodes.is_empty());
}

#[cfg(all(feature = "jit-dynasm", not(feature = "jit-stencil")))]
#[test]
fn jit_orr_reg_compiles_without_fallback() {
    let mut engine = HelmEngine::new(
        Isa::AArch64,
        ExecMode::Functional,
        VirtualTiming::new(1.0),
        0,
        0x4000,
    );
    engine.set_jit(true);

    engine.load_bytes(0x1000, &encode_orr_reg(1, 0, 2, 4, 1, 0).to_le_bytes());
    engine.set_pc(0x1000);
    {
        let a64 = engine
            .session
            .aarch64_mut()
            .and_then(Aarch64Core::state_mut)
            .expect("functional AArch64 state");
        a64.write_x(1, 0x1);
        a64.write_x(2, 0x10);
    }

    let stop = engine.run_jit(1);
    assert_eq!(stop, crate::StopReason::Quantum);

    let a64 = engine
        .session
        .aarch64()
        .and_then(Aarch64Core::state)
        .expect("functional AArch64 state");
    assert_eq!(a64.read_x(0), 0x101);
    assert_eq!(a64.pc, 0x1004);

    let stats = engine.jit_perf_stats();
    assert!(stats.blocks_compiled >= 1);
    assert!(stats.blocks_executed >= 1);
    assert_eq!(stats.fallback_count, 0);
    assert!(stats.unsupported_opcodes.is_empty());
}

#[cfg(all(feature = "jit-dynasm", not(feature = "jit-stencil")))]
#[test]
fn jit_ldrb_reg_offset_compiles_without_fallback() {
    let mut engine = HelmEngine::new(
        Isa::AArch64,
        ExecMode::Functional,
        VirtualTiming::new(1.0),
        0,
        0x4000,
    );
    engine.set_jit(true);

    engine.load_bytes(0x1000, &0x3862_6820u32.to_le_bytes()); // LDRB W0, [X1, X2]
    engine.load_bytes(0x2003, &[0xAB]);
    engine.set_pc(0x1000);
    {
        let a64 = engine
            .session
            .aarch64_mut()
            .and_then(Aarch64Core::state_mut)
            .expect("functional AArch64 state");
        a64.write_x(1, 0x2000);
        a64.write_x(2, 3);
    }

    let stop = engine.run_jit(1);
    assert_eq!(stop, crate::StopReason::Quantum);

    let a64 = engine
        .session
        .aarch64()
        .and_then(Aarch64Core::state)
        .expect("functional AArch64 state");
    assert_eq!(a64.read_x(0), 0xAB);
    assert_eq!(a64.pc, 0x1004);

    let stats = engine.jit_perf_stats();
    assert!(stats.blocks_compiled >= 1);
    assert!(stats.blocks_executed >= 1);
    assert_eq!(stats.fallback_count, 0);
    assert!(stats.unsupported_opcodes.is_empty());
}

#[cfg(all(feature = "jit-dynasm", not(feature = "jit-stencil")))]
#[test]
fn jit_ldrb_reg_offset_sxtw_compiles_without_fallback() {
    let mut engine = HelmEngine::new(
        Isa::AArch64,
        ExecMode::Functional,
        VirtualTiming::new(1.0),
        0,
        0x4000,
    );
    engine.set_jit(true);

    engine.load_bytes(0x1000, &0x3860_CA41u32.to_le_bytes()); // LDRB W1, [X18, W0, SXTW]
    engine.load_bytes(0x2005, &[0x7F]);
    engine.set_pc(0x1000);
    {
        let a64 = engine
            .session
            .aarch64_mut()
            .and_then(Aarch64Core::state_mut)
            .expect("functional AArch64 state");
        a64.write_x(0, 5);
        a64.write_x(18, 0x2000);
    }

    let stop = engine.run_jit(1);
    assert_eq!(stop, crate::StopReason::Quantum);

    let a64 = engine
        .session
        .aarch64()
        .and_then(Aarch64Core::state)
        .expect("functional AArch64 state");
    assert_eq!(a64.read_x(1), 0x7F);
    assert_eq!(a64.pc, 0x1004);

    let stats = engine.jit_perf_stats();
    assert!(stats.blocks_compiled >= 1);
    assert!(stats.blocks_executed >= 1);
    assert_eq!(stats.fallback_count, 0);
    assert!(stats.unsupported_opcodes.is_empty());
}

#[cfg(all(feature = "jit-dynasm", not(feature = "jit-stencil")))]
#[test]
fn jit_ldr_x_pre_index_preserves_pinned_x1() {
    let mut engine = HelmEngine::new(
        Isa::AArch64,
        ExecMode::Functional,
        VirtualTiming::new(1.0),
        0,
        0x4000,
    );
    engine.set_jit(true);

    engine.load_bytes(0x1000, &0xF841_8C24u32.to_le_bytes()); // LDR X4, [X1, #24]!
    engine.load_bytes(0x2018, &0x1122_3344_5566_7788u64.to_le_bytes());
    engine.set_pc(0x1000);
    {
        let a64 = engine
            .session
            .aarch64_mut()
            .and_then(Aarch64Core::state_mut)
            .expect("functional AArch64 state");
        a64.write_x(1, 0x2000);
    }

    let stop = engine.run_jit(1);
    assert_eq!(stop, crate::StopReason::Quantum);

    let a64 = engine
        .session
        .aarch64()
        .and_then(Aarch64Core::state)
        .expect("functional AArch64 state");
    assert_eq!(a64.read_x(4), 0x1122_3344_5566_7788);
    assert_eq!(a64.read_x(1), 0x2018, "pre-index writeback must preserve X1");
    assert_eq!(a64.pc, 0x1004);

    let stats = engine.jit_perf_stats();
    assert!(stats.blocks_compiled >= 1);
    assert!(stats.blocks_executed >= 1);
    assert_eq!(stats.fallback_count, 0);
    assert!(stats.unsupported_opcodes.is_empty());
}

#[cfg(all(feature = "jit-dynasm", not(feature = "jit-stencil")))]
#[test]
fn jit_ubfm_compiles_without_fallback() {
    let mut engine = HelmEngine::new(
        Isa::AArch64,
        ExecMode::Functional,
        VirtualTiming::new(1.0),
        0,
        0x4000,
    );
    engine.set_jit(true);

    engine.load_bytes(0x1000, &0xD344_FC00u32.to_le_bytes()); // LSR X0, X0, #4 (UBFM)
    engine.set_pc(0x1000);
    {
        let a64 = engine
            .session
            .aarch64_mut()
            .and_then(Aarch64Core::state_mut)
            .expect("functional AArch64 state");
        a64.write_x(0, 0xFF00);
    }

    let stop = engine.run_jit(1);
    assert_eq!(stop, crate::StopReason::Quantum);

    let a64 = engine
        .session
        .aarch64()
        .and_then(Aarch64Core::state)
        .expect("functional AArch64 state");
    assert_eq!(a64.read_x(0), 0x0FF0);
    assert_eq!(a64.pc, 0x1004);

    let stats = engine.jit_perf_stats();
    assert!(stats.blocks_compiled >= 1);
    assert!(stats.blocks_executed >= 1);
    assert_eq!(stats.fallback_count, 0);
    assert!(stats.unsupported_opcodes.is_empty());
}

#[cfg(all(feature = "jit-dynasm", not(feature = "jit-stencil")))]
#[test]
fn jit_se_elf_smoke_runs_without_native_fault() {
    let mut engine = HelmEngine::new(
        Isa::AArch64,
        ExecMode::Syscall,
        VirtualTiming::new(1.0),
        0,
        256 * 1024 * 1024,
    );
    engine
        .load_aarch64_elf(
            DEFAULT_INFLATE_TEST,
            &["inflate_test"],
            &[
                "HOME=/tmp/home/pmallapp",
                "TERM=dumb",
                "PATH=/usr/bin:/bin",
                "LANG=C",
                "USER=helm",
            ],
        )
        .expect("load AArch64 ELF");
    engine.set_jit(true);

    let stop = engine.run_jit(1);
    assert_eq!(stop, crate::StopReason::Quantum);
    assert!(engine.insns_retired >= 1);

    let stats = engine.jit_perf_stats();
    assert!(
        stats.blocks_executed >= 1 || stats.fallback_count >= 1,
        "SE-mode JIT should retire work through either compiled blocks or bounded fallback"
    );
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
    assert!(stats.compiled_guest_insns >= stats.blocks_compiled);
    assert!(stats.blocks_executed >= 1);
    assert!(stats.block_cache_hits >= 1);
    assert!(stats.cache_entries >= 1);
}

#[cfg(all(feature = "jit-dynasm", not(feature = "jit-stencil")))]
#[test]
fn jit_aarch64_branchy_loop_reports_longer_compiled_blocks() {
    let mut engine = HelmEngine::new(
        Isa::AArch64,
        ExecMode::Functional,
        VirtualTiming::new(1.0),
        0,
        0x2000,
    );
    engine.set_jit(true);

    let bytes: Vec<u8> = [
        encode_cbnz(1, 2, 2),           // CBNZ X2, +8 -> skip ADD if x2 != 0
        encode_add_imm(1, 0, 1, 1, 1),  // ADD X1, X1, #1
        encode_add_imm(1, 0, 2, 2, 1),  // ADD X2, X2, #1
        encode_subs_imm(1, 0, 1, 0, 0), // SUBS X0, X0, #1
        encode_b_cond(1, -4),           // B.NE back to 0x1000
    ]
    .into_iter()
    .flat_map(u32::to_le_bytes)
    .collect();
    engine.load_bytes(0x1000, &bytes);
    engine.set_pc(0x1000);

    {
        let a64 = engine
            .session
            .aarch64_mut()
            .and_then(Aarch64Core::state_mut)
            .expect("functional AArch64 state");
        a64.write_x(0, 3);
        a64.write_x(1, 10);
        a64.write_x(2, 0);
    }

    let stop = engine.run_jit(4);
    assert_eq!(stop, crate::StopReason::Quantum);

    let stats = engine.jit_perf_stats();
    assert!(stats.blocks_compiled >= 1);
    assert!(
        stats.compiled_guest_insns >= 4,
        "expected at least one branch-heavy block to compile past the conditional branch"
    );
    assert!(
        stats.compiled_guest_insns / stats.blocks_compiled >= 4,
        "average compiled block length should reflect conditional fallthrough continuity"
    );
}

#[cfg(all(feature = "jit-dynasm", not(feature = "jit-stencil")))]
#[test]
fn jit_aarch64_multi_block_hot_loop_compiles_trace_candidate() {
    let mut engine = HelmEngine::new(
        Isa::AArch64,
        ExecMode::Functional,
        VirtualTiming::new(1.0),
        0,
        0x2000,
    );
    engine.set_jit(true);

    let bytes: Vec<u8> = [
        encode_b(2),  // 0x1000 -> 0x1008
        0xD503_201F,  // nop padding at 0x1004
        encode_b(-2), // 0x1008 -> 0x1000
    ]
    .into_iter()
    .flat_map(u32::to_le_bytes)
    .collect();
    engine.load_bytes(0x1000, &bytes);
    engine.set_pc(0x1000);

    let stop = engine.run_jit(140);
    assert_eq!(stop, crate::StopReason::Quantum);

    let stats = engine.jit_perf_stats();
    assert!(stats.traces_compiled >= 1);
    assert!(stats.trace_guest_insns >= 1);
    assert_eq!(stats.traces_executed, 0);
    assert!(stats.trace_cache_entries >= 1);
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
    assert!(stats.compiled_guest_insns >= stats.blocks_compiled);
    assert!(stats.block_cache_hits >= 1);
    assert!(stats.block_cache_misses >= 1);
    assert!(stats.blocks_executed >= 1);
    assert!(stats.cache_entries >= 1);
}

#[cfg(any(feature = "jit-dynasm", feature = "jit-tiered"))]
#[test]
fn jit_trace_cache_invalidation_updates_retire_stats() {
    let mut engine = HelmEngine::new(
        Isa::AArch64,
        ExecMode::Functional,
        VirtualTiming::new(1.0),
        0,
        0x2000,
    );
    engine.set_jit(true);

    engine
        .jit_trace_cache
        .as_mut()
        .expect("trace cache")
        .insert(make_guard_test_trace(0x1000, 0x1010));

    engine.invalidate_jit_traces(TraceInvalidationEvent::AddressSpaceChange);

    assert!(engine
        .jit_trace_cache
        .as_ref()
        .expect("trace cache")
        .is_empty());
    let stats = engine.jit_perf_stats();
    assert_eq!(stats.trace_retired, 1);
    assert_eq!(stats.trace_cache_entries, 0);
}

#[cfg(all(feature = "jit-dynasm", not(feature = "jit-stencil")))]
#[test]
fn jit_trace_lookup_ordering_updates_hit_and_miss_stats_before_dispatch() {
    let mut engine = HelmEngine::new(
        Isa::AArch64,
        ExecMode::Functional,
        VirtualTiming::new(1.0),
        0,
        0x2000,
    );
    engine.set_jit(true);

    let bytes: Vec<u8> = [encode_b(0)]
        .into_iter()
        .flat_map(u32::to_le_bytes)
        .collect();
    engine.load_bytes(0x1000, &bytes);
    engine.set_pc(0x1000);
    engine
        .jit_trace_cache
        .as_mut()
        .expect("trace cache")
        .insert(make_test_trace(0x1000));

    let stop = engine.run_jit(2);
    assert_eq!(stop, crate::StopReason::Quantum);

    let stats = engine.jit_perf_stats();
    assert!(stats.trace_cache_hits >= 1);
    assert_eq!(stats.traces_executed, 0);
    assert!(stats.block_cache_hits >= 1);

    engine.set_pc(0x1100);
    engine.load_bytes(0x1100, &encode_b(0).to_le_bytes());
    let stop = engine.run_jit(1);
    assert_eq!(stop, crate::StopReason::Quantum);

    let stats = engine.jit_perf_stats();
    assert!(stats.trace_cache_misses >= 1);
}

#[cfg(all(feature = "jit-dynasm", not(feature = "jit-stencil")))]
#[test]
fn jit_trace_dispatch_executes_enabled_trace_before_block_cache() {
    let mut engine = HelmEngine::new(
        Isa::AArch64,
        ExecMode::Functional,
        VirtualTiming::new(1.0),
        0,
        0x2000,
    );
    engine.set_jit(true);
    engine.set_jit_runtime_config(JitRuntimeConfig {
        trace_dispatch_enabled: true,
        ..DEFAULT_RUNTIME_CONFIG
    });

    let bytes: Vec<u8> = [encode_add_imm(1, 0, 1, 1, 1), encode_add_imm(1, 0, 1, 1, 1)]
        .into_iter()
        .flat_map(u32::to_le_bytes)
        .collect();
    engine.load_bytes(0x1000, &bytes);
    engine.set_pc(0x1000);
    engine
        .jit_trace_cache
        .as_mut()
        .expect("trace cache")
        .insert(make_linear_test_trace(0x1000));

    let stop = engine.run_jit(2);
    assert_eq!(stop, crate::StopReason::Quantum);

    let a64 = engine
        .session
        .aarch64()
        .and_then(Aarch64Core::state)
        .expect("functional AArch64 state");
    assert_eq!(a64.read_x(1), 2);
    assert_eq!(a64.pc, 0x1008);

    let stats = engine.jit_perf_stats();
    assert_eq!(stats.traces_executed, 1);
    assert!(stats.trace_cache_hits >= 1);
    assert_eq!(stats.blocks_executed, 0);
}

#[cfg(all(feature = "jit-dynasm", not(feature = "jit-stencil")))]
#[test]
fn jit_trace_guard_exit_resumes_in_block_jit_and_updates_stats() {
    let mut engine = HelmEngine::new(
        Isa::AArch64,
        ExecMode::Functional,
        VirtualTiming::new(1.0),
        0,
        0x4000,
    );
    engine.set_jit(true);
    engine.set_jit_runtime_config(JitRuntimeConfig {
        trace_dispatch_enabled: true,
        ..DEFAULT_RUNTIME_CONFIG
    });

    let bytes: Vec<u8> = [
        encode_add_imm(1, 0, 1, 1, 1), // x1 += 1
        encode_cbnz(1, 3, 0),          // if x0 != 0, jump to 0x1010
        encode_add_imm(1, 0, 1, 1, 1), // off-trace fallthrough
        0xD503_201F,                   // padding at 0x100c
        encode_b(0),                   // 0x1010 self-loop
    ]
    .into_iter()
    .flat_map(u32::to_le_bytes)
    .collect();
    engine.load_bytes(0x1000, &bytes);
    engine.set_pc(0x1000);
    engine
        .jit_trace_cache
        .as_mut()
        .expect("trace cache")
        .insert(make_test_trace(0x1000));

    {
        let a64 = engine
            .session
            .aarch64_mut()
            .and_then(Aarch64Core::state_mut)
            .expect("functional AArch64 state");
        a64.write_x(0, 1);
        a64.write_x(1, 0);
    }

    let stop = engine.run_jit(3);
    assert_eq!(stop, crate::StopReason::Quantum);

    let a64 = engine
        .session
        .aarch64()
        .and_then(Aarch64Core::state)
        .expect("functional AArch64 state");
    assert_eq!(a64.read_x(0), 2);
    assert_eq!(a64.pc, 0x1010);

    let stats = engine.jit_perf_stats();
    assert_eq!(stats.traces_executed, 1);
    assert_eq!(stats.trace_guard_exits, 1);
    assert!(stats.blocks_executed >= 1);
}

#[cfg(feature = "jit-stencil")]
#[test]
fn jit_aarch64_system_mode_compiles_identity_mapped_block() {
    let mut engine = HelmEngine::new(
        Isa::AArch64,
        ExecMode::System,
        VirtualTiming::new(1.0),
        0,
        0x2000,
    );
    engine.set_jit(true);

    let mut sys_mem = HelmAddressSpace::new(FlatMem::new(0, 0x2000));
    let bytes: Vec<u8> = [encode_add_imm(1, 0, 1, 1, 1), encode_b(-1)]
        .into_iter()
        .flat_map(u32::to_le_bytes)
        .collect();
    sys_mem.ram.load_bytes(0x1000, &bytes);
    engine
        .install_test_aarch64_system_board(sys_mem)
        .expect("install test system board");
    engine.set_pc(0x1000);

    let stop = engine.run_jit(8);
    assert_eq!(stop, crate::StopReason::Quantum);

    let stats = engine.jit_perf_stats();
    assert!(
        stats.blocks_compiled >= 1,
        "system-mode JIT should compile from system memory rather than hand off immediately"
    );
    assert!(stats.blocks_executed >= 1);
    assert!(stats.block_cache_hits >= 1);
}

#[cfg(feature = "jit-tiered")]
#[test]
fn jit_aarch64_system_mode_tiered_keeps_stencil_hot_blocks() {
    let mut engine = HelmEngine::new(
        Isa::AArch64,
        ExecMode::System,
        VirtualTiming::new(1.0),
        0,
        0x2000,
    );
    engine.set_jit(true);

    let mut sys_mem = HelmAddressSpace::new(FlatMem::new(0, 0x2000));
    let bytes: Vec<u8> = [encode_add_imm(1, 0, 1, 1, 1), encode_b(-1)]
        .into_iter()
        .flat_map(u32::to_le_bytes)
        .collect();
    sys_mem.ram.load_bytes(0x1000, &bytes);
    engine
        .install_test_aarch64_system_board(sys_mem)
        .expect("install test system board");
    engine.set_pc(0x1000);

    let stop = engine.run_jit(u64::from(PROMOTE_THRESHOLD) * 2);
    assert_eq!(stop, crate::StopReason::Quantum);

    let stats = engine.jit_perf_stats();
    assert_eq!(
        stats.cache_promotions, 0,
        "FS mode should not hot-promote stencil blocks into dynasm"
    );
    assert!(stats.blocks_executed >= u64::from(PROMOTE_THRESHOLD));
}
#[cfg(all(feature = "jit-dynasm", not(feature = "jit-stencil")))]
#[test]
fn jit_trace_dispatch_config_is_forced_off_in_system_mode() {
    let mut engine = HelmEngine::new(
        Isa::AArch64,
        ExecMode::System,
        VirtualTiming::new(1.0),
        0,
        0x2000,
    );
    engine.set_jit_runtime_config(JitRuntimeConfig {
        trace_dispatch_enabled: true,
        ..DEFAULT_RUNTIME_CONFIG
    });

    let effective = engine.effective_jit_runtime_config();
    assert!(
        !effective.trace_dispatch_enabled,
        "system mode must keep live trace dispatch disabled"
    );
}

#[cfg(all(feature = "jit-dynasm", not(feature = "jit-stencil")))]
#[test]
fn jit_trace_guard_exit_retires_after_repeated_se_hits() {
    let mut engine = HelmEngine::new(
        Isa::AArch64,
        ExecMode::Functional,
        VirtualTiming::new(1.0),
        0,
        0x4000,
    );
    engine.set_jit(true);
    engine.set_jit_runtime_config(JitRuntimeConfig {
        trace_dispatch_enabled: true,
        ..DEFAULT_RUNTIME_CONFIG
    });

    let bytes: Vec<u8> = [
        encode_add_imm(1, 0, 1, 0, 0), // x0 += 1
        encode_cbnz(1, 3, 0),          // if x0 != 0, jump to 0x1010
        0xD503_201F,                   // padding at 0x1008
        0xD503_201F,                   // padding at 0x100c
        encode_b(-4),                  // 0x1010 -> 0x1000
    ]
    .into_iter()
    .flat_map(u32::to_le_bytes)
    .collect();
    engine.load_bytes(0x1000, &bytes);
    engine.set_pc(0x1000);
    engine
        .jit_trace_cache
        .as_mut()
        .expect("trace cache")
        .insert(make_guard_test_trace(0x1000, 0x1010));

    {
        let a64 = engine
            .session
            .aarch64_mut()
            .and_then(Aarch64Core::state_mut)
            .expect("functional AArch64 state");
        a64.write_x(0, 1);
    }

    let stop = engine.run_jit(u64::from(GUARD_MISS_THRESHOLD) * 3 + 2);
    assert_eq!(stop, crate::StopReason::Quantum);

    let a64 = engine
        .session
        .aarch64()
        .and_then(Aarch64Core::state)
        .expect("functional AArch64 state");
    assert_eq!(a64.read_x(0), u64::from(GUARD_MISS_THRESHOLD) + 2);

    let stats = engine.jit_perf_stats();
    assert_eq!(stats.traces_executed, u64::from(GUARD_MISS_THRESHOLD));
    assert_eq!(stats.trace_guard_exits, u64::from(GUARD_MISS_THRESHOLD));
    assert_eq!(stats.trace_retired, 1);
    assert_eq!(stats.trace_cache_entries, 0);
    assert!(
        stats.blocks_executed >= u64::from(GUARD_MISS_THRESHOLD) + 1,
        "block JIT should take over after trace retirement"
    );
}

#[cfg(all(feature = "jit-dynasm", not(feature = "jit-stencil")))]
#[test]
fn jit_trace_dispatch_executes_fused_subs_bne_fallthrough() {
    let mut engine = HelmEngine::new(
        Isa::AArch64,
        ExecMode::Functional,
        VirtualTiming::new(1.0),
        0,
        0x2000,
    );
    engine.set_jit(true);
    engine.set_jit_runtime_config(JitRuntimeConfig {
        trace_dispatch_enabled: true,
        ..DEFAULT_RUNTIME_CONFIG
    });

    let bytes: Vec<u8> = [
        encode_subs_imm(1, 0, 1, 0, 0),
        encode_b_cond(1, 3),
        encode_add_imm(1, 0, 1, 1, 1),
    ]
    .into_iter()
    .flat_map(u32::to_le_bytes)
    .collect();
    engine.load_bytes(0x1000, &bytes);
    engine.set_pc(0x1000);
    engine
        .jit_trace_cache
        .as_mut()
        .expect("trace cache")
        .insert(make_fused_subs_bne_trace(0x1000, 0x1010));

    {
        let a64 = engine
            .session
            .aarch64_mut()
            .and_then(Aarch64Core::state_mut)
            .expect("functional AArch64 state");
        a64.write_x(0, 1);
        a64.write_x(1, 0);
    }

    let stop = engine.run_jit(3);
    assert_eq!(stop, crate::StopReason::Quantum);

    let a64 = engine
        .session
        .aarch64()
        .and_then(Aarch64Core::state)
        .expect("functional AArch64 state");
    assert_eq!(a64.read_x(0), 0);
    assert_eq!(a64.read_x(1), 1);
    assert_eq!(a64.pc, 0x100c);

    let stats = engine.jit_perf_stats();
    assert_eq!(stats.traces_executed, 1);
    assert_eq!(stats.trace_guard_exits, 0);
    assert_eq!(stats.blocks_executed, 0);
}
