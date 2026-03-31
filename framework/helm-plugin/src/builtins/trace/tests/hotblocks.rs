use super::*;
use crate::runtime::{ArchContext, InsnClass, PluginInsnInfo};

fn sample_insn(pc: u64) -> PluginInsnInfo {
    PluginInsnInfo {
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
    let mut reg = HelmPluginRegistry::new();

    plugin.install(&mut reg, &HelmPluginArgs::parse(""));

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

#[test]
fn honors_pc_range_filter() {
    let mut plugin = HotBlocks::new();
    let mut reg = HelmPluginRegistry::new();

    plugin.install(
        &mut reg,
        &HelmPluginArgs::parse("pc_start=0x1800,pc_end=0x2800"),
    );

    reg.fire_insn_exec(0, &sample_insn(0x1000));
    reg.fire_insn_exec(0, &sample_insn(0x1800));
    reg.fire_insn_exec(0, &sample_insn(0x2000));
    reg.fire_insn_exec(0, &sample_insn(0x2800));
    reg.fire_insn_exec(0, &sample_insn(0x2000));

    let top = plugin.top(4);
    assert_eq!(top, vec![(0x2000, 2), (0x1800, 1)]);
}
