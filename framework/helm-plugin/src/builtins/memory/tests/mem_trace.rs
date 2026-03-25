use super::*;
use crate::runtime::MemInfo;

fn mem(vaddr: u64, is_store: bool, is_atomic: bool) -> MemInfo {
    MemInfo {
        vaddr,
        size: 8,
        is_store,
        is_atomic,
    }
}

#[test]
fn writes_only_filter_and_max_are_applied() {
    let mut plugin = MemTrace::new();
    let mut reg = HelmPluginRegistry::new();

    plugin.install(&mut reg, &HelmPluginArgs::parse("writes-only=true,max=1"));

    reg.fire_mem_access(0, &mem(0x1000, false, false));
    reg.fire_mem_access(0, &mem(0x2000, true, true));
    reg.fire_mem_access(0, &mem(0x3000, true, false));

    let entries = plugin.entries.lock().unwrap();
    assert_eq!(entries.as_slice(), ["[W] 0x0000000000002000 8 atomic"]);
}

#[test]
fn defaults_to_tracing_reads_and_writes() {
    let mut plugin = MemTrace::new();
    let mut reg = HelmPluginRegistry::new();

    plugin.install(&mut reg, &HelmPluginArgs::parse(""));

    reg.fire_mem_access(0, &mem(0x1000, false, false));
    reg.fire_mem_access(0, &mem(0x2000, true, false));

    let entries = plugin.entries.lock().unwrap();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0], "[R] 0x0000000000001000 8");
    assert_eq!(entries[1], "[W] 0x0000000000002000 8");
}
