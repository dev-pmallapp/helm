//! GICv3 LPI (Locality-specific Peripheral Interrupt) helpers.
//!
//! LPIs (INTIDs ≥ 8192) are message-based, edge-triggered interrupts
//! whose configuration and pending state live in memory tables pointed
//! to by `GICR_PROPBASER` and `GICR_PENDBASER`.  This module provides
//! software-side caches of those tables for use in simulation.

use super::common::LPI_START;

/// Decoded LPI configuration entry (1 byte in the hardware table).
#[derive(Debug, Clone, Copy)]
pub struct LpiConfig {
    /// Priority (bits \[7:2\] of the raw byte).
    pub priority: u8,
    /// Enable bit (bit 0).
    pub enabled: bool,
}

impl LpiConfig {
    /// Decode from a raw configuration byte.
    pub fn from_byte(byte: u8) -> Self {
        Self {
            priority: byte & 0xFC,
            enabled: byte & 1 != 0,
        }
    }

    /// Encode to a raw configuration byte.
    pub fn to_byte(self) -> u8 {
        (self.priority & 0xFC) | (self.enabled as u8)
    }
}

/// Software-side LPI pending table (mirrors `GICR_PENDBASER` content).
pub struct LpiPendingTable {
    bits: Vec<u32>,
    num_lpis: u32,
}

impl LpiPendingTable {
    /// Create a pending table supporting `num_lpis` LPIs (starting at
    /// INTID 8192).
    pub fn new(num_lpis: u32) -> Self {
        let words = num_lpis.div_ceil(32) as usize;
        Self {
            bits: vec![0; words],
            num_lpis,
        }
    }

    /// Set an LPI as pending.
    pub fn set_pending(&mut self, intid: u32) {
        if let Some(bit) = intid.checked_sub(LPI_START) {
            if bit < self.num_lpis {
                let idx = (bit / 32) as usize;
                let off = bit % 32;
                if idx < self.bits.len() {
                    self.bits[idx] |= 1 << off;
                }
            }
        }
    }

    /// Clear an LPI pending bit.
    pub fn clear_pending(&mut self, intid: u32) {
        if let Some(bit) = intid.checked_sub(LPI_START) {
            if bit < self.num_lpis {
                let idx = (bit / 32) as usize;
                let off = bit % 32;
                if idx < self.bits.len() {
                    self.bits[idx] &= !(1 << off);
                }
            }
        }
    }

    /// Check if an LPI is pending.
    pub fn is_pending(&self, intid: u32) -> bool {
        if let Some(bit) = intid.checked_sub(LPI_START) {
            if bit < self.num_lpis {
                let idx = (bit / 32) as usize;
                let off = bit % 32;
                return idx < self.bits.len() && self.bits[idx] & (1 << off) != 0;
            }
        }
        false
    }

    /// Clear all pending LPIs.
    pub fn clear_all(&mut self) {
        self.bits.iter_mut().for_each(|w| *w = 0);
    }
}

/// Software-side LPI configuration table (mirrors `GICR_PROPBASER`
/// content).
pub struct LpiConfigTable {
    configs: Vec<u8>,
    num_lpis: u32,
}

impl LpiConfigTable {
    /// Create a configuration table for `num_lpis` LPIs.
    pub fn new(num_lpis: u32) -> Self {
        Self {
            configs: vec![0; num_lpis as usize],
            num_lpis,
        }
    }

    /// Read configuration for an LPI.
    pub fn get(&self, intid: u32) -> Option<LpiConfig> {
        let idx = intid.checked_sub(LPI_START)?;
        self.configs
            .get(idx as usize)
            .map(|&b| LpiConfig::from_byte(b))
    }

    /// Write configuration for an LPI.
    pub fn set(&mut self, intid: u32, config: LpiConfig) {
        if let Some(idx) = intid.checked_sub(LPI_START) {
            if let Some(slot) = self.configs.get_mut(idx as usize) {
                *slot = config.to_byte();
            }
        }
    }

    /// Number of LPIs this table supports.
    pub fn num_lpis(&self) -> u32 {
        self.num_lpis
    }
}
