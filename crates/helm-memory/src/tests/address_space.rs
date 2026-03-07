use crate::address_space::*;

#[test]
fn write_then_read_returns_same_data() {
    let mut addr_space = AddressSpace::new();
    addr_space.map(0x1000, 256, (true, true, false));
    addr_space.write(0x1000, &[0xDE, 0xAD, 0xBE, 0xEF]).unwrap();

    let mut buf = [0u8; 4];
    addr_space.read(0x1000, &mut buf).unwrap();
    assert_eq!(buf, [0xDE, 0xAD, 0xBE, 0xEF]);
}

#[test]
fn read_unmapped_address_fails() {
    let mut addr_space = AddressSpace::new();
    let mut buf = [0u8; 4];
    let result = addr_space.read(0x9999, &mut buf);
    assert!(result.is_err());
}

#[test]
fn write_unmapped_address_fails() {
    let mut addr_space = AddressSpace::new();
    let result = addr_space.write(0x9999, &[1, 2, 3]);
    assert!(result.is_err());
}

#[test]
fn map_two_regions_no_overlap() {
    let mut addr_space = AddressSpace::new();
    addr_space.map(0x1000, 256, (true, true, false));
    addr_space.map(0x2000, 256, (true, true, false));
    addr_space.write(0x1000, &[0xAA]).unwrap();
    addr_space.write(0x2000, &[0xBB]).unwrap();
    let mut a = [0u8; 1];
    let mut b = [0u8; 1];
    addr_space.read(0x1000, &mut a).unwrap();
    addr_space.read(0x2000, &mut b).unwrap();
    assert_eq!(a[0], 0xAA);
    assert_eq!(b[0], 0xBB);
}

#[test]
fn address_space_default_constructs() {
    let mut addr_space = AddressSpace::default();
    assert!(addr_space.read(0, &mut [0u8; 1]).is_err());
}

#[test]
fn write_full_region_size() {
    let mut addr_space = AddressSpace::new();
    addr_space.map(0x0, 4, (true, true, false));
    let data = [1u8, 2, 3, 4];
    addr_space.write(0x0, &data).unwrap();
    let mut buf = [0u8; 4];
    addr_space.read(0x0, &mut buf).unwrap();
    assert_eq!(buf, data);
}

#[test]
fn read_across_region_boundary_fails() {
    let mut addr_space = AddressSpace::new();
    // No region mapped — any read fails
    let mut buf = [0u8; 8];
    assert!(addr_space.read(0xFFFF_FFF8, &mut buf).is_err());
}

#[test]
fn mapped_region_starts_as_zeros() {
    let mut addr_space = AddressSpace::new();
    addr_space.map(0x5000, 64, (true, false, false));
    let mut buf = [0xFFu8; 4];
    addr_space.read(0x5000, &mut buf).unwrap();
    assert_eq!(buf, [0u8; 4]);
}
