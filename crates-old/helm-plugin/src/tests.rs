//! Tests for the helm-plugin crate.

#[test]
fn test_plugin_api_available() {
    // Just ensure the API types are accessible
    use crate::api::{ComponentRegistry, HelmComponent, HelmPlugin, PluginArgs};
    let _args = PluginArgs::new();
    let _registry = ComponentRegistry::new();
}

#[cfg(feature = "builtins")]
#[test]
fn test_builtins_available() {
    // Ensure builtin plugins are accessible with feature flag
    use crate::builtins::memory::CacheSim;
    use crate::builtins::trace::InsnCount;
    let _insn_count = InsnCount::new();
    let _cache = CacheSim::new();
}

// ==========================================================================
// PluginArgs tests
// ==========================================================================

#[test]
fn plugin_args_new_is_empty() {
    use crate::api::PluginArgs;
    let args = PluginArgs::new();
    assert!(args.get("anything").is_none());
}

#[test]
fn plugin_args_parse_key_value_pairs() {
    use crate::api::PluginArgs;
    let args = PluginArgs::parse("key1=val1,key2=val2");
    assert_eq!(args.get("key1"), Some("val1"));
    assert_eq!(args.get("key2"), Some("val2"));
}

#[test]
fn plugin_args_parse_missing_key_returns_none() {
    use crate::api::PluginArgs;
    let args = PluginArgs::parse("a=1");
    assert!(args.get("b").is_none());
}

#[test]
fn plugin_args_get_or_returns_default_when_absent() {
    use crate::api::PluginArgs;
    let args = PluginArgs::new();
    assert_eq!(args.get_or("key", "fallback"), "fallback");
}

#[test]
fn plugin_args_get_or_returns_value_when_present() {
    use crate::api::PluginArgs;
    let args = PluginArgs::parse("key=hello");
    assert_eq!(args.get_or("key", "fallback"), "hello");
}

#[test]
fn plugin_args_get_usize_parses_integer() {
    use crate::api::PluginArgs;
    let args = PluginArgs::parse("n=42");
    assert_eq!(args.get_usize("n", 0), 42);
}

#[test]
fn plugin_args_get_usize_returns_default_when_absent() {
    use crate::api::PluginArgs;
    let args = PluginArgs::new();
    assert_eq!(args.get_usize("n", 99), 99);
}

#[test]
fn plugin_args_get_usize_returns_default_on_bad_value() {
    use crate::api::PluginArgs;
    let args = PluginArgs::parse("n=notanumber");
    assert_eq!(args.get_usize("n", 7), 7);
}

#[test]
fn plugin_args_parse_ignores_entries_without_equals() {
    use crate::api::PluginArgs;
    // "noeq" has no '=' so it is silently skipped
    let args = PluginArgs::parse("noeq,key=value");
    assert!(args.get("noeq").is_none());
    assert_eq!(args.get("key"), Some("value"));
}

// ==========================================================================
// PluginMetadata tests
// ==========================================================================

#[test]
fn plugin_metadata_fields_accessible() {
    use crate::api::metadata::{PluginMetadata, PLUGIN_API_VERSION};
    let meta = PluginMetadata {
        api_version: PLUGIN_API_VERSION,
        name: "test-plugin",
        version: "0.1.0",
        description: "A test plugin",
        author: "Test Author",
    };
    assert_eq!(meta.api_version, PLUGIN_API_VERSION);
    assert_eq!(meta.name, "test-plugin");
    assert_eq!(meta.version, "0.1.0");
    assert_eq!(meta.description, "A test plugin");
    assert_eq!(meta.author, "Test Author");
}

#[test]
fn plugin_api_version_is_nonzero() {
    use crate::api::metadata::PLUGIN_API_VERSION;
    assert!(PLUGIN_API_VERSION > 0);
}

// ==========================================================================
// ComponentRegistry tests
// ==========================================================================

#[test]
fn component_registry_starts_empty() {
    use crate::api::ComponentRegistry;
    let reg = ComponentRegistry::new();
    assert!(reg.list().is_empty());
}

#[test]
fn component_registry_register_and_list() {
    use crate::api::component::HelmComponent;
    use crate::api::{ComponentInfo, ComponentRegistry};

    struct Dummy;
    impl HelmComponent for Dummy {
        fn component_type(&self) -> &'static str {
            "test.dummy"
        }
    }

    let mut reg = ComponentRegistry::new();
    reg.register(ComponentInfo {
        type_name: "test.dummy",
        description: "dummy component",
        interfaces: &[],
        factory: Box::new(|| Box::new(Dummy)),
    });

    let types = reg.list();
    assert_eq!(types.len(), 1);
    assert!(types.contains(&"test.dummy"));
}

#[test]
fn component_registry_create_returns_none_for_unknown() {
    use crate::api::ComponentRegistry;
    let reg = ComponentRegistry::new();
    assert!(reg.create("nonexistent").is_none());
}

#[test]
fn component_registry_create_instantiates_component() {
    use crate::api::component::HelmComponent;
    use crate::api::{ComponentInfo, ComponentRegistry};

    struct Dummy;
    impl HelmComponent for Dummy {
        fn component_type(&self) -> &'static str {
            "test.dummy2"
        }
    }

    let mut reg = ComponentRegistry::new();
    reg.register(ComponentInfo {
        type_name: "test.dummy2",
        description: "dummy2",
        interfaces: &[],
        factory: Box::new(|| Box::new(Dummy)),
    });

    let inst = reg.create("test.dummy2");
    assert!(inst.is_some());
    assert_eq!(inst.unwrap().component_type(), "test.dummy2");
}

#[test]
fn component_registry_types_with_interface_filters_correctly() {
    use crate::api::component::HelmComponent;
    use crate::api::{ComponentInfo, ComponentRegistry};

    struct Alpha;
    impl HelmComponent for Alpha {
        fn component_type(&self) -> &'static str {
            "test.alpha"
        }
    }

    struct Beta;
    impl HelmComponent for Beta {
        fn component_type(&self) -> &'static str {
            "test.beta"
        }
    }

    let mut reg = ComponentRegistry::new();
    reg.register(ComponentInfo {
        type_name: "test.alpha",
        description: "alpha",
        interfaces: &["trace"],
        factory: Box::new(|| Box::new(Alpha)),
    });
    reg.register(ComponentInfo {
        type_name: "test.beta",
        description: "beta",
        interfaces: &["memory"],
        factory: Box::new(|| Box::new(Beta)),
    });

    let trace_types = reg.types_with_interface("trace");
    assert_eq!(trace_types.len(), 1);
    assert!(trace_types.contains(&"test.alpha"));

    let mem_types = reg.types_with_interface("memory");
    assert_eq!(mem_types.len(), 1);
    assert!(mem_types.contains(&"test.beta"));

    let none_types = reg.types_with_interface("nonexistent");
    assert!(none_types.is_empty());
}

#[test]
fn component_registry_multiple_registrations_grow_list() {
    use crate::api::component::HelmComponent;
    use crate::api::{ComponentInfo, ComponentRegistry};

    struct C1;
    impl HelmComponent for C1 {
        fn component_type(&self) -> &'static str {
            "test.c1"
        }
    }
    struct C2;
    impl HelmComponent for C2 {
        fn component_type(&self) -> &'static str {
            "test.c2"
        }
    }
    struct C3;
    impl HelmComponent for C3 {
        fn component_type(&self) -> &'static str {
            "test.c3"
        }
    }

    let mut reg = ComponentRegistry::new();
    reg.register(ComponentInfo {
        type_name: "test.c1",
        description: "",
        interfaces: &[],
        factory: Box::new(|| Box::new(C1)),
    });
    reg.register(ComponentInfo {
        type_name: "test.c2",
        description: "",
        interfaces: &[],
        factory: Box::new(|| Box::new(C2)),
    });
    reg.register(ComponentInfo {
        type_name: "test.c3",
        description: "",
        interfaces: &[],
        factory: Box::new(|| Box::new(C3)),
    });

    assert_eq!(reg.list().len(), 3);
}

// ==========================================================================
// MemFilter tests
// ==========================================================================

#[test]
fn mem_filter_all_matches_reads_and_writes() {
    use crate::runtime::callback::MemFilter;
    assert!(MemFilter::All.matches(false)); // read
    assert!(MemFilter::All.matches(true)); // write
}

#[test]
fn mem_filter_reads_only_matches_only_reads() {
    use crate::runtime::callback::MemFilter;
    assert!(MemFilter::ReadsOnly.matches(false));
    assert!(!MemFilter::ReadsOnly.matches(true));
}

#[test]
fn mem_filter_writes_only_matches_only_writes() {
    use crate::runtime::callback::MemFilter;
    assert!(!MemFilter::WritesOnly.matches(false));
    assert!(MemFilter::WritesOnly.matches(true));
}

// ==========================================================================
// PluginRegistry callback registration and dispatch tests
// ==========================================================================

#[test]
fn plugin_registry_starts_empty() {
    use crate::runtime::PluginRegistry;
    let reg = PluginRegistry::new();
    assert!(!reg.has_insn_callbacks());
    assert!(!reg.has_mem_callbacks());
}

#[test]
fn plugin_registry_has_insn_callbacks_after_registration() {
    use crate::runtime::PluginRegistry;
    let mut reg = PluginRegistry::new();
    reg.on_insn_exec(Box::new(|_vcpu, _insn| {}));
    assert!(reg.has_insn_callbacks());
}

#[test]
fn plugin_registry_has_mem_callbacks_after_registration() {
    use crate::runtime::{callback::MemFilter, PluginRegistry};
    let mut reg = PluginRegistry::new();
    reg.on_mem_access(MemFilter::All, Box::new(|_vcpu, _mem| {}));
    assert!(reg.has_mem_callbacks());
}

#[test]
fn plugin_registry_fire_vcpu_init_dispatches_to_all_callbacks() {
    use crate::runtime::PluginRegistry;
    use std::sync::{Arc, Mutex};

    let log: Arc<Mutex<Vec<usize>>> = Arc::new(Mutex::new(Vec::new()));

    let log1 = log.clone();
    let log2 = log.clone();

    let mut reg = PluginRegistry::new();
    reg.on_vcpu_init(Box::new(move |idx| {
        log1.lock().unwrap().push(idx);
    }));
    reg.on_vcpu_init(Box::new(move |idx| {
        log2.lock().unwrap().push(idx + 100);
    }));

    reg.fire_vcpu_init(3);

    let got = log.lock().unwrap().clone();
    assert_eq!(got.len(), 2);
    assert!(got.contains(&3));
    assert!(got.contains(&103));
}

#[test]
fn plugin_registry_fire_insn_exec_dispatches_to_all_callbacks() {
    use crate::runtime::{info::InsnInfo, PluginRegistry};
    use std::sync::{Arc, Mutex};

    let counter: Arc<Mutex<u64>> = Arc::new(Mutex::new(0));
    let c = counter.clone();

    let mut reg = PluginRegistry::new();
    reg.on_insn_exec(Box::new(move |_vcpu, _insn| {
        *c.lock().unwrap() += 1;
    }));

    let insn = InsnInfo {
        vaddr: 0x1000,
        bytes: vec![0x00, 0x00, 0x00, 0xd4],
        size: 4,
        mnemonic: "nop".to_string(),
        symbol: None,
    };

    reg.fire_insn_exec(0, &insn);
    reg.fire_insn_exec(0, &insn);
    reg.fire_insn_exec(1, &insn);

    assert_eq!(*counter.lock().unwrap(), 3);
}

#[test]
fn plugin_registry_fire_tb_exec_dispatches_to_all_callbacks() {
    use crate::runtime::{info::TbInfo, PluginRegistry};
    use std::sync::{Arc, Mutex};

    let pcs: Arc<Mutex<Vec<u64>>> = Arc::new(Mutex::new(Vec::new()));
    let p = pcs.clone();

    let mut reg = PluginRegistry::new();
    reg.on_tb_exec(Box::new(move |_vcpu, tb| {
        p.lock().unwrap().push(tb.pc);
    }));

    let tb = TbInfo {
        pc: 0xdeadbeef,
        insn_count: 5,
        size: 20,
    };
    reg.fire_tb_exec(0, &tb);

    let got = pcs.lock().unwrap().clone();
    assert_eq!(got, vec![0xdeadbeef]);
}

#[test]
fn plugin_registry_fire_mem_access_respects_filter() {
    use crate::runtime::{callback::MemFilter, info::MemInfo, PluginRegistry};
    use std::sync::{Arc, Mutex};

    let loads: Arc<Mutex<u32>> = Arc::new(Mutex::new(0));
    let stores: Arc<Mutex<u32>> = Arc::new(Mutex::new(0));

    let l = loads.clone();
    let s = stores.clone();

    let mut reg = PluginRegistry::new();
    reg.on_mem_access(
        MemFilter::ReadsOnly,
        Box::new(move |_, _| {
            *l.lock().unwrap() += 1;
        }),
    );
    reg.on_mem_access(
        MemFilter::WritesOnly,
        Box::new(move |_, _| {
            *s.lock().unwrap() += 1;
        }),
    );

    let read_info = MemInfo {
        vaddr: 0x1000,
        size: 4,
        is_store: false,
        paddr: None,
    };
    let write_info = MemInfo {
        vaddr: 0x2000,
        size: 8,
        is_store: true,
        paddr: None,
    };

    reg.fire_mem_access(0, &read_info);
    reg.fire_mem_access(0, &read_info);
    reg.fire_mem_access(0, &write_info);

    assert_eq!(*loads.lock().unwrap(), 2);
    assert_eq!(*stores.lock().unwrap(), 1);
}

#[test]
fn plugin_registry_fire_syscall_dispatches() {
    use crate::runtime::{info::SyscallInfo, PluginRegistry};
    use std::sync::{Arc, Mutex};

    let numbers: Arc<Mutex<Vec<u64>>> = Arc::new(Mutex::new(Vec::new()));
    let n = numbers.clone();

    let mut reg = PluginRegistry::new();
    reg.on_syscall(Box::new(move |info| {
        n.lock().unwrap().push(info.number);
    }));

    let info = SyscallInfo {
        number: 93,
        args: [0; 6],
        vcpu_idx: 0,
    };
    reg.fire_syscall(&info);

    assert_eq!(*numbers.lock().unwrap(), vec![93u64]);
}

#[test]
fn plugin_registry_fire_syscall_ret_dispatches() {
    use crate::runtime::{info::SyscallRetInfo, PluginRegistry};
    use std::sync::{Arc, Mutex};

    let rets: Arc<Mutex<Vec<u64>>> = Arc::new(Mutex::new(Vec::new()));
    let r = rets.clone();

    let mut reg = PluginRegistry::new();
    reg.on_syscall_ret(Box::new(move |info| {
        r.lock().unwrap().push(info.ret_value);
    }));

    let info = SyscallRetInfo {
        number: 1,
        ret_value: 42,
        vcpu_idx: 0,
    };
    reg.fire_syscall_ret(&info);

    assert_eq!(*rets.lock().unwrap(), vec![42u64]);
}

#[test]
fn plugin_registry_multiple_callbacks_same_event_all_fire() {
    use crate::runtime::{info::InsnInfo, PluginRegistry};
    use std::sync::{Arc, Mutex};

    let log: Arc<Mutex<Vec<u8>>> = Arc::new(Mutex::new(Vec::new()));
    let l1 = log.clone();
    let l2 = log.clone();
    let l3 = log.clone();

    let mut reg = PluginRegistry::new();
    reg.on_insn_exec(Box::new(move |_, _| {
        l1.lock().unwrap().push(1);
    }));
    reg.on_insn_exec(Box::new(move |_, _| {
        l2.lock().unwrap().push(2);
    }));
    reg.on_insn_exec(Box::new(move |_, _| {
        l3.lock().unwrap().push(3);
    }));

    let insn = InsnInfo {
        vaddr: 0,
        bytes: vec![],
        size: 0,
        mnemonic: "nop".into(),
        symbol: None,
    };
    reg.fire_insn_exec(0, &insn);

    let got = log.lock().unwrap().clone();
    assert_eq!(got.len(), 3);
    // Order is deterministic (insertion order)
    assert_eq!(got, vec![1, 2, 3]);
}

// ==========================================================================
// Scoreboard tests
// ==========================================================================

#[test]
fn scoreboard_new_has_correct_len() {
    use crate::runtime::Scoreboard;
    let sb = Scoreboard::<u64>::new(8);
    assert_eq!(sb.len(), 8);
    assert!(!sb.is_empty());
}

#[test]
fn scoreboard_zero_len_is_empty() {
    use crate::runtime::Scoreboard;
    let sb = Scoreboard::<u64>::new(0);
    assert_eq!(sb.len(), 0);
    assert!(sb.is_empty());
}

#[test]
fn scoreboard_default_value_is_type_default() {
    use crate::runtime::Scoreboard;
    let sb = Scoreboard::<u64>::new(4);
    // SAFETY: single-threaded test, no concurrent access
    for i in 0..4 {
        assert_eq!(*unsafe { &*sb.get(i) }, 0u64);
    }
}

#[test]
fn scoreboard_get_mut_allows_modification() {
    use crate::runtime::Scoreboard;
    let sb = Scoreboard::<u64>::new(4);
    // SAFETY: single-threaded, exclusive access per slot
    *sb.get_mut(0) = 100;
    *sb.get_mut(1) = 200;
    assert_eq!(*sb.get(0), 100);
    assert_eq!(*sb.get(1), 200);
    assert_eq!(*sb.get(2), 0);
}

#[test]
fn scoreboard_iter_yields_all_slots() {
    use crate::runtime::Scoreboard;
    let sb = Scoreboard::<u32>::new(3);
    *sb.get_mut(0) = 10;
    *sb.get_mut(1) = 20;
    *sb.get_mut(2) = 30;

    let vals: Vec<u32> = sb.iter().copied().collect();
    assert_eq!(vals, vec![10, 20, 30]);
}

#[test]
fn scoreboard_iter_sum_works() {
    use crate::runtime::Scoreboard;
    let sb = Scoreboard::<u64>::new(4);
    *sb.get_mut(0) = 5;
    *sb.get_mut(1) = 10;
    *sb.get_mut(2) = 15;
    *sb.get_mut(3) = 20;

    let total: u64 = sb.iter().sum();
    assert_eq!(total, 50);
}

// ==========================================================================
// HelmComponent lifecycle tests
// ==========================================================================

#[test]
fn helm_component_default_lifecycle_is_ok() {
    use crate::api::component::HelmComponent;

    struct MyComp;
    impl HelmComponent for MyComp {
        fn component_type(&self) -> &'static str {
            "test.mycomp"
        }
    }

    let mut c = MyComp;
    assert_eq!(c.component_type(), "test.mycomp");
    assert!(c.interfaces().is_empty());
    assert!(c.realize().is_ok());
    assert!(c.reset().is_ok());
    assert!(c.tick(100).is_ok());
}

#[test]
fn helm_component_interfaces_returned_when_provided() {
    use crate::api::component::HelmComponent;

    struct DevComp;
    impl HelmComponent for DevComp {
        fn component_type(&self) -> &'static str {
            "device.test"
        }
        fn interfaces(&self) -> &[&str] {
            &["memory-mapped", "irq"]
        }
    }

    let c = DevComp;
    assert_eq!(c.interfaces(), &["memory-mapped", "irq"]);
}

// ==========================================================================
// Built-in plugin: InsnCount
// ==========================================================================

#[cfg(feature = "builtins")]
#[test]
fn insn_count_starts_at_zero_before_install() {
    use crate::builtins::trace::InsnCount;
    let ic = InsnCount::new();
    assert_eq!(ic.total(), 0);
    assert!(ic.per_vcpu().is_empty());
}

#[cfg(feature = "builtins")]
#[test]
fn insn_count_name_is_correct() {
    use crate::api::HelmPlugin;
    use crate::builtins::trace::InsnCount;
    let ic = InsnCount::new();
    assert_eq!(ic.name(), "insn-count");
}

#[cfg(feature = "builtins")]
#[test]
fn insn_count_starts_at_zero_after_install() {
    use crate::api::{HelmPlugin, PluginArgs};
    use crate::builtins::trace::InsnCount;
    use crate::runtime::PluginRegistry;

    let mut reg = PluginRegistry::new();
    let mut ic = InsnCount::new();
    ic.install(&mut reg, &PluginArgs::new());

    assert_eq!(ic.total(), 0);
    assert_eq!(ic.per_vcpu().len(), 64); // scoreboard sized to 64 vCPUs
    assert!(ic.per_vcpu().iter().all(|&c| c == 0));
}

#[cfg(feature = "builtins")]
#[test]
fn insn_count_increments_on_fire_insn_exec() {
    use crate::api::{HelmPlugin, PluginArgs};
    use crate::builtins::trace::InsnCount;
    use crate::runtime::{info::InsnInfo, PluginRegistry};

    let mut reg = PluginRegistry::new();
    let mut ic = InsnCount::new();
    ic.install(&mut reg, &PluginArgs::new());

    let insn = InsnInfo {
        vaddr: 0x1000,
        bytes: vec![0x1f, 0x20, 0x03, 0xd5],
        size: 4,
        mnemonic: "nop".to_string(),
        symbol: None,
    };

    reg.fire_insn_exec(0, &insn);
    reg.fire_insn_exec(0, &insn);
    reg.fire_insn_exec(0, &insn);

    assert_eq!(ic.total(), 3);
    assert_eq!(ic.per_vcpu()[0], 3);
}

#[cfg(feature = "builtins")]
#[test]
fn insn_count_tracks_per_vcpu_independently() {
    use crate::api::{HelmPlugin, PluginArgs};
    use crate::builtins::trace::InsnCount;
    use crate::runtime::{info::InsnInfo, PluginRegistry};

    let mut reg = PluginRegistry::new();
    let mut ic = InsnCount::new();
    ic.install(&mut reg, &PluginArgs::new());

    let insn = InsnInfo {
        vaddr: 0x2000,
        bytes: vec![],
        size: 4,
        mnemonic: "add".to_string(),
        symbol: None,
    };

    // vCPU 0: 5 instructions, vCPU 1: 3 instructions, vCPU 2: 1 instruction
    for _ in 0..5 {
        reg.fire_insn_exec(0, &insn);
    }
    for _ in 0..3 {
        reg.fire_insn_exec(1, &insn);
    }
    reg.fire_insn_exec(2, &insn);

    assert_eq!(ic.total(), 9);
    assert_eq!(ic.per_vcpu()[0], 5);
    assert_eq!(ic.per_vcpu()[1], 3);
    assert_eq!(ic.per_vcpu()[2], 1);
}

#[cfg(feature = "builtins")]
#[test]
fn insn_count_total_is_sum_of_per_vcpu() {
    use crate::api::{HelmPlugin, PluginArgs};
    use crate::builtins::trace::InsnCount;
    use crate::runtime::{info::InsnInfo, PluginRegistry};

    let mut reg = PluginRegistry::new();
    let mut ic = InsnCount::new();
    ic.install(&mut reg, &PluginArgs::new());

    let insn = InsnInfo {
        vaddr: 0x3000,
        bytes: vec![],
        size: 4,
        mnemonic: "ldr".to_string(),
        symbol: None,
    };

    for vcpu in 0..8_usize {
        for _ in 0..(vcpu + 1) {
            reg.fire_insn_exec(vcpu, &insn);
        }
    }
    // vCPU 0: 1, 1: 2, ..., 7: 8 => total = 36
    let expected: u64 = (1..=8).sum();
    assert_eq!(ic.total(), expected);

    let per_vcpu = ic.per_vcpu();
    let sum: u64 = per_vcpu.iter().sum();
    assert_eq!(sum, ic.total());
}

// ==========================================================================
// Built-in plugin: HotBlocks
// ==========================================================================

#[cfg(feature = "builtins")]
#[test]
fn hotblocks_name_is_correct() {
    use crate::api::HelmPlugin;
    use crate::builtins::trace::HotBlocks;
    let hb = HotBlocks::new();
    assert_eq!(hb.name(), "hotblocks");
}

#[cfg(feature = "builtins")]
#[test]
fn hotblocks_top_returns_empty_before_install() {
    use crate::builtins::trace::HotBlocks;
    let hb = HotBlocks::new();
    assert!(hb.top(10).is_empty());
}

#[cfg(feature = "builtins")]
#[test]
fn hotblocks_install_registers_tb_exec_callback() {
    use crate::api::{HelmPlugin, PluginArgs};
    use crate::builtins::trace::HotBlocks;
    use crate::runtime::PluginRegistry;

    let mut reg = PluginRegistry::new();
    let mut hb = HotBlocks::new();
    hb.install(&mut reg, &PluginArgs::new());

    // Confirm that a tb_exec callback was registered
    assert!(!reg.tb_exec.is_empty());
}

#[cfg(feature = "builtins")]
#[test]
fn hotblocks_callbacks_fire_without_panic() {
    use crate::api::{HelmPlugin, PluginArgs};
    use crate::builtins::trace::HotBlocks;
    use crate::runtime::{info::TbInfo, PluginRegistry};

    let mut reg = PluginRegistry::new();
    let mut hb = HotBlocks::new();
    hb.install(&mut reg, &PluginArgs::new());

    let tb = TbInfo {
        pc: 0xabcd_0000,
        insn_count: 10,
        size: 40,
    };
    // Fire multiple times — should not panic
    for _ in 0..5 {
        reg.fire_tb_exec(0, &tb);
    }
}

#[cfg(feature = "builtins")]
#[test]
fn hotblocks_top_n_limits_result_count() {
    use crate::builtins::trace::HotBlocks;
    // top() on a fresh instance (no data) with various n values
    let hb = HotBlocks::new();
    assert!(hb.top(0).is_empty());
    assert!(hb.top(100).is_empty());
}

// ==========================================================================
// Built-in plugin: ExecLog
// ==========================================================================

#[cfg(feature = "builtins")]
#[test]
fn execlog_name_is_correct() {
    use crate::api::HelmPlugin;
    use crate::builtins::trace::ExecLog;
    let el = ExecLog::new();
    assert_eq!(el.name(), "execlog");
}

#[cfg(feature = "builtins")]
#[test]
fn execlog_lines_empty_before_install() {
    use crate::builtins::trace::ExecLog;
    let el = ExecLog::new();
    assert!(el.lines().is_empty());
}

#[cfg(feature = "builtins")]
#[test]
fn execlog_install_registers_insn_callback() {
    use crate::api::{HelmPlugin, PluginArgs};
    use crate::builtins::trace::ExecLog;
    use crate::runtime::PluginRegistry;

    let mut reg = PluginRegistry::new();
    let mut el = ExecLog::new();
    el.install(&mut reg, &PluginArgs::new());

    assert!(!reg.insn_exec.is_empty());
}

#[cfg(feature = "builtins")]
#[test]
fn execlog_fire_does_not_panic() {
    use crate::api::{HelmPlugin, PluginArgs};
    use crate::builtins::trace::ExecLog;
    use crate::runtime::{info::InsnInfo, PluginRegistry};

    let mut reg = PluginRegistry::new();
    let mut el = ExecLog::new();
    el.install(&mut reg, &PluginArgs::new());

    let insn = InsnInfo {
        vaddr: 0xffff_0000,
        bytes: vec![0x00, 0x01, 0x02, 0x03],
        size: 4,
        mnemonic: "str x0, [sp]".to_string(),
        symbol: Some("main".to_string()),
    };
    for _ in 0..10 {
        reg.fire_insn_exec(0, &insn);
    }
}

// ==========================================================================
// Built-in plugin: SyscallTrace
// ==========================================================================

#[cfg(feature = "builtins")]
#[test]
fn syscall_trace_name_is_correct() {
    use crate::api::HelmPlugin;
    use crate::builtins::trace::SyscallTrace;
    let st = SyscallTrace::new();
    assert_eq!(st.name(), "syscall-trace");
}

#[cfg(feature = "builtins")]
#[test]
fn syscall_trace_entries_empty_before_install() {
    use crate::builtins::trace::SyscallTrace;
    let st = SyscallTrace::new();
    assert!(st.entries().is_empty());
}

#[cfg(feature = "builtins")]
#[test]
fn syscall_trace_install_registers_callbacks() {
    use crate::api::{HelmPlugin, PluginArgs};
    use crate::builtins::trace::SyscallTrace;
    use crate::runtime::PluginRegistry;

    let mut reg = PluginRegistry::new();
    let mut st = SyscallTrace::new();
    st.install(&mut reg, &PluginArgs::new());

    assert!(!reg.syscall.is_empty());
    assert!(!reg.syscall_ret.is_empty());
}

#[cfg(feature = "builtins")]
#[test]
fn syscall_trace_fire_syscall_does_not_panic() {
    use crate::api::{HelmPlugin, PluginArgs};
    use crate::builtins::trace::SyscallTrace;
    use crate::runtime::{info::SyscallInfo, PluginRegistry};

    let mut reg = PluginRegistry::new();
    let mut st = SyscallTrace::new();
    st.install(&mut reg, &PluginArgs::new());

    let info = SyscallInfo {
        number: 64, // write
        args: [1, 0x1000, 13, 0, 0, 0],
        vcpu_idx: 0,
    };
    reg.fire_syscall(&info);
}

#[cfg(feature = "builtins")]
#[test]
fn syscall_trace_fire_syscall_ret_does_not_panic() {
    use crate::api::{HelmPlugin, PluginArgs};
    use crate::builtins::trace::SyscallTrace;
    use crate::runtime::{info::SyscallRetInfo, PluginRegistry};

    let mut reg = PluginRegistry::new();
    let mut st = SyscallTrace::new();
    st.install(&mut reg, &PluginArgs::new());

    let info = SyscallRetInfo {
        number: 64,
        ret_value: 13,
        vcpu_idx: 0,
    };
    reg.fire_syscall_ret(&info);
}

// ==========================================================================
// Built-in plugin: HowVec
// ==========================================================================

#[cfg(feature = "builtins")]
#[test]
fn howvec_name_is_correct() {
    use crate::api::HelmPlugin;
    use crate::builtins::trace::HowVec;
    let hv = HowVec::new();
    assert_eq!(hv.name(), "howvec");
}

#[cfg(feature = "builtins")]
#[test]
fn howvec_install_registers_insn_callback() {
    use crate::api::{HelmPlugin, PluginArgs};
    use crate::builtins::trace::HowVec;
    use crate::runtime::PluginRegistry;

    let mut reg = PluginRegistry::new();
    let mut hv = HowVec::new();
    hv.install(&mut reg, &PluginArgs::new());

    assert!(!reg.insn_exec.is_empty());
}

#[cfg(feature = "builtins")]
#[test]
fn howvec_fire_insn_exec_does_not_panic() {
    use crate::api::{HelmPlugin, PluginArgs};
    use crate::builtins::trace::HowVec;
    use crate::runtime::{info::InsnInfo, PluginRegistry};

    let mut reg = PluginRegistry::new();
    let mut hv = HowVec::new();
    hv.install(&mut reg, &PluginArgs::new());

    let mnemonics = [
        "add x0, x1, x2",
        "ldr x0, [x1]",
        "str x0, [sp]",
        "b 0x1000",
        "mul x0, x1, x2",
        "fadd d0, d1, d2",
        "svc #0",
        "nop",
    ];
    for mnemonic in &mnemonics {
        let insn = InsnInfo {
            vaddr: 0x1000,
            bytes: vec![],
            size: 4,
            mnemonic: mnemonic.to_string(),
            symbol: None,
        };
        reg.fire_insn_exec(0, &insn);
    }
}

// ==========================================================================
// Built-in plugin: CacheSim
// ==========================================================================

#[cfg(feature = "builtins")]
#[test]
fn cache_sim_name_is_correct() {
    use crate::api::HelmPlugin;
    use crate::builtins::memory::CacheSim;
    let cs = CacheSim::new();
    assert_eq!(cs.name(), "cache");
}

#[cfg(feature = "builtins")]
#[test]
fn cache_sim_l1d_hit_rate_is_zero_before_install() {
    use crate::builtins::memory::CacheSim;
    let cs = CacheSim::new();
    assert_eq!(cs.l1d_hit_rate(), 0.0);
}

#[cfg(feature = "builtins")]
#[test]
fn cache_sim_install_registers_mem_callback() {
    use crate::api::{HelmPlugin, PluginArgs};
    use crate::builtins::memory::CacheSim;
    use crate::runtime::PluginRegistry;

    let mut reg = PluginRegistry::new();
    let mut cs = CacheSim::new();
    cs.install(&mut reg, &PluginArgs::new());

    assert!(!reg.mem_access.is_empty());
}

#[cfg(feature = "builtins")]
#[test]
fn cache_sim_fire_mem_access_does_not_panic() {
    use crate::api::{HelmPlugin, PluginArgs};
    use crate::builtins::memory::CacheSim;
    use crate::runtime::{info::MemInfo, PluginRegistry};

    let mut reg = PluginRegistry::new();
    let mut cs = CacheSim::new();
    cs.install(&mut reg, &PluginArgs::new());

    let mem = MemInfo {
        vaddr: 0x8000,
        size: 8,
        is_store: false,
        paddr: Some(0x8000),
    };
    for _ in 0..100 {
        reg.fire_mem_access(0, &mem);
    }
}

#[cfg(feature = "builtins")]
#[test]
fn cache_sim_hit_rate_between_zero_and_one() {
    use crate::api::{HelmPlugin, PluginArgs};
    use crate::builtins::memory::CacheSim;
    use crate::runtime::{info::MemInfo, PluginRegistry};

    let mut reg = PluginRegistry::new();
    let mut cs = CacheSim::new();
    cs.install(&mut reg, &PluginArgs::new());

    let mem = MemInfo {
        vaddr: 0x4000,
        size: 4,
        is_store: false,
        paddr: None,
    };
    reg.fire_mem_access(0, &mem);

    let rate = cs.l1d_hit_rate();
    // Stub counts all as hits, so rate should be 1.0 (or in [0.0, 1.0])
    assert!((0.0..=1.0).contains(&rate));
}

// ==========================================================================
// register_builtins integration test
// ==========================================================================

#[cfg(feature = "builtins")]
#[test]
fn register_builtins_populates_all_six_plugins() {
    use crate::api::ComponentRegistry;
    use crate::runtime::register_builtins;

    let mut reg = ComponentRegistry::new();
    register_builtins(&mut reg);

    let types = reg.list();
    assert_eq!(types.len(), 7, "expected exactly 7 builtin plugins");

    assert!(types.contains(&"plugin.trace.insn-count"));
    assert!(types.contains(&"plugin.trace.execlog"));
    assert!(types.contains(&"plugin.trace.hotblocks"));
    assert!(types.contains(&"plugin.trace.howvec"));
    assert!(types.contains(&"plugin.trace.syscall-trace"));
    assert!(types.contains(&"plugin.memory.cache"));
}

#[cfg(feature = "builtins")]
#[test]
fn register_builtins_trace_interface_present() {
    use crate::api::ComponentRegistry;
    use crate::runtime::register_builtins;

    let mut reg = ComponentRegistry::new();
    register_builtins(&mut reg);

    let trace_types = reg.types_with_interface("trace");
    // insn-count, execlog, hotblocks, howvec, syscall-trace all implement "trace"
    assert!(trace_types.len() >= 5, "expected at least 5 trace plugins");
}

#[cfg(feature = "builtins")]
#[test]
fn register_builtins_memory_interface_present() {
    use crate::api::ComponentRegistry;
    use crate::runtime::register_builtins;

    let mut reg = ComponentRegistry::new();
    register_builtins(&mut reg);

    let mem_types = reg.types_with_interface("memory");
    assert!(!mem_types.is_empty());
    assert!(mem_types.contains(&"plugin.memory.cache"));
}

#[cfg(feature = "builtins")]
#[test]
fn register_builtins_profiling_interface_present() {
    use crate::api::ComponentRegistry;
    use crate::runtime::register_builtins;

    let mut reg = ComponentRegistry::new();
    register_builtins(&mut reg);

    let prof_types = reg.types_with_interface("profiling");
    // hotblocks, howvec, cache all have "profiling"
    assert!(prof_types.len() >= 2);
}

#[cfg(feature = "builtins")]
#[test]
fn register_builtins_each_plugin_can_be_instantiated() {
    use crate::api::ComponentRegistry;
    use crate::runtime::register_builtins;

    let mut reg = ComponentRegistry::new();
    register_builtins(&mut reg);

    for type_name in &[
        "plugin.trace.insn-count",
        "plugin.trace.execlog",
        "plugin.trace.hotblocks",
        "plugin.trace.howvec",
        "plugin.trace.syscall-trace",
        "plugin.memory.cache",
    ] {
        let inst = reg.create(type_name);
        assert!(inst.is_some(), "failed to instantiate {type_name}");
        let comp = inst.unwrap();
        assert_eq!(comp.component_type(), *type_name);
    }
}

// ==========================================================================
// PluginComponentAdapter tests
// ==========================================================================

#[cfg(feature = "builtins")]
#[test]
fn plugin_component_adapter_install_once_only() {
    use crate::api::PluginArgs;
    use crate::builtins::trace::InsnCount;
    use crate::runtime::{bridge::PluginComponentAdapter, PluginRegistry};

    let plugin: Box<dyn crate::api::HelmPlugin> = Box::new(InsnCount::new());
    let mut adapter = PluginComponentAdapter::new(plugin, "plugin.trace.insn-count", &["trace"]);

    let mut reg = PluginRegistry::new();
    let args = PluginArgs::new();

    // First install registers callbacks
    adapter.install(&mut reg, &args);
    let count_after_first = reg.insn_exec.len();

    // Second install should be a no-op (already installed)
    adapter.install(&mut reg, &args);
    let count_after_second = reg.insn_exec.len();

    assert_eq!(
        count_after_first, count_after_second,
        "second install should not register additional callbacks"
    );
}

#[cfg(feature = "builtins")]
#[test]
fn plugin_component_adapter_component_type_matches() {
    use crate::api::component::HelmComponent;
    use crate::builtins::trace::InsnCount;
    use crate::runtime::bridge::PluginComponentAdapter;

    let plugin: Box<dyn crate::api::HelmPlugin> = Box::new(InsnCount::new());
    let adapter = PluginComponentAdapter::new(plugin, "plugin.trace.insn-count", &["trace"]);

    assert_eq!(adapter.component_type(), "plugin.trace.insn-count");
}

#[cfg(feature = "builtins")]
#[test]
fn plugin_component_adapter_reset_allows_reinstall() {
    use crate::api::component::HelmComponent;
    use crate::api::PluginArgs;
    use crate::builtins::trace::InsnCount;
    use crate::runtime::{bridge::PluginComponentAdapter, PluginRegistry};

    let plugin: Box<dyn crate::api::HelmPlugin> = Box::new(InsnCount::new());
    let mut adapter = PluginComponentAdapter::new(plugin, "plugin.trace.insn-count", &["trace"]);

    let mut reg = PluginRegistry::new();
    let args = PluginArgs::new();

    adapter.install(&mut reg, &args);
    let count_before_reset = reg.insn_exec.len();

    // Reset marks the adapter as not installed
    adapter.reset().unwrap();

    // A fresh registry — after reset, install should register again
    let mut reg2 = PluginRegistry::new();
    adapter.install(&mut reg2, &args);

    assert_eq!(
        reg2.insn_exec.len(),
        count_before_reset,
        "reinstall after reset should register the same number of callbacks"
    );
}

// ==========================================================================
// InsnInfo / TbInfo / MemInfo / SyscallInfo field tests
// ==========================================================================

#[test]
fn insn_info_fields_accessible() {
    use crate::runtime::info::InsnInfo;
    let insn = InsnInfo {
        vaddr: 0xdead_beef,
        bytes: vec![0xab, 0xcd],
        size: 2,
        mnemonic: "bl #0x100".to_string(),
        symbol: Some("my_func".to_string()),
    };
    assert_eq!(insn.vaddr, 0xdead_beef);
    assert_eq!(insn.bytes, vec![0xab, 0xcd]);
    assert_eq!(insn.size, 2);
    assert_eq!(insn.mnemonic, "bl #0x100");
    assert_eq!(insn.symbol.as_deref(), Some("my_func"));
}

#[test]
fn tb_info_fields_accessible() {
    use crate::runtime::info::TbInfo;
    let tb = TbInfo {
        pc: 0x1_0000,
        insn_count: 7,
        size: 28,
    };
    assert_eq!(tb.pc, 0x1_0000);
    assert_eq!(tb.insn_count, 7);
    assert_eq!(tb.size, 28);
}

#[test]
fn mem_info_fields_accessible() {
    use crate::runtime::info::MemInfo;
    let m = MemInfo {
        vaddr: 0x5000,
        size: 8,
        is_store: true,
        paddr: Some(0x5000),
    };
    assert_eq!(m.vaddr, 0x5000);
    assert_eq!(m.size, 8);
    assert!(m.is_store);
    assert_eq!(m.paddr, Some(0x5000));
}

#[test]
fn syscall_info_fields_accessible() {
    use crate::runtime::info::SyscallInfo;
    let s = SyscallInfo {
        number: 93,
        args: [1, 2, 3, 4, 5, 6],
        vcpu_idx: 2,
    };
    assert_eq!(s.number, 93);
    assert_eq!(s.args, [1, 2, 3, 4, 5, 6]);
    assert_eq!(s.vcpu_idx, 2);
}

#[test]
fn syscall_ret_info_fields_accessible() {
    use crate::runtime::info::SyscallRetInfo;
    let s = SyscallRetInfo {
        number: 93,
        ret_value: 0,
        vcpu_idx: 0,
    };
    assert_eq!(s.number, 93);
    assert_eq!(s.ret_value, 0);
    assert_eq!(s.vcpu_idx, 0);
}

// ==========================================================================
// Built-in plugin tests
// ==========================================================================

#[test]
fn insn_count_new_total_is_zero() {
    use crate::builtins::trace::InsnCount;
    let counter = InsnCount::new();
    assert_eq!(counter.total(), 0);
}

#[test]
fn insn_count_per_vcpu_empty_before_install() {
    use crate::builtins::trace::InsnCount;
    let counter = InsnCount::new();
    assert!(counter.per_vcpu().is_empty());
}

#[test]
fn insn_count_install_and_fire() {
    use crate::api::plugin::{HelmPlugin, PluginArgs};
    use crate::builtins::trace::InsnCount;
    use crate::runtime::info::InsnInfo;
    use crate::runtime::registry::PluginRegistry;

    let mut counter = InsnCount::new();
    let mut reg = PluginRegistry::new();
    counter.install(&mut reg, &PluginArgs::new());

    let insn = InsnInfo {
        vaddr: 0x1000,
        size: 4,
        bytes: vec![0; 4],
        mnemonic: String::new(),
        symbol: None,
    };
    reg.fire_insn_exec(0, &insn);
    reg.fire_insn_exec(0, &insn);
    reg.fire_insn_exec(1, &insn);

    assert_eq!(counter.total(), 3);
    let per_vcpu = counter.per_vcpu();
    assert_eq!(per_vcpu[0], 2);
    assert_eq!(per_vcpu[1], 1);
}

#[test]
fn exec_log_new_is_empty() {
    use crate::builtins::trace::ExecLog;
    let log = ExecLog::new();
    assert!(log.lines().is_empty());
}

#[test]
fn exec_log_install_and_fire() {
    use crate::api::plugin::{HelmPlugin, PluginArgs};
    use crate::builtins::trace::ExecLog;
    use crate::runtime::info::InsnInfo;
    use crate::runtime::registry::PluginRegistry;

    let mut log = ExecLog::new();
    let mut reg = PluginRegistry::new();
    log.install(&mut reg, &PluginArgs::new());

    let insn = InsnInfo {
        vaddr: 0x1000,
        size: 4,
        bytes: vec![0x1F, 0x20, 0x03, 0xD5],
        mnemonic: "NOP".to_string(),
        symbol: None,
    };
    reg.fire_insn_exec(0, &insn);
}

#[test]
fn hot_blocks_new_is_empty() {
    use crate::builtins::trace::HotBlocks;
    let hb = HotBlocks::new();
    assert!(hb.top(10).is_empty());
}

#[test]
fn hot_blocks_install_and_fire() {
    use crate::api::plugin::{HelmPlugin, PluginArgs};
    use crate::builtins::trace::HotBlocks;
    use crate::runtime::info::TbInfo;
    use crate::runtime::registry::PluginRegistry;

    let mut hb = HotBlocks::new();
    let mut reg = PluginRegistry::new();
    hb.install(&mut reg, &PluginArgs::new());

    let tb = TbInfo {
        pc: 0x2000,
        size: 16,
        insn_count: 4,
    };
    reg.fire_tb_exec(0, &tb);
    reg.fire_tb_exec(0, &tb);
}

#[test]
fn syscall_trace_new_is_empty() {
    use crate::builtins::trace::SyscallTrace;
    let st = SyscallTrace::new();
    assert!(st.entries().is_empty());
}

#[test]
fn syscall_trace_install_and_fire() {
    use crate::api::plugin::{HelmPlugin, PluginArgs};
    use crate::builtins::trace::SyscallTrace;
    use crate::runtime::info::SyscallInfo;
    use crate::runtime::registry::PluginRegistry;

    let mut st = SyscallTrace::new();
    let mut reg = PluginRegistry::new();
    st.install(&mut reg, &PluginArgs::new());

    let info = SyscallInfo {
        number: 64,
        args: [1, 0, 0, 0, 0, 0],
        vcpu_idx: 0,
    };
    reg.fire_syscall(&info);
}

#[test]
fn cache_sim_new_zero_hit_rate() {
    use crate::builtins::memory::CacheSim;
    let cs = CacheSim::new();
    assert!(cs.l1d_hit_rate().is_nan() || cs.l1d_hit_rate() == 0.0);
}

#[test]
fn cache_sim_install_and_fire() {
    use crate::api::plugin::{HelmPlugin, PluginArgs};
    use crate::builtins::memory::CacheSim;
    use crate::runtime::info::MemInfo;
    use crate::runtime::registry::PluginRegistry;

    let mut cs = CacheSim::new();
    let mut reg = PluginRegistry::new();
    cs.install(&mut reg, &PluginArgs::new());

    let mem = MemInfo {
        vaddr: 0x1000,
        paddr: Some(0x1000),
        size: 4,
        is_store: false,
    };
    reg.fire_mem_access(0, &mem);
    reg.fire_mem_access(0, &mem);
}

// ==========================================================================
// Built-in plugin: FaultDetect
// ==========================================================================

#[cfg(feature = "builtins")]
#[test]
fn fault_detect_name_is_correct() {
    use crate::api::HelmPlugin;
    use crate::builtins::debug::FaultDetect;
    let fd = FaultDetect::new();
    assert_eq!(fd.name(), "fault-detect");
}

#[cfg(feature = "builtins")]
#[test]
fn fault_detect_no_reports_initially() {
    use crate::builtins::debug::FaultDetect;
    let fd = FaultDetect::new();
    assert!(fd.reports().is_empty());
}

#[cfg(feature = "builtins")]
#[test]
fn fault_detect_installs_callbacks() {
    use crate::api::{HelmPlugin, PluginArgs};
    use crate::builtins::debug::FaultDetect;
    use crate::runtime::PluginRegistry;

    let mut reg = PluginRegistry::new();
    let mut fd = FaultDetect::new();
    fd.install(&mut reg, &PluginArgs::new());

    assert!(!reg.insn_exec.is_empty());
    assert!(!reg.syscall.is_empty());
    assert!(!reg.syscall_ret.is_empty());
    assert!(!reg.fault.is_empty());
}

#[cfg(feature = "builtins")]
#[test]
fn fault_detect_records_null_jump_fault() {
    use crate::api::{HelmPlugin, PluginArgs};
    use crate::builtins::debug::FaultDetect;
    use crate::runtime::info::{ArchContext, FaultInfo, FaultKind};
    use crate::runtime::PluginRegistry;

    let mut reg = PluginRegistry::new();
    let mut fd = FaultDetect::new();
    fd.install(&mut reg, &PluginArgs::new());

    reg.fire_fault(&FaultInfo {
        vcpu_idx: 0,
        pc: 0,
        insn_word: 0,
        fault_kind: FaultKind::NullJump,
        message: "jump to NULL".into(),
        insn_count: 1000,
        arch_context: ArchContext::Aarch64 {
            x: [0; 31],
            sp: 0x7fff0000,
            pc: 0,
            nzcv: 0,
            tpidr_el0: 0x1000,
            current_el: 0,
        },
    });

    let reports = fd.reports();
    assert_eq!(reports.len(), 1);
    assert_eq!(reports[0].kind, FaultKind::NullJump);
    assert!(reports[0].summary.contains("NULL"));
}

#[cfg(feature = "builtins")]
#[test]
fn fault_detect_tracks_pc_history() {
    use crate::api::{HelmPlugin, PluginArgs};
    use crate::builtins::debug::FaultDetect;
    use crate::runtime::info::{ArchContext, FaultInfo, FaultKind, InsnInfo};
    use crate::runtime::PluginRegistry;

    let mut reg = PluginRegistry::new();
    let mut fd = FaultDetect::new();
    fd.install(&mut reg, &PluginArgs::parse("ring=8"));

    for pc in [0x1000u64, 0x1004, 0x1008, 0x100c] {
        reg.fire_insn_exec(
            0,
            &InsnInfo {
                vaddr: pc,
                bytes: vec![],
                size: 4,
                mnemonic: String::new(),
                symbol: None,
            },
        );
    }

    reg.fire_fault(&FaultInfo {
        vcpu_idx: 0,
        pc: 0,
        insn_word: 0,
        fault_kind: FaultKind::NullJump,
        message: "test".into(),
        insn_count: 4,
        arch_context: ArchContext::None,
    });

    let reports = fd.reports();
    assert_eq!(reports[0].pc_history, vec![0x1000, 0x1004, 0x1008, 0x100c]);
}

#[cfg(feature = "builtins")]
#[test]
fn fault_detect_logs_syscalls() {
    use crate::api::{HelmPlugin, PluginArgs};
    use crate::builtins::debug::FaultDetect;
    use crate::runtime::info::{ArchContext, FaultInfo, FaultKind, SyscallInfo};
    use crate::runtime::PluginRegistry;

    let mut reg = PluginRegistry::new();
    let mut fd = FaultDetect::new();
    fd.install(&mut reg, &PluginArgs::new());

    reg.fire_syscall(&SyscallInfo {
        number: 64,
        args: [1, 0x2000, 5, 0, 0, 0],
        vcpu_idx: 0,
    });

    reg.fire_fault(&FaultInfo {
        vcpu_idx: 0,
        pc: 0x3000,
        insn_word: 0,
        fault_kind: FaultKind::Undef,
        message: "undef insn".into(),
        insn_count: 10,
        arch_context: ArchContext::None,
    });

    let reports = fd.reports();
    assert_eq!(reports.len(), 1);
    assert!(!reports[0].syscall_log.is_empty());
    assert!(reports[0].syscall_log[0].contains("nr=64"));
}

#[cfg(feature = "builtins")]
#[test]
fn fault_detect_tls_aliasing_detected() {
    use crate::api::{HelmPlugin, PluginArgs};
    use crate::builtins::debug::FaultDetect;
    use crate::runtime::info::{FaultKind, SyscallInfo};
    use crate::runtime::PluginRegistry;

    let mut reg = PluginRegistry::new();
    let mut fd = FaultDetect::new();
    fd.install(&mut reg, &PluginArgs::new());

    let clone_flags: u64 = 0x7d0f00; // includes CLONE_SETTLS
    let tls_ptr: u64 = 0x1033d10;

    reg.fire_syscall(&SyscallInfo {
        number: 220,
        args: [clone_flags, 0, 0, tls_ptr, 0, 0],
        vcpu_idx: 0,
    });
    reg.fire_syscall(&SyscallInfo {
        number: 220,
        args: [clone_flags, 0, 0, tls_ptr, 0, 0],
        vcpu_idx: 1,
    });

    let reports = fd.reports();
    assert_eq!(reports.len(), 1);
    assert_eq!(reports[0].kind, FaultKind::TlsAliasing);
}

#[cfg(feature = "builtins")]
#[test]
fn fault_detect_unsupported_critical_syscall() {
    use crate::api::{HelmPlugin, PluginArgs};
    use crate::builtins::debug::FaultDetect;
    use crate::runtime::info::{FaultKind, SyscallRetInfo};
    use crate::runtime::PluginRegistry;

    let mut reg = PluginRegistry::new();
    let mut fd = FaultDetect::new();
    fd.install(&mut reg, &PluginArgs::new());

    let enosys = (-38i64) as u64;
    reg.fire_syscall_ret(&SyscallRetInfo {
        number: 222,
        ret_value: enosys,
        vcpu_idx: 0,
    });

    let reports = fd.reports();
    assert_eq!(reports.len(), 1);
    assert_eq!(reports[0].kind, FaultKind::UnsupportedSyscall);
}

#[cfg(feature = "builtins")]
#[test]
fn fault_detect_max_reports_respected() {
    use crate::api::{HelmPlugin, PluginArgs};
    use crate::builtins::debug::FaultDetect;
    use crate::runtime::info::{ArchContext, FaultInfo, FaultKind};
    use crate::runtime::PluginRegistry;

    let mut reg = PluginRegistry::new();
    let mut fd = FaultDetect::new();
    fd.install(&mut reg, &PluginArgs::parse("max_reports=2"));

    for i in 0..5 {
        reg.fire_fault(&FaultInfo {
            vcpu_idx: 0,
            pc: i,
            insn_word: 0,
            fault_kind: FaultKind::Undef,
            message: format!("fault {i}"),
            insn_count: i,
            arch_context: ArchContext::None,
        });
    }

    assert_eq!(fd.reports().len(), 2);
}

#[cfg(feature = "builtins")]
#[test]
fn fault_detect_in_builtin_registry() {
    use crate::api::ComponentRegistry;
    use crate::runtime::register_builtins;

    let mut reg = ComponentRegistry::new();
    register_builtins(&mut reg);

    assert!(reg.list().contains(&"plugin.debug.fault-detect"));
    let inst = reg.create("plugin.debug.fault-detect");
    assert!(inst.is_some());
}
