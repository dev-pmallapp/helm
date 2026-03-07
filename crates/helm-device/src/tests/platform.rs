use crate::backend::*;
use crate::device::Device;
use crate::platform::*;
use crate::transaction::Transaction;

// ── Platform builder ────────────────────────────────────────────────────────

#[test]
fn arm_virt_platform_creates() {
    let p = arm_virt_platform(Box::new(NullCharBackend), Box::new(NullCharBackend));
    assert_eq!(p.name, "arm-virt");
    assert!(!p.device_map().is_empty());
}

#[test]
fn arm_virt_uart_accessible() {
    let mut backend = BufferCharBackend::new();
    backend.inject(b"Q");
    let mut p = arm_virt_platform(Box::new(backend), Box::new(NullCharBackend));

    // Read PL011 PrimeCellID at uart0 base + 0xFF0
    let mut txn = Transaction::read(0x0900_0FF0, 4);
    p.system_bus.transact(&mut txn).unwrap();
    assert_eq!(txn.data_u32(), 0x0D, "PrimeCellID[0] should be 0x0D");
}

#[test]
fn arm_virt_uart0_write() {
    let mut p = arm_virt_platform(Box::new(BufferCharBackend::new()), Box::new(NullCharBackend));

    // Enable UART
    let mut txn = Transaction::write(0x0900_0034, 4, 0x301); // UARTEN | TXE | RXE
    p.system_bus.transact(&mut txn).unwrap();

    // Write 'A' to UARTDR
    let mut txn = Transaction::write(0x0900_0000, 4, b'A' as u64);
    p.system_bus.transact(&mut txn).unwrap();

    // Verify TX interrupt is set
    let mut txn = Transaction::read(0x0900_0040, 4); // UARTRIS
    p.system_bus.transact(&mut txn).unwrap();
    assert!(txn.data_u32() & 0x20 != 0, "TX interrupt should be set");
}

#[test]
fn arm_virt_uart1_at_offset() {
    let mut p = arm_virt_platform(Box::new(NullCharBackend), Box::new(NullCharBackend));

    // Read uart1 PrimeCellID at 0x0900_1FF0
    let mut txn = Transaction::read(0x0900_1FF0, 4);
    p.system_bus.transact(&mut txn).unwrap();
    assert_eq!(txn.data_u32(), 0x0D);
}

#[test]
fn arm_virt_apb_latency() {
    let mut p = arm_virt_platform(Box::new(NullCharBackend), Box::new(NullCharBackend));

    let mut txn = Transaction::read(0x0900_0018, 4); // UARTFR
    p.system_bus.transact(&mut txn).unwrap();
    // Stall = device (1) + APB bridge (1) + APB access (2) = 4
    assert_eq!(txn.stall_cycles, 4);
}

#[test]
fn arm_virt_reset() {
    let mut p = arm_virt_platform(Box::new(NullCharBackend), Box::new(NullCharBackend));
    assert!(p.reset().is_ok());
}

#[test]
fn platform_add_device() {
    use crate::arm::pl011::Pl011;

    let mut p = Platform::new("test");
    p.add_device("uart", 0x1000, Box::new(Pl011::new("uart", Box::new(NullCharBackend))));
    assert_eq!(p.device_map().len(), 1);
    assert_eq!(p.device_map()[0].0, "uart");
    assert_eq!(p.device_map()[0].1, 0x1000);
}

#[test]
fn platform_tick() {
    let mut p = arm_virt_platform(Box::new(NullCharBackend), Box::new(NullCharBackend));
    let events = p.tick(100).unwrap();
    // No events expected with null backends
    let _ = events;
}

// ── PL011 on AMBA APB bus ───────────────────────────────────────────────────

#[test]
fn pl011_on_apb_bus_loopback() {
    use crate::arm::pl011::*;
    use crate::proto::amba::ApbBus;

    let mut apb = ApbBus::new("apb", 0x10_0000);
    apb.attach(0x0000, 0x1000, Box::new(Pl011::new("uart0", Box::new(NullCharBackend))));

    // Enable UART with loopback + FIFO
    let mut txn = Transaction::write(0, 4, 0x381); // UARTEN | LBE | TXE | RXE
    txn.offset = 0x0034;
    Device::transact(&mut apb, &mut txn).unwrap();

    let mut txn = Transaction::write(0, 4, 0x10); // LCR_H: FEN
    txn.offset = 0x0030;
    Device::transact(&mut apb, &mut txn).unwrap();

    // Write 'Z'
    let mut txn = Transaction::write(0, 4, b'Z' as u64);
    txn.offset = 0x0000;
    Device::transact(&mut apb, &mut txn).unwrap();

    // Read back
    let mut txn = Transaction::read(0, 4);
    txn.offset = 0x0000;
    Device::transact(&mut apb, &mut txn).unwrap();
    assert_eq!(txn.data_u32(), b'Z' as u32);
}
