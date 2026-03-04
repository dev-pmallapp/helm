use crate::info::*;
use crate::plugin::*;
use crate::registry::PluginRegistry;
use crate::trace::*;

fn dummy_insn(vaddr: u64, mnemonic: &str) -> InsnInfo {
    InsnInfo {
        vaddr,
        bytes: vec![0; 4],
        size: 4,
        mnemonic: mnemonic.to_string(),
        symbol: None,
    }
}

fn dummy_tb(pc: u64, insn_count: usize) -> TbInfo {
    TbInfo {
        pc,
        insn_count,
        size: insn_count * 4,
    }
}

#[test]
fn insn_count_counts() {
    let mut plugin = InsnCount::new();
    let mut reg = PluginRegistry::new();
    plugin.install(&mut reg, &PluginArgs::new());

    let insn = dummy_insn(0x1000, "ADD_imm");
    reg.fire_insn_exec(0, &insn);
    reg.fire_insn_exec(0, &insn);
    reg.fire_insn_exec(1, &insn);

    assert_eq!(plugin.total(), 3);
    let per_vcpu = plugin.per_vcpu();
    assert_eq!(per_vcpu[0], 2);
    assert_eq!(per_vcpu[1], 1);
}

#[test]
fn syscall_trace_installs_without_panic() {
    let mut plugin = SyscallTrace::new();
    let mut reg = PluginRegistry::new();
    plugin.install(&mut reg, &PluginArgs::new());

    reg.fire_syscall(&SyscallInfo {
        number: 64,
        args: [1, 0x1000, 6, 0, 0, 0],
        vcpu_idx: 0,
    });
}

#[test]
fn plugin_args_parsing() {
    let args = PluginArgs::parse("regs=true,max=1000,output=trace.log");
    assert_eq!(args.get("regs"), Some("true"));
    assert_eq!(args.get_usize("max", 0), 1000);
    assert_eq!(args.get("output"), Some("trace.log"));
    assert_eq!(args.get("missing"), None);
    assert_eq!(args.get_or("missing", "default"), "default");
}

#[test]
fn registry_fires_all_callback_types() {
    let mut reg = PluginRegistry::new();

    let called = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
    let c1 = called.clone();
    let c2 = called.clone();
    let c3 = called.clone();

    reg.on_vcpu_init(Box::new(move |_| {
        c1.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }));
    reg.on_tb_exec(Box::new(move |_, _| {
        c2.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }));
    reg.on_syscall(Box::new(move |_| {
        c3.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }));

    reg.fire_vcpu_init(0);
    reg.fire_tb_exec(0, &dummy_tb(0x1000, 5));
    reg.fire_syscall(&SyscallInfo {
        number: 93,
        args: [0; 6],
        vcpu_idx: 0,
    });

    assert_eq!(called.load(std::sync::atomic::Ordering::Relaxed), 3);
}

#[test]
fn mem_filter_applied_by_registry() {
    let mut reg = PluginRegistry::new();
    let write_count = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
    let wc = write_count.clone();

    reg.on_mem_access(
        crate::callback::MemFilter::WritesOnly,
        Box::new(move |_, _| {
            wc.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }),
    );

    reg.fire_mem_access(
        0,
        &MemInfo {
            vaddr: 0x1000,
            size: 8,
            is_store: false,
            paddr: None,
        },
    );
    assert_eq!(write_count.load(std::sync::atomic::Ordering::Relaxed), 0);

    reg.fire_mem_access(
        0,
        &MemInfo {
            vaddr: 0x2000,
            size: 4,
            is_store: true,
            paddr: None,
        },
    );
    assert_eq!(write_count.load(std::sync::atomic::Ordering::Relaxed), 1);
}

#[test]
fn cache_sim_installs() {
    let mut plugin = crate::memory::CacheSim::new();
    let mut reg = PluginRegistry::new();
    plugin.install(&mut reg, &PluginArgs::new());
    assert!(reg.has_mem_callbacks());
}
