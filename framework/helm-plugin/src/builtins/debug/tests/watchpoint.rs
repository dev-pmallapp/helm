use super::*;
use crate::runtime::MemInfo;

fn mem(vaddr: u64, size: u8, is_store: bool) -> MemInfo {
    MemInfo {
        pc: 0x1234,
        raw: 0xF900_0040,
        opcode_name: "Str",
        class: crate::runtime::InsnClass::Store,
        vaddr,
        paddr: 0x5678,
        size,
        is_store,
        is_atomic: false,
        value_before: Some(0xdead),
        value_after: if is_store { Some(0xdead) } else { None },
    }
}

#[test]
fn parses_configuration_and_counts_overlapping_accesses() {
    let mut plugin = Watchpoint::new();
    let mut reg = HelmPluginRegistry::new();

    plugin.install(
        &mut reg,
        &HelmPluginArgs::parse("addr=0x1000,size=8,type=all,value=0xdead"),
    );

    reg.fire_mem_access(0, &mem(0x1004, 4, false));
    reg.fire_mem_access(0, &mem(0x2000, 4, true));

    let guard = plugin.config.lock().unwrap();
    assert_eq!(guard.addr, 0x1000);
    assert_eq!(guard.size, 8);
    assert!(!guard.match_paddr);
    assert!(!guard.writes_only);
    assert_eq!(guard.value, Some(0xdead));
    assert_eq!(guard.hit_count, 1);
}

#[test]
fn can_match_on_physical_address() {
    let mut plugin = Watchpoint::new();
    let mut reg = HelmPluginRegistry::new();

    plugin.install(
        &mut reg,
        &HelmPluginArgs::parse("addr=0x5678,size=8,type=all,space=pa"),
    );

    reg.fire_mem_access(0, &mem(0x1004, 4, false));

    let guard = plugin.config.lock().unwrap();
    assert!(guard.match_paddr);
    assert_eq!(guard.hit_count, 1);
}

#[test]
fn default_filter_only_traces_writes() {
    let mut plugin = Watchpoint::with_addr(0x1000, 8, true, None);
    let mut reg = HelmPluginRegistry::new();

    plugin.install(&mut reg, &HelmPluginArgs::parse(""));

    reg.fire_mem_access(0, &mem(0x1000, 4, false));
    reg.fire_mem_access(0, &mem(0x1000, 4, true));

    assert_eq!(plugin.config.lock().unwrap().hit_count, 1);
}

#[test]
fn value_filter_matches_observed_store_value() {
    let mut plugin = Watchpoint::new();
    let mut reg = HelmPluginRegistry::new();

    plugin.install(
        &mut reg,
        &HelmPluginArgs::parse("addr=0x1000,size=8,type=write,value=0x9"),
    );

    let miss = MemInfo {
        pc: 0x2000,
        raw: 0xF900_0040,
        opcode_name: "Str",
        class: crate::runtime::InsnClass::Store,
        vaddr: 0x1000,
        paddr: 0x3000,
        size: 8,
        is_store: true,
        is_atomic: false,
        value_before: Some(0x1111),
        value_after: Some(0x2222),
    };
    let hit = MemInfo {
        pc: 0x2004,
        raw: 0xF900_0060,
        opcode_name: "Str",
        class: crate::runtime::InsnClass::Store,
        vaddr: 0x1000,
        paddr: 0x3000,
        size: 8,
        is_store: true,
        is_atomic: false,
        value_before: Some(0x2222),
        value_after: Some(0x9),
    };

    reg.fire_mem_access(0, &miss);
    reg.fire_mem_access(0, &hit);

    assert_eq!(plugin.config.lock().unwrap().hit_count, 1);
    let guard = plugin.config.lock().unwrap();
    assert_eq!(guard.hits[0].pc, 0x2004);
    assert_eq!(guard.hits[0].raw, 0xF900_0060);
    assert_eq!(guard.hits[0].opcode_name, "Str");
    assert_eq!(guard.hits[0].class, crate::runtime::InsnClass::Store);
    assert_eq!(guard.captured_insns.last().unwrap().pc, 0x2004);
    assert_eq!(guard.captured_insns.last().unwrap().raw, 0xF900_0060);
}
