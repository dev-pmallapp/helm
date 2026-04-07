//! DMA-facing adapters for the active address-space surface.
//!
//! The current runtime does not expose a shared `MemoryMap` implementation for
//! DMA-capable devices. Instead, Phase 3 work routes DMA through a shared
//! [`HelmAddressSpace`] so RAM and MMIO use the same live physical-memory
//! ownership model as the rest of the engine.

use std::sync::{Arc, Mutex};

use helm_core::{DmaPort, MemFault};

use crate::HelmAddressSpace;

/// Shared DMA view over the live physical address space.
#[derive(Clone)]
pub struct SharedDmaPort {
    sys_mem: Arc<Mutex<HelmAddressSpace>>,
}

impl SharedDmaPort {
    /// Wrap a shared address space for DMA-capable callers.
    pub fn new(sys_mem: Arc<Mutex<HelmAddressSpace>>) -> Self {
        Self { sys_mem }
    }

    /// Return the wrapped address space.
    pub fn address_space(&self) -> &Arc<Mutex<HelmAddressSpace>> {
        &self.sys_mem
    }
}

impl DmaPort for SharedDmaPort {
    fn dma_read(&self, addr: u64, buf: &mut [u8]) -> Result<(), MemFault> {
        let mut sys_mem = self
            .sys_mem
            .lock()
            .expect("shared DMA address space mutex poisoned");
        sys_mem.read_bytes(addr, buf)
    }

    fn dma_write(&self, addr: u64, buf: &[u8]) -> Result<(), MemFault> {
        let mut sys_mem = self
            .sys_mem
            .lock()
            .expect("shared DMA address space mutex poisoned");
        sys_mem.write_bytes(addr, buf)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use helm_devices::Device;

    use crate::FlatMem;

    struct ByteMirrorDevice {
        bytes: [u8; 8],
    }

    impl ByteMirrorDevice {
        fn new() -> Self {
            Self { bytes: [0; 8] }
        }
    }

    impl Device for ByteMirrorDevice {
        fn read(&mut self, offset: u64, size: usize) -> u64 {
            let start = offset as usize;
            let end = start + size;
            let mut buf = [0u8; 8];
            buf[..size].copy_from_slice(&self.bytes[start..end]);
            u64::from_le_bytes(buf)
        }

        fn write(&mut self, offset: u64, size: usize, val: u64) {
            let start = offset as usize;
            let end = start + size;
            self.bytes[start..end].copy_from_slice(&val.to_le_bytes()[..size]);
        }

        fn region_size(&self) -> u64 {
            self.bytes.len() as u64
        }
    }

    #[test]
    fn shared_dma_port_round_trips_ram() {
        let sys_mem = Arc::new(Mutex::new(HelmAddressSpace::new(FlatMem::new(
            0x4000_0000,
            0x1000,
        ))));
        let dma = SharedDmaPort::new(Arc::clone(&sys_mem));

        dma.dma_write(0x4000_0020, &[0xAA, 0xBB, 0xCC]).unwrap();

        let mut buf = [0u8; 3];
        dma.dma_read(0x4000_0020, &mut buf).unwrap();
        assert_eq!(buf, [0xAA, 0xBB, 0xCC]);
    }

    #[test]
    fn shared_dma_port_reaches_mmio_through_address_space() {
        let mut sys = HelmAddressSpace::new(FlatMem::new(0x4000_0000, 0x1000));
        let idx = sys.add_device(0x0900_0000, Box::new(ByteMirrorDevice::new()));
        let sys_mem = Arc::new(Mutex::new(sys));
        let dma = SharedDmaPort::new(Arc::clone(&sys_mem));

        dma.dma_write(0x0900_0001, &[0x11, 0x22, 0x33]).unwrap();

        let mut buf = [0u8; 3];
        dma.dma_read(0x0900_0001, &mut buf).unwrap();
        assert_eq!(buf, [0x11, 0x22, 0x33]);

        let mut sys = sys_mem.lock().unwrap();
        let dev = sys.device_as_mut::<ByteMirrorDevice>(idx).unwrap();
        assert_eq!(&dev.bytes[1..4], &[0x11, 0x22, 0x33]);
    }
}
