//! Register rename unit — maps architectural registers to physical registers.

use helm_core::types::RegId;
use std::collections::HashMap;

/// Physical register identifier.
pub type PhysReg = u32;

pub struct RenameUnit {
    /// Current mapping: architectural -> physical.
    rat: HashMap<RegId, PhysReg>,
    /// Free list of physical registers.
    free_list: Vec<PhysReg>,
    next_phys: PhysReg,
}

impl Default for RenameUnit {
    fn default() -> Self {
        Self::new()
    }
}

impl RenameUnit {
    pub fn new() -> Self {
        Self {
            rat: HashMap::new(),
            free_list: Vec::new(),
            next_phys: 0,
        }
    }

    /// Rename a destination register, returning the allocated physical register.
    pub fn rename_dest(&mut self, arch_reg: RegId) -> PhysReg {
        let phys = self.alloc_phys();
        self.rat.insert(arch_reg, phys);
        phys
    }

    /// Look up the current physical mapping for an architectural source register.
    pub fn lookup_src(&self, arch_reg: RegId) -> PhysReg {
        self.rat
            .get(&arch_reg)
            .copied()
            .unwrap_or(arch_reg as PhysReg)
    }

    /// Release a physical register back to the free list.
    pub fn free(&mut self, phys: PhysReg) {
        self.free_list.push(phys);
    }

    fn alloc_phys(&mut self) -> PhysReg {
        if let Some(p) = self.free_list.pop() {
            p
        } else {
            let p = self.next_phys;
            self.next_phys += 1;
            p
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rename_dest_returns_unique_phys_regs() {
        let mut ru = RenameUnit::new();
        let p0 = ru.rename_dest(0);
        let p1 = ru.rename_dest(1);
        assert_ne!(p0, p1);
    }

    #[test]
    fn lookup_src_reflects_latest_rename() {
        let mut ru = RenameUnit::new();
        let p = ru.rename_dest(5);
        assert_eq!(ru.lookup_src(5), p);

        let p2 = ru.rename_dest(5); // re-rename same arch reg
        assert_ne!(p, p2);
        assert_eq!(ru.lookup_src(5), p2);
    }

    #[test]
    fn freed_regs_are_reused() {
        let mut ru = RenameUnit::new();
        let p0 = ru.rename_dest(0);
        ru.free(p0);
        let p1 = ru.rename_dest(1);
        assert_eq!(p0, p1, "freed phys reg should be reused");
    }
}
