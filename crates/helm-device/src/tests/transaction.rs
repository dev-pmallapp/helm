use crate::transaction::*;

#[test]
fn read_transaction_defaults() {
    let txn = Transaction::read(0x1000, 4);
    assert_eq!(txn.addr, 0x1000);
    assert_eq!(txn.size, 4);
    assert!(!txn.is_write);
    assert_eq!(txn.stall_cycles, 0);
    assert_eq!(txn.data, [0u8; 16]);
}

#[test]
fn write_transaction_stores_value() {
    let txn = Transaction::write(0x2000, 4, 0xDEAD_BEEF);
    assert!(txn.is_write);
    assert_eq!(txn.data_u32(), 0xDEAD_BEEF);
    assert_eq!(txn.data_u64(), 0xDEAD_BEEF);
}

#[test]
fn data_u64_roundtrip() {
    let mut txn = Transaction::read(0, 8);
    txn.set_data_u64(0x1234_5678_9ABC_DEF0);
    assert_eq!(txn.data_u64(), 0x1234_5678_9ABC_DEF0);
}

#[test]
fn data_u32_roundtrip() {
    let mut txn = Transaction::read(0, 4);
    txn.set_data_u32(0xCAFE_BABE);
    assert_eq!(txn.data_u32(), 0xCAFE_BABE);
}

#[test]
fn write_bytes_copies_data() {
    let txn = Transaction::write_bytes(0x100, &[0x01, 0x02, 0x03, 0x04]);
    assert_eq!(txn.size, 4);
    assert_eq!(&txn.data[..4], &[0x01, 0x02, 0x03, 0x04]);
}

#[test]
fn default_attrs() {
    let attrs = TransactionAttrs::default();
    assert_eq!(attrs.initiator_id, 0);
    assert!(!attrs.secure);
    assert!(attrs.cacheable);
    assert!(!attrs.privileged);
}

#[test]
fn with_attrs_builder() {
    let txn = Transaction::read(0, 4).with_attrs(TransactionAttrs {
        initiator_id: 1,
        secure: true,
        cacheable: false,
        privileged: true,
    });
    assert_eq!(txn.attrs.initiator_id, 1);
    assert!(txn.attrs.secure);
}
