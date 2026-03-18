use super::*;
use crate::runtime::{ArchContext, FaultInfo, FaultKind, InsnClass, InsnInfo, SyscallInfo};

fn insn(pc: u64) -> InsnInfo {
    InsnInfo {
        pc,
        raw: 0,
        size: 4,
        class: InsnClass::IntAlu,
        opcode_name: "add",
        is_stub: false,
        context: ArchContext::None,
    }
}

#[test]
fn ring_buffer_wraps_and_keeps_recent_pcs() {
    let mut plugin = FaultDetect::new();
    let mut reg = PluginRegistry::new();

    plugin.install(&mut reg, &PluginArgs::parse("history=3"));

    reg.fire_insn_exec(0, &insn(0x10));
    reg.fire_insn_exec(0, &insn(0x20));
    reg.fire_insn_exec(0, &insn(0x30));
    reg.fire_insn_exec(0, &insn(0x40));

    let guard = plugin.inner.lock().unwrap();
    assert_eq!(guard.recent_pcs(), vec![0x20, 0x30, 0x40]);
}

#[test]
fn syscall_log_is_recorded_before_faults() {
    let mut plugin = FaultDetect::new();
    let mut reg = PluginRegistry::new();

    plugin.install(&mut reg, &PluginArgs::parse("history=2"));

    reg.fire_syscall(&SyscallInfo {
        vcpu_idx: 1,
        number: 93,
        args: [0, 1, 2, 3, 4, 5],
    });
    reg.fire_fault(&FaultInfo {
        vcpu_idx: 1,
        pc: 0xdead,
        raw: 0xbeef,
        kind: FaultKind::IllegalInstruction,
        message: "boom".to_string(),
        insn_count: 7,
        context: ArchContext::None,
    });

    let guard = plugin.inner.lock().unwrap();
    assert_eq!(guard.syscall_log.len(), 1);
    assert!(guard.syscall_log[0].contains("vcpu=1 syscall=93"));
}
