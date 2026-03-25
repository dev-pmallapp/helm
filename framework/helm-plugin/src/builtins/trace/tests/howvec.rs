use super::*;
use crate::runtime::{ArchContext, InsnClass, PluginInsnInfo};

fn sample_insn(class: InsnClass) -> PluginInsnInfo {
    PluginInsnInfo {
        pc: 0x1000,
        raw: 0,
        size: 4,
        class,
        opcode_name: "op",
        is_stub: false,
        context: ArchContext::None,
    }
}

#[test]
fn aggregates_instruction_classes() {
    let mut plugin = HowVec::new();
    let mut reg = HelmPluginRegistry::new();

    plugin.install(&mut reg, &HelmPluginArgs::parse(""));

    reg.fire_insn_exec(0, &sample_insn(InsnClass::Load));
    reg.fire_insn_exec(0, &sample_insn(InsnClass::Load));
    reg.fire_insn_exec(0, &sample_insn(InsnClass::Branch));

    let counts = plugin.counts.lock().unwrap();
    assert_eq!(counts.get(&InsnClass::Load), Some(&2));
    assert_eq!(counts.get(&InsnClass::Branch), Some(&1));
    assert_eq!(counts.get(&InsnClass::Store), None);
}
