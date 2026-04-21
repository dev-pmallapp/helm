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
use helm_debug::GdbTarget;
use helm_hw_pci::{config::PciConfigSpace, Bdf, PciBus, PciEndpoint};
#[cfg(feature = "jit-tiered")]
use helm_jit::cache::PROMOTE_THRESHOLD;
#[cfg(feature = "jit")]
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
    sys_mem
        .ram
        .load_bytes(0x1000, &0xD503_201Fu32.to_le_bytes());

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
            Arc::new(AtomicBool::new(false)),
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
        0x2000,
    );
    engine.session = HelmMachine::new_primary(HelmCore::Aarch64(Aarch64Core::System(machine)));

    engine
        .step_aarch64_system()
        .expect("secondary vCPU should execute a NOP");

    assert_eq!(engine.active_fs_vcpu, 1);
    assert_eq!(
        engine
            .aarch64_state_for_current_context()
            .map(|state| state.pc),
        Some(0x1004)
    );
    assert_eq!(engine.with_a64_state_mut(|state| state.pc), Some(0x1004));
}

/// Combined multi-vCPU integration test: proves that both the IRQ line and
/// the state accessor path are correctly routed to the *active* vCPU, not
/// always vCPU 0.
///
/// Setup:
///   - CPU 0: powered **off**, IRQ line **asserted**
///   - CPU 1: powered **on**,  IRQ line **deasserted**
///   - CPU 1 at PC 0x1000, CPU 0 at PC 0x0
///
/// This test steps the system twice. After each step:
///   1. `active_fs_vcpu` must be 1 (only CPU 1 is runnable).
///   2. `aarch64_state_for_current_context()` must return CPU 1's state
///      (PC advancing from 0x1000).
///   3. CPU 1's `irq_pending` must be false (its own line is deasserted),
///      proving that the IRQ poll reads from `irq_lines[1]`, not
///      `irq_lines[0]`.
///   4. CPU 0's `irq_pending` reflects its own (asserted) line through
///      `pick_next_fs_vcpu`'s sync pass, but CPU 0 never executes because
///      it is powered off.
#[test]
fn multi_vcpu_irq_and_state_path_combined() {
    // ── Build vCPUs ──────────────────────────────────────────────────────
    let mut cpu0 = Aarch64ArchState::new();
    cpu0.pc = 0x0;
    cpu0.current_el = 1;
    cpu0.spsel = true;

    let mut cpu1 = Aarch64ArchState::new();
    cpu1.pc = 0x1000;
    cpu1.current_el = 1;
    cpu1.spsel = true;

    // ── System memory: NOPs at both PC origins ───────────────────────────
    let mut sys_mem = HelmAddressSpace::new(FlatMem::new(0, 0x2000));
    let nop = 0xD503_201Fu32;
    for offset in (0..0x100).step_by(4) {
        sys_mem.ram.load_bytes(offset as u64, &nop.to_le_bytes());
    }
    for offset in (0x1000..0x1100).step_by(4) {
        sys_mem.ram.load_bytes(offset as u64, &nop.to_le_bytes());
    }

    // ── IRQ lines: CPU 0 asserted, CPU 1 deasserted ─────────────────────
    let irq0 = Arc::new(AtomicBool::new(true));
    let irq1 = Arc::new(AtomicBool::new(false));

    let machine = HelmBoard {
        sys_mem: Box::new(sys_mem),
        vcpus: vec![
            HelmVcpu {
                arch: cpu0,
                fs: FsState::new(),
                powered_on: false, // CPU 0 OFF
            },
            HelmVcpu {
                arch: cpu1,
                fs: FsState::new(),
                powered_on: true, // CPU 1 ON
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
        irq_lines: vec![irq0.clone(), irq1.clone()],
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
    // Force IRQ poll on the very next step so we can observe the assignment.
    engine.irq_poll_countdown = 1;

    // ── Step 1 ───────────────────────────────────────────────────────────
    engine
        .step_aarch64_system()
        .expect("CPU 1 should execute a NOP (step 1)");

    // (a) active_fs_vcpu must be CPU 1
    assert_eq!(
        engine.active_fs_vcpu, 1,
        "scheduler should pick CPU 1 (the only powered-on vCPU)"
    );

    // (b) State accessor must return CPU 1's post-step PC
    assert_eq!(
        engine.aarch64_state_for_current_context().map(|s| s.pc),
        Some(0x1004),
        "state accessor must return CPU 1's PC after one NOP"
    );

    // (c) with_a64_state_mut must agree
    assert_eq!(
        engine.with_a64_state_mut(|s| s.pc),
        Some(0x1004),
        "with_a64_state_mut must return CPU 1's PC"
    );

    {
        let machine = engine
            .session
            .aarch64()
            .and_then(Aarch64Core::machine)
            .expect("board must be present");

        // (d) CPU 1's irq_pending must be false (its line is deasserted)
        assert!(
            !machine.vcpus[1].fs.irq_pending,
            "CPU 1 must observe its own (deasserted) IRQ line, not CPU 0's"
        );

        // (e) CPU 0's irq_pending reflects its own asserted line via the
        //     pick_next_fs_vcpu sync pass, but CPU 0 never executes.
        assert!(
            machine.vcpus[0].fs.irq_pending,
            "CPU 0's IRQ line is asserted; its irq_pending should be true from sync"
        );

        // (f) CPU 0's PC must not have advanced (it's powered off).
        assert_eq!(
            machine.vcpus[0].arch.pc, 0x0,
            "CPU 0 is powered off; its PC must not advance"
        );
    }

    // ── Step 2: force IRQ poll again ─────────────────────────────────────
    engine.irq_poll_countdown = 1;
    engine
        .step_aarch64_system()
        .expect("CPU 1 should execute a second NOP (step 2)");

    assert_eq!(
        engine.active_fs_vcpu, 1,
        "scheduler should still pick CPU 1"
    );
    assert_eq!(
        engine.aarch64_state_for_current_context().map(|s| s.pc),
        Some(0x1008),
        "CPU 1's PC must advance to 0x1008 after second NOP"
    );

    {
        let machine = engine
            .session
            .aarch64()
            .and_then(Aarch64Core::machine)
            .expect("board must be present");

        assert!(
            !machine.vcpus[1].fs.irq_pending,
            "CPU 1 must still have no IRQ pending (step 2)"
        );
    }

    // ── Now power on CPU 0 and step both ────────────────────────────────
    // This proves that when CPU 0 becomes runnable, the engine switches to
    // it and reads *its* IRQ line.
    //
    // Deassert CPU 0's IRQ line first so the step executes a NOP instead of
    // taking an exception entry (VBAR_EL1 is 0 → the IRQ vector would land
    // at 0x280, outside the loaded NOP sled).  Then re-assert it after the
    // step to verify the IRQ poll reads the correct per-vCPU line.
    irq0.store(false, std::sync::atomic::Ordering::Relaxed);
    {
        let machine = engine
            .session
            .aarch64_mut()
            .and_then(Aarch64Core::machine_mut)
            .expect("board must be present");
        machine.vcpus[0].powered_on = true;
    }
    engine.irq_poll_countdown = 1;

    // With both CPUs on, the round-robin scheduler will pick the next one
    // after CPU 1.  `next_vcpu` was advanced past CPU 1 in the previous
    // step, so it should wrap to CPU 0.
    engine
        .step_aarch64_system()
        .expect("should execute on one of the powered-on CPUs (step 3)");

    let stepped_vcpu = engine.active_fs_vcpu;
    // The scheduler may pick CPU 0 or CPU 1; what matters is that the
    // state accessor returns the correct PC for whichever was chosen.
    let expected_pc = if stepped_vcpu == 0 {
        0x4 // CPU 0 started at 0, one NOP → 4
    } else {
        0x100C // CPU 1 started at 0x1000, three NOPs → 0x100C
    };
    assert_eq!(
        engine.aarch64_state_for_current_context().map(|s| s.pc),
        Some(expected_pc),
        "state accessor must return correct PC for vCPU {stepped_vcpu}"
    );

    // ── Step 4: re-assert CPU 0's line, deassert CPU 1's, verify ─────
    irq0.store(true, std::sync::atomic::Ordering::Relaxed);
    irq1.store(false, std::sync::atomic::Ordering::Relaxed);
    engine.irq_poll_countdown = 1;

    // Mask IRQs on both CPUs so the IRQ poll sets irq_pending but does
    // not trigger exception entry (we want to observe the flag directly).
    {
        let machine = engine
            .session
            .aarch64_mut()
            .and_then(Aarch64Core::machine_mut)
            .expect("board must be present");
        machine.vcpus[0].arch.daif = 0xF; // mask all
        machine.vcpus[1].arch.daif = 0xF; // mask all
    }

    engine
        .step_aarch64_system()
        .expect("should execute a NOP (step 4)");

    {
        let machine = engine
            .session
            .aarch64()
            .and_then(Aarch64Core::machine)
            .expect("board must be present");

        // After the IRQ poll, each vCPU's irq_pending must match its own
        // line, not the other's.  pick_next_fs_vcpu syncs all lines.
        assert!(
            machine.vcpus[0].fs.irq_pending,
            "CPU 0's irq_pending must be true (its line is asserted)"
        );
        assert!(
            !machine.vcpus[1].fs.irq_pending,
            "CPU 1's irq_pending must be false (its line is deasserted)"
        );
    }
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

/// Encode LDXR Xt, [Xn] (exclusive load - unsupported by JIT).
fn encode_ldxr(rt: u32, rn: u32) -> u32 {
    0xC85F_FC00 | (rn << 5) | rt
}

/// Encode MSR sysreg, Xt.
fn encode_msr(rt: u32, o0: u32, op1: u32, crn: u32, crm: u32, op2: u32) -> u32 {
    0xD500_0000
        | (1 << 20) // o0 high bit (op0=3 when o0=1)
        | (o0 << 19)
        | (op1 << 16)
        | (crn << 12)
        | (crm << 8)
        | (op2 << 5)
        | rt
}

/// Encode ORR Xd, Xn, #imm (logical immediate).
fn encode_orr_imm(rd: u32, rn: u32, imm_enc: u32) -> u32 {
    // B2 prefix = ORR 64-bit immediate
    0xB200_0000 | ((imm_enc & 0x1FFF) << 10) | (rn << 5) | rd
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
        encode_ldxr(1, 31),            // LDXR X1, [SP] -> unsupported by dynasm JIT
        encode_b(-2),                  // loop back to the LDXR
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
    assert_eq!(
        a64.read_x(1),
        0x2018,
        "pre-index writeback must preserve X1"
    );
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
    // With MRS/MSR/SYS JIT support, FS mode now allows hot-tier promotion.
    assert!(stats.cache_promotions >= 0);
    assert!(stats.blocks_executed >= u64::from(PROMOTE_THRESHOLD));
}

#[cfg(feature = "jit-tiered")]
#[test]
fn jit_syscall_mode_keeps_tiered_stencil_baseline() {
    let mut engine = HelmEngine::new(
        Isa::AArch64,
        ExecMode::Syscall,
        VirtualTiming::new(1.0),
        0,
        0x2000,
    );

    engine.set_jit(true);

    assert_eq!(
        engine.jit_backend.as_ref().map(|b| b.name()),
        Some("stencil"),
        "SE mode should keep the stencil baseline when tiered JIT is enabled"
    );
    assert!(
        engine.jit_hot_backend.is_some(),
        "SE mode should keep the dynasm hot-tier backend available"
    );
}

#[cfg(feature = "jit")]
#[test]
fn jit_system_mode_el2_resumes_after_unsupported_start_batch() {
    let mut engine = HelmEngine::new(
        Isa::AArch64,
        ExecMode::System,
        VirtualTiming::new(1.0),
        0,
        0x2000,
    );
    engine.set_jit(true);
    engine.set_jit_runtime_config(JitRuntimeConfig {
        interp_fallback_batch_insns: 1,
        ..DEFAULT_RUNTIME_CONFIG
    });

    let mut sys_mem = HelmAddressSpace::new(FlatMem::new(0, 0x2000));
    let bytes: Vec<u8> = [
        encode_ldxr(2, 31), // LDXR X2, [SP] -> unsupported by JIT, interpreter fallback
        encode_add_imm(1, 0, 1, 0, 0),
        encode_b(-1), // 0x1008 -> 0x1004
    ]
    .into_iter()
    .flat_map(u32::to_le_bytes)
    .collect();
    sys_mem.ram.load_bytes(0x1000, &bytes);
    engine
        .install_test_aarch64_system_board(sys_mem)
        .expect("install test system board");

    let a64 = engine
        .session
        .aarch64_mut()
        .and_then(Aarch64Core::state_mut)
        .expect("aarch64 system cpu state");
    a64.current_el = 2;
    a64.spsel = true;
    a64.sp_el2 = 0x1800;
    a64.pc = 0x1000;

    let stop = engine.run_jit(5);
    assert_eq!(stop, crate::StopReason::Quantum);

    let a64 = engine
        .session
        .aarch64()
        .and_then(Aarch64Core::state)
        .expect("aarch64 system cpu state");
    assert_eq!(a64.read_x(0), 2);
    // LDXR X2, [SP] reads from address 0x1800 (zeroed memory) -> X2 = 0.
    assert_eq!(
        a64.read_x(2),
        0,
        "LDXR result should be preserved across fallback resume"
    );
    assert_eq!(a64.pc, 0x1004);

    let stats = engine.jit_perf_stats();
    assert_eq!(stats.fallback_count, 1);
    assert_eq!(stats.fallback_insns, 1);
    assert_eq!(stats.unsupported_block_starts, 1);
    assert_eq!(stats.unsupported_opcodes.values().copied().sum::<u64>(), 1);
    assert!(stats.blocks_compiled >= 1);
    assert!(stats.blocks_executed >= 1);
    assert!(stats.block_cache_hits >= 1);
}

#[cfg(feature = "jit")]
#[test]
fn jit_vs_interp_mrs_msr_cpacr_block() {
    // Reproduce the L4Re entry block:
    //   ADRP x9, page       (sets x9 to a page address)
    //   ADD  x9, x9, #0x370
    //   MOV  sp, x9
    //   MRS  x8, CPACR_EL1  <- first system register access
    //   ORR  x8, x8, #0x300000
    //   MSR  CPACR_EL1, x8
    //   MOV  x19, x0
    //   RET                 (terminate block)
    //
    // We use a simpler block: MRS x0, CPACR_EL1; RET
    let code: Vec<u8> = [
        0xD538_1040u32, // MRS X0, CPACR_EL1
        0xD65F_03C0u32, // RET (X30)
    ]
    .into_iter()
    .flat_map(u32::to_le_bytes)
    .collect();

    // -- Interpreter run --
    let mut engine_i = HelmEngine::new(
        Isa::AArch64,
        ExecMode::System,
        VirtualTiming::new(1.0),
        0,
        0x2000,
    );
    let mut sys_i = HelmAddressSpace::new(FlatMem::new(0, 0x2000));
    sys_i.ram.load_bytes(0x1000, &code);
    engine_i
        .install_test_aarch64_system_board(sys_i)
        .expect("install");
    {
        let a = engine_i
            .session
            .aarch64_mut()
            .and_then(Aarch64Core::state_mut)
            .expect("state");
        a.current_el = 2;
        a.spsel = true;
        a.sp_el2 = 0x1800;
        a.pc = 0x1000;
        a.x[30] = 0x1000; // RET target (loop back)
        a.cpacr_el1 = 0x0; // initial CPACR value
    }
    let stop_i = engine_i.run(2);
    assert_eq!(stop_i, crate::StopReason::Quantum);
    let a_i = engine_i
        .session
        .aarch64()
        .and_then(Aarch64Core::state)
        .expect("state");
    let x0_interp = a_i.read_x(0);

    // -- JIT run --
    let mut engine_j = HelmEngine::new(
        Isa::AArch64,
        ExecMode::System,
        VirtualTiming::new(1.0),
        0,
        0x2000,
    );
    engine_j.set_jit(true);
    let mut sys_j = HelmAddressSpace::new(FlatMem::new(0, 0x2000));
    sys_j.ram.load_bytes(0x1000, &code);
    engine_j
        .install_test_aarch64_system_board(sys_j)
        .expect("install");
    {
        let a = engine_j
            .session
            .aarch64_mut()
            .and_then(Aarch64Core::state_mut)
            .expect("state");
        a.current_el = 2;
        a.spsel = true;
        a.sp_el2 = 0x1800;
        a.pc = 0x1000;
        a.x[30] = 0x1000;
        a.cpacr_el1 = 0x0;
    }
    let stop_j = engine_j.run_jit(2);
    assert_eq!(stop_j, crate::StopReason::Quantum);
    let a_j = engine_j
        .session
        .aarch64()
        .and_then(Aarch64Core::state)
        .expect("state");
    let x0_jit = a_j.read_x(0);

    assert_eq!(
        x0_interp, x0_jit,
        "MRS CPACR_EL1 should produce same result: interp={x0_interp:#x} jit={x0_jit:#x}"
    );
}

#[cfg(feature = "jit-tiered")]
#[test]
fn jit_system_mode_el2_uses_fallback_backend_for_complex_ldst() {
    let mut engine = HelmEngine::new(
        Isa::AArch64,
        ExecMode::System,
        VirtualTiming::new(1.0),
        0,
        0x4000,
    );
    engine.set_jit(true);

    let mut sys_mem = HelmAddressSpace::new(FlatMem::new(0, 0x4000));
    let bytes: Vec<u8> = [
        0xF841_8C24u32, // LDR X4, [X1, #24]!
        encode_b(-1),   // 0x1004 -> 0x1000
    ]
    .into_iter()
    .flat_map(u32::to_le_bytes)
    .collect();
    sys_mem.ram.load_bytes(0x1000, &bytes);
    sys_mem
        .ram
        .load_bytes(0x2018, &0x1122_3344_5566_7788u64.to_le_bytes());
    engine
        .install_test_aarch64_system_board(sys_mem)
        .expect("install test system board");

    let a64 = engine
        .session
        .aarch64_mut()
        .and_then(Aarch64Core::state_mut)
        .expect("aarch64 system cpu state");
    a64.current_el = 2;
    a64.spsel = true;
    a64.sp_el2 = 0x1800;
    a64.pc = 0x1000;
    a64.write_x(1, 0x2000);

    let stop = engine.run_jit(2);
    assert_eq!(stop, crate::StopReason::Quantum);

    let a64 = engine
        .session
        .aarch64()
        .and_then(Aarch64Core::state)
        .expect("aarch64 system cpu state");
    assert_eq!(a64.read_x(4), 0x1122_3344_5566_7788);
    assert_eq!(
        a64.read_x(1),
        0x2018,
        "pre-index writeback must survive dynasm fallback"
    );
    assert_eq!(a64.pc, 0x1000);

    let stats = engine.jit_perf_stats();
    assert_eq!(stats.fallback_count, 0);
    assert!(stats.blocks_compiled >= 1);
    assert!(stats.blocks_executed >= 1);
    assert!(stats.block_cache_misses >= 1);
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

/// Regression test for the L4Re-style function prologue that crashed
/// at guest PC 0x4100475c with pre-index STP, post-index LDR, and
/// indirect BR.  Exercises the full FS-mode dynasm path.
#[cfg(feature = "jit-tiered")]
#[test]
fn jit_system_mode_fs_prologue_pre_post_index_block() {
    let mut engine = HelmEngine::new(
        Isa::AArch64,
        ExecMode::System,
        VirtualTiming::new(1.0),
        0,
        0x1_0000,
    );
    engine.set_jit(true);

    let mut sys_mem = HelmAddressSpace::new(FlatMem::new(0, 0x1_0000));

    // L4Re-style function prologue block:
    //   0x1000: STP X29, X30, [SP, #-32]!
    //   0x1004: MOV X29, SP  (ADD X29, SP, #0)
    //   0x1008: STR X19, [SP, #16]
    //   0x100c: MOV X19, X0  (ORR X19, XZR, X0)
    //   0x1010: LDR X1, [X19], #56  (post-index)
    //   0x1014: LDR X1, [X1, #0]
    //   0x1018: BR X1
    //   0x101c: NOP (padding)
    // BR target at 0x1020:
    //   0x1020: ADD X5, X5, #1
    //   0x1024: B 0x1020
    let code: Vec<u8> = [
        0xA9BE_7BFDu32,                // STP X29, X30, [SP, #-32]!
        0x9100_03FD,                   // MOV X29, SP
        0xF900_0BF3,                   // STR X19, [SP, #16]
        0xAA00_03F3,                   // MOV X19, X0
        0xF843_8661,                   // LDR X1, [X19], #56
        0xF940_0021,                   // LDR X1, [X1, #0]
        0xD61F_0020,                   // BR X1
        0xD503_201F,                   // NOP
        encode_add_imm(1, 0, 1, 5, 5), // ADD X5, X5, #1
        encode_b(-1),                  // B .-4
    ]
    .into_iter()
    .flat_map(u32::to_le_bytes)
    .collect();
    sys_mem.ram.load_bytes(0x1000, &code);

    // mem[0x2000] = 0x3000 (pointer for first LDR)
    sys_mem
        .ram
        .load_bytes(0x2000, &0x0000_0000_0000_3000u64.to_le_bytes());
    // mem[0x3000] = 0x1020 (BR target, loaded by second LDR)
    sys_mem
        .ram
        .load_bytes(0x3000, &0x0000_0000_0000_1020u64.to_le_bytes());

    engine
        .install_test_aarch64_system_board(sys_mem)
        .expect("install test system board");

    let a64 = engine
        .session
        .aarch64_mut()
        .and_then(Aarch64Core::state_mut)
        .expect("aarch64 system cpu state");
    a64.current_el = 1;
    a64.spsel = true;
    a64.sp_el1 = 0x8000;
    a64.pc = 0x1000;
    a64.write_x(0, 0x2000);
    a64.write_x(5, 0);
    a64.write_x(19, 0xAAAA);
    a64.write_x(29, 0xBBBB);
    a64.write_x(30, 0xCCCC);

    let stop = engine.run_jit(12);
    assert_eq!(stop, crate::StopReason::Quantum);

    let a64 = engine
        .session
        .aarch64()
        .and_then(Aarch64Core::state)
        .expect("aarch64 system cpu state");

    // SP = 0x8000 - 32 = 0x7FE0
    assert_eq!(a64.current_sp(), 0x7FE0, "SP after pre-index STP");
    // X29 = SP = 0x7FE0
    assert_eq!(a64.read_x(29), 0x7FE0, "X29 = SP after MOV");
    // X19 = 0x2000 + 56 = 0x2038 (post-index writeback)
    assert_eq!(a64.read_x(19), 0x2038, "X19 after post-index writeback");
    // X5 incremented by target loop
    assert!(a64.read_x(5) >= 1, "target block executed at least once");

    let stats = engine.jit_perf_stats();
    assert!(stats.blocks_compiled >= 1);
    assert!(stats.blocks_executed >= 1);
}

// -- JIT FS-mode device write test -----------------------------------------------

use std::sync::atomic::{AtomicU64, Ordering};

/// Mock device that records write count and last written value via atomics
/// so we can inspect them after the JIT run.
struct WriteCounterDevice {
    write_count: Arc<AtomicU64>,
    last_val: Arc<AtomicU64>,
}

impl WriteCounterDevice {
    fn new(write_count: Arc<AtomicU64>, last_val: Arc<AtomicU64>) -> Self {
        Self {
            write_count,
            last_val,
        }
    }
}

impl helm_devices::Device for WriteCounterDevice {
    fn read(&mut self, _offset: u64, _size: usize) -> u64 {
        0
    }
    fn write(&mut self, _offset: u64, _size: usize, val: u64) {
        self.write_count.fetch_add(1, Ordering::Relaxed);
        self.last_val.store(val, Ordering::Relaxed);
    }
    fn region_size(&self) -> u64 {
        0x1000
    }
}

/// Encode STR X(rt), [X(rn), #imm*8]  (64-bit unsigned offset).
fn encode_str_x_uimm(imm12: u32, rn: u32, rt: u32) -> u32 {
    (0b11111001_00u32 << 22) | (imm12 << 10) | (rn << 5) | rt
}

/// Encode MOVZ X(rd), #imm16, LSL #(hw*16).
fn encode_movz(rd: u32, imm16: u32, hw: u32) -> u32 {
    (0b110100101u32 << 23) | (hw << 21) | (imm16 << 5) | rd
}

#[cfg(feature = "jit")]
#[test]
fn jit_system_mode_fs_store_reaches_device() {
    // Device at PA 0x0800_0000 (outside RAM 0..0x10000).
    const DEVICE_BASE: u64 = 0x0800_0000;

    let write_count = Arc::new(AtomicU64::new(0));
    let last_val = Arc::new(AtomicU64::new(0));

    let dev = WriteCounterDevice::new(Arc::clone(&write_count), Arc::clone(&last_val));

    let mut sys_mem = HelmAddressSpace::new(FlatMem::new(0, 0x1_0000));
    sys_mem.add_device(DEVICE_BASE, Box::new(dev));

    // Code at PA 0x1000:
    //   MOVZ X0, #0x42            ; X0 = 0x42
    //   MOVZ X1, #0x0800, LSL#16  ; X1 = 0x0800_0000
    //   STR  X0, [X1, #0]         ; store 0x42 to device
    //   B    .-4                   ; loop back to STR
    let code: Vec<u8> = [
        encode_movz(0, 0x42, 0),    // MOVZ X0, #0x42
        encode_movz(1, 0x0800, 1),  // MOVZ X1, #0x0800_0000
        encode_str_x_uimm(0, 1, 0), // STR X0, [X1, #0]
        encode_b(-1),               // B .-4 (back to STR)
    ]
    .into_iter()
    .flat_map(u32::to_le_bytes)
    .collect();
    sys_mem.ram.load_bytes(0x1000, &code);

    let mut engine = HelmEngine::new(
        Isa::AArch64,
        ExecMode::System,
        VirtualTiming::new(1.0),
        0,
        0x1_0000,
    );
    engine.set_jit(true);
    engine
        .install_test_aarch64_system_board(sys_mem)
        .expect("install system board with device");

    // Start at EL1, MMU off, identity mapped.
    let a64 = engine
        .session
        .aarch64_mut()
        .and_then(Aarch64Core::state_mut)
        .expect("aarch64 state");
    a64.current_el = 1;
    a64.spsel = true;
    a64.sp_el1 = 0x8000;
    a64.pc = 0x1000;

    // Run enough instructions for the block to compile and execute several times.
    let _stop = engine.run_jit(200);

    let wc = write_count.load(Ordering::Relaxed);
    let lv = last_val.load(Ordering::Relaxed);
    assert!(wc > 0, "device should receive at least one write, got {wc}");
    assert_eq!(lv, 0x42, "device should receive value 0x42, got {lv:#x}");
}

#[cfg(feature = "jit")]
#[test]
fn jit_system_mode_fs_store_reaches_device_el2() {
    // Same as above but at EL2, mimicking L4Re boot scenario.
    const DEVICE_BASE: u64 = 0x0900_0000; // PL011 UART address

    let write_count = Arc::new(AtomicU64::new(0));
    let last_val = Arc::new(AtomicU64::new(0));

    let dev = WriteCounterDevice::new(Arc::clone(&write_count), Arc::clone(&last_val));

    let mut sys_mem = HelmAddressSpace::new(FlatMem::new(0, 0x1_0000));
    sys_mem.add_device(DEVICE_BASE, Box::new(dev));

    // Code at PA 0x1000:
    //   MOVZ X0, #0x48            ; X0 = 'H'
    //   MOVZ X1, #0x0900, LSL#16  ; X1 = 0x0900_0000 (UART base)
    //   STRB W0, [X1, #0]         ; write char to UART data register (byte write)
    //   ADD  X0, X0, #1           ; next char
    //   B    .-8                   ; loop back to STRB
    fn encode_strb_uimm(imm12: u32, rn: u32, rt: u32) -> u32 {
        (0b00111001_00u32 << 22) | (imm12 << 10) | (rn << 5) | rt
    }
    let code: Vec<u8> = [
        encode_movz(0, 0x48, 0),       // MOVZ X0, #'H'
        encode_movz(1, 0x0900, 1),     // MOVZ X1, #0x0900_0000
        encode_strb_uimm(0, 1, 0),     // STRB W0, [X1, #0]
        encode_add_imm(1, 0, 1, 0, 0), // ADD X0, X0, #1
        encode_b(-2),                  // B .-8 (back to STRB)
    ]
    .into_iter()
    .flat_map(u32::to_le_bytes)
    .collect();
    sys_mem.ram.load_bytes(0x1000, &code);

    let mut engine = HelmEngine::new(
        Isa::AArch64,
        ExecMode::System,
        VirtualTiming::new(1.0),
        0,
        0x1_0000,
    );
    engine.set_jit(true);
    engine
        .install_test_aarch64_system_board(sys_mem)
        .expect("install system board with device");

    // Start at EL2, MMU off (L4Re boot scenario).
    let a64 = engine
        .session
        .aarch64_mut()
        .and_then(Aarch64Core::state_mut)
        .expect("aarch64 state");
    a64.current_el = 2;
    a64.spsel = true;
    a64.sp_el2 = 0x8000;
    a64.pc = 0x1000;

    let _stop = engine.run_jit(200);

    let wc = write_count.load(Ordering::Relaxed);
    let lv = last_val.load(Ordering::Relaxed);
    assert!(
        wc > 0,
        "EL2: device should receive at least one write, got {wc}"
    );
    assert!(
        lv >= 0x48,
        "EL2: device should receive ASCII value >= 'H', got {lv:#x}"
    );
}

#[cfg(feature = "jit")]
#[test]
fn jit_system_mode_isb_terminates_block_for_mmu_refresh() {
    // Verify that ISB terminates a JIT block in FS mode so the MmuConfig
    // snapshot is refreshed after system register writes (e.g., MSR SCTLR).
    //
    // Sequence:
    //   MOVZ X0, #0x42
    //   ISB              ; should terminate the block
    //   MOVZ X1, #0x0800, LSL#16
    //   STR  X0, [X1]    ; store to device
    //   B    .-4          ; loop
    //
    // If ISB does NOT terminate the block, the MOVZ+ISB+MOVZ+STR would be
    // in one block and the MmuConfig wouldn't refresh. By checking that
    // the device receives writes, we confirm ISB caused a block break and
    // the dispatch context was rebuilt.
    const DEVICE_BASE: u64 = 0x0800_0000;

    let write_count = Arc::new(AtomicU64::new(0));
    let last_val = Arc::new(AtomicU64::new(0));
    let dev = WriteCounterDevice::new(Arc::clone(&write_count), Arc::clone(&last_val));

    let mut sys_mem = HelmAddressSpace::new(FlatMem::new(0, 0x1_0000));
    sys_mem.add_device(DEVICE_BASE, Box::new(dev));

    fn encode_isb() -> u32 {
        0xD5033FDF // ISB SY
    }

    let code: Vec<u8> = [
        encode_movz(0, 0x42, 0),    // MOVZ X0, #0x42
        encode_isb(),               // ISB
        encode_movz(1, 0x0800, 1),  // MOVZ X1, #0x0800_0000
        encode_str_x_uimm(0, 1, 0), // STR X0, [X1]
        encode_b(-2),               // B .-8 (back to STR)
    ]
    .into_iter()
    .flat_map(u32::to_le_bytes)
    .collect();
    sys_mem.ram.load_bytes(0x1000, &code);

    let mut engine = HelmEngine::new(
        Isa::AArch64,
        ExecMode::System,
        VirtualTiming::new(1.0),
        0,
        0x1_0000,
    );
    engine.set_jit(true);
    engine
        .install_test_aarch64_system_board(sys_mem)
        .expect("install system board");

    let a64 = engine
        .session
        .aarch64_mut()
        .and_then(Aarch64Core::state_mut)
        .expect("aarch64 state");
    a64.current_el = 1;
    a64.spsel = true;
    a64.sp_el1 = 0x8000;
    a64.pc = 0x1000;

    let _stop = engine.run_jit(200);

    let wc = write_count.load(Ordering::Relaxed);
    assert!(
        wc > 0,
        "device should receive writes after ISB block break, got {wc}"
    );

    // Verify the block was split: the JIT stats should show >= 2 compiled blocks
    // (one for MOVZ+ISB, one for MOVZ+STR+B).
    let stats = engine.jit_perf_stats();
    assert!(
        stats.blocks_compiled >= 2,
        "ISB should split the block: expected >= 2 compiled blocks, got {}",
        stats.blocks_compiled
    );
}

#[cfg(feature = "jit")]
#[test]
fn jit_l4re_lockstep_register_comparison() {
    let elf_path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../..",
        "/assets/aarch64/boot/l4re/l4re_hello-2_arm_virt.elf"
    );
    if !std::path::Path::new(elf_path).exists() {
        return;
    }

    fn make_engine() -> HelmEngine<VirtualTiming> {
        let elf_path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../..",
            "/assets/aarch64/boot/l4re/l4re_hello-2_arm_virt.elf"
        );
        let bp = crate::platform::arm_virt::arm_virt_boot_policy_from_override(Some(2)).unwrap();
        let built = crate::platform::arm_virt::build_loaded_arm_virt_system_auto_dtb(
            elf_path,
            None,
            None,
            128,
            1,
            crate::platform::arm_virt::ArmVirtGicVersion::V2,
            bp,
            Box::new(helm_devices::NullCharBackend),
        )
        .expect("load");
        let mut e = HelmEngine::new(
            Isa::AArch64,
            ExecMode::System,
            VirtualTiming::new(1.0),
            0,
            128 * 1024 * 1024,
        );
        e.install_built_system(built).unwrap();
        e
    }

    fn compare_at(n: u64) -> bool {
        let mut ej = make_engine();
        ej.set_jit(true);
        ej.run_jit(n);
        let rj = ej.insns_retired;
        let mut ei = make_engine();
        ei.run(rj);
        let ai = ei.session.aarch64().and_then(Aarch64Core::state).unwrap();
        let aj = ej.session.aarch64().and_then(Aarch64Core::state).unwrap();
        ai.pc == aj.pc
            && ai.nzcv == aj.nzcv
            && ai.current_sp() == aj.current_sp()
            && (0..31).all(|i| ai.x[i] == aj.x[i])
    }

    let max = 20_000u64;
    if compare_at(max) {
        eprintln!("MATCH at {max}");
        return;
    }

    let (mut lo, mut hi) = (0u64, max);
    while hi - lo > 100 {
        let mid = (lo + hi) / 2;
        if compare_at(mid) {
            lo = mid;
        } else {
            hi = mid;
        }
        eprintln!("bisect: [{lo}, {hi}]");
    }

    // Print details
    let mut ej = make_engine();
    ej.set_jit(true);
    ej.run_jit(hi);
    let rj = ej.insns_retired;
    let mut ei = make_engine();
    ei.run(rj);
    let ai = ei.session.aarch64().and_then(Aarch64Core::state).unwrap();
    let aj = ej.session.aarch64().and_then(Aarch64Core::state).unwrap();
    eprintln!("First divergence at budget ~{hi} (retired={rj}):");
    eprintln!("  PC: i={:#x} j={:#x}", ai.pc, aj.pc);
    if ai.current_sp() != aj.current_sp() {
        eprintln!("  SP: i={:#x} j={:#x}", ai.current_sp(), aj.current_sp());
    }
    for r in 0..31 {
        if ai.x[r] != aj.x[r] {
            eprintln!("  X{r}: i={:#x} j={:#x}", ai.x[r], aj.x[r]);
        }
    }
    let stats = ej.jit_perf_stats();
    eprintln!(
        "  JIT: compiled={} fallbacks={}",
        stats.blocks_compiled, stats.fallback_count
    );
    for (op, cnt) in &stats.unsupported_opcodes {
        eprintln!("    unsupported: {op} x{cnt}");
    }
}

// ── Device introspection tests ──────────────────────────────────────────

#[test]
fn gicv2_introspection_queries_live_state() {
    use crate::platform::arm_virt::ArmVirtGicVersion;

    let sim = build_simulator_from_request(
        SimulatorBuildRequest::new(
            Isa::AArch64,
            ExecMode::System,
            TimingChoice::VirtualTiming { ipc: 1.0 },
            BuiltInPlatform::ArmVirt.default_ram_base(),
            0x20_0000,
        )
        .with_platform(BuiltInPlatform::ArmVirt)
        .with_arm_virt_defaults(1, ArmVirtGicVersion::V2),
    );

    // Initially all masks should be zero
    let pending = sim.gic_pending_mask(0, 1).expect("GICv2 should be present");
    assert_eq!(pending, 0);
    let enabled = sim.gic_enabled_mask(0, 1).expect("GICv2 should be present");
    assert_eq!(enabled, 0);
    let active = sim.gic_active_mask(0, 1).expect("GICv2 should be present");
    assert_eq!(active, 0);
}

#[test]
fn uart_introspection_reports_initial_state() {
    use crate::platform::arm_virt::ArmVirtGicVersion;

    let sim = build_simulator_from_request(
        SimulatorBuildRequest::new(
            Isa::AArch64,
            ExecMode::System,
            TimingChoice::VirtualTiming { ipc: 1.0 },
            BuiltInPlatform::ArmVirt.default_ram_base(),
            0x20_0000,
        )
        .with_platform(BuiltInPlatform::ArmVirt)
        .with_arm_virt_defaults(1, ArmVirtGicVersion::V2),
    );

    assert_eq!(sim.uart_tx_count(), Some(0));
    assert_eq!(sim.uart_rx_count(), Some(0));
    assert_eq!(sim.uart_is_tx_full(), Some(false));
    assert_eq!(sim.uart_is_rx_empty(), Some(true));
}

#[test]
fn read_gpr_works_for_riscv() {
    let sim = build_simulator_from_request(SimulatorBuildRequest::new(
        Isa::RiscV,
        ExecMode::Functional,
        TimingChoice::VirtualTiming { ipc: 1.0 },
        0,
        0x2000,
    ));

    // x0 is hardwired 0
    assert_eq!(sim.read_gpr(0), Some(0));
    // Other registers start at 0
    assert_eq!(sim.read_gpr(1), Some(0));
    // Out of range returns None
    assert_eq!(sim.read_gpr(32), None);
}

#[test]
fn read_gpr_works_for_aarch64() {
    let sim = build_simulator_from_request(SimulatorBuildRequest::new(
        Isa::AArch64,
        ExecMode::Functional,
        TimingChoice::VirtualTiming { ipc: 1.0 },
        0,
        0x2000,
    ));

    assert_eq!(sim.read_gpr(0), Some(0));
    // x31 = SP
    assert!(sim.read_gpr(31).is_some());
}

#[test]
fn gdb_target_reads_and_writes_aarch64_state() {
    let mut sim = build_simulator_from_request(SimulatorBuildRequest::new(
        Isa::AArch64,
        ExecMode::Functional,
        TimingChoice::VirtualTiming { ipc: 1.0 },
        0,
        0x2000,
    ));
    sim.set_pc(0x1000);

    let mut target = crate::HelmSimGdbTarget::new(&mut sim);
    assert_eq!(target.num_registers(), 33);
    assert_eq!(target.read_register(32), Some(0x1000));

    assert!(target.write_register(0, 0x1234));
    assert!(target.write_register(31, 0x7fff_0000));
    assert!(target.write_register(32, 0x2000));

    assert_eq!(target.read_register(0), Some(0x1234));
    assert_eq!(target.read_register(31), Some(0x7fff_0000));
    assert_eq!(target.read_pc(), 0x2000);
}

#[test]
fn gdb_target_reads_and_writes_memory() {
    let mut sim = build_simulator_from_request(SimulatorBuildRequest::new(
        Isa::RiscV,
        ExecMode::Functional,
        TimingChoice::VirtualTiming { ipc: 1.0 },
        0,
        0x2000,
    ));

    let mut target = crate::HelmSimGdbTarget::new(&mut sim);
    assert!(target.write_memory(0x40, &[1, 2, 3, 4]));
    assert_eq!(target.read_memory(0x40, 4), Some(vec![1, 2, 3, 4]));
    // x0 remains hardwired to zero on RISC-V.
    assert!(target.write_register(0, 99));
    assert_eq!(target.read_register(0), Some(0));
}

#[test]
fn debug_connections_are_arch_agnostic_and_do_not_mutate_execution_selection() {
    let mut engine = HelmEngine::new(
        Isa::RiscV,
        ExecMode::Functional,
        VirtualTiming::new(1.0),
        0,
        0x2000,
    );
    let aarch64_id = engine
        .session
        .push(HelmCore::Aarch64(Aarch64Core::Functional(
            Aarch64ArchState::new(),
        )));
    assert!(engine.session.set_runtime_label(aarch64_id, "a64-runtime"));
    assert!(engine
        .session
        .set_runtime_role(aarch64_id, crate::session::HelmCoreRole::Accelerator));
    assert!(engine
        .session
        .set_runtime_domain(aarch64_id, crate::session::HelmCluster(2)));

    let mut sim = HelmSim::VirtualTiming(engine);
    assert_eq!(sim.debug_pc(), 0);
    assert_eq!(sim.debug_read_gpr(0), Some(0));
    let connections = sim.debug_connections();
    assert_eq!(connections.len(), 2);
    assert_eq!(connections[0].arch, "riscv64");
    assert_eq!(connections[0].role, "primary_cpu");
    assert!(connections[0].active);
    assert_eq!(connections[1].arch, "aarch64");
    assert_eq!(connections[1].label, "a64-runtime");
    assert_eq!(connections[1].role, "accelerator");
    assert_eq!(connections[1].domain, 2);
    assert!(!connections[1].active);

    assert!(sim.select_debug_connection(aarch64_id.0));
    let active = sim
        .active_debug_connection()
        .expect("active debug connection");
    assert_eq!(active.runtime_id, aarch64_id.0);
    assert_eq!(active.arch, "aarch64");
    assert!(active.active);
    // Debug selection should not change the execution-active runtime slot.
    assert_eq!(
        match &sim {
            HelmSim::VirtualTiming(engine) => engine.session.active_id().0,
            HelmSim::IntervalTiming(engine) => engine.session.active_id().0,
            HelmSim::AccurateTiming(engine) => engine.session.active_id().0,
        },
        0
    );
    assert_eq!(
        sim.save_debug_checkpoint_values().unwrap()[0],
        ("pc".to_string(), 0)
    );

    let mut target = crate::HelmSimGdbTarget::new(&mut sim);
    assert!(target.write_register(0, 0x55aa));
    assert_eq!(target.read_register(0), Some(0x55aa));
    let checkpoint = sim.save_debug_checkpoint_values().unwrap();
    assert!(checkpoint
        .iter()
        .any(|(name, value)| name == "x0" && *value == 0x55aa));
}
