use crate::device::Device;
use crate::transaction::Transaction;
use crate::virtio::blk::VirtioBlk;
use crate::virtio::features::*;
use crate::virtio::rng::VirtioRng;
use crate::virtio::transport::*;

fn make_rng() -> VirtioMmioTransport {
    VirtioMmioTransport::new(Box::new(VirtioRng::new()))
}

fn read_reg(dev: &mut VirtioMmioTransport, offset: u64) -> u32 {
    let mut txn = Transaction::read(0, 4);
    txn.offset = offset;
    Device::transact(dev, &mut txn).unwrap();
    txn.data_u32()
}

fn write_reg(dev: &mut VirtioMmioTransport, offset: u64, value: u32) {
    let mut txn = Transaction::write(0, 4, value as u64);
    txn.offset = offset;
    Device::transact(dev, &mut txn).unwrap();
}

#[test]
fn magic_value() {
    let mut t = make_rng();
    assert_eq!(read_reg(&mut t, 0x000), VIRTIO_MMIO_MAGIC);
}

#[test]
fn version_is_2() {
    let mut t = make_rng();
    assert_eq!(read_reg(&mut t, 0x004), VIRTIO_MMIO_VERSION);
}

#[test]
fn device_id_rng() {
    let mut t = make_rng();
    assert_eq!(read_reg(&mut t, 0x008), VIRTIO_DEV_RNG);
}

#[test]
fn device_id_blk() {
    let mut t = VirtioMmioTransport::new(Box::new(VirtioBlk::new(4096)));
    assert_eq!(read_reg(&mut t, 0x008), VIRTIO_DEV_BLK);
}

#[test]
fn vendor_id() {
    let mut t = make_rng();
    assert_eq!(read_reg(&mut t, 0x00C), HELM_VIRTIO_VENDOR_ID);
}

#[test]
fn initial_status_zero() {
    let mut t = make_rng();
    assert_eq!(read_reg(&mut t, 0x070), 0);
}

#[test]
fn status_lifecycle() {
    let mut t = make_rng();

    // ACKNOWLEDGE
    write_reg(&mut t, 0x070, VIRTIO_STATUS_ACKNOWLEDGE as u32);
    assert_eq!(read_reg(&mut t, 0x070), VIRTIO_STATUS_ACKNOWLEDGE as u32);

    // DRIVER
    let s = VIRTIO_STATUS_ACKNOWLEDGE | VIRTIO_STATUS_DRIVER;
    write_reg(&mut t, 0x070, s as u32);
    assert_eq!(read_reg(&mut t, 0x070), s as u32);

    // FEATURES_OK
    let s = s | VIRTIO_STATUS_FEATURES_OK;
    write_reg(&mut t, 0x070, s as u32);

    // DRIVER_OK
    let s = s | VIRTIO_STATUS_DRIVER_OK;
    write_reg(&mut t, 0x070, s as u32);
    assert_eq!(t.status(), s);
}

#[test]
fn reset_by_writing_zero() {
    let mut t = make_rng();
    write_reg(&mut t, 0x070, VIRTIO_STATUS_ACKNOWLEDGE as u32);
    assert_ne!(t.status(), 0);
    write_reg(&mut t, 0x070, 0);
    assert_eq!(t.status(), 0);
}

#[test]
fn feature_negotiation_low_bits() {
    let mut t = make_rng();
    write_reg(&mut t, 0x014, 0); // DeviceFeaturesSel = 0
    let low = read_reg(&mut t, 0x010);
    // RNG only has VERSION_1 (bit 32), so low bits should be 0
    assert_eq!(low, 0);
}

#[test]
fn feature_negotiation_high_bits() {
    let mut t = make_rng();
    write_reg(&mut t, 0x014, 1); // DeviceFeaturesSel = 1
    let high = read_reg(&mut t, 0x010);
    // VERSION_1 = bit 32 → bit 0 in high word
    assert!(high & 1 != 0);
}

#[test]
fn driver_features_write() {
    let mut t = make_rng();
    // Write low bits
    write_reg(&mut t, 0x024, 0); // DriverFeaturesSel = 0
    write_reg(&mut t, 0x020, 0xABCD);
    assert_eq!(t.driver_features() & 0xFFFF_FFFF, 0xABCD);

    // Write high bits
    write_reg(&mut t, 0x024, 1); // DriverFeaturesSel = 1
    write_reg(&mut t, 0x020, 0x0001); // VERSION_1
    assert!(t.driver_features() & VIRTIO_F_VERSION_1 != 0);
}

#[test]
fn queue_num_max() {
    let mut t = make_rng();
    write_reg(&mut t, 0x030, 0); // QueueSel = 0
    let max = read_reg(&mut t, 0x034);
    assert!(max > 0);
}

#[test]
fn queue_ready() {
    let mut t = make_rng();
    write_reg(&mut t, 0x030, 0); // QueueSel = 0
    assert_eq!(read_reg(&mut t, 0x044), 0);
    write_reg(&mut t, 0x044, 1);
    assert_eq!(read_reg(&mut t, 0x044), 1);
}

#[test]
fn interrupt_status_and_ack() {
    let mut t = make_rng();
    t.raise_irq();
    assert_eq!(read_reg(&mut t, 0x060), VIRTIO_IRQ_VQUEUE);
    write_reg(&mut t, 0x064, VIRTIO_IRQ_VQUEUE);
    assert_eq!(read_reg(&mut t, 0x060), 0);
}

#[test]
fn config_change_irq() {
    let mut t = make_rng();
    t.raise_config_irq();
    assert_eq!(read_reg(&mut t, 0x060), VIRTIO_IRQ_CONFIG);
}

#[test]
fn blk_config_space_capacity() {
    let mut t = VirtioMmioTransport::new(Box::new(VirtioBlk::new(1024 * 1024)));
    // Config at 0x100, capacity is first 8 bytes
    let low = read_reg(&mut t, 0x100);
    let high = read_reg(&mut t, 0x104);
    let capacity = low as u64 | ((high as u64) << 32);
    assert_eq!(capacity, 1024 * 1024 / 512); // 2048 sectors
}

#[test]
fn device_trait_name() {
    let t = make_rng();
    assert_eq!(t.name(), "virtio-rng");
}

#[test]
fn device_trait_regions() {
    let t = make_rng();
    let regions = t.regions();
    assert_eq!(regions.len(), 1);
    assert_eq!(regions[0].size, 0x200);
}

#[test]
fn device_trait_reset() {
    let mut t = make_rng();
    write_reg(&mut t, 0x070, VIRTIO_STATUS_ACKNOWLEDGE as u32);
    Device::reset(&mut t).unwrap();
    assert_eq!(t.status(), 0);
}

#[test]
fn queue_desc_addr_setup() {
    let mut t = make_rng();
    write_reg(&mut t, 0x030, 0); // QueueSel = 0
    write_reg(&mut t, 0x080, 0x1000); // desc low
    write_reg(&mut t, 0x084, 0x0); // desc high
    write_reg(&mut t, 0x090, 0x2000); // avail low
    write_reg(&mut t, 0x094, 0x0); // avail high
    write_reg(&mut t, 0x0A0, 0x3000); // used low
    write_reg(&mut t, 0x0A4, 0x0); // used high

    let q = t.queues()[0].as_split().unwrap();
    assert_eq!(q.desc_addr, 0x1000);
    assert_eq!(q.avail_addr, 0x2000);
    assert_eq!(q.used_addr, 0x3000);
}

#[test]
fn queue_reset() {
    let mut t = make_rng();
    write_reg(&mut t, 0x030, 0); // QueueSel = 0
    write_reg(&mut t, 0x044, 1); // QueueReady = 1
    assert_eq!(read_reg(&mut t, 0x044), 1);
    write_reg(&mut t, 0x0C0, 1); // QueueReset
    assert_eq!(read_reg(&mut t, 0x044), 0);
}

#[test]
fn transact_adds_stall_cycle() {
    let mut t = make_rng();
    let mut txn = Transaction::read(0, 4);
    txn.offset = 0x000;
    Device::transact(&mut t, &mut txn).unwrap();
    assert_eq!(txn.stall_cycles, 1);
}
