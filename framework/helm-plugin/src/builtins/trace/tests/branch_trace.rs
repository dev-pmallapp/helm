use super::*;
use crate::runtime::{BranchInfo, BranchKind};

#[test]
fn aggregates_taken_and_not_taken_per_pc() {
    let mut plugin = BranchTrace::new();
    let mut reg = HelmPluginRegistry::new();

    plugin.install(&mut reg, &HelmPluginArgs::parse("top=3"));

    reg.fire_branch(
        0,
        &BranchInfo {
            pc: 0x1000,
            target: 0x2000,
            taken: true,
            kind: BranchKind::DirectCond,
        },
    );
    reg.fire_branch(
        0,
        &BranchInfo {
            pc: 0x1000,
            target: 0x1004,
            taken: false,
            kind: BranchKind::DirectCond,
        },
    );
    reg.fire_branch(
        0,
        &BranchInfo {
            pc: 0x1000,
            target: 0x2000,
            taken: true,
            kind: BranchKind::DirectCond,
        },
    );
    reg.fire_branch(
        0,
        &BranchInfo {
            pc: 0x3000,
            target: 0x4000,
            taken: false,
            kind: BranchKind::Call,
        },
    );

    let records = plugin.records.lock().unwrap();
    let entry = records.get(&0x1000).unwrap();
    assert_eq!(plugin.top_n, 3);
    assert_eq!(entry.taken, 2);
    assert_eq!(entry.not_taken, 1);
    assert_eq!(records.get(&0x3000).unwrap().not_taken, 1);
}
