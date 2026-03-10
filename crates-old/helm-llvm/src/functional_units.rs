//! Functional unit modeling for hardware accelerators
//!
//! This module provides configurable functional unit resources inspired by gem5-SALAM.
//! Functional units can be configured as limited or unlimited, pipelined or non-pipelined.

/// Functional unit types matching SALAM's categorization
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FunctionalUnitType {
    Counter,
    IntAdder,
    IntMultiplier,
    IntDivider,
    IntShifter,
    IntBit,
    FPSPAdder,      // Single precision FP adder
    FPDPAdder,      // Double precision FP adder
    FPSPMultiplier, // Single precision FP multiplier
    FPDPMultiplier, // Double precision FP multiplier
    FPSPDivider,    // Single precision FP divider
    FPDPDivider,    // Double precision FP divider
    Compare,
    GEP, // GetElementPtr address calculation
    Conversion,
    LoadStore,
    Branch,
}

/// Configuration for a functional unit
#[derive(Debug, Clone)]
pub struct FunctionalUnitConfig {
    /// Number of units (-1 = unlimited)
    pub count: i32,
    /// Latency in cycles
    pub latency: u32,
    /// Whether the unit is pipelined
    pub pipelined: bool,
}

impl Default for FunctionalUnitConfig {
    fn default() -> Self {
        Self {
            count: -1, // Unlimited by default
            latency: 1,
            pipelined: true,
        }
    }
}

/// A single functional unit instance
#[derive(Debug, Clone)]
pub struct FunctionalUnit {
    pub fu_type: FunctionalUnitType,
    pub latency: u32,
    pub pipelined: bool,
    /// Cycles remaining until this unit is free
    pub busy_cycles: u32,
    /// Pipeline stages (for pipelined units)
    pub pipeline_depth: u32,
}

impl FunctionalUnit {
    pub fn new(fu_type: FunctionalUnitType, latency: u32, pipelined: bool) -> Self {
        Self {
            fu_type,
            latency,
            pipelined,
            busy_cycles: 0,
            pipeline_depth: if pipelined { latency } else { 0 },
        }
    }

    /// Check if the unit is available
    pub fn is_available(&self) -> bool {
        if self.pipelined {
            true // Pipelined units always accept new operations
        } else {
            self.busy_cycles == 0
        }
    }

    /// Issue an operation to this unit
    pub fn issue(&mut self) {
        if self.pipelined {
            // Pipelined units accept every cycle
        } else {
            // Non-pipelined units block for full latency
            self.busy_cycles = self.latency;
        }
    }

    /// Advance one cycle
    pub fn tick(&mut self) {
        if self.busy_cycles > 0 {
            self.busy_cycles -= 1;
        }
    }
}

/// Pool of functional units with configurable resources
#[derive(Debug, Clone)]
pub struct FunctionalUnitPool {
    units: std::collections::HashMap<FunctionalUnitType, Vec<FunctionalUnit>>,
    configs: std::collections::HashMap<FunctionalUnitType, FunctionalUnitConfig>,
}

impl FunctionalUnitPool {
    /// Create a new functional unit pool with default unlimited resources
    pub fn new() -> Self {
        Self {
            units: std::collections::HashMap::new(),
            configs: std::collections::HashMap::new(),
        }
    }

    /// Configure a functional unit type
    pub fn configure(&mut self, fu_type: FunctionalUnitType, config: FunctionalUnitConfig) {
        self.configs.insert(fu_type, config.clone());

        // Create the units if count > 0
        if config.count > 0 {
            let units: Vec<FunctionalUnit> = (0..config.count)
                .map(|_| FunctionalUnit::new(fu_type, config.latency, config.pipelined))
                .collect();
            self.units.insert(fu_type, units);
        }
    }

    /// Try to allocate a functional unit for an operation
    pub fn try_allocate(&mut self, fu_type: FunctionalUnitType) -> bool {
        // Check if unlimited resources
        let config = self.configs.get(&fu_type);
        if config.is_none() || config.unwrap().count < 0 {
            return true; // Unlimited resources
        }

        // Try to find an available unit
        if let Some(units) = self.units.get_mut(&fu_type) {
            for unit in units.iter_mut() {
                if unit.is_available() {
                    unit.issue();
                    return true;
                }
            }
        }

        false // No units available
    }

    /// Advance all units by one cycle
    pub fn tick(&mut self) {
        for units in self.units.values_mut() {
            for unit in units.iter_mut() {
                unit.tick();
            }
        }
    }

    /// Get number of available units of a type
    pub fn available_count(&self, fu_type: FunctionalUnitType) -> i32 {
        let config = self.configs.get(&fu_type);
        if config.is_none() || config.unwrap().count < 0 {
            return -1; // Unlimited
        }

        if let Some(units) = self.units.get(&fu_type) {
            units.iter().filter(|u| u.is_available()).count() as i32
        } else {
            0
        }
    }
}

impl Default for FunctionalUnitPool {
    fn default() -> Self {
        Self::new()
    }
}

/// Builder for configuring functional units (gem5-SALAM style)
pub struct FunctionalUnitPoolBuilder {
    pool: FunctionalUnitPool,
}

impl FunctionalUnitPoolBuilder {
    pub fn new() -> Self {
        Self {
            pool: FunctionalUnitPool::new(),
        }
    }

    /// Set unlimited resources (SALAM default)
    pub fn unlimited(self) -> Self {
        self
    }

    /// Configure integer adders
    pub fn with_int_adders(mut self, count: i32, latency: u32, pipelined: bool) -> Self {
        self.pool.configure(
            FunctionalUnitType::IntAdder,
            FunctionalUnitConfig {
                count,
                latency,
                pipelined,
            },
        );
        self
    }

    /// Configure integer multipliers
    pub fn with_int_multipliers(mut self, count: i32, latency: u32, pipelined: bool) -> Self {
        self.pool.configure(
            FunctionalUnitType::IntMultiplier,
            FunctionalUnitConfig {
                count,
                latency,
                pipelined,
            },
        );
        self
    }

    /// Configure FP multipliers (single precision)
    pub fn with_fp_sp_multipliers(mut self, count: i32, latency: u32, pipelined: bool) -> Self {
        self.pool.configure(
            FunctionalUnitType::FPSPMultiplier,
            FunctionalUnitConfig {
                count,
                latency,
                pipelined,
            },
        );
        self
    }

    /// Configure FP multipliers (double precision)
    pub fn with_fp_dp_multipliers(mut self, count: i32, latency: u32, pipelined: bool) -> Self {
        self.pool.configure(
            FunctionalUnitType::FPDPMultiplier,
            FunctionalUnitConfig {
                count,
                latency,
                pipelined,
            },
        );
        self
    }

    /// Configure load/store units
    pub fn with_load_store_units(mut self, count: i32, latency: u32) -> Self {
        self.pool.configure(
            FunctionalUnitType::LoadStore,
            FunctionalUnitConfig {
                count,
                latency,
                pipelined: true, // LSUs are typically pipelined
            },
        );
        self
    }

    /// Build the functional unit pool
    pub fn build(self) -> FunctionalUnitPool {
        self.pool
    }
}

impl Default for FunctionalUnitPoolBuilder {
    fn default() -> Self {
        Self::new()
    }
}
