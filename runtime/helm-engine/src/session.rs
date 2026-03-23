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

pub(crate) struct RuntimeSet {
    pub(crate) active: RuntimeId,
    pub(crate) runtimes: Vec<Runtime>,
}

impl Default for RuntimeSet {
    fn default() -> Self {
        Self {
            active: RuntimeId(0),
            runtimes: Vec::new(),
        }
    }
}

impl RuntimeSet {
    pub(crate) fn new_primary(runtime: Runtime) -> Self {
        Self {
            active: RuntimeId(0),
            runtimes: vec![runtime],
        }
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn push(&mut self, runtime: Runtime) -> RuntimeId {
        let id = RuntimeId(self.runtimes.len());
        self.runtimes.push(runtime);
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
    selection: RuntimeSelectionPolicy,
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

impl SimulationSession {
    pub(crate) fn from_runtimes(runtimes: RuntimeSet) -> Self {
        let active = runtimes.active_id();
        Self {
            runtimes,
            selection: RuntimeSelectionPolicy::Fixed(active),
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
    pub(crate) fn set_active(&mut self, id: RuntimeId) -> bool {
        let changed = self.runtimes.set_active(id);
        if changed {
            self.selection = RuntimeSelectionPolicy::Fixed(id);
        }
        changed
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn selection_policy(&self) -> &RuntimeSelectionPolicy {
        &self.selection
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn set_selection_policy(&mut self, selection: RuntimeSelectionPolicy) {
        self.selection = selection;
        self.sync_active_with_policy();
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn advance_selection(&mut self) {
        match self.selection {
            RuntimeSelectionPolicy::Fixed(id) => {
                let _ = self.runtimes.set_active(id);
            }
            RuntimeSelectionPolicy::RoundRobin => {
                if self.runtimes.runtimes.is_empty() {
                    return;
                }
                let next = (self.runtimes.active_id().0 + 1) % self.runtimes.runtimes.len();
                let _ = self.runtimes.set_active(RuntimeId(next));
            }
        }
    }

    pub(crate) fn replace_primary(&mut self, runtime: Runtime) {
        self.runtimes = RuntimeSet::new_primary(runtime);
        self.selection = RuntimeSelectionPolicy::Fixed(RuntimeId(0));
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

    fn sync_active_with_policy(&mut self) {
        match self.selection {
            RuntimeSelectionPolicy::Fixed(id) => {
                let _ = self.runtimes.set_active(id);
            }
            RuntimeSelectionPolicy::RoundRobin => {}
        }
    }
}
