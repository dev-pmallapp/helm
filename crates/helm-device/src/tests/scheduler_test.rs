use crate::bus::DeviceBus;
use crate::device::DeviceEvent;
use crate::scheduler::*;
use helm_core::HelmResult;

struct DummyDevice {
    name: String,
    ticked: u64,
}

impl DummyDevice {
    fn new(name: &str) -> Self {
        Self { name: name.to_string(), ticked: 0 }
    }
}

impl TickableDevice for DummyDevice {
    fn run_until(&mut self, target_tick: u64, _bus: &mut DeviceBus) -> HelmResult<Vec<DeviceEvent>> {
        self.ticked = target_tick;
        Ok(vec![])
    }
    fn name(&self) -> &str {
        &self.name
    }
}

#[test]
fn new_scheduler_has_no_threads() {
    let sched = DeviceScheduler::new();
    assert_eq!(sched.num_threads(), 0);
}

#[test]
fn add_thread_increments_count() {
    let mut sched = DeviceScheduler::new();
    sched.add(DeviceThread::new("dev0", 1_000_000, Box::new(DummyDevice::new("dev0"))));
    assert_eq!(sched.num_threads(), 1);
}

#[test]
fn thread_accessor() {
    let mut sched = DeviceScheduler::new();
    sched.add(DeviceThread::new("dev0", 1_000_000, Box::new(DummyDevice::new("dev0"))));
    assert_eq!(sched.thread(0).unwrap().name, "dev0");
    assert!(sched.thread(1).is_none());
}

#[test]
fn device_thread_global_time_zero_initially() {
    let dt = DeviceThread::new("t", 1_000_000, Box::new(DummyDevice::new("t")));
    assert_eq!(dt.clock_ticks, 0);
    assert_eq!(dt.global_time(), 0);
}

#[test]
fn device_thread_global_time_proportional_to_ticks() {
    let mut dt = DeviceThread::new("t", 1_000_000_000, Box::new(DummyDevice::new("t")));
    dt.clock_ticks = 1;
    let time_one = dt.global_time();
    dt.clock_ticks = 2;
    let time_two = dt.global_time();
    assert!(time_two > time_one);
    assert_eq!(time_two, 2 * time_one);
}

#[test]
fn step_advances_slowest_thread() {
    let mut sched = DeviceScheduler::new();
    sched.add(DeviceThread::new("fast", 2_000_000, Box::new(DummyDevice::new("fast"))));
    sched.add(DeviceThread::new("slow", 1_000_000, Box::new(DummyDevice::new("slow"))));
    let mut bus = DeviceBus::system();
    let _events = sched.step(&mut bus).unwrap();
}
