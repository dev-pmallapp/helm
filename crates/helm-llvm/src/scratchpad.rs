//! Scratchpad memory for hardware accelerators
//!
//! Scratchpad memory provides fast, deterministic local storage for accelerators.
//! Unlike caches, scratchpads have predictable access latency and no coherence overhead.

use crate::error::{Error, Result};

/// Scratchpad memory configuration
#[derive(Debug, Clone)]
pub struct ScratchpadConfig {
    /// Size in bytes
    pub size: usize,
    /// Access latency in cycles
    pub access_latency: u32,
    /// Number of read/write ports
    pub ports: usize,
    /// Power per access (mW)
    pub power_per_access: f64,
}

impl Default for ScratchpadConfig {
    fn default() -> Self {
        Self {
            size: 65536,         // 64KB
            access_latency: 1,   // 1 cycle
            ports: 2,            // 2 ports (1 read, 1 write)
            power_per_access: 0.1,
        }
    }
}

/// Scratchpad memory device
///
/// Provides fast, deterministic local storage for hardware accelerators.
/// Common in FPGA and ASIC designs as an alternative to caches.
#[derive(Debug)]
pub struct ScratchpadMemory {
    config: ScratchpadConfig,
    /// Memory storage
    data: Vec<u8>,
    /// Port busy cycles
    port_busy: Vec<u32>,
    /// Access statistics
    reads: u64,
    writes: u64,
    total_energy: f64,
}

impl ScratchpadMemory {
    /// Create a new scratchpad memory
    pub fn new(config: ScratchpadConfig) -> Self {
        Self {
            data: vec![0; config.size],
            port_busy: vec![0; config.ports],
            reads: 0,
            writes: 0,
            total_energy: 0.0,
            config,
        }
    }

    /// Try to read from scratchpad
    pub fn try_read(&mut self, addr: usize, size: usize) -> Result<Vec<u8>> {
        // Check bounds
        if addr + size > self.config.size {
            return Err(Error::Other(format!(
                "Scratchpad access out of bounds: addr={}, size={}, capacity={}",
                addr, size, self.config.size
            )));
        }

        // Find available port
        for port in &mut self.port_busy {
            if *port == 0 {
                // Reserve port
                *port = self.config.access_latency;

                // Read data
                let data = self.data[addr..addr + size].to_vec();

                // Update statistics
                self.reads += 1;
                self.total_energy += self.config.power_per_access;

                return Ok(data);
            }
        }

        Err(Error::ResourceExhausted(
            "All scratchpad ports busy".to_string(),
        ))
    }

    /// Try to write to scratchpad
    pub fn try_write(&mut self, addr: usize, data: &[u8]) -> Result<()> {
        // Check bounds
        if addr + data.len() > self.config.size {
            return Err(Error::Other(format!(
                "Scratchpad access out of bounds: addr={}, size={}, capacity={}",
                addr,
                data.len(),
                self.config.size
            )));
        }

        // Find available port
        for port in &mut self.port_busy {
            if *port == 0 {
                // Reserve port
                *port = self.config.access_latency;

                // Write data
                self.data[addr..addr + data.len()].copy_from_slice(data);

                // Update statistics
                self.writes += 1;
                self.total_energy += self.config.power_per_access;

                return Ok(());
            }
        }

        Err(Error::ResourceExhausted(
            "All scratchpad ports busy".to_string(),
        ))
    }

    /// Advance one cycle
    pub fn tick(&mut self) {
        for port in &mut self.port_busy {
            if *port > 0 {
                *port -= 1;
            }
        }
    }

    /// Check if scratchpad has available port
    pub fn has_available_port(&self) -> bool {
        self.port_busy.iter().any(|&busy| busy == 0)
    }

    /// Get statistics
    pub fn stats(&self) -> ScratchpadStats {
        ScratchpadStats {
            reads: self.reads,
            writes: self.writes,
            total_energy: self.total_energy,
            utilization: self.calculate_utilization(),
        }
    }

    fn calculate_utilization(&self) -> f64 {
        let busy_ports = self.port_busy.iter().filter(|&&b| b > 0).count();
        busy_ports as f64 / self.config.ports as f64
    }

    /// Get configuration
    pub fn config(&self) -> &ScratchpadConfig {
        &self.config
    }

    /// Get size
    pub fn size(&self) -> usize {
        self.config.size
    }
}

/// Scratchpad memory statistics
#[derive(Debug, Clone)]
pub struct ScratchpadStats {
    pub reads: u64,
    pub writes: u64,
    pub total_energy: f64,
    pub utilization: f64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_scratchpad_basic() {
        let config = ScratchpadConfig::default();
        let mut sp = ScratchpadMemory::new(config);

        assert_eq!(sp.size(), 65536);
        assert!(sp.has_available_port());
    }

    #[test]
    fn test_scratchpad_read_write() {
        let config = ScratchpadConfig {
            ports: 1, // Use 1 port to test port busy logic
            ..Default::default()
        };
        let mut sp = ScratchpadMemory::new(config);

        // Write data
        let data = vec![1, 2, 3, 4];
        sp.try_write(0, &data).unwrap();

        // Port should be busy (only 1 port)
        assert!(!sp.has_available_port());

        // Tick to free port
        sp.tick();
        assert!(sp.has_available_port());

        // Read back
        let read_data = sp.try_read(0, 4).unwrap();
        assert_eq!(read_data, vec![1, 2, 3, 4]);
    }

    #[test]
    fn test_scratchpad_port_contention() {
        let config = ScratchpadConfig {
            ports: 1,
            ..Default::default()
        };
        let mut sp = ScratchpadMemory::new(config);

        // First access succeeds
        sp.try_write(0, &[1, 2, 3, 4]).unwrap();

        // Second access fails (port busy)
        let result = sp.try_write(4, &[5, 6, 7, 8]);
        assert!(result.is_err());
    }

    #[test]
    fn test_scratchpad_bounds_check() {
        let config = ScratchpadConfig {
            size: 1024,
            ..Default::default()
        };
        let mut sp = ScratchpadMemory::new(config);

        // Out of bounds write should fail
        let result = sp.try_write(1020, &[1, 2, 3, 4, 5]);
        assert!(result.is_err());
    }

    #[test]
    fn test_scratchpad_stats() {
        let config = ScratchpadConfig::default();
        let mut sp = ScratchpadMemory::new(config);

        sp.try_write(0, &[1, 2, 3, 4]).unwrap();
        sp.tick();
        sp.try_read(0, 4).unwrap();

        let stats = sp.stats();
        assert_eq!(stats.reads, 1);
        assert_eq!(stats.writes, 1);
        assert!(stats.total_energy > 0.0);
    }
}
