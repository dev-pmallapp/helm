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

#[derive(Default)]
pub(crate) struct SimulationSession {
    pub(crate) runtimes: RuntimeSet,
    scheduler: SessionScheduler,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RuntimeSelectionScope {
    All,
    Compute,
    Role(RuntimeRole),
}

impl RuntimeSelectionScope {
    fn matches(self, role: RuntimeRole) -> bool {
        match self {
            Self::All => true,
            Self::Compute => role.participates_in_round_robin(),
            Self::Role(expected) => role == expected,
        }
    }

    fn falls_back_to_slot_order(self) -> bool {
        matches!(self, Self::Compute)
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

pub(crate) enum SessionProgress {
    RetiredInstruction,
    YieldedQuantum,
}

#[derive(Default)]
struct RuntimeTopology {
    all: Vec<RuntimeId>,
    compute: Vec<RuntimeId>,
    primary_cpu: Vec<RuntimeId>,
    cpu: Vec<RuntimeId>,
    accelerator: Vec<RuntimeId>,
    service: Vec<RuntimeId>,
}

impl RuntimeTopology {
    fn from_set(set: &RuntimeSet) -> Self {
        let mut topology = Self::default();
        for (idx, meta) in set.metadata.iter().enumerate() {
            let id = RuntimeId(idx);
            topology.all.push(id);
            match meta.role {
                RuntimeRole::PrimaryCpu => {
                    topology.primary_cpu.push(id);
                    topology.compute.push(id);
                }
                RuntimeRole::Cpu => {
                    topology.cpu.push(id);
                    topology.compute.push(id);
                }
                RuntimeRole::Accelerator => topology.accelerator.push(id),
                RuntimeRole::Service => topology.service.push(id),
            }
        }
        topology
    }

    fn ids(&self, scope: RuntimeSelectionScope) -> &[RuntimeId] {
        match scope {
            RuntimeSelectionScope::All => &self.all,
            RuntimeSelectionScope::Compute => &self.compute,
            RuntimeSelectionScope::Role(RuntimeRole::PrimaryCpu) => &self.primary_cpu,
            RuntimeSelectionScope::Role(RuntimeRole::Cpu) => &self.cpu,
            RuntimeSelectionScope::Role(RuntimeRole::Accelerator) => &self.accelerator,
            RuntimeSelectionScope::Role(RuntimeRole::Service) => &self.service,
        }
    }

    fn preferred(&self, scope: RuntimeSelectionScope) -> Option<RuntimeId> {
        match scope {
            RuntimeSelectionScope::Compute => self
                .primary_cpu
                .first()
                .copied()
                .or_else(|| self.compute.first().copied()),
            _ => self.ids(scope).first().copied(),
        }
    }
}

struct SessionScheduler {
    selection: RuntimeSelectionPolicy,
    topology: RuntimeTopology,
}

impl Default for SessionScheduler {
    fn default() -> Self {
        Self {
            selection: RuntimeSelectionPolicy::default(),
            topology: RuntimeTopology::default(),
        }
    }
}

impl SessionScheduler {
    fn new(set: &RuntimeSet) -> Self {
        Self {
            selection: RuntimeSelectionPolicy::Fixed(set.active_id()),
            topology: RuntimeTopology::from_set(set),
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

    fn set_selection_policy(&mut self, set: &mut RuntimeSet, selection: RuntimeSelectionPolicy) {
        self.selection = selection;
        self.sync_active_with_policy(set);
    }

    fn on_runtime_topology_changed(&mut self, set: &mut RuntimeSet) {
        self.topology = RuntimeTopology::from_set(set);
        self.sync_active_with_policy(set);
    }

    fn advance_selection(&mut self, set: &mut RuntimeSet) {
        match self.selection {
            RuntimeSelectionPolicy::Fixed(id) => {
                let _ = set.set_active(id);
            }
            RuntimeSelectionPolicy::RoundRobin { scope, .. } => {
                if let Some(next) = self.next_round_robin_id(set, scope) {
                    let _ = set.set_active(next);
                }
            }
        }
    }

    fn on_progress(&mut self, set: &mut RuntimeSet, progress: SessionProgress) {
        if !self.selection.should_advance(progress) {
            return;
        }
        self.advance_selection(set);
    }

    fn sync_active_with_policy(&mut self, set: &mut RuntimeSet) {
        match self.selection {
            RuntimeSelectionPolicy::Fixed(id) => {
                let _ = set.set_active(id);
            }
            RuntimeSelectionPolicy::RoundRobin { scope, .. } => {
                if let Some(id) = self.sync_round_robin_id(set, scope) {
                    let _ = set.set_active(id);
                }
            }
        }
    }

    fn next_round_robin_id(
        &self,
        set: &RuntimeSet,
        scope: RuntimeSelectionScope,
    ) -> Option<RuntimeId> {
        if set.runtimes.is_empty() {
            return None;
        }

        if self.has_runtime_in_scope(set, scope) {
            self.next_runtime_in_scope(set, set.active_id(), scope)
        } else if scope.falls_back_to_slot_order() {
            Some(RuntimeId((set.active_id().0 + 1) % set.runtimes.len()))
        } else {
            Some(set.active_id())
        }
    }

    fn sync_round_robin_id(
        &self,
        set: &RuntimeSet,
        scope: RuntimeSelectionScope,
    ) -> Option<RuntimeId> {
        if set.runtimes.is_empty() {
            return None;
        }

        if !self.has_runtime_in_scope(set, scope) {
            return Some(set.active_id());
        }

        let active = set.active_id();
        let active_role = set.metadata(active).map(|meta| meta.role);
        if active_role.is_some_and(|role| scope.matches(role)) {
            return Some(active);
        }

        self.preferred_runtime_in_scope(set, scope)
    }

    fn has_runtime_in_scope(&self, set: &RuntimeSet, scope: RuntimeSelectionScope) -> bool {
        let _ = set;
        !self.topology.ids(scope).is_empty()
    }

    fn preferred_runtime_in_scope(
        &self,
        _set: &RuntimeSet,
        scope: RuntimeSelectionScope,
    ) -> Option<RuntimeId> {
        self.topology.preferred(scope)
    }

    fn next_runtime_in_scope(
        &self,
        set: &RuntimeSet,
        start: RuntimeId,
        scope: RuntimeSelectionScope,
    ) -> Option<RuntimeId> {
        let _ = set;
        let ids = self.topology.ids(scope);
        if ids.is_empty() {
            return None;
        }

        if let Some(position) = ids.iter().position(|id| *id == start) {
            return Some(ids[(position + 1) % ids.len()]);
        }

        ids.first().copied()
    }
}

impl SimulationSession {
    pub(crate) fn from_runtimes(runtimes: RuntimeSet) -> Self {
        Self {
            scheduler: SessionScheduler::new(&runtimes),
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
        self.scheduler.on_runtime_topology_changed(&mut self.runtimes);
        id
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn active_id(&self) -> RuntimeId {
        self.runtimes.active_id()
    }

    pub(crate) fn active_isa(&self) -> Option<Isa> {
        self.runtimes.active().map(Runtime::isa)
    }

    pub(crate) fn active_mode(&self) -> Option<ExecMode> {
        self.runtimes.active().and_then(Runtime::mode)
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
            self.scheduler
                .on_runtime_topology_changed(&mut self.runtimes);
            true
        } else {
            false
        }
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn set_active(&mut self, id: RuntimeId) -> bool {
        self.scheduler.set_active(&mut self.runtimes, id)
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn selection_policy(&self) -> &RuntimeSelectionPolicy {
        self.scheduler.selection_policy()
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn set_selection_policy(&mut self, selection: RuntimeSelectionPolicy) {
        self.scheduler
            .set_selection_policy(&mut self.runtimes, selection);
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn advance_selection(&mut self) {
        self.scheduler.advance_selection(&mut self.runtimes);
    }

    pub(crate) fn on_progress(&mut self, progress: SessionProgress) {
        self.scheduler.on_progress(&mut self.runtimes, progress);
    }

    pub(crate) fn replace_primary(&mut self, runtime: Runtime) {
        self.runtimes = RuntimeSet::new_primary(runtime);
        self.scheduler = SessionScheduler::new(&self.runtimes);
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
