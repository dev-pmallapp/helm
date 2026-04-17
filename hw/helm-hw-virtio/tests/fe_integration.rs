//! FE (Functional Execution) integration tests for VirtIO devices.
//!
//! These tests exercise VirtIO devices through the same object stack a CPU
//! would use in FE/FS mode:
//!
//! ```text
//!   (test code acts as the CPU)
//!     └─► HelmAddressSpace::read / write(addr, size, AccessType)
//!               │
//!               ├─► AddressMap hit → VirtioMmioTransport (MMIO region)
//!               │     └─► VirtioBackend (blk / rng / console / net)
//!               │
//!               └─► FlatMem (RAM — descriptor tables, rings, data buffers)
//! ```
//!
//! Virtqueue rings live in RAM; the device processes them when the test
//! writes to the QUEUE_NOTIFY MMIO register — exactly as a Linux driver would.

#![allow(dead_code)]

use helm_core::{AccessType, MemInterface};
use helm_devices::BufferCharBackend;
use helm_memory::{FlatMem, HelmAddressSpace};

use helm_hw_virtio::{
    blk::VirtioBlk,
    console::VirtioConsole,
    net::{VirtioNet, VIRTIO_NET_HDR_SIZE},
    proto::{
        features::*,
        transport::{
            VirtioMmioTransport, STATUS_ACKNOWLEDGE, STATUS_DRIVER, STATUS_DRIVER_OK,
            STATUS_FEATURES_OK,
        },
        virtqueue::{RamBlockBackend, VirtQueue},
    },
    rng::VirtioRng,
    VirtioBackend,
};

// ── Address constants ─────────────────────────────────────────────────────────

/// Base address for the primary VirtIO device under test (QEMU virt slot 0).
const VIRTIO_BASE: u64 = 0x0A00_0000;

/// RAM base (where descriptor tables, rings, and data buffers live).
const RAM_BASE: u64 = 0x4000_0000;

// ── MMIO register offsets (§4.2.2) ───────────────────────────────────────────

const REG_MAGIC: u64 = 0x000;
const REG_VERSION: u64 = 0x004;
const REG_DEVICE_ID: u64 = 0x008;
const REG_VENDOR_ID: u64 = 0x00C;
const REG_DEVICE_FEATURES: u64 = 0x010;
const REG_DEVICE_FEAT_SEL: u64 = 0x014;
const REG_DRIVER_FEATURES: u64 = 0x020;
const REG_DRIVER_FEAT_SEL: u64 = 0x024;
const REG_QUEUE_SEL: u64 = 0x030;
const REG_QUEUE_NUM_MAX: u64 = 0x034;
const REG_QUEUE_NUM: u64 = 0x038;
const REG_QUEUE_READY: u64 = 0x044;
const REG_QUEUE_NOTIFY: u64 = 0x050;
const REG_INTERRUPT_STATUS: u64 = 0x060;
const REG_INTERRUPT_ACK: u64 = 0x064;
const REG_STATUS: u64 = 0x070;
const REG_QUEUE_DESC_LO: u64 = 0x080;
const REG_QUEUE_DESC_HI: u64 = 0x084;
const REG_QUEUE_DRIVER_LO: u64 = 0x090;
const REG_QUEUE_DRIVER_HI: u64 = 0x094;
const REG_QUEUE_DEVICE_LO: u64 = 0x0A0;
const REG_QUEUE_DEVICE_HI: u64 = 0x0A4;
const REG_CONFIG_SPACE: u64 = 0x100;

// ── RAM layout for virtqueue tests ────────────────────────────────────────────
//
//   RAM_BASE + 0x0000  descriptor table (16 bytes × QUEUE_SIZE)
//   RAM_BASE + 0x0200  available ring   (4 + QUEUE_SIZE×2 bytes)
//   RAM_BASE + 0x0400  used ring        (4 + QUEUE_SIZE×8 bytes)
//   RAM_BASE + 0x1000  data buffer      (up to 4 KiB)
//   RAM_BASE + 0x2000  request header   (blk: 16 bytes)
//   RAM_BASE + 0x3000  status byte      (blk: 1 byte)

const DESC_BASE: u64 = RAM_BASE + 0x0000;
const AVAIL_BASE: u64 = RAM_BASE + 0x0200;
const USED_BASE: u64 = RAM_BASE + 0x0400;
const DATA_BUFFER: u64 = RAM_BASE + 0x1000;
const HDR_BUFFER: u64 = RAM_BASE + 0x2000;
const STATUS_BYTE: u64 = RAM_BASE + 0x3000;

const QUEUE_SIZE: u16 = 16;

// ── Helpers ───────────────────────────────────────────────────────────────────

fn make_sys() -> HelmAddressSpace {
    HelmAddressSpace::new(FlatMem::new(RAM_BASE, 16 * 1024 * 1024))
}

fn attach(sys: &mut HelmAddressSpace, backend: Box<dyn VirtioBackend>) -> usize {
    sys.add_device(VIRTIO_BASE, Box::new(VirtioMmioTransport::new(backend)))
}

fn process_transport(
    sys: &mut HelmAddressSpace,
    idx: usize,
) -> helm_hw_virtio::VirtioPendingEvents {
    let sys_ptr: *mut HelmAddressSpace = sys;
    let transport = std::ptr::from_mut::<VirtioMmioTransport>(
        sys.device_as_mut::<VirtioMmioTransport>(idx)
            .expect("virtio transport must be present"),
    );
    // SAFETY: This helper is test-only and invokes process_pending() on one
    // known transport using the same address-space object that owns it. The
    // transport only accesses guest memory and its own internal queue state.
    #[allow(unsafe_code)]
    unsafe {
        (*transport).process_pending(&mut *sys_ptr)
    }
}

fn mmio_read(sys: &mut HelmAddressSpace, reg: u64) -> u32 {
    sys.read(VIRTIO_BASE + reg, 4, AccessType::Load).unwrap() as u32
}

fn mmio_write(sys: &mut HelmAddressSpace, reg: u64, val: u32) {
    sys.write(VIRTIO_BASE + reg, 4, val as u64, AccessType::Store)
        .unwrap();
}

fn ram_read8(sys: &mut HelmAddressSpace, addr: u64) -> u8 {
    sys.read(addr, 1, AccessType::Load).unwrap() as u8
}

fn ram_read16(sys: &mut HelmAddressSpace, addr: u64) -> u16 {
    sys.read(addr, 2, AccessType::Load).unwrap() as u16
}

fn ram_read32(sys: &mut HelmAddressSpace, addr: u64) -> u32 {
    sys.read(addr, 4, AccessType::Load).unwrap() as u32
}

fn ram_write8(sys: &mut HelmAddressSpace, addr: u64, val: u8) {
    sys.write(addr, 1, val as u64, AccessType::Store).unwrap();
}

fn ram_write16(sys: &mut HelmAddressSpace, addr: u64, val: u16) {
    sys.write(addr, 2, val as u64, AccessType::Store).unwrap();
}

fn ram_write32(sys: &mut HelmAddressSpace, addr: u64, val: u32) {
    sys.write(addr, 4, val as u64, AccessType::Store).unwrap();
}

fn ram_write64(sys: &mut HelmAddressSpace, addr: u64, val: u64) {
    sys.write(addr, 8, val, AccessType::Store).unwrap();
}

/// Simulate the standard VirtIO driver negotiation sequence via MMIO.
fn driver_negotiate(sys: &mut HelmAddressSpace, features_lo: u32, features_hi: u32) {
    mmio_write(sys, REG_STATUS, STATUS_ACKNOWLEDGE);
    mmio_write(sys, REG_STATUS, STATUS_ACKNOWLEDGE | STATUS_DRIVER);
    mmio_write(sys, REG_DRIVER_FEAT_SEL, 0);
    mmio_write(sys, REG_DRIVER_FEATURES, features_lo);
    mmio_write(sys, REG_DRIVER_FEAT_SEL, 1);
    mmio_write(sys, REG_DRIVER_FEATURES, features_hi);
    mmio_write(
        sys,
        REG_STATUS,
        STATUS_ACKNOWLEDGE | STATUS_DRIVER | STATUS_FEATURES_OK,
    );
    mmio_write(
        sys,
        REG_STATUS,
        STATUS_ACKNOWLEDGE | STATUS_DRIVER | STATUS_FEATURES_OK | STATUS_DRIVER_OK,
    );
}

/// Set up queue 0 in the transport via MMIO writes.
fn setup_queue0(sys: &mut HelmAddressSpace) {
    setup_queue(sys, 0, DESC_BASE, AVAIL_BASE, USED_BASE);
}

fn setup_queue(sys: &mut HelmAddressSpace, queue_sel: u32, desc: u64, avail: u64, used: u64) {
    mmio_write(sys, REG_QUEUE_SEL, queue_sel);
    mmio_write(sys, REG_QUEUE_NUM, QUEUE_SIZE as u32);
    mmio_write(sys, REG_QUEUE_DESC_LO, (desc & 0xFFFF_FFFF) as u32);
    mmio_write(sys, REG_QUEUE_DESC_HI, (desc >> 32) as u32);
    mmio_write(sys, REG_QUEUE_DRIVER_LO, (avail & 0xFFFF_FFFF) as u32);
    mmio_write(sys, REG_QUEUE_DRIVER_HI, (avail >> 32) as u32);
    mmio_write(sys, REG_QUEUE_DEVICE_LO, (used & 0xFFFF_FFFF) as u32);
    mmio_write(sys, REG_QUEUE_DEVICE_HI, (used >> 32) as u32);
    mmio_write(sys, REG_QUEUE_READY, 1);
}

/// Write a descriptor into the RAM descriptor table at `idx`.
fn write_desc(sys: &mut HelmAddressSpace, idx: u16, addr: u64, len: u32, flags: u16, next: u16) {
    let base = DESC_BASE + idx as u64 * 16;
    ram_write64(sys, base, addr);
    ram_write32(sys, base + 8, len);
    ram_write16(sys, base + 12, flags);
    ram_write16(sys, base + 14, next);
}

/// Push `desc_head` into the available ring and advance avail_idx.
fn avail_push(sys: &mut HelmAddressSpace, desc_head: u16) {
    let idx = ram_read16(sys, AVAIL_BASE + 2);
    let slot = (idx % QUEUE_SIZE) as u64;
    ram_write16(sys, AVAIL_BASE + 4 + slot * 2, desc_head);
    ram_write16(sys, AVAIL_BASE + 2, idx.wrapping_add(1));
}

/// Read the used ring idx (number of completed entries posted by device).
fn used_idx(sys: &mut HelmAddressSpace) -> u16 {
    ram_read16(sys, USED_BASE + 2)
}

// ── Tests: MMIO identification ────────────────────────────────────────────────

#[test]
fn magic_and_version_are_correct() {
    let mut sys = make_sys();
    attach(&mut sys, Box::new(VirtioRng::new()));

    assert_eq!(
        mmio_read(&mut sys, REG_MAGIC),
        0x7472_6976,
        "magic = \"virt\" LE"
    );
    assert_eq!(mmio_read(&mut sys, REG_VERSION), 2, "MMIO transport v2");
}

#[test]
fn device_id_blk() {
    let mut sys = make_sys();
    attach(
        &mut sys,
        Box::new(VirtioBlk::new(
            Box::new(RamBlockBackend::zeroed(4096)),
            false,
        )),
    );
    assert_eq!(mmio_read(&mut sys, REG_DEVICE_ID), VIRTIO_DEVICE_BLK);
    assert_eq!(mmio_read(&mut sys, REG_VENDOR_ID), VIRTIO_VENDOR_ID as u32);
}

#[test]
fn device_id_rng() {
    let mut sys = make_sys();
    attach(&mut sys, Box::new(VirtioRng::new()));
    assert_eq!(mmio_read(&mut sys, REG_DEVICE_ID), VIRTIO_DEVICE_RNG);
}

#[test]
fn device_id_console() {
    let mut sys = make_sys();
    attach(
        &mut sys,
        Box::new(VirtioConsole::new(Box::new(BufferCharBackend::new()))),
    );
    assert_eq!(mmio_read(&mut sys, REG_DEVICE_ID), VIRTIO_DEVICE_CONSOLE);
}

#[test]
fn device_id_net() {
    let mut sys = make_sys();
    attach(
        &mut sys,
        Box::new(VirtioNet::new([0x52, 0x54, 0x00, 0x12, 0x34, 0x56])),
    );
    assert_eq!(mmio_read(&mut sys, REG_DEVICE_ID), VIRTIO_DEVICE_NET);
}

// ── Tests: feature negotiation ────────────────────────────────────────────────

#[test]
fn rng_features_page0_is_zero_page1_has_version1() {
    let mut sys = make_sys();
    attach(&mut sys, Box::new(VirtioRng::new()));

    mmio_write(&mut sys, REG_DEVICE_FEAT_SEL, 0);
    assert_eq!(
        mmio_read(&mut sys, REG_DEVICE_FEATURES),
        0,
        "RNG has no page-0 features"
    );

    mmio_write(&mut sys, REG_DEVICE_FEAT_SEL, 1);
    let hi = mmio_read(&mut sys, REG_DEVICE_FEATURES);
    assert!(hi & 1 != 0, "VERSION_1 = bit 32 → page-1 bit 0");
}

#[test]
fn blk_features_page0_has_size_max_and_blk_size() {
    let mut sys = make_sys();
    attach(
        &mut sys,
        Box::new(VirtioBlk::new(
            Box::new(RamBlockBackend::zeroed(512)),
            false,
        )),
    );

    mmio_write(&mut sys, REG_DEVICE_FEAT_SEL, 0);
    let lo = mmio_read(&mut sys, REG_DEVICE_FEATURES);
    assert!(lo & (1 << 1) != 0, "VIRTIO_BLK_F_SIZE_MAX");
    assert!(lo & (1 << 6) != 0, "VIRTIO_BLK_F_BLK_SIZE");
}

#[test]
fn blk_readonly_feature_bit_set() {
    let mut sys = make_sys();
    attach(
        &mut sys,
        Box::new(VirtioBlk::new(Box::new(RamBlockBackend::zeroed(512)), true)),
    );

    mmio_write(&mut sys, REG_DEVICE_FEAT_SEL, 0);
    let lo = mmio_read(&mut sys, REG_DEVICE_FEATURES);
    assert!(lo & (1 << 5) != 0, "VIRTIO_BLK_F_RO");
}

#[test]
fn console_features_include_size() {
    let mut sys = make_sys();
    attach(
        &mut sys,
        Box::new(VirtioConsole::new(Box::new(BufferCharBackend::new()))),
    );

    mmio_write(&mut sys, REG_DEVICE_FEAT_SEL, 0);
    let lo = mmio_read(&mut sys, REG_DEVICE_FEATURES);
    assert!(
        lo & VIRTIO_CONSOLE_F_SIZE as u32 != 0,
        "VIRTIO_CONSOLE_F_SIZE"
    );
}

#[test]
fn net_features_include_mac() {
    let mut sys = make_sys();
    attach(
        &mut sys,
        Box::new(VirtioNet::new([0x52, 0x54, 0x00, 0x12, 0x34, 0x56])),
    );

    mmio_write(&mut sys, REG_DEVICE_FEAT_SEL, 0);
    let lo = mmio_read(&mut sys, REG_DEVICE_FEATURES);
    assert!(lo & VIRTIO_NET_F_MAC as u32 != 0, "VIRTIO_NET_F_MAC");
}

// ── Tests: status lifecycle ───────────────────────────────────────────────────

#[test]
fn initial_status_is_zero() {
    let mut sys = make_sys();
    attach(&mut sys, Box::new(VirtioRng::new()));
    assert_eq!(mmio_read(&mut sys, REG_STATUS), 0);
}

#[test]
fn full_status_lifecycle_via_mmio() {
    let mut sys = make_sys();
    attach(&mut sys, Box::new(VirtioRng::new()));

    driver_negotiate(&mut sys, 0, 1 /* VERSION_1 in page 1 */);

    let expected = STATUS_ACKNOWLEDGE | STATUS_DRIVER | STATUS_FEATURES_OK | STATUS_DRIVER_OK;
    assert_eq!(mmio_read(&mut sys, REG_STATUS), expected);
}

#[test]
fn reset_by_writing_zero_status() {
    let mut sys = make_sys();
    attach(&mut sys, Box::new(VirtioRng::new()));

    driver_negotiate(&mut sys, 0, 1);
    assert_ne!(mmio_read(&mut sys, REG_STATUS), 0);

    mmio_write(&mut sys, REG_STATUS, 0);
    assert_eq!(mmio_read(&mut sys, REG_STATUS), 0);
}

// ── Tests: queue setup ────────────────────────────────────────────────────────

#[test]
fn queue0_num_max_for_blk() {
    let mut sys = make_sys();
    attach(
        &mut sys,
        Box::new(VirtioBlk::new(
            Box::new(RamBlockBackend::zeroed(4096)),
            false,
        )),
    );

    mmio_write(&mut sys, REG_QUEUE_SEL, 0);
    assert_eq!(mmio_read(&mut sys, REG_QUEUE_NUM_MAX), 128);
}

#[test]
fn queue0_ready_after_setup() {
    let mut sys = make_sys();
    attach(
        &mut sys,
        Box::new(VirtioBlk::new(
            Box::new(RamBlockBackend::zeroed(4096)),
            false,
        )),
    );
    setup_queue0(&mut sys);
    assert_eq!(mmio_read(&mut sys, REG_QUEUE_READY), 1);
}

#[test]
fn out_of_range_queue_returns_zero_max() {
    let mut sys = make_sys();
    attach(
        &mut sys,
        Box::new(VirtioBlk::new(
            Box::new(RamBlockBackend::zeroed(4096)),
            false,
        )),
    );

    mmio_write(&mut sys, REG_QUEUE_SEL, 99);
    assert_eq!(mmio_read(&mut sys, REG_QUEUE_NUM_MAX), 0);
}

// ── Tests: interrupt ACK round-trip ──────────────────────────────────────────

#[test]
fn interrupt_status_initially_zero_and_ack_is_noop() {
    let mut sys = make_sys();
    attach(&mut sys, Box::new(VirtioRng::new()));

    assert_eq!(mmio_read(&mut sys, REG_INTERRUPT_STATUS), 0);
    // ACK with nothing pending should not panic
    mmio_write(&mut sys, REG_INTERRUPT_ACK, 0x3);
    assert_eq!(mmio_read(&mut sys, REG_INTERRUPT_STATUS), 0);
}

// ── Tests: two devices on one HelmAddressSpace ──────────────────────────────────────

#[test]
fn two_devices_independently_addressable() {
    const BLK_BASE: u64 = 0x0A00_0000;
    const RNG_BASE: u64 = 0x0A00_1000;

    let mut sys = HelmAddressSpace::new(FlatMem::new(RAM_BASE, 4 * 1024 * 1024));
    sys.add_device(
        BLK_BASE,
        Box::new(VirtioMmioTransport::new(Box::new(VirtioBlk::new(
            Box::new(RamBlockBackend::zeroed(4096)),
            false,
        )))),
    );
    sys.add_device(
        RNG_BASE,
        Box::new(VirtioMmioTransport::new(Box::new(VirtioRng::new()))),
    );

    let blk_id = sys
        .read(BLK_BASE + REG_DEVICE_ID, 4, AccessType::Load)
        .unwrap() as u32;
    let rng_id = sys
        .read(RNG_BASE + REG_DEVICE_ID, 4, AccessType::Load)
        .unwrap() as u32;
    assert_eq!(blk_id, VIRTIO_DEVICE_BLK);
    assert_eq!(rng_id, VIRTIO_DEVICE_RNG);

    // Status write to BLK must not affect RNG
    sys.write(
        BLK_BASE + REG_STATUS,
        4,
        STATUS_ACKNOWLEDGE as u64,
        AccessType::Store,
    )
    .unwrap();
    let rng_status = sys
        .read(RNG_BASE + REG_STATUS, 4, AccessType::Load)
        .unwrap() as u32;
    assert_eq!(rng_status, 0);
}

// ── Tests: VirtioBlk — virtqueue end-to-end ───────────────────────────────────

/// Build a block IN (sector read) request in RAM:
///   desc[0] header   → read-only, 16 bytes, next→1
///   desc[1] data buf → write-only, 512 bytes, next→2
///   desc[2] status   → write-only, 1 byte, end
fn setup_blk_read_chain(sys: &mut HelmAddressSpace, sector: u64) {
    // header: type=0 (IN), reserved=0, sector
    ram_write32(sys, HDR_BUFFER, 0); // type = IN
    ram_write32(sys, HDR_BUFFER + 4, 0); // reserved
    ram_write64(sys, HDR_BUFFER + 8, sector); // sector number

    write_desc(sys, 0, HDR_BUFFER, 16, 0b001 /* NEXT */, 1);
    write_desc(sys, 1, DATA_BUFFER, 512, 0b011 /* NEXT|WRITE */, 2);
    write_desc(sys, 2, STATUS_BYTE, 1, 0b010 /* WRITE */, 0);
    avail_push(sys, 0);
}

fn setup_blk_write_chain(sys: &mut HelmAddressSpace, sector: u64, payload: &[u8]) {
    ram_write32(sys, HDR_BUFFER, 1); // type = OUT
    ram_write32(sys, HDR_BUFFER + 4, 0);
    ram_write64(sys, HDR_BUFFER + 8, sector);
    for (i, &b) in payload.iter().enumerate() {
        ram_write8(sys, DATA_BUFFER + i as u64, b);
    }

    write_desc(sys, 0, HDR_BUFFER, 16, 0b001, 1);
    write_desc(sys, 1, DATA_BUFFER, payload.len() as u32, 0b001, 2);
    write_desc(sys, 2, STATUS_BYTE, 1, 0b010, 0);
    avail_push(sys, 0);
}

#[test]
fn blk_queue_notify_via_mmio_and_used_ring_advances() {
    let mut disk = vec![0u8; 4096];
    disk[0] = 0xDE;
    disk[1] = 0xAD;
    disk[511] = 0xFF;

    let mut sys = make_sys();
    attach(
        &mut sys,
        Box::new(VirtioBlk::new(Box::new(RamBlockBackend::new(disk)), false)),
    );

    driver_negotiate(&mut sys, 0, 1);
    setup_queue0(&mut sys);
    setup_blk_read_chain(&mut sys, 0);

    // Doorbell via MMIO
    mmio_write(&mut sys, REG_QUEUE_NOTIFY, 0);

    // Drive the virtqueue manually — transport set _notify_pending; we process here
    let mut q = VirtQueue::new(QUEUE_SIZE, DESC_BASE, AVAIL_BASE, USED_BASE);
    {
        let mem = &mut sys;
        let head = q
            .pop_chain(mem)
            .expect("virtqueue access must succeed")
            .expect("chain must be available");
        let chain = q.collect_chain(mem, head).unwrap();

        assert_eq!(chain.len(), 3);
        assert!(!chain[0].2, "header: read-only");
        assert!(chain[1].2, "data:   write-only");
        assert!(chain[2].2, "status: write-only");

        // Verify header decoded correctly from RAM
        let mut hdr = [0u8; 16];
        mem.read_bytes(chain[0].0, &mut hdr).unwrap();
        assert_eq!(
            u32::from_le_bytes(hdr[0..4].try_into().unwrap()),
            0,
            "type=IN"
        );
        assert_eq!(
            u64::from_le_bytes(hdr[8..16].try_into().unwrap()),
            0,
            "sector=0"
        );

        // Simulate device writing sector data into DATA_BUFFER
        let mut sector_data = vec![0xDEu8, 0xAD];
        sector_data.resize(512, 0);
        sector_data[511] = 0xFF;
        mem.write_bytes(chain[1].0, &sector_data).unwrap();

        // Write OK status
        mem.write_bytes(chain[2].0, &[0u8]).unwrap();

        q.push_used(mem, head, 512 + 1).unwrap();
    }

    assert_eq!(
        used_idx(&mut sys),
        1,
        "used ring must advance after processing"
    );
    assert_eq!(ram_read8(&mut sys, DATA_BUFFER), 0xDE);
    assert_eq!(ram_read8(&mut sys, DATA_BUFFER + 1), 0xAD);
    assert_eq!(ram_read8(&mut sys, DATA_BUFFER + 511), 0xFF);
    assert_eq!(ram_read8(&mut sys, STATUS_BYTE), 0, "status=OK");
}

#[test]
fn blk_write_chain_header_is_read_only() {
    let mut sys = make_sys();
    attach(
        &mut sys,
        Box::new(VirtioBlk::new(
            Box::new(RamBlockBackend::zeroed(4096)),
            false,
        )),
    );

    setup_queue0(&mut sys);

    // OUT request: type=1, sector=0
    ram_write32(&mut sys, HDR_BUFFER, 1); // type = OUT
    ram_write32(&mut sys, HDR_BUFFER + 4, 0);
    ram_write64(&mut sys, HDR_BUFFER + 8, 0);

    // desc[0] header read-only, desc[1] data read-only (driver→device), desc[2] status write-only
    write_desc(&mut sys, 0, HDR_BUFFER, 16, 0b001, 1);
    write_desc(&mut sys, 1, DATA_BUFFER, 512, 0b001, 2); // NEXT, no WRITE = driver sends data
    write_desc(&mut sys, 2, STATUS_BYTE, 1, 0b010, 0);
    avail_push(&mut sys, 0);

    mmio_write(&mut sys, REG_QUEUE_NOTIFY, 0);

    let mut q = VirtQueue::new(QUEUE_SIZE, DESC_BASE, AVAIL_BASE, USED_BASE);
    let mem = &mut sys;
    let head = q.pop_chain(mem).unwrap().unwrap();
    let chain = q.collect_chain(mem, head).unwrap();

    assert!(!chain[0].2, "header: read-only");
    assert!(!chain[1].2, "data:   read-only for OUT");
    assert!(chain[2].2, "status: write-only");
}

#[test]
fn blk_process_pending_read_request_end_to_end() {
    let mut disk = vec![0u8; 4096];
    disk[0] = 0xDE;
    disk[1] = 0xAD;
    disk[511] = 0xFF;

    let mut sys = make_sys();
    let idx = attach(
        &mut sys,
        Box::new(VirtioBlk::new(Box::new(RamBlockBackend::new(disk)), false)),
    );

    driver_negotiate(&mut sys, 0, 1);
    setup_queue0(&mut sys);
    setup_blk_read_chain(&mut sys, 0);

    mmio_write(&mut sys, REG_QUEUE_NOTIFY, 0);
    let events = process_transport(&mut sys, idx);

    assert!(events.queue_irq);
    assert_eq!(used_idx(&mut sys), 1);
    assert_eq!(ram_read8(&mut sys, DATA_BUFFER), 0xDE);
    assert_eq!(ram_read8(&mut sys, DATA_BUFFER + 1), 0xAD);
    assert_eq!(ram_read8(&mut sys, DATA_BUFFER + 511), 0xFF);
    assert_eq!(ram_read8(&mut sys, STATUS_BYTE), 0);
    assert_eq!(mmio_read(&mut sys, REG_INTERRUPT_STATUS), 1);
}

#[test]
fn blk_process_pending_write_then_read_round_trips_data() {
    let mut sys = make_sys();
    let idx = attach(
        &mut sys,
        Box::new(VirtioBlk::new(
            Box::new(RamBlockBackend::zeroed(4096)),
            false,
        )),
    );

    driver_negotiate(&mut sys, 0, 1);
    setup_queue0(&mut sys);

    let payload = b"virtio-blk-write";
    setup_blk_write_chain(&mut sys, 0, payload);
    mmio_write(&mut sys, REG_QUEUE_NOTIFY, 0);
    let events = process_transport(&mut sys, idx);
    assert!(events.queue_irq);
    assert_eq!(ram_read8(&mut sys, STATUS_BYTE), 0);
    let read_back = sys
        .with_device_mut::<VirtioMmioTransport, _>(idx, |transport| {
            transport
                .backend_as_mut::<VirtioBlk>()
                .expect("block backend")
                .read_bytes(0, payload.len())
        })
        .unwrap();
    assert_eq!(read_back, payload);
}

#[test]
fn blk_get_id_fills_data_buffer() {
    let mut sys = make_sys();
    let idx = attach(
        &mut sys,
        Box::new(VirtioBlk::new(
            Box::new(RamBlockBackend::zeroed(4096)),
            false,
        )),
    );

    driver_negotiate(&mut sys, 0, 1);
    setup_queue0(&mut sys);

    ram_write32(&mut sys, HDR_BUFFER, 8); // GET_ID
    ram_write32(&mut sys, HDR_BUFFER + 4, 0);
    ram_write64(&mut sys, HDR_BUFFER + 8, 0);
    write_desc(&mut sys, 0, HDR_BUFFER, 16, 0b001, 1);
    write_desc(&mut sys, 1, DATA_BUFFER, 20, 0b001 | 0b010, 2);
    write_desc(&mut sys, 2, STATUS_BYTE, 1, 0b010, 0);
    avail_push(&mut sys, 0);

    mmio_write(&mut sys, REG_QUEUE_NOTIFY, 0);
    let events = process_transport(&mut sys, idx);
    assert!(events.queue_irq);
    let id = (0..20)
        .map(|i| ram_read8(&mut sys, DATA_BUFFER + i as u64))
        .collect::<Vec<_>>();
    assert_eq!(&id, b"helm-virtio-blk\0\0\0\0\0");
    assert_eq!(ram_read8(&mut sys, STATUS_BYTE), 0);
}

#[test]
fn blk_out_of_range_request_returns_ioerr() {
    let mut sys = make_sys();
    let idx = attach(
        &mut sys,
        Box::new(VirtioBlk::new(
            Box::new(RamBlockBackend::zeroed(512)),
            false,
        )),
    );

    driver_negotiate(&mut sys, 0, 1);
    setup_queue0(&mut sys);
    setup_blk_read_chain(&mut sys, 8); // beyond 1 sector

    mmio_write(&mut sys, REG_QUEUE_NOTIFY, 0);
    let events = process_transport(&mut sys, idx);
    assert!(events.queue_irq);
    assert_eq!(ram_read8(&mut sys, STATUS_BYTE), 1);
}

#[test]
fn blk_read_only_write_returns_ioerr() {
    let mut sys = make_sys();
    let idx = attach(
        &mut sys,
        Box::new(VirtioBlk::new(Box::new(RamBlockBackend::zeroed(512)), true)),
    );

    driver_negotiate(&mut sys, 0, 1);
    setup_queue0(&mut sys);
    setup_blk_write_chain(&mut sys, 0, b"abc");

    mmio_write(&mut sys, REG_QUEUE_NOTIFY, 0);
    let events = process_transport(&mut sys, idx);
    assert!(events.queue_irq);
    assert_eq!(ram_read8(&mut sys, STATUS_BYTE), 1);
}

// ── Tests: VirtioRng — virtqueue end-to-end ───────────────────────────────────

#[test]
fn rng_notify_and_entropy_written_to_ram() {
    let mut sys = make_sys();
    attach(&mut sys, Box::new(VirtioRng::new()));

    driver_negotiate(&mut sys, 0, 1);
    setup_queue0(&mut sys);

    // Single write-only descriptor for entropy
    write_desc(&mut sys, 0, DATA_BUFFER, 64, 0b010 /* WRITE */, 0);
    avail_push(&mut sys, 0);

    mmio_write(&mut sys, REG_QUEUE_NOTIFY, 0);

    let mut q = VirtQueue::new(QUEUE_SIZE, DESC_BASE, AVAIL_BASE, USED_BASE);
    {
        let mem = &mut sys;
        let head = q
            .pop_chain(mem)
            .expect("virtqueue access must succeed")
            .expect("chain must be available");
        let chain = q.collect_chain(mem, head).unwrap();

        assert_eq!(chain.len(), 1);
        assert!(chain[0].2, "entropy buffer must be write-only");

        let mut rng = VirtioRng::new();
        let mut buf = vec![0u8; chain[0].1 as usize];
        rng.fill_entropy(&mut buf);
        mem.write_bytes(chain[0].0, &buf).unwrap();

        q.push_used(mem, head, chain[0].1).unwrap();
    }

    assert_eq!(used_idx(&mut sys), 1);
    let any_nonzero = (0..64).any(|i| ram_read8(&mut sys, DATA_BUFFER + i) != 0);
    assert!(any_nonzero, "entropy buffer must not be all zeros");
}

#[test]
fn rng_two_requests_sequential() {
    let mut sys = make_sys();
    attach(&mut sys, Box::new(VirtioRng::new()));
    setup_queue0(&mut sys);

    // First request: 32 bytes at DATA_BUFFER
    write_desc(&mut sys, 0, DATA_BUFFER, 32, 0b010, 0);
    avail_push(&mut sys, 0);
    // Second request: 32 bytes at DATA_BUFFER+0x100
    write_desc(&mut sys, 1, DATA_BUFFER + 0x100, 32, 0b010, 0);
    avail_push(&mut sys, 1);

    let mut q = VirtQueue::new(QUEUE_SIZE, DESC_BASE, AVAIL_BASE, USED_BASE);
    let mut rng = VirtioRng::new();

    for _ in 0..2 {
        let mem = &mut sys;
        let head = q.pop_chain(mem).unwrap().unwrap();
        let chain = q.collect_chain(mem, head).unwrap();
        let mut buf = vec![0u8; chain[0].1 as usize];
        rng.fill_entropy(&mut buf);
        mem.write_bytes(chain[0].0, &buf).unwrap();
        q.push_used(mem, head, chain[0].1).unwrap();
    }

    assert_eq!(used_idx(&mut sys), 2, "both requests processed");
}

// ── Tests: VirtioConsole — transmitq ─────────────────────────────────────────

#[test]
fn console_transmit_payload_readable_from_descriptor() {
    const CON_BASE: u64 = 0x0A00_2000;

    let mut sys = HelmAddressSpace::new(FlatMem::new(RAM_BASE, 16 * 1024 * 1024));
    sys.add_device(
        CON_BASE,
        Box::new(VirtioMmioTransport::new(Box::new(VirtioConsole::new(
            Box::new(BufferCharBackend::new()),
        )))),
    );

    // Write "hello\n" into RAM
    let msg = b"hello\n";
    for (i, &b) in msg.iter().enumerate() {
        sys.write(DATA_BUFFER + i as u64, 1, b as u64, AccessType::Store)
            .unwrap();
    }

    // desc[0]: read-only (driver→device), no chaining
    let base = DESC_BASE;
    sys.write(base, 8, DATA_BUFFER, AccessType::Store).unwrap();
    sys.write(base + 8, 4, msg.len() as u64, AccessType::Store)
        .unwrap();
    sys.write(base + 12, 2, 0u64, AccessType::Store).unwrap(); // read-only
    sys.write(base + 14, 2, 0u64, AccessType::Store).unwrap();

    // Manually set avail ring for queue 1 (transmitq)
    ram_write16(&mut sys, AVAIL_BASE + 2, 0);
    ram_write16(&mut sys, AVAIL_BASE + 4, 0); // ring[0] = desc 0
    ram_write16(&mut sys, AVAIL_BASE + 2, 1);

    // Notify queue 1 (transmitq) via MMIO
    sys.write(CON_BASE + REG_QUEUE_NOTIFY, 4, 1, AccessType::Store)
        .unwrap();

    // Walk the descriptor and read back the payload
    let mut q = VirtQueue::new(QUEUE_SIZE, DESC_BASE, AVAIL_BASE, USED_BASE);
    let mem = &mut sys;

    let head = q
        .pop_chain(mem)
        .expect("virtqueue access must succeed")
        .expect("transmit chain must be available");
    let chain = q.collect_chain(mem, head).unwrap();

    assert_eq!(chain.len(), 1);
    assert!(
        !chain[0].2,
        "transmit descriptor is read-only (driver→device)"
    );

    let mut payload = vec![0u8; chain[0].1 as usize];
    mem.read_bytes(chain[0].0, &mut payload).unwrap();
    assert_eq!(&payload, b"hello\n");

    q.push_used(mem, head, 0).unwrap();
    assert_eq!(used_idx(&mut sys), 1);
}

// ── Tests: VirtioNet — config space ──────────────────────────────────────────

#[test]
fn net_mac_readable_via_config_space_mmio() {
    let mac = [0x52u8, 0x54, 0x00, 0xAB, 0xCD, 0xEF];
    let mut sys = make_sys();
    attach(&mut sys, Box::new(VirtioNet::new(mac)));

    let cfg0 = mmio_read(&mut sys, REG_CONFIG_SPACE);
    assert_eq!(cfg0, u32::from_le_bytes([mac[0], mac[1], mac[2], mac[3]]));

    let cfg4 = mmio_read(&mut sys, REG_CONFIG_SPACE + 4);
    assert_eq!((cfg4 & 0xFFFF) as u16, u16::from_le_bytes([mac[4], mac[5]]));
}

#[test]
fn net_inject_and_count() {
    let mac = [0x52u8, 0x54, 0x00, 0x12, 0x34, 0x56];
    let mut net = VirtioNet::new(mac);
    net.inject_packet(vec![0u8; 64]);
    net.inject_packet(vec![0xFFu8; 128]);
    assert_eq!(net.rx_pending_count(), 2);
    assert!(net.pop_rx_frame().is_some());
    assert_eq!(net.rx_pending_count(), 1);
}

#[test]
fn net_process_pending_transmit_captures_frame_payload() {
    let mac = [0x52u8, 0x54, 0x00, 0x12, 0x34, 0x56];
    let mut sys = make_sys();
    let idx = attach(&mut sys, Box::new(VirtioNet::new(mac)));

    driver_negotiate(&mut sys, 0, 1);
    setup_queue(&mut sys, 1, DESC_BASE, AVAIL_BASE, USED_BASE);

    let frame = b"\xaa\xbb\xcc\xdd";
    for i in 0..VIRTIO_NET_HDR_SIZE {
        ram_write8(&mut sys, DATA_BUFFER + i as u64, 0);
    }
    for (i, &b) in frame.iter().enumerate() {
        ram_write8(
            &mut sys,
            DATA_BUFFER + VIRTIO_NET_HDR_SIZE as u64 + i as u64,
            b,
        );
    }
    write_desc(
        &mut sys,
        0,
        DATA_BUFFER,
        (VIRTIO_NET_HDR_SIZE + frame.len()) as u32,
        0,
        0,
    );
    avail_push(&mut sys, 0);

    mmio_write(&mut sys, REG_QUEUE_NOTIFY, 1);
    let events = process_transport(&mut sys, idx);
    assert!(events.queue_irq);

    let packets = sys
        .with_device_mut::<VirtioMmioTransport, _>(idx, |transport| {
            transport
                .backend_as_mut::<VirtioNet>()
                .expect("net backend")
                .drain_tx()
        })
        .unwrap();
    assert_eq!(packets, vec![frame.to_vec()]);
    assert_eq!(used_idx(&mut sys), 1);
}

#[test]
fn net_process_pending_receive_writes_header_and_payload() {
    let mac = [0x52u8, 0x54, 0x00, 0x12, 0x34, 0x56];
    let mut sys = make_sys();
    let idx = attach(&mut sys, Box::new(VirtioNet::new(mac)));

    driver_negotiate(&mut sys, 0, 1);
    setup_queue0(&mut sys);

    sys.with_device_mut::<VirtioMmioTransport, _>(idx, |transport| {
        transport
            .backend_as_mut::<VirtioNet>()
            .expect("net backend")
            .inject_packet(vec![0x11, 0x22, 0x33, 0x44]);
    })
    .unwrap();

    write_desc(
        &mut sys,
        0,
        DATA_BUFFER,
        (VIRTIO_NET_HDR_SIZE + 4) as u32,
        0b010,
        0,
    );
    avail_push(&mut sys, 0);
    mmio_write(&mut sys, REG_QUEUE_NOTIFY, 0);

    let events = process_transport(&mut sys, idx);
    assert!(events.queue_irq);
    for i in 0..VIRTIO_NET_HDR_SIZE {
        assert_eq!(ram_read8(&mut sys, DATA_BUFFER + i as u64), 0);
    }
    assert_eq!(
        (
            ram_read8(&mut sys, DATA_BUFFER + VIRTIO_NET_HDR_SIZE as u64),
            ram_read8(&mut sys, DATA_BUFFER + VIRTIO_NET_HDR_SIZE as u64 + 1),
            ram_read8(&mut sys, DATA_BUFFER + VIRTIO_NET_HDR_SIZE as u64 + 2),
            ram_read8(&mut sys, DATA_BUFFER + VIRTIO_NET_HDR_SIZE as u64 + 3)
        ),
        (0x11, 0x22, 0x33, 0x44)
    );
    assert_eq!(used_idx(&mut sys), 1);
}
