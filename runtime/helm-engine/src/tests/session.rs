use crate::session::{
    HelmAdvancePolicy, HelmCluster, HelmClusterProgress, HelmCore, HelmCoreId, HelmCoreRole,
    HelmCoreScope, HelmMachine, HelmSchedulePolicy, RunStep,
};
use crate::{Aarch64Core, ExecMode, RiscvCore};
use helm_arch::Aarch64ArchState;

#[test]
fn runtime_set_tracks_active_runtime() {
    let mut session = HelmMachine::new_primary(HelmCore::Riscv(RiscvCore::default()));
    let aarch64_id = session.push(HelmCore::Aarch64(Aarch64Core::Disabled));

    assert!(matches!(
        session.runtimes.active(),
        Some(HelmCore::Riscv(_))
    ));
    assert_eq!(session.active_id(), HelmCoreId(0));

    assert!(session.set_active(aarch64_id));
    assert!(matches!(
        session.runtimes.active(),
        Some(HelmCore::Aarch64(_))
    ));
    assert_eq!(session.active_id(), aarch64_id);
}

#[test]
fn runtime_set_rejects_invalid_active_index() {
    let mut session = HelmMachine::new_primary(HelmCore::Riscv(RiscvCore::default()));
    assert!(!session.set_active(HelmCoreId(99)));
    assert_eq!(session.active_id(), HelmCoreId(0));
    assert!(matches!(
        session.runtimes.active(),
        Some(HelmCore::Riscv(_))
    ));
}

#[test]
fn session_active_runtime_cache_tracks_switches() {
    let mut session = HelmMachine::new_primary(HelmCore::Riscv(RiscvCore::default()));
    let aarch64_id = session.push(HelmCore::Aarch64(Aarch64Core::Functional(
        Aarch64ArchState::new(),
    )));

    assert_eq!(session.active_isa(), Some(crate::Isa::RiscV));
    assert_eq!(session.active_mode(), Some(ExecMode::Functional));

    assert!(session.set_active(aarch64_id));
    assert_eq!(session.active_isa(), Some(crate::Isa::AArch64));
    assert_eq!(session.active_mode(), Some(ExecMode::Functional));

    session.set_selection_policy(HelmSchedulePolicy::round_robin());
    session.on_progress(RunStep::RetiredInstruction);
    assert_eq!(session.active_isa(), Some(crate::Isa::RiscV));
    assert_eq!(session.active_mode(), Some(ExecMode::Functional));
}

#[test]
fn session_riscv_mode_api_refreshes_active_mode_cache() {
    let mut session = HelmMachine::new_primary(HelmCore::Riscv(RiscvCore::default()));

    assert_eq!(session.active_mode(), Some(ExecMode::Functional));
    assert!(session.set_riscv_mode(ExecMode::Syscall));
    assert_eq!(session.active_mode(), Some(ExecMode::Syscall));
    assert!(session.set_riscv_syscall_handler(Some(Box::new(
        crate::se::LinuxRiscv64SyscallHandler::new(0x1000),
    ))));
}

#[test]
fn session_fixed_policy_tracks_explicit_active_runtime() {
    let mut session = HelmMachine::new_primary(HelmCore::Riscv(RiscvCore::default()));
    let aarch64_id = session.push(HelmCore::Aarch64(Aarch64Core::Disabled));

    session.set_selection_policy(HelmSchedulePolicy::Fixed(aarch64_id));

    assert!(matches!(
        session.selection_policy(),
        HelmSchedulePolicy::Fixed(id) if *id == aarch64_id
    ));
    assert_eq!(session.active_id(), aarch64_id);
    assert!(matches!(
        session.runtimes.active(),
        Some(HelmCore::Aarch64(_))
    ));
}

#[test]
fn session_round_robin_advances_active_runtime() {
    let mut session = HelmMachine::new_primary(HelmCore::Riscv(RiscvCore::default()));
    let aarch64_id = session.push(HelmCore::Aarch64(Aarch64Core::Disabled));

    session.set_selection_policy(HelmSchedulePolicy::round_robin());
    session.advance_selection();
    assert_eq!(session.active_id(), aarch64_id);
    assert!(matches!(
        session.runtimes.active(),
        Some(HelmCore::Aarch64(_))
    ));

    session.advance_selection();
    assert_eq!(session.active_id(), HelmCoreId(0));
    assert!(matches!(
        session.runtimes.active(),
        Some(HelmCore::Riscv(_))
    ));
}

#[test]
fn session_progress_hook_advances_round_robin_policy() {
    let mut session = HelmMachine::new_primary(HelmCore::Riscv(RiscvCore::default()));
    let aarch64_id = session.push(HelmCore::Aarch64(Aarch64Core::Disabled));
    session.set_selection_policy(HelmSchedulePolicy::round_robin());

    session.on_progress(RunStep::RetiredInstruction);
    assert_eq!(session.active_id(), aarch64_id);

    session.on_progress(RunStep::YieldedQuantum);
    assert_eq!(session.active_id(), HelmCoreId(0));
}

#[test]
fn session_round_robin_skips_non_cpu_roles() {
    let mut session = HelmMachine::new_primary(HelmCore::Riscv(RiscvCore::default()));
    let accel_id = session.push(HelmCore::Aarch64(Aarch64Core::Disabled));
    let cpu_id = session.push(HelmCore::Riscv(RiscvCore::default()));

    assert!(session.set_runtime_role(accel_id, HelmCoreRole::Accelerator));
    session.set_selection_policy(HelmSchedulePolicy::round_robin());

    session.advance_selection();
    assert_eq!(session.active_id(), cpu_id);

    session.advance_selection();
    assert_eq!(session.active_id(), HelmCoreId(0));
}

#[test]
fn session_round_robin_resyncs_when_active_role_changes() {
    let mut session = HelmMachine::new_primary(HelmCore::Riscv(RiscvCore::default()));
    let cpu_id = session.push(HelmCore::Aarch64(Aarch64Core::Disabled));

    assert!(session.set_active(cpu_id));
    session.set_selection_policy(HelmSchedulePolicy::round_robin());
    assert_eq!(session.active_id(), cpu_id);

    assert!(session.set_runtime_role(cpu_id, HelmCoreRole::Service));
    assert_eq!(session.active_id(), HelmCoreId(0));
}

#[test]
fn session_round_robin_can_target_specific_runtime_roles() {
    let mut session = HelmMachine::new_primary(HelmCore::Riscv(RiscvCore::default()));
    let service0 = session.push(HelmCore::Aarch64(Aarch64Core::Disabled));
    let service1 = session.push(HelmCore::Riscv(RiscvCore::default()));

    assert!(session.set_runtime_role(service0, HelmCoreRole::Service));
    assert!(session.set_runtime_role(service1, HelmCoreRole::Service));
    session.set_selection_policy(HelmSchedulePolicy::round_robin_scope(HelmCoreScope::Role(
        HelmCoreRole::Service,
    )));

    assert_eq!(session.active_id(), service0);
    session.advance_selection();
    assert_eq!(session.active_id(), service1);
    session.advance_selection();
    assert_eq!(session.active_id(), service0);
}

#[test]
fn session_progress_hook_can_limit_round_robin_to_quantum_yields() {
    let mut session = HelmMachine::new_primary(HelmCore::Riscv(RiscvCore::default()));
    let cpu1 = session.push(HelmCore::Aarch64(Aarch64Core::Disabled));
    session.set_selection_policy(HelmSchedulePolicy::round_robin_with(
        HelmCoreScope::Compute,
        HelmAdvancePolicy::YieldedQuantum,
    ));

    session.on_progress(RunStep::RetiredInstruction);
    assert_eq!(session.active_id(), HelmCoreId(0));

    session.on_progress(RunStep::YieldedQuantum);
    assert_eq!(session.active_id(), cpu1);
}

#[test]
fn session_progress_hook_can_limit_all_scope_round_robin_to_retired_instructions() {
    let mut session = HelmMachine::new_primary(HelmCore::Riscv(RiscvCore::default()));
    let service_id = session.push(HelmCore::Aarch64(Aarch64Core::Disabled));

    assert!(session.set_runtime_role(service_id, HelmCoreRole::Service));
    session.set_selection_policy(HelmSchedulePolicy::round_robin_with(
        HelmCoreScope::All,
        HelmAdvancePolicy::RetiredInstruction,
    ));

    session.on_progress(RunStep::YieldedQuantum);
    assert_eq!(session.active_id(), HelmCoreId(0));

    session.on_progress(RunStep::RetiredInstruction);
    assert_eq!(session.active_id(), service_id);
}

#[test]
fn session_push_refreshes_scoped_scheduler_topology() {
    let mut session = HelmMachine::new_primary(HelmCore::Riscv(RiscvCore::default()));

    assert!(session.set_runtime_role(HelmCoreId(0), HelmCoreRole::Service));
    session.set_selection_policy(HelmSchedulePolicy::round_robin_scope(
        HelmCoreScope::Compute,
    ));
    assert_eq!(session.active_id(), HelmCoreId(0));

    let cpu_id = session.push(HelmCore::Aarch64(Aarch64Core::Disabled));
    assert_eq!(session.active_id(), cpu_id);
}

#[test]
fn session_round_robin_can_target_specific_coordination_domains() {
    let mut session = HelmMachine::new_primary(HelmCore::Riscv(RiscvCore::default()));
    let domain1_cpu0 = session.push(HelmCore::Aarch64(Aarch64Core::Disabled));
    let domain1_cpu1 = session.push(HelmCore::Riscv(RiscvCore::default()));

    assert!(session.set_runtime_domain(domain1_cpu0, HelmCluster(1)));
    assert!(session.set_runtime_domain(domain1_cpu1, HelmCluster(1)));
    session.set_selection_policy(HelmSchedulePolicy::round_robin_scope(
        HelmCoreScope::Domain(HelmCluster(1)),
    ));

    assert_eq!(session.active_id(), domain1_cpu0);
    session.advance_selection();
    assert_eq!(session.active_id(), domain1_cpu1);
    session.advance_selection();
    assert_eq!(session.active_id(), domain1_cpu0);
}

#[test]
fn session_domain_changes_resync_domain_scoped_scheduler() {
    let mut session = HelmMachine::new_primary(HelmCore::Riscv(RiscvCore::default()));
    let cpu1 = session.push(HelmCore::Aarch64(Aarch64Core::Disabled));
    let cpu2 = session.push(HelmCore::Riscv(RiscvCore::default()));

    assert!(session.set_runtime_domain(cpu1, HelmCluster(1)));
    assert!(session.set_runtime_domain(cpu2, HelmCluster(1)));
    session.set_selection_policy(HelmSchedulePolicy::round_robin_scope(
        HelmCoreScope::Domain(HelmCluster(1)),
    ));
    assert_eq!(session.active_id(), cpu1);

    assert!(session.set_runtime_domain(cpu1, HelmCluster(2)));
    assert_eq!(session.active_id(), cpu2);
}

#[test]
fn session_round_robin_can_target_compute_within_a_domain() {
    let mut session = HelmMachine::new_primary(HelmCore::Riscv(RiscvCore::default()));
    let domain1_cpu0 = session.push(HelmCore::Aarch64(Aarch64Core::Disabled));
    let domain1_service = session.push(HelmCore::Riscv(RiscvCore::default()));
    let domain1_cpu1 = session.push(HelmCore::Aarch64(Aarch64Core::Disabled));

    assert!(session.set_runtime_domain(domain1_cpu0, HelmCluster(1)));
    assert!(session.set_runtime_domain(domain1_service, HelmCluster(1)));
    assert!(session.set_runtime_domain(domain1_cpu1, HelmCluster(1)));
    assert!(session.set_runtime_role(domain1_service, HelmCoreRole::Service));

    session.set_selection_policy(HelmSchedulePolicy::round_robin_scope(
        HelmCoreScope::ComputeInDomain(HelmCluster(1)),
    ));

    assert_eq!(session.active_id(), domain1_cpu0);
    session.advance_selection();
    assert_eq!(session.active_id(), domain1_cpu1);
    session.advance_selection();
    assert_eq!(session.active_id(), domain1_cpu0);
}

#[test]
fn session_compute_domain_scope_resyncs_when_domain_compute_membership_changes() {
    let mut session = HelmMachine::new_primary(HelmCore::Riscv(RiscvCore::default()));
    let domain1_cpu0 = session.push(HelmCore::Aarch64(Aarch64Core::Disabled));
    let domain1_cpu1 = session.push(HelmCore::Riscv(RiscvCore::default()));

    assert!(session.set_runtime_domain(domain1_cpu0, HelmCluster(1)));
    assert!(session.set_runtime_domain(domain1_cpu1, HelmCluster(1)));
    session.set_selection_policy(HelmSchedulePolicy::round_robin_scope(
        HelmCoreScope::ComputeInDomain(HelmCluster(1)),
    ));
    assert_eq!(session.active_id(), domain1_cpu0);

    assert!(session.set_runtime_role(domain1_cpu0, HelmCoreRole::Service));
    assert_eq!(session.active_id(), domain1_cpu1);
}

#[test]
fn session_domain_progress_tracks_active_runtime_domain() {
    let mut session = HelmMachine::new_primary(HelmCore::Riscv(RiscvCore::default()));
    let cpu1 = session.push(HelmCore::Aarch64(Aarch64Core::Disabled));

    assert!(session.set_runtime_domain(cpu1, HelmCluster(1)));

    session.on_progress(RunStep::RetiredInstruction);
    assert_eq!(
        session.domain_progress(HelmCluster::SYSTEM),
        Some(HelmClusterProgress {
            retired_instructions: 1,
            yielded_quanta: 0,
        })
    );
    assert_eq!(
        session.domain_progress(HelmCluster(1)),
        Some(HelmClusterProgress::default())
    );

    assert!(session.set_active(cpu1));
    session.on_progress(RunStep::YieldedQuantum);
    assert_eq!(
        session.domain_progress(HelmCluster(1)),
        Some(HelmClusterProgress {
            retired_instructions: 0,
            yielded_quanta: 1,
        })
    );
}

#[test]
fn session_domain_progress_follows_domain_reassignment() {
    let mut session = HelmMachine::new_primary(HelmCore::Riscv(RiscvCore::default()));

    session.on_progress(RunStep::RetiredInstruction);
    assert_eq!(
        session.domain_progress(HelmCluster::SYSTEM),
        Some(HelmClusterProgress {
            retired_instructions: 1,
            yielded_quanta: 0,
        })
    );

    assert!(session.set_runtime_domain(HelmCoreId(0), HelmCluster(3)));
    session.on_progress(RunStep::RetiredInstruction);
    assert_eq!(
        session.domain_progress(HelmCluster::SYSTEM),
        Some(HelmClusterProgress {
            retired_instructions: 1,
            yielded_quanta: 0,
        })
    );
    assert_eq!(
        session.domain_progress(HelmCluster(3)),
        Some(HelmClusterProgress {
            retired_instructions: 1,
            yielded_quanta: 0,
        })
    );
}

#[test]
fn session_replace_primary_rebuilds_coordination_state() {
    let mut session = HelmMachine::new_primary(HelmCore::Riscv(RiscvCore::default()));

    assert!(session.set_runtime_domain(HelmCoreId(0), HelmCluster(3)));
    session.on_progress(RunStep::RetiredInstruction);
    assert_eq!(
        session.domain_progress(HelmCluster(3)),
        Some(HelmClusterProgress {
            retired_instructions: 1,
            yielded_quanta: 0,
        })
    );

    session.replace_primary(HelmCore::Aarch64(Aarch64Core::Disabled));

    assert_eq!(session.active_id(), HelmCoreId(0));
    assert_eq!(
        session.runtime_domain(HelmCoreId(0)),
        Some(HelmCluster::SYSTEM)
    );
    assert_eq!(
        session.domain_progress(HelmCluster::SYSTEM),
        Some(HelmClusterProgress::default())
    );
    assert_eq!(session.domain_progress(HelmCluster(3)), None);
}

#[test]
fn session_tracks_runtime_labels_and_roles() {
    let mut session = HelmMachine::new_primary(HelmCore::Riscv(RiscvCore::default()));
    let accel_id = session.push(HelmCore::Aarch64(Aarch64Core::Disabled));

    assert_eq!(session.runtime_label(HelmCoreId(0)), Some("runtime-0"));
    assert_eq!(
        session.runtime_role(HelmCoreId(0)),
        Some(HelmCoreRole::PrimaryCpu)
    );
    assert_eq!(
        session.runtime_domain(HelmCoreId(0)),
        Some(HelmCluster::SYSTEM)
    );
    assert_eq!(session.runtime_role(accel_id), Some(HelmCoreRole::Cpu));
    assert_eq!(session.runtime_domain(accel_id), Some(HelmCluster::SYSTEM));

    assert!(session.set_runtime_label(accel_id, "gpu0"));
    assert!(session.set_runtime_role(accel_id, HelmCoreRole::Accelerator));
    assert!(session.set_runtime_domain(accel_id, HelmCluster(7)));

    assert_eq!(session.runtime_label(accel_id), Some("gpu0"));
    assert_eq!(
        session.runtime_role(accel_id),
        Some(HelmCoreRole::Accelerator)
    );
    assert_eq!(session.runtime_domain(accel_id), Some(HelmCluster(7)));
}
