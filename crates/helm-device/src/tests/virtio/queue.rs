use crate::virtio::features::*;
use crate::virtio::queue::*;

// ── Split virtqueue tests ───────────────────────────────────────────────────

#[test]
fn split_queue_initial_state() {
    let q = SplitVirtqueue::new(256);
    assert_eq!(q.size, 256);
    assert!(!q.ready);
    assert!(!q.has_available());
    assert_eq!(q.num_available(), 0);
}

#[test]
fn split_queue_push_pop_avail() {
    let mut q = SplitVirtqueue::new(16);
    q.push_avail(0);
    q.push_avail(3);
    assert!(q.has_available());
    assert_eq!(q.num_available(), 2);
    assert_eq!(q.pop_avail(), Some(0));
    assert_eq!(q.pop_avail(), Some(3));
    assert_eq!(q.pop_avail(), None);
}

#[test]
fn split_queue_push_used() {
    let mut q = SplitVirtqueue::new(16);
    assert_eq!(q.used_idx, 0);
    q.push_used(5, 1024);
    assert_eq!(q.used_idx, 1);
    assert_eq!(q.used_ring[0].id, 5);
    assert_eq!(q.used_ring[0].len, 1024);
}

#[test]
fn split_queue_descriptor_chain() {
    let mut q = SplitVirtqueue::new(16);
    // desc[0] -> desc[1] -> desc[2]
    q.set_desc(0, 0x1000, 512, VRING_DESC_F_NEXT, 1);
    q.set_desc(1, 0x2000, 256, VRING_DESC_F_NEXT | VRING_DESC_F_WRITE, 2);
    q.set_desc(2, 0x3000, 128, VRING_DESC_F_WRITE, 0);

    let chain = q.walk_chain(0);
    assert_eq!(chain.len(), 3);
    assert_eq!(chain[0].addr, 0x1000);
    assert_eq!(chain[0].len, 512);
    assert!(chain[0].has_next());
    assert!(!chain[0].is_write());
    assert!(chain[1].is_write());
    assert!(chain[2].is_write());
    assert!(!chain[2].has_next());
}

#[test]
fn split_queue_wrapping() {
    let mut q = SplitVirtqueue::new(4);
    // Push 4 + pop 4 + push 2 more to test wrapping
    for i in 0..4u16 {
        q.push_avail(i);
    }
    for i in 0..4 {
        assert_eq!(q.pop_avail(), Some(i));
    }
    q.push_avail(10);
    q.push_avail(11);
    assert_eq!(q.pop_avail(), Some(10));
    assert_eq!(q.pop_avail(), Some(11));
}

#[test]
fn split_queue_reset() {
    let mut q = SplitVirtqueue::new(8);
    q.push_avail(0);
    q.push_used(0, 100);
    q.ready = true;
    q.reset();
    assert!(!q.ready);
    assert_eq!(q.avail_idx, 0);
    assert_eq!(q.used_idx, 0);
    assert!(!q.has_available());
}

#[test]
fn split_queue_set_desc_out_of_bounds() {
    let mut q = SplitVirtqueue::new(4);
    // Should not panic
    q.set_desc(100, 0, 0, 0, 0);
}

#[test]
fn vring_desc_flags() {
    let d = VringDesc {
        addr: 0,
        len: 0,
        flags: VRING_DESC_F_NEXT | VRING_DESC_F_WRITE,
        next: 1,
    };
    assert!(d.has_next());
    assert!(d.is_write());
    assert!(!d.is_indirect());

    let d2 = VringDesc {
        addr: 0,
        len: 0,
        flags: VRING_DESC_F_INDIRECT,
        next: 0,
    };
    assert!(d2.is_indirect());
    assert!(!d2.has_next());
}

// ── Packed virtqueue tests ──────────────────────────────────────────────────

#[test]
fn packed_queue_initial_state() {
    let q = PackedVirtqueue::new(64);
    assert_eq!(q.size, 64);
    assert!(!q.ready);
    assert!(q.device_wrap_counter);
}

#[test]
fn packed_queue_avail_detection() {
    let mut q = PackedVirtqueue::new(4);
    assert!(!q.desc_is_avail(0)); // Not available initially

    // Make descriptor 0 available
    q.desc_table[0].flags = VRING_PACKED_DESC_F_AVAIL;
    assert!(q.desc_is_avail(0));
}

#[test]
fn packed_queue_pop_avail() {
    let mut q = PackedVirtqueue::new(4);
    q.desc_table[0].flags = VRING_PACKED_DESC_F_AVAIL;
    q.desc_table[0].len = 512;
    q.desc_table[0].id = 0;

    assert_eq!(q.pop_avail(), Some(0));
    assert_eq!(q.pop_avail(), None); // No more available
}

#[test]
fn packed_queue_wrap_counter() {
    let mut q = PackedVirtqueue::new(2);
    // Make both descriptors available
    q.desc_table[0].flags = VRING_PACKED_DESC_F_AVAIL;
    q.desc_table[1].flags = VRING_PACKED_DESC_F_AVAIL;

    assert!(q.device_wrap_counter);
    q.pop_avail(); // idx 0
    q.pop_avail(); // idx 1 → wraps, counter flips
    assert!(!q.device_wrap_counter);
}

#[test]
fn packed_queue_reset() {
    let mut q = PackedVirtqueue::new(4);
    q.device_wrap_counter = false;
    q.device_next_off = 3;
    q.ready = true;
    q.reset();
    assert!(q.device_wrap_counter);
    assert_eq!(q.device_next_off, 0);
    assert!(!q.ready);
}

// ── Virtqueue enum tests ────────────────────────────────────────────────────

#[test]
fn virtqueue_enum_split() {
    let vq = Virtqueue::new_split(32);
    assert_eq!(vq.size(), 32);
    assert!(vq.as_split().is_some());
    assert!(vq.as_packed().is_none());
}

#[test]
fn virtqueue_enum_packed() {
    let vq = Virtqueue::new_packed(64);
    assert_eq!(vq.size(), 64);
    assert!(vq.as_packed().is_some());
    assert!(vq.as_split().is_none());
}

#[test]
fn virtqueue_set_ready() {
    let mut vq = Virtqueue::new_split(16);
    assert!(!vq.ready());
    vq.set_ready(true);
    assert!(vq.ready());
}

#[test]
fn virtqueue_reset() {
    let mut vq = Virtqueue::new_split(8);
    vq.set_ready(true);
    vq.reset();
    assert!(!vq.ready());
}
