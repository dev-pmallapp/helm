//! FlatMemoryAccess — wraps AddressSpace as a MemoryAccess for SE mode.

use crate::address_space::AddressSpace;
use helm_core::mem::{MemFault, MemFaultKind, MemoryAccess};
use helm_core::types::Addr;

/// Flat (no MMU) memory access borrowing an AddressSpace.
pub struct FlatMemoryAccess<'a> {
    pub space: &'a mut AddressSpace,
}

impl MemoryAccess for FlatMemoryAccess<'_> {
    fn read(&mut self, addr: Addr, size: usize) -> Result<u64, MemFault> {
        let mut buf = [0u8; 8];
        self.space.read(addr, &mut buf[..size]).map_err(|_| MemFault {
            addr,
            is_write: false,
            kind: MemFaultKind::Unmapped,
        })?;
        Ok(u64::from_le_bytes(buf))
    }

    fn write(&mut self, addr: Addr, size: usize, val: u64) -> Result<(), MemFault> {
        let bytes = val.to_le_bytes();
        self.space.write(addr, &bytes[..size]).map_err(|_| MemFault {
            addr,
            is_write: true,
            kind: MemFaultKind::Unmapped,
        })
    }

    fn fetch(&mut self, addr: Addr, buf: &mut [u8]) -> Result<(), MemFault> {
        self.space.read(addr, buf).map_err(|_| MemFault {
            addr,
            is_write: false,
            kind: MemFaultKind::Unmapped,
        })
    }
}

/// Owning variant — takes ownership of the AddressSpace.
/// Use this when the MemoryAccess needs to live as `Box<dyn MemoryAccess>`.
pub struct OwnedFlatMemory {
    pub space: AddressSpace,
}

impl OwnedFlatMemory {
    pub fn new(space: AddressSpace) -> Self {
        Self { space }
    }
}

impl MemoryAccess for OwnedFlatMemory {
    fn read(&mut self, addr: Addr, size: usize) -> Result<u64, MemFault> {
        let mut buf = [0u8; 8];
        self.space.read(addr, &mut buf[..size]).map_err(|_| MemFault {
            addr,
            is_write: false,
            kind: MemFaultKind::Unmapped,
        })?;
        Ok(u64::from_le_bytes(buf))
    }

    fn write(&mut self, addr: Addr, size: usize, val: u64) -> Result<(), MemFault> {
        let bytes = val.to_le_bytes();
        self.space.write(addr, &bytes[..size]).map_err(|_| MemFault {
            addr,
            is_write: true,
            kind: MemFaultKind::Unmapped,
        })
    }

    fn fetch(&mut self, addr: Addr, buf: &mut [u8]) -> Result<(), MemFault> {
        self.space.read(addr, buf).map_err(|_| MemFault {
            addr,
            is_write: false,
            kind: MemFaultKind::Unmapped,
        })
    }
}
