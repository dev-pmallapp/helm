# helm-engine — Test Plan

> Test strategy and test cases for `helm-engine` and its bus framework.
> Cross-references: [`HLD.md`](./HLD.md) · [`LLD-world.md`](./LLD-world.md) · [`LLD-bus-framework.md`](./LLD-bus-framework.md)

---

## Table of Contents

1. [Test Categories](#1-test-categories)
2. [UART Unit Tests](#2-uart-unit-tests)
3. [PCI Enumeration Tests](#3-pci-enumeration-tests)
4. [I2C Transaction Tests](#4-i2c-transaction-tests)
5. [SPI Transaction Tests](#5-spi-transaction-tests)
6. [Interrupt Routing Tests](#6-interrupt-routing-tests)
7. [Event Bus Observation Tests](#7-event-bus-observation-tests)
8. [Reset Tests](#8-reset-tests)
9. [Fuzzing Tests](#9-fuzzing-tests)
10. [Test Infrastructure](#10-test-infrastructure)

---

## 1. Test Categories

| Category | Runner | Location |
|---|---|---|
| Device unit tests | `cargo test` | `crates/helm-engine/tests/` and `#[cfg(test)]` in device crates |
| Bus unit tests | `cargo test` | `crates/helm-devices/src/bus/*/mod.rs` `#[cfg(test)]` |
| Fuzzing targets | `cargo fuzz` | `crates/helm-engine/fuzz/fuzz_targets/` |
| Integration tests (Python) | `pytest` | `crates/helm-python/tests/python/test_world.py` (see helm-python TEST.md) |

All Rust tests run with `cargo test -p helm-engine -p helm-devices`.

---

## 2. UART Unit Tests

```rust
// crates/helm-engine/tests/uart_tests.rs

use helm_world::{World, HelmObjectId};
use helm_devices::uart::Uart16550;

const UART_BASE: u64 = 0x10000000;
const CLOCK_HZ:  u32 = 1_843_200;

// Baud rate timing: 9600 baud at 1.8432 MHz
// cycles_per_bit = 1_843_200 / (16 * 9600) = 12
// 10 bits (1 start + 8 data + 1 stop) = 120 cycles; advance 200 for margin
const BAUD_9600_ADVANCE: u64 = 200;

fn uart_world() -> (World, HelmObjectId) {
    let mut world = World::new();
    let uart = world.add_device("uart", Box::new(Uart16550::new(CLOCK_HZ)));
    world.map_device(uart, UART_BASE);
    world.wire_interrupt(uart, "irq_out", world.irq_sink());
    world.elaborate();
    (world, uart)
}

// ── Basic TX ─────────────────────────────────────────────────────────────────

#[test]
fn test_uart_tx_fires_thre_interrupt() {
    let (mut world, uart) = uart_world();

    // Enable THRE interrupt (IER bit 1)
    world.mmio_write(UART_BASE + 1, 1, 0x02);

    // Write byte to THR (TX Holding Register, offset 0, DLAB=0)
    world.mmio_write(UART_BASE, 1, b'A' as u64);

    // Advance one baud period
    world.advance(BAUD_9600_ADVANCE);

    // THRE interrupt must be asserted
    let irqs = world.pending_interrupts();
    assert!(
        irqs.iter().any(|(_, pin)| pin == "irq_out"),
        "THRE interrupt not asserted: irqs = {:?}", irqs
    );
}

#[test]
fn test_uart_lsr_thre_bit_set_after_drain() {
    let (mut world, _uart) = uart_world();

    world.mmio_write(UART_BASE, 1, b'B' as u64);
    world.advance(BAUD_9600_ADVANCE);

    // LSR bit 5 (THRE) and bit 6 (TEMT) should be set after TX drains
    let lsr = world.mmio_read(UART_BASE + 5, 1);
    assert_ne!(lsr & 0x20, 0, "LSR THRE bit not set after TX drain");
    assert_ne!(lsr & 0x40, 0, "LSR TEMT bit not set after TX drain");
}

// ── Loopback ─────────────────────────────────────────────────────────────────

#[test]
fn test_uart_rx_loopback() {
    let (mut world, _uart) = uart_world();

    // Enable loopback mode: MCR bit 4
    world.mmio_write(UART_BASE + 4, 1, 0x10);

    // Write to TX — in loopback this feeds into RX FIFO
    world.mmio_write(UART_BASE, 1, b'Z' as u64);
    world.advance(BAUD_9600_ADVANCE);

    // LSR bit 0 (DR: Data Ready) must be set
    let lsr = world.mmio_read(UART_BASE + 5, 1);
    assert_ne!(lsr & 0x01, 0, "DR bit not set after loopback");

    // Read the received byte
    let rx = world.mmio_read(UART_BASE, 1);
    assert_eq!(rx, b'Z' as u64, "Loopback byte mismatch");
}

// ── FIFO overflow ─────────────────────────────────────────────────────────────

#[test]
fn test_uart_rx_fifo_overflow() {
    let (mut world, _uart) = uart_world();

    // Enable FIFO mode (FCR bit 0), 16-byte RX FIFO depth
    world.mmio_write(UART_BASE + 2, 1, 0x01);

    // Enable loopback to feed RX FIFO
    world.mmio_write(UART_BASE + 4, 1, 0x10);

    // Write 17 bytes — one over the 16-byte FIFO depth
    for byte in 0..17u64 {
        world.mmio_write(UART_BASE, 1, byte & 0xFF);
        world.advance(BAUD_9600_ADVANCE);
    }

    // LSR bit 1 (OE: Overrun Error) must be set
    let lsr = world.mmio_read(UART_BASE + 5, 1);
    assert_ne!(lsr & 0x02, 0, "LSR OE bit not set on FIFO overflow");
}

// ── Divisor latch ────────────────────────────────────────────────────────────

#[test]
fn test_uart_divisor_latch_readback() {
    let (mut world, _uart) = uart_world();

    // DLAB=1: LCR bit 7
    world.mmio_write(UART_BASE + 3, 1, 0x80);

    // Write divisor for 115200 baud: 1.8432 MHz / (16 * 115200) = 1
    world.mmio_write(UART_BASE,     1, 0x01);  // DLL = 1
    world.mmio_write(UART_BASE + 1, 1, 0x00);  // DLM = 0

    // Readback (still in DLAB mode)
    let dll = world.mmio_read(UART_BASE, 1);
    let dlm = world.mmio_read(UART_BASE + 1, 1);
    assert_eq!(dll, 0x01, "DLL readback mismatch");
    assert_eq!(dlm, 0x00, "DLM readback mismatch");

    // Clear DLAB: set 8N1
    world.mmio_write(UART_BASE + 3, 1, 0x03);

    // After DLAB=0, offset 0 is THR (write) / RBR (read), not DLL
    // Write to THR should not corrupt DLL
    world.mmio_write(UART_BASE, 1, b'X' as u64);

    // Re-enable DLAB and verify DLL unchanged
    world.mmio_write(UART_BASE + 3, 1, 0x80);
    let dll2 = world.mmio_read(UART_BASE, 1);
    assert_eq!(dll2, 0x01, "DLL corrupted after THR write");
}

// ── No interrupt when IER=0 ──────────────────────────────────────────────────

#[test]
fn test_uart_no_interrupt_without_ier() {
    let (mut world, _uart) = uart_world();

    // IER = 0 (no interrupts enabled) — default after reset
    world.mmio_write(UART_BASE + 1, 1, 0x00);

    // TX
    world.mmio_write(UART_BASE, 1, b'Q' as u64);
    world.advance(BAUD_9600_ADVANCE);

    // No IRQ should be pending
    assert!(
        world.pending_interrupts().is_empty(),
        "Unexpected IRQ with IER=0: {:?}", world.pending_interrupts()
    );
}
```

---

## 3. PCI Enumeration Tests

```rust
// crates/helm-engine/tests/pci_tests.rs

use helm_world::World;
use helm_devices::bus::pci::{PciBus, VirtioBlkEndpoint, VirtioNetEndpoint};
use helm_devices::bus::{BusAddress, BusDeviceMeta};

const VENDOR_VIRTIO: u16 = 0x1AF4;
const DEVICE_VIRTIO_BLK: u16 = 0x1001;
const DEVICE_VIRTIO_NET: u16 = 0x1000;
const PCI_ECAM_BASE: u64 = 0x3000_0000;

fn pci_world_with_two_devices() -> (World, helm_world::HelmObjectId) {
    let mut world = World::new();

    let mut pci = PciBus::new("pci0");
    pci.attach_endpoint(0, 0, Box::new(VirtioBlkEndpoint::new(VENDOR_VIRTIO, DEVICE_VIRTIO_BLK))).unwrap();
    pci.attach_endpoint(1, 0, Box::new(VirtioNetEndpoint::new(VENDOR_VIRTIO, DEVICE_VIRTIO_NET))).unwrap();

    let bus_id = world.add_device("pci0", Box::new(pci));
    world.map_device(bus_id, PCI_ECAM_BASE);
    world.elaborate();

    (world, bus_id)
}

// ── Bus trait enumeration ─────────────────────────────────────────────────────

#[test]
fn test_pci_enumerate_two_devices() {
    let (world, bus_id) = pci_world_with_two_devices();

    let bus  = world.get_bus(bus_id).expect("pci0 not found");
    let devs = bus.enumerate();

    assert_eq!(devs.len(), 2, "Expected 2 PCI devices");
}

#[test]
fn test_pci_vendor_ids_correct() {
    let (world, bus_id) = pci_world_with_two_devices();

    let bus  = world.get_bus(bus_id).expect("pci0");
    let devs = bus.enumerate();

    for dev in &devs {
        if let BusDeviceMeta::Pci { vendor_id, .. } = dev.metadata {
            assert_eq!(vendor_id, VENDOR_VIRTIO, "unexpected vendor_id for {}", dev.name);
        }
    }
}

#[test]
fn test_pci_device_ids_distinct() {
    let (world, bus_id) = pci_world_with_two_devices();

    let bus     = world.get_bus(bus_id).expect("pci0");
    let devs    = bus.enumerate();
    let dev_ids: Vec<u16> = devs.iter().filter_map(|d| {
        if let BusDeviceMeta::Pci { device_id, .. } = d.metadata { Some(device_id) } else { None }
    }).collect();

    assert!(dev_ids.contains(&DEVICE_VIRTIO_BLK), "VirtIO BLK not found");
    assert!(dev_ids.contains(&DEVICE_VIRTIO_NET), "VirtIO NET not found");
}

// ── MMIO config space reads (firmware path) ───────────────────────────────────

fn ecam_addr(bus: u8, device: u8, function: u8, offset: u16) -> u64 {
    PCI_ECAM_BASE
        | ((bus as u64) << 20)
        | ((device as u64) << 15)
        | ((function as u64) << 12)
        | offset as u64
}

#[test]
fn test_pci_config_read_vendor_device_id() {
    let (world, _) = pci_world_with_two_devices();

    // Read vendor_id | device_id at offset 0 for bus 0, device 0, function 0
    let vid_did = world.mmio_read(ecam_addr(0, 0, 0, 0), 4);
    let vendor = (vid_did & 0xFFFF) as u16;
    let device = ((vid_did >> 16) & 0xFFFF) as u16;

    assert_eq!(vendor, VENDOR_VIRTIO,      "Config space vendor_id wrong");
    assert_eq!(device, DEVICE_VIRTIO_BLK,  "Config space device_id wrong");
}

#[test]
fn test_pci_missing_device_returns_all_fs() {
    let (world, _) = pci_world_with_two_devices();

    // Bus 0, device 31 (unused) — should return all-Fs (PCI spec)
    let vid_did = world.mmio_read(ecam_addr(0, 31, 0, 0), 4);
    assert_eq!(vid_did, 0xFFFF_FFFF_FFFF_FFFF, "Missing PCI device should return all-Fs");
}

#[test]
fn test_pci_config_write_bar() {
    let (mut world, _) = pci_world_with_two_devices();

    // Write to BAR 0 (offset 0x10) of device 0
    world.mmio_write(ecam_addr(0, 0, 0, 0x10), 4, 0xFFFF_FFFF);

    // Read back — BAR should decode to region size (device-specific)
    let bar = world.mmio_read(ecam_addr(0, 0, 0, 0x10), 4);
    // The VirtIO BLK device must decode the BAR write correctly
    assert_ne!(bar, 0, "BAR readback should not be zero");
}
```

---

## 4. I2C Transaction Tests

```rust
// crates/helm-engine/tests/i2c_tests.rs

use helm_world::World;
use helm_devices::bus::i2c::{I2cBus, Tmp102};

const I2C_BASE: u64 = 0x1000_1000;
const TMP102_ADDR: u8 = 0x48;

fn i2c_world_with_sensor() -> (World, helm_world::HelmObjectId) {
    let mut world = World::new();

    let mut i2c = I2cBus::new("i2c0");
    i2c.attach_i2c(Box::new(Tmp102::new()), TMP102_ADDR).unwrap();

    let i2c_id = world.add_device("i2c0", Box::new(i2c));
    world.map_device(i2c_id, I2C_BASE);
    world.wire_interrupt(i2c_id, "irq_out", world.irq_sink());
    world.elaborate();

    (world, i2c_id)
}

#[test]
fn test_i2c_sensor_read_default_temperature() {
    let (mut world, _) = i2c_world_with_sensor();

    // START with READ direction
    world.mmio_write(I2C_BASE + 1, 1, ((TMP102_ADDR as u64) << 1) | 1);
    world.mmio_write(I2C_BASE,     1, 0x01);  // START
    world.advance(10);

    let status = world.mmio_read(I2C_BASE + 4, 1);
    assert_eq!(status & 0x04, 0, "NACK from TMP102");

    // Read high byte
    world.mmio_write(I2C_BASE, 1, 0x04);  // READ
    world.advance(10);
    let high = world.mmio_read(I2C_BASE + 3, 1);

    // Read low byte
    world.mmio_write(I2C_BASE, 1, 0x04);
    world.advance(10);
    let low = world.mmio_read(I2C_BASE + 3, 1);

    // STOP
    world.mmio_write(I2C_BASE, 1, 0x02);
    world.advance(5);

    // TMP102 default = 25°C = raw 0x0C80; top 12 bits = 0x0C8
    let raw = ((high as u16) << 4) | ((low as u16) >> 4);
    assert_eq!(raw, 0x0C8, "TMP102 temperature mismatch (expected 25°C)");
}

#[test]
fn test_i2c_nack_on_missing_device() {
    let (mut world, _) = i2c_world_with_sensor();

    // Address a device that does not exist (0x50)
    world.mmio_write(I2C_BASE + 1, 1, (0x50u64 << 1) | 1);
    world.mmio_write(I2C_BASE,     1, 0x01);  // START
    world.advance(10);

    let status = world.mmio_read(I2C_BASE + 4, 1);
    assert_ne!(status & 0x04, 0, "Expected NACK for missing I2C device");
}
```

---

## 5. SPI Transaction Tests

```rust
// crates/helm-engine/tests/spi_tests.rs

use helm_world::World;
use helm_devices::bus::spi::{SpiBus, SpiNorFlash};

const SPI_BASE: u64 = 0x1000_2000;
const FLASH_SIZE: usize = 8 * 1024 * 1024;  // 8 MiB
const JEDEC_CMD: u8 = 0x9F;
const WINBOND_MFG_ID: u8 = 0xEF;

fn spi_world_with_flash() -> (World, helm_world::HelmObjectId) {
    let mut world = World::new();

    let mut spi = SpiBus::new("spi0");
    spi.attach_spi(Box::new(SpiNorFlash::new(FLASH_SIZE)), 0).unwrap();  // CS 0

    let spi_id = world.add_device("spi0", Box::new(spi));
    world.map_device(spi_id, SPI_BASE);
    world.elaborate();

    (world, spi_id)
}

fn spi_cs_assert(world: &mut World) {
    world.mmio_write(SPI_BASE + 3, 1, 0xFE);  // CS_REG: CS 0 active (bit 0 = 0)
}

fn spi_cs_deassert(world: &mut World) {
    world.mmio_write(SPI_BASE + 3, 1, 0xFF);  // CS_REG: all deselected
}

fn spi_transfer(world: &mut World, byte: u8) -> u8 {
    world.mmio_write(SPI_BASE + 0, 1, byte as u64);  // TX_DATA
    world.mmio_write(SPI_BASE + 2, 1, 0x01);          // CONTROL: start
    world.advance(10);
    world.mmio_read(SPI_BASE + 1, 1) as u8             // RX_DATA
}

#[test]
fn test_spi_flash_jedec_manufacturer_id() {
    let (mut world, _) = spi_world_with_flash();

    spi_cs_assert(&mut world);
    spi_transfer(&mut world, JEDEC_CMD);     // send READ ID command
    let mfg_id = spi_transfer(&mut world, 0x00);  // dummy — receive mfg ID
    spi_cs_deassert(&mut world);

    assert_eq!(mfg_id, WINBOND_MFG_ID, "SPI flash manufacturer ID mismatch");
}

#[test]
fn test_spi_flash_read_write_page() {
    let (mut world, _) = spi_world_with_flash();

    // Write Enable (WREN = 0x06)
    spi_cs_assert(&mut world);
    spi_transfer(&mut world, 0x06);
    spi_cs_deassert(&mut world);

    // Page Program (PP = 0x02) at address 0x001000
    spi_cs_assert(&mut world);
    spi_transfer(&mut world, 0x02);           // PP command
    spi_transfer(&mut world, 0x00);           // addr[23:16]
    spi_transfer(&mut world, 0x10);           // addr[15:8]
    spi_transfer(&mut world, 0x00);           // addr[7:0]
    spi_transfer(&mut world, 0xAB);           // data byte
    spi_cs_deassert(&mut world);
    world.advance(100_000);                   // tPP (page program time)

    // Read back: READ command (0x03)
    spi_cs_assert(&mut world);
    spi_transfer(&mut world, 0x03);           // READ command
    spi_transfer(&mut world, 0x00);           // addr[23:16]
    spi_transfer(&mut world, 0x10);           // addr[15:8]
    spi_transfer(&mut world, 0x00);           // addr[7:0]
    let readback = spi_transfer(&mut world, 0x00);  // dummy — receive data
    spi_cs_deassert(&mut world);

    assert_eq!(readback, 0xAB, "SPI flash page program/read mismatch");
}
```

---

## 6. Interrupt Routing Tests

```rust
// crates/helm-engine/tests/irq_tests.rs

use helm_world::World;
use helm_devices::uart::Uart16550;
use helm_devices::plic::Plic;

const UART_BASE: u64 = 0x10000000;
const PLIC_BASE: u64 = 0x0c000000;

#[test]
fn test_uart_irq_routes_through_plic() {
    let mut world = World::new();

    let uart = world.add_device("uart", Box::new(Uart16550::new(1_843_200)));
    let plic = world.add_device("plic", Box::new(Plic::new(32)));

    world.map_device(uart, UART_BASE);
    world.map_device(plic, PLIC_BASE);

    // Wire: uart.irq_out → plic input 10
    let plic_input_10 = world.plic_input_sink(plic, 10);
    world.wire_interrupt(uart, "irq_out", plic_input_10);

    world.elaborate();

    // Enable UART TX interrupt
    world.mmio_write(UART_BASE + 1, 1, 0x02);

    // Enable PLIC source 10
    world.mmio_write(PLIC_BASE + 0x2000, 4, 1 << 10);  // PLIC enable register

    // Set PLIC priority for source 10
    world.mmio_write(PLIC_BASE + 0x40, 4, 1);  // source 10 priority = 1

    // TX to trigger THRE interrupt
    world.mmio_write(UART_BASE, 1, b'T' as u64);
    world.advance(200);

    // PLIC pending register: source 10 bit should be set
    // PLIC pending: offset 0x1000, contains one bit per source
    let pending = world.mmio_read(PLIC_BASE + 0x1000, 4);
    assert_ne!(pending & (1 << 10), 0, "UART IRQ not reflected in PLIC pending register");
}

#[test]
fn test_irq_deasserted_after_ack() {
    let mut world = World::new();

    let uart = world.add_device("uart", Box::new(Uart16550::new(1_843_200)));
    world.map_device(uart, UART_BASE);
    world.wire_interrupt(uart, "irq_out", world.irq_sink());
    world.elaborate();

    world.mmio_write(UART_BASE + 1, 1, 0x02);  // enable THRE IRQ
    world.mmio_write(UART_BASE, 1, b'U' as u64);
    world.advance(200);

    // Interrupt asserted
    assert!(!world.pending_interrupts().is_empty());

    // Acknowledge by reading IIR (Interrupt Identity Register, offset 2)
    let _iir = world.mmio_read(UART_BASE + 2, 1);

    // After IIR read, THRE interrupt should be cleared
    world.advance(10);
    let irqs = world.pending_interrupts();
    assert!(irqs.is_empty(), "IRQ not cleared after IIR read: {:?}", irqs);
}
```

---

## 7. Event Bus Observation Tests

```rust
// crates/helm-engine/tests/eventbus_tests.rs

use std::sync::{Arc, Mutex};
use helm_world::World;
use helm_devices::uart::Uart16550;
use helm_devices::bus::event_bus::{HelmEvent, HelmEventKind};

const UART_BASE: u64 = 0x10000000;

#[test]
fn test_memwrite_events_observed() {
    let mut world = World::new();
    let uart = world.add_device("uart", Box::new(Uart16550::new(1_843_200)));
    world.map_device(uart, UART_BASE);
    world.elaborate();

    let writes: Arc<Mutex<Vec<(u64, u64)>>> = Arc::new(Mutex::new(Vec::new()));
    let writes_clone = Arc::clone(&writes);

    let _handle = world.on_event(HelmEventKind::MemWrite, move |event| {
        if let HelmEvent::MemWrite { addr, val, .. } = event {
            writes_clone.lock().unwrap().push((*addr, *val));
        }
    });

    world.mmio_write(UART_BASE,     1, 0x03);  // LCR
    world.mmio_write(UART_BASE + 3, 1, 0x80);  // DLAB

    let w = writes.lock().unwrap();
    assert!(w.contains(&(UART_BASE, 0x03)),     "LCR write not observed");
    assert!(w.contains(&(UART_BASE + 3, 0x80)), "DLAB write not observed");
}

#[test]
fn test_event_handle_drop_unsubscribes() {
    let mut world = World::new();
    let uart = world.add_device("uart", Box::new(Uart16550::new(1_843_200)));
    world.map_device(uart, UART_BASE);
    world.elaborate();

    let count: Arc<Mutex<u32>> = Arc::new(Mutex::new(0));
    let count_clone = Arc::clone(&count);

    {
        let _handle = world.on_event(HelmEventKind::MemWrite, move |_| {
            *count_clone.lock().unwrap() += 1;
        });

        world.mmio_write(UART_BASE, 1, 0x01);  // observed
        // _handle drops here → unsubscribed
    }

    world.mmio_write(UART_BASE, 1, 0x02);  // NOT observed

    let c = *count.lock().unwrap();
    assert_eq!(c, 1, "Expected exactly 1 event before unsubscribe, got {c}");
}

#[test]
fn test_multiple_subscribers_same_event() {
    let mut world = World::new();
    let uart = world.add_device("uart", Box::new(Uart16550::new(1_843_200)));
    world.map_device(uart, UART_BASE);
    world.elaborate();

    let count_a: Arc<Mutex<u32>> = Arc::new(Mutex::new(0));
    let count_b: Arc<Mutex<u32>> = Arc::new(Mutex::new(0));

    let ca = Arc::clone(&count_a);
    let cb = Arc::clone(&count_b);

    let _h1 = world.on_event(HelmEventKind::MemWrite, move |_| { *ca.lock().unwrap() += 1; });
    let _h2 = world.on_event(HelmEventKind::MemWrite, move |_| { *cb.lock().unwrap() += 1; });

    world.mmio_write(UART_BASE, 1, 0xAA);
    world.mmio_write(UART_BASE, 1, 0xBB);

    assert_eq!(*count_a.lock().unwrap(), 2, "subscriber A missed events");
    assert_eq!(*count_b.lock().unwrap(), 2, "subscriber B missed events");
}
```

---

## 8. Reset Tests

```rust
// crates/helm-engine/tests/reset_tests.rs

use helm_world::World;
use helm_devices::uart::Uart16550;

const UART_BASE: u64 = 0x10000000;

fn make_uart_world() -> (World, helm_world::HelmObjectId) {
    let mut world = World::new();
    let uart = world.add_device("uart", Box::new(Uart16550::new(1_843_200)));
    world.map_device(uart, UART_BASE);
    world.wire_interrupt(uart, "irq_out", world.irq_sink());
    world.elaborate();
    (world, uart)
}

#[test]
fn test_reset_via_re_instantiation_has_clean_state() {
    // First world: dirty state
    {
        let (mut world, _) = make_uart_world();
        world.mmio_write(UART_BASE + 3, 1, 0x80);  // DLAB=1
        world.mmio_write(UART_BASE,     1, 0xFF);  // DLL=0xFF (non-default)
        world.advance(1000);
        // world drops here
    }

    // Second world: clean state
    let (world, _) = make_uart_world();

    // DLL should be at power-on default (0x00)
    // Access via DLAB: can only be checked via another write sequence
    // Verify tick = 0
    assert_eq!(world.current_tick(), 0, "Fresh world should start at tick 0");
    assert!(world.pending_interrupts().is_empty(), "Fresh world should have no pending IRQs");
}

#[test]
fn test_device_reset_via_signal() {
    let (mut world, uart) = make_uart_world();

    // Dirty state: non-default LCR
    world.mmio_write(UART_BASE + 3, 1, 0xAB);

    // Assert hardware reset signal
    world.signal_raise(uart, "reset");
    world.advance(1);

    // After reset, LCR should be 0x00 (power-on default)
    let lcr = world.mmio_read(UART_BASE + 3, 1);
    assert_eq!(lcr, 0x00, "LCR not reset to 0x00 after reset signal");
}

#[test]
fn test_repeated_world_construction_is_deterministic() {
    // Run the same test sequence 5 times with fresh worlds
    for iteration in 0..5 {
        let (mut world, uart) = make_uart_world();

        world.mmio_write(UART_BASE + 1, 1, 0x02);   // IER: THRE enable
        world.mmio_write(UART_BASE,     1, b'X' as u64);
        world.advance(200);

        let irqs = world.pending_interrupts();
        assert!(
            irqs.iter().any(|(_, pin)| pin == "irq_out"),
            "iteration {iteration}: THRE not asserted"
        );

        assert_eq!(world.current_tick(), 200, "iteration {iteration}: unexpected tick");
    }
}
```

---

## 9. Fuzzing Tests

```rust
// crates/helm-engine/fuzz/fuzz_targets/uart_mmio.rs

#![no_main]

use libfuzzer_sys::fuzz_target;
use helm_world::World;
use helm_devices::uart::Uart16550;

/// Fuzz target: drive arbitrary MMIO sequences at the UART.
///
/// The fuzzer provides a byte stream. Each 7-byte packet drives one MMIO operation:
///   [0..2]: offset as u16 LE (constrained to UART register range 0–7)
///   [2]:    size tag (0→1, 1→2, 2→4, 3→1, MSB→read vs write)
///   [3..7]: value as u32 LE
///
/// Invariant: must not panic. Sanitizers catch all memory safety issues.
fuzz_target!(|data: &[u8]| {
    let mut world = World::new();
    let uart = world.add_device("uart", Box::new(Uart16550::new(1_843_200)));
    world.map_device(uart, 0x1000);
    world.elaborate();

    for chunk in data.chunks(7) {
        if chunk.len() < 7 { break; }

        let raw_offset = u16::from_le_bytes(chunk[0..2].try_into().unwrap()) as u64;
        let offset     = raw_offset % 8;   // constrain to UART register space
        let size_tag   = chunk[2];
        let size       = match size_tag & 0x03 { 0 => 1, 1 => 2, 2 => 4, _ => 1 };
        let val        = u32::from_le_bytes(chunk[3..7].try_into().unwrap()) as u64;
        let is_read    = size_tag & 0x80 != 0;

        if is_read {
            let _ = world.mmio_read(0x1000 + offset, size);
        } else {
            world.mmio_write(0x1000 + offset, size, val);
        }

        // Advance a small number of ticks to allow timer callbacks to fire
        world.advance(10);
    }
    // Invariant: we reach here without panic
});
```

```rust
// crates/helm-engine/fuzz/fuzz_targets/uart_plic_interleaved.rs

#![no_main]

use libfuzzer_sys::fuzz_target;
use helm_world::World;
use helm_devices::uart::Uart16550;
use helm_devices::plic::Plic;

/// Fuzz target: interleaved UART + PLIC MMIO sequences.
///
/// Exercises interrupt routing between two devices with random stimulus.
fuzz_target!(|data: &[u8]| {
    let mut world = World::new();

    let uart = world.add_device("uart", Box::new(Uart16550::new(1_843_200)));
    let plic = world.add_device("plic", Box::new(Plic::new(32)));

    world.map_device(uart, 0x10000000);
    world.map_device(plic, 0x0c000000);

    let plic_input_10 = world.plic_input_sink(plic, 10);
    world.wire_interrupt(uart, "irq_out", plic_input_10);

    world.elaborate();

    for chunk in data.chunks(8) {
        if chunk.len() < 8 { break; }

        // chunk[0] bit 0: target (0=uart, 1=plic)
        let (base, max_offset) = if chunk[0] & 1 == 0 {
            (0x10000000u64, 8u64)
        } else {
            (0x0c000000u64, 0x200_0000u64)  // PLIC is large
        };

        let offset = (u16::from_le_bytes(chunk[1..3].try_into().unwrap()) as u64) % max_offset;
        let size   = match chunk[3] % 3 { 0 => 1, 1 => 2, _ => 4 };
        let val    = u32::from_le_bytes(chunk[4..8].try_into().unwrap()) as u64;

        if chunk[0] & 0x80 != 0 {
            let _ = world.mmio_read(base + offset, size);
        } else {
            world.mmio_write(base + offset, size, val);
        }

        world.advance(5);
    }
});
```

### Running the Fuzzers

```bash
# Install cargo-fuzz if not already installed
cargo install cargo-fuzz

# Run UART fuzzer with AddressSanitizer (default in cargo-fuzz)
cargo fuzz run uart_mmio \
  --manifest-path crates/helm-engine/Cargo.toml \
  -- -max_len=1024 -timeout=10

# Run UART+PLIC interleaved fuzzer
cargo fuzz run uart_plic_interleaved \
  --manifest-path crates/helm-engine/Cargo.toml \
  -- -max_len=2048 -timeout=10

# Reproduce a crash
cargo fuzz run uart_mmio \
  --manifest-path crates/helm-engine/Cargo.toml \
  fuzz/artifacts/uart_mmio/crash-<hash>

# Run with UBSan (catches integer overflow in baud rate math, FIFO wrapping, etc.)
RUSTFLAGS="-Z sanitizer=undefined" \
cargo fuzz run uart_mmio \
  --manifest-path crates/helm-engine/Cargo.toml
```

---

## 10. Test Infrastructure

### Test Helper Macros

```rust
// crates/helm-engine/tests/helpers.rs

/// Assert that a UART IRQ is pending, with a descriptive panic message.
#[macro_export]
macro_rules! assert_irq_pending {
    ($world:expr, $uart_id:expr) => {{
        let irqs = $world.pending_interrupts();
        assert!(
            irqs.iter().any(|(id, _)| *id == $uart_id),
            "Expected IRQ from {:?} but pending_interrupts() = {:?}",
            $uart_id, irqs
        );
    }};
}

/// Assert that no IRQs are pending.
#[macro_export]
macro_rules! assert_no_irqs {
    ($world:expr) => {{
        let irqs = $world.pending_interrupts();
        assert!(
            irqs.is_empty(),
            "Expected no pending IRQs but got: {:?}", irqs
        );
    }};
}
```

### Running All Tests

```bash
# Run all helm-engine and device unit tests
cargo test -p helm-engine -p helm-devices -- --nocapture

# Run with verbose output
cargo test -p helm-engine -- --nocapture --test-threads=1

# Run a specific test
cargo test -p helm-engine test_uart_tx_fires_thre_interrupt -- --nocapture

# Run all tests in the workspace (includes helm-engine world module)
cargo test --workspace
```
