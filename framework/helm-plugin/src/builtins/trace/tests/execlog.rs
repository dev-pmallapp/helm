use super::*;
use crate::runtime::{ArchContext, InsnClass, PluginInsnInfo};

fn sample_insn(context: ArchContext) -> PluginInsnInfo {
    PluginInsnInfo {
        pc: 0x1234,
        raw: 0xd503201f,
        size: 4,
        class: InsnClass::Nop,
        opcode_name: "nop",
        is_stub: false,
        context,
    }
}

#[test]
fn respects_max_and_aarch64_register_formatting() {
    let mut plugin = ExecLog::new();
    let mut reg = HelmPluginRegistry::new();

    plugin.install(&mut reg, &HelmPluginArgs::parse("max=2,regs=true"));

    let mut x = [0u64; 31];
    x[0] = 1;
    x[5] = 0x55;
    let insn = sample_insn(ArchContext::Aarch64 {
        x,
        sp: 0x8000,
        pc: 0x1234,
        nzcv: 0x6000_0000,
        current_el: 2,
        tpidrro_el0: 0x1234_5000,
    });
    reg.fire_insn_exec(0, &insn);
    reg.fire_insn_exec(1, &insn);
    reg.fire_insn_exec(2, &insn);

    let lines = plugin.lines();
    assert_eq!(lines.len(), 2);
    assert!(lines[0].contains("vcpu=0"));
    assert!(lines[0].contains("sp=0x0000000000008000"));
    assert!(lines[0].contains("nzcv=0x60000000"));
    assert!(lines[0].contains("el=2"));
    assert!(lines[0].contains("tpidrro_el0=0x0000000012345000"));
    assert!(lines[0].contains("x0=0x1"));
    assert!(lines[0].contains("x5=0x55"));
    assert!(!lines[0].contains("x1="));
    assert!(lines[1].contains("vcpu=1"));
}

#[test]
fn omits_registers_when_disabled() {
    let mut plugin = ExecLog::new();
    let mut reg = HelmPluginRegistry::new();

    plugin.install(&mut reg, &HelmPluginArgs::parse("regs=false"));

    let mut x = [0u64; 32];
    x[1] = 0x22;
    reg.fire_insn_exec(0, &sample_insn(ArchContext::RiscV { x, pc: 0x1234 }));

    let lines = plugin.lines();
    assert_eq!(lines.len(), 1);
    assert!(lines[0].contains("pc=0x0000000000001234"));
    assert!(!lines[0].contains("x1=0x22"));
}

#[test]
fn filters_by_pc_when_requested() {
    let mut plugin = ExecLog::new();
    let mut reg = HelmPluginRegistry::new();

    plugin.install(&mut reg, &HelmPluginArgs::parse("pc=0x1234,max=4"));

    let hit = sample_insn(ArchContext::None);
    let miss = PluginInsnInfo {
        pc: 0x9999,
        ..sample_insn(ArchContext::None)
    };
    reg.fire_insn_exec(0, &miss);
    reg.fire_insn_exec(0, &hit);

    let lines = plugin.lines();
    assert_eq!(lines.len(), 1);
    assert!(lines[0].contains("pc=0x0000000000001234"));
}

#[test]
fn filters_by_pc_range_when_requested() {
    let mut plugin = ExecLog::new();
    let mut reg = HelmPluginRegistry::new();

    plugin.install(
        &mut reg,
        &HelmPluginArgs::parse("pc_start=0x1200,pc_end=0x1300,max=8"),
    );

    reg.fire_insn_exec(
        0,
        &PluginInsnInfo {
            pc: 0x11fc,
            ..sample_insn(ArchContext::None)
        },
    );
    reg.fire_insn_exec(
        0,
        &PluginInsnInfo {
            pc: 0x1200,
            ..sample_insn(ArchContext::None)
        },
    );
    reg.fire_insn_exec(
        0,
        &PluginInsnInfo {
            pc: 0x12fc,
            ..sample_insn(ArchContext::None)
        },
    );
    reg.fire_insn_exec(
        0,
        &PluginInsnInfo {
            pc: 0x1300,
            ..sample_insn(ArchContext::None)
        },
    );

    let lines = plugin.lines();
    assert_eq!(lines.len(), 2);
    assert!(lines[0].contains("pc=0x0000000000001200"));
    assert!(lines[1].contains("pc=0x00000000000012fc"));
}

#[test]
fn tail_mode_keeps_last_matches() {
    let mut plugin = ExecLog::new();
    let mut reg = HelmPluginRegistry::new();

    plugin.install(&mut reg, &HelmPluginArgs::parse("max=2,tail=true"));

    reg.fire_insn_exec(
        0,
        &PluginInsnInfo {
            pc: 0x1000,
            ..sample_insn(ArchContext::None)
        },
    );
    reg.fire_insn_exec(
        0,
        &PluginInsnInfo {
            pc: 0x1004,
            ..sample_insn(ArchContext::None)
        },
    );
    reg.fire_insn_exec(
        0,
        &PluginInsnInfo {
            pc: 0x1008,
            ..sample_insn(ArchContext::None)
        },
    );

    let lines = plugin.lines();
    assert_eq!(lines.len(), 2);
    assert!(lines[0].contains("pc=0x0000000000001004"));
    assert!(lines[1].contains("pc=0x0000000000001008"));
}
