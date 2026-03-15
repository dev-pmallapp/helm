//! helm-plugin integration tests.

use helm_plugin::runtime::*;
use helm_plugin::api::PluginArgs;

// ── PluginArgs tests ───────────────────────────────────────────────────────────

#[test]
fn plugin_args_parse() {
    let args = PluginArgs::parse("size=32KB,assoc=8,verbose=true");
    assert_eq!(args.get("size"), Some("32KB"));
    assert_eq!(args.get("assoc"), Some("8"));
    assert_eq!(args.get_usize("assoc"), Some(8));
    assert_eq!(args.get("missing"), None);
    assert_eq!(args.get_or("missing", "default"), "default");
}

#[test]
fn plugin_args_empty() {
    let args = PluginArgs::parse("");
    assert_eq!(args.get("any"), None);
}

#[test]
fn plugin_args_get_bool() {
    let args = PluginArgs::parse("verbose=true,quiet=false,flag=1");
    assert_eq!(args.get_bool("verbose"), Some(true));
    assert_eq!(args.get_bool("quiet"), Some(false));
    assert_eq!(args.get_bool("flag"), Some(true));
    assert_eq!(args.get_bool("missing"), None);
}

#[test]
fn plugin_args_get_usize_missing() {
    let args = PluginArgs::parse("x=10");
    assert_eq!(args.get_usize("missing"), None);
    assert_eq!(args.get_usize("x"), Some(10));
}

// ── PluginRegistry tests ───────────────────────────────────────────────────────

#[test]
fn registry_no_callbacks_by_default() {
    let reg = PluginRegistry::new();
    assert!(!reg.has_insn_callbacks());
    assert!(!reg.has_mem_callbacks());
    assert!(!reg.has_branch_callbacks());
}

#[test]
fn registry_insn_callback_fires() {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;

    let mut reg = PluginRegistry::new();
    let counter = Arc::new(AtomicU64::new(0));
    let c = counter.clone();
    reg.on_insn_exec(Box::new(move |_vcpu, _insn| {
        c.fetch_add(1, Ordering::Relaxed);
    }));

    assert!(reg.has_insn_callbacks());

    let insn = InsnInfo { pc: 0x1000, raw: 0, size: 4, class: InsnClass::IntAlu };
    reg.fire_insn_exec(0, &insn);
    reg.fire_insn_exec(0, &insn);
    reg.fire_insn_exec(0, &insn);

    assert_eq!(counter.load(Ordering::Relaxed), 3);
}

#[test]
fn registry_mem_filter_reads_only() {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;

    let mut reg = PluginRegistry::new();
    let counter = Arc::new(AtomicU64::new(0));
    let c = counter.clone();
    reg.on_mem_access(MemFilter::ReadsOnly, Box::new(move |_vcpu, _info| {
        c.fetch_add(1, Ordering::Relaxed);
    }));

    let load = MemInfo { vaddr: 0x2000, size: 8, is_store: false, is_atomic: false };
    let store = MemInfo { vaddr: 0x2000, size: 8, is_store: true, is_atomic: false };

    reg.fire_mem_access(0, &load);
    reg.fire_mem_access(0, &store);  // should NOT fire
    reg.fire_mem_access(0, &load);

    assert_eq!(counter.load(Ordering::Relaxed), 2);
}

#[test]
fn registry_mem_filter_writes_only() {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;

    let mut reg = PluginRegistry::new();
    let counter = Arc::new(AtomicU64::new(0));
    let c = counter.clone();
    reg.on_mem_access(MemFilter::WritesOnly, Box::new(move |_vcpu, _info| {
        c.fetch_add(1, Ordering::Relaxed);
    }));

    let load = MemInfo { vaddr: 0x2000, size: 8, is_store: false, is_atomic: false };
    let store = MemInfo { vaddr: 0x2000, size: 8, is_store: true, is_atomic: false };

    reg.fire_mem_access(0, &load);  // should NOT fire
    reg.fire_mem_access(0, &store);

    assert_eq!(counter.load(Ordering::Relaxed), 1);
}

#[test]
fn registry_mem_filter_all() {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;

    let mut reg = PluginRegistry::new();
    let counter = Arc::new(AtomicU64::new(0));
    let c = counter.clone();
    reg.on_mem_access(MemFilter::All, Box::new(move |_vcpu, _info| {
        c.fetch_add(1, Ordering::Relaxed);
    }));

    let load = MemInfo { vaddr: 0x3000, size: 4, is_store: false, is_atomic: false };
    let store = MemInfo { vaddr: 0x3000, size: 4, is_store: true, is_atomic: false };

    reg.fire_mem_access(0, &load);
    reg.fire_mem_access(0, &store);
    reg.fire_mem_access(0, &load);

    assert_eq!(counter.load(Ordering::Relaxed), 3);
}

#[test]
fn registry_syscall_callback() {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;

    let mut reg = PluginRegistry::new();
    let nr_seen = Arc::new(AtomicU64::new(0));
    let n = nr_seen.clone();
    reg.on_syscall(Box::new(move |info| {
        n.store(info.number, Ordering::Relaxed);
    }));

    reg.fire_syscall(&SyscallInfo { vcpu_idx: 0, number: 64, args: [1, 2, 3, 0, 0, 0] });
    assert_eq!(nr_seen.load(Ordering::Relaxed), 64);
}

#[test]
fn registry_syscall_ret_callback() {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;

    let mut reg = PluginRegistry::new();
    let ret_seen = Arc::new(AtomicU64::new(0));
    let r = ret_seen.clone();
    reg.on_syscall_ret(Box::new(move |info| {
        r.store(info.ret_value, Ordering::Relaxed);
    }));

    reg.fire_syscall_ret(&SyscallRetInfo { vcpu_idx: 0, number: 64, ret_value: 42 });
    assert_eq!(ret_seen.load(Ordering::Relaxed), 42);
}

#[test]
fn registry_branch_callback() {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;

    let mut reg = PluginRegistry::new();
    let taken_count = Arc::new(AtomicU64::new(0));
    let t = taken_count.clone();
    reg.on_branch(Box::new(move |_vcpu, info| {
        if info.taken { t.fetch_add(1, Ordering::Relaxed); }
    }));

    assert!(reg.has_branch_callbacks());

    reg.fire_branch(0, &BranchInfo { pc: 0x1000, target: 0x1008, taken: true, kind: BranchKind::DirectCond });
    reg.fire_branch(0, &BranchInfo { pc: 0x100C, target: 0x1020, taken: false, kind: BranchKind::DirectCond });
    reg.fire_branch(0, &BranchInfo { pc: 0x1010, target: 0x2000, taken: true, kind: BranchKind::Call });

    assert_eq!(taken_count.load(Ordering::Relaxed), 2);
}

#[test]
fn registry_fault_callback() {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;

    let mut reg = PluginRegistry::new();
    let fired = Arc::new(AtomicU64::new(0));
    let f = fired.clone();
    reg.on_fault(Box::new(move |_info| {
        f.fetch_add(1, Ordering::Relaxed);
    }));

    let fault = FaultInfo {
        vcpu_idx: 0,
        pc: 0x1000,
        raw: 0xDEAD,
        kind: FaultKind::IllegalInstruction,
        message: "bad opcode".into(),
        insn_count: 42,
        context: ArchContext::None,
    };
    reg.fire_fault(&fault);
    assert_eq!(fired.load(Ordering::Relaxed), 1);
}

#[test]
fn registry_vcpu_init_callback() {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;

    let mut reg = PluginRegistry::new();
    let seen_vcpu = Arc::new(AtomicU64::new(u64::MAX));
    let s = seen_vcpu.clone();
    reg.on_vcpu_init(Box::new(move |vcpu| {
        s.store(vcpu as u64, Ordering::Relaxed);
    }));

    reg.fire_vcpu_init(3);
    assert_eq!(seen_vcpu.load(Ordering::Relaxed), 3);
}

#[test]
fn registry_multiple_callbacks_same_event() {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;

    let mut reg = PluginRegistry::new();
    let c1 = Arc::new(AtomicU64::new(0));
    let c2 = Arc::new(AtomicU64::new(0));
    let c1c = c1.clone();
    let c2c = c2.clone();

    reg.on_insn_exec(Box::new(move |_, _| { c1c.fetch_add(1, Ordering::Relaxed); }));
    reg.on_insn_exec(Box::new(move |_, _| { c2c.fetch_add(10, Ordering::Relaxed); }));

    let insn = InsnInfo { pc: 0, raw: 0, size: 4, class: InsnClass::Nop };
    reg.fire_insn_exec(0, &insn);

    assert_eq!(c1.load(Ordering::Relaxed), 1);
    assert_eq!(c2.load(Ordering::Relaxed), 10);
}

// ── Scoreboard tests ───────────────────────────────────────────────────────────

#[test]
fn scoreboard_basic() {
    let sb = Scoreboard::<u64>::new(4);
    assert_eq!(sb.len(), 4);
    assert_eq!(*sb.get(0), 0);

    *sb.get_mut(0) = 42;
    *sb.get_mut(1) = 100;
    assert_eq!(*sb.get(0), 42);
    assert_eq!(*sb.get(1), 100);
}

#[test]
fn scoreboard_iter_sum() {
    let sb = Scoreboard::<u64>::new(4);
    *sb.get_mut(0) = 10;
    *sb.get_mut(1) = 20;
    *sb.get_mut(2) = 30;
    *sb.get_mut(3) = 40;
    let total: u64 = sb.iter().sum();
    assert_eq!(total, 100);
}

#[test]
fn scoreboard_is_empty() {
    let sb = Scoreboard::<u64>::new(0);
    assert!(sb.is_empty());
    assert_eq!(sb.len(), 0);

    let sb2 = Scoreboard::<u64>::new(1);
    assert!(!sb2.is_empty());
}

// ── MemFilter tests ────────────────────────────────────────────────────────────

#[test]
fn mem_filter_all() {
    assert!(MemFilter::All.matches(true));
    assert!(MemFilter::All.matches(false));
}

#[test]
fn mem_filter_reads() {
    assert!(MemFilter::ReadsOnly.matches(false));
    assert!(!MemFilter::ReadsOnly.matches(true));
}

#[test]
fn mem_filter_writes() {
    assert!(MemFilter::WritesOnly.matches(true));
    assert!(!MemFilter::WritesOnly.matches(false));
}

// ── InsnClass tests ────────────────────────────────────────────────────────────

#[test]
fn insn_class_debug() {
    assert_eq!(format!("{:?}", InsnClass::Branch), "Branch");
    assert_eq!(format!("{:?}", InsnClass::Load), "Load");
    assert_eq!(format!("{:?}", InsnClass::Store), "Store");
    assert_eq!(format!("{:?}", InsnClass::Nop), "Nop");
    assert_eq!(format!("{:?}", InsnClass::Unknown), "Unknown");
}

// ── FaultInfo tests ────────────────────────────────────────────────────────────

#[test]
fn fault_info_display() {
    let f = FaultInfo {
        vcpu_idx: 0,
        pc: 0x1000,
        raw: 0xDEAD,
        kind: FaultKind::IllegalInstruction,
        message: "bad opcode".into(),
        insn_count: 42,
        context: ArchContext::None,
    };
    assert_eq!(format!("{}", f.kind), "IllegalInstruction");
}

#[test]
fn fault_kind_display_variants() {
    assert_eq!(format!("{}", FaultKind::MemoryFault), "MemoryFault");
    assert_eq!(format!("{}", FaultKind::StackCorruption), "StackCorruption");
    assert_eq!(format!("{}", FaultKind::NullDereference), "NullDereference");
    assert_eq!(format!("{}", FaultKind::WildJump), "WildJump");
    assert_eq!(format!("{}", FaultKind::UnsupportedSyscall), "UnsupportedSyscall");
    assert_eq!(format!("{}", FaultKind::Breakpoint), "Breakpoint");
}

// ── BranchKind tests ──────────────────────────────────────────────────────────

#[test]
fn branch_kind_debug() {
    assert_eq!(format!("{:?}", BranchKind::Call), "Call");
    assert_eq!(format!("{:?}", BranchKind::Return), "Return");
    assert_eq!(format!("{:?}", BranchKind::IndirectJump), "IndirectJump");
    assert_eq!(format!("{:?}", BranchKind::IndirectCall), "IndirectCall");
    assert_eq!(format!("{:?}", BranchKind::DirectUncond), "DirectUncond");
}
