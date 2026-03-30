#![allow(missing_docs)]

use anyhow::{Context, Result};

// Re-export the PyO3 module init function so append_to_inittab! can find it.
// The helm-python crate sets [lib] name = "_helm_ng", so the crate name is _helm_ng.
use _helm_ng::_helm_ng;
use helm_diag::{DiagSink, install_monitor};

/// Print available CPU models and exit. Triggered by `--cpu help`.
pub fn print_cpu_help() {
    println!("Available CPU models:");
    println!();
    for (name, desc) in helm_arch::ArmCoreModel::list_models() {
        println!("  {name:<16} {desc}");
    }
    println!();
    println!("Usage: --cpu <model>   (or --core-model <model> in Python scripts)");
}

/// Print available machine/platform types and exit. Triggered by `--machine help`.
pub fn print_machine_help() {
    println!("Available machines/platforms:");
    println!();
    for info in helm_platform::list_platforms() {
        println!("  {:<16} {} [{}]", info.name, info.description, info.isa);
    }
}

/// Check argv for `--cpu help` or `--machine help`. Returns true if handled (caller should exit).
pub fn handle_help_flags(args: &[String]) -> bool {
    for (i, a) in args.iter().enumerate() {
        if a == "--cpu" || a == "--core-model" || a == "--core" {
            if let Some(next) = args.get(i + 1) {
                if next == "help" || next == "?" || next == "list" {
                    print_cpu_help();
                    return true;
                }
            }
        }
        if a.starts_with("--cpu=") {
            let val = &a["--cpu=".len()..];
            if val == "help" || val == "?" || val == "list" {
                print_cpu_help();
                return true;
            }
        }
        if a == "--machine" {
            if let Some(next) = args.get(i + 1) {
                if next == "help" || next == "?" || next == "list" {
                    print_machine_help();
                    return true;
                }
            }
        }
        if a.starts_with("--machine=") {
            let val = &a["--machine=".len()..];
            if val == "help" || val == "?" || val == "list" {
                print_machine_help();
                return true;
            }
        }
    }
    false
}

/// Boot the embedded Python interpreter and execute the selected config script.
pub fn run_python(
    script_path: Option<String>,
    script_args: &[String],
    default_script: &str,
    embedded_argv0: &str,
    embedded_log_label: &str,
    sim_trace_uri: Option<&str>,
) -> Result<()> {
    pyo3::append_to_inittab!(_helm_ng);
    pyo3::prepare_freethreaded_python();

    // Only install a sim-trace MonitorSink when --sim-trace= is explicitly
    // given. Without it the fallback behaviour applies: Stub/Warn/Info/Error
    // fall back to stderr, Branch events are silently dropped.
    // This keeps normal `helm-system-aarch64` runs quiet (no [BRNC] flood).
    let _sink = sim_trace_uri.map(|uri| {
        let (sink, monitor) = DiagSink::open_or_stderr(Some(uri));
        install_monitor(monitor);
        eprintln!("[helm] sim-trace -> {uri}");
        sink
    });

    pyo3::Python::with_gil(|py| {
        use pyo3::prelude::*;
        use pyo3::types::{PyDict, PyList};

        #[allow(deprecated)]
        let sys = py.import_bound("sys")
            .map_err(|e| anyhow::anyhow!("import sys failed: {e}"))?;
        let path = sys.getattr("path")
            .map_err(|e| anyhow::anyhow!("sys.path failed: {e}"))?;
        sys.setattr("_helm_launcher", embedded_log_label)
            .map_err(|e| anyhow::anyhow!("sys._helm_launcher failed: {e}"))?;

        let cwd = std::env::current_dir().unwrap_or_default();
        let python_dir = cwd.join("python");
        path.call_method1("insert", (0i32, python_dir.to_string_lossy().as_ref()))
            .map_err(|e| anyhow::anyhow!("sys.path insert failed: {e}"))?;

        let (code, argv0): (String, String) = match &script_path {
            Some(p) => {
                let script_dir = std::path::Path::new(p.as_str())
                    .parent()
                    .unwrap_or(std::path::Path::new("."))
                    .to_string_lossy()
                    .into_owned();
                path.call_method1("insert", (0i32, script_dir.as_str()))
                    .map_err(|e| anyhow::anyhow!("sys.path insert failed: {e}"))?;

                let code = std::fs::read_to_string(p)
                    .with_context(|| format!("cannot read script {p}"))?;
                (code, p.clone())
            }
            None => {
                log::info!("{embedded_log_label}: using embedded script");
                (default_script.to_string(), embedded_argv0.to_string())
            }
        };

        let mut argv_items = vec![argv0.clone()];
        argv_items.extend_from_slice(script_args);
        #[allow(deprecated)]
        let argv_list = PyList::new_bound(py, &argv_items);
        sys.setattr("argv", &argv_list)
            .map_err(|e| anyhow::anyhow!("sys.argv failed: {e}"))?;

        #[allow(deprecated)]
        let main_mod = py.import_bound("__main__")
            .map_err(|e| anyhow::anyhow!("import __main__ failed: {e}"))?;
        let globals = PyDict::new_bound(py);
        globals.set_item("__name__", "__main__")
            .map_err(|e| anyhow::anyhow!("set __name__ failed: {e}"))?;
        globals.set_item("__file__", &argv0)
            .map_err(|e| anyhow::anyhow!("set __file__ failed: {e}"))?;
        globals.set_item("__builtins__", main_mod.getattr("__builtins__")
            .map_err(|e| anyhow::anyhow!("read __builtins__ failed: {e}"))?)
            .map_err(|e| anyhow::anyhow!("set __builtins__ failed: {e}"))?;

        py.run_bound(&code, Some(&globals), Some(&globals)).map_err(|e: pyo3::PyErr| {
            e.print(py);
            anyhow::anyhow!("Python script exited with an error")
        })?;

        Ok(())
    })
}
