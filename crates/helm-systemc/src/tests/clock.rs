use crate::clock::ClockDomain;

#[test]
fn cycles_to_ns_at_1ghz() {
    let clk = ClockDomain::new("cpu", 1_000_000_000);
    assert!((clk.cycles_to_ns(1000) - 1000.0).abs() < 0.01);
}

#[test]
fn ns_to_cycles_at_2ghz() {
    let clk = ClockDomain::new("cpu", 2_000_000_000);
    assert_eq!(clk.ns_to_cycles(500.0), 1000);
}

#[test]
fn cross_domain_conversion() {
    let cpu = ClockDomain::new("cpu", 2_000_000_000);
    let bus = ClockDomain::new("bus", 100_000_000);
    // 2000 CPU cycles @ 2 GHz = 1000 ns = 100 bus cycles @ 100 MHz
    assert_eq!(cpu.convert_to(2000, &bus), 100);
}

// --- period stored ---

#[test]
fn frequency_stored_verbatim() {
    let clk = ClockDomain::new("peri", 48_000_000);
    assert_eq!(clk.frequency_hz, 48_000_000);
}

#[test]
fn name_stored_verbatim() {
    let clk = ClockDomain::new("my_clock", 1_000_000);
    assert_eq!(clk.name, "my_clock");
}

#[test]
fn name_accepts_string_owned() {
    let name = String::from("dyn_clock");
    let clk = ClockDomain::new(name, 100_000_000);
    assert_eq!(clk.name, "dyn_clock");
}

// --- period round-trip at common frequencies ---

#[test]
fn cycles_to_ns_zero_cycles_is_zero() {
    let clk = ClockDomain::new("cpu", 1_000_000_000);
    assert_eq!(clk.cycles_to_ns(0), 0.0);
}

#[test]
fn cycles_to_ns_at_100mhz() {
    // 100 MHz → 10 ns per cycle
    let clk = ClockDomain::new("bus", 100_000_000);
    assert!((clk.cycles_to_ns(1) - 10.0).abs() < 1e-6);
}

#[test]
fn cycles_to_ns_at_500mhz() {
    // 500 MHz → 2 ns per cycle
    let clk = ClockDomain::new("core", 500_000_000);
    assert!((clk.cycles_to_ns(1) - 2.0).abs() < 1e-6);
}

#[test]
fn cycles_to_ns_at_3ghz() {
    // 3 GHz → 1/3 ns per cycle; 3 cycles should be ≈ 1 ns
    let clk = ClockDomain::new("cpu", 3_000_000_000);
    assert!((clk.cycles_to_ns(3) - 1.0).abs() < 1e-6);
}

// --- ns_to_cycles round-trips ---

#[test]
fn ns_to_cycles_zero_ns_is_zero() {
    let clk = ClockDomain::new("cpu", 1_000_000_000);
    assert_eq!(clk.ns_to_cycles(0.0), 0);
}

#[test]
fn ns_to_cycles_at_1ghz() {
    // 1 GHz → 1 cycle per ns
    let clk = ClockDomain::new("cpu", 1_000_000_000);
    assert_eq!(clk.ns_to_cycles(1.0), 1);
    assert_eq!(clk.ns_to_cycles(1000.0), 1000);
}

#[test]
fn ns_to_cycles_at_100mhz() {
    // 100 MHz → 1 cycle per 10 ns
    let clk = ClockDomain::new("bus", 100_000_000);
    assert_eq!(clk.ns_to_cycles(10.0), 1);
    assert_eq!(clk.ns_to_cycles(100.0), 10);
}

#[test]
fn cycles_to_ns_then_back_is_identity() {
    let clk = ClockDomain::new("cpu", 1_000_000_000);
    let cycles: u64 = 12_345;
    let ns = clk.cycles_to_ns(cycles);
    assert_eq!(clk.ns_to_cycles(ns), cycles);
}

// --- cross-domain conversion ---

#[test]
fn convert_same_frequency_is_identity() {
    let a = ClockDomain::new("a", 250_000_000);
    let b = ClockDomain::new("b", 250_000_000);
    assert_eq!(a.convert_to(100, &b), 100);
}

#[test]
fn convert_slow_to_fast_multiplies() {
    // 50 MHz → 100 MHz: same wall time → twice as many fast cycles
    let slow = ClockDomain::new("slow", 50_000_000);
    let fast = ClockDomain::new("fast", 100_000_000);
    assert_eq!(slow.convert_to(10, &fast), 20);
}

#[test]
fn convert_fast_to_slow_divides() {
    // 400 MHz → 100 MHz: same wall time → quarter as many slow cycles
    let fast = ClockDomain::new("fast", 400_000_000);
    let slow = ClockDomain::new("slow", 100_000_000);
    assert_eq!(fast.convert_to(400, &slow), 100);
}

#[test]
fn convert_to_does_not_mutate_self() {
    let src = ClockDomain::new("src", 1_000_000_000);
    let dst = ClockDomain::new("dst", 500_000_000);
    let _ = src.convert_to(1000, &dst);
    // frequency_hz must be unchanged after conversion
    assert_eq!(src.frequency_hz, 1_000_000_000);
}

// --- clone / debug ---

#[test]
fn clock_domain_is_cloneable() {
    let clk = ClockDomain::new("orig", 1_000_000_000);
    let cloned = clk.clone();
    assert_eq!(cloned.frequency_hz, clk.frequency_hz);
    assert_eq!(cloned.name, clk.name);
}

#[test]
fn clock_domain_debug_contains_name() {
    let clk = ClockDomain::new("debugclock", 1_000_000_000);
    let s = format!("{clk:?}");
    assert!(s.contains("debugclock"));
}
