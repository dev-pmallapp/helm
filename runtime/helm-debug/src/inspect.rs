//! Inspection API — dump arch state, memory ranges, device state on demand.

use std::collections::BTreeMap;

/// A symbol visible to the current debug target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SymbolView {
    pub name: String,
    pub addr: u64,
    pub size: u64,
}

/// A snapshot of inspectable state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InspectionResult {
    pub arch: Option<String>,
    pub int_regs: Vec<(String, u64)>,
    pub pc: u64,
    pub symbols: Vec<SymbolView>,
    pub extras: BTreeMap<String, String>,
}

impl InspectionResult {
    pub fn new(pc: u64) -> Self {
        Self {
            arch: None,
            int_regs: Vec::new(),
            pc,
            symbols: Vec::new(),
            extras: BTreeMap::new(),
        }
    }

    pub fn set_arch(&mut self, arch: impl Into<String>) {
        self.arch = Some(arch.into());
    }

    pub fn add_reg(&mut self, name: impl Into<String>, val: u64) {
        self.int_regs.push((name.into(), val));
    }

    pub fn add_symbol(&mut self, name: impl Into<String>, addr: u64, size: u64) {
        self.symbols.push(SymbolView {
            name: name.into(),
            addr,
            size,
        });
    }

    pub fn add_extra(&mut self, key: impl Into<String>, val: impl Into<String>) {
        self.extras.insert(key.into(), val.into());
    }

    pub fn to_text(&self) -> String {
        let mut out = String::new();
        if let Some(arch) = &self.arch {
            out.push_str(&format!("ARCH = {arch}\n"));
        }
        out.push_str(&format!("PC = {:#018x}\n", self.pc));
        for (name, val) in &self.int_regs {
            out.push_str(&format!("{name:>4} = {val:#018x}\n"));
        }
        for symbol in &self.symbols {
            out.push_str(&format!(
                "symbol {:#018x} {:#x} {}\n",
                symbol.addr, symbol.size, symbol.name
            ));
        }
        for (key, val) in &self.extras {
            out.push_str(&format!("{key}: {val}\n"));
        }
        out
    }
}

/// Trait for components that support inspection.
pub trait Inspectable {
    fn inspect(&self) -> InspectionResult;
    fn inspect_memory(&mut self, addr: u64, len: usize) -> Option<Vec<u8>>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formatting() {
        let mut r = InspectionResult::new(0x8000_0000);
        r.set_arch("aarch64");
        r.add_reg("x0", 42);
        r.add_symbol("_start", 0x8000_0000, 0x40);
        r.add_extra("NZCV", "0b0100");
        let text = r.to_text();
        assert!(text.contains("ARCH = aarch64"));
        assert!(text.contains("PC = 0x0000000080000000"));
        assert!(text.contains("symbol 0x0000000080000000 0x40 _start"));
        assert!(text.contains("NZCV: 0b0100"));
    }
}
