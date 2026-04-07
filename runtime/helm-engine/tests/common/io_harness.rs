#![allow(dead_code)]

use std::sync::{Arc, Mutex};

use helm_core::{AccessType, DmaPort, MemFault, MemInterface};
use helm_devices::InterruptSink;
use helm_engine::{ExecMode, HelmEngine, Isa, StopReason};
use helm_memory::{FlatMem, HelmAddressSpace};
use helm_timing::VirtualTiming;

#[derive(Clone, Default)]
pub struct RecordingInterruptSink {
    asserts: Arc<Mutex<Vec<u64>>>,
    deasserts: Arc<Mutex<Vec<u64>>>,
}

impl RecordingInterruptSink {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn asserted(&self) -> Vec<u64> {
        self.asserts.lock().unwrap().clone()
    }

    pub fn deasserted(&self) -> Vec<u64> {
        self.deasserts.lock().unwrap().clone()
    }

    pub fn assert_count(&self) -> usize {
        self.asserts.lock().unwrap().len()
    }

    pub fn last_assert(&self) -> Option<u64> {
        self.asserts.lock().unwrap().last().copied()
    }
}

impl InterruptSink for RecordingInterruptSink {
    fn on_assert(&self, wire_id: helm_devices::WireId) {
        self.asserts.lock().unwrap().push(wire_id.as_u64());
    }

    fn on_deassert(&self, wire_id: helm_devices::WireId) {
        self.deasserts.lock().unwrap().push(wire_id.as_u64());
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MmioDeviceHandle {
    pub idx: usize,
    pub base: u64,
    pub size: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DummyDmaRequester {
    requester_id: u32,
    substream_id: Option<u32>,
}

impl DummyDmaRequester {
    pub fn new(requester_id: u32) -> Self {
        Self {
            requester_id,
            substream_id: None,
        }
    }

    pub fn with_substream(mut self, substream_id: u32) -> Self {
        self.substream_id = Some(substream_id);
        self
    }

    pub fn requester_id(&self) -> u32 {
        self.requester_id
    }

    pub fn substream_id(&self) -> Option<u32> {
        self.substream_id
    }

    pub fn dma_copy_via_port(
        &self,
        port: &dyn DmaPort,
        src: u64,
        dst: u64,
        len: usize,
    ) -> Result<(), MemFault> {
        let mut buf = vec![0u8; len];
        port.dma_read(src, &mut buf)?;
        port.dma_write(dst, &buf)
    }
}

pub struct IoTestBoard {
    engine: HelmEngine<VirtualTiming>,
    ram_size: u64,
    next_alloc: u64,
}

impl IoTestBoard {
    pub fn aarch64_minimal(ram_size: u64) -> Self {
        let timing = VirtualTiming::new(1.0);
        let mut engine = HelmEngine::new(
            Isa::AArch64,
            ExecMode::System,
            timing,
            0,
            ram_size.try_into().expect("ram_size must fit usize"),
        );
        let sys_mem = HelmAddressSpace::new(FlatMem::new(0, ram_size as usize));
        engine.install_test_aarch64_system_board(sys_mem).unwrap();
        Self {
            engine,
            ram_size,
            next_alloc: 0x1000,
        }
    }

    pub fn map_device(
        &mut self,
        base: u64,
        device: Box<dyn helm_devices::Device>,
    ) -> MmioDeviceHandle {
        let size = device.region_size();
        let idx = self
            .engine
            .with_system_memory_mut(|sys| sys.add_device(base, device))
            .expect("system memory missing");
        MmioDeviceHandle { idx, base, size }
    }

    pub fn alloc_ram(&mut self, size: u64, align: u64) -> u64 {
        let align = align.max(1);
        let start = (self.next_alloc + (align - 1)) & !(align - 1);
        let end = start + size;
        assert!(
            end <= self.ram_size,
            "IoTestBoard::alloc_ram out of space start={start:#x} size={size:#x} end={end:#x} ram={:#x}",
            self.ram_size
        );
        self.next_alloc = end;
        start
    }

    pub fn write_guest(&mut self, pa: u64, bytes: &[u8]) {
        self.engine
            .with_system_memory_mut(|sys| sys.write_bytes(pa, bytes).unwrap())
            .expect("system memory missing");
    }

    pub fn read_guest(&mut self, pa: u64, len: usize) -> Vec<u8> {
        self.engine
            .with_system_memory_mut(|sys| {
                let mut buf = vec![0u8; len];
                sys.read_bytes(pa, &mut buf).unwrap();
                buf
            })
            .expect("system memory missing")
    }

    pub fn load_words(&mut self, pc: u64, words: &[u32]) {
        let mut bytes = Vec::with_capacity(words.len() * 4);
        for word in words {
            bytes.extend_from_slice(&word.to_le_bytes());
        }
        self.engine
            .with_system_memory_mut(|sys| sys.ram.load_bytes(pc, &bytes))
            .expect("system memory missing");
        self.engine.set_pc(pc);
    }

    pub fn set_reg_x(&mut self, reg: usize, value: u64) {
        self.engine
            .with_a64_state_mut(|a64| a64.x[reg] = value)
            .expect("AArch64 state missing");
    }

    pub fn reg_x(&mut self, reg: usize) -> u64 {
        self.engine
            .with_a64_state_mut(|a64| a64.x[reg])
            .expect("AArch64 state missing")
    }

    pub fn run_insns(&mut self, n: u64) -> StopReason {
        self.engine.run(n)
    }

    pub fn assert_reg_x(&mut self, reg: usize, expected: u64) {
        let actual = self.reg_x(reg);
        assert_eq!(
            actual, expected,
            "x{reg}: expected {expected:#018x}, got {actual:#018x}"
        );
    }

    pub fn mmio_read(&mut self, addr: u64, size: usize) -> u64 {
        self.engine
            .with_system_memory_mut(|sys| sys.read(addr, size, AccessType::Load).unwrap())
            .expect("system memory missing")
    }

    pub fn mmio_write(&mut self, addr: u64, size: usize, value: u64) {
        self.engine
            .with_system_memory_mut(|sys| sys.write(addr, size, value, AccessType::Store).unwrap())
            .expect("system memory missing");
    }

    pub fn with_device_mut<T: helm_devices::Device + 'static, R>(
        &mut self,
        idx: usize,
        f: impl FnOnce(&mut T) -> R,
    ) -> Option<R> {
        self.engine
            .with_system_memory_mut(|sys| sys.with_device_mut::<T, _>(idx, f))
            .expect("system memory missing")
    }

    pub fn assert_device<T: helm_devices::Device + 'static>(
        &mut self,
        idx: usize,
        f: impl FnOnce(&mut T),
    ) {
        let ran = self.with_device_mut::<T, _>(idx, |dev| {
            f(dev);
        });
        assert!(ran.is_some(), "device index {idx} had unexpected type");
    }

    pub fn dma_copy_physical(
        &mut self,
        requester: &DummyDmaRequester,
        src: u64,
        dst: u64,
        len: usize,
    ) -> Result<(), MemFault> {
        let _ = requester;
        let mut buf = vec![0u8; len];
        self.engine
            .with_system_memory_mut(|sys| {
                sys.read_bytes(src, &mut buf)?;
                sys.write_bytes(dst, &buf)
            })
            .expect("system memory missing")
    }
}
