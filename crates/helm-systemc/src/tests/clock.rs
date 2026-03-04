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
