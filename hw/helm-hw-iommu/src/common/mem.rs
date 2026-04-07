//! Shared byte-memory contract re-export plus IOMMU test-memory helpers.

pub use helm_core::ByteMem;

/// Simple Vec-backed guest memory for testing.
#[cfg(test)]
pub struct TestMem {
    data: Vec<u8>,
}

#[cfg(test)]
impl TestMem {
    pub fn new(size: usize) -> Self {
        Self {
            data: vec![0u8; size],
        }
    }

    pub fn write_u64(&mut self, addr: u64, val: u64) {
        let a = addr as usize;
        self.data[a..a + 8].copy_from_slice(&val.to_le_bytes());
    }
}

#[cfg(test)]
impl ByteMem for TestMem {
    fn read_bytes(&mut self, pa: u64, buf: &mut [u8]) -> Result<(), helm_core::MemFault> {
        let a = pa as usize;
        if a + buf.len() > self.data.len() {
            return Err(helm_core::MemFault::AccessFault { addr: pa });
        }
        buf.copy_from_slice(&self.data[a..a + buf.len()]);
        Ok(())
    }

    fn write_bytes(&mut self, pa: u64, data: &[u8]) -> Result<(), helm_core::MemFault> {
        let a = pa as usize;
        if a + data.len() > self.data.len() {
            return Err(helm_core::MemFault::AccessFault { addr: pa });
        }
        self.data[a..a + data.len()].copy_from_slice(data);
        Ok(())
    }
}
