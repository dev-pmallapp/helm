use crate::api::{HelmPlugin, HelmPluginArgs};
use crate::runtime::{MemFilter, HelmPluginRegistry};
use std::sync::{Arc, Mutex};

struct WatchConfig {
    addr: u64,
    size: u64,
    writes_only: bool,
    value: Option<u64>,
    hit_count: u64,
}

/// Address watchpoint — fires a fault callback when a watched address is accessed.
pub struct Watchpoint {
    config: Arc<Mutex<WatchConfig>>,
}

impl Watchpoint {
    pub fn new() -> Self {
        Self {
            config: Arc::new(Mutex::new(WatchConfig {
                addr: 0,
                size: 8,
                writes_only: true,
                value: None,
                hit_count: 0,
            })),
        }
    }

    pub fn with_addr(addr: u64, size: u64, writes_only: bool, value: Option<u64>) -> Self {
        Self {
            config: Arc::new(Mutex::new(WatchConfig {
                addr,
                size,
                writes_only,
                value,
                hit_count: 0,
            })),
        }
    }
}

impl Default for Watchpoint {
    fn default() -> Self {
        Self::new()
    }
}

impl HelmPlugin for Watchpoint {
    fn name(&self) -> &str {
        "watchpoint"
    }

    fn install(&mut self, reg: &mut HelmPluginRegistry, args: &HelmPluginArgs) {
        // Parse args: addr=0x1000,size=8,type=write,value=0xDEAD
        if let Some(addr_str) = args.get("addr") {
            let addr = if let Some(hex) = addr_str.strip_prefix("0x") {
                u64::from_str_radix(hex, 16).unwrap_or(0)
            } else {
                addr_str.parse::<u64>().unwrap_or(0)
            };
            let mut guard = self.config.lock().unwrap();
            guard.addr = addr;
        }
        if let Some(s) = args.get("size") {
            self.config.lock().unwrap().size = s.parse::<u64>().unwrap_or(8);
        }
        if let Some(ty) = args.get("type") {
            self.config.lock().unwrap().writes_only = ty != "all";
        }
        if let Some(val_str) = args.get("value") {
            let val = if let Some(hex) = val_str.strip_prefix("0x") {
                u64::from_str_radix(hex, 16).ok()
            } else {
                val_str.parse::<u64>().ok()
            };
            self.config.lock().unwrap().value = val;
        }

        let config = Arc::clone(&self.config);
        let filter = if self.config.lock().unwrap().writes_only {
            MemFilter::WritesOnly
        } else {
            MemFilter::All
        };

        reg.on_mem_access(
            filter,
            Box::new(move |_vcpu_idx, info| {
                let mut guard = config.lock().unwrap();
                let access_end = info.vaddr + info.size as u64;
                let watch_end = guard.addr + guard.size;

                // Check overlap
                if info.vaddr < watch_end && access_end > guard.addr {
                    // If value condition is set, we can't check it here (no value in MemInfo)
                    // so we fire unconditionally on address match
                    if guard.value.is_some() {
                        // Value matching would require MemInfo.value — fire anyway
                    }
                    guard.hit_count += 1;
                    eprintln!(
                        "[watchpoint] HIT #{} at {:#018x} (size={}, {})",
                        guard.hit_count,
                        info.vaddr,
                        info.size,
                        if info.is_store { "WRITE" } else { "READ" }
                    );
                }
            }),
        );
    }

    fn atexit(&mut self) {
        let guard = self.config.lock().unwrap();
        eprintln!(
            "[watchpoint] {} hit(s) on watch addr={:#018x} size={}",
            guard.hit_count, guard.addr, guard.size
        );
    }
}

#[cfg(test)]
#[path = "tests/watchpoint.rs"]
mod tests;
