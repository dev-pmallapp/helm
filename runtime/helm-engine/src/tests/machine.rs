use crate::session::{
    HelmCluster, HelmClusterProgress, HelmCore, HelmCoreRole, HelmCoreScope, HelmMachine, RunStep,
};
use crate::{Aarch64Core, RiscvCore};

#[test]
fn session_machine_coordination_view_reports_runtime_and_domain_state() {
    let mut session = HelmMachine::new_primary(HelmCore::Riscv(RiscvCore::default()));
    let cpu1 = session.push(HelmCore::Aarch64(Aarch64Core::Disabled));
    let svc = session.push(HelmCore::Riscv(RiscvCore::default()));

    assert!(session.set_runtime_domain(cpu1, HelmCluster(1)));
    assert!(session.set_runtime_domain(svc, HelmCluster(1)));
    assert!(session.set_runtime_role(svc, HelmCoreRole::Service));
    assert!(session.set_runtime_label(svc, "svc0"));
    assert!(session.set_active(cpu1));
    session.on_progress(RunStep::RetiredInstruction);

    let view = session.machine_coordination_view();
    assert_eq!(view.active_runtime, cpu1);
    assert_eq!(view.runtimes.len(), 3);

    let svc_view = view
        .runtimes
        .iter()
        .find(|runtime| runtime.id == svc)
        .expect("service runtime missing from machine view");
    assert_eq!(svc_view.label, "svc0");
    assert_eq!(svc_view.role, HelmCoreRole::Service);
    assert_eq!(svc_view.domain, HelmCluster(1));
    assert!(!svc_view.active);

    let domain1 = view
        .domains
        .iter()
        .find(|domain| domain.domain == HelmCluster(1))
        .expect("domain 1 missing from machine view");
    assert_eq!(domain1.runtime_ids, vec![cpu1, svc]);
    assert_eq!(domain1.compute_runtime_ids, vec![cpu1]);
    assert_eq!(
        domain1.progress,
        HelmClusterProgress {
            retired_instructions: 1,
            yielded_quanta: 0,
        }
    );
}

#[test]
fn session_machine_coordination_view_retains_progress_for_empty_domains() {
    let mut session = HelmMachine::new_primary(HelmCore::Riscv(RiscvCore::default()));

    assert!(session.set_runtime_domain(crate::session::HelmCoreId(0), HelmCluster(4)));
    session.on_progress(RunStep::RetiredInstruction);
    assert!(session.set_runtime_domain(crate::session::HelmCoreId(0), HelmCluster::SYSTEM));

    let view = session.machine_coordination_view();
    let domain4 = view
        .domains
        .iter()
        .find(|domain| domain.domain == HelmCluster(4))
        .expect("domain 4 progress should remain visible");
    assert!(domain4.runtime_ids.is_empty());
    assert!(domain4.compute_runtime_ids.is_empty());
    assert_eq!(
        domain4.progress,
        HelmClusterProgress {
            retired_instructions: 1,
            yielded_quanta: 0,
        }
    );
}

#[test]
fn session_machine_coordination_state_summarizes_domains() {
    let mut session = HelmMachine::new_primary(HelmCore::Riscv(RiscvCore::default()));
    let cpu1 = session.push(HelmCore::Aarch64(Aarch64Core::Disabled));
    let svc = session.push(HelmCore::Riscv(RiscvCore::default()));
    let accel = session.push(HelmCore::Aarch64(Aarch64Core::Disabled));

    assert!(session.set_runtime_domain(cpu1, HelmCluster(1)));
    assert!(session.set_runtime_domain(svc, HelmCluster(1)));
    assert!(session.set_runtime_domain(accel, HelmCluster(2)));
    assert!(session.set_runtime_role(svc, HelmCoreRole::Service));
    assert!(session.set_runtime_role(accel, HelmCoreRole::Accelerator));
    assert!(session.set_active(cpu1));
    session.on_progress(RunStep::RetiredInstruction);
    session.on_progress(RunStep::RetiredInstruction);
    assert!(session.set_active(accel));
    session.on_progress(RunStep::YieldedQuantum);

    let state = session.machine_coordination_state();
    assert_eq!(state.total_runtime_count(), 4);
    assert_eq!(state.total_compute_runtime_count(), 2);

    let domain1 = state
        .domain_summary(HelmCluster(1))
        .expect("domain 1 summary missing");
    assert_eq!(domain1.runtime_count, 2);
    assert_eq!(domain1.compute_runtime_count, 1);
    assert_eq!(domain1.primary_cpu_count, 0);
    assert_eq!(domain1.cpu_count, 1);
    assert_eq!(domain1.service_count, 1);
    assert_eq!(domain1.accelerator_count, 0);
    assert_eq!(
        domain1.progress,
        HelmClusterProgress {
            retired_instructions: 2,
            yielded_quanta: 0,
        }
    );

    let busiest = state
        .busiest_domain_by_retired_instructions()
        .expect("busiest domain missing");
    assert_eq!(busiest.domain, HelmCluster(1));
}

#[test]
fn session_machine_policy_feedback_prefers_busiest_compute_domain() {
    let mut session = HelmMachine::new_primary(HelmCore::Riscv(RiscvCore::default()));
    let cpu1 = session.push(HelmCore::Aarch64(Aarch64Core::Disabled));
    let cpu2 = session.push(HelmCore::Riscv(RiscvCore::default()));

    assert!(session.set_runtime_domain(cpu1, HelmCluster(1)));
    assert!(session.set_runtime_domain(cpu2, HelmCluster(2)));

    assert!(session.set_active(cpu1));
    session.on_progress(RunStep::RetiredInstruction);
    session.on_progress(RunStep::RetiredInstruction);
    assert!(session.set_active(cpu2));
    session.on_progress(RunStep::RetiredInstruction);

    let feedback = session.machine_policy_feedback();
    assert_eq!(
        feedback.preferred_scope,
        Some(HelmCoreScope::ComputeInDomain(HelmCluster(1)))
    );
    assert_eq!(feedback.busiest_domain, Some(HelmCluster(1)));
    assert_eq!(
        feedback.busiest_domain_progress,
        Some(HelmClusterProgress {
            retired_instructions: 2,
            yielded_quanta: 0,
        })
    );
}

#[test]
fn session_machine_policy_feedback_falls_back_to_global_compute_scope() {
    let session = HelmMachine::new_primary(HelmCore::Riscv(RiscvCore::default()));

    let feedback = session.machine_policy_feedback();
    assert_eq!(feedback.preferred_scope, Some(HelmCoreScope::Compute));
    assert_eq!(feedback.busiest_domain, Some(HelmCluster::SYSTEM));
    assert_eq!(
        feedback.busiest_domain_progress,
        Some(HelmClusterProgress::default())
    );
}
