use crate::arm::pl011::*;
use crate::backend::*;
use crate::device::Device;
use crate::transaction::Transaction;

fn read_reg(uart: &mut Pl011, offset: u64) -> u32 {
    let mut txn = Transaction::read(0, 4);
    txn.offset = offset;
    Device::transact(uart, &mut txn).unwrap();
    txn.data_u32()
}

fn write_reg(uart: &mut Pl011, offset: u64, value: u32) {
    let mut txn = Transaction::write(0, 4, value as u64);
    txn.offset = offset;
    Device::transact(uart, &mut txn).unwrap();
}

// ── Identification registers ────────────────────────────────────────────────

#[test]
fn periph_id_matches_pl011() {
    let mut uart = Pl011::new("uart", Box::new(NullCharBackend));
    assert_eq!(read_reg(&mut uart, 0xFE0), 0x11); // part number low
    assert_eq!(read_reg(&mut uart, 0xFE4), 0x10); // part number high + designer
}

#[test]
fn cell_id_matches_primecell() {
    let mut uart = Pl011::new("uart", Box::new(NullCharBackend));
    assert_eq!(read_reg(&mut uart, 0xFF0), 0x0D);
    assert_eq!(read_reg(&mut uart, 0xFF4), 0xF0);
    assert_eq!(read_reg(&mut uart, 0xFF8), 0x05);
    assert_eq!(read_reg(&mut uart, 0xFFC), 0xB1);
}

// ── Flag register ───────────────────────────────────────────────────────────

#[test]
fn initial_flags_txfe_rxfe() {
    let mut uart = Pl011::new("uart", Box::new(NullCharBackend));
    let fr = read_reg(&mut uart, 0x018);
    assert!(fr & FR_TXFE != 0, "TX FIFO should be empty");
    assert!(fr & FR_RXFE != 0, "RX FIFO should be empty");
    assert!(fr & FR_TXFF == 0, "TX FIFO should not be full");
}

// ── TX (write) ──────────────────────────────────────────────────────────────

#[test]
fn write_char_sets_tx_interrupt() {
    let mut uart = Pl011::new("uart", Box::new(BufferCharBackend::new()));

    // Enable UART
    write_reg(&mut uart, 0x034, (CR_UARTEN | CR_TXE | CR_RXE) as u32);

    // Write 'A'
    write_reg(&mut uart, 0x000, b'A' as u32);

    // TX interrupt should be set (FIFO immediately "empty" again)
    let ris = read_reg(&mut uart, 0x040);
    assert!(ris & INT_TX != 0, "TX interrupt should be set");
}

#[test]
fn write_multiple_chars() {
    let mut uart = Pl011::new("uart", Box::new(BufferCharBackend::new()));
    write_reg(&mut uart, 0x034, (CR_UARTEN | CR_TXE | CR_RXE) as u32);
    write_reg(&mut uart, 0x000, b'H' as u32);
    write_reg(&mut uart, 0x000, b'i' as u32);
    write_reg(&mut uart, 0x000, b'!' as u32);
    // Verify TX interrupt stays set
    assert!(read_reg(&mut uart, 0x040) & INT_TX != 0);
}

// ── RX (read) ───────────────────────────────────────────────────────────────

#[test]
fn read_char_from_backend() {
    let mut backend = BufferCharBackend::new();
    backend.inject(b"X");
    let mut uart = Pl011::new("uart", Box::new(backend));
    write_reg(&mut uart, 0x034, (CR_UARTEN | CR_TXE | CR_RXE) as u32);

    // Read data register
    let data = read_reg(&mut uart, 0x000);
    assert_eq!(data, b'X' as u32);
}

#[test]
fn rx_fifo_multiple_chars() {
    let mut backend = BufferCharBackend::new();
    backend.inject(b"ABC");
    let mut uart = Pl011::new("uart", Box::new(backend));
    // Enable FIFOs
    write_reg(&mut uart, 0x030, LCR_H_FEN as u32);

    assert_eq!(read_reg(&mut uart, 0x000), b'A' as u32);
    assert_eq!(read_reg(&mut uart, 0x000), b'B' as u32);
    assert_eq!(read_reg(&mut uart, 0x000), b'C' as u32);
}

#[test]
fn rx_empty_returns_zero() {
    let mut uart = Pl011::new("uart", Box::new(NullCharBackend));
    assert_eq!(read_reg(&mut uart, 0x000), 0);
}

#[test]
fn rx_interrupt_clears_when_empty() {
    let mut backend = BufferCharBackend::new();
    backend.inject(b"X");
    let mut uart = Pl011::new("uart", Box::new(backend));

    // Read the char → RX interrupt should clear
    read_reg(&mut uart, 0x000);
    let ris = read_reg(&mut uart, 0x040);
    assert!(
        ris & INT_RX == 0,
        "RX interrupt should be clear after reading all data"
    );
}

// ── Control register ────────────────────────────────────────────────────────

#[test]
fn control_register_readback() {
    let mut uart = Pl011::new("uart", Box::new(NullCharBackend));
    let cr_val = (CR_UARTEN | CR_TXE | CR_RXE | CR_LBE) as u32;
    write_reg(&mut uart, 0x034, cr_val);
    assert_eq!(read_reg(&mut uart, 0x034), cr_val);
}

// ── Loopback ────────────────────────────────────────────────────────────────

#[test]
fn loopback_mode() {
    let mut uart = Pl011::new("uart", Box::new(NullCharBackend));
    // Enable UART with loopback + FIFOs
    write_reg(
        &mut uart,
        0x034,
        (CR_UARTEN | CR_TXE | CR_RXE | CR_LBE) as u32,
    );
    write_reg(&mut uart, 0x030, LCR_H_FEN as u32);

    // Write 'Z' → should appear in RX FIFO
    write_reg(&mut uart, 0x000, b'Z' as u32);
    let data = read_reg(&mut uart, 0x000);
    assert_eq!(data, b'Z' as u32);
}

// ── Interrupt mask ──────────────────────────────────────────────────────────

#[test]
fn interrupt_mask_controls_mis() {
    let mut uart = Pl011::new("uart", Box::new(NullCharBackend));
    // TX interrupt is initially raw-set
    let ris = read_reg(&mut uart, 0x040);
    assert!(ris & INT_TX != 0);

    // MIS should be 0 because IMSC is 0
    let mis = read_reg(&mut uart, 0x044);
    assert_eq!(mis, 0);

    // Enable TX interrupt mask
    write_reg(&mut uart, 0x03C, INT_TX);
    let mis = read_reg(&mut uart, 0x044);
    assert!(mis & INT_TX != 0, "masked TX interrupt should be visible");
    assert!(uart.irq_level, "IRQ should be asserted");
}

#[test]
fn interrupt_clear() {
    let mut uart = Pl011::new("uart", Box::new(NullCharBackend));
    write_reg(&mut uart, 0x03C, INT_TX); // unmask TX
    assert!(uart.irq_level);

    // Clear TX interrupt
    write_reg(&mut uart, 0x048, INT_TX);
    let ris = read_reg(&mut uart, 0x040);
    assert!(ris & INT_TX == 0, "TX interrupt should be cleared");
    assert!(!uart.irq_level);
}

// ── Baud rate registers ─────────────────────────────────────────────────────

#[test]
fn baud_rate_readback() {
    let mut uart = Pl011::new("uart", Box::new(NullCharBackend));
    write_reg(&mut uart, 0x028, 26); // IBRD for 115200 baud @ 48 MHz
    write_reg(&mut uart, 0x02C, 3); // FBRD
    assert_eq!(read_reg(&mut uart, 0x028), 26);
    assert_eq!(read_reg(&mut uart, 0x02C), 3);
}

// ── Line control / FIFO enable ──────────────────────────────────────────────

#[test]
fn writing_lcr_h_flushes_fifo() {
    let mut backend = BufferCharBackend::new();
    backend.inject(b"old data");
    let mut uart = Pl011::new("uart", Box::new(backend));
    write_reg(&mut uart, 0x030, LCR_H_FEN as u32);

    // Fill RX FIFO
    read_reg(&mut uart, 0x018); // trigger fill

    // Write LCR_H again → should flush
    write_reg(&mut uart, 0x030, LCR_H_FEN as u32);
    let fr = read_reg(&mut uart, 0x018);
    assert!(
        fr & FR_RXFE != 0,
        "RX FIFO should be empty after LCR_H write"
    );
}

// ── Device trait ────────────────────────────────────────────────────────────

#[test]
fn device_name() {
    let uart = Pl011::new("my-uart", Box::new(NullCharBackend));
    assert_eq!(uart.name(), "my-uart");
}

#[test]
fn device_regions() {
    let uart = Pl011::new("uart", Box::new(NullCharBackend));
    assert_eq!(uart.regions().len(), 1);
    assert_eq!(uart.regions()[0].size, 0x1000);
}

#[test]
fn device_reset() {
    let mut uart = Pl011::new("uart", Box::new(NullCharBackend));
    write_reg(&mut uart, 0x03C, 0x7FF); // set IMSC
    Device::reset(&mut uart).unwrap();
    assert_eq!(read_reg(&mut uart, 0x03C), 0); // IMSC cleared
}

#[test]
fn device_checkpoint_restore() {
    let mut uart = Pl011::new("uart", Box::new(NullCharBackend));
    write_reg(&mut uart, 0x028, 42);

    let state = uart.checkpoint().unwrap();
    Device::reset(&mut uart).unwrap();
    assert_eq!(read_reg(&mut uart, 0x028), 0);

    uart.restore(&state).unwrap();
    assert_eq!(read_reg(&mut uart, 0x028), 42);
}

#[test]
fn transact_adds_stall() {
    let mut uart = Pl011::new("uart", Box::new(NullCharBackend));
    let mut txn = Transaction::read(0, 4);
    txn.offset = 0x018;
    Device::transact(&mut uart, &mut txn).unwrap();
    assert_eq!(txn.stall_cycles, 1);
}
