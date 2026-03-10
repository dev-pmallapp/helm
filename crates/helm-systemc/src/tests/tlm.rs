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

// --- address field ---

#[test]
fn read_stores_address() {
    let txn = TlmTransaction::read(0xDEAD_BEEF, 1);
    assert_eq!(txn.address, 0xDEAD_BEEF);
}

#[test]
fn write_stores_address() {
    let txn = TlmTransaction::write(0x0000_1234, vec![0xAB]);
    assert_eq!(txn.address, 0x0000_1234);
}

#[test]
fn read_address_zero_is_valid() {
    let txn = TlmTransaction::read(0, 4);
    assert_eq!(txn.address, 0);
}

// --- data field ---

#[test]
fn read_data_buffer_is_all_zeros() {
    let txn = TlmTransaction::read(0, 8);
    assert!(txn.data.iter().all(|&b| b == 0));
}

#[test]
fn read_data_length_matches_request() {
    for len in [1usize, 2, 4, 8, 16, 64] {
        let txn = TlmTransaction::read(0, len);
        assert_eq!(txn.data.len(), len, "length mismatch for {len}");
    }
}

#[test]
fn write_data_preserved_exactly() {
    let payload = vec![0x01, 0x02, 0x03, 0x04];
    let txn = TlmTransaction::write(0, payload.clone());
    assert_eq!(txn.data, payload);
}

#[test]
fn write_empty_data_is_accepted() {
    let txn = TlmTransaction::write(0x100, vec![]);
    assert_eq!(txn.data.len(), 0);
}

// --- streaming_width field ---

#[test]
fn read_streaming_width_equals_length() {
    let txn = TlmTransaction::read(0, 16);
    assert_eq!(txn.streaming_width, 16);
}

#[test]
fn write_streaming_width_equals_data_length() {
    let txn = TlmTransaction::write(0, vec![0u8; 32]);
    assert_eq!(txn.streaming_width, 32);
}

// --- byte_enables default ---

#[test]
fn read_byte_enables_is_none_by_default() {
    let txn = TlmTransaction::read(0, 4);
    assert!(txn.byte_enables.is_none());
}

#[test]
fn write_byte_enables_is_none_by_default() {
    let txn = TlmTransaction::write(0, vec![0xAA]);
    assert!(txn.byte_enables.is_none());
}

// --- initial response is IncompleteResponse ---

#[test]
fn read_initial_response_is_incomplete() {
    let txn = TlmTransaction::read(0, 4);
    assert_eq!(txn.response, TlmResponse::IncompleteResponse);
}

#[test]
fn write_initial_response_is_incomplete() {
    let txn = TlmTransaction::write(0, vec![0]);
    assert_eq!(txn.response, TlmResponse::IncompleteResponse);
}

#[test]
fn initial_delay_is_zero() {
    let r = TlmTransaction::read(0, 4);
    let w = TlmTransaction::write(0, vec![0]);
    assert_eq!(r.delay_ns, 0.0);
    assert_eq!(w.delay_ns, 0.0);
}

// --- is_ok reflects response field ---

#[test]
fn is_ok_true_when_response_ok() {
    let mut txn = TlmTransaction::read(0, 4);
    txn.response = TlmResponse::Ok;
    assert!(txn.is_ok());
}

#[test]
fn is_ok_false_for_generic_error() {
    let mut txn = TlmTransaction::read(0, 4);
    txn.response = TlmResponse::GenericError;
    assert!(!txn.is_ok());
}

#[test]
fn is_ok_false_for_address_error() {
    let mut txn = TlmTransaction::read(0, 4);
    txn.response = TlmResponse::AddressError;
    assert!(!txn.is_ok());
}

#[test]
fn is_ok_false_for_command_error() {
    let mut txn = TlmTransaction::read(0, 4);
    txn.response = TlmResponse::CommandError;
    assert!(!txn.is_ok());
}

#[test]
fn is_ok_false_for_burst_error() {
    let mut txn = TlmTransaction::read(0, 4);
    txn.response = TlmResponse::BurstError;
    assert!(!txn.is_ok());
}

#[test]
fn is_ok_false_for_byte_enable_error() {
    let mut txn = TlmTransaction::read(0, 4);
    txn.response = TlmResponse::ByteEnableError;
    assert!(!txn.is_ok());
}

// --- command equality ---

#[test]
fn tlm_command_read_ne_write() {
    assert_ne!(TlmCommand::Read, TlmCommand::Write);
}

#[test]
fn tlm_command_ignore_ne_read() {
    assert_ne!(TlmCommand::Ignore, TlmCommand::Read);
}

// --- response equality ---

#[test]
fn tlm_response_ok_ne_error() {
    assert_ne!(TlmResponse::Ok, TlmResponse::GenericError);
}

// --- clone ---

#[test]
fn tlm_transaction_clone_is_independent() {
    let original = TlmTransaction::write(0x8000, vec![0xCA, 0xFE]);
    let mut cloned = original.clone();
    cloned.response = TlmResponse::Ok;
    // Original must not be mutated through the clone.
    assert_eq!(original.response, TlmResponse::IncompleteResponse);
}

// --- JSON roundtrip preserves all fields ---

#[test]
fn json_roundtrip_preserves_command() {
    let txn = TlmTransaction::write(0, vec![1, 2, 3]);
    let back: TlmTransaction = serde_json::from_str(&serde_json::to_string(&txn).unwrap()).unwrap();
    assert_eq!(back.command, TlmCommand::Write);
}

#[test]
fn json_roundtrip_preserves_response() {
    let mut txn = TlmTransaction::read(0, 4);
    txn.response = TlmResponse::AddressError;
    let back: TlmTransaction = serde_json::from_str(&serde_json::to_string(&txn).unwrap()).unwrap();
    assert_eq!(back.response, TlmResponse::AddressError);
}

#[test]
fn json_roundtrip_preserves_data() {
    let payload = vec![0xDE, 0xAD, 0xBE, 0xEF];
    let txn = TlmTransaction::write(0x5000, payload.clone());
    let back: TlmTransaction = serde_json::from_str(&serde_json::to_string(&txn).unwrap()).unwrap();
    assert_eq!(back.data, payload);
}

#[test]
fn json_roundtrip_preserves_delay_ns() {
    let mut txn = TlmTransaction::read(0, 4);
    txn.delay_ns = 42.5;
    let back: TlmTransaction = serde_json::from_str(&serde_json::to_string(&txn).unwrap()).unwrap();
    assert!((back.delay_ns - 42.5).abs() < f64::EPSILON);
}

#[test]
fn json_roundtrip_preserves_byte_enables_none() {
    let txn = TlmTransaction::read(0, 4);
    let back: TlmTransaction = serde_json::from_str(&serde_json::to_string(&txn).unwrap()).unwrap();
    assert!(back.byte_enables.is_none());
}

// --- debug output ---

#[test]
fn tlm_transaction_debug_contains_address() {
    let txn = TlmTransaction::read(0xABCD, 4);
    let s = format!("{txn:?}");
    assert!(s.contains("abcd") || s.contains("ABCD") || s.contains("43981"));
}

#[test]
fn tlm_command_debug_is_non_empty() {
    assert!(!format!("{:?}", TlmCommand::Read).is_empty());
    assert!(!format!("{:?}", TlmCommand::Write).is_empty());
    assert!(!format!("{:?}", TlmCommand::Ignore).is_empty());
}

#[test]
fn tlm_response_debug_is_non_empty() {
    assert!(!format!("{:?}", TlmResponse::Ok).is_empty());
    assert!(!format!("{:?}", TlmResponse::GenericError).is_empty());
}
