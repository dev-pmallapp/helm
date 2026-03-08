use crate::se::thread::*;
use helm_isa::arm::regs::Aarch64Regs;

fn make_scheduler() -> Scheduler {
    Scheduler::new(Aarch64Regs::default(), 1)
}

#[test]
fn new_scheduler_has_one_thread() {
    let sched = make_scheduler();
    assert_eq!(sched.live_count(), 1);
    assert_eq!(sched.current_tid(), 1);
}

#[test]
fn new_scheduler_is_runnable() {
    let sched = make_scheduler();
    assert!(sched.any_runnable());
}

#[test]
fn new_scheduler_not_deadlocked() {
    let sched = make_scheduler();
    assert!(!sched.is_deadlocked());
}

#[test]
fn spawn_increments_tid() {
    let mut sched = make_scheduler();
    let tid = sched.spawn(CloneRequest {
        flags: 0,
        child_stack: 0x8000,
        parent_tid_ptr: 0,
        child_tid_ptr: 0,
        tls: 0,
    });
    assert_eq!(tid, 2);
    assert_eq!(sched.live_count(), 2);
}

#[test]
fn spawn_child_stack_set() {
    let mut sched = make_scheduler();
    sched.spawn(CloneRequest {
        flags: 0,
        child_stack: 0xBEEF,
        parent_tid_ptr: 0,
        child_tid_ptr: 0,
        tls: 0,
    });
    sched.try_switch();
    assert_eq!(sched.current_regs().sp, 0xBEEF);
}

#[test]
fn exit_current_decrements_live_count() {
    let mut sched = make_scheduler();
    sched.exit_current();
    assert_eq!(sched.live_count(), 0);
}

#[test]
fn block_current_switches_thread() {
    let mut sched = make_scheduler();
    sched.spawn(CloneRequest {
        flags: 0,
        child_stack: 0x8000,
        parent_tid_ptr: 0,
        child_tid_ptr: 0,
        tls: 0,
    });
    let original_tid = sched.current_tid();
    sched.block_current(ThreadState::BlockedRead);
    assert_ne!(sched.current_tid(), original_tid);
}

#[test]
fn futex_wake_unblocks_waiting_thread() {
    let mut sched = make_scheduler();
    sched.spawn(CloneRequest {
        flags: 0,
        child_stack: 0x8000,
        parent_tid_ptr: 0,
        child_tid_ptr: 0,
        tls: 0,
    });
    sched.block_current(ThreadState::FutexWait { uaddr: 0x100, val: 1 });
    let woken = sched.futex_wake(0x100, 1);
    assert_eq!(woken, 1);
}

#[test]
fn futex_wake_wrong_addr_wakes_none() {
    let mut sched = make_scheduler();
    sched.block_current(ThreadState::FutexWait { uaddr: 0x100, val: 1 });
    let woken = sched.futex_wake(0x200, 1);
    assert_eq!(woken, 0);
}

#[test]
fn wake_io_waiters_unblocks_blocked_read() {
    let mut sched = make_scheduler();
    sched.spawn(CloneRequest {
        flags: 0,
        child_stack: 0x8000,
        parent_tid_ptr: 0,
        child_tid_ptr: 0,
        tls: 0,
    });
    sched.block_current(ThreadState::BlockedRead);
    assert!(!sched.any_runnable() || sched.live_count() > 1);
    sched.wake_io_waiters();
    assert!(sched.any_runnable());
}

#[test]
fn try_switch_round_robins() {
    let mut sched = make_scheduler();
    sched.spawn(CloneRequest {
        flags: 0,
        child_stack: 0x8000,
        parent_tid_ptr: 0,
        child_tid_ptr: 0,
        tls: 0,
    });
    sched.spawn(CloneRequest {
        flags: 0,
        child_stack: 0x9000,
        parent_tid_ptr: 0,
        child_tid_ptr: 0,
        tls: 0,
    });
    let t0 = sched.current_tid();
    sched.try_switch();
    let t1 = sched.current_tid();
    sched.try_switch();
    let t2 = sched.current_tid();
    assert_ne!(t0, t1);
    assert_ne!(t1, t2);
}

#[test]
fn save_load_regs_round_trip() {
    let mut sched = make_scheduler();
    let mut regs = Aarch64Regs::default();
    regs.x[0] = 0xCAFE;
    regs.pc = 0x4000;
    sched.save_regs(&regs);
    let mut loaded = Aarch64Regs::default();
    sched.load_regs(&mut loaded);
    assert_eq!(loaded.x[0], 0xCAFE);
    assert_eq!(loaded.pc, 0x4000);
}

#[test]
fn deadlock_detected_when_all_blocked() {
    let mut sched = make_scheduler();
    sched.block_current(ThreadState::FutexWait { uaddr: 0x100, val: 1 });
    assert!(sched.is_deadlocked());
}

#[test]
fn break_deadlock_unblocks_futex_waiters() {
    let mut sched = make_scheduler();
    sched.block_current(ThreadState::FutexWait { uaddr: 0x100, val: 1 });
    assert!(sched.break_deadlock());
    assert!(sched.any_runnable());
}

#[test]
fn exit_with_clear_tid_returns_address() {
    let mut sched = make_scheduler();
    sched.spawn(CloneRequest {
        flags: 0x200000,
        child_stack: 0x8000,
        parent_tid_ptr: 0,
        child_tid_ptr: 0x5000,
        tls: 0,
    });
    sched.try_switch();
    let addr = sched.exit_current();
    assert_eq!(addr, Some(0x5000));
}

#[test]
fn spawn_with_settls_sets_child_tpidr() {
    let mut sched = make_scheduler();
    let mut parent_regs = Aarch64Regs::default();
    parent_regs.tpidr_el0 = 0xAAAA_0000;
    sched.save_regs(&parent_regs);

    const CLONE_SETTLS: u64 = 0x0008_0000;
    sched.spawn(CloneRequest {
        flags: CLONE_SETTLS,
        child_stack: 0x8000,
        parent_tid_ptr: 0,
        child_tid_ptr: 0,
        tls: 0xBBBB_0000,
    });
    sched.try_switch();
    assert_eq!(sched.current_regs().tpidr_el0, 0xBBBB_0000);
}

#[test]
fn spawn_without_settls_inherits_parent_tpidr() {
    let mut sched = make_scheduler();
    let mut parent_regs = Aarch64Regs::default();
    parent_regs.tpidr_el0 = 0xAAAA_0000;
    sched.save_regs(&parent_regs);

    sched.spawn(CloneRequest {
        flags: 0,
        child_stack: 0x8000,
        parent_tid_ptr: 0,
        child_tid_ptr: 0,
        tls: 0xDEAD_BEEF,
    });
    sched.try_switch();
    assert_eq!(
        sched.current_regs().tpidr_el0,
        0xAAAA_0000,
        "child should inherit parent TPIDR_EL0 when CLONE_SETTLS is not set"
    );
}

#[test]
fn set_last_spawned_tpidr_overrides_child() {
    let mut sched = make_scheduler();
    sched.spawn(CloneRequest {
        flags: 0,
        child_stack: 0x8000,
        parent_tid_ptr: 0,
        child_tid_ptr: 0,
        tls: 0,
    });
    sched.set_last_spawned_tpidr(0xCAFE_0000);
    sched.try_switch();
    assert_eq!(sched.current_regs().tpidr_el0, 0xCAFE_0000);
}
