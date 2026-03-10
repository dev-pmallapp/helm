use crate::scheduler::*;
use helm_core::ir::{MicroOp, MicroOpFlags, Opcode};

fn make_uop() -> MicroOp {
    MicroOp {
        guest_pc: 0,
        opcode: Opcode::IntAlu,
        sources: vec![],
        dest: None,
        immediate: None,
        flags: MicroOpFlags::default(),
    }
}

#[test]
fn empty_scheduler_selects_nothing() {
    let mut sched = Scheduler::new(4);
    let issued = sched.select(4);
    assert!(issued.is_empty());
}

#[test]
fn insert_respects_capacity() {
    let mut sched = Scheduler::new(2);
    assert!(sched.insert(make_uop(), 0));
    assert!(sched.insert(make_uop(), 1));
    assert!(!sched.insert(make_uop(), 2), "should reject when full");
}

#[test]
fn wakeup_and_select_issues_ready() {
    let mut sched = Scheduler::new(4);
    sched.insert(make_uop(), 0);
    sched.insert(make_uop(), 1);
    sched.wakeup(&[]);
    let issued = sched.select(1);
    assert_eq!(issued.len(), 1, "should respect width limit");
}

#[test]
fn select_all_ready_within_width() {
    let mut sched = Scheduler::new(8);
    sched.insert(make_uop(), 0);
    sched.insert(make_uop(), 1);
    sched.insert(make_uop(), 2);
    sched.wakeup(&[]);
    let issued = sched.select(4); // width=4, 3 ready
    assert_eq!(issued.len(), 3);
}

#[test]
fn selected_entries_removed_from_scheduler() {
    let mut sched = Scheduler::new(4);
    sched.insert(make_uop(), 0);
    sched.wakeup(&[]);
    sched.select(4); // consume the entry
                     // Scheduler should be empty now
    let issued = sched.select(4);
    assert!(issued.is_empty());
}

#[test]
fn not_woken_entries_not_selected() {
    let mut sched = Scheduler::new(4);
    sched.insert(make_uop(), 0); // not woken
    let issued = sched.select(4);
    assert!(issued.is_empty());
}
