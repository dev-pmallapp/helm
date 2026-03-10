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

// --- BridgeConfig defaults ---

#[test]
fn default_config_mode_is_in_process() {
    let cfg = BridgeConfig::default();
    assert_eq!(cfg.mode, BridgeMode::InProcess);
}

#[test]
fn default_config_timing_is_loosely_timed() {
    let cfg = BridgeConfig::default();
    assert_eq!(cfg.timing, TlmTimingMode::LooselyTimed);
}

#[test]
fn default_config_quantum_is_10_us() {
    let cfg = BridgeConfig::default();
    assert!((cfg.quantum_ns - 10_000.0).abs() < f64::EPSILON);
}

#[test]
fn default_config_cpu_freq_is_1ghz() {
    let cfg = BridgeConfig::default();
    assert_eq!(cfg.cpu_frequency_hz, 1_000_000_000);
}

// --- BridgeConfig clone / debug ---

#[test]
fn bridge_config_is_cloneable() {
    let cfg = BridgeConfig::default();
    let cloned = cfg.clone();
    assert_eq!(cloned.mode, cfg.mode);
    assert_eq!(cloned.cpu_frequency_hz, cfg.cpu_frequency_hz);
}

#[test]
fn bridge_config_debug_is_non_empty() {
    let cfg = BridgeConfig::default();
    assert!(!format!("{cfg:?}").is_empty());
}

// --- BridgeMode equality ---

#[test]
fn bridge_mode_variants_are_distinct() {
    assert_ne!(BridgeMode::InProcess, BridgeMode::SharedMemory);
    assert_ne!(BridgeMode::SharedMemory, BridgeMode::Socket);
    assert_ne!(BridgeMode::InProcess, BridgeMode::Socket);
}

#[test]
fn bridge_mode_is_copy() {
    let mode = BridgeMode::Socket;
    let copied = mode;
    assert_eq!(mode, copied);
}

// --- TlmTimingMode equality ---

#[test]
fn timing_mode_variants_are_distinct() {
    assert_ne!(
        TlmTimingMode::LooselyTimed,
        TlmTimingMode::ApproximatelyTimed
    );
}

#[test]
fn timing_mode_is_copy() {
    let t = TlmTimingMode::ApproximatelyTimed;
    let copied = t;
    assert_eq!(t, copied);
}

// --- StubBridge initial state ---

#[test]
fn stub_bridge_starts_at_time_zero() {
    let cfg = BridgeConfig::default();
    let bridge = StubBridge::new(&cfg);
    assert_eq!(bridge.systemc_time_ns(), 0.0);
}

// --- StubBridge transact clears delay ---

#[test]
fn stub_bridge_transact_clears_delay_on_write() {
    let cfg = BridgeConfig::default();
    let mut bridge = StubBridge::new(&cfg);
    let mut txn = TlmTransaction::write(0x4000, vec![0xFF]);
    txn.delay_ns = 999.0; // non-zero to verify it is reset
    bridge.transact(&mut txn).unwrap();
    assert_eq!(txn.delay_ns, 0.0);
    assert!(txn.is_ok());
}

// --- StubBridge multiple quantums ---

#[test]
fn stub_bridge_time_accumulates_over_quantums() {
    let cfg = BridgeConfig {
        quantum_ns: 250.0,
        ..Default::default()
    };
    let mut bridge = StubBridge::new(&cfg);
    for _ in 0..4 {
        bridge.sync_quantum().unwrap();
    }
    assert!((bridge.systemc_time_ns() - 1000.0).abs() < f64::EPSILON);
}

#[test]
fn stub_bridge_single_quantum_matches_config() {
    let cfg = BridgeConfig {
        quantum_ns: 1.0,
        ..Default::default()
    };
    let mut bridge = StubBridge::new(&cfg);
    bridge.sync_quantum().unwrap();
    assert!((bridge.systemc_time_ns() - 1.0).abs() < f64::EPSILON);
}

// --- StubBridge interleaved transact / sync ---

#[test]
fn stub_bridge_transact_does_not_advance_time() {
    let cfg = BridgeConfig {
        quantum_ns: 50.0,
        ..Default::default()
    };
    let mut bridge = StubBridge::new(&cfg);
    let mut txn = TlmTransaction::read(0, 4);
    bridge.transact(&mut txn).unwrap();
    // transact must not advance the simulated clock
    assert_eq!(bridge.systemc_time_ns(), 0.0);
}

#[test]
fn stub_bridge_mixed_read_write_both_succeed() {
    let cfg = BridgeConfig::default();
    let mut bridge = StubBridge::new(&cfg);

    let mut rtxn = TlmTransaction::read(0x100, 8);
    bridge.transact(&mut rtxn).unwrap();
    assert!(rtxn.is_ok());

    let mut wtxn = TlmTransaction::write(0x200, vec![1, 2, 3, 4]);
    bridge.transact(&mut wtxn).unwrap();
    assert!(wtxn.is_ok());
}

// --- custom BridgeConfig fields stored ---

#[test]
fn bridge_config_custom_fields_stored() {
    let cfg = BridgeConfig {
        mode: BridgeMode::Socket,
        timing: TlmTimingMode::ApproximatelyTimed,
        quantum_ns: 5_000.0,
        cpu_frequency_hz: 2_000_000_000,
    };
    assert_eq!(cfg.mode, BridgeMode::Socket);
    assert_eq!(cfg.timing, TlmTimingMode::ApproximatelyTimed);
    assert!((cfg.quantum_ns - 5_000.0).abs() < f64::EPSILON);
    assert_eq!(cfg.cpu_frequency_hz, 2_000_000_000);
}
