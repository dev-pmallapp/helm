use crate::fs::FsState;
use crate::platform::arm_virt::ArmVirtDevices;
use crate::se::LinuxAarch64SyscallHandler;
use crate::se::SyscallHandler;
use crate::system_mem::SystemMem;
use crate::{ExecMode, Isa};
use helm_arch::Aarch64ArchState;
use helm_hw_intc::GicSharedState;

pub(crate) struct Aarch64Vcpu {
    pub(crate) arch: Aarch64ArchState,
    pub(crate) fs: FsState,
    pub(crate) powered_on: bool,
}

pub(crate) struct Aarch64FsMachine {
    pub(crate) sys_mem: SystemMem,
    pub(crate) vcpus: Vec<Aarch64Vcpu>,
    pub(crate) next_vcpu: usize,
    #[allow(dead_code)]
    pub(crate) devs: ArmVirtDevices,
    pub(crate) irq_lines: Vec<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    #[allow(dead_code)]
    pub(crate) gic: Option<std::sync::Arc<std::sync::Mutex<GicSharedState>>>,
}

pub(crate) enum Aarch64Runtime {
    Disabled,
    Functional(Aarch64ArchState),
    Syscall {
        state: Aarch64ArchState,
        handler: LinuxAarch64SyscallHandler,
    },
    System(Aarch64FsMachine),
}

impl Default for Aarch64Runtime {
    fn default() -> Self {
        Self::Disabled
    }
}

impl Aarch64Runtime {
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

    pub(crate) fn machine(&self) -> Option<&Aarch64FsMachine> {
        match self {
            Self::System(machine) => Some(machine),
            _ => None,
        }
    }

    pub(crate) fn machine_mut(&mut self) -> Option<&mut Aarch64FsMachine> {
        match self {
            Self::System(machine) => Some(machine),
            _ => None,
        }
    }
}

pub(crate) struct RiscvRuntime {
    pub(crate) iregs: [u64; 32],
    pub(crate) fregs: [u64; 32],
    pub(crate) csrs: Box<[u64; 4096]>,
    pub(crate) pc: u64,
    pub(crate) mode: ExecMode,
    pub(crate) syscall_handler: Option<Box<dyn SyscallHandler>>,
    #[allow(dead_code)]
    pub(crate) lr_addr: Option<u64>,
}

impl Default for RiscvRuntime {
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

pub(crate) enum Runtime {
    Riscv(RiscvRuntime),
    Aarch64(Aarch64Runtime),
}

impl Runtime {
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
pub(crate) struct RuntimeId(pub(crate) usize);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct RuntimeCoordinationDomain(pub(crate) u8);

impl RuntimeCoordinationDomain {
    pub(crate) const SYSTEM: Self = Self(0);

    fn as_index(self) -> usize {
        self.0 as usize
    }
}

impl Default for RuntimeCoordinationDomain {
    fn default() -> Self {
        Self::SYSTEM
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub(crate) enum RuntimeRole {
    PrimaryCpu,
    Cpu,
    Accelerator,
    Service,
}

impl Default for RuntimeRole {
    fn default() -> Self {
        Self::Cpu
    }
}

impl RuntimeRole {
    fn participates_in_round_robin(self) -> bool {
        matches!(self, Self::PrimaryCpu | Self::Cpu)
    }
}

#[derive(Debug, Clone)]
pub(crate) struct RuntimeMeta {
    pub(crate) label: String,
    pub(crate) role: RuntimeRole,
    pub(crate) domain: RuntimeCoordinationDomain,
}

pub(crate) struct RuntimeSet {
    pub(crate) active: RuntimeId,
    pub(crate) runtimes: Vec<Runtime>,
    metadata: Vec<RuntimeMeta>,
}

impl Default for RuntimeSet {
    fn default() -> Self {
        Self {
            active: RuntimeId(0),
            runtimes: Vec::new(),
            metadata: Vec::new(),
        }
    }
}

impl RuntimeSet {
    pub(crate) fn new_primary(runtime: Runtime) -> Self {
        Self {
            active: RuntimeId(0),
            runtimes: vec![runtime],
            metadata: vec![RuntimeMeta {
                label: "runtime-0".to_string(),
                role: RuntimeRole::PrimaryCpu,
                domain: RuntimeCoordinationDomain::SYSTEM,
            }],
        }
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn push(&mut self, runtime: Runtime) -> RuntimeId {
        let id = RuntimeId(self.runtimes.len());
        self.runtimes.push(runtime);
        self.metadata.push(RuntimeMeta {
            label: format!("runtime-{}", id.0),
            role: RuntimeRole::Cpu,
            domain: RuntimeCoordinationDomain::SYSTEM,
        });
        id
    }

    pub(crate) fn active_id(&self) -> RuntimeId {
        self.active
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn set_active(&mut self, id: RuntimeId) -> bool {
        if id.0 < self.runtimes.len() {
            self.active = id;
            true
        } else {
            false
        }
    }

    pub(crate) fn runtime(&self, id: RuntimeId) -> Option<&Runtime> {
        self.runtimes.get(id.0)
    }

    pub(crate) fn runtime_mut(&mut self, id: RuntimeId) -> Option<&mut Runtime> {
        self.runtimes.get_mut(id.0)
    }

    pub(crate) fn metadata(&self, id: RuntimeId) -> Option<&RuntimeMeta> {
        self.metadata.get(id.0)
    }

    pub(crate) fn metadata_mut(&mut self, id: RuntimeId) -> Option<&mut RuntimeMeta> {
        self.metadata.get_mut(id.0)
    }

    pub(crate) fn active(&self) -> Option<&Runtime> {
        self.runtime(self.active)
    }

    pub(crate) fn active_mut(&mut self) -> Option<&mut Runtime> {
        self.runtime_mut(self.active)
    }

    pub(crate) fn riscv(&self) -> Option<&RiscvRuntime> {
        match self.active()? {
            Runtime::Riscv(runtime) => Some(runtime),
            Runtime::Aarch64(_) => None,
        }
    }

    pub(crate) fn riscv_mut(&mut self) -> Option<&mut RiscvRuntime> {
        match self.active_mut()? {
            Runtime::Riscv(runtime) => Some(runtime),
            Runtime::Aarch64(_) => None,
        }
    }

    pub(crate) fn aarch64(&self) -> Option<&Aarch64Runtime> {
        match self.active()? {
            Runtime::Aarch64(runtime) => Some(runtime),
            Runtime::Riscv(_) => None,
        }
    }

    pub(crate) fn aarch64_mut(&mut self) -> Option<&mut Aarch64Runtime> {
        match self.active_mut()? {
            Runtime::Aarch64(runtime) => Some(runtime),
            Runtime::Riscv(_) => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RuntimeSelectionScope {
    All,
    Compute,
    ComputeInDomain(RuntimeCoordinationDomain),
    Role(RuntimeRole),
    Domain(RuntimeCoordinationDomain),
}

impl RuntimeSelectionScope {
    fn falls_back_to_slot_order(self) -> bool {
        matches!(self, Self::Compute | Self::ComputeInDomain(_))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProgressAdvancePolicy {
    EveryProgress,
    RetiredInstruction,
    YieldedQuantum,
}

impl ProgressAdvancePolicy {
    fn should_advance(self, progress: SessionProgress) -> bool {
        match self {
            Self::EveryProgress => true,
            Self::RetiredInstruction => matches!(progress, SessionProgress::RetiredInstruction),
            Self::YieldedQuantum => matches!(progress, SessionProgress::YieldedQuantum),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RuntimeSelectionPolicy {
    Fixed(RuntimeId),
    RoundRobin {
        scope: RuntimeSelectionScope,
        advance_on: ProgressAdvancePolicy,
    },
}

impl RuntimeSelectionPolicy {
    pub(crate) fn round_robin() -> Self {
        Self::RoundRobin {
            scope: RuntimeSelectionScope::Compute,
            advance_on: ProgressAdvancePolicy::EveryProgress,
        }
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn round_robin_scope(scope: RuntimeSelectionScope) -> Self {
        Self::RoundRobin {
            scope,
            advance_on: ProgressAdvancePolicy::EveryProgress,
        }
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn round_robin_with(
        scope: RuntimeSelectionScope,
        advance_on: ProgressAdvancePolicy,
    ) -> Self {
        Self::RoundRobin { scope, advance_on }
    }

    fn should_advance(self, progress: SessionProgress) -> bool {
        match self {
            Self::Fixed(_) => true,
            Self::RoundRobin { advance_on, .. } => advance_on.should_advance(progress),
        }
    }
}

impl Default for RuntimeSelectionPolicy {
    fn default() -> Self {
        Self::Fixed(RuntimeId(0))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SessionProgress {
    RetiredInstruction,
    YieldedQuantum,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct DomainProgress {
    pub(crate) retired_instructions: u64,
    pub(crate) yielded_quanta: u64,
}

impl DomainProgress {
    fn record(&mut self, progress: SessionProgress) {
        match progress {
            SessionProgress::RetiredInstruction => {
                self.retired_instructions = self.retired_instructions.saturating_add(1);
            }
            SessionProgress::YieldedQuantum => {
                self.yielded_quanta = self.yielded_quanta.saturating_add(1);
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RuntimeCoordinationView {
    pub(crate) id: RuntimeId,
    pub(crate) label: String,
    pub(crate) isa: Isa,
    pub(crate) mode: Option<ExecMode>,
    pub(crate) role: RuntimeRole,
    pub(crate) domain: RuntimeCoordinationDomain,
    pub(crate) active: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DomainCoordinationView {
    pub(crate) domain: RuntimeCoordinationDomain,
    pub(crate) runtime_ids: Vec<RuntimeId>,
    pub(crate) compute_runtime_ids: Vec<RuntimeId>,
    pub(crate) progress: DomainProgress,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MachineCoordinationView {
    pub(crate) active_runtime: RuntimeId,
    pub(crate) runtimes: Vec<RuntimeCoordinationView>,
    pub(crate) domains: Vec<DomainCoordinationView>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ActiveRuntimeCoordination {
    id: RuntimeId,
    isa: Isa,
    mode: Option<ExecMode>,
    domain: RuntimeCoordinationDomain,
}

#[derive(Default)]
struct RuntimeTopology {
    all: Vec<RuntimeId>,
    compute: Vec<RuntimeId>,
    compute_by_domain: Vec<Vec<RuntimeId>>,
    primary_cpu: Vec<RuntimeId>,
    cpu: Vec<RuntimeId>,
    accelerator: Vec<RuntimeId>,
    service: Vec<RuntimeId>,
    domains: Vec<Vec<RuntimeId>>,
    runtime_domains: Vec<RuntimeCoordinationDomain>,
}

impl RuntimeTopology {
    fn from_set(set: &RuntimeSet) -> Self {
        let mut topology = Self::default();
        for (idx, meta) in set.metadata.iter().enumerate() {
            let id = RuntimeId(idx);
            topology.all.push(id);
            topology.push_domain(meta.domain, id);
            topology.runtime_domains.push(meta.domain);
            if meta.role.participates_in_round_robin() {
                topology.compute.push(id);
                topology.push_compute_domain(meta.domain, id);
            }
            match meta.role {
                RuntimeRole::PrimaryCpu => {
                    topology.primary_cpu.push(id);
                }
                RuntimeRole::Cpu => {
                    topology.cpu.push(id);
                }
                RuntimeRole::Accelerator => topology.accelerator.push(id),
                RuntimeRole::Service => topology.service.push(id),
            }
        }
        topology
    }

    fn push_domain(&mut self, domain: RuntimeCoordinationDomain, id: RuntimeId) {
        let idx = domain.as_index();
        if self.domains.len() <= idx {
            self.domains.resize_with(idx + 1, Vec::new);
        }
        self.domains[idx].push(id);
    }

    fn push_compute_domain(&mut self, domain: RuntimeCoordinationDomain, id: RuntimeId) {
        let idx = domain.as_index();
        if self.compute_by_domain.len() <= idx {
            self.compute_by_domain.resize_with(idx + 1, Vec::new);
        }
        self.compute_by_domain[idx].push(id);
    }

    fn ids(&self, scope: RuntimeSelectionScope) -> &[RuntimeId] {
        match scope {
            RuntimeSelectionScope::All => &self.all,
            RuntimeSelectionScope::Compute => &self.compute,
            RuntimeSelectionScope::ComputeInDomain(domain) => self
                .compute_by_domain
                .get(domain.as_index())
                .map(Vec::as_slice)
                .unwrap_or(&[]),
            RuntimeSelectionScope::Role(RuntimeRole::PrimaryCpu) => &self.primary_cpu,
            RuntimeSelectionScope::Role(RuntimeRole::Cpu) => &self.cpu,
            RuntimeSelectionScope::Role(RuntimeRole::Accelerator) => &self.accelerator,
            RuntimeSelectionScope::Role(RuntimeRole::Service) => &self.service,
            RuntimeSelectionScope::Domain(domain) => self
                .domains
                .get(domain.as_index())
                .map(Vec::as_slice)
                .unwrap_or(&[]),
        }
    }

    fn preferred(&self, scope: RuntimeSelectionScope) -> Option<RuntimeId> {
        match scope {
            RuntimeSelectionScope::Compute => self
                .primary_cpu
                .first()
                .copied()
                .or_else(|| self.compute.first().copied()),
            RuntimeSelectionScope::ComputeInDomain(domain) => self
                .compute_by_domain
                .get(domain.as_index())
                .and_then(|ids| ids.first().copied()),
            _ => self.ids(scope).first().copied(),
        }
    }
}

#[derive(Default)]
struct SessionCoordinationState {
    topology: RuntimeTopology,
    progress_by_domain: Vec<DomainProgress>,
    active: Option<ActiveRuntimeCoordination>,
}

impl SessionCoordinationState {
    fn new(set: &RuntimeSet) -> Self {
        let mut coordination = Self {
            topology: RuntimeTopology::from_set(set),
            progress_by_domain: Vec::new(),
            active: None,
        };
        coordination.sync_progress_with_topology();
        coordination.sync_active_with_set(set);
        coordination
    }

    fn sync_with_set(&mut self, set: &RuntimeSet) {
        self.topology = RuntimeTopology::from_set(set);
        self.sync_progress_with_topology();
        self.sync_active_with_set(set);
    }

    fn sync_active_with_set(&mut self, set: &RuntimeSet) {
        let active_id = set.active_id();
        self.active = set.active().map(|runtime| ActiveRuntimeCoordination {
            id: active_id,
            isa: runtime.isa(),
            mode: runtime.mode(),
            domain: set
                .metadata(active_id)
                .map(|meta| meta.domain)
                .unwrap_or_default(),
        });
    }

    fn sync_active_if_needed(&mut self, set: &RuntimeSet) {
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
                    .resize(max_domain + 1, DomainProgress::default());
            }
        }
    }

    fn record_progress(&mut self, progress: SessionProgress) {
        let Some(active) = self.active else {
            return;
        };
        let idx = active.domain.as_index();
        if self.progress_by_domain.len() <= idx {
            self.progress_by_domain
                .resize(idx + 1, DomainProgress::default());
        }
        self.progress_by_domain[idx].record(progress);
    }

    fn domain_progress(&self, domain: RuntimeCoordinationDomain) -> Option<DomainProgress> {
        self.progress_by_domain.get(domain.as_index()).copied()
    }

    fn has_runtime_in_scope(&self, scope: RuntimeSelectionScope) -> bool {
        !self.topology.ids(scope).is_empty()
    }

    fn active_id(&self) -> Option<RuntimeId> {
        self.active.map(|active| active.id)
    }

    fn active_isa(&self) -> Option<Isa> {
        self.active.map(|active| active.isa)
    }

    fn active_mode(&self) -> Option<ExecMode> {
        self.active.and_then(|active| active.mode)
    }

    fn preferred_runtime_in_scope(&self, scope: RuntimeSelectionScope) -> Option<RuntimeId> {
        self.topology.preferred(scope)
    }

    fn next_runtime_in_scope(
        &self,
        start: RuntimeId,
        scope: RuntimeSelectionScope,
    ) -> Option<RuntimeId> {
        let ids = self.topology.ids(scope);
        if ids.is_empty() {
            return None;
        }

        if let Some(position) = ids.iter().position(|id| *id == start) {
            return Some(ids[(position + 1) % ids.len()]);
        }

        ids.first().copied()
    }

    fn scope_contains(&self, scope: RuntimeSelectionScope, id: RuntimeId) -> bool {
        self.topology.ids(scope).contains(&id)
    }

    fn machine_view(&self, runtimes: &RuntimeSet) -> MachineCoordinationView {
        let runtimes_view = runtimes
            .runtimes
            .iter()
            .enumerate()
            .map(|(idx, runtime)| {
                let id = RuntimeId(idx);
                let meta = runtimes
                    .metadata(id)
                    .expect("runtime metadata must exist for each runtime");
                RuntimeCoordinationView {
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

        let domain_len = self.topology.domains.len().max(self.progress_by_domain.len());
        let mut domains = Vec::new();
        for idx in 0..domain_len {
            let runtime_ids = self.topology.domains.get(idx).cloned().unwrap_or_default();
            let compute_runtime_ids = self
                .topology
                .compute_by_domain
                .get(idx)
                .cloned()
                .unwrap_or_default();
            let progress = self.progress_by_domain.get(idx).copied().unwrap_or_default();
            if runtime_ids.is_empty()
                && compute_runtime_ids.is_empty()
                && progress == DomainProgress::default()
            {
                continue;
            }
            domains.push(DomainCoordinationView {
                domain: RuntimeCoordinationDomain(idx as u8),
                runtime_ids,
                compute_runtime_ids,
                progress,
            });
        }

        MachineCoordinationView {
            active_runtime: runtimes.active_id(),
            runtimes: runtimes_view,
            domains,
        }
    }
}

#[derive(Default)]
pub(crate) struct SimulationSession {
    pub(crate) runtimes: RuntimeSet,
    coordination: SessionCoordinationState,
    scheduler: SessionScheduler,
}

struct SessionScheduler {
    selection: RuntimeSelectionPolicy,
}

impl Default for SessionScheduler {
    fn default() -> Self {
        Self {
            selection: RuntimeSelectionPolicy::default(),
        }
    }
}

impl SessionScheduler {
    fn new(active: RuntimeId) -> Self {
        Self {
            selection: RuntimeSelectionPolicy::Fixed(active),
        }
    }

    fn set_active(&mut self, set: &mut RuntimeSet, id: RuntimeId) -> bool {
        let changed = set.set_active(id);
        if changed {
            self.selection = RuntimeSelectionPolicy::Fixed(id);
        }
        changed
    }

    fn selection_policy(&self) -> &RuntimeSelectionPolicy {
        &self.selection
    }

    fn set_selection_policy(
        &mut self,
        set: &mut RuntimeSet,
        coordination: &SessionCoordinationState,
        selection: RuntimeSelectionPolicy,
    ) {
        self.selection = selection;
        self.sync_active_with_policy(set, coordination);
    }

    fn advance_selection(&mut self, set: &mut RuntimeSet, coordination: &SessionCoordinationState) {
        match self.selection {
            RuntimeSelectionPolicy::Fixed(id) => {
                let _ = set.set_active(id);
            }
            RuntimeSelectionPolicy::RoundRobin { scope, .. } => {
                if let Some(next) = self.next_round_robin_id(set, coordination, scope) {
                    let _ = set.set_active(next);
                }
            }
        }
    }

    fn on_progress(
        &mut self,
        set: &mut RuntimeSet,
        coordination: &SessionCoordinationState,
        progress: SessionProgress,
    ) {
        if !self.selection.should_advance(progress) {
            return;
        }
        self.advance_selection(set, coordination);
    }

    fn sync_active_with_policy(
        &mut self,
        set: &mut RuntimeSet,
        coordination: &SessionCoordinationState,
    ) {
        match self.selection {
            RuntimeSelectionPolicy::Fixed(id) => {
                let _ = set.set_active(id);
            }
            RuntimeSelectionPolicy::RoundRobin { scope, .. } => {
                if let Some(id) = self.sync_round_robin_id(set, coordination, scope) {
                    let _ = set.set_active(id);
                }
            }
        }
    }

    fn next_round_robin_id(
        &self,
        set: &RuntimeSet,
        coordination: &SessionCoordinationState,
        scope: RuntimeSelectionScope,
    ) -> Option<RuntimeId> {
        if set.runtimes.is_empty() {
            return None;
        }

        if coordination.has_runtime_in_scope(scope) {
            coordination.next_runtime_in_scope(set.active_id(), scope)
        } else if scope.falls_back_to_slot_order() {
            Some(RuntimeId((set.active_id().0 + 1) % set.runtimes.len()))
        } else {
            Some(set.active_id())
        }
    }

    fn sync_round_robin_id(
        &self,
        set: &RuntimeSet,
        coordination: &SessionCoordinationState,
        scope: RuntimeSelectionScope,
    ) -> Option<RuntimeId> {
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

impl SimulationSession {
    pub(crate) fn from_runtimes(runtimes: RuntimeSet) -> Self {
        Self {
            coordination: SessionCoordinationState::new(&runtimes),
            scheduler: SessionScheduler::new(runtimes.active_id()),
            runtimes,
        }
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn new_primary(runtime: Runtime) -> Self {
        Self::from_runtimes(RuntimeSet::new_primary(runtime))
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn push(&mut self, runtime: Runtime) -> RuntimeId {
        let id = self.runtimes.push(runtime);
        self.coordination.sync_with_set(&self.runtimes);
        self.scheduler
            .sync_active_with_policy(&mut self.runtimes, &self.coordination);
        self.coordination.sync_active_if_needed(&self.runtimes);
        id
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn active_id(&self) -> RuntimeId {
        self.coordination
            .active_id()
            .unwrap_or_else(|| self.runtimes.active_id())
    }

    pub(crate) fn active_isa(&self) -> Option<Isa> {
        self.coordination
            .active_isa()
            .or_else(|| self.runtimes.active().map(Runtime::isa))
    }

    pub(crate) fn active_mode(&self) -> Option<ExecMode> {
        self.coordination
            .active_mode()
            .or_else(|| self.runtimes.active().and_then(Runtime::mode))
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn runtime_label(&self, id: RuntimeId) -> Option<&str> {
        self.runtimes.metadata(id).map(|meta| meta.label.as_str())
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn runtime_role(&self, id: RuntimeId) -> Option<RuntimeRole> {
        self.runtimes.metadata(id).map(|meta| meta.role)
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn runtime_domain(&self, id: RuntimeId) -> Option<RuntimeCoordinationDomain> {
        self.runtimes.metadata(id).map(|meta| meta.domain)
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn domain_progress(
        &self,
        domain: RuntimeCoordinationDomain,
    ) -> Option<DomainProgress> {
        self.coordination.domain_progress(domain)
    }

    pub(crate) fn refresh_active_runtime_cache(&mut self) {
        self.coordination.sync_active_with_set(&self.runtimes);
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn machine_coordination_view(&self) -> MachineCoordinationView {
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
    pub(crate) fn set_runtime_label(&mut self, id: RuntimeId, label: impl Into<String>) -> bool {
        if let Some(meta) = self.runtimes.metadata_mut(id) {
            meta.label = label.into();
            true
        } else {
            false
        }
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn set_runtime_role(&mut self, id: RuntimeId, role: RuntimeRole) -> bool {
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
    pub(crate) fn set_runtime_domain(
        &mut self,
        id: RuntimeId,
        domain: RuntimeCoordinationDomain,
    ) -> bool {
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
    pub(crate) fn set_active(&mut self, id: RuntimeId) -> bool {
        let changed = self.scheduler.set_active(&mut self.runtimes, id);
        if changed {
            self.coordination.sync_active_if_needed(&self.runtimes);
        }
        changed
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn selection_policy(&self) -> &RuntimeSelectionPolicy {
        self.scheduler.selection_policy()
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn set_selection_policy(&mut self, selection: RuntimeSelectionPolicy) {
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

    pub(crate) fn on_progress(&mut self, progress: SessionProgress) {
        self.coordination.record_progress(progress);
        self.scheduler
            .on_progress(&mut self.runtimes, &self.coordination, progress);
        self.coordination.sync_active_if_needed(&self.runtimes);
    }

    pub(crate) fn replace_primary(&mut self, runtime: Runtime) {
        self.runtimes = RuntimeSet::new_primary(runtime);
        self.coordination = SessionCoordinationState::new(&self.runtimes);
        self.scheduler = SessionScheduler::new(self.runtimes.active_id());
    }

    pub(crate) fn riscv(&self) -> Option<&RiscvRuntime> {
        self.runtimes.riscv()
    }

    pub(crate) fn riscv_mut(&mut self) -> Option<&mut RiscvRuntime> {
        self.runtimes.riscv_mut()
    }

    pub(crate) fn aarch64(&self) -> Option<&Aarch64Runtime> {
        self.runtimes.aarch64()
    }

    pub(crate) fn aarch64_mut(&mut self) -> Option<&mut Aarch64Runtime> {
        self.runtimes.aarch64_mut()
    }
}
