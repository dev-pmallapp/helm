use crate::mem::{MemFault, MemFaultKind, MemoryAccess};
use crate::types::Addr;
use std::collections::HashMap;

struct MockMemory {
    data: HashMap<Addr, u8>,
}

impl MockMemory {
    fn new() -> Self {
        Self {
            data: HashMap::new(),
        }
    }
}

impl MemoryAccess for MockMemory {
    fn read(&mut self, addr: Addr, size: usize) -> Result<u64, MemFault> {
        let mut val = 0u64;
        for i in 0..size {
            let byte = self.data.get(&(addr + i as u64)).copied().unwrap_or(0);
            val |= (byte as u64) << (i * 8);
        }
        Ok(val)
    }

    fn write(&mut self, addr: Addr, size: usize, val: u64) -> Result<(), MemFault> {
        for i in 0..size {
            self.data.insert(addr + i as u64, (val >> (i * 8)) as u8);
        }
        Ok(())
    }

    fn fetch(&mut self, addr: Addr, buf: &mut [u8]) -> Result<(), MemFault> {
        for (i, b) in buf.iter_mut().enumerate() {
            *b = self.data.get(&(addr + i as u64)).copied().unwrap_or(0);
        }
        Ok(())
    }
}

#[test]
fn read_write_all_sizes() {
    let mut mem = MockMemory::new();
    for size in [1usize, 2, 4, 8] {
        let val: u64 = 0xDEAD_BEEF_CAFE_BABE >> (64 - size * 8);
        let addr = size as u64 * 0x100;
        mem.write(addr, size, val).unwrap();
        assert_eq!(mem.read(addr, size).unwrap(), val, "size={size}");
    }
}

#[test]
fn fetch_bytes() {
    let mut mem = MockMemory::new();
    mem.write(0x1000, 4, 0x11223344).unwrap();
    let mut buf = [0u8; 4];
    mem.fetch(0x1000, &mut buf).unwrap();
    assert_eq!(buf, [0x44, 0x33, 0x22, 0x11]);
}

#[test]
fn wide_read_write_default() {
    let mut mem = MockMemory::new();
    let data = [1u8, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16];
    mem.write_wide(0x2000, &data).unwrap();
    let mut out = [0u8; 16];
    mem.read_wide(0x2000, &mut out).unwrap();
    assert_eq!(out, data);
}

#[test]
fn copy_bulk_default() {
    let mut mem = MockMemory::new();
    for i in 0..16u8 {
        mem.write(0x1000 + i as u64, 1, i as u64).unwrap();
    }
    mem.copy_bulk(0x2000, 0x1000, 16).unwrap();
    for i in 0..16u8 {
        assert_eq!(mem.read(0x2000 + i as u64, 1).unwrap(), i as u64);
    }
}

#[test]
fn fill_bulk_default() {
    let mut mem = MockMemory::new();
    mem.fill_bulk(0x3000, 0xAB, 8).unwrap();
    for i in 0..8u64 {
        assert_eq!(mem.read(0x3000 + i, 1).unwrap(), 0xAB);
    }
}

#[test]
fn compare_exchange_success() {
    let mut mem = MockMemory::new();
    mem.write(0x4000, 8, 42).unwrap();
    let old = mem.compare_exchange(0x4000, 8, 42, 99).unwrap();
    assert_eq!(old, 42);
    assert_eq!(mem.read(0x4000, 8).unwrap(), 99);
}

#[test]
fn compare_exchange_failure() {
    let mut mem = MockMemory::new();
    mem.write(0x4000, 8, 42).unwrap();
    let old = mem.compare_exchange(0x4000, 8, 100, 99).unwrap();
    assert_eq!(old, 42);
    assert_eq!(mem.read(0x4000, 8).unwrap(), 42); // unchanged
}

#[test]
fn mem_fault_display() {
    let fault = MemFault {
        addr: 0xDEAD,
        is_write: true,
        kind: MemFaultKind::Unmapped,
    };
    let s = format!("{}", fault);
    assert!(s.contains("0xdead"));
    assert!(s.contains("Unmapped"));
}
