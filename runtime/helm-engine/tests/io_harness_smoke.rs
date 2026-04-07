//! Smoke tests for the reusable kernel-free IO/DMA test harness.

mod common;

use std::sync::{Arc, Mutex};

use common::io_harness::{DummyDmaRequester, IoTestBoard, RecordingInterruptSink};
use helm_devices::{BufferCharBackend, CharBackend, Device, InterruptPin, InterruptSink, WireId};
use helm_hw_char::Pl011;

struct SharedCharBackend {
    inner: Arc<Mutex<BufferCharBackend>>,
}

impl SharedCharBackend {
    fn new(inner: Arc<Mutex<BufferCharBackend>>) -> Self {
        Self { inner }
    }
}

impl CharBackend for SharedCharBackend {
    fn write(&mut self, data: &[u8]) -> usize {
        self.inner.lock().unwrap().write(data)
    }

    fn read(&mut self) -> Option<u8> {
        self.inner.lock().unwrap().read()
    }

    fn can_write(&self) -> bool {
        self.inner.lock().unwrap().can_write()
    }

    fn can_read(&self) -> bool {
        self.inner.lock().unwrap().can_read()
    }
}

struct PulseIrqDevice {
    value: u32,
    irq_out: InterruptPin,
}

impl PulseIrqDevice {
    fn new() -> Self {
        Self {
            value: 0,
            irq_out: InterruptPin::new(),
        }
    }
}

impl Device for PulseIrqDevice {
    fn read(&mut self, offset: u64, _size: usize) -> u64 {
        if offset == 0 {
            self.value as u64
        } else {
            0
        }
    }

    fn write(&mut self, offset: u64, _size: usize, val: u64) {
        if offset == 0 {
            self.value = val as u32;
            if val != 0 {
                self.irq_out.assert();
            } else {
                self.irq_out.deassert();
            }
        }
    }

    fn region_size(&self) -> u64 {
        0x1000
    }
}

#[test]
fn cpu_store_can_drive_uart_tx_without_guest_kernel() {
    const UART_BASE: u64 = 0x0900_0000;

    let mut io = IoTestBoard::aarch64_minimal(0x20_000);
    let shared = Arc::new(Mutex::new(BufferCharBackend::new()));
    let backend = SharedCharBackend::new(Arc::clone(&shared));
    io.map_device(UART_BASE, Box::new(Pl011::new(Box::new(backend))));

    io.load_words(0, &[0xF9000040]); // STR X0, [X2]
    io.set_reg_x(0, b'H' as u64);
    io.set_reg_x(2, UART_BASE);
    assert!(matches!(io.run_insns(1), helm_engine::StopReason::Quantum));

    let output = shared.lock().unwrap().drain_tx();
    assert_eq!(output, b"H");
}

#[test]
fn cpu_load_can_observe_device_to_cpu_data_path() {
    const UART_BASE: u64 = 0x0900_0000;

    let mut io = IoTestBoard::aarch64_minimal(0x20_000);
    let mut backend = BufferCharBackend::new();
    backend.inject_rx(b"Z");
    io.map_device(UART_BASE, Box::new(Pl011::new(Box::new(backend))));

    io.load_words(0, &[0xF9400041]); // LDR X1, [X2]
    io.set_reg_x(2, UART_BASE);
    assert!(matches!(io.run_insns(1), helm_engine::StopReason::Quantum));
    io.assert_reg_x(1, b'Z' as u64);
}

#[test]
fn cpu_write_can_trigger_device_interrupt_observed_by_sink() {
    const DEV_BASE: u64 = 0x0A00_0000;
    const IRQ_WIRE: u64 = 33;

    let mut io = IoTestBoard::aarch64_minimal(0x20_000);
    let sink = Arc::new(RecordingInterruptSink::new());
    let dev = io.map_device(DEV_BASE, Box::new(PulseIrqDevice::new()));
    io.assert_device::<PulseIrqDevice>(dev.idx, |d| {
        d.irq_out.wire(
            WireId::from(IRQ_WIRE),
            sink.clone() as Arc<dyn InterruptSink>,
        );
    });

    io.load_words(0, &[0xF9000040]); // STR X0, [X2]
    io.set_reg_x(0, 1);
    io.set_reg_x(2, DEV_BASE);
    assert!(matches!(io.run_insns(1), helm_engine::StopReason::Quantum));

    assert_eq!(sink.assert_count(), 1);
    assert_eq!(sink.last_assert(), Some(IRQ_WIRE));
}

#[test]
fn dummy_dma_requester_can_copy_guest_ram_without_device_specific_code() {
    let mut io = IoTestBoard::aarch64_minimal(0x20_000);
    let req = DummyDmaRequester::new(0x20);
    let src = io.alloc_ram(64, 64);
    let dst = io.alloc_ram(64, 64);
    io.write_guest(src, b"abc");

    io.dma_copy_physical(&req, src, dst, 3).unwrap();

    assert_eq!(io.read_guest(dst, 3), b"abc");
}
