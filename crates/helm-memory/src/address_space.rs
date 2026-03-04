//! Flat guest address space for syscall-emulation mode.

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

/// A simple flat address space used in SE mode.
pub struct AddressSpace {
    regions: Vec<MemRegion>,
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
        }
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
    pub fn read(&self, addr: Addr, buf: &mut [u8]) -> HelmResult<()> {
        for region in &self.regions {
            if addr >= region.base && addr + buf.len() as u64 <= region.base + region.size {
                let offset = (addr - region.base) as usize;
                buf.copy_from_slice(&region.data[offset..offset + buf.len()]);
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
        Err(helm_core::HelmError::Memory {
            addr,
            reason: "unmapped address".into(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_then_read_returns_same_data() {
        let mut addr_space = AddressSpace::new();
        addr_space.map(0x1000, 256, (true, true, false));
        addr_space.write(0x1000, &[0xDE, 0xAD, 0xBE, 0xEF]).unwrap();

        let mut buf = [0u8; 4];
        addr_space.read(0x1000, &mut buf).unwrap();
        assert_eq!(buf, [0xDE, 0xAD, 0xBE, 0xEF]);
    }

    #[test]
    fn read_unmapped_address_fails() {
        let addr_space = AddressSpace::new();
        let mut buf = [0u8; 4];
        let result = addr_space.read(0x9999, &mut buf);
        assert!(result.is_err());
    }

    #[test]
    fn write_unmapped_address_fails() {
        let mut addr_space = AddressSpace::new();
        let result = addr_space.write(0x9999, &[1, 2, 3]);
        assert!(result.is_err());
    }
}
