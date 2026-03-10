use crate::pci::Bdf;

// ── Display format ────────────────────────────────────────────────────────────

#[test]
fn display_zeros() {
    let bdf = Bdf {
        bus: 0,
        device: 0,
        function: 0,
    };
    assert_eq!(format!("{bdf}"), "00:00.0");
}

#[test]
fn display_max() {
    let bdf = Bdf {
        bus: 0xFF,
        device: 0x1F,
        function: 0x7,
    };
    assert_eq!(format!("{bdf}"), "ff:1f.7");
}

#[test]
fn display_example() {
    let bdf = Bdf {
        bus: 0,
        device: 0x1F,
        function: 7,
    };
    assert_eq!(format!("{bdf}"), "00:1f.7");
}

#[test]
fn display_mid_values() {
    let bdf = Bdf {
        bus: 1,
        device: 2,
        function: 3,
    };
    assert_eq!(format!("{bdf}"), "01:02.3");
}

// ── ECAM round-trip ───────────────────────────────────────────────────────────

#[test]
fn ecam_round_trip_zero() {
    let bdf = Bdf {
        bus: 0,
        device: 0,
        function: 0,
    };
    let off = bdf.ecam_offset(0);
    let (back, reg) = Bdf::from_ecam_offset(off);
    assert_eq!(back, bdf);
    assert_eq!(reg, 0);
}

#[test]
fn ecam_round_trip_all_fields() {
    let bdf = Bdf {
        bus: 1,
        device: 2,
        function: 3,
    };
    let off = bdf.ecam_offset(0x14);
    let (back, reg) = Bdf::from_ecam_offset(off);
    assert_eq!(back, bdf);
    assert_eq!(reg, 0x14);
}

#[test]
fn ecam_round_trip_max() {
    let bdf = Bdf {
        bus: 0xFF,
        device: 0x1F,
        function: 0x7,
    };
    let off = bdf.ecam_offset(0xFFC);
    let (back, reg) = Bdf::from_ecam_offset(off);
    assert_eq!(back, bdf);
    assert_eq!(reg, 0xFFC);
}

#[test]
fn from_ecam_offset_known_value() {
    // bus=1, dev=2, fn=3, reg=0x10
    let ecam = (1u64 << 20) | (2u64 << 15) | (3u64 << 12) | 0x10;
    let (bdf, reg) = Bdf::from_ecam_offset(ecam);
    assert_eq!(bdf.bus, 1);
    assert_eq!(bdf.device, 2);
    assert_eq!(bdf.function, 3);
    assert_eq!(reg, 0x10);
}

#[test]
fn ecam_offset_encodes_bus_dev_fn() {
    let bdf = Bdf {
        bus: 5,
        device: 7,
        function: 2,
    };
    let off = bdf.ecam_offset(0);
    assert_eq!((off >> 20) & 0xFF, 5);
    assert_eq!((off >> 15) & 0x1F, 7);
    assert_eq!((off >> 12) & 0x07, 2);
}

#[test]
fn reg_bits_are_12_bit_mask() {
    let bdf = Bdf {
        bus: 0,
        device: 0,
        function: 0,
    };
    // reg >= 0x1000 should be masked to 12 bits
    let off = bdf.ecam_offset(0x1ABC);
    let (_, reg) = Bdf::from_ecam_offset(off);
    assert_eq!(reg, 0xABC);
}

// ── Derive traits ─────────────────────────────────────────────────────────────

#[test]
fn hash_and_eq() {
    use std::collections::HashSet;
    let a = Bdf {
        bus: 0,
        device: 1,
        function: 2,
    };
    let b = Bdf {
        bus: 0,
        device: 1,
        function: 2,
    };
    let c = Bdf {
        bus: 1,
        device: 0,
        function: 0,
    };
    let mut set = HashSet::new();
    set.insert(a);
    assert!(set.contains(&b));
    assert!(!set.contains(&c));
}

#[test]
fn copy_semantics() {
    let a = Bdf {
        bus: 1,
        device: 2,
        function: 3,
    };
    let b = a;
    assert_eq!(a, b);
}

#[test]
fn new_clamps_device_and_function() {
    let bdf = Bdf::new(0, 0xFF, 0xFF);
    assert_eq!(bdf.device, 0x1F);
    assert_eq!(bdf.function, 0x07);
}
