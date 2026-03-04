//! Clock-domain conversion utilities.

/// Describes a clock domain with a fixed frequency.
#[derive(Debug, Clone)]
pub struct ClockDomain {
    pub name: String,
    pub frequency_hz: u64,
}

impl ClockDomain {
    pub fn new(name: impl Into<String>, frequency_hz: u64) -> Self {
        Self {
            name: name.into(),
            frequency_hz,
        }
    }

    /// Convert cycles in this domain to nanoseconds.
    pub fn cycles_to_ns(&self, cycles: u64) -> f64 {
        (cycles as f64 / self.frequency_hz as f64) * 1e9
    }

    /// Convert nanoseconds to cycles in this domain.
    pub fn ns_to_cycles(&self, ns: f64) -> u64 {
        ((ns / 1e9) * self.frequency_hz as f64) as u64
    }

    /// Convert cycles from this domain to another domain.
    pub fn convert_to(&self, cycles: u64, target: &ClockDomain) -> u64 {
        let ns = self.cycles_to_ns(cycles);
        target.ns_to_cycles(ns)
    }
}
