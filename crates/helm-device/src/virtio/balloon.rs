//! VirtIO memory balloon device (type 5) — spec 5.5.
//!
//! Allows the host to reclaim guest memory by "inflating" the balloon
//! (guest gives pages to device) or return it by "deflating".

use crate::virtio::features::*;
use crate::virtio::queue::Virtqueue;
use crate::virtio::transport::VirtioDeviceBackend;
use serde::{Deserialize, Serialize};

/// Config space (spec 5.5.4).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct VirtioBalloonConfig {
    /// Target number of 4 KB pages the balloon should hold.
    pub num_pages: u32,
    /// Actual number of 4 KB pages currently held.
    pub actual: u32,
    /// Free page hint command ID.
    pub free_page_hint_cmd_id: u32,
    /// Poison page value.
    pub poison_val: u32,
}

/// Balloon statistics tags (spec 5.5.6.3).
pub const VIRTIO_BALLOON_S_SWAP_IN: u16 = 0;
pub const VIRTIO_BALLOON_S_SWAP_OUT: u16 = 1;
pub const VIRTIO_BALLOON_S_MAJFLT: u16 = 2;
pub const VIRTIO_BALLOON_S_MINFLT: u16 = 3;
pub const VIRTIO_BALLOON_S_MEMFREE: u16 = 4;
pub const VIRTIO_BALLOON_S_MEMTOT: u16 = 5;
pub const VIRTIO_BALLOON_S_AVAIL: u16 = 6;
pub const VIRTIO_BALLOON_S_CACHES: u16 = 7;
pub const VIRTIO_BALLOON_S_HTLB_PGALLOC: u16 = 8;
pub const VIRTIO_BALLOON_S_HTLB_PGFAIL: u16 = 9;

pub struct VirtioBalloon {
    config: VirtioBalloonConfig,
    config_bytes: Vec<u8>,
    /// Pages currently held by the balloon.
    pub inflated_pages: Vec<u32>,
    pub pending_irq: bool,
}

impl VirtioBalloon {
    pub fn new() -> Self {
        let config = VirtioBalloonConfig::default();
        let config_bytes = balloon_config_to_bytes(&config);
        Self {
            config,
            config_bytes,
            inflated_pages: Vec::new(),
            pending_irq: false,
        }
    }

    /// Set the target number of pages for the balloon.
    pub fn set_target(&mut self, num_pages: u32) {
        self.config.num_pages = num_pages;
        self.config_bytes = balloon_config_to_bytes(&self.config);
    }

    fn process_inflate(&mut self, queues: &mut [Virtqueue]) {
        let q = match queues.get_mut(0) {
            Some(Virtqueue::Split(q)) => q,
            _ => return,
        };
        while let Some(head) = q.pop_avail() {
            let chain = q.walk_chain(head);
            // Each descriptor contains an array of u32 PFNs.
            let pfns = chain.iter().map(|d| d.len / 4).sum::<u32>();
            for i in 0..pfns {
                self.inflated_pages.push(i);
            }
            self.config.actual = self.inflated_pages.len() as u32;
            self.config_bytes = balloon_config_to_bytes(&self.config);
            q.push_used(head, 0);
            self.pending_irq = true;
        }
    }

    fn process_deflate(&mut self, queues: &mut [Virtqueue]) {
        let q = match queues.get_mut(1) {
            Some(Virtqueue::Split(q)) => q,
            _ => return,
        };
        while let Some(head) = q.pop_avail() {
            let chain = q.walk_chain(head);
            let pfns = chain.iter().map(|d| d.len / 4).sum::<u32>();
            for _ in 0..pfns {
                self.inflated_pages.pop();
            }
            self.config.actual = self.inflated_pages.len() as u32;
            self.config_bytes = balloon_config_to_bytes(&self.config);
            q.push_used(head, 0);
            self.pending_irq = true;
        }
    }
}

impl Default for VirtioBalloon {
    fn default() -> Self {
        Self::new()
    }
}

impl VirtioDeviceBackend for VirtioBalloon {
    fn device_id(&self) -> u32 {
        VIRTIO_DEV_BALLOON
    }

    fn device_features(&self) -> u64 {
        VIRTIO_F_VERSION_1
            | VIRTIO_BALLOON_F_DEFLATE_ON_OOM
            | VIRTIO_BALLOON_F_STATS_VQ
            | VIRTIO_BALLOON_F_FREE_PAGE_HINT
            | VIRTIO_BALLOON_F_REPORTING
    }

    fn config_size(&self) -> u32 {
        self.config_bytes.len() as u32
    }

    fn read_config(&self, offset: u32) -> u8 {
        self.config_bytes.get(offset as usize).copied().unwrap_or(0)
    }

    fn write_config(&mut self, offset: u32, value: u8) {
        if let Some(b) = self.config_bytes.get_mut(offset as usize) {
            *b = value;
        }
        // Guest writes to `actual` field at offset 4
        if offset >= 4 && offset < 8 {
            let idx = (offset - 4) as usize;
            let mut bytes = self.config.actual.to_le_bytes();
            bytes[idx] = value;
            self.config.actual = u32::from_le_bytes(bytes);
        }
    }

    fn num_queues(&self) -> u16 {
        5 // inflateq, deflateq, statsq, free_page_vq, reporting_vq
    }

    fn queue_notify(&mut self, queue_idx: u16, queues: &mut [Virtqueue]) {
        match queue_idx {
            0 => self.process_inflate(queues),
            1 => self.process_deflate(queues),
            _ => {}
        }
    }

    fn reset(&mut self) {
        self.inflated_pages.clear();
        self.config.actual = 0;
        self.config_bytes = balloon_config_to_bytes(&self.config);
        self.pending_irq = false;
    }

    fn name(&self) -> &str {
        "virtio-balloon"
    }
}

fn balloon_config_to_bytes(config: &VirtioBalloonConfig) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(16);
    bytes.extend_from_slice(&config.num_pages.to_le_bytes());
    bytes.extend_from_slice(&config.actual.to_le_bytes());
    bytes.extend_from_slice(&config.free_page_hint_cmd_id.to_le_bytes());
    bytes.extend_from_slice(&config.poison_val.to_le_bytes());
    bytes
}
