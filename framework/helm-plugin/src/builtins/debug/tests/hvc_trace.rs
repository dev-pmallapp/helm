use super::*;
use crate::runtime::{ArchContext, ExceptionCause, ExceptionInfo, HelmPluginRegistry};

const ESR_HVC: u32 = (0x16 << 26) | (1 << 25);
const ESR_SMC: u32 = (0x17 << 26) | (1 << 25);
const ESR_DATA_ABORT_EL1: u32 = 0x25 << 26;

fn aarch64_ctx_with(args: [u64; 8]) -> ArchContext {
    let mut x = [0u64; 31];
    for (i, v) in args.iter().enumerate() {
        x[i] = *v;
    }
    ArchContext::Aarch64 {
        x,
        sp: 0,
        pc: 0,
        nzcv: 0,
        current_el: 1,
        tpidrro_el0: 0,
    }
}

fn fire_hvc(reg: &HelmPluginRegistry, imm16: u16, args: [u64; 8], insn_count: u64) {
    reg.fire_exception(&ExceptionInfo {
        vcpu_idx: 0,
        cause: ExceptionCause::Sync,
        from_el: 1,
        target_el: 2,
        vector_pc: 0x4000_0400,
        elr: 0xffff_8000_0000_1234,
        spsr: 0,
        esr: ESR_HVC | (imm16 as u32),
        far: 0,
        insn_count,
        context: aarch64_ctx_with(args),
    });
}

#[test]
fn captures_hvc_with_argument_registers() {
    let mut plugin = HvcTrace::new();
    let mut reg = HelmPluginRegistry::new();
    plugin.install(&mut reg, &HelmPluginArgs::parse(""));

    fire_hvc(&reg, 0x42, [1, 2, 3, 4, 5, 6, 7, 8], 100);

    let inner = plugin.inner.lock().unwrap();
    assert_eq!(inner.entries.len(), 1);
    let entry = &inner.entries[0];
    assert_eq!(entry.cause_label, "HVC");
    assert_eq!(entry.imm16, 0x42);
    assert_eq!(entry.args, [1, 2, 3, 4, 5, 6, 7, 8]);
    assert_eq!(entry.insn_count, 100);
    assert_eq!(entry.target_el, 2);
}

#[test]
fn ignores_smc_in_default_hvc_only_mode() {
    let mut plugin = HvcTrace::new();
    let mut reg = HelmPluginRegistry::new();
    plugin.install(&mut reg, &HelmPluginArgs::parse(""));

    reg.fire_exception(&ExceptionInfo {
        vcpu_idx: 0,
        cause: ExceptionCause::Sync,
        from_el: 1,
        target_el: 3,
        vector_pc: 0x8000_0400,
        elr: 0,
        spsr: 0,
        esr: ESR_SMC,
        far: 0,
        insn_count: 0,
        context: ArchContext::None,
    });

    let inner = plugin.inner.lock().unwrap();
    assert_eq!(inner.entries.len(), 0);
    assert_eq!(inner.matched, 0);
}

#[test]
fn captures_both_when_kind_both() {
    let mut plugin = HvcTrace::new();
    let mut reg = HelmPluginRegistry::new();
    plugin.install(&mut reg, &HelmPluginArgs::parse("kind=both"));

    fire_hvc(&reg, 0x1, [0; 8], 1);
    reg.fire_exception(&ExceptionInfo {
        vcpu_idx: 0,
        cause: ExceptionCause::Sync,
        from_el: 1,
        target_el: 3,
        vector_pc: 0,
        elr: 0,
        spsr: 0,
        esr: ESR_SMC,
        far: 0,
        insn_count: 2,
        context: ArchContext::None,
    });

    let inner = plugin.inner.lock().unwrap();
    assert_eq!(inner.entries.len(), 2);
    assert_eq!(inner.entries[0].cause_label, "HVC");
    assert_eq!(inner.entries[1].cause_label, "SMC");
}

#[test]
fn ring_buffer_drops_oldest_when_full() {
    let mut plugin = HvcTrace::new();
    let mut reg = HelmPluginRegistry::new();
    plugin.install(&mut reg, &HelmPluginArgs::parse("max=2"));

    fire_hvc(&reg, 1, [0; 8], 10);
    fire_hvc(&reg, 2, [0; 8], 20);
    fire_hvc(&reg, 3, [0; 8], 30);

    let inner = plugin.inner.lock().unwrap();
    assert_eq!(inner.entries.len(), 2);
    assert_eq!(inner.entries[0].imm16, 2);
    assert_eq!(inner.entries[1].imm16, 3);
    assert_eq!(inner.dropped, 1);
    assert_eq!(inner.matched, 3);
}

#[test]
fn ignores_non_sync_and_unrelated_ec() {
    let mut plugin = HvcTrace::new();
    let mut reg = HelmPluginRegistry::new();
    plugin.install(&mut reg, &HelmPluginArgs::parse(""));

    // IRQ entry: should be ignored.
    reg.fire_exception(&ExceptionInfo {
        vcpu_idx: 0,
        cause: ExceptionCause::Irq,
        from_el: 1,
        target_el: 2,
        vector_pc: 0,
        elr: 0,
        spsr: 0,
        esr: 0,
        far: 0,
        insn_count: 0,
        context: ArchContext::None,
    });
    // Sync data-abort: should be ignored.
    reg.fire_exception(&ExceptionInfo {
        vcpu_idx: 0,
        cause: ExceptionCause::Sync,
        from_el: 1,
        target_el: 2,
        vector_pc: 0,
        elr: 0,
        spsr: 0,
        esr: ESR_DATA_ABORT_EL1,
        far: 0xdead,
        insn_count: 0,
        context: ArchContext::None,
    });

    let inner = plugin.inner.lock().unwrap();
    assert_eq!(inner.entries.len(), 0);
    assert_eq!(inner.matched, 0);
}
