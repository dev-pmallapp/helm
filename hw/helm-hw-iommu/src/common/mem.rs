//! Guest physical memory access trait for IOMMU table walks.

/// Trait for reading/writing guest physical memory (used by table walks).
///
/// Decouples IOMMU implementations from `FlatMem` so tests can use a
/// simple `Vec<u8>`.
pub trait GuestMem {
    /// Read `size` bytes at guest physical address `pa`. Returns LE u64.
    fn guest_read(&self, pa: u64, size: usize) -> u64;
    /// Write `size` bytes at guest physical address `pa`. Value is LE u64.
    fn guest_write(&mut self, pa: u64, size: usize, val: u64);
}

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
impl GuestMem for TestMem {
    fn guest_read(&self, pa: u64, size: usize) -> u64 {
        let a = pa as usize;
        if a + size > self.data.len() {
            return 0;
        }
        let mut buf = [0u8; 8];
        let n = size.min(8);
        buf[..n].copy_from_slice(&self.data[a..a + n]);
        u64::from_le_bytes(buf)
    }

    fn guest_write(&mut self, pa: u64, size: usize, val: u64) {
        let a = pa as usize;
        if a + size > self.data.len() {
            return;
        }
        let bytes = val.to_le_bytes();
        let n = size.min(8);
        self.data[a..a + n].copy_from_slice(&bytes[..n]);
    }
}
