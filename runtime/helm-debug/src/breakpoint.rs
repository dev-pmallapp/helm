//! Breakpoint engine — fires actions when PC matches a breakpoint address.
//!
//! Uses a `HashSet<u64>` for O(1) address checks on the hot path. The full
//! breakpoint metadata (action, hit count, enabled flag) is stored in a Vec
//! indexed by breakpoint ID.

use std::collections::HashSet;

/// Action to take when a breakpoint fires.
#[derive(Debug, Clone)]
pub enum BreakAction {
    Break,
    Log,
    Callback(u64),
}

/// A PC breakpoint.
#[derive(Debug, Clone)]
pub struct Breakpoint {
    pub id: u32,
    pub addr: u64,
    pub action: BreakAction,
    pub enabled: bool,
    pub hit_count: u64,
}

/// Result of checking PC against breakpoints.
#[derive(Debug)]
pub enum BreakResult {
    None,
    Hit {
        breakpoint_id: u32,
        addr: u64,
        action: BreakAction,
    },
}

/// Engine managing PC breakpoints.
pub struct BreakpointEngine {
    breakpoints: Vec<Breakpoint>,
    /// O(1) lookup set of enabled breakpoint addresses.
    addr_set: HashSet<u64>,
    next_id: u32,
}

impl BreakpointEngine {
    pub fn new() -> Self {
        Self {
            breakpoints: Vec::new(),
            addr_set: HashSet::new(),
            next_id: 0,
        }
    }

    pub fn add(&mut self, addr: u64, action: BreakAction) -> u32 {
        self.add_with_state(addr, action, true, 0)
    }

    pub fn add_with_state(
        &mut self,
        addr: u64,
        action: BreakAction,
        enabled: bool,
        hit_count: u64,
    ) -> u32 {
        let id = self.next_id;
        self.next_id += 1;
        self.breakpoints.push(Breakpoint {
            id,
            addr,
            action,
            enabled,
            hit_count,
        });
        self.rebuild_addr_set_for(addr);
        id
    }

    pub fn remove(&mut self, id: u32) -> bool {
        if let Some(pos) = self.breakpoints.iter().position(|b| b.id == id) {
            let removed = self.breakpoints.remove(pos);
            self.rebuild_addr_set_for(removed.addr);
            true
        } else {
            false
        }
    }

    pub fn set_enabled(&mut self, id: u32, enabled: bool) -> bool {
        let addr = self.breakpoints.iter_mut().find(|b| b.id == id).map(|bp| {
            bp.enabled = enabled;
            bp.addr
        });
        if let Some(addr) = addr {
            self.rebuild_addr_set_for(addr);
            true
        } else {
            false
        }
    }

    /// Rebuild the addr_set entry for a specific address (handles multiple
    /// breakpoints at the same address and enabled/disabled state).
    fn rebuild_addr_set_for(&mut self, addr: u64) {
        let any_enabled = self.breakpoints.iter().any(|b| b.addr == addr && b.enabled);
        if any_enabled {
            self.addr_set.insert(addr);
        } else {
            self.addr_set.remove(&addr);
        }
    }

    pub fn check(&mut self, pc: u64) -> BreakResult {
        if !self.addr_set.contains(&pc) {
            return BreakResult::None;
        }
        for bp in &mut self.breakpoints {
            if bp.enabled && bp.addr == pc {
                bp.hit_count += 1;
                return BreakResult::Hit {
                    breakpoint_id: bp.id,
                    addr: bp.addr,
                    action: bp.action.clone(),
                };
            }
        }
        BreakResult::None
    }

    pub fn count(&self) -> usize {
        self.breakpoints.len()
    }
    pub fn clear(&mut self) {
        self.breakpoints.clear();
        self.addr_set.clear();
    }
    pub fn list(&self) -> &[Breakpoint] {
        &self.breakpoints
    }
    pub fn get(&self, id: u32) -> Option<&Breakpoint> {
        self.breakpoints.iter().find(|b| b.id == id)
    }
}

impl Default for BreakpointEngine {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn breakpoint_hit() {
        let mut e = BreakpointEngine::new();
        e.add(0x8000_0000, BreakAction::Break);
        assert!(matches!(e.check(0x8000_0000), BreakResult::Hit { .. }));
        assert!(matches!(e.check(0x8000_0004), BreakResult::None));
    }

    #[test]
    fn hit_count_increments() {
        let mut e = BreakpointEngine::new();
        let id = e.add(0x1000, BreakAction::Log);
        e.check(0x1000);
        e.check(0x1000);
        assert_eq!(e.get(id).unwrap().hit_count, 2);
    }

    #[test]
    fn disabled_skipped() {
        let mut e = BreakpointEngine::new();
        let id = e.add(0x1000, BreakAction::Break);
        e.set_enabled(id, false);
        assert!(matches!(e.check(0x1000), BreakResult::None));
    }
}
