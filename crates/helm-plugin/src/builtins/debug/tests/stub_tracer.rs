use super::*;
use crate::runtime::{ArchContext, InsnClass, InsnInfo};

fn insn(pc: u64, raw: u32, opcode_name: &'static str, is_stub: bool) -> InsnInfo {
    InsnInfo {
        pc,
        raw,
        size: 4,
        class: InsnClass::Unknown,
        opcode_name,
        is_stub,
        context: ArchContext::None,
    }
}

#[test]
fn counts_stub_categories_and_caps_unique_encodings() {
    let mut plugin = StubTracer::new();
    let mut reg = PluginRegistry::new();

    plugin.install(&mut reg, &PluginArgs::parse("max=2"));

    reg.fire_insn_exec(0, &insn(0x1000, 0x1, "foo", false));
    reg.fire_insn_exec(0, &insn(0x1004, 0x10, "foo", true));
    reg.fire_insn_exec(0, &insn(0x1008, 0x20, "bar", true));
    reg.fire_insn_exec(0, &insn(0x100c, 0x30, "baz", true));
    reg.fire_insn_exec(0, &insn(0x1010, 0x10, "foo", true));

    let data = plugin.stubs.lock().unwrap();
    assert_eq!(data.total_insns, 5);
    assert_eq!(data.total_stubs, 4);
    assert_eq!(data.by_name.get("foo"), Some(&2));
    assert_eq!(data.by_name.get("bar"), Some(&1));
    assert_eq!(data.by_name.get("baz"), Some(&1));
    assert_eq!(data.by_encoding.len(), 2);
    assert_eq!(data.by_encoding.get(&0x10).unwrap().2, 2);
    assert_eq!(data.by_encoding.get(&0x20).unwrap().2, 1);
    assert!(!data.by_encoding.contains_key(&0x30));
}
