use crate::session::{
    DomainCoordinationView, DomainProgress, MachineCoordinationView, RuntimeCoordinationDomain,
    RuntimeCoordinationView, RuntimeRole,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MachineDomainSummary {
    pub(crate) domain: RuntimeCoordinationDomain,
    pub(crate) runtime_count: usize,
    pub(crate) compute_runtime_count: usize,
    pub(crate) primary_cpu_count: usize,
    pub(crate) cpu_count: usize,
    pub(crate) accelerator_count: usize,
    pub(crate) service_count: usize,
    pub(crate) progress: DomainProgress,
}

impl MachineDomainSummary {
    fn from_view(view: &DomainCoordinationView, runtimes: &[RuntimeCoordinationView]) -> Self {
        let mut primary_cpu_count = 0;
        let mut cpu_count = 0;
        let mut accelerator_count = 0;
        let mut service_count = 0;

        for runtime_id in &view.runtime_ids {
            if let Some(runtime) = runtimes.iter().find(|runtime| runtime.id == *runtime_id) {
                match runtime.role {
                    RuntimeRole::PrimaryCpu => primary_cpu_count += 1,
                    RuntimeRole::Cpu => cpu_count += 1,
                    RuntimeRole::Accelerator => accelerator_count += 1,
                    RuntimeRole::Service => service_count += 1,
                }
            }
        }

        Self {
            domain: view.domain,
            runtime_count: view.runtime_ids.len(),
            compute_runtime_count: view.compute_runtime_ids.len(),
            primary_cpu_count,
            cpu_count,
            accelerator_count,
            service_count,
            progress: view.progress,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MachineCoordinationState {
    active_runtime: crate::session::RuntimeId,
    runtimes: Vec<RuntimeCoordinationView>,
    domains: Vec<MachineDomainSummary>,
}

impl MachineCoordinationState {
    pub(crate) fn from_view(view: MachineCoordinationView) -> Self {
        let domains = view
            .domains
            .iter()
            .map(|domain| MachineDomainSummary::from_view(domain, &view.runtimes))
            .collect();
        Self {
            active_runtime: view.active_runtime,
            runtimes: view.runtimes,
            domains,
        }
    }

    #[allow(dead_code)]
    pub(crate) fn active_runtime(&self) -> crate::session::RuntimeId {
        self.active_runtime
    }

    #[allow(dead_code)]
    pub(crate) fn runtimes(&self) -> &[RuntimeCoordinationView] {
        &self.runtimes
    }

    #[allow(dead_code)]
    pub(crate) fn domains(&self) -> &[MachineDomainSummary] {
        &self.domains
    }

    pub(crate) fn domain_summary(
        &self,
        domain: RuntimeCoordinationDomain,
    ) -> Option<&MachineDomainSummary> {
        self.domains.iter().find(|summary| summary.domain == domain)
    }

    pub(crate) fn total_runtime_count(&self) -> usize {
        self.runtimes.len()
    }

    pub(crate) fn total_compute_runtime_count(&self) -> usize {
        self.domains
            .iter()
            .map(|summary| summary.compute_runtime_count)
            .sum()
    }

    pub(crate) fn busiest_domain_by_retired_instructions(&self) -> Option<&MachineDomainSummary> {
        self.domains
            .iter()
            .max_by_key(|summary| summary.progress.retired_instructions)
    }
}
