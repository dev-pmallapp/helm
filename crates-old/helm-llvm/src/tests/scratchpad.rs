//! Tests for scratchpad memory

use crate::scratchpad::{ScratchpadConfig, ScratchpadMemory, ScratchpadStats};

// ─── Construction & basic accessors ──────────────────────────────────────────

#[test]
fn test_scratchpad_config_default() {
    let cfg = ScratchpadConfig::default();
    assert_eq!(cfg.size, 65536);
    assert_eq!(cfg.access_latency, 1);
    assert_eq!(cfg.ports, 2);
    assert!(cfg.power_per_access > 0.0);
}

#[test]
fn test_scratchpad_size_matches_config() {
    let cfg = ScratchpadConfig {
        size: 4096,
        ..Default::default()
    };
    let sp = ScratchpadMemory::new(cfg);
    assert_eq!(sp.size(), 4096);
}

#[test]
fn test_scratchpad_config_accessor() {
    let cfg = ScratchpadConfig {
        size: 2048,
        access_latency: 3,
        ports: 4,
        power_per_access: 0.5,
    };
    let sp = ScratchpadMemory::new(cfg.clone());
    assert_eq!(sp.config().size, 2048);
    assert_eq!(sp.config().access_latency, 3);
    assert_eq!(sp.config().ports, 4);
}

// ─── Port availability ────────────────────────────────────────────────────────

#[test]
fn test_scratchpad_two_ports_allows_two_simultaneous_accesses() {
    let cfg = ScratchpadConfig {
        size: 1024,
        ports: 2,
        access_latency: 2,
        power_per_access: 0.1,
    };
    let mut sp = ScratchpadMemory::new(cfg);

    // Both ports should be free initially
    assert!(sp.has_available_port());

    // Use first port
    sp.try_write(0, &[1, 2, 3, 4]).unwrap();
    assert!(sp.has_available_port(), "second port should still be free");

    // Use second port
    sp.try_write(4, &[5, 6, 7, 8]).unwrap();
    assert!(!sp.has_available_port(), "both ports now busy");
}

#[test]
fn test_scratchpad_single_port_no_concurrent_access() {
    let cfg = ScratchpadConfig {
        size: 1024,
        ports: 1,
        access_latency: 1,
        power_per_access: 0.1,
    };
    let mut sp = ScratchpadMemory::new(cfg);

    sp.try_write(0, &[1]).unwrap();
    assert!(!sp.has_available_port());

    // Second write must fail
    assert!(sp.try_write(1, &[2]).is_err());
    assert!(sp.try_read(0, 1).is_err());
}

#[test]
fn test_scratchpad_port_freed_after_tick() {
    let cfg = ScratchpadConfig {
        size: 1024,
        ports: 1,
        access_latency: 1,
        power_per_access: 0.1,
    };
    let mut sp = ScratchpadMemory::new(cfg);

    sp.try_write(0, &[0xAB]).unwrap();
    assert!(!sp.has_available_port());

    sp.tick();
    assert!(sp.has_available_port());
}

#[test]
fn test_scratchpad_port_freed_after_multiple_ticks() {
    let cfg = ScratchpadConfig {
        size: 1024,
        ports: 1,
        access_latency: 3,
        power_per_access: 0.1,
    };
    let mut sp = ScratchpadMemory::new(cfg);

    sp.try_write(0, &[0xFF]).unwrap();
    assert!(!sp.has_available_port());
    sp.tick();
    assert!(!sp.has_available_port());
    sp.tick();
    assert!(!sp.has_available_port());
    sp.tick();
    assert!(sp.has_available_port());
}

// ─── Bounds checking ─────────────────────────────────────────────────────────

#[test]
fn test_scratchpad_read_out_of_bounds_returns_error() {
    let cfg = ScratchpadConfig {
        size: 16,
        ..Default::default()
    };
    let mut sp = ScratchpadMemory::new(cfg);

    // Reading 4 bytes starting at offset 14 exceeds size 16
    let result = sp.try_read(14, 4);
    assert!(result.is_err());
}

#[test]
fn test_scratchpad_write_out_of_bounds_returns_error() {
    let cfg = ScratchpadConfig {
        size: 16,
        ..Default::default()
    };
    let mut sp = ScratchpadMemory::new(cfg);

    let result = sp.try_write(15, &[1, 2, 3]);
    assert!(result.is_err());
}

#[test]
fn test_scratchpad_access_at_exact_boundary_succeeds() {
    let cfg = ScratchpadConfig {
        size: 16,
        ports: 2,
        ..Default::default()
    };
    let mut sp = ScratchpadMemory::new(cfg);

    // Writing the last byte (offset 15, length 1) must succeed
    sp.try_write(15, &[0xBE]).unwrap();
    sp.tick();
    let data = sp.try_read(15, 1).unwrap();
    assert_eq!(data, vec![0xBE]);
}

// ─── Read/write correctness ───────────────────────────────────────────────────

#[test]
fn test_scratchpad_write_then_read_roundtrip() {
    let cfg = ScratchpadConfig {
        size: 256,
        ports: 2,
        access_latency: 1,
        power_per_access: 0.1,
    };
    let mut sp = ScratchpadMemory::new(cfg);

    let payload = vec![0xDE, 0xAD, 0xBE, 0xEF];
    sp.try_write(64, &payload).unwrap();
    sp.tick();
    let result = sp.try_read(64, 4).unwrap();
    assert_eq!(result, payload);
}

#[test]
fn test_scratchpad_data_initialised_to_zero() {
    let cfg = ScratchpadConfig {
        size: 128,
        ports: 1,
        ..Default::default()
    };
    let mut sp = ScratchpadMemory::new(cfg);

    let data = sp.try_read(0, 8).unwrap();
    assert_eq!(data, vec![0u8; 8]);
}

// ─── Statistics ───────────────────────────────────────────────────────────────

#[test]
fn test_scratchpad_stats_initial_zero() {
    let cfg = ScratchpadConfig::default();
    let sp = ScratchpadMemory::new(cfg);
    let stats = sp.stats();
    assert_eq!(stats.reads, 0);
    assert_eq!(stats.writes, 0);
    assert_eq!(stats.total_energy, 0.0);
}

#[test]
fn test_scratchpad_stats_count_reads_and_writes() {
    let cfg = ScratchpadConfig {
        size: 256,
        ports: 2,
        access_latency: 1,
        power_per_access: 1.0,
    };
    let mut sp = ScratchpadMemory::new(cfg);

    sp.try_write(0, &[1, 2]).unwrap();
    sp.tick();
    sp.try_write(2, &[3, 4]).unwrap();
    sp.tick();
    sp.try_read(0, 2).unwrap();
    sp.tick();
    sp.try_read(2, 2).unwrap();

    let stats = sp.stats();
    assert_eq!(stats.writes, 2);
    assert_eq!(stats.reads, 2);
}

#[test]
fn test_scratchpad_stats_energy_accumulates() {
    let power = 2.5_f64;
    let cfg = ScratchpadConfig {
        size: 64,
        ports: 2,
        access_latency: 1,
        power_per_access: power,
    };
    let mut sp = ScratchpadMemory::new(cfg);

    sp.try_write(0, &[0]).unwrap();
    sp.tick();
    sp.try_read(0, 1).unwrap();

    let stats = sp.stats();
    // One write + one read → 2 accesses × power_per_access
    let expected = power * 2.0;
    assert!(
        (stats.total_energy - expected).abs() < 1e-9,
        "expected {expected}, got {}",
        stats.total_energy
    );
}

#[test]
fn test_scratchpad_utilization_zero_when_idle() {
    let cfg = ScratchpadConfig::default();
    let sp = ScratchpadMemory::new(cfg);
    let stats = sp.stats();
    assert_eq!(stats.utilization, 0.0);
}

#[test]
fn test_scratchpad_utilization_full_when_all_ports_busy() {
    let cfg = ScratchpadConfig {
        size: 256,
        ports: 2,
        access_latency: 2,
        power_per_access: 0.1,
    };
    let mut sp = ScratchpadMemory::new(cfg);

    // Occupy both ports
    sp.try_write(0, &[1]).unwrap();
    sp.try_write(1, &[2]).unwrap();

    let stats = sp.stats();
    assert!(
        (stats.utilization - 1.0).abs() < 1e-9,
        "expected 1.0, got {}",
        stats.utilization
    );
}
