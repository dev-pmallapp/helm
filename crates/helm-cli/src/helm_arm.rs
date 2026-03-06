//! helm-arm — AArch64 SE mode runner.
//!
//! Usage:
//!     helm-arm examples/se-fish-static.py
//!     helm-arm --binary ./my-arm-binary -- arg1 arg2
//!     helm-arm --plugin insn-count --plugin syscall-trace examples/se-fish-static.py
//!
//! When given a .py file, it reads JSON-formatted configuration from it
//! (the script prints its config to stdout).  When given --binary, it
//! runs the binary directly.

use anyhow::{Context, Result};
use clap::Parser;
use helm_plugin::api::ComponentRegistry;
use helm_plugin::runtime::{register_builtins, PluginComponentAdapter};
use helm_plugin::{PluginArgs, PluginRegistry};
use std::process::Command;

#[derive(Parser)]
#[command(name = "helm-arm", about = "HELM AArch64 syscall-emulation runner")]
struct Cli {
    /// Python config script (e.g. examples/se-fish-static.py).
    #[arg()]
    script_or_binary: Option<String>,

    /// Direct binary mode (skip Python).
    #[arg(short, long)]
    binary: Option<String>,

    /// Maximum instructions to execute.
    #[arg(long, default_value_t = 100_000_000)]
    max_insns: u64,

    /// Enable a plugin by short name (e.g. insn-count, execlog, hotblocks,
    /// howvec, syscall-trace, cache).  Can be repeated.
    #[arg(long = "plugin", value_name = "NAME")]
    plugins: Vec<String>,

    /// Guest arguments (after --).
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    guest_args: Vec<String>,
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
}

fn default_max_insns() -> u64 {
    100_000_000
}

/// Resolve short plugin name to fully-qualified type name.
fn resolve_plugin_name(short: &str) -> String {
    match short {
        "cache" => "plugin.memory.cache".to_string(),
        other => format!("plugin.trace.{other}"),
    }
}

fn main() -> Result<()> {
    env_logger::init();
    let cli = Cli::parse();

    if let Some(binary) = &cli.binary {
        let mut all_guest_args = Vec::new();
        if let Some(extra) = &cli.script_or_binary {
            all_guest_args.push(extra.clone());
        }
        all_guest_args.extend(cli.guest_args.iter().cloned());
        run_binary(binary, &all_guest_args, cli.max_insns, &cli.plugins)
    } else if let Some(script) = &cli.script_or_binary {
        if script.ends_with(".py") {
            run_from_python_config(script, cli.max_insns, &cli.plugins)
        } else {
            run_binary(script, &cli.guest_args, cli.max_insns, &cli.plugins)
        }
    } else {
        eprintln!("Usage: helm-arm <script.py> or helm-arm --binary <path> [-- args...]");
        std::process::exit(1);
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

    for name in names {
        let fqn = resolve_plugin_name(name);
        match comp_reg.create(&fqn) {
            Some(comp) => {
                // Downcast the Box<dyn HelmComponent> to our adapter
                // The only concrete type created by register_builtins is
                // PluginComponentAdapter, so we can transmute via Box::into_raw.
                let raw = Box::into_raw(comp);
                // SAFETY: register_builtins only creates PluginComponentAdapter
                let mut adapter = unsafe { *Box::from_raw(raw as *mut PluginComponentAdapter) };
                adapter.install(&mut plugin_reg, &PluginArgs::new());
                adapters.push(adapter);
                eprintln!("HELM: enabled plugin {fqn}");
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
) -> Result<()> {
    eprintln!("HELM: loading config from {script}");

    let output = Command::new("python3")
        .arg(script)
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

    eprintln!(
        "HELM SE: binary={} argv={:?} max_insns={}",
        config.binary, argv, max_insns
    );

    let (plugin_reg, mut adapters) = build_plugin_registry(plugin_names)?;
    let plugins = if adapters.is_empty() {
        None
    } else {
        Some(&plugin_reg)
    };

    let result =
        helm_engine::run_aarch64_se_with_plugins(&config.binary, &argv, &envp, max_insns, plugins)?;

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
            "HELM: exited with code {} after {} instructions",
            result.exit_code, result.instructions_executed
        );
    }
    std::process::exit(result.exit_code as i32);
}

fn run_binary(
    binary: &str,
    guest_args: &[String],
    max_insns: u64,
    plugin_names: &[String],
) -> Result<()> {
    let mut argv_strings = vec![binary.to_string()];
    argv_strings.extend(guest_args.iter().cloned());
    let argv: Vec<&str> = argv_strings.iter().map(|s| s.as_str()).collect();

    let envp = ["HOME=/tmp", "TERM=dumb", "PATH=/usr/bin:/bin", "LANG=C", "USER=helm", "FISH_UNIT_TESTS_RUNNING=1"];
    eprintln!("HELM SE: binary={binary} argv={argv:?} max_insns={max_insns}");

    let (plugin_reg, mut adapters) = build_plugin_registry(plugin_names)?;
    let plugins = if adapters.is_empty() {
        None
    } else {
        Some(&plugin_reg)
    };

    let result =
        helm_engine::run_aarch64_se_with_plugins(binary, &argv, &envp, max_insns, plugins)?;

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
            "HELM: exited with code {} after {} instructions",
            result.exit_code, result.instructions_executed
        );
    }
    std::process::exit(result.exit_code as i32);
}
