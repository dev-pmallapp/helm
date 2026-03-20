use crate::api::HelmPlugin;
use super::super::trace_window_fault::TraceWindowFault;
use crate::api::PluginArgs;
use crate::runtime::{
    ArchContext, BranchInfo, BranchKind, FaultInfo, FaultKind, InsnClass, InsnInfo, MemInfo,
    PluginRegistry, SyscallInfo,
};

fn insn(pc: u64, raw: u32, opcode_name: &'static str) -> InsnInfo {
    InsnInfo {
        pc,
        raw,
        size: 4,
        class: InsnClass::IntAlu,
        opcode_name,
        is_stub: false,
        context: ArchContext::None,
    }
}

#[test]
fn trace_window_collects_recent_events() {
    let mut plugin = TraceWindowFault::new();
    let mut reg = PluginRegistry::new();
    plugin.install(
        &mut reg,
        &PluginArgs::parse("insns=2,mem=2,branches=2,syscalls=2"),
    );

    reg.fire_insn_exec(0, &insn(0x10, 0x1111, "mov"));
    reg.fire_insn_exec(0, &insn(0x20, 0x2222, "add"));
    reg.fire_insn_exec(0, &insn(0x30, 0x3333, "sub"));

    reg.fire_mem_access(
        0,
        &MemInfo {
            vaddr: 0x1000,
            size: 8,
            is_store: false,
            is_atomic: false,
        },
    );
    reg.fire_mem_access(
        0,
        &MemInfo {
            vaddr: 0x2000,
            size: 4,
            is_store: true,
            is_atomic: true,
        },
    );

    reg.fire_branch(
        0,
        &BranchInfo {
            pc: 0x30,
            target: 0x40,
            taken: true,
            kind: BranchKind::DirectCond,
        },
    );

    reg.fire_syscall(&SyscallInfo {
        vcpu_idx: 0,
        number: 93,
        args: [1, 2, 3, 4, 5, 6],
    });

    reg.fire_fault(&FaultInfo {
        vcpu_idx: 0,
        pc: 0x40,
        raw: 0xd4210000,
        kind: FaultKind::Breakpoint,
        message: "boom".to_string(),
        insn_count: 7,
        context: ArchContext::None,
    });

    let guard = plugin.inner.lock().unwrap();
    let insns = guard.insns.entries();
    assert_eq!(insns.len(), 2);
    assert_eq!(insns[0].pc, 0x20);
    assert_eq!(insns[1].pc, 0x30);
    assert_eq!(insns[1].opcode_name, "sub");

    let mem = guard.mem.entries();
    assert_eq!(mem.len(), 2);
    assert_eq!(mem[0].vaddr, 0x1000);
    assert_eq!(mem[1].vaddr, 0x2000);
    assert!(mem[1].is_store);
    assert!(mem[1].is_atomic);

    let branches = guard.branches.entries();
    assert_eq!(branches.len(), 1);
    assert_eq!(branches[0].pc, 0x30);
    assert_eq!(branches[0].target, 0x40);

    let syscalls = guard.syscalls.entries();
    assert_eq!(syscalls.len(), 1);
    assert_eq!(syscalls[0].number, 93);
}
