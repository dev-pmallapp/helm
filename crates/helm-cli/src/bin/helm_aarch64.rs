//! helm-arm — AArch64 SE mode runner (thin launcher).
//! helm-aarch64 — AArch64 SE mode runner (thin launcher).
//!
//! When given a `.py` script, sets `sys.argv` and executes it.
//! Otherwise runs the embedded default `run_binary.py` script.
//!
//! Usage:
//!     helm-aarch64 examples/se/run_binary.py --binary ./hello
//!     helm-aarch64 ./hello                    # uses embedded run_binary.py
//!     helm-aarch64 ./hello --no-config -c "echo hi"

use anyhow::{Context, Result};

// Re-export the PyO3 module init function so append_to_inittab! can find it.
use _helm_core::_helm_core;

/// Embedded default script — used when no `.py` is given on the command line.
const DEFAULT_SCRIPT: &str = include_str!("../../../../examples/se/run_binary.py");

fn main() -> Result<()> {
    env_logger::init();
    let raw_args: Vec<String> = std::env::args().collect();

    let (script_path, script_args) = detect_script(&raw_args);

    run_python(&script_path, &script_args)
}

/// Scan raw argv to find a `.py` script.
///
/// Returns `(script_file_or_None, args_to_forward)`.
/// When no `.py` is found, returns `(None, all_args_after_binary)`.
fn detect_script(raw: &[String]) -> (Option<String>, Vec<String>) {
    let args = &raw[1..];

    for (i, a) in args.iter().enumerate() {
        if a.ends_with(".py") {
            let rest = args[i + 1..].to_vec();
            return (Some(a.clone()), rest);
        }
    }

    // No .py found — forward everything as --binary <first_arg> <rest>
    // so the embedded run_binary.py can parse them.
    if !args.is_empty() {
        let mut forwarded = vec!["--binary".to_string(), args[0].clone()];
        if args.len() > 1 {
            forwarded.push("--args".to_string());
            forwarded.extend_from_slice(&args[1..]);
        }
        (None, forwarded)
    } else {
        (None, Vec::new())
    }
}

/// Execute a Python script (from file or embedded string) with sys.argv set.
fn run_python(script_path: &Option<String>, script_args: &[String]) -> Result<()> {
    pyo3::append_to_inittab!(_helm_core);
    pyo3::prepare_freethreaded_python();

    pyo3::Python::with_gil(|py| {
        use pyo3::prelude::*;
        use pyo3::types::PyList;

        let cwd = std::env::current_dir().unwrap_or_default();
        let python_dir = cwd.join("python");

        let sys = py
            .import("sys")
            .map_err(|e| anyhow::anyhow!("failed to import sys: {e}"))?;
        let path = sys
            .getattr("path")
            .map_err(|e| anyhow::anyhow!("failed to get sys.path: {e}"))?;
        path.call_method1("insert", (0, python_dir.to_string_lossy().as_ref()))
            .map_err(|e| anyhow::anyhow!("failed to update sys.path: {e}"))?;

        let (code, argv0) = match script_path {
            Some(ref p) => {
                let script_dir = std::path::Path::new(p.as_str())
                    .parent()
                    .unwrap_or(std::path::Path::new("."))
                    .to_string_lossy()
                    .into_owned();
                path.call_method1("insert", (0, script_dir.as_str()))
                    .map_err(|e| anyhow::anyhow!("failed to update sys.path: {e}"))?;

                let code =
                    std::fs::read_to_string(p).with_context(|| format!("failed to read {p}"))?;
                (code, p.clone())
            }
            None => {
                eprintln!("HELM: using embedded run_binary.py script");
                (DEFAULT_SCRIPT.to_string(), "<embedded-se>".to_string())
            }
        };

        let mut argv_items: Vec<String> = vec![argv0];
        argv_items.extend_from_slice(script_args);
        let argv_list = PyList::new(py, &argv_items)
            .map_err(|e| anyhow::anyhow!("failed to build sys.argv: {e}"))?;
        sys.setattr("argv", argv_list)
            .map_err(|e| anyhow::anyhow!("failed to set sys.argv: {e}"))?;

        py.run(&std::ffi::CString::new(code).unwrap(), None, None)
            .map_err(|e| {
                e.print(py);
                anyhow::anyhow!("Python script failed")
            })?;

        Ok(())
    })
}
