use crate::address_map::AddressMap;
use crate::device::Device;
use crate::device_ctx::DeviceCtx;
use crate::irq::IrqRouter;
use crate::region::{MemRegion, RegionKind};
use crate::transaction::Transaction;
use helm_core::HelmResult;

struct StubDev {
    region: MemRegion,
}

impl StubDev {
    fn new(size: u64) -> Self {
        Self {
            region: MemRegion {
                name: "stub".into(),
                base: 0,
                size,
                kind: RegionKind::Io,
                priority: 0,
            },
        }
    }
}

impl Device for StubDev {
    fn transact(&mut self, txn: &mut Transaction) -> HelmResult<()> {
        if !txn.is_write {
            txn.set_data_u64(0xAA);
        }
        Ok(())
    }
    fn regions(&self) -> &[MemRegion] {
        std::slice::from_ref(&self.region)
    }
    fn name(&self) -> &str {
        "stub"
    }
}

#[test]
fn realize_maps_region() {
    let mut map = AddressMap::new();
    let mut router = IrqRouter::new();

    let dev = StubDev::new(0x100);
    let id = map.attach("dev", Box::new(dev));

    let mut ctx = DeviceCtx::new(id, &mut map, &mut router);
    let _handle = ctx.map_region(0x5000, 0x100, 0);

    // Commit and verify
    map.commit();
    assert!(map.lookup(0x5000).is_some());
    assert_eq!(map.read_fast(0x5000, 4).unwrap(), 0xAA);
}

#[test]
fn unrealize_unmaps_region() {
    let mut map = AddressMap::new();
    let mut router = IrqRouter::new();

    let dev = StubDev::new(0x100);
    let id = map.attach("dev", Box::new(dev));

    // Realize
    let handle = {
        let mut ctx = DeviceCtx::new(id, &mut map, &mut router);
        ctx.map_region(0x5000, 0x100, 0)
    };
    map.commit();
    assert!(map.lookup(0x5000).is_some());

    // Unrealize
    {
        let mut ctx = DeviceCtx::new(id, &mut map, &mut router);
        ctx.unmap_region(handle);
    }
    map.commit();
    assert!(map.lookup(0x5000).is_none());
}

#[test]
fn irq_lifecycle() {
    let mut map = AddressMap::new();
    let mut router = IrqRouter::new();

    let dev = StubDev::new(0x100);
    let id = map.attach("dev", Box::new(dev));

    // Connect IRQ during realize
    let route_idx = {
        let mut ctx = DeviceCtx::new(id, &mut map, &mut router);
        ctx.connect_irq(0, 0, 33)
    };

    assert_eq!(router.routes().len(), 1);
    assert_eq!(router.routes()[0].dest_irq, 33);

    // Disconnect during unrealize
    {
        let mut ctx = DeviceCtx::new(id, &mut map, &mut router);
        ctx.disconnect_irq(route_idx);
    }
    assert!(router.routes().is_empty());
}

#[test]
fn disconnect_all_irqs() {
    let mut map = AddressMap::new();
    let mut router = IrqRouter::new();

    let dev = StubDev::new(0x100);
    let id = map.attach("dev", Box::new(dev));

    // Connect multiple IRQs
    {
        let mut ctx = DeviceCtx::new(id, &mut map, &mut router);
        ctx.connect_irq(0, 0, 33);
        ctx.connect_irq(1, 0, 34);
    }
    assert_eq!(router.routes().len(), 2);

    // Disconnect all
    {
        let mut ctx = DeviceCtx::new(id, &mut map, &mut router);
        ctx.disconnect_all_irqs();
    }
    assert!(router.routes().is_empty());
}
