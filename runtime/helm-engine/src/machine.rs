use crate::session::{
    HelmCluster, HelmClusterProgress, HelmClusterView, HelmCoreRole, HelmCoreScope, HelmCoreView,
    HelmMachineView,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MachineDomainSummary {
    pub(crate) domain: HelmCluster,
    pub(crate) runtime_count: usize,
    pub(crate) compute_runtime_count: usize,
    pub(crate) primary_cpu_count: usize,
    pub(crate) cpu_count: usize,
    pub(crate) accelerator_count: usize,
    pub(crate) service_count: usize,
    pub(crate) progress: HelmClusterProgress,
}

impl MachineDomainSummary {
    fn from_view(view: &HelmClusterView, runtimes: &[HelmCoreView]) -> Self {
        let mut primary_cpu_count = 0;
        let mut cpu_count = 0;
        let mut accelerator_count = 0;
        let mut service_count = 0;

        for runtime_id in &view.runtime_ids {
            if let Some(runtime) = runtimes.iter().find(|runtime| runtime.id == *runtime_id) {
                match runtime.role {
                    HelmCoreRole::PrimaryCpu => primary_cpu_count += 1,
                    HelmCoreRole::Cpu => cpu_count += 1,
                    HelmCoreRole::Accelerator => accelerator_count += 1,
                    HelmCoreRole::Service => service_count += 1,
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
    active_runtime: crate::session::HelmCoreId,
    runtimes: Vec<HelmCoreView>,
    domains: Vec<MachineDomainSummary>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MachinePolicyFeedback {
    pub(crate) preferred_scope: Option<HelmCoreScope>,
    pub(crate) busiest_domain: Option<HelmCluster>,
    pub(crate) busiest_domain_progress: Option<HelmClusterProgress>,
}

impl MachineCoordinationState {
    pub(crate) fn from_view(view: HelmMachineView) -> Self {
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
    pub(crate) fn active_runtime(&self) -> crate::session::HelmCoreId {
        self.active_runtime
    }

    #[allow(dead_code)]
    pub(crate) fn runtimes(&self) -> &[HelmCoreView] {
        &self.runtimes
    }

    #[allow(dead_code)]
    pub(crate) fn domains(&self) -> &[MachineDomainSummary] {
        &self.domains
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn domain_summary(&self, domain: HelmCluster) -> Option<&MachineDomainSummary> {
        self.domains.iter().find(|summary| summary.domain == domain)
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn total_runtime_count(&self) -> usize {
        self.runtimes.len()
    }

    pub(crate) fn total_compute_runtime_count(&self) -> usize {
        self.domains
            .iter()
            .map(|summary| summary.compute_runtime_count)
            .sum()
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn busiest_domain_by_retired_instructions(&self) -> Option<&MachineDomainSummary> {
        self.domains
            .iter()
            .max_by_key(|summary| summary.progress.retired_instructions)
    }

    pub(crate) fn policy_feedback(&self) -> MachinePolicyFeedback {
        let busiest_compute_domain = self
            .domains
            .iter()
            .filter(|summary| summary.compute_runtime_count > 0)
            .max_by_key(|summary| summary.progress.retired_instructions);

        if let Some(domain) = busiest_compute_domain {
            if domain.progress.retired_instructions > 0 {
                return MachinePolicyFeedback {
                    preferred_scope: Some(HelmCoreScope::ComputeInDomain(domain.domain)),
                    busiest_domain: Some(domain.domain),
                    busiest_domain_progress: Some(domain.progress),
                };
            }
        }

        let preferred_scope =
            (self.total_compute_runtime_count() > 0).then_some(HelmCoreScope::Compute);

        MachinePolicyFeedback {
            preferred_scope,
            busiest_domain: busiest_compute_domain.map(|domain| domain.domain),
            busiest_domain_progress: busiest_compute_domain.map(|domain| domain.progress),
        }
    }
}
