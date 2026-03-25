use super::*;
use crate::runtime::{ArchContext, InsnClass, PluginInsnInfo};

fn sample_insn(pc: u64) -> PluginInsnInfo {
    PluginInsnInfo {
        pc,
        raw: 0,
        size: 4,
        class: InsnClass::Nop,
        opcode_name: "nop",
        is_stub: false,
        context: ArchContext::None,
    }
}

#[test]
fn install_sizes_scoreboard_and_counts_per_vcpu() {
    let mut plugin = InsnCount::new();
    let mut reg = HelmPluginRegistry::new();

    plugin.install(&mut reg, &HelmPluginArgs::parse("vcpus=2"));

    let insn = sample_insn(0x1000);
    reg.fire_insn_exec(0, &insn);
    reg.fire_insn_exec(1, &insn);
    reg.fire_insn_exec(1, &insn);
    reg.fire_insn_exec(3, &insn);

    assert_eq!(plugin.num_vcpus, 2);
    assert_eq!(plugin.per_vcpu(), vec![1, 2]);
    assert_eq!(plugin.total(), 3);
}

#[test]
fn install_defaults_to_single_vcpu() {
    let mut plugin = InsnCount::new();
    let mut reg = HelmPluginRegistry::new();

    plugin.install(&mut reg, &HelmPluginArgs::parse(""));
    reg.fire_insn_exec(0, &sample_insn(0x2000));

    assert_eq!(plugin.per_vcpu(), vec![1]);
    assert_eq!(plugin.total(), 1);
}
