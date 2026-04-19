use super::*;
use crate::runtime::{ArchContext, FaultInfo, FaultKind, InsnClass, PluginInsnInfo};

fn aarch64_context() -> ArchContext {
    let mut x = [0u64; 31];
    x[0] = 0x10;
    x[1] = 0x11;
    x[22] = 0x22;
    x[29] = 0x29;
    x[30] = 0x30;
    ArchContext::Aarch64 {
        x,
        sp: 0x8000,
        pc: 0x4000,
        nzcv: 0xa000_0000,
        current_el: 1,
        tpidrro_el0: 0xb300_0400,
    }
}

fn riscv_context() -> ArchContext {
    let mut x = [0u64; 32];
    x[1] = 0x11;
    x[2] = 0x22;
    x[8] = 0x88;
    x[10] = 0xaa;
    ArchContext::RiscV { x, pc: 0x1234 }
}

fn insn_with_context(pc: u64, context: ArchContext) -> PluginInsnInfo {
    PluginInsnInfo {
        pc,
        raw: 0xd503_201f,
        size: 4,
        class: InsnClass::Nop,
        opcode_name: "Nop",
        is_stub: false,
        context,
    }
}

#[test]
fn parses_named_register_lists_without_arch_specific_decoding() {
    let selection = parse_selection("pc+sp+lr+fp+x22+current_el+nzcv+tpidrro");
    assert_eq!(
        selection,
        RegisterSelection::Named(vec![
            "pc".to_string(),
            "sp".to_string(),
            "lr".to_string(),
            "fp".to_string(),
            "x22".to_string(),
            "current_el".to_string(),
            "nzcv".to_string(),
            "tpidrro".to_string(),
        ])
    );
}

#[test]
fn all_selector_is_deferred_until_context_is_known() {
    assert_eq!(parse_selection("all"), RegisterSelection::All);
    assert_eq!(parse_selection(""), RegisterSelection::Default);
}

#[test]
fn formats_requested_aarch64_registers_via_arch_context_api() {
    let selection = parse_selection("pc+sp+lr+fp+x22+current_el+nzcv+tpidrro_el0");
    let formatted = format_registers(&selection, &aarch64_context());
    assert_eq!(
        formatted,
        vec![
            "pc=0x0000000000004000".to_string(),
            "sp=0x0000000000008000".to_string(),
            "lr=0x0000000000000030".to_string(),
            "fp=0x0000000000000029".to_string(),
            "x22=0x0000000000000022".to_string(),
            "current_el=1".to_string(),
            "nzcv=0xa0000000".to_string(),
            "tpidrro_el0=0x00000000b3000400".to_string(),
        ]
    );
}

#[test]
fn formats_requested_riscv_registers_via_arch_context_api() {
    let selection = parse_selection("pc+sp+ra+fp+a0+current_el");
    let formatted = format_registers(&selection, &riscv_context());
    assert_eq!(
        formatted,
        vec![
            "pc=0x0000000000001234".to_string(),
            "sp=0x0000000000000022".to_string(),
            "ra=0x0000000000000011".to_string(),
            "fp=0x0000000000000088".to_string(),
            "a0=0x00000000000000aa".to_string(),
            "current_el=<unsupported:riscv64>".to_string(),
        ]
    );
}

#[test]
fn atexit_dump_uses_last_instruction_context() {
    let mut plugin = RegisterDump::new();
    let mut reg = HelmPluginRegistry::new();

    plugin.install(
        &mut reg,
        &HelmPluginArgs::parse("regs=pc+sp+x22,dump=atexit,vcpu=2"),
    );
    reg.fire_insn_exec(2, &insn_with_context(0x4000, aarch64_context()));

    let guard = plugin.inner.lock().unwrap();
    let context = guard.last_contexts.get(&2).expect("context recorded");
    let formatted = format_registers(&plugin.config.selection, context);
    assert_eq!(
        formatted,
        vec![
            "pc=0x0000000000004000".to_string(),
            "sp=0x0000000000008000".to_string(),
            "x22=0x0000000000000022".to_string(),
        ]
    );
}

#[test]
fn fault_callback_records_last_fault() {
    let mut plugin = RegisterDump::new();
    let mut reg = HelmPluginRegistry::new();
    let context = aarch64_context();

    plugin.install(&mut reg, &HelmPluginArgs::parse("regs=pc+lr,dump=fault"));
    reg.fire_fault(&FaultInfo {
        vcpu_idx: 0,
        pc: 0x4000,
        raw: 0,
        kind: FaultKind::MemoryFault,
        message: "boom".to_string(),
        insn_count: 12,
        context,
    });

    let guard = plugin.inner.lock().unwrap();
    let fault = guard.last_fault.as_ref().expect("fault recorded");
    assert_eq!(fault.vcpu_idx, 0);
    assert_eq!(fault.insn_count, 12);
}
