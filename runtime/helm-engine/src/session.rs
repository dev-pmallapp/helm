use crate::address_space::HelmAddressSpace;
use crate::fs::FsState;
use crate::platform::arm_virt::ArmVirtDevices;
use crate::se::LinuxAarch64SyscallHandler;
use crate::se::SyscallHandler;
use crate::{ExecMode, Isa};
use helm_arch::Aarch64ArchState;
use helm_devices::MessageInterruptEmitter;
use helm_hw_intc::{GicSharedState, GicV3SharedState};
use helm_platform::{QuirkKey, QuirkSet};

pub(crate) struct HelmVcpu {
    pub(crate) arch: Aarch64ArchState,
    pub(crate) fs: FsState,
    pub(crate) powered_on: bool,
}

pub(crate) struct HelmBoard {
    pub(crate) sys_mem: HelmAddressSpace,
    pub(crate) vcpus: Vec<HelmVcpu>,
    pub(crate) next_vcpu: usize,
    #[allow(dead_code)]
    pub(crate) devs: ArmVirtDevices,
    pub(crate) quirks: QuirkSet,
    pub(crate) irq_lines: Vec<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    pub(crate) gic: Option<HelmGic>,
    pub(crate) pci_msi: Option<MessageInterruptEmitter>,
}

impl HelmBoard {
    pub(crate) fn has_quirk(&self, key: QuirkKey) -> bool {
        self.quirks.contains(key)
    }
}

pub(crate) struct BuiltAarch64System {
    pub(crate) board: HelmBoard,
}

pub(crate) enum BuiltSystem {
    Aarch64(BuiltAarch64System),
}

pub(crate) enum HelmGic {
    #[cfg_attr(not(test), allow(dead_code))]
    V2(std::sync::Arc<std::sync::Mutex<GicSharedState>>),
    V3(std::sync::Arc<std::sync::Mutex<GicV3SharedState>>),
}

pub(crate) enum Aarch64Core {
    Disabled,
    Functional(Aarch64ArchState),
    Syscall {
        state: Aarch64ArchState,
        handler: LinuxAarch64SyscallHandler,
    },
    System(HelmBoard),
}

impl Default for Aarch64Core {
    fn default() -> Self {
        Self::Disabled
    }
}

impl Aarch64Core {
    pub(crate) fn mode(&self) -> Option<ExecMode> {
        match self {
            Self::Disabled => None,
            Self::Functional(_) => Some(ExecMode::Functional),
            Self::Syscall { .. } => Some(ExecMode::Syscall),
            Self::System(_) => Some(ExecMode::System),
        }
    }

    pub(crate) fn state(&self) -> Option<&Aarch64ArchState> {
        match self {
            Self::Disabled => None,
            Self::Functional(state) => Some(state),
            Self::Syscall { state, .. } => Some(state),
            Self::System(machine) => machine.vcpus.first().map(|vcpu| &vcpu.arch),
        }
    }

    pub(crate) fn state_mut(&mut self) -> Option<&mut Aarch64ArchState> {
        match self {
            Self::Disabled => None,
            Self::Functional(state) => Some(state),
            Self::Syscall { state, .. } => Some(state),
            Self::System(machine) => machine.vcpus.first_mut().map(|vcpu| &mut vcpu.arch),
        }
    }

    pub(crate) fn handler(&self) -> Option<&LinuxAarch64SyscallHandler> {
        match self {
            Self::Syscall { handler, .. } => Some(handler),
            _ => None,
        }
    }

    pub(crate) fn handler_mut(&mut self) -> Option<&mut LinuxAarch64SyscallHandler> {
        match self {
            Self::Syscall { handler, .. } => Some(handler),
            _ => None,
        }
    }

    pub(crate) fn machine(&self) -> Option<&HelmBoard> {
        match self {
            Self::System(machine) => Some(machine),
            _ => None,
        }
    }

    pub(crate) fn machine_mut(&mut self) -> Option<&mut HelmBoard> {
        match self {
            Self::System(machine) => Some(machine),
            _ => None,
        }
    }
}

pub(crate) struct RiscvCore {
    pub(crate) iregs: [u64; 32],
    pub(crate) fregs: [u64; 32],
    pub(crate) csrs: Box<[u64; 4096]>,
    pub(crate) pc: u64,
    pub(crate) mode: ExecMode,
    pub(crate) syscall_handler: Option<Box<dyn SyscallHandler>>,
    #[allow(dead_code)]
    pub(crate) lr_addr: Option<u64>,
}

impl Default for RiscvCore {
    fn default() -> Self {
        Self {
            iregs: [0u64; 32],
            fregs: [0u64; 32],
            csrs: Box::new([0u64; 4096]),
            pc: 0,
            mode: ExecMode::Functional,
            syscall_handler: None,
            lr_addr: None,
        }
    }
}

pub(crate) enum HelmCore {
    Riscv(RiscvCore),
    Aarch64(Aarch64Core),
}

impl HelmCore {
    pub(crate) fn isa(&self) -> Isa {
        match self {
            Self::Riscv(_) => Isa::RiscV,
            Self::Aarch64(_) => Isa::AArch64,
        }
    }

    pub(crate) fn mode(&self) -> Option<ExecMode> {
        match self {
            Self::Riscv(runtime) => Some(runtime.mode),
            Self::Aarch64(runtime) => runtime.mode(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct HelmCoreId(pub(crate) usize);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct HelmCluster(pub(crate) u8);

impl HelmCluster {
    pub(crate) const SYSTEM: Self = Self(0);

    fn as_index(self) -> usize {
        self.0 as usize
    }
}

impl Default for HelmCluster {
    fn default() -> Self {
        Self::SYSTEM
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub(crate) enum HelmCoreRole {
    PrimaryCpu,
    Cpu,
    Accelerator,
    Service,
}

impl Default for HelmCoreRole {
    fn default() -> Self {
        Self::Cpu
    }
}

impl HelmCoreRole {
    fn participates_in_round_robin(self) -> bool {
        matches!(self, Self::PrimaryCpu | Self::Cpu)
    }
}

#[derive(Debug, Clone)]
pub(crate) struct HelmCoreMeta {
    pub(crate) label: String,
    pub(crate) role: HelmCoreRole,
    pub(crate) domain: HelmCluster,
}

pub(crate) struct HelmCoreSet {
    pub(crate) active: HelmCoreId,
    pub(crate) runtimes: Vec<HelmCore>,
    metadata: Vec<HelmCoreMeta>,
}

impl Default for HelmCoreSet {
    fn default() -> Self {
        Self {
            active: HelmCoreId(0),
            runtimes: Vec::new(),
            metadata: Vec::new(),
        }
    }
}

impl HelmCoreSet {
    pub(crate) fn new_primary(runtime: HelmCore) -> Self {
        Self {
            active: HelmCoreId(0),
            runtimes: vec![runtime],
            metadata: vec![HelmCoreMeta {
                label: "runtime-0".to_string(),
                role: HelmCoreRole::PrimaryCpu,
                domain: HelmCluster::SYSTEM,
            }],
        }
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn push(&mut self, runtime: HelmCore) -> HelmCoreId {
        let id = HelmCoreId(self.runtimes.len());
        self.runtimes.push(runtime);
        self.metadata.push(HelmCoreMeta {
            label: format!("runtime-{}", id.0),
            role: HelmCoreRole::Cpu,
            domain: HelmCluster::SYSTEM,
        });
        id
    }

    pub(crate) fn active_id(&self) -> HelmCoreId {
        self.active
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn set_active(&mut self, id: HelmCoreId) -> bool {
        if id.0 < self.runtimes.len() {
            self.active = id;
            true
        } else {
            false
        }
    }

    pub(crate) fn runtime(&self, id: HelmCoreId) -> Option<&HelmCore> {
        self.runtimes.get(id.0)
    }

    pub(crate) fn runtime_mut(&mut self, id: HelmCoreId) -> Option<&mut HelmCore> {
        self.runtimes.get_mut(id.0)
    }

    pub(crate) fn metadata(&self, id: HelmCoreId) -> Option<&HelmCoreMeta> {
        self.metadata.get(id.0)
    }

    pub(crate) fn metadata_mut(&mut self, id: HelmCoreId) -> Option<&mut HelmCoreMeta> {
        self.metadata.get_mut(id.0)
    }

    pub(crate) fn active(&self) -> Option<&HelmCore> {
        self.runtime(self.active)
    }

    pub(crate) fn active_mut(&mut self) -> Option<&mut HelmCore> {
        self.runtime_mut(self.active)
    }

    pub(crate) fn riscv(&self) -> Option<&RiscvCore> {
        match self.active()? {
            HelmCore::Riscv(runtime) => Some(runtime),
            HelmCore::Aarch64(_) => None,
        }
    }

    pub(crate) fn riscv_mut(&mut self) -> Option<&mut RiscvCore> {
        match self.active_mut()? {
            HelmCore::Riscv(runtime) => Some(runtime),
            HelmCore::Aarch64(_) => None,
        }
    }

    pub(crate) fn aarch64(&self) -> Option<&Aarch64Core> {
        match self.active()? {
            HelmCore::Aarch64(runtime) => Some(runtime),
            HelmCore::Riscv(_) => None,
        }
    }

    pub(crate) fn aarch64_mut(&mut self) -> Option<&mut Aarch64Core> {
        match self.active_mut()? {
            HelmCore::Aarch64(runtime) => Some(runtime),
            HelmCore::Riscv(_) => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) enum HelmCoreScope {
    All,
    Compute,
    ComputeInDomain(HelmCluster),
    Role(HelmCoreRole),
    Domain(HelmCluster),
}

impl HelmCoreScope {
    fn falls_back_to_slot_order(self) -> bool {
        matches!(self, Self::Compute | Self::ComputeInDomain(_))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) enum HelmAdvancePolicy {
    EveryProgress,
    RetiredInstruction,
    YieldedQuantum,
}

impl HelmAdvancePolicy {
    fn should_advance(self, progress: RunStep) -> bool {
        match self {
            Self::EveryProgress => true,
            Self::RetiredInstruction => matches!(progress, RunStep::RetiredInstruction),
            Self::YieldedQuantum => matches!(progress, RunStep::YieldedQuantum),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HelmSchedulePolicy {
    Fixed(HelmCoreId),
    RoundRobin {
        scope: HelmCoreScope,
        advance_on: HelmAdvancePolicy,
    },
}

impl HelmSchedulePolicy {
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn round_robin() -> Self {
        Self::RoundRobin {
            scope: HelmCoreScope::Compute,
            advance_on: HelmAdvancePolicy::EveryProgress,
        }
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn round_robin_scope(scope: HelmCoreScope) -> Self {
        Self::RoundRobin {
            scope,
            advance_on: HelmAdvancePolicy::EveryProgress,
        }
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn round_robin_with(scope: HelmCoreScope, advance_on: HelmAdvancePolicy) -> Self {
        Self::RoundRobin { scope, advance_on }
    }

    fn should_advance(self, progress: RunStep) -> bool {
        match self {
            Self::Fixed(_) => true,
            Self::RoundRobin { advance_on, .. } => advance_on.should_advance(progress),
        }
    }
}

impl Default for HelmSchedulePolicy {
    fn default() -> Self {
        Self::Fixed(HelmCoreId(0))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RunStep {
    RetiredInstruction,
    YieldedQuantum,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct HelmClusterProgress {
    pub(crate) retired_instructions: u64,
    pub(crate) yielded_quanta: u64,
}

impl HelmClusterProgress {
    fn record(&mut self, progress: RunStep) {
        match progress {
            RunStep::RetiredInstruction => {
                self.retired_instructions = self.retired_instructions.saturating_add(1);
            }
            RunStep::YieldedQuantum => {
                self.yielded_quanta = self.yielded_quanta.saturating_add(1);
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HelmCoreView {
    pub(crate) id: HelmCoreId,
    pub(crate) label: String,
    pub(crate) isa: Isa,
    pub(crate) mode: Option<ExecMode>,
    pub(crate) role: HelmCoreRole,
    pub(crate) domain: HelmCluster,
    pub(crate) active: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HelmClusterView {
    pub(crate) domain: HelmCluster,
    pub(crate) runtime_ids: Vec<HelmCoreId>,
    pub(crate) compute_runtime_ids: Vec<HelmCoreId>,
    pub(crate) progress: HelmClusterProgress,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HelmMachineView {
    pub(crate) active_runtime: HelmCoreId,
    pub(crate) runtimes: Vec<HelmCoreView>,
    pub(crate) domains: Vec<HelmClusterView>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ActiveCore {
    id: HelmCoreId,
    isa: Isa,
    mode: Option<ExecMode>,
    domain: HelmCluster,
}

#[derive(Default)]
struct CoreTopology {
    all: Vec<HelmCoreId>,
    compute: Vec<HelmCoreId>,
    compute_by_domain: Vec<Vec<HelmCoreId>>,
    primary_cpu: Vec<HelmCoreId>,
    cpu: Vec<HelmCoreId>,
    accelerator: Vec<HelmCoreId>,
    service: Vec<HelmCoreId>,
    domains: Vec<Vec<HelmCoreId>>,
    runtime_domains: Vec<HelmCluster>,
}

impl CoreTopology {
    fn from_set(set: &HelmCoreSet) -> Self {
        let mut topology = Self::default();
        for (idx, meta) in set.metadata.iter().enumerate() {
            let id = HelmCoreId(idx);
            topology.all.push(id);
            topology.push_domain(meta.domain, id);
            topology.runtime_domains.push(meta.domain);
            if meta.role.participates_in_round_robin() {
                topology.compute.push(id);
                topology.push_compute_domain(meta.domain, id);
            }
            match meta.role {
                HelmCoreRole::PrimaryCpu => {
                    topology.primary_cpu.push(id);
                }
                HelmCoreRole::Cpu => {
                    topology.cpu.push(id);
                }
                HelmCoreRole::Accelerator => topology.accelerator.push(id),
                HelmCoreRole::Service => topology.service.push(id),
            }
        }
        topology
    }

    fn push_domain(&mut self, domain: HelmCluster, id: HelmCoreId) {
        let idx = domain.as_index();
        if self.domains.len() <= idx {
            self.domains.resize_with(idx + 1, Vec::new);
        }
        self.domains[idx].push(id);
    }

    fn push_compute_domain(&mut self, domain: HelmCluster, id: HelmCoreId) {
        let idx = domain.as_index();
        if self.compute_by_domain.len() <= idx {
            self.compute_by_domain.resize_with(idx + 1, Vec::new);
        }
        self.compute_by_domain[idx].push(id);
    }

    fn ids(&self, scope: HelmCoreScope) -> &[HelmCoreId] {
        match scope {
            HelmCoreScope::All => &self.all,
            HelmCoreScope::Compute => &self.compute,
            HelmCoreScope::ComputeInDomain(domain) => self
                .compute_by_domain
                .get(domain.as_index())
                .map(Vec::as_slice)
                .unwrap_or(&[]),
            HelmCoreScope::Role(HelmCoreRole::PrimaryCpu) => &self.primary_cpu,
            HelmCoreScope::Role(HelmCoreRole::Cpu) => &self.cpu,
            HelmCoreScope::Role(HelmCoreRole::Accelerator) => &self.accelerator,
            HelmCoreScope::Role(HelmCoreRole::Service) => &self.service,
            HelmCoreScope::Domain(domain) => self
                .domains
                .get(domain.as_index())
                .map(Vec::as_slice)
                .unwrap_or(&[]),
        }
    }

    fn preferred(&self, scope: HelmCoreScope) -> Option<HelmCoreId> {
        match scope {
            HelmCoreScope::Compute => self
                .primary_cpu
                .first()
                .copied()
                .or_else(|| self.compute.first().copied()),
            HelmCoreScope::ComputeInDomain(domain) => self
                .compute_by_domain
                .get(domain.as_index())
                .and_then(|ids| ids.first().copied()),
            _ => self.ids(scope).first().copied(),
        }
    }
}

#[derive(Default)]
struct CoordinationState {
    topology: CoreTopology,
    progress_by_domain: Vec<HelmClusterProgress>,
    active: Option<ActiveCore>,
}

impl CoordinationState {
    fn new(set: &HelmCoreSet) -> Self {
        let mut coordination = Self {
            topology: CoreTopology::from_set(set),
            progress_by_domain: Vec::new(),
            active: None,
        };
        coordination.sync_progress_with_topology();
        coordination.sync_active_with_set(set);
        coordination
    }

    fn sync_with_set(&mut self, set: &HelmCoreSet) {
        self.topology = CoreTopology::from_set(set);
        self.sync_progress_with_topology();
        self.sync_active_with_set(set);
    }

    fn sync_active_with_set(&mut self, set: &HelmCoreSet) {
        let active_id = set.active_id();
        self.active = set.active().map(|runtime| ActiveCore {
            id: active_id,
            isa: runtime.isa(),
            mode: runtime.mode(),
            domain: set
                .metadata(active_id)
                .map(|meta| meta.domain)
                .unwrap_or_default(),
        });
    }

    fn sync_active_if_needed(&mut self, set: &HelmCoreSet) {
        let active_id = set.active_id();
        if self.active.map(|active| active.id) != Some(active_id) {
            self.sync_active_with_set(set);
        }
    }

    fn sync_progress_with_topology(&mut self) {
        let max_domain = self
            .topology
            .runtime_domains
            .iter()
            .map(|domain| domain.as_index())
            .max();
        if let Some(max_domain) = max_domain {
            if self.progress_by_domain.len() <= max_domain {
                self.progress_by_domain
                    .resize(max_domain + 1, HelmClusterProgress::default());
            }
        }
    }

    fn record_progress(&mut self, progress: RunStep) {
        let Some(active) = self.active else {
            return;
        };
        let idx = active.domain.as_index();
        if self.progress_by_domain.len() <= idx {
            self.progress_by_domain
                .resize(idx + 1, HelmClusterProgress::default());
        }
        self.progress_by_domain[idx].record(progress);
    }

    fn domain_progress(&self, domain: HelmCluster) -> Option<HelmClusterProgress> {
        self.progress_by_domain.get(domain.as_index()).copied()
    }

    fn has_runtime_in_scope(&self, scope: HelmCoreScope) -> bool {
        !self.topology.ids(scope).is_empty()
    }

    fn active_id(&self) -> Option<HelmCoreId> {
        self.active.map(|active| active.id)
    }

    fn active_isa(&self) -> Option<Isa> {
        self.active.map(|active| active.isa)
    }

    fn active_mode(&self) -> Option<ExecMode> {
        self.active.and_then(|active| active.mode)
    }

    fn preferred_runtime_in_scope(&self, scope: HelmCoreScope) -> Option<HelmCoreId> {
        self.topology.preferred(scope)
    }

    fn next_runtime_in_scope(&self, start: HelmCoreId, scope: HelmCoreScope) -> Option<HelmCoreId> {
        let ids = self.topology.ids(scope);
        if ids.is_empty() {
            return None;
        }

        if let Some(position) = ids.iter().position(|id| *id == start) {
            return Some(ids[(position + 1) % ids.len()]);
        }

        ids.first().copied()
    }

    fn scope_contains(&self, scope: HelmCoreScope, id: HelmCoreId) -> bool {
        self.topology.ids(scope).contains(&id)
    }

    fn machine_view(&self, runtimes: &HelmCoreSet) -> HelmMachineView {
        let runtimes_view = runtimes
            .runtimes
            .iter()
            .enumerate()
            .map(|(idx, runtime)| {
                let id = HelmCoreId(idx);
                let meta = runtimes
                    .metadata(id)
                    .expect("runtime metadata must exist for each runtime");
                HelmCoreView {
                    id,
                    label: meta.label.clone(),
                    isa: runtime.isa(),
                    mode: runtime.mode(),
                    role: meta.role,
                    domain: meta.domain,
                    active: id == runtimes.active_id(),
                }
            })
            .collect();

        let domain_len = self
            .topology
            .domains
            .len()
            .max(self.progress_by_domain.len());
        let mut domains = Vec::new();
        for idx in 0..domain_len {
            let runtime_ids = self.topology.domains.get(idx).cloned().unwrap_or_default();
            let compute_runtime_ids = self
                .topology
                .compute_by_domain
                .get(idx)
                .cloned()
                .unwrap_or_default();
            let progress = self
                .progress_by_domain
                .get(idx)
                .copied()
                .unwrap_or_default();
            if runtime_ids.is_empty()
                && compute_runtime_ids.is_empty()
                && progress == HelmClusterProgress::default()
            {
                continue;
            }
            domains.push(HelmClusterView {
                domain: HelmCluster(idx as u8),
                runtime_ids,
                compute_runtime_ids,
                progress,
            });
        }

        HelmMachineView {
            active_runtime: runtimes.active_id(),
            runtimes: runtimes_view,
            domains,
        }
    }
}

#[derive(Default)]
pub(crate) struct HelmMachine {
    pub(crate) runtimes: HelmCoreSet,
    coordination: CoordinationState,
    scheduler: CoreScheduler,
}

struct CoreScheduler {
    selection: HelmSchedulePolicy,
}

impl Default for CoreScheduler {
    fn default() -> Self {
        Self {
            selection: HelmSchedulePolicy::default(),
        }
    }
}

impl CoreScheduler {
    fn new(active: HelmCoreId) -> Self {
        Self {
            selection: HelmSchedulePolicy::Fixed(active),
        }
    }

    fn set_active(&mut self, set: &mut HelmCoreSet, id: HelmCoreId) -> bool {
        let changed = set.set_active(id);
        if changed {
            self.selection = HelmSchedulePolicy::Fixed(id);
        }
        changed
    }

    fn selection_policy(&self) -> &HelmSchedulePolicy {
        &self.selection
    }

    fn set_selection_policy(
        &mut self,
        set: &mut HelmCoreSet,
        coordination: &CoordinationState,
        selection: HelmSchedulePolicy,
    ) {
        self.selection = selection;
        self.sync_active_with_policy(set, coordination);
    }

    fn advance_selection(&mut self, set: &mut HelmCoreSet, coordination: &CoordinationState) {
        match self.selection {
            HelmSchedulePolicy::Fixed(id) => {
                let _ = set.set_active(id);
            }
            HelmSchedulePolicy::RoundRobin { scope, .. } => {
                if let Some(next) = self.next_round_robin_id(set, coordination, scope) {
                    let _ = set.set_active(next);
                }
            }
        }
    }

    fn on_progress(
        &mut self,
        set: &mut HelmCoreSet,
        coordination: &CoordinationState,
        progress: RunStep,
    ) {
        if !self.selection.should_advance(progress) {
            return;
        }
        self.advance_selection(set, coordination);
    }

    fn sync_active_with_policy(&mut self, set: &mut HelmCoreSet, coordination: &CoordinationState) {
        match self.selection {
            HelmSchedulePolicy::Fixed(id) => {
                let _ = set.set_active(id);
            }
            HelmSchedulePolicy::RoundRobin { scope, .. } => {
                if let Some(id) = self.sync_round_robin_id(set, coordination, scope) {
                    let _ = set.set_active(id);
                }
            }
        }
    }

    fn next_round_robin_id(
        &self,
        set: &HelmCoreSet,
        coordination: &CoordinationState,
        scope: HelmCoreScope,
    ) -> Option<HelmCoreId> {
        if set.runtimes.is_empty() {
            return None;
        }

        if coordination.has_runtime_in_scope(scope) {
            coordination.next_runtime_in_scope(set.active_id(), scope)
        } else if scope.falls_back_to_slot_order() {
            Some(HelmCoreId((set.active_id().0 + 1) % set.runtimes.len()))
        } else {
            Some(set.active_id())
        }
    }

    fn sync_round_robin_id(
        &self,
        set: &HelmCoreSet,
        coordination: &CoordinationState,
        scope: HelmCoreScope,
    ) -> Option<HelmCoreId> {
        if set.runtimes.is_empty() {
            return None;
        }

        if !coordination.has_runtime_in_scope(scope) {
            return Some(set.active_id());
        }

        let active = set.active_id();
        if coordination.scope_contains(scope, active) {
            return Some(active);
        }

        coordination.preferred_runtime_in_scope(scope)
    }
}

impl HelmMachine {
    pub(crate) fn from_runtimes(runtimes: HelmCoreSet) -> Self {
        Self {
            coordination: CoordinationState::new(&runtimes),
            scheduler: CoreScheduler::new(runtimes.active_id()),
            runtimes,
        }
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn new_primary(runtime: HelmCore) -> Self {
        Self::from_runtimes(HelmCoreSet::new_primary(runtime))
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn push(&mut self, runtime: HelmCore) -> HelmCoreId {
        let id = self.runtimes.push(runtime);
        self.coordination.sync_with_set(&self.runtimes);
        self.scheduler
            .sync_active_with_policy(&mut self.runtimes, &self.coordination);
        self.coordination.sync_active_if_needed(&self.runtimes);
        id
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn active_id(&self) -> HelmCoreId {
        self.coordination
            .active_id()
            .unwrap_or_else(|| self.runtimes.active_id())
    }

    pub(crate) fn active_isa(&self) -> Option<Isa> {
        self.coordination
            .active_isa()
            .or_else(|| self.runtimes.active().map(HelmCore::isa))
    }

    pub(crate) fn active_mode(&self) -> Option<ExecMode> {
        self.coordination
            .active_mode()
            .or_else(|| self.runtimes.active().and_then(HelmCore::mode))
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn runtime_label(&self, id: HelmCoreId) -> Option<&str> {
        self.runtimes.metadata(id).map(|meta| meta.label.as_str())
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn runtime_role(&self, id: HelmCoreId) -> Option<HelmCoreRole> {
        self.runtimes.metadata(id).map(|meta| meta.role)
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn runtime_domain(&self, id: HelmCoreId) -> Option<HelmCluster> {
        self.runtimes.metadata(id).map(|meta| meta.domain)
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn domain_progress(&self, domain: HelmCluster) -> Option<HelmClusterProgress> {
        self.coordination.domain_progress(domain)
    }

    pub(crate) fn refresh_active_runtime_cache(&mut self) {
        self.coordination.sync_active_with_set(&self.runtimes);
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn machine_coordination_view(&self) -> HelmMachineView {
        self.coordination.machine_view(&self.runtimes)
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn machine_coordination_state(&self) -> crate::machine::MachineCoordinationState {
        crate::machine::MachineCoordinationState::from_view(self.machine_coordination_view())
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn machine_policy_feedback(&self) -> crate::machine::MachinePolicyFeedback {
        self.machine_coordination_state().policy_feedback()
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn set_runtime_label(&mut self, id: HelmCoreId, label: impl Into<String>) -> bool {
        if let Some(meta) = self.runtimes.metadata_mut(id) {
            meta.label = label.into();
            true
        } else {
            false
        }
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn set_runtime_role(&mut self, id: HelmCoreId, role: HelmCoreRole) -> bool {
        if let Some(meta) = self.runtimes.metadata_mut(id) {
            meta.role = role;
            self.coordination.sync_with_set(&self.runtimes);
            self.scheduler
                .sync_active_with_policy(&mut self.runtimes, &self.coordination);
            self.coordination.sync_active_if_needed(&self.runtimes);
            true
        } else {
            false
        }
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn set_runtime_domain(&mut self, id: HelmCoreId, domain: HelmCluster) -> bool {
        if let Some(meta) = self.runtimes.metadata_mut(id) {
            meta.domain = domain;
            self.coordination.sync_with_set(&self.runtimes);
            self.scheduler
                .sync_active_with_policy(&mut self.runtimes, &self.coordination);
            self.coordination.sync_active_if_needed(&self.runtimes);
            true
        } else {
            false
        }
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn set_active(&mut self, id: HelmCoreId) -> bool {
        let changed = self.scheduler.set_active(&mut self.runtimes, id);
        if changed {
            self.coordination.sync_active_if_needed(&self.runtimes);
        }
        changed
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn selection_policy(&self) -> &HelmSchedulePolicy {
        self.scheduler.selection_policy()
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn set_selection_policy(&mut self, selection: HelmSchedulePolicy) {
        self.scheduler
            .set_selection_policy(&mut self.runtimes, &self.coordination, selection);
        self.coordination.sync_active_if_needed(&self.runtimes);
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn advance_selection(&mut self) {
        self.scheduler
            .advance_selection(&mut self.runtimes, &self.coordination);
        self.coordination.sync_active_if_needed(&self.runtimes);
    }

    pub(crate) fn on_progress(&mut self, progress: RunStep) {
        self.coordination.record_progress(progress);
        self.scheduler
            .on_progress(&mut self.runtimes, &self.coordination, progress);
        self.coordination.sync_active_if_needed(&self.runtimes);
    }

    pub(crate) fn replace_primary(&mut self, runtime: HelmCore) {
        self.runtimes = HelmCoreSet::new_primary(runtime);
        self.coordination = CoordinationState::new(&self.runtimes);
        self.scheduler = CoreScheduler::new(self.runtimes.active_id());
    }

    pub(crate) fn riscv(&self) -> Option<&RiscvCore> {
        self.runtimes.riscv()
    }

    pub(crate) fn riscv_mut(&mut self) -> Option<&mut RiscvCore> {
        self.runtimes.riscv_mut()
    }

    pub(crate) fn set_riscv_mode(&mut self, mode: ExecMode) -> bool {
        if let Some(riscv) = self.riscv_mut() {
            riscv.mode = mode;
            self.refresh_active_runtime_cache();
            true
        } else {
            false
        }
    }

    pub(crate) fn set_riscv_syscall_handler(
        &mut self,
        handler: Option<Box<dyn SyscallHandler>>,
    ) -> bool {
        if let Some(riscv) = self.riscv_mut() {
            riscv.syscall_handler = handler;
            self.refresh_active_runtime_cache();
            true
        } else {
            false
        }
    }

    pub(crate) fn aarch64(&self) -> Option<&Aarch64Core> {
        self.runtimes.aarch64()
    }

    pub(crate) fn aarch64_mut(&mut self) -> Option<&mut Aarch64Core> {
        self.runtimes.aarch64_mut()
    }
}
