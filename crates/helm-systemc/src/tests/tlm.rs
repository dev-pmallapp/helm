use crate::tlm::*;

#[test]
fn read_transaction_has_zero_data() {
    let txn = TlmTransaction::read(0x1000, 4);
    assert_eq!(txn.address, 0x1000);
    assert_eq!(txn.data.len(), 4);
    assert_eq!(txn.command, TlmCommand::Read);
    assert!(!txn.is_ok()); // incomplete until processed
}

#[test]
fn write_transaction_carries_data() {
    let txn = TlmTransaction::write(0x2000, vec![0xDE, 0xAD]);
    assert_eq!(txn.command, TlmCommand::Write);
    assert_eq!(txn.data, vec![0xDE, 0xAD]);
}

#[test]
fn roundtrip_through_json() {
    let txn = TlmTransaction::read(0x3000, 8);
    let json = serde_json::to_string(&txn).unwrap();
    let back: TlmTransaction = serde_json::from_str(&json).unwrap();
    assert_eq!(back.address, 0x3000);
}
