use crate::device::*;
use crate::mmio::MemoryMappedDevice;
use crate::region::MemRegion;
use crate::transaction::Transaction;
use helm_core::HelmResult;

struct StubDevice;

impl Device for StubDevice {
    fn transact(&mut self, txn: &mut Transaction) -> HelmResult<()> {
        if !txn.is_write {
            txn.set_data_u64(0x42);
        }
        Ok(())
    }
    fn regions(&self) -> &[MemRegion] {
        &[]
    }
    fn name(&self) -> &str {
        "stub"
    }
}

#[test]
fn default_reset_is_ok() {
    let mut dev = StubDevice;
    assert!(dev.reset().is_ok());
}

#[test]
fn default_checkpoint_returns_null() {
    let dev = StubDevice;
    let val = dev.checkpoint().unwrap();
    assert!(val.is_null());
}

#[test]
fn default_restore_is_ok() {
    let mut dev = StubDevice;
    assert!(dev.restore(&serde_json::Value::Null).is_ok());
}

#[test]
fn default_tick_returns_empty() {
    let mut dev = StubDevice;
    let events = dev.tick(100).unwrap();
    assert!(events.is_empty());
}

#[test]
fn legacy_wrapper_wraps_mmio_device() {
    struct LegacyDev;
    impl MemoryMappedDevice for LegacyDev {
        fn read(&mut self, _offset: u64, _size: usize) -> HelmResult<crate::mmio::DeviceAccess> {
            Ok(crate::mmio::DeviceAccess {
                data: 99,
                stall_cycles: 0,
            })
        }
        fn write(&mut self, _offset: u64, _size: usize, _value: u64) -> HelmResult<u64> {
            Ok(0)
        }
        fn region_size(&self) -> u64 {
            4
        }
        fn device_name(&self) -> &str {
            "legacy"
        }
    }

    let wrapper = LegacyWrapper::new(LegacyDev);
    assert_eq!(wrapper.inner().device_name(), "legacy");
}

#[test]
fn device_event_irq_variant() {
    let evt = DeviceEvent::Irq {
        line: 5,
        assert: true,
    };
    if let DeviceEvent::Irq { line, assert } = evt {
        assert_eq!(line, 5);
        assert!(assert);
    } else {
        panic!("wrong variant");
    }
}

#[test]
fn device_event_dma_complete_variant() {
    let evt = DeviceEvent::DmaComplete { channel: 3 };
    if let DeviceEvent::DmaComplete { channel } = evt {
        assert_eq!(channel, 3);
    } else {
        panic!("wrong variant");
    }
}

#[test]
fn log_level_variants_distinct() {
    let levels = [
        LogLevel::Trace,
        LogLevel::Debug,
        LogLevel::Info,
        LogLevel::Warn,
        LogLevel::Error,
    ];
    for i in 0..levels.len() {
        for j in (i + 1)..levels.len() {
            assert_ne!(levels[i], levels[j]);
        }
    }
}
