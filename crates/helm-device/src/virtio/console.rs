//! VirtIO console device (type 3) — spec 5.3.
//!
//! Provides a virtual serial console. Supports single-port and
//! multi-port configurations with emergency write.

use crate::virtio::features::*;
use crate::virtio::queue::Virtqueue;
use crate::virtio::transport::VirtioDeviceBackend;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;

/// Config space (spec 5.3.4).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VirtioConsoleConfig {
    pub cols: u16,
    pub rows: u16,
    pub max_nr_ports: u32,
    pub emerg_wr: u32,
}

impl Default for VirtioConsoleConfig {
    fn default() -> Self {
        Self {
            cols: 80,
            rows: 25,
            max_nr_ports: 1,
            emerg_wr: 0,
        }
    }
}

/// Console control message types (spec 5.3.6.2).
pub const VIRTIO_CONSOLE_DEVICE_READY: u16 = 0;
pub const VIRTIO_CONSOLE_DEVICE_ADD: u16 = 1;
pub const VIRTIO_CONSOLE_DEVICE_REMOVE: u16 = 2;
pub const VIRTIO_CONSOLE_PORT_READY: u16 = 3;
pub const VIRTIO_CONSOLE_CONSOLE_PORT: u16 = 4;
pub const VIRTIO_CONSOLE_RESIZE: u16 = 5;
pub const VIRTIO_CONSOLE_PORT_OPEN: u16 = 6;
pub const VIRTIO_CONSOLE_PORT_NAME: u16 = 7;

/// VirtIO console device.
pub struct VirtioConsole {
    config: VirtioConsoleConfig,
    config_bytes: Vec<u8>,
    /// Output buffer: data written by the guest.
    pub output: VecDeque<u8>,
    /// Input buffer: data to deliver to the guest.
    pub input: VecDeque<u8>,
    pub pending_irq: bool,
    multiport: bool,
}

impl VirtioConsole {
    pub fn new() -> Self {
        let config = VirtioConsoleConfig::default();
        let config_bytes = console_config_to_bytes(&config);
        Self {
            config,
            config_bytes,
            output: VecDeque::new(),
            input: VecDeque::new(),
            pending_irq: false,
            multiport: false,
        }
    }

    pub fn with_size(mut self, cols: u16, rows: u16) -> Self {
        self.config.cols = cols;
        self.config.rows = rows;
        self.config_bytes = console_config_to_bytes(&self.config);
        self
    }

    pub fn with_multiport(mut self, max_ports: u32) -> Self {
        self.multiport = true;
        self.config.max_nr_ports = max_ports;
        self.config_bytes = console_config_to_bytes(&self.config);
        self
    }

    /// Inject input data for the guest to read.
    pub fn inject_input(&mut self, data: &[u8]) {
        self.input.extend(data);
    }

    /// Drain output data written by the guest.
    pub fn drain_output(&mut self) -> Vec<u8> {
        self.output.drain(..).collect()
    }

    fn process_transmit(&mut self, queues: &mut [Virtqueue]) {
        let q = match queues.get_mut(1) {
            Some(Virtqueue::Split(q)) => q,
            _ => return,
        };

        while let Some(head) = q.pop_avail() {
            let chain = q.walk_chain(head);
            for desc in &chain {
                // Record output bytes
                for _ in 0..desc.len {
                    self.output.push_back(0); // simulated byte
                }
            }
            q.push_used(head, 0);
            self.pending_irq = true;
        }
    }
}

impl Default for VirtioConsole {
    fn default() -> Self {
        Self::new()
    }
}

impl VirtioDeviceBackend for VirtioConsole {
    fn device_id(&self) -> u32 {
        VIRTIO_DEV_CONSOLE
    }

    fn device_features(&self) -> u64 {
        let mut f = VIRTIO_F_VERSION_1 | VIRTIO_CONSOLE_F_SIZE | VIRTIO_CONSOLE_F_EMERG_WRITE;
        if self.multiport {
            f |= VIRTIO_CONSOLE_F_MULTIPORT;
        }
        f
    }

    fn config_size(&self) -> u32 {
        self.config_bytes.len() as u32
    }

    fn read_config(&self, offset: u32) -> u8 {
        self.config_bytes.get(offset as usize).copied().unwrap_or(0)
    }

    fn write_config(&mut self, offset: u32, value: u8) {
        // Emergency write at offset 8
        if offset >= 8 && offset < 12 {
            self.output.push_back(value);
        }
        if let Some(b) = self.config_bytes.get_mut(offset as usize) {
            *b = value;
        }
    }

    fn num_queues(&self) -> u16 {
        if self.multiport {
            4
        } else {
            2
        } // receiveq, transmitq [, ctrl_receiveq, ctrl_transmitq]
    }

    fn queue_notify(&mut self, queue_idx: u16, queues: &mut [Virtqueue]) {
        if queue_idx == 1 {
            self.process_transmit(queues);
        }
    }

    fn reset(&mut self) {
        self.output.clear();
        self.input.clear();
        self.pending_irq = false;
    }

    fn name(&self) -> &str {
        "virtio-console"
    }
}

fn console_config_to_bytes(config: &VirtioConsoleConfig) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(12);
    bytes.extend_from_slice(&config.cols.to_le_bytes());
    bytes.extend_from_slice(&config.rows.to_le_bytes());
    bytes.extend_from_slice(&config.max_nr_ports.to_le_bytes());
    bytes.extend_from_slice(&config.emerg_wr.to_le_bytes());
    bytes
}
