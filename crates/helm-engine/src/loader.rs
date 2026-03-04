//! Minimal ELF/binary loader for SE mode.

use helm_core::types::Addr;
use helm_core::HelmResult;
use helm_memory::address_space::AddressSpace;

/// Load result: entry point and populated address space.
pub struct LoadedBinary {
    pub entry_point: Addr,
    pub address_space: AddressSpace,
}

/// Load a binary file into the guest address space (stub).
pub fn load_binary(path: &str) -> HelmResult<LoadedBinary> {
    let _data = std::fs::read(path)?;
    let mut address_space = AddressSpace::new();

    // Stub: map a region and copy the raw bytes.
    // A real implementation would parse ELF headers.
    let base: Addr = 0x40_0000;
    let size = _data.len() as u64;
    address_space.map(base, size, (true, false, true));
    address_space.write(base, &_data)?;

    // Map a stack region.
    let stack_base: Addr = 0x7FFF_0000;
    let stack_size: u64 = 0x10000;
    address_space.map(stack_base, stack_size, (true, true, false));

    Ok(LoadedBinary {
        entry_point: base,
        address_space,
    })
}
