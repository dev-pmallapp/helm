//! Tests for SeSession pause/load/continue.

use crate::se::session::{SeSession, StopReason};

const FISH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/binaries/fish");

#[test]
fn session_run_returns_insn_limit() {
    let mut s =
        SeSession::new(FISH, &["fish", "--no-config", "-c", "true"], &["HOME=/tmp"]).unwrap();
    let reason = s.run(100);
    assert_eq!(reason, StopReason::InsnLimit);
    assert!(s.insn_count() >= 100);
}

#[test]
fn session_run_until_insns() {
    let mut s =
        SeSession::new(FISH, &["fish", "--no-config", "-c", "true"], &["HOME=/tmp"]).unwrap();
    s.run(500);
    let r = s.run_until_insns(1000);
    assert_eq!(r, StopReason::InsnLimit);
    assert!(s.insn_count() >= 1000);
}

#[test]
fn session_hot_load_plugin() {
    let mut s =
        SeSession::new(FISH, &["fish", "--no-config", "-c", "true"], &["HOME=/tmp"]).unwrap();
    s.run(1000);
    let loaded = s.add_plugin("fault-detect", "");
    assert!(loaded, "fault-detect plugin should be found");
    s.run(1000);
    assert!(s.insn_count() >= 2000);
}

#[test]
fn session_hot_load_with_args() {
    let mut s =
        SeSession::new(FISH, &["fish", "--no-config", "-c", "true"], &["HOME=/tmp"]).unwrap();
    s.run(500);
    let loaded = s.add_plugin("fault-detect", "after_insns=100,ring=16");
    assert!(loaded);
    s.run(500);
}

#[test]
fn session_run_until_pc() {
    let mut s =
        SeSession::new(FISH, &["fish", "--no-config", "-c", "true"], &["HOME=/tmp"]).unwrap();
    let entry = 0x411120u64;
    let r = s.run_until_pc(entry, 10);
    assert!(
        r == StopReason::Breakpoint { pc: entry } || r == StopReason::InsnLimit,
        "expected breakpoint or limit, got {r:?}"
    );
}

#[test]
fn session_unknown_plugin_returns_false() {
    let mut s =
        SeSession::new(FISH, &["fish", "--no-config", "-c", "true"], &["HOME=/tmp"]).unwrap();
    assert!(!s.add_plugin("nonexistent-plugin-xyz", ""));
}
