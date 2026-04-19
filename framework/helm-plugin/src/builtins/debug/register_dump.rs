use crate::api::{HelmPlugin, HelmPluginArgs};
use crate::runtime::{ArchContext, FaultInfo, HelmPluginRegistry, PluginInsnInfo};
use helm_diag::{is_monitor_discarding, sim_info};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DumpTrigger {
    Fault,
    Atexit,
    Both,
}

impl DumpTrigger {
    fn parse(value: &str) -> Self {
        match value {
            "atexit" => Self::Atexit,
            "both" => Self::Both,
            _ => Self::Fault,
        }
    }

    fn on_fault(self) -> bool {
        matches!(self, Self::Fault | Self::Both)
    }

    fn on_atexit(self) -> bool {
        matches!(self, Self::Atexit | Self::Both)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum RegisterSelection {
    Default,
    All,
    Named(Vec<String>),
}

fn parse_selection(spec: &str) -> RegisterSelection {
    if spec.trim().is_empty() {
        return RegisterSelection::Default;
    }
    if spec.trim().eq_ignore_ascii_case("all") {
        return RegisterSelection::All;
    }

    let mut out = Vec::new();
    for token in spec.split(|c: char| matches!(c, '+' | '|' | ';' | ' ')) {
        let token = token.trim().to_ascii_lowercase();
        if token.is_empty() {
            continue;
        }
        if token == "all" {
            return RegisterSelection::All;
        }
        out.push(token);
    }

    if out.is_empty() {
        RegisterSelection::Default
    } else {
        RegisterSelection::Named(out)
    }
}

#[derive(Clone, Debug)]
struct DumpConfig {
    selection: RegisterSelection,
    trigger: DumpTrigger,
    vcpu: Option<usize>,
}

#[derive(Default)]
struct Inner {
    last_contexts: BTreeMap<usize, ArchContext>,
    last_fault: Option<FaultInfo>,
}

fn selected_register_names(selection: &RegisterSelection, context: &ArchContext) -> Vec<String> {
    match selection {
        RegisterSelection::Default => context.default_register_names(),
        RegisterSelection::All => context.all_register_names(),
        RegisterSelection::Named(names) => names.clone(),
    }
}

fn format_registers(selection: &RegisterSelection, context: &ArchContext) -> Vec<String> {
    if matches!(context, ArchContext::None) {
        return vec!["arch_context=none".to_string()];
    }

    selected_register_names(selection, context)
        .into_iter()
        .map(|name| match context.lookup_register(&name) {
            Some((label, value)) => {
                if label == "current_el" {
                    format!("{label}={value}")
                } else if label == "nzcv" {
                    format!("{label}={:#010x}", value as u32)
                } else {
                    format!("{label}={value:#018x}")
                }
            }
            None => format!("{name}=<unsupported:{arch}>", arch = context.arch_name()),
        })
        .collect()
}

fn emit_dump(reason: &str, vcpu_idx: usize, selection: &RegisterSelection, context: &ArchContext) {
    let regs = format_registers(selection, context).join(" ");
    sim_info!(
        component = "register-dump",
        "reason={} vcpu={} arch={} {}",
        reason,
        vcpu_idx,
        context.arch_name(),
        regs
    );
    if is_monitor_discarding() {
        eprintln!(
            "[register-dump] reason={} vcpu={} arch={} {}",
            reason,
            vcpu_idx,
            context.arch_name(),
            regs
        );
    }
}

pub struct RegisterDump {
    config: Arc<DumpConfig>,
    inner: Arc<Mutex<Inner>>,
}

impl RegisterDump {
    pub fn new() -> Self {
        Self {
            config: Arc::new(DumpConfig {
                selection: RegisterSelection::Default,
                trigger: DumpTrigger::Fault,
                vcpu: None,
            }),
            inner: Arc::new(Mutex::new(Inner::default())),
        }
    }
}

impl Default for RegisterDump {
    fn default() -> Self {
        Self::new()
    }
}

impl HelmPlugin for RegisterDump {
    fn name(&self) -> &str {
        "register_dump"
    }

    fn install(&mut self, reg: &mut HelmPluginRegistry, args: &HelmPluginArgs) {
        self.config = Arc::new(DumpConfig {
            selection: parse_selection(args.get_or("regs", "")),
            trigger: DumpTrigger::parse(args.get_or("dump", "fault")),
            vcpu: args.get_usize("vcpu"),
        });
        self.inner = Arc::new(Mutex::new(Inner::default()));

        let inner_insn = Arc::clone(&self.inner);
        reg.on_insn_exec(Box::new(move |vcpu_idx, insn: &PluginInsnInfo| {
            inner_insn
                .lock()
                .unwrap()
                .last_contexts
                .insert(vcpu_idx, insn.context.clone());
        }));

        let inner_fault = Arc::clone(&self.inner);
        let fault_cfg = Arc::clone(&self.config);
        reg.on_fault(Box::new(move |fault: &FaultInfo| {
            let mut guard = inner_fault.lock().unwrap();
            guard.last_fault = Some(fault.clone());
            if !fault_cfg.trigger.on_fault() {
                return;
            }
            if fault_cfg.vcpu.is_some() && fault_cfg.vcpu != Some(fault.vcpu_idx) {
                return;
            }
            emit_dump("fault", fault.vcpu_idx, &fault_cfg.selection, &fault.context);
        }));
    }

    fn atexit(&mut self) {
        if !self.config.trigger.on_atexit() {
            return;
        }

        let guard = self.inner.lock().unwrap();
        if let Some(vcpu) = self.config.vcpu {
            if let Some(context) = guard.last_contexts.get(&vcpu) {
                emit_dump("atexit", vcpu, &self.config.selection, context);
            }
            return;
        }

        for (vcpu_idx, context) in &guard.last_contexts {
            emit_dump("atexit", *vcpu_idx, &self.config.selection, context);
        }
    }
}

#[cfg(test)]
#[path = "tests/register_dump.rs"]
mod tests;
