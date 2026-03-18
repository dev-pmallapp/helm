//! `helm-system-aarch64` — AArch64 full-system launcher.
//!
//! This is the FS-mode counterpart to `helm-aarch64`.
//! When given a `.py` script, it executes that script with the embedded
//! Python interpreter. Otherwise it runs the embedded default FS script.

use anyhow::Result;
use helm_cli::run_python;

/// Embedded default FS script — used when no `.py` is given on the command line.
const DEFAULT_SCRIPT: &str = include_str!("../../../../examples/fs/virt.py");

fn main() -> Result<()> {
    env_logger::builder()
        .filter_level(log::LevelFilter::Warn)
        .parse_default_env()
        .init();

    let raw_args: Vec<String> = std::env::args().collect();
    // Extract --sim-trace=URI before handing args to the script
    let sim_trace_uri: Option<String> = raw_args.iter()
        .find(|a| a.starts_with("--sim-trace="))
        .map(|a| a["--sim-trace=".len()..].to_string());
    let filtered_args: Vec<String> = raw_args.iter()
        .filter(|a| !a.starts_with("--sim-trace="))
        .cloned()
        .collect();
    let (script_path, script_args) = detect_script(&filtered_args);
    run_python(
        script_path,
        &script_args,
        DEFAULT_SCRIPT,
        "examples/fs/virt.py",
        "helm-system-aarch64",
        sim_trace_uri.as_deref(),
    )
}

/// Scan argv to find a `.py` script.
///
/// Returns `(Some(path), rest_of_args)` when a `.py` is found,
/// or `(None, all_args_after_binary)` when using the embedded FS script.
fn detect_script(raw: &[String]) -> (Option<String>, Vec<String>) {
    let args = &raw[1..];

    for (i, a) in args.iter().enumerate() {
        if a.ends_with(".py") {
            return (Some(a.clone()), args[i + 1..].to_vec());
        }
    }

    (None, args.to_vec())
}
