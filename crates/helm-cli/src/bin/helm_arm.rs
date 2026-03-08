//! helm-arm — AArch64 SE mode runner.
//!
//! Usage:
//!     helm-arm ./hello
//!     helm-arm -E HOME=/tmp -E LANG=C ./fish --no-config -c "echo hi"
//!     helm-arm -cpu o3 -caches -l2cache ./bench
//!     helm-arm -strace -plugin insn-count ./test arg1 arg2
//!     helm-arm -max-insns 1000000 -d exec ./workload
//!     helm-arm examples/se/aarch64/run_binary.py
//!
//! The binary and its arguments are positional (like QEMU user-mode).
//! Python scripts (.py) are executed with the embedded interpreter,
//! giving them direct access to `_helm_core` (SeSession, etc.).

use anyhow::{Context, Result};
use clap::Parser;
use helm_plugin::api::ComponentRegistry;
use helm_plugin::runtime::{register_builtins, PluginComponentAdapter};
use helm_plugin::{PluginArgs, PluginRegistry};

// Re-export the PyO3 module init function so append_to_inittab! can find it.
use _helm_core::_helm_core;

#[derive(Parser)]
#[command(name = "helm-arm", about = "HELM AArch64 syscall-emulation runner")]
struct Cli {
    /// Binary to execute (or .py script for embedded Python).
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
        run_from_python(binary)
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
            "HOME=/tmp".into(),
            "TERM=dumb".into(),
            "PATH=/usr/bin:/bin".into(),
            "LANG=C".into(),
            "USER=helm".into(),
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
    eprintln!(
        "HELM SE: binary={binary} argv={argv:?} cpu={timing_level} max_insns={}",
        cli.max_insns
    );

    let (plugin_reg, mut adapters) = build_plugin_registry(&all_plugins)?;
    let plugins = if adapters.is_empty() {
        None
    } else {
        Some(&plugin_reg)
    };

    let mut timing_model = build_timing_from_cpu_type(timing_level);
    let mut backend = helm_engine::ExecBackend::interpretive();
    let result = helm_engine::run_aarch64_se_timed(
        binary,
        &argv,
        &envp_refs,
        cli.max_insns,
        timing_model.as_mut(),
        &mut backend,
        None,
        plugins,
        None,
    )?;

    for adapter in &mut adapters {
        adapter.atexit();
    }

    if result.hit_limit {
        eprintln!(
            "HELM: hit instruction limit after {} instructions",
            result.instructions_executed
        );
    } else {
        let ipc = if result.virtual_cycles > 0 {
            result.instructions_executed as f64 / result.virtual_cycles as f64
        } else {
            0.0
        };
        eprintln!(
            "HELM: exited with code {} after {} instructions ({} cycles, IPC={:.3})",
            result.exit_code, result.instructions_executed, result.virtual_cycles, ipc
        );
    }
    std::process::exit(result.exit_code as i32);
}

fn build_timing_from_cpu_type(cpu: &str) -> Box<dyn helm_timing::TimingModel> {
    match cpu {
        "timing" => Box::new(helm_timing::model::ApeModelDetailed::default()),
        "minor" => Box::new(helm_timing::model::ApeModelDetailed {
            int_mul_latency: 3,
            int_div_latency: 9,
            load_latency: 3,
            branch_penalty: 6,
            ..Default::default()
        }),
        "o3" => Box::new(helm_timing::model::ApeModelDetailed {
            int_mul_latency: 3,
            int_div_latency: 12,
            fp_alu_latency: 4,
            fp_mul_latency: 5,
            fp_div_latency: 15,
            load_latency: 4,
            branch_penalty: 10,
            ..Default::default()
        }),
        "big" => Box::new(helm_timing::model::ApeModelDetailed {
            int_mul_latency: 3,
            int_div_latency: 10,
            fp_alu_latency: 3,
            fp_mul_latency: 4,
            fp_div_latency: 12,
            load_latency: 3,
            branch_penalty: 14,
            ..Default::default()
        }),
        _ => Box::new(helm_timing::model::FeModel),
    }
}

/// Build a PluginRegistry from the requested plugin names and return
/// both the registry and the list of adapters (needed for atexit).
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

// ── Embedded Python interpreter ─────────────────────────────────────────────

/// Run a `.py` script with the embedded Python interpreter.
///
/// The `_helm_core` module is registered before Python starts, so
/// scripts can `import _helm_core` and use `SeSession` directly.
fn run_from_python(script: &str) -> Result<()> {
    eprintln!("HELM: running Python script with embedded interpreter: {script}");

    pyo3::append_to_inittab!(_helm_core);
    pyo3::prepare_freethreaded_python();

    pyo3::Python::with_gil(|py| {
        use pyo3::prelude::*;

        let cwd = std::env::current_dir().unwrap_or_default();
        let python_dir = cwd.join("python");
        let script_dir = std::path::Path::new(script)
            .parent()
            .unwrap_or(std::path::Path::new("."))
            .to_string_lossy()
            .into_owned();

        let sys = py
            .import("sys")
            .map_err(|e| anyhow::anyhow!("failed to import sys: {e}"))?;
        let path = sys
            .getattr("path")
            .map_err(|e| anyhow::anyhow!("failed to get sys.path: {e}"))?;
        path.call_method1("insert", (0, python_dir.to_string_lossy().as_ref()))
            .map_err(|e| anyhow::anyhow!("failed to update sys.path: {e}"))?;
        path.call_method1("insert", (0, script_dir.as_str()))
            .map_err(|e| anyhow::anyhow!("failed to update sys.path: {e}"))?;

        let code =
            std::fs::read_to_string(script).with_context(|| format!("failed to read {script}"))?;

        py.run(&std::ffi::CString::new(code).unwrap(), None, None)
            .map_err(|e| {
                e.print(py);
                anyhow::anyhow!("Python script failed")
            })?;

        Ok(())
    })
}
