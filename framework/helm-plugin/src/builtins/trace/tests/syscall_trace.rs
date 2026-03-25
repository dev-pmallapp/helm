use super::*;
use crate::runtime::{SyscallInfo, SyscallRetInfo};

#[test]
fn records_entry_and_return_lines_in_order() {
    let mut plugin = SyscallTrace::new();
    let mut reg = HelmPluginRegistry::new();

    plugin.install(&mut reg, &HelmPluginArgs::parse(""));

    reg.fire_syscall(&SyscallInfo {
        vcpu_idx: 0,
        number: 64,
        args: [1, 2, 3, 4, 5, 6],
    });
    reg.fire_syscall_ret(&SyscallRetInfo {
        vcpu_idx: 0,
        number: 64,
        ret_value: u64::MAX,
    });

    assert_eq!(
        plugin.entries(),
        vec![
            "[strace] syscall=64 args=[0x1, 0x2, 0x3, 0x4, 0x5, 0x6]".to_string(),
            "[strace]  → ret=0xffffffffffffffff (-1)".to_string(),
        ]
    );
}
