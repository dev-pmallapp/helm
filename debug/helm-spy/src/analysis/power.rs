//! Instruction-class energy estimation model.

use crate::events::InsnClass;
use std::collections::HashMap;

/// Energy cost in picojoules per instruction class.
#[derive(Debug, Clone)]
pub struct EnergyTable {
    costs: HashMap<InsnClass, f64>,
    default_cost: f64,
}

impl EnergyTable {
    pub fn new(default_pj: f64) -> Self {
        Self {
            costs: HashMap::new(),
            default_cost: default_pj,
        }
    }

    pub fn cortex_a55() -> Self {
        let mut t = Self::new(50.0);
        t.set(InsnClass::IntAlu, 30.0);
        t.set(InsnClass::IntMul, 80.0);
        t.set(InsnClass::Branch, 25.0);
        t.set(InsnClass::Load, 100.0);
        t.set(InsnClass::Store, 90.0);
        t.set(InsnClass::FpAlu, 120.0);
        t.set(InsnClass::SimdAlu, 150.0);
        t.set(InsnClass::System, 40.0);
        t.set(InsnClass::Nop, 10.0);
        t.set(InsnClass::Atomic, 200.0);
        t
    }

    pub fn set(&mut self, class: InsnClass, pj: f64) {
        self.costs.insert(class, pj);
    }
    pub fn cost(&self, class: InsnClass) -> f64 {
        self.costs.get(&class).copied().unwrap_or(self.default_cost)
    }
}

/// Power model that estimates energy from instruction mix.
pub struct PowerModel {
    table: EnergyTable,
    total_energy_pj: f64,
    total_insns: u64,
    class_counts: HashMap<InsnClass, u64>,
}

impl PowerModel {
    pub fn new(table: EnergyTable) -> Self {
        Self {
            table,
            total_energy_pj: 0.0,
            total_insns: 0,
            class_counts: HashMap::new(),
        }
    }

    pub fn cortex_a55() -> Self {
        Self::new(EnergyTable::cortex_a55())
    }

    pub fn on_insn(&mut self, class: InsnClass) {
        self.total_energy_pj += self.table.cost(class);
        self.total_insns += 1;
        *self.class_counts.entry(class).or_insert(0) += 1;
    }

    pub fn total_energy_pj(&self) -> f64 {
        self.total_energy_pj
    }
    pub fn total_energy_nj(&self) -> f64 {
        self.total_energy_pj / 1000.0
    }
    pub fn avg_energy_per_insn(&self) -> f64 {
        if self.total_insns == 0 {
            0.0
        } else {
            self.total_energy_pj / self.total_insns as f64
        }
    }
    pub fn total_insns(&self) -> u64 {
        self.total_insns
    }

    pub fn breakdown(&self) -> Vec<(InsnClass, u64, f64)> {
        let mut r: Vec<_> = self
            .class_counts
            .iter()
            .map(|(&c, &n)| (c, n, n as f64 * self.table.cost(c)))
            .collect();
        r.sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap_or(std::cmp::Ordering::Equal));
        r
    }

    pub fn reset(&mut self) {
        self.total_energy_pj = 0.0;
        self.total_insns = 0;
        self.class_counts.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accumulation() {
        let mut m = PowerModel::cortex_a55();
        m.on_insn(InsnClass::IntAlu);
        m.on_insn(InsnClass::Load);
        m.on_insn(InsnClass::Store);
        assert_eq!(m.total_insns(), 3);
        assert!((m.total_energy_pj() - 220.0).abs() < 1e-10);
    }

    #[test]
    fn reset_clears() {
        let mut m = PowerModel::cortex_a55();
        m.on_insn(InsnClass::IntAlu);
        m.reset();
        assert_eq!(m.total_insns(), 0);
    }
}
