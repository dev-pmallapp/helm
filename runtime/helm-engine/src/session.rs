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

    fn is_primary_compute(self) -> bool {
        matches!(self, Self::PrimaryCpu)
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

pub(crate) enum RuntimeSelectionPolicy {
    Fixed(RuntimeId),
    #[allow(dead_code)]
    RoundRobin,
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

    fn set_selection_policy(&mut self, set: &mut RuntimeSet, selection: RuntimeSelectionPolicy) {
        self.selection = selection;
        self.sync_active_with_policy(set);
    }

    fn advance_selection(&mut self, set: &mut RuntimeSet) {
        match self.selection {
            RuntimeSelectionPolicy::Fixed(id) => {
                let _ = set.set_active(id);
            }
            RuntimeSelectionPolicy::RoundRobin => {
                if let Some(next) = self.next_round_robin_id(set) {
                    let _ = set.set_active(next);
                }
            }
        }
    }

    fn on_progress(&mut self, set: &mut RuntimeSet, _progress: SessionProgress) {
        self.advance_selection(set);
    }

    fn sync_active_with_policy(&mut self, set: &mut RuntimeSet) {
        match self.selection {
            RuntimeSelectionPolicy::Fixed(id) => {
                let _ = set.set_active(id);
            }
            RuntimeSelectionPolicy::RoundRobin => {
                if let Some(id) = self.sync_round_robin_id(set) {
                    let _ = set.set_active(id);
                }
            }
        }
    }

    fn next_round_robin_id(&self, set: &RuntimeSet) -> Option<RuntimeId> {
        if set.runtimes.is_empty() {
            return None;
        }

        if self.has_round_robin_compute_roles(set) {
            self.next_runtime_with_role(set, set.active_id())
        } else {
            Some(RuntimeId((set.active_id().0 + 1) % set.runtimes.len()))
        }
    }

    fn sync_round_robin_id(&self, set: &RuntimeSet) -> Option<RuntimeId> {
        if set.runtimes.is_empty() {
            return None;
        }

        if !self.has_round_robin_compute_roles(set) {
            return Some(set.active_id());
        }

        let active = set.active_id();
        let active_role = set.metadata(active).map(|meta| meta.role);
        if active_role.is_some_and(RuntimeRole::participates_in_round_robin) {
            return Some(active);
        }

        self.primary_compute_runtime(set)
            .or_else(|| self.first_compute_runtime(set))
    }

    fn has_round_robin_compute_roles(&self, set: &RuntimeSet) -> bool {
        set.metadata
            .iter()
            .any(|meta| meta.role.participates_in_round_robin())
    }

    fn primary_compute_runtime(&self, set: &RuntimeSet) -> Option<RuntimeId> {
        set.metadata
            .iter()
            .position(|meta| meta.role.is_primary_compute())
            .map(RuntimeId)
    }

    fn first_compute_runtime(&self, set: &RuntimeSet) -> Option<RuntimeId> {
        set.metadata
            .iter()
            .position(|meta| meta.role.participates_in_round_robin())
            .map(RuntimeId)
    }

    fn next_runtime_with_role(&self, set: &RuntimeSet, start: RuntimeId) -> Option<RuntimeId> {
        let len = set.runtimes.len();
        if len == 0 {
            return None;
        }

        for offset in 1..=len {
            let idx = (start.0 + offset) % len;
            let role = set.metadata(RuntimeId(idx)).map(|meta| meta.role);
            if role.is_some_and(RuntimeRole::participates_in_round_robin) {
                return Some(RuntimeId(idx));
            }
        }
        None
    }
}

impl SimulationSession {
    pub(crate) fn from_runtimes(runtimes: RuntimeSet) -> Self {
        let active = runtimes.active_id();
        Self {
            runtimes,
            scheduler: SessionScheduler::new(active),
        }
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn new_primary(runtime: Runtime) -> Self {
        Self::from_runtimes(RuntimeSet::new_primary(runtime))
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn push(&mut self, runtime: Runtime) -> RuntimeId {
        self.runtimes.push(runtime)
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
            self.scheduler.sync_active_with_policy(&mut self.runtimes);
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
        self.scheduler = SessionScheduler::new(RuntimeId(0));
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
