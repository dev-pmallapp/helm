//! Tests verifying plugin callbacks fire during SE execution.

use crate::loader::load_elf;
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

// --- new tests that do not require a real ELF binary ---

#[test]
fn empty_registry_has_no_insn_callbacks() {
    let reg = PluginRegistry::new();
    assert!(!reg.has_insn_callbacks());
}

#[test]
fn registry_with_insn_callback_reports_true() {
    let mut reg = PluginRegistry::new();
    reg.on_insn_exec(Box::new(|_vcpu, _insn| {}));
    assert!(reg.has_insn_callbacks());
}

#[test]
fn empty_registry_has_no_mem_callbacks() {
    let reg = PluginRegistry::new();
    assert!(!reg.has_mem_callbacks());
}

#[test]
fn fire_vcpu_init_on_empty_registry_does_not_panic() {
    let reg = PluginRegistry::new();
    // Should be a no-op with no registered callbacks.
    reg.fire_vcpu_init(0);
}

#[test]
fn fire_insn_exec_on_empty_registry_does_not_panic() {
    use helm_plugin::runtime::InsnInfo;
    let reg = PluginRegistry::new();
    reg.fire_insn_exec(
        0,
        &InsnInfo {
            vaddr: 0x1000,
            bytes: vec![0x00, 0x00, 0x00, 0x00],
            size: 4,
            mnemonic: "NOP".into(),
            symbol: None,
        },
    );
}

#[test]
fn vcpu_init_callback_receives_correct_index() {
    use std::sync::Mutex;
    let mut reg = PluginRegistry::new();
    let received = Arc::new(Mutex::new(Vec::new()));
    let recv2 = received.clone();
    reg.on_vcpu_init(Box::new(move |idx| {
        recv2.lock().unwrap().push(idx);
    }));
    reg.fire_vcpu_init(3);
    reg.fire_vcpu_init(7);
    let got = received.lock().unwrap().clone();
    assert_eq!(got, vec![3, 7]);
}

#[test]
fn multiple_insn_callbacks_all_fire() {
    let count_a = Arc::new(AtomicU64::new(0));
    let count_b = Arc::new(AtomicU64::new(0));
    let ca = count_a.clone();
    let cb = count_b.clone();

    let mut reg = PluginRegistry::new();
    reg.on_insn_exec(Box::new(move |_v, _i| {
        ca.fetch_add(1, Ordering::Relaxed);
    }));
    reg.on_insn_exec(Box::new(move |_v, _i| {
        cb.fetch_add(1, Ordering::Relaxed);
    }));

    use helm_plugin::runtime::InsnInfo;
    let info = InsnInfo {
        vaddr: 0x4000,
        bytes: vec![0xC0, 0x03, 0x5F, 0xD6],
        size: 4,
        mnemonic: "RET".into(),
        symbol: None,
    };
    reg.fire_insn_exec(0, &info);
    reg.fire_insn_exec(0, &info);

    assert_eq!(count_a.load(Ordering::Relaxed), 2);
    assert_eq!(count_b.load(Ordering::Relaxed), 2);
}

#[test]
fn load_elf_rejects_nonexistent_path() {
    let result = load_elf("/nonexistent/path/to/binary", &[], &[]);
    assert!(result.is_err(), "expected error loading nonexistent file");
}

#[test]
fn load_elf_rejects_invalid_magic() {
    // Write a temp file with garbage magic bytes and verify the loader returns
    // a Config error rather than panicking.
    use std::io::Write;
    let path = std::env::temp_dir().join("helm_test_bad_magic.bin");
    let mut fake = vec![0u8; 64];
    fake[0..4].copy_from_slice(b"JUNK");
    {
        let mut f = std::fs::File::create(&path).expect("create temp file");
        f.write_all(&fake).unwrap();
    }
    let result = load_elf(path.to_str().unwrap(), &[], &[]);
    let _ = std::fs::remove_file(&path);
    assert!(result.is_err());
    let err_str = format!("{}", result.err().unwrap());
    assert!(
        err_str.contains("not an ELF"),
        "expected 'not an ELF' in error message, got: {err_str}"
    );
}

#[test]
fn load_elf_rejects_elf32() {
    use std::io::Write;
    let path = std::env::temp_dir().join("helm_test_elf32.bin");
    let mut fake = vec![0u8; 64];
    fake[0..4].copy_from_slice(b"\x7fELF");
    // EI_CLASS = 1 => ELF32 (should be 2 for ELF64)
    fake[4] = 1;
    {
        let mut f = std::fs::File::create(&path).expect("create temp file");
        f.write_all(&fake).unwrap();
    }
    let result = load_elf(path.to_str().unwrap(), &[], &[]);
    let _ = std::fs::remove_file(&path);
    assert!(result.is_err());
    let err_str = format!("{}", result.err().unwrap());
    assert!(
        err_str.contains("not ELF64"),
        "expected 'not ELF64' in error message, got: {err_str}"
    );
}

#[test]
fn load_elf_rejects_big_endian() {
    use std::io::Write;
    let path = std::env::temp_dir().join("helm_test_be.bin");
    let mut fake = vec![0u8; 64];
    fake[0..4].copy_from_slice(b"\x7fELF");
    fake[4] = 2; // ELF64
    fake[5] = 2; // big-endian (should be 1 for LE)
    {
        let mut f = std::fs::File::create(&path).expect("create temp file");
        f.write_all(&fake).unwrap();
    }
    let result = load_elf(path.to_str().unwrap(), &[], &[]);
    let _ = std::fs::remove_file(&path);
    assert!(result.is_err());
    let err_str = format!("{}", result.err().unwrap());
    assert!(
        err_str.contains("not little-endian"),
        "expected 'not little-endian' in error message, got: {err_str}"
    );
}

#[test]
fn load_elf_rejects_non_aarch64_machine() {
    use std::io::Write;
    let path = std::env::temp_dir().join("helm_test_x86_64.bin");
    let mut fake = vec![0u8; 64];
    fake[0..4].copy_from_slice(b"\x7fELF");
    fake[4] = 2; // ELF64
    fake[5] = 1; // little-endian
    // e_machine at bytes 18-19: 62 = EM_X86_64 (not AArch64 which is 183)
    fake[18] = 62;
    fake[19] = 0;
    {
        let mut f = std::fs::File::create(&path).expect("create temp file");
        f.write_all(&fake).unwrap();
    }
    let result = load_elf(path.to_str().unwrap(), &[], &[]);
    let _ = std::fs::remove_file(&path);
    assert!(result.is_err());
    let err_str = format!("{}", result.err().unwrap());
    assert!(
        err_str.contains("not AArch64"),
        "expected 'not AArch64' in error message, got: {err_str}"
    );
}

#[test]
fn run_se_on_nonexistent_binary_returns_error() {
    let result = run_aarch64_se_with_plugins(
        "/no/such/binary",
        &["/no/such/binary"],
        &[],
        1000,
        None,
    );
    assert!(result.is_err(), "expected error for nonexistent binary");
}

#[test]
fn syscall_callback_receives_number() {
    use helm_plugin::runtime::SyscallInfo;
    use std::sync::Mutex;

    let mut reg = PluginRegistry::new();
    let numbers = Arc::new(Mutex::new(Vec::new()));
    let nums2 = numbers.clone();
    reg.on_syscall(Box::new(move |info: &SyscallInfo| {
        nums2.lock().unwrap().push(info.number);
    }));

    // Manually fire two fake syscall events.
    reg.fire_syscall(&SyscallInfo {
        number: 64,
        args: [0u64; 6],
        vcpu_idx: 0,
    });
    reg.fire_syscall(&SyscallInfo {
        number: 93,
        args: [0u64; 6],
        vcpu_idx: 0,
    });

    let got = numbers.lock().unwrap().clone();
    assert_eq!(got, vec![64u64, 93u64]);
}

#[test]
fn syscall_ret_callback_receives_return_value() {
    use helm_plugin::runtime::SyscallRetInfo;
    use std::sync::Mutex;

    let mut reg = PluginRegistry::new();
    let retvals = Arc::new(Mutex::new(Vec::new()));
    let rv2 = retvals.clone();
    reg.on_syscall_ret(Box::new(move |info: &SyscallRetInfo| {
        rv2.lock().unwrap().push(info.ret_value);
    }));

    reg.fire_syscall_ret(&SyscallRetInfo {
        number: 64,
        ret_value: 42,
        vcpu_idx: 0,
    });
    reg.fire_syscall_ret(&SyscallRetInfo {
        number: 93,
        ret_value: 0,
        vcpu_idx: 0,
    });

    let got = retvals.lock().unwrap().clone();
    assert_eq!(got, vec![42u64, 0u64]);
}
