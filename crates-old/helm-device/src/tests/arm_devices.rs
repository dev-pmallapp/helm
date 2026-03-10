use crate::backend::*;
use crate::device::Device;
use crate::transaction::Transaction;

fn read_reg(dev: &mut dyn Device, offset: u64) -> u32 {
    let mut txn = Transaction::read(0, 4);
    txn.offset = offset;
    dev.transact(&mut txn).unwrap();
    txn.data_u32()
}

fn write_reg(dev: &mut dyn Device, offset: u64, value: u32) {
    let mut txn = Transaction::write(0, 4, value as u64);
    txn.offset = offset;
    dev.transact(&mut txn).unwrap();
}

// ── SP804 Dual Timer ────────────────────────────────────────────────────────

#[test]
fn sp804_primecell_id() {
    let mut t = crate::arm::sp804::Sp804::new("timer");
    assert_eq!(read_reg(&mut t, 0xFF0), 0x0D);
}

#[test]
fn sp804_load_and_value() {
    let mut t = crate::arm::sp804::Sp804::new("timer");
    write_reg(&mut t, 0x00, 1000); // Timer1 LOAD
    assert_eq!(read_reg(&mut t, 0x00), 1000);
    assert_eq!(read_reg(&mut t, 0x04), 1000); // VALUE = LOAD after write
}

#[test]
fn sp804_countdown() {
    let mut t = crate::arm::sp804::Sp804::new("timer");
    write_reg(&mut t, 0x00, 100);
    write_reg(&mut t, 0x08, 0xE2); // enable + periodic + interrupt + 32bit
    let events = t.tick(50).unwrap();
    assert!(events.is_empty());
    assert_eq!(read_reg(&mut t, 0x04), 50); // 100 - 50
}

#[test]
fn sp804_interrupt_on_zero() {
    let mut t = crate::arm::sp804::Sp804::new("timer");
    write_reg(&mut t, 0x00, 10);
    write_reg(&mut t, 0x08, 0xE2);
    let events = t.tick(15).unwrap();
    assert!(!events.is_empty());
    assert_eq!(read_reg(&mut t, 0x10), 1); // RIS set
}

// ── PL031 RTC ───────────────────────────────────────────────────────────────

#[test]
fn pl031_primecell_id() {
    let mut r = crate::arm::pl031::Pl031::new("rtc");
    assert_eq!(read_reg(&mut r, 0xFE0), 0x31);
}

#[test]
fn pl031_initial_time() {
    let r = crate::arm::pl031::Pl031::new("rtc").set_time(1000);
    let mut rtc = r;
    assert_eq!(read_reg(&mut rtc, 0x000), 1000);
}

// ── SP805 Watchdog ──────────────────────────────────────────────────────────

#[test]
fn sp805_load_and_value() {
    let mut w = crate::arm::sp805::Sp805::new("wdog");
    write_reg(&mut w, 0x000, 5000);
    assert_eq!(read_reg(&mut w, 0x004), 5000);
}

#[test]
fn sp805_locked_rejects_writes() {
    let mut w = crate::arm::sp805::Sp805::new("wdog");
    // Lock it
    write_reg(&mut w, 0xC00, 0);
    // Try to write LOAD — should be rejected
    write_reg(&mut w, 0x000, 42);
    assert_ne!(read_reg(&mut w, 0x000), 42);
}

#[test]
fn sp805_unlock() {
    let mut w = crate::arm::sp805::Sp805::new("wdog");
    write_reg(&mut w, 0xC00, 0); // lock
    write_reg(&mut w, 0xC00, 0x1ACC_E551); // unlock
    write_reg(&mut w, 0x000, 42);
    assert_eq!(read_reg(&mut w, 0x000), 42);
}

// ── PL061 GPIO ──────────────────────────────────────────────────────────────

#[test]
fn pl061_primecell_id() {
    let mut g = crate::arm::pl061::Pl061::new("gpio");
    assert_eq!(read_reg(&mut g, 0xFE0), 0x61);
}

#[test]
fn pl061_direction() {
    let mut g = crate::arm::pl061::Pl061::new("gpio");
    write_reg(&mut g, 0x400, 0xFF); // all outputs
    assert_eq!(read_reg(&mut g, 0x400), 0xFF);
}

#[test]
fn pl061_data_masked_read() {
    let mut g = crate::arm::pl061::Pl061::new("gpio");
    write_reg(&mut g, 0x400, 0xFF); // outputs
                                    // Write via address mask: bits [9:2] = 0xFF → address 0x3FC
    write_reg(&mut g, 0x3FC, 0xA5);
    // Read with mask 0xFF → address 0x3FC
    assert_eq!(read_reg(&mut g, 0x3FC), 0xA5);
    // Read with mask 0x0F → address 0x03C
    assert_eq!(read_reg(&mut g, 0x03C), 0x05);
}

// ── RealView System Registers ───────────────────────────────────────────────

#[test]
fn sysregs_board_id() {
    let mut s = crate::arm::sysregs::RealViewSysRegs::realview_pb_a8();
    assert_eq!(read_reg(&mut s, 0x000), 0x0178_0000);
}

#[test]
fn sysregs_lock_unlock() {
    let mut s = crate::arm::sysregs::RealViewSysRegs::realview_pb_a8();
    // Locked by default — LED write should be rejected
    write_reg(&mut s, 0x008, 0xFF);
    assert_eq!(read_reg(&mut s, 0x008), 0);
    // Unlock
    write_reg(&mut s, 0x020, 0xA05F);
    write_reg(&mut s, 0x008, 0xFF);
    assert_eq!(read_reg(&mut s, 0x008), 0xFF);
}

// ── GIC ─────────────────────────────────────────────────────────────────────

#[test]
fn gic_typer() {
    let mut g = crate::arm::gic::Gic::new("gic", 96);
    let typer = read_reg(&mut g, 0x004);
    assert_eq!(typer & 0x1F, 2); // 3 groups of 32 = 96 IRQs → ITLinesNumber = 2
}

#[test]
fn gic_enable_and_pending() {
    use crate::irq::InterruptController;
    let mut g = crate::arm::gic::Gic::new("gic", 96);

    // Enable distributor
    write_reg(&mut g, 0x000, 1);
    // Enable IRQ 33 (in register 1, bit 1)
    write_reg(&mut g, 0x104, 1 << 1); // ISENABLER1, bit 1 = IRQ 33

    // Inject IRQ 33
    g.inject(33, true);
    assert!(g.pending_for_cpu(0));

    // Enable CPU interface
    write_reg(&mut g, 0x1000, 1);
    // Acknowledge
    let irq = read_reg(&mut g, 0x100C); // GICC_IAR
    assert_eq!(irq, 33);
}

// ── BCM2837 System Timer ────────────────────────────────────────────────────

#[test]
fn bcm_sys_timer_counter() {
    let mut t = crate::arm::bcm_sys_timer::BcmSysTimer::new("timer");
    t.tick(1000).unwrap();
    assert_eq!(read_reg(&mut t, 0x04), 1000); // CLO
}

#[test]
fn bcm_sys_timer_compare_match() {
    let mut t = crate::arm::bcm_sys_timer::BcmSysTimer::new("timer");
    write_reg(&mut t, 0x10, 500); // C1 = 500
    let events = t.tick(500).unwrap();
    assert!(!events.is_empty());
    assert_eq!(read_reg(&mut t, 0x00) & 2, 2); // CS bit 1 set
}

// ── BCM2837 Mailbox ─────────────────────────────────────────────────────────

#[test]
fn bcm_mailbox_write_read() {
    let mut m = crate::arm::bcm_mailbox::BcmMailbox::rpi3();
    // Write to mailbox 1 (channel 8 = property tag)
    write_reg(&mut m, 0x20, 0x1000_0008);
    // Read from mailbox 0
    let val = read_reg(&mut m, 0x00);
    assert_eq!(val & 0xF, 8); // channel preserved
}

#[test]
fn bcm_mailbox_empty_status() {
    let mut m = crate::arm::bcm_mailbox::BcmMailbox::rpi3();
    let status = read_reg(&mut m, 0x18);
    assert!(status & 0x4000_0000 != 0); // MBOX_EMPTY
}

// ── BCM2837 Mini UART ───────────────────────────────────────────────────────

#[test]
fn bcm_mini_uart_lsr_tx_ready() {
    let mut u = crate::arm::bcm_mini_uart::BcmMiniUart::new("uart1", Box::new(NullCharBackend));
    let lsr = read_reg(&mut u, 0x14);
    assert!(lsr & 0x20 != 0); // TX empty
}

#[test]
fn bcm_mini_uart_rx() {
    let mut backend = BufferCharBackend::new();
    backend.inject(b"H");
    let mut u = crate::arm::bcm_mini_uart::BcmMiniUart::new("uart1", Box::new(backend));
    let data = read_reg(&mut u, 0x00);
    assert_eq!(data, b'H' as u32);
}

// ── Platform builders ───────────────────────────────────────────────────────

#[test]
fn realview_pb_platform_builds() {
    let p = crate::platform::realview_pb_platform(Box::new(NullCharBackend));
    assert_eq!(p.name, "realview-pb-a8");
    // Should have: sysregs, timer01, rtc, uart0-3, watchdog, gpio0-2, gic = 12 devices
    assert_eq!(p.device_map().len(), 12);
}

#[test]
fn realview_pb_uart0_accessible() {
    let mut p = crate::platform::realview_pb_platform(Box::new(NullCharBackend));
    // Read PL011 PrimeCellID at uart0 base (0x1000_9000 + 0xFE0)
    let mut txn = Transaction::read(0x1000_9FE0, 4);
    p.system_bus.transact(&mut txn).unwrap();
    assert_eq!(txn.data_u32(), 0x11); // PL011 PeriphID0
}

#[test]
fn realview_pb_gic_accessible() {
    let mut p = crate::platform::realview_pb_platform(Box::new(NullCharBackend));
    let mut txn = Transaction::read(0x1F00_0004, 4); // GICD_TYPER
    p.system_bus.transact(&mut txn).unwrap();
    assert_eq!(txn.data_u32() & 0x1F, 2); // 96 IRQs → IT_LINES = 2
}

#[test]
fn rpi3_platform_builds() {
    let p = crate::platform::rpi3_platform(Box::new(NullCharBackend), Box::new(NullCharBackend));
    assert_eq!(p.name, "rpi3");
    assert_eq!(p.device_map().len(), 5);
}

#[test]
fn rpi3_uart0_accessible() {
    let mut p =
        crate::platform::rpi3_platform(Box::new(NullCharBackend), Box::new(NullCharBackend));
    let mut txn = Transaction::read(0x3F20_1FE0, 4); // PL011 PeriphID0
    p.system_bus.transact(&mut txn).unwrap();
    assert_eq!(txn.data_u32(), 0x11);
}

#[test]
fn rpi3_sys_timer_accessible() {
    let mut p =
        crate::platform::rpi3_platform(Box::new(NullCharBackend), Box::new(NullCharBackend));
    // Tick the platform, then read timer counter
    p.tick(1234).unwrap();
    let mut txn = Transaction::read(0x3F00_3004, 4); // CLO
    p.system_bus.transact(&mut txn).unwrap();
    assert_eq!(txn.data_u32(), 1234);
}

// ── Fast-path correctness ───────────────────────────────────────────────────

#[test]
fn pl011_fast_path_matches_transact() {
    let mut uart = crate::arm::pl011::Pl011::new("uart", Box::new(NullCharBackend));

    // Write via transact
    write_reg(&mut uart, 0x028, 42);
    // Read via fast path
    let fast_val = uart.read_fast(0x028, 4).unwrap();
    assert_eq!(fast_val, 42);

    // Write via fast path
    uart.write_fast(0x028, 4, 99).unwrap();
    // Read via transact
    assert_eq!(read_reg(&mut uart, 0x028), 99);
}

#[test]
fn bus_fast_path_routes_correctly() {
    let mut p = crate::platform::realview_pb_platform(Box::new(NullCharBackend));
    // Fast-path read of SysRegs board ID
    let val = p.system_bus.read_fast(0x1000_0000, 4).unwrap();
    assert_eq!(val, 0x0178_0000);
}
