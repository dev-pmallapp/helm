use super::*;
use crate::runtime::MemInfo;

fn mem(vaddr: u64) -> MemInfo {
    MemInfo {
        pc: 0,
        raw: 0,
        opcode_name: "",
        class: crate::runtime::InsnClass::Unknown,
        vaddr,
        paddr: vaddr,
        size: 8,
        is_store: false,
        is_atomic: false,
        value_before: None,
        value_after: None,
    }
}

#[test]
fn parse_size_handles_units_and_fallback() {
    assert_eq!(parse_size("32KB"), 32 * 1024);
    assert_eq!(parse_size("2mb"), 2 * 1024 * 1024);
    assert_eq!(parse_size("64"), 64);
    assert_eq!(parse_size("bad"), 32 * 1024);
}

#[test]
fn records_hits_and_misses_for_repeated_lines() {
    let mut plugin = CacheSim::new();
    let mut reg = HelmPluginRegistry::new();

    plugin.install(
        &mut reg,
        &HelmPluginArgs::parse("l1d_size=64,l1d_assoc=1,l1d_line=16"),
    );

    reg.fire_mem_access(0, &mem(0x0));
    reg.fire_mem_access(0, &mem(0x0));

    assert_eq!(plugin.hits(), 1);
    assert_eq!(plugin.misses(), 1);
    assert_eq!(plugin.hit_rate(), 0.5);
}

#[test]
fn evicts_lru_entry_on_set_conflict() {
    let mut plugin = CacheSim::new();
    let mut reg = HelmPluginRegistry::new();

    plugin.install(
        &mut reg,
        &HelmPluginArgs::parse("l1d_size=64,l1d_assoc=1,l1d_line=16"),
    );

    reg.fire_mem_access(0, &mem(0x0));
    reg.fire_mem_access(0, &mem(0x40));
    reg.fire_mem_access(0, &mem(0x0));

    assert_eq!(plugin.hits(), 0);
    assert_eq!(plugin.misses(), 3);
}
