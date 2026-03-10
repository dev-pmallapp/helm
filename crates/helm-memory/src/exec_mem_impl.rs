//! ExecMem implementation for AddressSpace.

use crate::address_space::AddressSpace;
use helm_core::exec_mem::ExecMem;
use helm_core::types::Addr;
use helm_core::HelmResult;

impl ExecMem for AddressSpace {
    #[inline]
    fn read_bytes(&mut self, addr: Addr, buf: &mut [u8]) -> HelmResult<()> {
        self.read(addr, buf)
    }

    #[inline]
    fn write_bytes(&mut self, addr: Addr, data: &[u8]) -> HelmResult<()> {
        self.write(addr, data)
    }

    fn read_phys(&self, addr: Addr, buf: &mut [u8]) -> HelmResult<()> {
        AddressSpace::read_phys(self, addr, buf)
    }

    fn host_ptr_for_pa(&self, pa: u64) -> Option<*mut u8> {
        AddressSpace::host_ptr_for_pa(self, pa)
    }
}
