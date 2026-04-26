use crate::api::{HelmPlugin, HelmPluginArgs};
use crate::runtime::{ArchContext, ExceptionCause, ExceptionInfo, HelmPluginRegistry};
use helm_diag::sim_info;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

/// Default ring-buffer depth when the user does not pass `max=`.
const DEFAULT_MAX: usize = 64;

/// EC value (top 6 bits of ESR_EL2) for HVC from AArch64.
const EC_HVC_A64: u8 = 0x16;
/// EC value for SMC from AArch64.
const EC_SMC_A64: u8 = 0x17;

/// Trace mode: which hypercall flavours the plugin should record.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TraceMode {
    HvcOnly,
    SmcOnly,
    Both,
}

impl TraceMode {
    fn parse(args: &HelmPluginArgs) -> Self {
        match args.get_or("kind", "hvc") {
            "smc" => Self::SmcOnly,
            "both" | "all" => Self::Both,
            _ => Self::HvcOnly,
        }
    }

    fn matches(self, ec: u8) -> bool {
        match self {
            Self::HvcOnly => ec == EC_HVC_A64,
            Self::SmcOnly => ec == EC_SMC_A64,
            Self::Both => ec == EC_HVC_A64 || ec == EC_SMC_A64,
        }
    }
}

/// One captured hypercall trap.
#[derive(Clone, Debug)]
struct TraceEntry {
    insn_count: u64,
    vcpu_idx: usize,
    cause_label: &'static str,
    from_el: u8,
    target_el: u8,
    vector_pc: u64,
    elr: u64,
    imm16: u16,
    /// Snapshot of x0..x7 at the moment of the trap. These cover every
    /// hypercall ABI we currently care about (PSCI: x0..x3, L4 IPC:
    /// x0..x7).
    args: [u64; 8],
}

struct Inner {
    mode: TraceMode,
    max: usize,
    entries: VecDeque<TraceEntry>,
    dropped: u64,
    matched: u64,
    dump_on_atexit: bool,
}

impl Inner {
    fn new() -> Self {
        Self {
            mode: TraceMode::HvcOnly,
            max: DEFAULT_MAX,
            entries: VecDeque::new(),
            dropped: 0,
            matched: 0,
            dump_on_atexit: true,
        }
    }

    fn install_args(&mut self, args: &HelmPluginArgs) {
        self.mode = TraceMode::parse(args);
        self.max = args.get_usize("max").unwrap_or(DEFAULT_MAX).max(1);
        self.dump_on_atexit = args.get_bool("dump").unwrap_or(true);
        self.entries.clear();
        self.dropped = 0;
        self.matched = 0;
    }

    fn record(&mut self, info: &ExceptionInfo) {
        if info.cause != ExceptionCause::Sync {
            return;
        }
        let ec = info.ec();
        if !self.mode.matches(ec) {
            return;
        }
        self.matched += 1;
        let cause_label = match ec {
            EC_HVC_A64 => "HVC",
            EC_SMC_A64 => "SMC",
            _ => "?",
        };
        let mut args = [0u64; 8];
        if let ArchContext::Aarch64 { x, .. } = &info.context {
            args.copy_from_slice(&x[0..8]);
        }
        if self.entries.len() == self.max {
            self.entries.pop_front();
            self.dropped = self.dropped.saturating_add(1);
        }
        self.entries.push_back(TraceEntry {
            insn_count: info.insn_count,
            vcpu_idx: info.vcpu_idx,
            cause_label,
            from_el: info.from_el,
            target_el: info.target_el,
            vector_pc: info.vector_pc,
            elr: info.elr,
            imm16: info.imm16(),
            args,
        });
        // Always emit the line live so `sim_info!` consumers see hypercall
        // traffic in real time without waiting for atexit.
        emit_entry(self.entries.back().unwrap());
    }
}

fn emit_entry(entry: &TraceEntry) {
    sim_info!(
        component = "hvc-trace",
        pc = entry.vector_pc,
        "{cause} vcpu={vcpu} from_el={from_el} target_el={target_el} insn={insn} imm16={imm16:#06x} elr={elr:#018x} x0={x0:#x} x1={x1:#x} x2={x2:#x} x3={x3:#x} x4={x4:#x} x5={x5:#x} x6={x6:#x} x7={x7:#x}",
        cause = entry.cause_label,
        vcpu = entry.vcpu_idx,
        from_el = entry.from_el,
        target_el = entry.target_el,
        insn = entry.insn_count,
        imm16 = entry.imm16,
        elr = entry.elr,
        x0 = entry.args[0],
        x1 = entry.args[1],
        x2 = entry.args[2],
        x3 = entry.args[3],
        x4 = entry.args[4],
        x5 = entry.args[5],
        x6 = entry.args[6],
        x7 = entry.args[7],
    );
}

/// Built-in plugin: capture HVC (and optionally SMC) trap entries with
/// argument registers, target EL, return address, and immediate.
///
/// Args (`key=value,...`):
/// - `kind`: `hvc` (default), `smc`, or `both`/`all`
/// - `max`: ring-buffer depth (default `64`)
/// - `dump`: `true`/`false` whether to print a final summary at exit
///
/// Lives on the `on_exception` hook and filters by ESR EC field, so it adds
/// no per-instruction overhead.
pub struct HvcTrace {
    inner: Arc<Mutex<Inner>>,
}

impl HvcTrace {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(Inner::new())),
        }
    }
}

impl Default for HvcTrace {
    fn default() -> Self {
        Self::new()
    }
}

impl HelmPlugin for HvcTrace {
    fn name(&self) -> &str {
        "hvc_trace"
    }

    fn install(&mut self, reg: &mut HelmPluginRegistry, args: &HelmPluginArgs) {
        self.inner = Arc::new(Mutex::new(Inner::new()));
        self.inner.lock().unwrap().install_args(args);

        let inner = Arc::clone(&self.inner);
        reg.on_exception(Box::new(move |info: &ExceptionInfo| {
            inner.lock().unwrap().record(info);
        }));
    }

    fn atexit(&mut self) {
        let guard = self.inner.lock().unwrap();
        if !guard.dump_on_atexit {
            return;
        }
        sim_info!(
            component = "hvc-trace",
            "summary mode={mode:?} matched={matched} dropped={dropped} buffered={buffered}",
            mode = guard.mode,
            matched = guard.matched,
            dropped = guard.dropped,
            buffered = guard.entries.len(),
        );
    }
}

#[cfg(test)]
#[path = "tests/hvc_trace.rs"]
mod tests;
