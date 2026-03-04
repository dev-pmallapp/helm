//! helm-arm — AArch64 SE mode runner.
//!
//! Usage:
//!     helm-arm examples/se-fish-static.py
//!     helm-arm --binary ./my-arm-binary -- arg1 arg2
//!
//! When given a .py file, it reads JSON-formatted configuration from it
//! (the script prints its config to stdout).  When given --binary, it
//! runs the binary directly.

use anyhow::{Context, Result};
use clap::Parser;
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

fn main() -> Result<()> {
    env_logger::init();
    let cli = Cli::parse();

    if let Some(binary) = &cli.binary {
        run_binary(binary, &cli.guest_args, cli.max_insns)
    } else if let Some(script) = &cli.script_or_binary {
        if script.ends_with(".py") {
            run_from_python_config(script, cli.max_insns)
        } else {
            run_binary(script, &cli.guest_args, cli.max_insns)
        }
    } else {
        eprintln!("Usage: helm-arm <script.py> or helm-arm --binary <path> [-- args...]");
        std::process::exit(1);
    }
}

fn run_from_python_config(script: &str, max_insns_override: u64) -> Result<()> {
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

    let result = helm_engine::run_aarch64_se(&config.binary, &argv, &envp, max_insns)?;

    eprintln!(
        "HELM: exited with code {} after {} instructions",
        result.exit_code, result.instructions_executed
    );
    std::process::exit(result.exit_code as i32);
}

fn run_binary(binary: &str, guest_args: &[String], max_insns: u64) -> Result<()> {
    let mut argv_strings = vec![binary.to_string()];
    argv_strings.extend(guest_args.iter().cloned());
    let argv: Vec<&str> = argv_strings.iter().map(|s| s.as_str()).collect();

    let envp = ["HOME=/tmp", "TERM=dumb", "PATH=/usr/bin:/bin", "LANG=C"];

    eprintln!("HELM SE: binary={binary} argv={argv:?} max_insns={max_insns}");

    let result = helm_engine::run_aarch64_se(binary, &argv, &envp, max_insns)?;

    eprintln!(
        "HELM: exited with code {} after {} instructions",
        result.exit_code, result.instructions_executed
    );
    std::process::exit(result.exit_code as i32);
}
