use crate::address_map::*;
use crate::device::{Device, DeviceId};
use crate::region::{MemRegion, RegionKind};
use crate::transaction::Transaction;
use helm_core::HelmResult;
use std::sync::{Arc, Mutex};

/// Minimal test device that reads/writes a single register.
struct RegDevice {
    name: String,
    value: u64,
    region: MemRegion,
}

impl RegDevice {
    fn new(name: &str, size: u64) -> Self {
        Self {
            name: name.to_string(),
            value: 0,
            region: MemRegion {
                name: name.to_string(),
                base: 0,
                size,
                kind: RegionKind::Io,
                priority: 0,
            },
        }
    }

    fn with_value(mut self, v: u64) -> Self {
        self.value = v;
        self
    }
}

impl Device for RegDevice {
    fn transact(&mut self, txn: &mut Transaction) -> HelmResult<()> {
        if txn.is_write {
            self.value = txn.data_u64();
        } else {
            txn.set_data_u64(self.value);
        }
        Ok(())
    }
    fn regions(&self) -> &[MemRegion] {
        std::slice::from_ref(&self.region)
    }
    fn name(&self) -> &str {
        &self.name
    }
}

#[test]
fn attach_map_dispatch() {
    let mut map = AddressMap::new();
    let dev = RegDevice::new("uart", 0x1000).with_value(0xDEAD);
    let id = map.attach("uart", Box::new(dev));
    map.map_region(id, 0x0900_0000, 0x1000, 0);
    map.commit();

    // Dispatch a read
    let mut txn = Transaction::read(0x0900_0000, 4);
    map.dispatch(&mut txn).unwrap();
    assert_eq!(txn.data_u64(), 0xDEAD);

    // Dispatch a write
    let mut txn = Transaction::write(0x0900_0000, 4, 0xBEEF);
    map.dispatch(&mut txn).unwrap();

    // Read back
    let mut txn = Transaction::read(0x0900_0000, 4);
    map.dispatch(&mut txn).unwrap();
    assert_eq!(txn.data_u64(), 0xBEEF);
}

#[test]
fn binary_search_correctness() {
    let mut map = AddressMap::new();

    // Add 100 devices at sequential 0x1000-spaced addresses
    for i in 0..100u64 {
        let dev = RegDevice::new(&format!("dev{i}"), 0x1000).with_value(i);
        let id = map.attach(format!("dev{i}"), Box::new(dev));
        map.map_region(id, i * 0x1000, 0x1000, 0);
    }
    map.commit();

    // Verify each device resolves correctly
    for i in 0..100u64 {
        let mut txn = Transaction::read(i * 0x1000, 4);
        map.dispatch(&mut txn).unwrap();
        assert_eq!(txn.data_u64(), i, "device {i} returned wrong value");
    }

    // Verify unmapped address fails
    let mut txn = Transaction::read(100 * 0x1000, 4);
    assert!(map.dispatch(&mut txn).is_err());
}

#[test]
fn detach_removes_regions() {
    let mut map = AddressMap::new();
    let dev = RegDevice::new("uart", 0x1000).with_value(42);
    let id = map.attach("uart", Box::new(dev));
    map.map_region(id, 0x1000, 0x1000, 0);
    map.commit();

    // Verify it works
    assert!(map.lookup(0x1000).is_some());

    // Detach
    let returned = map.detach(id);
    assert!(returned.is_some());
    map.commit();

    // Should no longer be mapped
    assert!(map.lookup(0x1000).is_none());
}

#[test]
fn transactional_batch() {
    let mut map = AddressMap::new();

    // Attach 3 devices and map in a batch before committing
    let d0 = RegDevice::new("a", 0x100).with_value(10);
    let d1 = RegDevice::new("b", 0x100).with_value(20);
    let d2 = RegDevice::new("c", 0x100).with_value(30);

    let id0 = map.attach("a", Box::new(d0));
    let id1 = map.attach("b", Box::new(d1));
    let id2 = map.attach("c", Box::new(d2));

    map.map_region(id0, 0x0000, 0x100, 0);
    map.map_region(id1, 0x1000, 0x100, 0);
    map.map_region(id2, 0x2000, 0x100, 0);

    // Before commit, nothing is mapped
    assert!(map.lookup(0x0000).is_none());

    map.commit();

    // After commit, all three are mapped
    assert!(map.lookup(0x0000).is_some());
    assert!(map.lookup(0x1000).is_some());
    assert!(map.lookup(0x2000).is_some());

    assert_eq!(map.flat_view().len(), 3);
}

#[test]
fn listener_notified() {
    #[derive(Default)]
    struct Recorder {
        adds: Vec<(u64, u64, DeviceId)>,
        removes: Vec<(u64, u64, DeviceId)>,
    }

    // Use Arc<Mutex> for interior mutability
    let recorder = Arc::new(Mutex::new(Recorder::default()));

    struct ListenerAdapter(Arc<Mutex<Recorder>>);
    impl AddressMapListener for ListenerAdapter {
        fn on_region_add(&mut self, start: u64, end: u64, device_id: DeviceId) {
            self.0.lock().unwrap().adds.push((start, end, device_id));
        }
        fn on_region_remove(&mut self, start: u64, end: u64, device_id: DeviceId) {
            self.0.lock().unwrap().removes.push((start, end, device_id));
        }
    }

    let mut map = AddressMap::new();
    map.add_listener(Box::new(ListenerAdapter(recorder.clone())));

    let dev = RegDevice::new("test", 0x100);
    let id = map.attach("test", Box::new(dev));
    map.map_region(id, 0x5000, 0x100, 0);
    map.commit();

    let r = recorder.lock().unwrap();
    assert_eq!(r.adds.len(), 1);
    assert_eq!(r.adds[0], (0x5000, 0x5100, id));
    assert_eq!(r.removes.len(), 0);
    drop(r);

    // Now detach
    map.detach(id);
    map.commit();

    let r = recorder.lock().unwrap();
    assert_eq!(r.removes.len(), 1);
    assert_eq!(r.removes[0], (0x5000, 0x5100, id));
}

#[test]
fn fast_path_parity() {
    let mut map = AddressMap::new();
    let dev = RegDevice::new("uart", 0x1000).with_value(0x1234);
    let id = map.attach("uart", Box::new(dev));
    map.map_region(id, 0x0900_0000, 0x1000, 0);
    map.commit();

    // read_fast should match dispatch result
    let val = map.read_fast(0x0900_0000, 4).unwrap();
    assert_eq!(val, 0x1234);

    // write_fast
    map.write_fast(0x0900_0000, 4, 0x5678).unwrap();
    let val = map.read_fast(0x0900_0000, 4).unwrap();
    assert_eq!(val, 0x5678);
}

#[test]
fn num_devices_and_names() {
    let mut map = AddressMap::new();
    assert_eq!(map.num_devices(), 0);

    let id = map.attach("uart0", Box::new(RegDevice::new("uart0", 0x100)));
    assert_eq!(map.num_devices(), 1);
    assert_eq!(map.device_name(id), Some("uart0"));

    map.detach(id);
    map.commit();
    assert_eq!(map.num_devices(), 0);
}

#[test]
fn unmapped_address_returns_error() {
    let mut map = AddressMap::new();
    let dev = RegDevice::new("uart", 0x100);
    let id = map.attach("uart", Box::new(dev));
    map.map_region(id, 0x1000, 0x100, 0);
    map.commit();

    assert!(map.read_fast(0x2000, 4).is_err());
    assert!(map.write_fast(0x2000, 4, 0).is_err());
}
