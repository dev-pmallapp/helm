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
    let addr_space = AddressSpace::new();
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
