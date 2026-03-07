use crate::device::Device;
use crate::virtio::transport::*;
use crate::virtio::features::*;
use crate::virtio::queue::*;

// ── Helper ──────────────────────────────────────────────────────────────────

fn check_device(backend: Box<dyn VirtioDeviceBackend>, expected_id: u32, expected_name: &str) {
    assert_eq!(backend.device_id(), expected_id);
    assert_eq!(backend.name(), expected_name);
    assert!(backend.device_features() & VIRTIO_F_VERSION_1 != 0);
    assert!(backend.num_queues() > 0);

    let transport = VirtioMmioTransport::new(backend);
    assert_eq!(transport.queues().len() as u16, transport.backend().num_queues());
}

// ── Block device ────────────────────────────────────────────────────────────

#[test]
fn blk_device_id() {
    use crate::virtio::blk::*;
    check_device(Box::new(VirtioBlk::new(4096)), VIRTIO_DEV_BLK, "virtio-blk");
}

#[test]
fn blk_config_capacity() {
    use crate::virtio::blk::*;
    let blk = VirtioBlk::new(1024 * 1024);
    // First 8 bytes of config = capacity in sectors
    let cap_low = blk.read_config(0) as u64
        | (blk.read_config(1) as u64) << 8
        | (blk.read_config(2) as u64) << 16
        | (blk.read_config(3) as u64) << 24;
    assert_eq!(cap_low, 2048); // 1MB / 512
}

#[test]
fn blk_readonly() {
    use crate::virtio::blk::*;
    let blk = VirtioBlk::new_readonly(vec![0u8; 4096]);
    assert!(blk.device_features() & VIRTIO_BLK_F_RO != 0);
}

#[test]
fn blk_read_write_sectors() {
    use crate::virtio::blk::*;
    let mut blk = VirtioBlk::new(4096);
    assert!(blk.write_sectors(0, &[0xAA; 512]));
    let data = blk.read_sectors(0, 1).unwrap();
    assert_eq!(data[0], 0xAA);
    assert_eq!(data.len(), 512);
}

#[test]
fn blk_queue_notify() {
    use crate::virtio::blk::*;
    let mut blk = VirtioBlk::new(4096);
    let mut queues = vec![Virtqueue::new_split(16)];
    let q = queues[0].as_split_mut().unwrap();
    q.set_desc(0, 0, 16, 0, 0);
    q.push_avail(0);
    blk.queue_notify(0, &mut queues);
    assert_eq!(queues[0].as_split().unwrap().used_idx, 1);
}

// ── Network device ──────────────────────────────────────────────────────────

#[test]
fn net_device_id() {
    use crate::virtio::net::*;
    check_device(Box::new(VirtioNet::new()), VIRTIO_DEV_NET, "virtio-net");
}

#[test]
fn net_default_mac() {
    use crate::virtio::net::*;
    let net = VirtioNet::new();
    assert_eq!(net.read_config(0), 0x52);
    assert_eq!(net.read_config(1), 0x54);
    assert_eq!(net.read_config(2), 0x00);
}

#[test]
fn net_custom_mac() {
    use crate::virtio::net::*;
    let net = VirtioNet::new().with_mac([0xDE, 0xAD, 0xBE, 0xEF, 0x00, 0x01]);
    assert_eq!(net.read_config(0), 0xDE);
    assert_eq!(net.read_config(1), 0xAD);
}

#[test]
fn net_three_queues() {
    use crate::virtio::net::*;
    let net = VirtioNet::new();
    assert_eq!(net.num_queues(), 3); // rx, tx, ctrl
}

#[test]
fn net_tx_drain() {
    use crate::virtio::net::*;
    let mut net = VirtioNet::new();
    let mut queues = vec![Virtqueue::new_split(16); 3];
    let q = queues[1].as_split_mut().unwrap();
    q.set_desc(0, 0x1000, 64, 0, 0);
    q.push_avail(0);
    net.queue_notify(1, &mut queues);
    assert_eq!(net.tx_queue.len(), 1);
    let packets = net.drain_tx();
    assert_eq!(packets.len(), 1);
    assert!(net.tx_queue.is_empty());
}

// ── Console device ──────────────────────────────────────────────────────────

#[test]
fn console_device_id() {
    use crate::virtio::console::*;
    check_device(Box::new(VirtioConsole::new()), VIRTIO_DEV_CONSOLE, "virtio-console");
}

#[test]
fn console_config_size() {
    use crate::virtio::console::*;
    let console = VirtioConsole::new().with_size(132, 43);
    assert_eq!(console.read_config(0), 132u16.to_le_bytes()[0]);
    assert_eq!(console.read_config(1), 132u16.to_le_bytes()[1]);
    assert_eq!(console.read_config(2), 43u16.to_le_bytes()[0]);
}

#[test]
fn console_multiport() {
    use crate::virtio::console::*;
    let console = VirtioConsole::new().with_multiport(4);
    assert!(console.device_features() & VIRTIO_CONSOLE_F_MULTIPORT != 0);
    assert_eq!(console.num_queues(), 4);
}

// ── RNG device ──────────────────────────────────────────────────────────────

#[test]
fn rng_device_id() {
    use crate::virtio::rng::*;
    check_device(Box::new(VirtioRng::new()), VIRTIO_DEV_RNG, "virtio-rng");
}

#[test]
fn rng_no_config() {
    use crate::virtio::rng::*;
    let rng = VirtioRng::new();
    assert_eq!(rng.config_size(), 0);
}

#[test]
fn rng_queue_notify() {
    use crate::virtio::rng::*;
    let mut rng = VirtioRng::new();
    let mut queues = vec![Virtqueue::new_split(16)];
    let q = queues[0].as_split_mut().unwrap();
    q.set_desc(0, 0x1000, 64, VRING_DESC_F_WRITE, 0);
    q.push_avail(0);
    rng.queue_notify(0, &mut queues);
    assert_eq!(queues[0].as_split().unwrap().used_idx, 1);
    assert_eq!(queues[0].as_split().unwrap().used_ring[0].len, 64);
}

// ── Balloon device ──────────────────────────────────────────────────────────

#[test]
fn balloon_device_id() {
    use crate::virtio::balloon::*;
    check_device(Box::new(VirtioBalloon::new()), VIRTIO_DEV_BALLOON, "virtio-balloon");
}

#[test]
fn balloon_set_target() {
    use crate::virtio::balloon::*;
    let mut b = VirtioBalloon::new();
    b.set_target(1000);
    let low = b.read_config(0) as u32
        | (b.read_config(1) as u32) << 8
        | (b.read_config(2) as u32) << 16
        | (b.read_config(3) as u32) << 24;
    assert_eq!(low, 1000);
}

// ── SCSI host ───────────────────────────────────────────────────────────────

#[test]
fn scsi_device_id() {
    use crate::virtio::scsi::*;
    check_device(Box::new(VirtioScsi::new()), VIRTIO_DEV_SCSI, "virtio-scsi");
}

#[test]
fn scsi_queue_count() {
    use crate::virtio::scsi::*;
    let scsi = VirtioScsi::new().with_num_queues(4);
    assert_eq!(scsi.num_queues(), 6); // ctrl + event + 4 request
}

// ── GPU device ──────────────────────────────────────────────────────────────

#[test]
fn gpu_device_id() {
    use crate::virtio::gpu::*;
    check_device(Box::new(VirtioGpu::new(2)), VIRTIO_DEV_GPU, "virtio-gpu");
}

#[test]
fn gpu_scanout_count() {
    use crate::virtio::gpu::*;
    let gpu = VirtioGpu::new(4);
    assert_eq!(gpu.scanouts.len(), 4);
}

// ── Input device ────────────────────────────────────────────────────────────

#[test]
fn input_keyboard() {
    use crate::virtio::input::*;
    check_device(Box::new(VirtioInput::keyboard()), VIRTIO_DEV_INPUT, "virtio-input");
}

#[test]
fn input_mouse() {
    use crate::virtio::input::*;
    let input = VirtioInput::mouse();
    assert_eq!(input.device_id(), VIRTIO_DEV_INPUT);
}

#[test]
fn input_inject_event() {
    use crate::virtio::input::*;
    let mut input = VirtioInput::keyboard();
    input.inject_key(30); // 'A' key
    assert_eq!(input.event_queue.len(), 4); // press + syn + release + syn
}

// ── Crypto device ───────────────────────────────────────────────────────────

#[test]
fn crypto_device_id() {
    use crate::virtio::crypto::*;
    check_device(Box::new(VirtioCrypto::new()), VIRTIO_DEV_CRYPTO, "virtio-crypto");
}

// ── Vsock device ────────────────────────────────────────────────────────────

#[test]
fn vsock_device_id() {
    use crate::virtio::vsock::*;
    check_device(Box::new(VirtioVsock::new(3)), VIRTIO_DEV_VSOCK, "virtio-vsock");
}

#[test]
fn vsock_config_cid() {
    use crate::virtio::vsock::*;
    let vsock = VirtioVsock::new(42);
    let cid_low = vsock.read_config(0) as u64
        | (vsock.read_config(1) as u64) << 8
        | (vsock.read_config(2) as u64) << 16
        | (vsock.read_config(3) as u64) << 24;
    assert_eq!(cid_low, 42);
}

// ── Filesystem device ───────────────────────────────────────────────────────

#[test]
fn fs_device_id() {
    use crate::virtio::fs::*;
    check_device(Box::new(VirtioFs::new("myfs")), VIRTIO_DEV_FS, "virtio-fs");
}

#[test]
fn fs_tag() {
    use crate::virtio::fs::*;
    let fs = VirtioFs::new("testfs");
    assert_eq!(fs.read_config(0), b't');
    assert_eq!(fs.read_config(1), b'e');
}

// ── PMEM device ─────────────────────────────────────────────────────────────

#[test]
fn pmem_device_id() {
    use crate::virtio::pmem::*;
    check_device(Box::new(VirtioPmem::new(0x8000_0000, 0x1000_0000)), VIRTIO_DEV_PMEM, "virtio-pmem");
}

// ── IOMMU device ────────────────────────────────────────────────────────────

#[test]
fn iommu_device_id() {
    use crate::virtio::iommu::*;
    check_device(Box::new(VirtioIommu::new()), VIRTIO_DEV_IOMMU, "virtio-iommu");
}

// ── Sound device ────────────────────────────────────────────────────────────

#[test]
fn sound_device_id() {
    use crate::virtio::sound::*;
    check_device(Box::new(VirtioSound::default()), VIRTIO_DEV_SOUND, "virtio-sound");
}

// ── GPIO device ─────────────────────────────────────────────────────────────

#[test]
fn gpio_device_id() {
    use crate::virtio::gpio::*;
    check_device(Box::new(VirtioGpio::new(32)), VIRTIO_DEV_GPIO, "virtio-gpio");
}

#[test]
fn gpio_pin_set_get() {
    use crate::virtio::gpio::*;
    let mut gpio = VirtioGpio::new(8);
    gpio.set_pin(3, 1);
    assert_eq!(gpio.get_pin(3), 1);
    assert_eq!(gpio.get_pin(0), 0);
}

// ── I2C adapter ─────────────────────────────────────────────────────────────

#[test]
fn i2c_device_id() {
    use crate::virtio::i2c::*;
    check_device(Box::new(VirtioI2c::new()), VIRTIO_DEV_I2C, "virtio-i2c");
}

// ── SCMI device ─────────────────────────────────────────────────────────────

#[test]
fn scmi_device_id() {
    use crate::virtio::scmi::*;
    check_device(Box::new(VirtioScmi::new()), VIRTIO_DEV_SCMI, "virtio-scmi");
}

// ── Memory device ───────────────────────────────────────────────────────────

#[test]
fn mem_device_id() {
    use crate::virtio::mem::*;
    check_device(Box::new(VirtioMem::new(0x1_0000_0000, 0x4000_0000, 0x200_0000)), VIRTIO_DEV_MEM, "virtio-mem");
}

// ── Watchdog device ─────────────────────────────────────────────────────────

#[test]
fn watchdog_device_id() {
    use crate::virtio::watchdog::*;
    check_device(Box::new(VirtioWatchdog::new(5000)), VIRTIO_DEV_WATCHDOG, "virtio-watchdog");
}

#[test]
fn watchdog_kick_resets_counter() {
    use crate::virtio::watchdog::*;
    let mut wd = VirtioWatchdog::new(100);
    wd.armed = true;
    wd.ticks_since_kick = 50;
    wd.kick();
    assert_eq!(wd.ticks_since_kick, 0);
}

#[test]
fn watchdog_expiry() {
    use crate::virtio::watchdog::*;
    let mut wd = VirtioWatchdog::new(100);
    wd.armed = true;
    wd.ticks_since_kick = 101;
    assert!(wd.is_expired());
}

// ── CAN device ──────────────────────────────────────────────────────────────

#[test]
fn can_device_id() {
    use crate::virtio::can::*;
    check_device(Box::new(VirtioCan::new()), VIRTIO_DEV_CAN, "virtio-can");
}

// ── Bluetooth device ────────────────────────────────────────────────────────

#[test]
fn bt_device_id() {
    use crate::virtio::bt::*;
    check_device(Box::new(VirtioBt::new()), VIRTIO_DEV_BT, "virtio-bt");
}

// ── Video encoder ───────────────────────────────────────────────────────────

#[test]
fn video_enc_device_id() {
    use crate::virtio::video::*;
    check_device(Box::new(VirtioVideoEncoder::new()), VIRTIO_DEV_VIDEO_ENC, "virtio-video-enc");
}

// ── Video decoder ───────────────────────────────────────────────────────────

#[test]
fn video_dec_device_id() {
    use crate::virtio::video::*;
    check_device(Box::new(VirtioVideoDecoder::new()), VIRTIO_DEV_VIDEO_DEC, "virtio-video-dec");
}

// ── Transport integration ───────────────────────────────────────────────────

#[test]
fn all_devices_wrap_in_transport() {
    use crate::virtio::*;
    let devices: Vec<Box<dyn VirtioDeviceBackend>> = vec![
        Box::new(VirtioBlk::new(4096)),
        Box::new(VirtioNet::new()),
        Box::new(VirtioConsole::new()),
        Box::new(VirtioRng::new()),
        Box::new(VirtioBalloon::new()),
        Box::new(VirtioScsi::new()),
        Box::new(VirtioGpu::new(1)),
        Box::new(VirtioInput::keyboard()),
        Box::new(VirtioCrypto::new()),
        Box::new(VirtioVsock::new(3)),
        Box::new(VirtioFs::new("test")),
        Box::new(VirtioPmem::new(0, 0x1000)),
        Box::new(VirtioIommu::new()),
        Box::new(VirtioSound::default()),
        Box::new(VirtioGpio::new(8)),
        Box::new(VirtioI2c::new()),
        Box::new(VirtioScmi::new()),
        Box::new(VirtioMem::new(0, 0x1000, 0x200)),
        Box::new(VirtioWatchdog::new(1000)),
        Box::new(VirtioCan::new()),
        Box::new(VirtioBt::new()),
        Box::new(VirtioVideoEncoder::new()),
        Box::new(VirtioVideoDecoder::new()),
    ];

    for backend in devices {
        let name = backend.name().to_string();
        let transport = VirtioMmioTransport::new(backend);
        assert!(transport.queues().len() > 0, "device {} has no queues", name);
        assert!(!transport.regions().is_empty(), "device {} has no regions", name);
    }
}
