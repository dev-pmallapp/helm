use super::*;
use crate::runtime::MemInfo;

fn mem(vaddr: u64, size: u8, is_store: bool) -> MemInfo {
    MemInfo {
        vaddr,
        size,
        is_store,
        is_atomic: false,
    }
}

#[test]
fn parses_configuration_and_counts_overlapping_accesses() {
    let mut plugin = Watchpoint::new();
    let mut reg = PluginRegistry::new();

    plugin.install(
        &mut reg,
        &PluginArgs::parse("addr=0x1000,size=8,type=all,value=0xdead"),
    );

    reg.fire_mem_access(0, &mem(0x1004, 4, false));
    reg.fire_mem_access(0, &mem(0x2000, 4, true));

    let guard = plugin.config.lock().unwrap();
    assert_eq!(guard.addr, 0x1000);
    assert_eq!(guard.size, 8);
    assert!(!guard.writes_only);
    assert_eq!(guard.value, Some(0xdead));
    assert_eq!(guard.hit_count, 1);
}

#[test]
fn default_filter_only_traces_writes() {
    let mut plugin = Watchpoint::with_addr(0x1000, 8, true, None);
    let mut reg = PluginRegistry::new();

    plugin.install(&mut reg, &PluginArgs::parse(""));

    reg.fire_mem_access(0, &mem(0x1000, 4, false));
    reg.fire_mem_access(0, &mem(0x1000, 4, true));

    assert_eq!(plugin.config.lock().unwrap().hit_count, 1);
}
