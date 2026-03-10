use crate::backends::{IntervalBackend, NullBackend};
use helm_core::insn::*;
use helm_core::timing::{AccuracyLevel, TimingBackend};

fn make_insn(class: InsnClass) -> DecodedInsn {
    DecodedInsn {
        class,
        len: 4,
        ..DecodedInsn::default()
    }
}

fn make_outcome() -> ExecOutcome {
    ExecOutcome::default()
}

// --- NullBackend tests ---

#[test]
fn null_backend_accuracy_is_fe() {
    let timing = NullBackend;
    assert_eq!(timing.accuracy(), AccuracyLevel::FE);
}

#[test]
fn null_backend_always_returns_zero() {
    let mut timing = NullBackend;
    for class in [
        InsnClass::IntAlu,
        InsnClass::IntMul,
        InsnClass::Load,
        InsnClass::Store,
        InsnClass::Branch,
        InsnClass::Syscall,
        InsnClass::FpDiv,
    ] {
        let insn = make_insn(class);
        assert_eq!(timing.account(&insn, &make_outcome()), 0, "class={class:?}");
    }
}

// --- IntervalBackend tests ---

#[test]
fn interval_accuracy_is_ite() {
    let timing = IntervalBackend::default();
    assert_eq!(timing.accuracy(), AccuracyLevel::ITE);
}

#[test]
fn interval_int_alu_returns_configured_latency() {
    let mut timing = IntervalBackend::default();
    let insn = make_insn(InsnClass::IntAlu);
    let stall = timing.account(&insn, &make_outcome());
    assert_eq!(stall, timing.int_alu_latency);
}

#[test]
fn interval_int_mul_returns_configured_latency() {
    let mut timing = IntervalBackend::default();
    let insn = make_insn(InsnClass::IntMul);
    let stall = timing.account(&insn, &make_outcome());
    assert_eq!(stall, timing.int_mul_latency);
}

#[test]
fn interval_int_div_returns_configured_latency() {
    let mut timing = IntervalBackend::default();
    let insn = make_insn(InsnClass::IntDiv);
    let stall = timing.account(&insn, &make_outcome());
    assert_eq!(stall, timing.int_div_latency);
}

#[test]
fn interval_load_includes_mem_latency() {
    let mut timing = IntervalBackend::default();
    let insn = make_insn(InsnClass::Load);
    let outcome = ExecOutcome {
        mem_access_count: 1,
        mem_accesses: [
            MemAccessInfo { addr: 0x1000, size: 8, is_write: false },
            MemAccessInfo::default(),
        ],
        ..ExecOutcome::default()
    };
    let stall = timing.account(&insn, &outcome);
    assert!(stall > timing.load_latency, "should include mem latency");
}

#[test]
fn interval_branch_taken_adds_penalty() {
    let mut timing = IntervalBackend::default();
    let insn = make_insn(InsnClass::CondBranch);
    let outcome_taken = ExecOutcome {
        branch_taken: true,
        ..ExecOutcome::default()
    };
    let outcome_not_taken = ExecOutcome {
        branch_taken: false,
        ..ExecOutcome::default()
    };
    let stall_taken = timing.account(&insn, &outcome_taken);
    let stall_not_taken = timing.account(&insn, &outcome_not_taken);
    assert_eq!(stall_taken - stall_not_taken, timing.branch_penalty);
}

#[test]
fn interval_custom_latencies() {
    let mut timing = IntervalBackend {
        int_alu_latency: 42,
        ..IntervalBackend::default()
    };
    let insn = make_insn(InsnClass::IntAlu);
    assert_eq!(timing.account(&insn, &make_outcome()), 42);
}
