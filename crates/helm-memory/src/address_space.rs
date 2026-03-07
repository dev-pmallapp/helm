//! Guest address space with optional I/O fallback for FS mode.

use helm_core::types::Addr;
use helm_core::HelmResult;

/// Memory region descriptor.
#[derive(Debug, Clone)]
pub struct MemRegion {
    pub base: Addr,
    pub size: u64,
    pub data: Vec<u8>,
    pub readable: bool,
    pub writable: bool,
    pub executable: bool,
}

/// I/O fallback handler for addresses not backed by RAM.
///
/// When a read/write misses all RAM regions, the AddressSpace calls
/// this handler. Used in FS mode to route device MMIO accesses.
pub trait IoHandler {
    /// Read `size` bytes from I/O address. Returns the value, or None
    /// if no device is mapped at this address.
    fn io_read(&mut self, addr: Addr, size: usize) -> Option<u64>;
    /// Write `size` bytes to I/O address. Returns true if handled.
    fn io_write(&mut self, addr: Addr, size: usize, value: u64) -> bool;
}

/// A flat address space with optional I/O fallback.
pub struct AddressSpace {
    regions: Vec<MemRegion>,
    /// Optional I/O handler for device MMIO in FS mode.
    io: Option<Box<dyn IoHandler>>,
}

impl Default for AddressSpace {
    fn default() -> Self {
        Self::new()
    }
}

impl AddressSpace {
    pub fn new() -> Self {
        Self {
            regions: Vec::new(),
            io: None,
        }
    }

    /// Set the I/O fallback handler for unmapped addresses.
    pub fn set_io_handler(&mut self, handler: Box<dyn IoHandler>) {
        self.io = Some(handler);
    }

    /// Map a new region.
    pub fn map(&mut self, base: Addr, size: u64, rwx: (bool, bool, bool)) {
        self.regions.push(MemRegion {
            base,
            size,
            data: vec![0u8; size as usize],
            readable: rwx.0,
            writable: rwx.1,
            executable: rwx.2,
        });
    }

    /// Read bytes from the address space.
    pub fn read(&mut self, addr: Addr, buf: &mut [u8]) -> HelmResult<()> {
        for region in &self.regions {
            if addr >= region.base && addr + buf.len() as u64 <= region.base + region.size {
                let offset = (addr - region.base) as usize;
                buf.copy_from_slice(&region.data[offset..offset + buf.len()]);
                return Ok(());
            }
        }
        // I/O fallback
        if let Some(ref mut io) = self.io {
            if let Some(val) = io.io_read(addr, buf.len()) {
                let bytes = val.to_le_bytes();
                buf.copy_from_slice(&bytes[..buf.len()]);
                return Ok(());
            }
        }
        Err(helm_core::HelmError::Memory {
            addr,
            reason: "unmapped address".into(),
        })
    }

    /// Write bytes into the address space.
    pub fn write(&mut self, addr: Addr, data: &[u8]) -> HelmResult<()> {
        for region in &mut self.regions {
            if addr >= region.base && addr + data.len() as u64 <= region.base + region.size {
                let offset = (addr - region.base) as usize;
                region.data[offset..offset + data.len()].copy_from_slice(data);
                return Ok(());
            }
        }
        // I/O fallback
        if let Some(ref mut io) = self.io {
            let mut val = 0u64;
            let bytes = val.to_le_bytes();
            let mut buf = [0u8; 8];
            buf[..data.len()].copy_from_slice(data);
            val = u64::from_le_bytes(buf);
            if io.io_write(addr, data.len(), val) {
                return Ok(());
            }
        }
        Err(helm_core::HelmError::Memory {
            addr,
            reason: "unmapped address".into(),
        })
    }
}
