use crate::address_map::AddressMap;
use crate::coop_scheduler::*;
use crate::device::Device;
use crate::region::{MemRegion, RegionKind};
use crate::transaction::Transaction;
use helm_core::HelmResult;

/// Test device that counts ticks.
struct TickCounter {
    name: String,
    ticks: u64,
    region: MemRegion,
    clock: u64,
}

impl TickCounter {
    fn new(name: &str, clock_hz: u64) -> Self {
        Self {
            name: name.to_string(),
            ticks: 0,
            region: MemRegion {
                name: name.to_string(),
                base: 0,
                size: 0x100,
                kind: RegionKind::Io,
                priority: 0,
            },
            clock: clock_hz,
        }
    }
}

impl Device for TickCounter {
    fn transact(&mut self, txn: &mut Transaction) -> HelmResult<()> {
        if !txn.is_write {
            txn.set_data_u64(self.ticks);
        }
        Ok(())
    }
    fn regions(&self) -> &[MemRegion] {
        std::slice::from_ref(&self.region)
    }
    fn name(&self) -> &str {
        &self.name
    }
    fn clock_hz(&self) -> u64 {
        self.clock
    }
    fn tick(&mut self, cycles: u64) -> HelmResult<Vec<crate::device::DeviceEvent>> {
        self.ticks += cycles;
        Ok(vec![])
    }
}

#[test]
fn empty_scheduler() {
    let mut sched = CoopScheduler::new();
    let mut map = AddressMap::new();
    assert_eq!(sched.num_entries(), 0);
    assert!(sched.step(&mut map).unwrap().is_empty());
}

#[test]
fn register_and_step() {
    let mut map = AddressMap::new();
    let dev = TickCounter::new("timer", 1_000_000);
    let id = map.attach("timer", Box::new(dev));
    map.map_region(id, 0x1000, 0x100, 0);
    map.commit();

    let mut sched = CoopScheduler::new();
    sched.register(id, 1_000_000);
    assert_eq!(sched.num_entries(), 1);

    // Step once
    sched.step(&mut map).unwrap();

    // Device should have been ticked once
    let val = map.read_fast(0x1000, 4).unwrap();
    assert_eq!(val, 1);
}

#[test]
fn multi_clock_ordering() {
    let mut map = AddressMap::new();

    // Fast device at 2 MHz, slow at 1 MHz
    let fast = TickCounter::new("fast", 2_000_000);
    let slow = TickCounter::new("slow", 1_000_000);
    let id_fast = map.attach("fast", Box::new(fast));
    let id_slow = map.attach("slow", Box::new(slow));
    map.map_region(id_fast, 0x1000, 0x100, 0);
    map.map_region(id_slow, 0x2000, 0x100, 0);
    map.commit();

    let mut sched = CoopScheduler::new();
    sched.register(id_fast, 2_000_000);
    sched.register(id_slow, 1_000_000);

    // Run 3 steps. The fast device should tick more since its timestamp
    // advances half as much per tick (higher freq = smaller timestep).
    sched.run_steps(3, &mut map).unwrap();

    let fast_ticks = map.read_fast(0x1000, 4).unwrap();
    let slow_ticks = map.read_fast(0x2000, 4).unwrap();

    // With 3 steps and the fast device having half the period,
    // the scheduler should interleave: fast(0→250fs), slow(0→500fs), fast(250→500fs)
    assert_eq!(fast_ticks, 2);
    assert_eq!(slow_ticks, 1);
}

#[test]
fn unregister_mid_run() {
    let mut map = AddressMap::new();
    let dev1 = TickCounter::new("dev1", 1_000_000);
    let dev2 = TickCounter::new("dev2", 1_000_000);
    let id1 = map.attach("dev1", Box::new(dev1));
    let id2 = map.attach("dev2", Box::new(dev2));
    map.map_region(id1, 0x1000, 0x100, 0);
    map.map_region(id2, 0x2000, 0x100, 0);
    map.commit();

    let mut sched = CoopScheduler::new();
    sched.register(id1, 1_000_000);
    sched.register(id2, 1_000_000);

    // Run a few steps
    sched.run_steps(4, &mut map).unwrap();

    // Unregister dev1
    sched.unregister(id1);
    assert_eq!(sched.num_entries(), 1);

    // Continue running — should only tick dev2
    let ticks_before = map.read_fast(0x1000, 4).unwrap();
    sched.run_steps(4, &mut map).unwrap();
    let ticks_after = map.read_fast(0x1000, 4).unwrap();

    // dev1 should NOT have advanced
    assert_eq!(ticks_before, ticks_after);
}

#[test]
fn renormalize_prevents_overflow() {
    let mut map = AddressMap::new();
    let dev = TickCounter::new("dev", 1_000_000);
    let id = map.attach("dev", Box::new(dev));
    map.commit();

    let mut sched = CoopScheduler::new();
    sched.register(id, 1_000_000);

    // Run many steps to build up timestamp
    sched.run_steps(100, &mut map).unwrap();

    let clock = sched.clock(id).unwrap();
    let ts_before = clock.time_fs();
    assert!(ts_before > 0);

    // Renormalize
    sched.renormalize();

    let clock = sched.clock(id).unwrap();
    assert_eq!(clock.time_fs(), 0);
}

#[test]
fn double_register_is_idempotent() {
    let mut sched = CoopScheduler::new();
    sched.register(0, 1_000_000);
    sched.register(0, 1_000_000); // same ID again
    assert_eq!(sched.num_entries(), 1);
}

#[test]
fn device_clock_step() {
    let mut clock = DeviceClock::new(1_000_000_000); // 1 GHz
    assert_eq!(clock.time_fs(), 0);

    clock.step(1);
    // 1 cycle at 1 GHz = 1ns = 1_000_000 fs
    assert_eq!(clock.time_fs(), 1_000_000);

    clock.step(10);
    assert_eq!(clock.time_fs(), 11_000_000);
}
