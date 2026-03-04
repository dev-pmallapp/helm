use crate::bridge::*;
use crate::tlm::*;

#[test]
fn stub_bridge_returns_ok() {
    let config = BridgeConfig::default();
    let mut bridge = StubBridge::new(&config);
    let mut txn = TlmTransaction::read(0x1000, 4);
    bridge.transact(&mut txn).unwrap();
    assert!(txn.is_ok());
    assert_eq!(txn.delay_ns, 0.0);
}

#[test]
fn stub_bridge_advances_time() {
    let config = BridgeConfig {
        quantum_ns: 100.0,
        ..Default::default()
    };
    let mut bridge = StubBridge::new(&config);
    assert_eq!(bridge.systemc_time_ns(), 0.0);
    bridge.sync_quantum().unwrap();
    assert!((bridge.systemc_time_ns() - 100.0).abs() < f64::EPSILON);
}
