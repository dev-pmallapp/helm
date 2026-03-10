use crate::backend::*;

// ── CharBackend ─────────────────────────────────────────────────────────────

#[test]
fn buffer_char_write_and_drain() {
    let mut b = BufferCharBackend::new();
    b.write(b"hello").unwrap();
    assert_eq!(b.output_string(), "hello");
    let drained = b.output();
    assert_eq!(drained, b"hello");
    assert!(b.output_ref().is_empty());
}

#[test]
fn buffer_char_inject_and_read() {
    let mut b = BufferCharBackend::new();
    assert!(!b.can_read());
    b.inject(b"world");
    assert!(b.can_read());
    let mut buf = [0u8; 3];
    let n = b.read(&mut buf).unwrap();
    assert_eq!(n, 3);
    assert_eq!(&buf, b"wor");
    let n = b.read(&mut buf).unwrap();
    assert_eq!(n, 2);
    assert_eq!(&buf[..2], b"ld");
}

#[test]
fn null_char_discards_output() {
    let mut b = NullCharBackend;
    assert_eq!(b.write(b"discard me").unwrap(), 10);
    assert!(!b.can_read());
    let mut buf = [0u8; 4];
    assert_eq!(b.read(&mut buf).unwrap(), 0);
}

#[test]
fn buffer_char_backend_name() {
    let b = BufferCharBackend::new();
    assert_eq!(b.backend_name(), "buffer");
}

// ── BlockBackend ────────────────────────────────────────────────────────────

#[test]
fn memory_block_read_write() {
    let mut b = MemoryBlockBackend::new(1024);
    b.write_at(0, b"hello").unwrap();
    let mut buf = [0u8; 5];
    b.read_at(0, &mut buf).unwrap();
    assert_eq!(&buf, b"hello");
}

#[test]
fn memory_block_size() {
    let b = MemoryBlockBackend::new(4096);
    assert_eq!(b.size(), 4096);
}

#[test]
fn memory_block_readonly_rejects_write() {
    let mut b = MemoryBlockBackend::from_data(vec![0u8; 512]).readonly();
    assert!(b.is_readonly());
    assert!(b.write_at(0, b"no").is_err());
}

#[test]
fn memory_block_read_past_end() {
    let b = MemoryBlockBackend::new(8);
    let mut buf = [0u8; 16];
    let n = b.read_at(0, &mut buf).unwrap();
    assert_eq!(n, 8);
}

// ── NetBackend ──────────────────────────────────────────────────────────────

#[test]
fn buffer_net_send_recv() {
    let mut b = BufferNetBackend::new();
    b.send(b"packet1").unwrap();
    assert_eq!(b.drain_tx().len(), 1);
}

#[test]
fn buffer_net_inject_rx() {
    let mut b = BufferNetBackend::new();
    b.inject_rx(vec![1, 2, 3]);
    assert!(b.can_recv());
    let mut buf = [0u8; 10];
    let n = b.recv(&mut buf).unwrap();
    assert_eq!(n, 3);
    assert_eq!(&buf[..3], &[1, 2, 3]);
    assert!(!b.can_recv());
}

#[test]
fn null_net_discards() {
    let mut b = NullNetBackend;
    assert_eq!(b.send(b"drop").unwrap(), 4);
    assert!(!b.can_recv());
}
