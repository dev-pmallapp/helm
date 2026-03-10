use crate::memory::*;
use crate::micro_op::MemSize;

#[test]
fn simple_memory_new_is_zeroed() {
    let mut mem = SimpleMemory::new(64);
    let data = mem.read(0, MemSize::Word).unwrap();
    assert_eq!(data, vec![0, 0, 0, 0]);
}

#[test]
fn simple_memory_write_read_roundtrip() {
    let mut mem = SimpleMemory::new(64);
    mem.write(0, &[0xAA, 0xBB, 0xCC, 0xDD]).unwrap();
    let data = mem.read(0, MemSize::Word).unwrap();
    assert_eq!(data, vec![0xAA, 0xBB, 0xCC, 0xDD]);
}

#[test]
fn simple_memory_read_out_of_bounds_fails() {
    let mut mem = SimpleMemory::new(4);
    let result = mem.read(4, MemSize::Byte);
    assert!(result.is_err());
}

#[test]
fn simple_memory_write_out_of_bounds_fails() {
    let mut mem = SimpleMemory::new(4);
    let result = mem.write(4, &[1]);
    assert!(result.is_err());
}

#[test]
fn simple_memory_with_latency_sets_latencies() {
    let mut mem = SimpleMemory::new(64).with_latency(3, 2);
    assert!(mem.can_issue_load());
    assert!(mem.can_issue_store());

    mem.read(0, MemSize::Byte).unwrap();
    assert!(!mem.can_issue_load());

    mem.tick();
    mem.tick();
    assert!(!mem.can_issue_load());
    mem.tick();
    assert!(mem.can_issue_load());
}

#[test]
fn simple_memory_store_latency() {
    let mut mem = SimpleMemory::new(64).with_latency(1, 2);
    assert!(mem.can_issue_store());
    mem.write(0, &[1]).unwrap();
    assert!(!mem.can_issue_store());
    mem.tick();
    assert!(!mem.can_issue_store());
    mem.tick();
    assert!(mem.can_issue_store());
}

#[test]
fn simple_memory_read_byte_returns_one_byte() {
    let mut mem = SimpleMemory::new(64);
    mem.write(0, &[0xFF]).unwrap();
    let data = mem.read(0, MemSize::Byte).unwrap();
    assert_eq!(data.len(), 1);
    assert_eq!(data[0], 0xFF);
}

#[test]
fn simple_memory_read_double_word() {
    let mut mem = SimpleMemory::new(64);
    mem.write(0, &[1, 2, 3, 4, 5, 6, 7, 8]).unwrap();
    let data = mem.read(0, MemSize::DoubleWord).unwrap();
    assert_eq!(data.len(), 8);
    assert_eq!(data, vec![1, 2, 3, 4, 5, 6, 7, 8]);
}

#[test]
fn simple_memory_read_half_word() {
    let mut mem = SimpleMemory::new(64);
    mem.write(0, &[0xAB, 0xCD]).unwrap();
    let data = mem.read(0, MemSize::HalfWord).unwrap();
    assert_eq!(data.len(), 2);
    assert_eq!(data, vec![0xAB, 0xCD]);
}

#[test]
fn hybrid_memory_scratchpad_region() {
    use crate::scratchpad::{ScratchpadConfig, ScratchpadMemory};

    let sp = ScratchpadMemory::new(ScratchpadConfig {
        size: 256,
        ..Default::default()
    });
    let main = Box::new(SimpleMemory::new(1024));
    let mut hybrid = HybridMemory::new(sp, 0x1000, main);

    hybrid.write(0x1000, &[1, 2, 3, 4]).unwrap();
    let data = hybrid.read(0x1000, MemSize::Word).unwrap();
    assert_eq!(data, vec![1, 2, 3, 4]);
}

#[test]
fn hybrid_memory_main_region() {
    use crate::scratchpad::{ScratchpadConfig, ScratchpadMemory};

    let sp = ScratchpadMemory::new(ScratchpadConfig {
        size: 256,
        ..Default::default()
    });
    let main = Box::new(SimpleMemory::new(1024));
    let mut hybrid = HybridMemory::new(sp, 0x1000, main);

    hybrid.write(0x0, &[5, 6, 7, 8]).unwrap();
    let data = hybrid.read(0x0, MemSize::Word).unwrap();
    assert_eq!(data, vec![5, 6, 7, 8]);
}

#[test]
fn hybrid_memory_tick_advances_both() {
    use crate::scratchpad::{ScratchpadConfig, ScratchpadMemory};

    let sp = ScratchpadMemory::new(ScratchpadConfig {
        size: 256,
        ..Default::default()
    });
    let main = Box::new(SimpleMemory::new(1024));
    let mut hybrid = HybridMemory::new(sp, 0x1000, main);
    hybrid.tick();
}
