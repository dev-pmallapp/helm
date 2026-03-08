//! VirtIO sound device (type 25) — spec 5.14.

use crate::virtio::features::*;
use crate::virtio::queue::Virtqueue;
use crate::virtio::transport::VirtioDeviceBackend;
use serde::{Deserialize, Serialize};

/// Sound control request types (spec 5.14.6.6).
pub const VIRTIO_SND_R_JACK_INFO: u32 = 1;
pub const VIRTIO_SND_R_JACK_REMAP: u32 = 2;
pub const VIRTIO_SND_R_PCM_INFO: u32 = 0x0100;
pub const VIRTIO_SND_R_PCM_SET_PARAMS: u32 = 0x0101;
pub const VIRTIO_SND_R_PCM_PREPARE: u32 = 0x0102;
pub const VIRTIO_SND_R_PCM_RELEASE: u32 = 0x0103;
pub const VIRTIO_SND_R_PCM_START: u32 = 0x0104;
pub const VIRTIO_SND_R_PCM_STOP: u32 = 0x0105;
pub const VIRTIO_SND_R_CHMAP_INFO: u32 = 0x0200;

/// PCM formats.
pub const VIRTIO_SND_PCM_FMT_IMA_ADPCM: u8 = 0;
pub const VIRTIO_SND_PCM_FMT_MU_LAW: u8 = 1;
pub const VIRTIO_SND_PCM_FMT_A_LAW: u8 = 2;
pub const VIRTIO_SND_PCM_FMT_S8: u8 = 3;
pub const VIRTIO_SND_PCM_FMT_U8: u8 = 4;
pub const VIRTIO_SND_PCM_FMT_S16: u8 = 5;
pub const VIRTIO_SND_PCM_FMT_U16: u8 = 6;
pub const VIRTIO_SND_PCM_FMT_S24: u8 = 7;
pub const VIRTIO_SND_PCM_FMT_U24: u8 = 8;
pub const VIRTIO_SND_PCM_FMT_S32: u8 = 9;
pub const VIRTIO_SND_PCM_FMT_U32: u8 = 10;
pub const VIRTIO_SND_PCM_FMT_FLOAT: u8 = 11;
pub const VIRTIO_SND_PCM_FMT_FLOAT64: u8 = 12;

/// PCM sample rates (as indices).
pub const VIRTIO_SND_PCM_RATE_5512: u8 = 0;
pub const VIRTIO_SND_PCM_RATE_8000: u8 = 1;
pub const VIRTIO_SND_PCM_RATE_11025: u8 = 2;
pub const VIRTIO_SND_PCM_RATE_16000: u8 = 3;
pub const VIRTIO_SND_PCM_RATE_22050: u8 = 4;
pub const VIRTIO_SND_PCM_RATE_32000: u8 = 5;
pub const VIRTIO_SND_PCM_RATE_44100: u8 = 6;
pub const VIRTIO_SND_PCM_RATE_48000: u8 = 7;
pub const VIRTIO_SND_PCM_RATE_64000: u8 = 8;
pub const VIRTIO_SND_PCM_RATE_88200: u8 = 9;
pub const VIRTIO_SND_PCM_RATE_96000: u8 = 10;
pub const VIRTIO_SND_PCM_RATE_176400: u8 = 11;
pub const VIRTIO_SND_PCM_RATE_192000: u8 = 12;
pub const VIRTIO_SND_PCM_RATE_384000: u8 = 13;

/// Config space (spec 5.14.4).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct VirtioSoundConfig {
    pub jacks: u32,
    pub streams: u32,
    pub chmaps: u32,
}

pub struct VirtioSound {
    config: VirtioSoundConfig,
    config_bytes: Vec<u8>,
    pub pending_irq: bool,
}

impl VirtioSound {
    pub fn new(jacks: u32, streams: u32, chmaps: u32) -> Self {
        let config = VirtioSoundConfig {
            jacks,
            streams,
            chmaps,
        };
        let config_bytes = sound_config_to_bytes(&config);
        Self {
            config,
            config_bytes,
            pending_irq: false,
        }
    }

    /// Return the device configuration.
    pub fn config(&self) -> &VirtioSoundConfig {
        &self.config
    }
}

impl Default for VirtioSound {
    fn default() -> Self {
        Self::new(0, 1, 1)
    }
}

impl VirtioDeviceBackend for VirtioSound {
    fn device_id(&self) -> u32 {
        VIRTIO_DEV_SOUND
    }
    fn device_features(&self) -> u64 {
        VIRTIO_F_VERSION_1
    }

    fn config_size(&self) -> u32 {
        self.config_bytes.len() as u32
    }
    fn read_config(&self, offset: u32) -> u8 {
        self.config_bytes.get(offset as usize).copied().unwrap_or(0)
    }
    fn write_config(&mut self, _offset: u32, _value: u8) {}

    fn num_queues(&self) -> u16 {
        4
    } // controlq, eventq, txq, rxq

    fn queue_notify(&mut self, queue_idx: u16, queues: &mut [Virtqueue]) {
        let q = match queues.get_mut(queue_idx as usize) {
            Some(Virtqueue::Split(q)) => q,
            _ => return,
        };
        while let Some(head) = q.pop_avail() {
            q.push_used(head, 0);
            self.pending_irq = true;
        }
    }

    fn reset(&mut self) {
        self.pending_irq = false;
    }
    fn name(&self) -> &str {
        "virtio-sound"
    }
}

fn sound_config_to_bytes(c: &VirtioSoundConfig) -> Vec<u8> {
    let mut b = Vec::with_capacity(12);
    b.extend_from_slice(&c.jacks.to_le_bytes());
    b.extend_from_slice(&c.streams.to_le_bytes());
    b.extend_from_slice(&c.chmaps.to_le_bytes());
    b
}
