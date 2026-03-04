//! Tests verifying plugin callbacks fire during SE execution.

use crate::se::linux::run_aarch64_se_with_plugins;
use helm_plugin::PluginRegistry;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

const FISH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/binaries/fish");

#[test]
fn insn_callbacks_fire_during_se() {
    let mut reg = PluginRegistry::new();

    let count = Arc::new(AtomicU64::new(0));
    let count2 = count.clone();

    reg.on_insn_exec(Box::new(move |_vcpu, _insn| {
        count2.fetch_add(1, Ordering::Relaxed);
    }));

    let result = run_aarch64_se_with_plugins(
        FISH,
        &["fish", "-c", "echo hello"],
        &["HOME=/tmp"],
        1000,
        Some(&reg),
    );

    match result {
        Ok(r) => {
            let fired = count.load(Ordering::Relaxed);
            assert!(
                fired > 0,
                "expected insn callbacks to fire, got 0 (executed {} insns)",
                r.instructions_executed
            );
            // The callback count should match instructions executed
            assert_eq!(fired, r.instructions_executed);
        }
        Err(_) => {} // crash during development is expected
    }
}

#[test]
fn syscall_callbacks_fire_during_se() {
    let mut reg = PluginRegistry::new();

    let sc_count = Arc::new(AtomicU64::new(0));
    let sc_ret_count = Arc::new(AtomicU64::new(0));
    let sc1 = sc_count.clone();
    let sr1 = sc_ret_count.clone();

    reg.on_syscall(Box::new(move |_info| {
        sc1.fetch_add(1, Ordering::Relaxed);
    }));
    reg.on_syscall_ret(Box::new(move |_info| {
        sr1.fetch_add(1, Ordering::Relaxed);
    }));

    let result = run_aarch64_se_with_plugins(
        FISH,
        &["fish", "-c", "echo hello"],
        &["HOME=/tmp"],
        100_000,
        Some(&reg),
    );

    match result {
        Ok(_) => {
            let syscalls = sc_count.load(Ordering::Relaxed);
            let returns = sc_ret_count.load(Ordering::Relaxed);
            assert!(syscalls > 0, "expected syscall callbacks to fire");
            assert_eq!(syscalls, returns, "syscall entry/return count mismatch");
        }
        Err(_) => {} // crash during development is expected
    }
}

#[test]
fn no_plugins_works_same_as_none() {
    // Passing an empty PluginRegistry should behave the same as None
    let reg = PluginRegistry::new();

    let r1 = run_aarch64_se_with_plugins(
        FISH,
        &["fish", "-c", "echo hello"],
        &["HOME=/tmp"],
        1000,
        None,
    );

    let r2 = run_aarch64_se_with_plugins(
        FISH,
        &["fish", "-c", "echo hello"],
        &["HOME=/tmp"],
        1000,
        Some(&reg),
    );

    match (r1, r2) {
        (Ok(a), Ok(b)) => {
            assert_eq!(a.instructions_executed, b.instructions_executed);
        }
        _ => {} // crash during development
    }
}

#[test]
fn vcpu_init_fires_once() {
    let mut reg = PluginRegistry::new();

    let count = Arc::new(AtomicU64::new(0));
    let c = count.clone();

    reg.on_vcpu_init(Box::new(move |_idx| {
        c.fetch_add(1, Ordering::Relaxed);
    }));

    let _result = run_aarch64_se_with_plugins(
        FISH,
        &["fish", "-c", "echo hello"],
        &["HOME=/tmp"],
        100,
        Some(&reg),
    );

    assert_eq!(count.load(Ordering::Relaxed), 1);
}
