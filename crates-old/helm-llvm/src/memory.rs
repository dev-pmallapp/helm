//! Memory backend interface for LLVM accelerators
//!
//! This module provides the interface between LLVM IR load/store operations
//! and the underlying memory system (helm-memory).

use crate::error::{Error, Result};
use crate::micro_op::MemSize;

/// Memory backend trait
///
/// Implementations connect to helm-memory, scratchpad, or other memory systems.
pub trait MemoryBackend: Send + Sync {
    /// Read from memory
    fn read(&mut self, addr: u64, size: MemSize) -> Result<Vec<u8>>;

    /// Write to memory
    fn write(&mut self, addr: u64, data: &[u8]) -> Result<()>;

    /// Advance one cycle
    fn tick(&mut self);

    /// Check if a memory operation can be issued
    fn can_issue_load(&self) -> bool;
    fn can_issue_store(&self) -> bool;
}

/// Simple memory backend for testing/standalone use
#[derive(Debug)]
pub struct SimpleMemory {
    data: Vec<u8>,
    load_latency: u32,
    store_latency: u32,
    pending_loads: u32,
    pending_stores: u32,
}

impl SimpleMemory {
    pub fn new(size: usize) -> Self {
        Self {
            data: vec![0; size],
            load_latency: 2,
            store_latency: 1,
            pending_loads: 0,
            pending_stores: 0,
        }
    }

    pub fn with_latency(mut self, load: u32, store: u32) -> Self {
        self.load_latency = load;
        self.store_latency = store;
        self
    }
}

impl MemoryBackend for SimpleMemory {
    fn read(&mut self, addr: u64, size: MemSize) -> Result<Vec<u8>> {
        let bytes = match size {
            MemSize::Byte => 1,
            MemSize::HalfWord => 2,
            MemSize::Word => 4,
            MemSize::DoubleWord => 8,
        };

        let addr = addr as usize;
        if addr + bytes > self.data.len() {
            return Err(Error::Other(format!(
                "Memory read out of bounds: addr={:#x}, size={}",
                addr, bytes
            )));
        }

        self.pending_loads = self.load_latency;
        Ok(self.data[addr..addr + bytes].to_vec())
    }

    fn write(&mut self, addr: u64, data: &[u8]) -> Result<()> {
        let addr = addr as usize;
        if addr + data.len() > self.data.len() {
            return Err(Error::Other(format!(
                "Memory write out of bounds: addr={:#x}, size={}",
                addr,
                data.len()
            )));
        }

        self.data[addr..addr + data.len()].copy_from_slice(data);
        self.pending_stores = self.store_latency;
        Ok(())
    }

    fn tick(&mut self) {
        if self.pending_loads > 0 {
            self.pending_loads -= 1;
        }
        if self.pending_stores > 0 {
            self.pending_stores -= 1;
        }
    }

    fn can_issue_load(&self) -> bool {
        self.pending_loads == 0
    }

    fn can_issue_store(&self) -> bool {
        self.pending_stores == 0
    }
}

/// Memory backend that combines scratchpad and main memory
pub struct HybridMemory {
    scratchpad: crate::scratchpad::ScratchpadMemory,
    scratchpad_base: u64,
    main_memory: Box<dyn MemoryBackend>,
}

impl HybridMemory {
    pub fn new(
        scratchpad: crate::scratchpad::ScratchpadMemory,
        scratchpad_base: u64,
        main_memory: Box<dyn MemoryBackend>,
    ) -> Self {
        Self {
            scratchpad,
            scratchpad_base,
            main_memory,
        }
    }

    fn in_scratchpad(&self, addr: u64) -> bool {
        let scratchpad_end = self.scratchpad_base + self.scratchpad.size() as u64;
        addr >= self.scratchpad_base && addr < scratchpad_end
    }
}

impl MemoryBackend for HybridMemory {
    fn read(&mut self, addr: u64, size: MemSize) -> Result<Vec<u8>> {
        if self.in_scratchpad(addr) {
            let offset = (addr - self.scratchpad_base) as usize;
            let bytes = match size {
                MemSize::Byte => 1,
                MemSize::HalfWord => 2,
                MemSize::Word => 4,
                MemSize::DoubleWord => 8,
            };
            self.scratchpad.try_read(offset, bytes)
        } else {
            self.main_memory.read(addr, size)
        }
    }

    fn write(&mut self, addr: u64, data: &[u8]) -> Result<()> {
        if self.in_scratchpad(addr) {
            let offset = (addr - self.scratchpad_base) as usize;
            self.scratchpad.try_write(offset, data)
        } else {
            self.main_memory.write(addr, data)
        }
    }

    fn tick(&mut self) {
        self.scratchpad.tick();
        self.main_memory.tick();
    }

    fn can_issue_load(&self) -> bool {
        self.scratchpad.has_available_port() && self.main_memory.can_issue_load()
    }

    fn can_issue_store(&self) -> bool {
        self.scratchpad.has_available_port() && self.main_memory.can_issue_store()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_simple_memory() {
        let mut mem = SimpleMemory::new(1024);

        // Write and read
        mem.write(0, &[1, 2, 3, 4]).unwrap();
        let data = mem.read(0, MemSize::Word).unwrap();
        assert_eq!(data, vec![1, 2, 3, 4]);
    }

    #[test]
    fn test_memory_latency() {
        let mut mem = SimpleMemory::new(1024).with_latency(2, 1);

        assert!(mem.can_issue_load());
        mem.read(0, MemSize::Word).unwrap();
        assert!(!mem.can_issue_load());

        mem.tick();
        assert!(!mem.can_issue_load());
        mem.tick();
        assert!(mem.can_issue_load());
    }

    #[test]
    fn test_hybrid_memory() {
        use crate::scratchpad::{ScratchpadConfig, ScratchpadMemory};

        let sp_config = ScratchpadConfig {
            size: 1024,
            ..Default::default()
        };
        let scratchpad = ScratchpadMemory::new(sp_config);
        let main_mem = Box::new(SimpleMemory::new(65536));

        let mut hybrid = HybridMemory::new(scratchpad, 0x1000, main_mem);

        // Write to scratchpad region
        hybrid.write(0x1000, &[1, 2, 3, 4]).unwrap();
        let data = hybrid.read(0x1000, MemSize::Word).unwrap();
        assert_eq!(data, vec![1, 2, 3, 4]);

        // Write to main memory region
        hybrid.write(0x2000, &[5, 6, 7, 8]).unwrap();
        let data = hybrid.read(0x2000, MemSize::Word).unwrap();
        assert_eq!(data, vec![5, 6, 7, 8]);
    }
}
