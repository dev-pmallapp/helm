use super::super::pc_trace::PcTrace;
use crate::api::{HelmPlugin, HelmPluginArgs};
use crate::runtime::{
    ArchContext, FaultInfo, FaultKind, HelmPluginRegistry, InsnClass, MemInfo, PluginInsnInfo,
};

fn aarch64_insn(pc: u64, raw: u32, x1: u64, x4: u64) -> PluginInsnInfo {
    let mut regs = [0u64; 31];
    regs[1] = x1;
    regs[4] = x4;
    PluginInsnInfo {
        pc,
        raw,
        size: 4,
        class: InsnClass::Load,
        opcode_name: "ldr",
        is_stub: false,
        context: ArchContext::Aarch64 {
            x: regs,
            sp: 0x8000,
            pc,
            nzcv: 0,
            current_el: 0,
            tpidrro_el0: 0,
        },
    }
}

#[test]
fn pc_trace_filters_hits_and_tracks_mem_for_matching_pc() {
    let mut plugin = PcTrace::new();
    let mut reg = HelmPluginRegistry::new();
    plugin.install(
        &mut reg,
        &HelmPluginArgs::parse("pc=0x1000,max=4,mem=reads,mem-max=2,regs=delta,dump=fault"),
    );

    reg.fire_mem_access(
        0,
        &MemInfo {
            pc: 0x1000,
            raw: 0xF841_8C24,
            opcode_name: "ldr",
            class: InsnClass::Load,
            vaddr: 0x2018,
            paddr: 0x2018,
            size: 8,
            is_store: false,
            is_atomic: false,
            value_before: Some(0x1122_3344_5566_7788),
            value_after: None,
        },
    );
    reg.fire_insn_exec(0, &aarch64_insn(0x1000, 0xF841_8C24, 0x2000, 0));
    reg.fire_insn_exec(0, &aarch64_insn(0x2000, 0xD503_201F, 0x3000, 0));
    reg.fire_mem_access(
        0,
        &MemInfo {
            pc: 0x2000,
            raw: 0xD503_201F,
            opcode_name: "nop",
            class: InsnClass::Nop,
            vaddr: 0x9999,
            paddr: 0x9999,
            size: 4,
            is_store: false,
            is_atomic: false,
            value_before: Some(0),
            value_after: None,
        },
    );
    reg.fire_mem_access(
        0,
        &MemInfo {
            pc: 0x1000,
            raw: 0xF841_8C24,
            opcode_name: "ldr",
            class: InsnClass::Load,
            vaddr: 0x2030,
            paddr: 0x2030,
            size: 8,
            is_store: false,
            is_atomic: false,
            value_before: Some(0x8877_6655_4433_2211),
            value_after: None,
        },
    );
    reg.fire_insn_exec(
        0,
        &aarch64_insn(0x1000, 0xF841_8C24, 0x2018, 0x1122_3344_5566_7788),
    );

    reg.fire_fault(&FaultInfo {
        vcpu_idx: 0,
        pc: 0x1000,
        raw: 0xF841_8C24,
        kind: FaultKind::Breakpoint,
        message: "stop".to_string(),
        insn_count: 3,
        context: ArchContext::None,
    });

    let guard = plugin.inner.lock().unwrap();
    assert_eq!(guard.hits.len(), 2);
    assert_eq!(guard.hits[0].pc, 0x1000);
    assert_eq!(guard.hits[0].mem.len(), 1);
    assert_eq!(guard.hits[0].mem[0].vaddr, 0x2018);
    assert_eq!(guard.hits[1].mem.len(), 1);
    assert_eq!(guard.hits[1].mem[0].vaddr, 0x2030);
    assert!(guard.hits[1].context_summary.contains("x1=0x2018"));
    assert!(guard.dumped);
}

#[test]
fn pc_trace_ignores_writes_when_configured_for_reads_only() {
    let mut plugin = PcTrace::new();
    let mut reg = HelmPluginRegistry::new();
    plugin.install(
        &mut reg,
        &HelmPluginArgs::parse("pc=0x1000,max=2,mem=reads,mem-max=2,regs=none,dump=atexit"),
    );

    reg.fire_insn_exec(0, &aarch64_insn(0x1000, 0xF900_0020, 0x2000, 0x55));
    reg.fire_mem_access(
        0,
        &MemInfo {
            pc: 0x1000,
            raw: 0xF900_0020,
            opcode_name: "str",
            class: InsnClass::Store,
            vaddr: 0x2000,
            paddr: 0x2000,
            size: 8,
            is_store: true,
            is_atomic: false,
            value_before: Some(0),
            value_after: Some(0x55),
        },
    );

    let guard = plugin.inner.lock().unwrap();
    assert_eq!(guard.hits.len(), 1);
    assert!(guard.hits[0].mem.is_empty());
}
