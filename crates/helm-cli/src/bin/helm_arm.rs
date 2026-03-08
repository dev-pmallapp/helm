//! helm-arm — AArch64 SE mode runner.
//!
//! Usage:
//!     helm-arm ./hello
//!     helm-arm -E HOME=/tmp -E LANG=C ./fish --no-config -c "echo hi"
//!     helm-arm -cpu o3 -caches -l2cache ./bench
//!     helm-arm -strace -plugin insn-count ./test arg1 arg2
//!     helm-arm -max-insns 1000000 -d exec ./workload
//!
//! The binary and its arguments are positional (like QEMU user-mode).
//! Python config scripts (.py) are still supported for backward compat.

use anyhow::{Context, Result};
use clap::Parser;
use helm_plugin::api::ComponentRegistry;
use helm_plugin::runtime::{register_builtins, PluginComponentAdapter};
use helm_plugin::{PluginArgs, PluginRegistry};
use std::process::Command;

#[derive(Parser)]
#[command(name = "helm-arm", about = "HELM AArch64 syscall-emulation runner")]
struct Cli {
    /// Binary to execute (or .py config script for backward compat).
    #[arg()]
    binary: Option<String>,

    /// Guest arguments (everything after the binary).
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    guest_args: Vec<String>,

    /// Maximum instructions to execute (0 = unlimited).
    #[arg(short = 'n', long = "max-insns", default_value_t = 100_000_000)]
    max_insns: u64,

    /// CPU model: atomic (IPC=1), timing, minor, o3, big.
    #[arg(long = "cpu", default_value = "atomic")]
    cpu_type: String,

    /// Enable L1 caches.
    #[arg(long = "caches", default_value_t = false)]
    caches: bool,

    /// Enable L2 cache.
    #[arg(long = "l2cache", default_value_t = false)]
    l2cache: bool,

    /// Set target environment variable (repeatable).
    #[arg(short = 'E', value_name = "VAR=VALUE")]
    env_vars: Vec<String>,

    /// Log system calls.
    #[arg(long = "strace", default_value_t = false)]
    strace: bool,

    /// Enable a plugin (repeatable: insn-count, execlog, hotblocks,
    /// howvec, syscall-trace, cache).
    #[arg(long = "plugin", value_name = "NAME")]
    plugins: Vec<String>,
}

#[derive(serde::Deserialize)]
struct PyConfig {
    binary: String,
    #[serde(default)]
    argv: Vec<String>,
    #[serde(default)]
    envp: Vec<String>,
    #[serde(default = "default_max_insns")]
    max_insns: u64,
    #[serde(default)]
    platform: Option<PlatformCfg>,
    #[serde(default)]
    plugins: Vec<String>,
}

#[derive(serde::Deserialize, Default)]
struct PlatformCfg {
    #[serde(default)]
    name: String,
    #[serde(default)]
    isa: String,
    #[serde(default)]
    cores: Vec<CoreCfg>,
    #[serde(default)]
    memory: Option<MemoryCfg>,
    #[serde(default)]
    timing: Option<TimingCfg>,
}

#[derive(serde::Deserialize, Default)]
struct CoreCfg {
    #[serde(default)]
    name: String,
    #[serde(default = "default_width")]
    width: u32,
    #[serde(default = "default_rob")]
    rob_size: u32,
    #[serde(default = "default_iq")]
    iq_size: u32,
    #[serde(default = "default_lq")]
    lq_size: u32,
    #[serde(default = "default_sq")]
    sq_size: u32,
    #[serde(default, deserialize_with = "deser_bp")]
    branch_predictor: Option<BpCfg>,
}

#[derive(serde::Deserialize, Default)]
struct BpCfg {
    #[serde(default)]
    kind: String,
}

impl BpCfg {
    fn label(&self) -> &str {
        if self.kind.is_empty() { "static" } else { &self.kind }
    }
}

fn deser_bp<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<BpCfg>, D::Error> {
    use serde::de;
    struct BpVisitor;
    impl<'de> de::Visitor<'de> for BpVisitor {
        type Value = Option<BpCfg>;
        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            write!(f, "a string or map describing a branch predictor")
        }
        fn visit_str<E: de::Error>(self, v: &str) -> Result<Self::Value, E> {
            Ok(Some(BpCfg { kind: v.to_string() }))
        }
        fn visit_map<A: de::MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
            let mut kind = String::new();
            while let Some(key) = map.next_key::<String>()? {
                if key == "kind" { kind = map.next_value()?; }
                else { let _: serde::de::IgnoredAny = map.next_value()?; }
            }
            Ok(Some(BpCfg { kind }))
        }
        fn visit_none<E: de::Error>(self) -> Result<Self::Value, E> { Ok(None) }
        fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> { Ok(None) }
    }
    d.deserialize_any(BpVisitor)
}

#[derive(serde::Deserialize, Default)]
struct MemoryCfg {
    #[serde(default = "default_dram_lat")]
    dram_latency_cycles: u64,
    #[serde(default)]
    l1i: Option<CacheCfg>,
    #[serde(default)]
    l1d: Option<CacheCfg>,
    #[serde(default)]
    l2: Option<CacheCfg>,
    #[serde(default)]
    l3: Option<CacheCfg>,
}

#[derive(serde::Deserialize)]
struct CacheCfg {
    size: String,
    #[serde(default = "default_assoc")]
    associativity: u32,
    #[serde(default = "default_cache_lat")]
    latency_cycles: u64,
    #[serde(default = "default_line")]
    line_size: u32,
}

#[derive(serde::Deserialize, Default)]
struct TimingCfg {
    #[serde(default = "default_level")]
    level: String,
    #[serde(default)]
    int_alu_latency: Option<u64>,
    #[serde(default)]
    int_mul_latency: Option<u64>,
    #[serde(default)]
    int_div_latency: Option<u64>,
    #[serde(default)]
    fp_alu_latency: Option<u64>,
    #[serde(default)]
    fp_mul_latency: Option<u64>,
    #[serde(default)]
    fp_div_latency: Option<u64>,
    #[serde(default)]
    load_latency: Option<u64>,
    #[serde(default)]
    store_latency: Option<u64>,
    #[serde(default)]
    branch_penalty: Option<u64>,
    #[serde(default)]
    l1_latency: Option<u64>,
    #[serde(default)]
    l2_latency: Option<u64>,
    #[serde(default)]
    l3_latency: Option<u64>,
    #[serde(default)]
    dram_latency: Option<u64>,
}

fn default_width() -> u32 { 4 }
fn default_rob() -> u32 { 192 }
fn default_iq() -> u32 { 64 }
fn default_lq() -> u32 { 32 }
fn default_sq() -> u32 { 32 }
fn default_dram_lat() -> u64 { 100 }
fn default_assoc() -> u32 { 8 }
fn default_cache_lat() -> u64 { 1 }
fn default_line() -> u32 { 64 }
fn default_level() -> String { "FE".into() }

fn default_max_insns() -> u64 {
    100_000_000
}

/// Resolve short plugin name to fully-qualified type name.
fn resolve_plugin_name(short: &str) -> String {
    match short {
        "cache" => "plugin.memory.cache".to_string(),
        "fault-detect" => "plugin.debug.fault-detect".to_string(),
        other => format!("plugin.trace.{other}"),
    }
}

fn main() -> Result<()> {
    env_logger::init();
    let cli = Cli::parse();

    let Some(binary) = &cli.binary else {
        eprintln!("Usage: helm-arm [options] <binary> [guest args...]");
        std::process::exit(1);
    };

    if binary.ends_with(".py") {
        run_from_python_config(binary, cli.max_insns, &cli.plugins, &cli.guest_args)
    } else {
        run_direct(&cli)
    }
}

fn run_direct(cli: &Cli) -> Result<()> {
    let binary = cli.binary.as_deref().unwrap();

    let mut argv_strings = vec![binary.to_string()];
    argv_strings.extend(cli.guest_args.iter().cloned());
    let argv: Vec<&str> = argv_strings.iter().map(|s| s.as_str()).collect();

    let envp: Vec<String> = if cli.env_vars.is_empty() {
        vec![
            "HOME=/tmp".into(), "TERM=dumb".into(),
            "PATH=/usr/bin:/bin".into(), "LANG=C".into(), "USER=helm".into(),
        ]
    } else {
        cli.env_vars.clone()
    };
    let envp_refs: Vec<&str> = envp.iter().map(|s| s.as_str()).collect();

    let mut all_plugins = cli.plugins.clone();
    if cli.strace {
        all_plugins.push("syscall-trace".to_string());
    }

    let timing_level = cli.cpu_type.as_str();
    eprintln!("HELM SE: binary={binary} argv={argv:?} cpu={timing_level} max_insns={}",
              cli.max_insns);

    let (plugin_reg, mut adapters) = build_plugin_registry(&all_plugins)?;
    let plugins = if adapters.is_empty() { None } else { Some(&plugin_reg) };

    let mut timing_model = build_timing_from_cpu_type(timing_level);
    let mut backend = helm_engine::ExecBackend::interpretive();
    let result = helm_engine::run_aarch64_se_timed(
        binary, &argv, &envp_refs, cli.max_insns,
        timing_model.as_mut(), &mut backend, None, plugins, None,
    )?;

    for adapter in &mut adapters {
        adapter.atexit();
    }

    if result.hit_limit {
        eprintln!("HELM: hit instruction limit after {} instructions", result.instructions_executed);
    } else {
        let ipc = if result.virtual_cycles > 0 {
            result.instructions_executed as f64 / result.virtual_cycles as f64
        } else {
            0.0
        };
        eprintln!("HELM: exited with code {} after {} instructions ({} cycles, IPC={:.3})",
                  result.exit_code, result.instructions_executed,
                  result.virtual_cycles, ipc);
    }
    std::process::exit(result.exit_code as i32);
}

fn build_timing_from_cpu_type(cpu: &str) -> Box<dyn helm_timing::TimingModel> {
    match cpu {
        "timing" => Box::new(helm_timing::model::ApeModelDetailed::default()),
        "minor" => Box::new(helm_timing::model::ApeModelDetailed {
            int_mul_latency: 3, int_div_latency: 9,
            load_latency: 3, branch_penalty: 6,
            ..Default::default()
        }),
        "o3" => Box::new(helm_timing::model::ApeModelDetailed {
            int_mul_latency: 3, int_div_latency: 12,
            fp_alu_latency: 4, fp_mul_latency: 5, fp_div_latency: 15,
            load_latency: 4, branch_penalty: 10,
            ..Default::default()
        }),
        "big" => Box::new(helm_timing::model::ApeModelDetailed {
            int_mul_latency: 3, int_div_latency: 10,
            fp_alu_latency: 3, fp_mul_latency: 4, fp_div_latency: 12,
            load_latency: 3, branch_penalty: 14,
            ..Default::default()
        }),
        _ => Box::new(helm_timing::model::FeModel),
    }
}

/// Build a PluginRegistry from the requested plugin names and return
/// both the registry and the list of adapters (needed for atexit).
fn build_timing(cfg: Option<&TimingCfg>) -> Box<dyn helm_timing::TimingModel> {
    let Some(cfg) = cfg else {
        return Box::new(helm_timing::model::FeModel);
    };
    match cfg.level.to_uppercase().as_str() {
        "APE" | "CAE" => Box::new(helm_timing::model::ApeModelDetailed {
            int_alu_latency: cfg.int_alu_latency.unwrap_or(1),
            int_mul_latency: cfg.int_mul_latency.unwrap_or(3),
            int_div_latency: cfg.int_div_latency.unwrap_or(12),
            fp_alu_latency: cfg.fp_alu_latency.unwrap_or(4),
            fp_mul_latency: cfg.fp_mul_latency.unwrap_or(5),
            fp_div_latency: cfg.fp_div_latency.unwrap_or(15),
            load_latency: cfg.load_latency.unwrap_or(4),
            store_latency: cfg.store_latency.unwrap_or(1),
            branch_penalty: cfg.branch_penalty.unwrap_or(10),
            l1_latency: cfg.l1_latency.unwrap_or(3),
            l2_latency: cfg.l2_latency.unwrap_or(12),
            l3_latency: cfg.l3_latency.unwrap_or(40),
            dram_latency: cfg.dram_latency.unwrap_or(200),
        }),
        _ => Box::new(helm_timing::model::FeModel),
    }
}

fn build_plugin_registry(
    names: &[String],
) -> Result<(PluginRegistry, Vec<PluginComponentAdapter>)> {
    let mut comp_reg = ComponentRegistry::new();
    register_builtins(&mut comp_reg);

    let mut plugin_reg = PluginRegistry::new();
    let mut adapters: Vec<PluginComponentAdapter> = Vec::new();

    for spec in names {
        let (name, args_str) = match spec.split_once(':') {
            Some((n, a)) => (n, a),
            None => (spec.as_str(), ""),
        };
        let fqn = resolve_plugin_name(name);
        let plugin_args = if args_str.is_empty() {
            PluginArgs::new()
        } else {
            PluginArgs::parse(args_str)
        };
        match comp_reg.create(&fqn) {
            Some(comp) => {
                let raw = Box::into_raw(comp);
                // SAFETY: register_builtins only creates PluginComponentAdapter
                let mut adapter = unsafe { *Box::from_raw(raw as *mut PluginComponentAdapter) };
                adapter.install(&mut plugin_reg, &plugin_args);
                adapters.push(adapter);
                if args_str.is_empty() {
                    eprintln!("HELM: enabled plugin {fqn}");
                } else {
                    eprintln!("HELM: enabled plugin {fqn} ({args_str})");
                }
            }
            None => {
                eprintln!("HELM: unknown plugin '{name}' (resolved as {fqn}), skipping");
            }
        }
    }

    Ok((plugin_reg, adapters))
}

fn run_from_python_config(
    script: &str,
    max_insns_override: u64,
    plugin_names: &[String],
    script_args: &[String],
) -> Result<()> {
    eprintln!("HELM: loading config from {script}");

    let output = Command::new("python3")
        .arg(script)
        .args(script_args)
        .env("PYTHONPATH", {
            let exe = std::env::current_exe().unwrap_or_default();
            let base = exe.parent().and_then(|p| p.parent()).and_then(|p| p.parent())
                .unwrap_or(std::path::Path::new("."));
            let cwd = std::env::current_dir().unwrap_or_default();
            format!("{}:{}:{}", cwd.join("python").display(),
                    cwd.display(), base.join("python").display())
        })
        .output()
        .with_context(|| format!("failed to run {script}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("{script} failed:\n{stderr}");
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let config: PyConfig = serde_json::from_str(&stdout)
        .with_context(|| format!("failed to parse JSON from {script}"))?;

    let max_insns = if max_insns_override != 100_000_000 {
        max_insns_override
    } else {
        config.max_insns
    };

    let argv: Vec<&str> = config.argv.iter().map(|s| s.as_str()).collect();
    let envp: Vec<&str> = config.envp.iter().map(|s| s.as_str()).collect();

    let platform_name = config.platform.as_ref().map(|p| p.name.as_str()).unwrap_or("default");
    let timing_level = config.platform.as_ref()
        .and_then(|p| p.timing.as_ref())
        .map(|t| t.level.as_str())
        .unwrap_or("FE");
    eprintln!(
        "HELM SE: binary={} argv={:?} max_insns={} platform={} timing={}",
        config.binary, argv, max_insns, platform_name, timing_level
    );

    if let Some(ref plat) = config.platform {
        if let Some(ref mem) = plat.memory {
            let mut parts = vec![];
            if mem.l1i.is_some() { parts.push("L1i"); }
            if mem.l1d.is_some() { parts.push("L1d"); }
            if mem.l2.is_some() { parts.push("L2"); }
            if mem.l3.is_some() { parts.push("L3"); }
            if !parts.is_empty() {
                eprintln!("HELM: caches: {} | DRAM latency: {} cycles",
                    parts.join("+"), mem.dram_latency_cycles);
            }
        }
        for core in &plat.cores {
            let bp = core.branch_predictor.as_ref()
                .map(|b| b.label()).unwrap_or("static");
            eprintln!("HELM: core {} width={} ROB={} IQ={} BP={}",
                core.name, core.width, core.rob_size, core.iq_size, bp);
        }
    }

    let all_plugins: Vec<String> = plugin_names.iter().cloned()
        .chain(config.plugins.iter().cloned())
        .collect();
    let (plugin_reg, mut adapters) = build_plugin_registry(&all_plugins)?;
    let plugins = if adapters.is_empty() {
        None
    } else {
        Some(&plugin_reg)
    };

    let mut timing_model: Box<dyn helm_timing::TimingModel> = build_timing(
        config.platform.as_ref().and_then(|p| p.timing.as_ref()),
    );
    let mut backend = helm_engine::ExecBackend::interpretive();
    let result = helm_engine::run_aarch64_se_timed(
        &config.binary, &argv, &envp, max_insns,
        timing_model.as_mut(), &mut backend, None, plugins, None,
    )?;

    // Fire atexit on all adapters
    for adapter in &mut adapters {
        adapter.atexit();
    }

    if result.hit_limit {
        eprintln!(
            "HELM: hit instruction limit after {} instructions (did not exit)",
            result.instructions_executed
        );
    } else {
        eprintln!(
            "HELM: exited with code {} after {} instructions ({} virtual cycles, IPC={:.3})",
            result.exit_code, result.instructions_executed, result.virtual_cycles,
            if result.virtual_cycles > 0 {
                result.instructions_executed as f64 / result.virtual_cycles as f64
            } else { 0.0 }
        );
    }
    std::process::exit(result.exit_code as i32);
}
