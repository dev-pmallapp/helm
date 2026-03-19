// Integration test: sim_stub!, sim_warn!, sim_info! macro call sites.

use helm_diag::{DiagSink, install_monitor, uninstall_monitor};

fn open_capture() -> (DiagSink, std::path::PathBuf) {
    let path = std::env::temp_dir().join(format!(
        "helm-diag-macro-test-{}.log",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH).unwrap().subsec_nanos()
    ));
    // Clean up any leftover from prior runs.
    std::fs::remove_file(&path).ok();
    let uri = format!("file:{}", path.display());
    let (sink, monitor) = DiagSink::open(&uri).unwrap();
    install_monitor(monitor);
    (sink, path)
}

fn read_lines(path: &std::path::Path) -> Vec<String> {
    use std::io::BufRead;
    let f = std::fs::File::open(path).unwrap();
    std::io::BufReader::new(f).lines().map(|l| l.unwrap()).collect()
}

fn cleanup(path: &std::path::Path) {
    std::fs::remove_file(path).ok();
}

// T-MACRO-01
#[test]
fn sim_stub_with_pc() {
    let (sink, path) = open_capture();
    helm_diag::sim_stub!(component = "test-crate", pc = 0x4000_0000_u64, "stub message {}", 42);
    uninstall_monitor();
    drop(sink);
    let lines = read_lines(&path);
    assert!(!lines.is_empty());
    assert!(lines[0].contains("[STUB]"),       "expected STUB: {:?}", lines[0]);
    assert!(lines[0].contains("test-crate"),   "expected component: {:?}", lines[0]);
    assert!(lines[0].contains("stub message 42"), "expected message: {:?}", lines[0]);
    assert!(lines[0].contains("0x0000000040000000"),
        "expected PC in line: {:?}", lines[0]);
    cleanup(&path);
}

// T-MACRO-02
#[test]
fn sim_stub_without_pc() {
    let (sink, path) = open_capture();
    helm_diag::sim_stub!(component = "test-crate", "no pc stub");
    uninstall_monitor();
    drop(sink);
    let lines = read_lines(&path);
    assert!(!lines.is_empty());
    assert!(lines[0].contains("[STUB]"),    "expected STUB: {:?}", lines[0]);
    assert!(lines[0].contains("pc=?"),      "expected pc=? when no pc given: {:?}", lines[0]);
    assert!(lines[0].contains("no pc stub"), "expected message: {:?}", lines[0]);
    cleanup(&path);
}

// T-MACRO-03
#[test]
fn sim_warn_with_pc() {
    let (sink, path) = open_capture();
    helm_diag::sim_warn!(component = "pl011", pc = 0x0900_0018_u64, "write to read-only reg {:#x}", 0x18_u32);
    uninstall_monitor();
    drop(sink);
    let lines = read_lines(&path);
    assert!(!lines.is_empty());
    assert!(lines[0].contains("[WARN]"),    "expected WARN: {:?}", lines[0]);
    assert!(lines[0].contains("pl011"),     "expected component: {:?}", lines[0]);
    cleanup(&path);
}

// T-MACRO-04
#[test]
fn sim_warn_without_pc() {
    let (sink, path) = open_capture();
    helm_diag::sim_warn!(component = "helm-loader", "ELF has PT_LOAD with zero filesz");
    uninstall_monitor();
    drop(sink);
    let lines = read_lines(&path);
    assert!(!lines.is_empty());
    assert!(lines[0].contains("[WARN]"),    "expected WARN: {:?}", lines[0]);
    assert!(lines[0].contains("pc=?"),      "expected pc=? when no pc given: {:?}", lines[0]);
    cleanup(&path);
}

// T-MACRO-05
#[test]
fn sim_info_emits_info_level() {
    let (sink, path) = open_capture();
    helm_diag::sim_info!(component = "helm-loader", "ELF loaded: entry={:#018x}", 0x4000_0000_u64);
    uninstall_monitor();
    drop(sink);
    let lines = read_lines(&path);
    assert!(!lines.is_empty());
    assert!(lines[0].contains("[INFO]"),       "expected INFO: {:?}", lines[0]);
    assert!(lines[0].contains("helm-loader"), "expected component: {:?}", lines[0]);
    assert!(lines[0].contains("ELF loaded"),  "expected message: {:?}", lines[0]);
    cleanup(&path);
}

// T-MACRO-06
#[test]
fn macros_accept_format_args() {
    let (sink, path) = open_capture();
    let x: u32 = 0xABCD;
    let y: u64 = 0x1234_5678;
    helm_diag::sim_stub!(component = "test", "x={x:#x} y={y:#018x}");
    helm_diag::sim_warn!(component = "test", pc = y, "x={x:#x}");
    helm_diag::sim_info!(component = "test", "y={y:#018x}");
    uninstall_monitor();
    drop(sink);
    let lines = read_lines(&path);
    assert_eq!(lines.len(), 3, "expected 3 lines: {lines:?}");
    cleanup(&path);
}

// T-MACRO-07
#[test]
fn macros_without_monitor_do_not_panic() {
    uninstall_monitor();
    helm_diag::sim_stub!(component = "test", "no monitor stub");
    helm_diag::sim_warn!(component = "test", "no monitor warn");
    helm_diag::sim_info!(component = "test", "no monitor info");
}
