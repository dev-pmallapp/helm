//! Inspection API — dump arch state, memory ranges, device state on demand.

use std::collections::BTreeMap;

/// A snapshot of inspectable state.
#[derive(Debug, Clone)]
pub struct InspectionResult {
    pub int_regs: Vec<(String, u64)>,
    pub pc: u64,
    pub extras: BTreeMap<String, String>,
}

impl InspectionResult {
    pub fn new(pc: u64) -> Self {
        Self {
            int_regs: Vec::new(),
            pc,
            extras: BTreeMap::new(),
        }
    }

    pub fn add_reg(&mut self, name: impl Into<String>, val: u64) {
        self.int_regs.push((name.into(), val));
    }

    pub fn add_extra(&mut self, key: impl Into<String>, val: impl Into<String>) {
        self.extras.insert(key.into(), val.into());
    }

    pub fn to_text(&self) -> String {
        let mut out = format!("PC = {:#018x}\n", self.pc);
        for (name, val) in &self.int_regs {
            out.push_str(&format!("{name:>4} = {val:#018x}\n"));
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formatting() {
        let mut r = InspectionResult::new(0x8000_0000);
        r.add_reg("x0", 42);
        r.add_extra("NZCV", "0b0100");
        let text = r.to_text();
        assert!(text.contains("PC = 0x0000000080000000"));
        assert!(text.contains("NZCV: 0b0100"));
    }
}
