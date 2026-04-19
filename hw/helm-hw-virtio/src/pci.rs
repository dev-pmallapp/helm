//! Modern VirtIO PCI transport over a single BAR0 window.
//!
//! This is the current standard `virtio-pci` implementation in helm-ng. It
//! exposes the modern capability-linked PCI config surface together with a BAR0
//! register window containing:
//!
//! - common configuration
//! - ISR status
//! - notify region
//! - device-specific config
//!
//! The transport is backed by the existing [`crate::VirtioBackend`] trait, so
//! it shares today’s queue/config semantics with the MMIO transport while
//! presenting a spec-shaped PCI enumeration surface.

use std::sync::{Arc, Mutex};

use helm_devices::Device;
use helm_devices::MessageInterrupt;
use helm_hw_pci::{config::PciConfigSpace, PciEndpoint};
use thiserror::Error;

use crate::proto::features::{
    VIRTIO_DEVICE_BLK, VIRTIO_DEVICE_CONSOLE, VIRTIO_DEVICE_NET, VIRTIO_F_VERSION_1,
};
use crate::proto::transport::STATUS_FAILED;
use crate::proto::virtqueue::VirtQueue;
use crate::rng::VirtioRng;
use crate::VirtioBackend;

const VIRTIO_PCI_VENDOR_ID: u16 = 0x1AF4;
const VIRTIO_PCI_DEVICE_ID_BASE: u16 = 0x1040;
const VIRTIO_PCI_CAP_ID: u8 = 0x09;
const MSIX_CAP_ID: u8 = 0x11;

const BAR0_SIZE: u64 = 0x1000;
const BAR4_SIZE: u64 = 0x1000;
const COMMON_CFG_OFFSET: u64 = 0x000;
const COMMON_CFG_LEN: u32 = 0x038;
const ISR_OFFSET: u64 = 0x038;
const ISR_LEN: u32 = 0x004;
const NOTIFY_OFFSET: u64 = 0x040;
const NOTIFY_OFF_MULTIPLIER: u32 = 4;
const NOTIFY_LEN: u32 = 0x020;
const DEVICE_CFG_OFFSET: u64 = 0x100;
const DEVICE_CFG_LEN: u32 = 0x080;
const MSIX_TABLE_OFFSET: u64 = 0x000;
const MSIX_TABLE_STRIDE: u64 = 0x10;
const MSIX_PBA_OFFSET: u64 = 0x800;

const CAP_PTR_OFFSET: u16 = 0x34;
const STATUS_OFFSET: u16 = 0x06;
const STATUS_CAP_LIST: u32 = 0x0010;
const SUBSYSTEM_VENDOR_OFFSET: u16 = 0x2C;
const SUBSYSTEM_ID_OFFSET: u16 = 0x2E;
const MSIX_CAP_OFFSET: u16 = 0x90;
const MSIX_CONTROL_OFFSET: u16 = MSIX_CAP_OFFSET + 2;
const MSIX_TABLE_OFFSET_REG: u16 = MSIX_CAP_OFFSET + 4;
const MSIX_PBA_OFFSET_REG: u16 = MSIX_CAP_OFFSET + 8;
const MSIX_MASK_BIT: u16 = 1 << 14;
const MSIX_ENABLE_BIT: u16 = 1 << 15;

const REG_DEVICE_FEATURE_SEL: u64 = 0x00;
const REG_DEVICE_FEATURE: u64 = 0x04;
const REG_DRIVER_FEATURE_SEL: u64 = 0x08;
const REG_DRIVER_FEATURE: u64 = 0x0C;
const REG_MSIX_CONFIG_AND_QUEUE_COUNT: u64 = 0x10;
const REG_DEVICE_STATUS_AND_QUEUE_SEL: u64 = 0x14;
const REG_QUEUE_SIZE_AND_VECTOR: u64 = 0x18;
const REG_QUEUE_ENABLE_AND_NOTIFY_OFF: u64 = 0x1C;
const REG_QUEUE_DESC_LOW: u64 = 0x20;
const REG_QUEUE_DESC_HIGH: u64 = 0x24;
const REG_QUEUE_DRIVER_LOW: u64 = 0x28;
const REG_QUEUE_DRIVER_HIGH: u64 = 0x2C;
const REG_QUEUE_DEVICE_LOW: u64 = 0x30;
const REG_QUEUE_DEVICE_HIGH: u64 = 0x34;
const VIRTIO_IRQ_VQUEUE: u32 = 1;
const VIRTIO_IRQ_CONFIG: u32 = 2;

const MAX_QUEUES: usize = 8;

#[repr(u8)]
#[derive(Clone, Copy)]
enum VirtioPciCapType {
    Common = 1,
    Notify = 2,
    Isr = 3,
    Device = 4,
    Pci = 5,
}

#[derive(Debug, Default, Clone, Copy)]
struct QueueState {
    max_size: u16,
    size: u16,
    ready: bool,
    desc_addr: u64,
    driver_addr: u64,
    device_addr: u64,
    last_avail_idx: u16,
    used_idx: u16,
}

#[derive(Debug, Default, Clone, Copy)]
struct MsixVector {
    addr_lo: u32,
    addr_hi: u32,
    data: u32,
    masked: bool,
    pending: bool,
}

struct VirtioPciState {
    backend: Box<dyn VirtioBackend>,
    device_features_sel: u32,
    driver_features: [u32; 2],
    driver_features_sel: u32,
    queue_sel: u16,
    queues: [QueueState; MAX_QUEUES],
    interrupt_status: u32,
    status: u8,
    config_generation: u8,
    msix_enabled: bool,
    msix_function_mask: bool,
    msix_config: u16,
    queue_msix_vectors: Vec<u16>,
    msix_vectors: Vec<MsixVector>,
}

/// PCI config-space endpoint for the modern VirtIO PCI transport.
pub struct VirtioPciEndpoint {
    config: Mutex<PciConfigSpace>,
    vendor_id: u16,
    device_id: u16,
    class_code: u32,
    state: Arc<Mutex<VirtioPciState>>,
}

/// BAR0 MMIO window for the modern VirtIO PCI transport.
pub struct VirtioPciBar0Device {
    state: Arc<Mutex<VirtioPciState>>,
}

/// BAR4 MSI-X table and pending-bit array window.
pub struct VirtioPciBar4Device {
    state: Arc<Mutex<VirtioPciState>>,
}

/// Cloneable handle for draining pending queue work on a PCI transport.
#[derive(Clone)]
pub struct VirtioPciPendingProcessor {
    state: Arc<Mutex<VirtioPciState>>,
}

/// Result of draining one standard `virtio-pci` transport's pending work.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct VirtioPciPendingResult {
    /// Queue activity completed and raised a queue interrupt condition.
    pub queue_irq: bool,
    /// Device config changed and raised a config interrupt condition.
    pub config_irq: bool,
    /// Ready MSI-X messages emitted by this processing pass.
    pub msix_messages: Vec<MessageInterrupt>,
}

/// Errors from constructing a standard `virtio-pci` transport surface.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum VirtioPciBuildError {
    /// The backend reported a VirtIO device ID that cannot be projected into the PCI ID range.
    #[error("VirtIO device type {0:#x} exceeds 16-bit PCI transport support")]
    DeviceTypeExceeds16Bit(u32),
    /// BAR0 base does not fit this model's 32-bit BAR support.
    #[error("PCI BAR0 base {0:#x} exceeds 32-bit BAR support")]
    Bar0BaseExceeds32Bit(u64),
    /// BAR4 base cannot be derived from BAR0 without overflowing `u64`.
    #[error("PCI BAR4 base overflow from BAR0 base {0:#x}")]
    Bar4BaseOverflow(u64),
    /// BAR4 base does not fit this model's 32-bit BAR support.
    #[error("PCI BAR4 base {0:#x} exceeds 32-bit BAR support")]
    Bar4BaseExceeds32Bit(u64),
}

/// Build a modern VirtIO PCI transport pair for one backend.
pub fn build_virtio_pci_pair(
    backend: Box<dyn VirtioBackend>,
    base: u64,
) -> Result<(VirtioPciEndpoint, VirtioPciBar0Device, VirtioPciBar4Device), VirtioPciBuildError> {
    let virtio_device_id = backend.device_type();
    let virtio_device_id_u16 = u16::try_from(virtio_device_id)
        .map_err(|_| VirtioPciBuildError::DeviceTypeExceeds16Bit(virtio_device_id))?;
    let pci_device_id = VIRTIO_PCI_DEVICE_ID_BASE.wrapping_add(virtio_device_id_u16);
    let class_code = class_code_for(virtio_device_id);
    let bar4_base = base
        .checked_add(BAR0_SIZE)
        .ok_or(VirtioPciBuildError::Bar4BaseOverflow(base))?;

    let mut config = PciConfigSpace::new(VIRTIO_PCI_VENDOR_ID, pci_device_id, class_code, 0x00);
    config.set_bar_size(0, BAR0_SIZE as u32);
    config.set_bar_size(4, BAR4_SIZE as u32);
    let base_u32 =
        u32::try_from(base).map_err(|_| VirtioPciBuildError::Bar0BaseExceeds32Bit(base))?;
    let bar4_base_u32 = u32::try_from(bar4_base)
        .map_err(|_| VirtioPciBuildError::Bar4BaseExceeds32Bit(bar4_base))?;
    config.write(0x10, 4, base_u32);
    config.write(0x20, 4, bar4_base_u32);
    config.write(STATUS_OFFSET, 2, STATUS_CAP_LIST);
    config.write(SUBSYSTEM_VENDOR_OFFSET, 2, VIRTIO_PCI_VENDOR_ID.into());
    config.write(SUBSYSTEM_ID_OFFSET, 2, virtio_device_id);
    config.write(CAP_PTR_OFFSET, 1, 0x40);

    write_vendor_cap(
        &mut config,
        0x40,
        0x50,
        VirtioPciCapType::Common,
        0,
        COMMON_CFG_OFFSET as u32,
        COMMON_CFG_LEN,
        0,
    );
    write_vendor_cap(
        &mut config,
        0x50,
        0x60,
        VirtioPciCapType::Isr,
        0,
        ISR_OFFSET as u32,
        ISR_LEN,
        0,
    );
    write_vendor_cap(
        &mut config,
        0x60,
        0x74,
        VirtioPciCapType::Notify,
        0,
        NOTIFY_OFFSET as u32,
        NOTIFY_LEN,
        NOTIFY_OFF_MULTIPLIER,
    );
    write_vendor_cap(
        &mut config,
        0x74,
        0x84,
        VirtioPciCapType::Device,
        0,
        DEVICE_CFG_OFFSET as u32,
        DEVICE_CFG_LEN,
        0,
    );
    write_vendor_cap(
        &mut config,
        0x84,
        MSIX_CAP_OFFSET as u8,
        VirtioPciCapType::Pci,
        0,
        0,
        4,
        0,
    );

    let active_queues = (0..MAX_QUEUES)
        .take_while(|&idx| backend.queue_max_size(idx) > 0)
        .count()
        .max(1);
    let msix_vectors_len = active_queues + 1;
    write_msix_capability(
        &mut config,
        MSIX_CAP_OFFSET,
        msix_vectors_len as u16,
        4,
        MSIX_TABLE_OFFSET as u32,
        4,
        MSIX_PBA_OFFSET as u32,
    );

    let mut queues = [QueueState::default(); MAX_QUEUES];
    for (idx, queue) in queues.iter_mut().enumerate() {
        queue.max_size = backend.queue_max_size(idx) as u16;
        queue.size = queue.max_size;
    }

    let state = Arc::new(Mutex::new(VirtioPciState {
        backend,
        device_features_sel: 0,
        driver_features: [0; 2],
        driver_features_sel: 0,
        queue_sel: 0,
        queues,
        interrupt_status: 0,
        status: 0,
        config_generation: 0,
        msix_enabled: false,
        msix_function_mask: false,
        msix_config: 0xFFFF,
        queue_msix_vectors: vec![0xFFFF; active_queues],
        msix_vectors: vec![MsixVector::default(); msix_vectors_len],
    }));

    Ok((
        VirtioPciEndpoint {
            config: Mutex::new(config),
            vendor_id: VIRTIO_PCI_VENDOR_ID,
            device_id: pci_device_id,
            class_code,
            state: Arc::clone(&state),
        },
        VirtioPciBar0Device {
            state: Arc::clone(&state),
        },
        VirtioPciBar4Device { state },
    ))
}

/// Build a modern VirtIO PCI RNG transport pair.
pub fn build_virtio_pci_rng_pair(
    base: u64,
    seed: u64,
) -> Result<(VirtioPciEndpoint, VirtioPciBar0Device, VirtioPciBar4Device), VirtioPciBuildError> {
    build_virtio_pci_pair(Box::new(VirtioRng::with_seed(seed)), base)
}

impl PciEndpoint for VirtioPciEndpoint {
    fn config_read(&self, offset: u16, size: usize) -> u32 {
        self.config
            .lock()
            .expect("virtio pci config mutex poisoned")
            .read(offset, size)
    }

    fn config_write(&mut self, offset: u16, size: usize, val: u32) {
        let mut config = self
            .config
            .lock()
            .expect("virtio pci config mutex poisoned");
        config.write(offset, size, val);
        if overlaps(offset, size, MSIX_CONTROL_OFFSET, 2) {
            let control = config.read(MSIX_CONTROL_OFFSET, 2) as u16;
            let mut state = self.state.lock().expect("virtio pci state mutex poisoned");
            state.msix_enabled = (control & MSIX_ENABLE_BIT) != 0;
            state.msix_function_mask = (control & MSIX_MASK_BIT) != 0;
        }
    }

    fn vendor_id(&self) -> u16 {
        self.vendor_id
    }

    fn device_id(&self) -> u16 {
        self.device_id
    }

    fn class_code(&self) -> u32 {
        self.class_code
    }

    fn bar_base(&self, bar_index: u8) -> Option<u64> {
        self.config
            .lock()
            .expect("virtio pci config mutex poisoned")
            .bar_address(bar_index as usize)
    }

    fn bar_size(&self, bar_index: u8) -> Option<u64> {
        self.config
            .lock()
            .expect("virtio pci config mutex poisoned")
            .bar_size(bar_index as usize)
    }
}

impl Device for VirtioPciBar0Device {
    fn read(&mut self, offset: u64, size: usize) -> u64 {
        let mut state = self.state.lock().expect("virtio pci state mutex poisoned");
        let Some(size) = supported_width(size) else {
            return 0;
        };
        if crosses_word_boundary(offset, size) {
            return 0;
        }
        if offset < COMMON_CFG_OFFSET + u64::from(COMMON_CFG_LEN) {
            let word_offset = offset & !0x3;
            return u64::from(extract_subword(
                common_cfg_read(&state, word_offset),
                offset,
                size,
            ));
        }
        if (ISR_OFFSET..ISR_OFFSET + u64::from(ISR_LEN)).contains(&offset) {
            let val = state.interrupt_status;
            state.interrupt_status = 0;
            return u64::from(extract_subword(val, offset, size));
        }
        if (DEVICE_CFG_OFFSET..DEVICE_CFG_OFFSET + u64::from(DEVICE_CFG_LEN)).contains(&offset) {
            let word_offset = (offset - DEVICE_CFG_OFFSET) & !0x3;
            let config = state.backend.read_config(word_offset as u32);
            return u64::from(extract_subword(config, offset, size));
        }
        0
    }

    fn write(&mut self, offset: u64, size: usize, val: u64) {
        let mut state = self.state.lock().expect("virtio pci state mutex poisoned");
        let Some(size) = supported_width(size) else {
            return;
        };
        if crosses_word_boundary(offset, size) {
            return;
        }
        if offset < COMMON_CFG_OFFSET + u64::from(COMMON_CFG_LEN) {
            let word_offset = offset & !0x3;
            let old = common_cfg_read(&state, word_offset);
            let merged = merge_subword(old, offset, size, val);
            common_cfg_write(&mut state, word_offset, merged);
            return;
        }
        if (NOTIFY_OFFSET..NOTIFY_OFFSET + u64::from(NOTIFY_LEN)).contains(&offset) {
            let queue = ((offset - NOTIFY_OFFSET) / u64::from(NOTIFY_OFF_MULTIPLIER)) as usize;
            state.backend.queue_notify(queue, None);
            return;
        }
        if (DEVICE_CFG_OFFSET..DEVICE_CFG_OFFSET + u64::from(DEVICE_CFG_LEN)).contains(&offset) {
            let word_offset = (offset - DEVICE_CFG_OFFSET) & !0x3;
            let old = state.backend.read_config(word_offset as u32);
            let merged = merge_subword(old, offset, size, val);
            state.backend.write_config(word_offset as u32, merged);
            state.config_generation = state.config_generation.wrapping_add(1);
            raise_config_irq(&mut state);
        }
    }

    fn region_size(&self) -> u64 {
        BAR0_SIZE
    }
}

impl VirtioPciBar0Device {
    /// Return a cloneable pending-work processor for this transport.
    pub fn pending_processor(&self) -> VirtioPciPendingProcessor {
        VirtioPciPendingProcessor {
            state: Arc::clone(&self.state),
        }
    }
}

impl Device for VirtioPciBar4Device {
    fn read(&mut self, offset: u64, size: usize) -> u64 {
        let state = self.state.lock().expect("virtio pci state mutex poisoned");
        let Some(size) = supported_width(size) else {
            return 0;
        };
        if crosses_word_boundary(offset, size) {
            return 0;
        }
        if offset >= MSIX_PBA_OFFSET {
            return u64::from(extract_subword(
                read_msix_pba(&state, offset & !0x3),
                offset,
                size,
            ));
        }
        u64::from(extract_subword(
            read_msix_table(&state, offset & !0x3),
            offset,
            size,
        ))
    }

    fn write(&mut self, offset: u64, size: usize, val: u64) {
        let mut state = self.state.lock().expect("virtio pci state mutex poisoned");
        let Some(size) = supported_width(size) else {
            return;
        };
        if crosses_word_boundary(offset, size) {
            return;
        }
        if offset >= MSIX_PBA_OFFSET {
            return;
        }
        let word_offset = offset & !0x3;
        let old = read_msix_table(&state, word_offset);
        let merged = merge_subword(old, offset, size, val);
        write_msix_table(&mut state, word_offset, merged);
    }

    fn region_size(&self) -> u64 {
        BAR4_SIZE
    }
}

fn class_code_for(device_id: u32) -> u32 {
    match device_id {
        VIRTIO_DEVICE_NET => 0x020000,
        VIRTIO_DEVICE_BLK => 0x018000,
        VIRTIO_DEVICE_CONSOLE => 0x078000,
        _ => 0xFF0000,
    }
}

fn write_vendor_cap(
    config: &mut PciConfigSpace,
    offset: u16,
    next: u8,
    cap_type: VirtioPciCapType,
    bar: u8,
    bar_offset: u32,
    length: u32,
    notify_off_multiplier: u32,
) {
    let cap_len = if matches!(cap_type, VirtioPciCapType::Notify) {
        20u8
    } else {
        16u8
    };
    let header = u32::from(VIRTIO_PCI_CAP_ID)
        | (u32::from(next) << 8)
        | (u32::from(cap_len) << 16)
        | (u32::from(cap_type as u8) << 24);
    config.write(offset, 4, header);
    config.write(offset + 4, 4, u32::from(bar));
    config.write(offset + 8, 4, bar_offset);
    config.write(offset + 12, 4, length);
    if matches!(cap_type, VirtioPciCapType::Notify) {
        config.write(offset + 16, 4, notify_off_multiplier);
    }
}

fn write_msix_capability(
    config: &mut PciConfigSpace,
    offset: u16,
    num_vectors: u16,
    table_bar: u8,
    table_offset: u32,
    pba_bar: u8,
    pba_offset: u32,
) {
    let control = num_vectors.saturating_sub(1) & 0x07FF;
    let header = u32::from(MSIX_CAP_ID) | (12u32 << 16);
    config.write(offset, 4, header);
    config.write(offset + 2, 2, u32::from(control));
    config.write(
        MSIX_TABLE_OFFSET_REG,
        4,
        table_offset | u32::from(table_bar & 0x7),
    );
    config.write(
        MSIX_PBA_OFFSET_REG,
        4,
        pba_offset | u32::from(pba_bar & 0x7),
    );
}

fn common_cfg_read(state: &VirtioPciState, offset: u64) -> u32 {
    match offset & !3 {
        REG_DEVICE_FEATURE_SEL => state.device_features_sel,
        REG_DEVICE_FEATURE => {
            let features = state.backend.device_features() | VIRTIO_F_VERSION_1;
            if state.device_features_sel == 0 {
                features as u32
            } else {
                (features >> 32) as u32
            }
        }
        REG_DRIVER_FEATURE_SEL => state.driver_features_sel,
        REG_DRIVER_FEATURE => {
            if state.driver_features_sel == 0 {
                state.driver_features[0]
            } else {
                state.driver_features[1]
            }
        }
        REG_MSIX_CONFIG_AND_QUEUE_COUNT => {
            u32::from(state.msix_config) | ((state.queue_msix_vectors.len() as u32) << 16)
        }
        REG_DEVICE_STATUS_AND_QUEUE_SEL => {
            u32::from(state.status)
                | (u32::from(state.config_generation) << 8)
                | (u32::from(state.queue_sel) << 16)
        }
        REG_QUEUE_SIZE_AND_VECTOR => {
            let queue = state.queues[state.queue_sel as usize];
            let vector = state
                .queue_msix_vectors
                .get(state.queue_sel as usize)
                .copied()
                .unwrap_or(0xFFFF);
            u32::from(queue.size) | (u32::from(vector) << 16)
        }
        REG_QUEUE_ENABLE_AND_NOTIFY_OFF => {
            let queue = state.queues[state.queue_sel as usize];
            u32::from(queue.ready) | (u32::from(state.queue_sel) << 16)
        }
        REG_QUEUE_DESC_LOW => state.queues[state.queue_sel as usize].desc_addr as u32,
        REG_QUEUE_DESC_HIGH => (state.queues[state.queue_sel as usize].desc_addr >> 32) as u32,
        REG_QUEUE_DRIVER_LOW => state.queues[state.queue_sel as usize].driver_addr as u32,
        REG_QUEUE_DRIVER_HIGH => (state.queues[state.queue_sel as usize].driver_addr >> 32) as u32,
        REG_QUEUE_DEVICE_LOW => state.queues[state.queue_sel as usize].device_addr as u32,
        REG_QUEUE_DEVICE_HIGH => (state.queues[state.queue_sel as usize].device_addr >> 32) as u32,
        _ => 0,
    }
}

fn common_cfg_write(state: &mut VirtioPciState, offset: u64, value: u32) {
    match offset & !3 {
        REG_DEVICE_FEATURE_SEL => state.device_features_sel = value,
        REG_DRIVER_FEATURE_SEL => state.driver_features_sel = value,
        REG_DRIVER_FEATURE => {
            let page = state.driver_features_sel.min(1) as usize;
            state.driver_features[page] = value;
        }
        REG_MSIX_CONFIG_AND_QUEUE_COUNT => {
            state.msix_config = value as u16;
        }
        REG_DEVICE_STATUS_AND_QUEUE_SEL => {
            let status = value as u8;
            if status == 0 {
                do_reset(state);
            } else {
                state.status = status;
            }
            state.queue_sel = ((value >> 16) as usize).min(MAX_QUEUES - 1) as u16;
        }
        REG_QUEUE_SIZE_AND_VECTOR => {
            let queue = &mut state.queues[state.queue_sel as usize];
            if !queue.ready && (value as u16) != 0 {
                queue.size = (value as u16).min(queue.max_size.max(1));
            }
            if let Some(vector) = state.queue_msix_vectors.get_mut(state.queue_sel as usize) {
                *vector = (value >> 16) as u16;
            }
        }
        REG_QUEUE_ENABLE_AND_NOTIFY_OFF => {
            state.queues[state.queue_sel as usize].ready = (value & 0xFFFF) != 0;
        }
        REG_QUEUE_DESC_LOW => {
            let queue = &mut state.queues[state.queue_sel as usize];
            queue.desc_addr = (queue.desc_addr & 0xFFFF_FFFF_0000_0000) | u64::from(value);
        }
        REG_QUEUE_DESC_HIGH => {
            let queue = &mut state.queues[state.queue_sel as usize];
            queue.desc_addr = (queue.desc_addr & 0x0000_0000_FFFF_FFFF) | (u64::from(value) << 32);
        }
        REG_QUEUE_DRIVER_LOW => {
            let queue = &mut state.queues[state.queue_sel as usize];
            queue.driver_addr = (queue.driver_addr & 0xFFFF_FFFF_0000_0000) | u64::from(value);
        }
        REG_QUEUE_DRIVER_HIGH => {
            let queue = &mut state.queues[state.queue_sel as usize];
            queue.driver_addr =
                (queue.driver_addr & 0x0000_0000_FFFF_FFFF) | (u64::from(value) << 32);
        }
        REG_QUEUE_DEVICE_LOW => {
            let queue = &mut state.queues[state.queue_sel as usize];
            queue.device_addr = (queue.device_addr & 0xFFFF_FFFF_0000_0000) | u64::from(value);
        }
        REG_QUEUE_DEVICE_HIGH => {
            let queue = &mut state.queues[state.queue_sel as usize];
            queue.device_addr =
                (queue.device_addr & 0x0000_0000_FFFF_FFFF) | (u64::from(value) << 32);
        }
        _ => {}
    }
}

fn do_reset(state: &mut VirtioPciState) {
    state.device_features_sel = 0;
    state.driver_features = [0; 2];
    state.driver_features_sel = 0;
    state.queue_sel = 0;
    state.interrupt_status = 0;
    state.status = 0;
    state.config_generation = 0;
    state.msix_enabled = false;
    state.msix_function_mask = false;
    state.msix_config = 0xFFFF;
    for vector in &mut state.queue_msix_vectors {
        *vector = 0xFFFF;
    }
    for vector in &mut state.msix_vectors {
        *vector = MsixVector::default();
    }
    for queue in &mut state.queues {
        let max_size = queue.max_size;
        *queue = QueueState {
            max_size,
            size: max_size,
            ..QueueState::default()
        };
    }
    state.backend.reset();
}

impl VirtioPciPendingProcessor {
    /// Process any backend-latched queue work against guest memory.
    pub fn process_pending(&self, mem: &mut dyn helm_core::ByteMem) -> VirtioPciPendingResult {
        let mut state = self.state.lock().expect("virtio pci state mutex poisoned");
        let mut queues = Vec::new();
        let mut indices = Vec::new();

        for (idx, queue) in state.queues.iter().enumerate() {
            if !queue.ready || queue.size == 0 {
                continue;
            }
            queues.push(VirtQueue::new_with_progress(
                queue.size,
                queue.desc_addr,
                queue.driver_addr,
                queue.device_addr,
                queue.last_avail_idx,
                queue.used_idx,
            ));
            indices.push(idx);
        }

        let events = state.backend.process_pending(mem, &mut queues);
        for (idx, queue) in indices.into_iter().zip(queues.into_iter()) {
            let (last_avail_idx, used_idx) = queue.progress();
            state.queues[idx].last_avail_idx = last_avail_idx;
            state.queues[idx].used_idx = used_idx;
        }

        if events.queue_irq {
            state.interrupt_status |= VIRTIO_IRQ_VQUEUE;
        }
        if events.failed {
            state.status |= STATUS_FAILED as u8;
        }
        if events.config_irq || events.failed {
            state.interrupt_status |= VIRTIO_IRQ_CONFIG;
            state.config_generation = state.config_generation.wrapping_add(1);
        }
        if events.queue_irq {
            let queue_vectors: Vec<(usize, u16)> = state
                .queue_msix_vectors
                .iter()
                .copied()
                .enumerate()
                .collect();
            for (idx, vector) in queue_vectors {
                if vector != 0xFFFF && idx < state.queues.len() && state.queues[idx].ready {
                    mark_msix_vector(&mut state, vector as usize);
                }
            }
        }
        let config_vector = state.msix_config;
        if (events.config_irq || events.failed) && config_vector != 0xFFFF {
            mark_msix_vector(&mut state, config_vector as usize);
        }
        let msix_messages = drain_ready_msix_messages(&mut state);

        VirtioPciPendingResult {
            queue_irq: events.queue_irq,
            config_irq: events.config_irq,
            msix_messages,
        }
    }
}

fn raise_config_irq(state: &mut VirtioPciState) {
    state.interrupt_status |= VIRTIO_IRQ_CONFIG;
    if state.msix_config != 0xFFFF {
        mark_msix_vector(state, state.msix_config as usize);
    }
}

fn mark_msix_vector(state: &mut VirtioPciState, idx: usize) {
    if !state.msix_enabled {
        return;
    }
    let Some(vector) = state.msix_vectors.get_mut(idx) else {
        return;
    };
    vector.pending = true;
}

fn drain_ready_msix_messages(state: &mut VirtioPciState) -> Vec<MessageInterrupt> {
    if !state.msix_enabled || state.msix_function_mask {
        return Vec::new();
    }

    let mut messages = Vec::new();
    for vector in &mut state.msix_vectors {
        if !vector.pending || vector.masked {
            continue;
        }
        vector.pending = false;
        let addr = u64::from(vector.addr_lo) | (u64::from(vector.addr_hi) << 32);
        messages.push(MessageInterrupt::new(addr, vector.data));
    }
    messages
}

fn overlaps(start: u16, size: usize, target_start: u16, target_size: usize) -> bool {
    let end = start.saturating_add(size as u16);
    let target_end = target_start.saturating_add(target_size as u16);
    start < target_end && target_start < end
}

fn supported_width(size: usize) -> Option<usize> {
    match size {
        1 | 2 | 4 => Some(size),
        _ => None,
    }
}

fn crosses_word_boundary(offset: u64, size: usize) -> bool {
    (offset & 0x3) + size as u64 > 4
}

use helm_devices::{extract_subword, merge_subword};

fn read_msix_table(state: &VirtioPciState, offset: u64) -> u32 {
    let idx = (offset / MSIX_TABLE_STRIDE) as usize;
    let field = offset % MSIX_TABLE_STRIDE;
    let Some(vector) = state.msix_vectors.get(idx) else {
        return 0;
    };
    match field {
        0x00 => vector.addr_lo,
        0x04 => vector.addr_hi,
        0x08 => vector.data,
        0x0C => u32::from(vector.masked),
        _ => 0,
    }
}

fn write_msix_table(state: &mut VirtioPciState, offset: u64, value: u32) {
    let idx = (offset / MSIX_TABLE_STRIDE) as usize;
    let field = offset % MSIX_TABLE_STRIDE;
    let Some(vector) = state.msix_vectors.get_mut(idx) else {
        return;
    };
    match field {
        0x00 => vector.addr_lo = value,
        0x04 => vector.addr_hi = value,
        0x08 => vector.data = value,
        0x0C => {
            vector.masked = (value & 0x1) != 0;
            if !vector.masked {
                vector.pending = false;
            }
        }
        _ => {}
    }
}

fn read_msix_pba(state: &VirtioPciState, offset: u64) -> u32 {
    let chunk = ((offset - MSIX_PBA_OFFSET) / 4) as usize;
    let bit_base = chunk * 32;
    let mut val = 0u32;
    for bit in 0..32 {
        if state
            .msix_vectors
            .get(bit_base + bit)
            .is_some_and(|vector| vector.pending)
        {
            val |= 1 << bit;
        }
    }
    val
}

#[cfg(test)]
mod tests {
    use super::*;
    use helm_core::{AccessType, MemInterface};
    use helm_hw_pci::Bdf;
    use helm_hw_pci::PciBus;
    use helm_memory::{FlatMem, HelmAddressSpace};
    use std::any::Any;

    const DESC_BASE: u64 = 0x2000;
    const AVAIL_BASE: u64 = 0x2200;
    const USED_BASE: u64 = 0x2400;
    const DATA_BASE: u64 = 0x2800;

    struct InvalidTypeBackend;

    impl VirtioBackend for InvalidTypeBackend {
        fn device_type(&self) -> u32 {
            0x1_0000
        }

        fn vendor_id(&self) -> u32 {
            0
        }

        fn device_features(&self) -> u64 {
            0
        }

        fn queue_max_size(&self, _queue: usize) -> u32 {
            0
        }

        fn queue_notify(&mut self, _queue: usize, _mem: Option<&mut dyn helm_core::ByteMem>) {}

        fn read_config(&self, _offset: u32) -> u32 {
            0
        }

        fn write_config(&mut self, _offset: u32, _val: u32) {}

        fn reset(&mut self) {}

        fn as_any_mut(&mut self) -> &mut dyn Any {
            self
        }
    }

    #[test]
    fn virtio_pci_rng_pair_enumerates_and_exposes_common_cfg() {
        const BASE: u64 = 0x0A00_2000;
        let (endpoint, bar0, bar4) = build_virtio_pci_rng_pair(BASE, 0x1234_5678).unwrap();
        let mut bus = PciBus::new("pci0");
        bus.attach_endpoint(Bdf::new(0, 1, 0), Box::new(endpoint))
            .unwrap();

        let mut sys = HelmAddressSpace::new(FlatMem::new(0, 0));
        sys.add_device(0x3000_0000, Box::new(bus));
        sys.add_device(BASE, Box::new(bar0));
        sys.add_device(BASE + BAR0_SIZE, Box::new(bar4));

        let vendor_device = sys
            .read(0x3000_0000 + (1u64 << 15), 4, AccessType::Load)
            .unwrap();
        assert_eq!(vendor_device as u32, 0x1044_1AF4);

        let cap_ptr = sys
            .read(0x3000_0000 + (1u64 << 15) + 0x34, 1, AccessType::Load)
            .unwrap();
        assert_eq!(cap_ptr, 0x40);
        let msix_cap = sys
            .read(
                0x3000_0000 + (1u64 << 15) + u64::from(MSIX_CAP_OFFSET),
                4,
                AccessType::Load,
            )
            .unwrap() as u32;
        assert_eq!(msix_cap & 0xFF, u32::from(MSIX_CAP_ID));

        sys.write(BASE + REG_DEVICE_FEATURE_SEL, 4, 1, AccessType::Store)
            .unwrap();
        let features = sys
            .read(BASE + REG_DEVICE_FEATURE, 4, AccessType::Load)
            .unwrap();
        assert_eq!(features, 1);

        let queue_size = sys
            .read(BASE + REG_QUEUE_SIZE_AND_VECTOR, 4, AccessType::Load)
            .unwrap();
        assert_eq!(queue_size & 0xFFFF, 64);

        sys.write(
            BASE + REG_MSIX_CONFIG_AND_QUEUE_COUNT,
            4,
            7,
            AccessType::Store,
        )
        .unwrap();
        let msix_cfg = sys
            .read(BASE + REG_MSIX_CONFIG_AND_QUEUE_COUNT, 4, AccessType::Load)
            .unwrap();
        assert_eq!(msix_cfg & 0xFFFF, 7);

        sys.write(BASE + BAR0_SIZE + 0x00, 4, 0xFEE0_0000, AccessType::Store)
            .unwrap();
        sys.write(BASE + BAR0_SIZE + 0x08, 4, 0x41, AccessType::Store)
            .unwrap();
        let addr_lo = sys
            .read(BASE + BAR0_SIZE + 0x00, 4, AccessType::Load)
            .unwrap();
        let data = sys
            .read(BASE + BAR0_SIZE + 0x08, 4, AccessType::Load)
            .unwrap();
        assert_eq!(addr_lo, 0xFEE0_0000);
        assert_eq!(data, 0x41);
    }

    #[test]
    fn builder_rejects_out_of_range_device_type() {
        match build_virtio_pci_pair(Box::new(InvalidTypeBackend), 0x0A00_0000) {
            Err(err) => assert_eq!(err, VirtioPciBuildError::DeviceTypeExceeds16Bit(0x1_0000)),
            Ok(_) => panic!("out-of-range virtio device type should be rejected"),
        }
    }

    #[test]
    fn builder_rejects_bar0_base_above_32bit_range() {
        match build_virtio_pci_rng_pair(0x1_0000_0000, 0x1234_5678) {
            Err(err) => assert_eq!(
                err,
                VirtioPciBuildError::Bar0BaseExceeds32Bit(0x1_0000_0000)
            ),
            Ok(_) => panic!("BAR0 base above 32-bit support should be rejected"),
        }
    }

    #[test]
    fn builder_rejects_bar4_base_above_32bit_range() {
        match build_virtio_pci_rng_pair(0xFFFF_F800, 0x1234_5678) {
            Err(err) => assert_eq!(
                err,
                VirtioPciBuildError::Bar4BaseExceeds32Bit(0x1_0000_0800)
            ),
            Ok(_) => panic!("BAR4 base above 32-bit support should be rejected"),
        }
    }

    #[test]
    fn builder_rejects_bar4_base_u64_overflow() {
        match build_virtio_pci_rng_pair(u64::MAX - (BAR0_SIZE - 1), 0x1234_5678) {
            Err(err) => assert_eq!(
                err,
                VirtioPciBuildError::Bar4BaseOverflow(u64::MAX - (BAR0_SIZE - 1))
            ),
            Ok(_) => panic!("BAR4 base overflow should be rejected"),
        }
    }

    #[test]
    fn queue_notify_sets_isr_and_read_clears_it() {
        const BASE: u64 = 0x0A00_4000;
        let (_endpoint, bar0, _bar4) = build_virtio_pci_rng_pair(BASE, 0x1234_5678).unwrap();
        let processor = bar0.pending_processor();
        let mut sys = HelmAddressSpace::new(FlatMem::new(0, 0x10000));
        sys.add_device(BASE, Box::new(bar0));

        sys.write(DESC_BASE, 8, DATA_BASE, AccessType::Store)
            .unwrap();
        sys.write(DESC_BASE + 8, 4, 32, AccessType::Store).unwrap();
        sys.write(DESC_BASE + 12, 2, 0x2, AccessType::Store)
            .unwrap();
        sys.write(DESC_BASE + 14, 2, 0, AccessType::Store).unwrap();
        sys.write(AVAIL_BASE + 4, 2, 0, AccessType::Store).unwrap();
        sys.write(AVAIL_BASE + 2, 2, 1, AccessType::Store).unwrap();
        sys.write(BASE + REG_QUEUE_SIZE_AND_VECTOR, 4, 8, AccessType::Store)
            .unwrap();
        sys.write(BASE + REG_QUEUE_DESC_LOW, 4, DESC_BASE, AccessType::Store)
            .unwrap();
        sys.write(
            BASE + REG_QUEUE_DRIVER_LOW,
            4,
            AVAIL_BASE,
            AccessType::Store,
        )
        .unwrap();
        sys.write(BASE + REG_QUEUE_DEVICE_LOW, 4, USED_BASE, AccessType::Store)
            .unwrap();
        sys.write(
            BASE + REG_QUEUE_ENABLE_AND_NOTIFY_OFF,
            4,
            1,
            AccessType::Store,
        )
        .unwrap();
        sys.write(BASE + NOTIFY_OFFSET, 4, 0, AccessType::Store)
            .unwrap();

        let result = processor.process_pending(&mut sys);
        assert!(result.queue_irq);
        let isr = sys.read(BASE + ISR_OFFSET, 4, AccessType::Load).unwrap();
        assert_eq!(isr, u64::from(VIRTIO_IRQ_VQUEUE));
        let isr_cleared = sys.read(BASE + ISR_OFFSET, 4, AccessType::Load).unwrap();
        assert_eq!(isr_cleared, 0);
    }

    #[test]
    fn masked_queue_vector_sets_pba_pending_bit() {
        const BASE: u64 = 0x0A00_6000;
        let (mut endpoint, bar0, bar4) = build_virtio_pci_rng_pair(BASE, 0x1234_5678).unwrap();
        let processor = bar0.pending_processor();
        let mut sys = HelmAddressSpace::new(FlatMem::new(0, 0x10000));
        sys.add_device(BASE, Box::new(bar0));
        sys.add_device(BASE + BAR0_SIZE, Box::new(bar4));

        sys.write(DESC_BASE, 8, DATA_BASE, AccessType::Store)
            .unwrap();
        sys.write(DESC_BASE + 8, 4, 32, AccessType::Store).unwrap();
        sys.write(DESC_BASE + 12, 2, 0x2, AccessType::Store)
            .unwrap();
        sys.write(DESC_BASE + 14, 2, 0, AccessType::Store).unwrap();
        sys.write(AVAIL_BASE + 4, 2, 0, AccessType::Store).unwrap();
        sys.write(AVAIL_BASE + 2, 2, 1, AccessType::Store).unwrap();

        endpoint.config_write(MSIX_CONTROL_OFFSET, 2, u32::from(MSIX_ENABLE_BIT));
        sys.write(
            BASE + BAR0_SIZE + MSIX_TABLE_OFFSET + MSIX_TABLE_STRIDE + 0x0C,
            4,
            1,
            AccessType::Store,
        )
        .unwrap();
        sys.write(
            BASE + REG_QUEUE_SIZE_AND_VECTOR,
            4,
            64 | (1u64 << 16),
            AccessType::Store,
        )
        .unwrap();
        sys.write(BASE + REG_QUEUE_DESC_LOW, 4, DESC_BASE, AccessType::Store)
            .unwrap();
        sys.write(
            BASE + REG_QUEUE_DRIVER_LOW,
            4,
            AVAIL_BASE,
            AccessType::Store,
        )
        .unwrap();
        sys.write(BASE + REG_QUEUE_DEVICE_LOW, 4, USED_BASE, AccessType::Store)
            .unwrap();
        sys.write(
            BASE + REG_QUEUE_ENABLE_AND_NOTIFY_OFF,
            4,
            1,
            AccessType::Store,
        )
        .unwrap();
        sys.write(BASE + NOTIFY_OFFSET, 4, 0, AccessType::Store)
            .unwrap();

        let result = processor.process_pending(&mut sys);
        assert!(result.queue_irq);
        let isr = sys.read(BASE + ISR_OFFSET, 4, AccessType::Load).unwrap();
        assert_eq!(isr, u64::from(VIRTIO_IRQ_VQUEUE));
        let pba = sys
            .read(BASE + BAR0_SIZE + MSIX_PBA_OFFSET, 4, AccessType::Load)
            .unwrap();
        assert_eq!(pba & 0b10, 0b10);
    }

    #[test]
    fn masked_config_vector_sets_pba_pending_bit() {
        const BASE: u64 = 0x0A00_8000;
        let (mut endpoint, mut bar0, mut bar4) =
            build_virtio_pci_rng_pair(BASE, 0x1234_5678).unwrap();

        endpoint.config_write(MSIX_CONTROL_OFFSET, 2, u32::from(MSIX_ENABLE_BIT));
        bar4.write(MSIX_TABLE_STRIDE + 0x0C, 4, 1);
        bar0.write(REG_MSIX_CONFIG_AND_QUEUE_COUNT, 4, 1);
        bar0.write(DEVICE_CFG_OFFSET, 4, 0xAA55);

        assert_eq!(bar0.read(ISR_OFFSET, 4), u64::from(VIRTIO_IRQ_CONFIG));
        let pba = bar4.read(MSIX_PBA_OFFSET, 4);
        assert_eq!(pba & 0b10, 0b10);
    }

    #[test]
    fn common_cfg_subword_accesses_use_little_endian_layout() {
        const BASE: u64 = 0x0A00_A000;
        let (_endpoint, mut bar0, _bar4) = build_virtio_pci_rng_pair(BASE, 0x1234_5678).unwrap();

        bar0.write(REG_DEVICE_STATUS_AND_QUEUE_SEL, 4, 0x0003_0034);

        assert_eq!(bar0.read(REG_DEVICE_STATUS_AND_QUEUE_SEL, 1), 0x34);
        assert_eq!(bar0.read(REG_DEVICE_STATUS_AND_QUEUE_SEL + 2, 2), 0x0003);
    }

    #[test]
    fn device_cfg_byte_reads_work_for_net_mac() {
        const BASE: u64 = 0x0A00_C000;
        let (_endpoint, mut bar0, _bar4) = build_virtio_pci_pair(
            Box::new(crate::net::VirtioNet::new([
                0x52, 0x54, 0x00, 0x12, 0x34, 0x56,
            ])),
            BASE,
        )
        .unwrap();

        assert_eq!(bar0.read(DEVICE_CFG_OFFSET + 1, 1), 0x54);
        assert_eq!(bar0.read(DEVICE_CFG_OFFSET + 4, 2), 0x5634);
    }

    #[test]
    fn msix_table_subword_write_updates_word() {
        const BASE: u64 = 0x0A00_E000;
        let (_endpoint, _bar0, mut bar4) = build_virtio_pci_rng_pair(BASE, 0x1234_5678).unwrap();

        bar4.write(0x08, 2, 0x3344);
        assert_eq!(bar4.read(0x08, 4), 0x3344);
        bar4.write(0x0C, 1, 0x01);
        assert_eq!(bar4.read(0x0C, 1), 0x01);
    }

    #[test]
    fn isr_byte_read_clears_status() {
        const BASE: u64 = 0x0A01_0000;
        let (_endpoint, mut bar0, _bar4) = build_virtio_pci_rng_pair(BASE, 0x1234_5678).unwrap();

        // Trigger a config change interrupt by writing device config
        bar0.write(DEVICE_CFG_OFFSET, 4, 0xAA);
        // ISR should have VIRTIO_IRQ_CONFIG (bit 1)
        assert_eq!(bar0.read(ISR_OFFSET, 1), u64::from(VIRTIO_IRQ_CONFIG));
        // Reading ISR should clear it
        assert_eq!(bar0.read(ISR_OFFSET, 1), 0);
    }

    #[test]
    fn bar0_cross_word_boundary_returns_zero() {
        const BASE: u64 = 0x0A01_2000;
        let (_endpoint, mut bar0, _bar4) = build_virtio_pci_rng_pair(BASE, 0x1234_5678).unwrap();

        // 2-byte read at offset 3 crosses word boundary
        assert_eq!(bar0.read(REG_DEVICE_FEATURE_SEL + 3, 2), 0);
        // 4-byte read at offset 1 crosses
        assert_eq!(bar0.read(REG_DEVICE_FEATURE_SEL + 1, 4), 0);
    }

    #[test]
    fn bar0_unsupported_width_returns_zero() {
        const BASE: u64 = 0x0A01_4000;
        let (_endpoint, mut bar0, _bar4) = build_virtio_pci_rng_pair(BASE, 0x1234_5678).unwrap();

        assert_eq!(bar0.read(REG_DEVICE_FEATURE_SEL, 3), 0);
        assert_eq!(bar0.read(REG_DEVICE_FEATURE_SEL, 8), 0);
    }

    #[test]
    fn bar0_unsupported_width_write_ignored() {
        const BASE: u64 = 0x0A01_6000;
        let (_endpoint, mut bar0, _bar4) = build_virtio_pci_rng_pair(BASE, 0x1234_5678).unwrap();

        bar0.write(REG_DEVICE_STATUS_AND_QUEUE_SEL, 4, 0x34);
        // size=3 write should be ignored
        bar0.write(REG_DEVICE_STATUS_AND_QUEUE_SEL, 3, 0);
        assert_eq!(bar0.read(REG_DEVICE_STATUS_AND_QUEUE_SEL, 4), 0x34);
    }

    #[test]
    fn bar4_cross_word_boundary_returns_zero() {
        const BASE: u64 = 0x0A01_8000;
        let (_endpoint, _bar0, mut bar4) = build_virtio_pci_rng_pair(BASE, 0x1234_5678).unwrap();

        bar4.write(0x00, 4, 0xDEAD_BEEF);
        // 2-byte read crossing word boundary
        assert_eq!(bar4.read(0x03, 2), 0);
    }

    #[test]
    fn bar4_unsupported_width_returns_zero() {
        const BASE: u64 = 0x0A01_A000;
        let (_endpoint, _bar0, mut bar4) = build_virtio_pci_rng_pair(BASE, 0x1234_5678).unwrap();

        assert_eq!(bar4.read(0x00, 5), 0);
    }

    #[test]
    fn pba_byte_read_extracts_correct_lane() {
        const BASE: u64 = 0x0A01_C000;
        let (mut endpoint, mut bar0, mut bar4) =
            build_virtio_pci_rng_pair(BASE, 0x1234_5678).unwrap();

        // Enable MSI-X, mask vector 0, pend it via config write
        endpoint.config_write(MSIX_CONTROL_OFFSET, 2, u32::from(MSIX_ENABLE_BIT));
        bar4.write(MSIX_TABLE_OFFSET + 0x0C, 4, 1); // mask vector 0
        bar0.write(REG_MSIX_CONFIG_AND_QUEUE_COUNT, 4, 0); // config vector = 0
        bar0.write(DEVICE_CFG_OFFSET, 4, 0xBB); // trigger config IRQ

        // PBA word should have bit 0 set for vector 0
        let pba_word = bar4.read(MSIX_PBA_OFFSET, 4);
        assert_ne!(pba_word & 0x1, 0);
        // Byte read of PBA should also show bit 0
        assert_ne!(bar4.read(MSIX_PBA_OFFSET, 1) & 0x1, 0);
    }
}
