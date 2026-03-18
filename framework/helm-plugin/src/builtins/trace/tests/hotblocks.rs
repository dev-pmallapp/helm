use super::*;
use crate::runtime::{ArchContext, InsnClass, InsnInfo};

fn sample_insn(pc: u64) -> InsnInfo {
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
fn top_returns_counts_sorted_descending() {
    let mut plugin = HotBlocks::new();
    let mut reg = PluginRegistry::new();

    plugin.install(&mut reg, &PluginArgs::parse(""));

    reg.fire_insn_exec(0, &sample_insn(0x1000));
    reg.fire_insn_exec(0, &sample_insn(0x2000));
    reg.fire_insn_exec(0, &sample_insn(0x1000));
    reg.fire_insn_exec(0, &sample_insn(0x3000));
    reg.fire_insn_exec(0, &sample_insn(0x1000));

    let top = plugin.top(2);
    assert_eq!(top[0], (0x1000, 3));
    assert_eq!(top[1].1, 1);
    assert!(matches!(top[1].0, 0x2000 | 0x3000));
}
