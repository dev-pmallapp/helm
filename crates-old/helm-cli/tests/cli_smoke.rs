use std::process::Command;

fn helm_bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_helm"))
}

fn helm_arm_bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_helm-arm"))
}

#[test]
fn helm_no_args_exits_nonzero() {
    let status = helm_bin().output().expect("failed to run helm");
    assert!(!status.status.success());
}

#[test]
fn helm_help_flag_exits_zero() {
    let output = helm_bin()
        .arg("--help")
        .output()
        .expect("failed to run helm");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("HELM"));
}

#[test]
fn helm_arm_no_args_exits_nonzero() {
    let output = helm_arm_bin().output().expect("failed to run helm-arm");
    assert!(!output.status.success());
}

#[test]
fn helm_arm_help_flag_exits_zero() {
    let output = helm_arm_bin()
        .arg("--help")
        .output()
        .expect("failed to run helm-arm");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("helm-arm"));
}

#[test]
fn helm_invalid_binary_exits_nonzero() {
    let output = helm_bin()
        .args(["--binary", "/nonexistent/path/to/binary"])
        .output()
        .expect("failed to run helm");
    assert!(!output.status.success());
}

#[test]
fn helm_arm_invalid_binary_exits_nonzero() {
    let output = helm_arm_bin()
        .args(["--binary", "/nonexistent/path/to/binary"])
        .output()
        .expect("failed to run helm-arm");
    assert!(!output.status.success());
}
