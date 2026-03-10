//! VirtIO crypto device (type 20) — spec 5.9.
//!
//! Virtual cryptographic accelerator supporting symmetric cipher,
//! hash, MAC, and AEAD operations.

use crate::virtio::features::*;
use crate::virtio::queue::Virtqueue;
use crate::virtio::transport::VirtioDeviceBackend;
use serde::{Deserialize, Serialize};

/// Crypto service types.
pub const VIRTIO_CRYPTO_SERVICE_CIPHER: u32 = 0;
pub const VIRTIO_CRYPTO_SERVICE_HASH: u32 = 1;
pub const VIRTIO_CRYPTO_SERVICE_MAC: u32 = 2;
pub const VIRTIO_CRYPTO_SERVICE_AEAD: u32 = 3;
pub const VIRTIO_CRYPTO_SERVICE_AKCIPHER: u32 = 4;

/// Config space (spec 5.9.4).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct VirtioCryptoConfig {
    pub status: u32,
    pub max_dataqueues: u32,
    pub crypto_services: u32,
    pub cipher_algo_l: u32,
    pub cipher_algo_h: u32,
    pub hash_algo: u32,
    pub mac_algo_l: u32,
    pub mac_algo_h: u32,
    pub aead_algo: u32,
    pub max_cipher_key_len: u32,
    pub max_auth_key_len: u32,
    pub akcipher_algo: u32,
    pub max_size: u64,
}

pub struct VirtioCrypto {
    config: VirtioCryptoConfig,
    config_bytes: Vec<u8>,
    pub pending_irq: bool,
}

impl VirtioCrypto {
    pub fn new() -> Self {
        let config = VirtioCryptoConfig {
            status: 0,
            max_dataqueues: 1,
            crypto_services: (1 << VIRTIO_CRYPTO_SERVICE_CIPHER)
                | (1 << VIRTIO_CRYPTO_SERVICE_HASH)
                | (1 << VIRTIO_CRYPTO_SERVICE_MAC)
                | (1 << VIRTIO_CRYPTO_SERVICE_AEAD),
            cipher_algo_l: 0xFFFF_FFFF,
            cipher_algo_h: 0,
            hash_algo: 0xFFFF_FFFF,
            mac_algo_l: 0xFFFF_FFFF,
            mac_algo_h: 0,
            aead_algo: 0xFFFF_FFFF,
            max_cipher_key_len: 64,
            max_auth_key_len: 128,
            akcipher_algo: 0,
            max_size: 0x10_0000,
        };
        let config_bytes = crypto_config_to_bytes(&config);
        Self {
            config,
            config_bytes,
            pending_irq: false,
        }
    }

    /// Return the device configuration.
    pub fn config(&self) -> &VirtioCryptoConfig {
        &self.config
    }
}

impl Default for VirtioCrypto {
    fn default() -> Self {
        Self::new()
    }
}

impl VirtioDeviceBackend for VirtioCrypto {
    fn device_id(&self) -> u32 {
        VIRTIO_DEV_CRYPTO
    }

    fn device_features(&self) -> u64 {
        VIRTIO_F_VERSION_1
            | VIRTIO_CRYPTO_F_REVISION_1
            | VIRTIO_CRYPTO_F_CIPHER_STATELESS_MODE
            | VIRTIO_CRYPTO_F_HASH_STATELESS_MODE
            | VIRTIO_CRYPTO_F_MAC_STATELESS_MODE
            | VIRTIO_CRYPTO_F_AEAD_STATELESS_MODE
    }

    fn config_size(&self) -> u32 {
        self.config_bytes.len() as u32
    }
    fn read_config(&self, offset: u32) -> u8 {
        self.config_bytes.get(offset as usize).copied().unwrap_or(0)
    }
    fn write_config(&mut self, _offset: u32, _value: u8) {}

    fn num_queues(&self) -> u16 {
        2
    } // dataq + controlq

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
        "virtio-crypto"
    }
}

fn crypto_config_to_bytes(c: &VirtioCryptoConfig) -> Vec<u8> {
    let mut b = Vec::with_capacity(56);
    b.extend_from_slice(&c.status.to_le_bytes());
    b.extend_from_slice(&c.max_dataqueues.to_le_bytes());
    b.extend_from_slice(&c.crypto_services.to_le_bytes());
    b.extend_from_slice(&c.cipher_algo_l.to_le_bytes());
    b.extend_from_slice(&c.cipher_algo_h.to_le_bytes());
    b.extend_from_slice(&c.hash_algo.to_le_bytes());
    b.extend_from_slice(&c.mac_algo_l.to_le_bytes());
    b.extend_from_slice(&c.mac_algo_h.to_le_bytes());
    b.extend_from_slice(&c.aead_algo.to_le_bytes());
    b.extend_from_slice(&c.max_cipher_key_len.to_le_bytes());
    b.extend_from_slice(&c.max_auth_key_len.to_le_bytes());
    b.extend_from_slice(&c.akcipher_algo.to_le_bytes());
    b.extend_from_slice(&c.max_size.to_le_bytes());
    b
}
