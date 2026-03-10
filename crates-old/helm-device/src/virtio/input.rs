//! VirtIO input device (type 18) — spec 5.8.
//!
//! Provides keyboard, mouse, and tablet input to the guest.
//! Uses Linux evdev event types for compatibility.

use crate::virtio::features::*;
use crate::virtio::queue::Virtqueue;
use crate::virtio::transport::VirtioDeviceBackend;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;

/// Input config select values (spec 5.8.2).
pub const VIRTIO_INPUT_CFG_UNSET: u8 = 0x00;
pub const VIRTIO_INPUT_CFG_ID_NAME: u8 = 0x01;
pub const VIRTIO_INPUT_CFG_ID_SERIAL: u8 = 0x02;
pub const VIRTIO_INPUT_CFG_ID_DEVIDS: u8 = 0x03;
pub const VIRTIO_INPUT_CFG_PROP_BITS: u8 = 0x10;
pub const VIRTIO_INPUT_CFG_EV_BITS: u8 = 0x11;
pub const VIRTIO_INPUT_CFG_ABS_INFO: u8 = 0x12;

/// Input event (matches Linux input_event).
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct VirtioInputEvent {
    pub event_type: u16,
    pub code: u16,
    pub value: u32,
}

/// Linux evdev event types.
pub const EV_SYN: u16 = 0x00;
pub const EV_KEY: u16 = 0x01;
pub const EV_REL: u16 = 0x02;
pub const EV_ABS: u16 = 0x03;
pub const EV_MSC: u16 = 0x04;
pub const EV_SW: u16 = 0x05;
pub const EV_LED: u16 = 0x11;
pub const EV_SND: u16 = 0x12;
pub const EV_REP: u16 = 0x14;

/// Input device subtype.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputDeviceKind {
    Keyboard,
    Mouse,
    Tablet,
}

/// Config space for the input device (dynamic, based on select/subsel).
#[derive(Debug, Clone)]
pub struct VirtioInputConfig {
    pub select: u8,
    pub subsel: u8,
    pub size: u8,
    pub data: [u8; 128],
}

impl Default for VirtioInputConfig {
    fn default() -> Self {
        Self {
            select: 0,
            subsel: 0,
            size: 0,
            data: [0u8; 128],
        }
    }
}

pub struct VirtioInput {
    kind: InputDeviceKind,
    device_name: String,
    config: VirtioInputConfig,
    config_bytes: Vec<u8>,
    /// Pending input events to deliver to the guest.
    pub event_queue: VecDeque<VirtioInputEvent>,
    pub pending_irq: bool,
}

impl VirtioInput {
    pub fn keyboard() -> Self {
        Self::new(InputDeviceKind::Keyboard, "HELM Virtual Keyboard")
    }

    pub fn mouse() -> Self {
        Self::new(InputDeviceKind::Mouse, "HELM Virtual Mouse")
    }

    pub fn tablet() -> Self {
        Self::new(InputDeviceKind::Tablet, "HELM Virtual Tablet")
    }

    fn new(kind: InputDeviceKind, name: &str) -> Self {
        let mut config = VirtioInputConfig::default();
        let name_bytes = name.as_bytes();
        let len = name_bytes.len().min(128);
        config.data[..len].copy_from_slice(&name_bytes[..len]);
        config.size = len as u8;
        config.select = VIRTIO_INPUT_CFG_ID_NAME;

        let config_bytes = input_config_to_bytes(&config);

        Self {
            kind,
            device_name: name.to_string(),
            config,
            config_bytes,
            event_queue: VecDeque::new(),
            pending_irq: false,
        }
    }

    /// Inject an input event.
    pub fn inject_event(&mut self, event_type: u16, code: u16, value: u32) {
        self.event_queue.push_back(VirtioInputEvent {
            event_type,
            code,
            value,
        });
    }

    /// Inject a key press and release.
    pub fn inject_key(&mut self, code: u16) {
        self.inject_event(EV_KEY, code, 1); // press
        self.inject_event(EV_SYN, 0, 0);
        self.inject_event(EV_KEY, code, 0); // release
        self.inject_event(EV_SYN, 0, 0);
    }

    fn deliver_events(&mut self, queues: &mut [Virtqueue]) {
        let q = match queues.get_mut(0) {
            Some(Virtqueue::Split(q)) => q,
            _ => return,
        };

        while let Some(_event) = self.event_queue.front() {
            if let Some(head) = q.pop_avail() {
                self.event_queue.pop_front();
                q.push_used(head, 8); // sizeof VirtioInputEvent
                self.pending_irq = true;
            } else {
                break;
            }
        }
    }
}

impl VirtioDeviceBackend for VirtioInput {
    fn device_id(&self) -> u32 {
        VIRTIO_DEV_INPUT
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

    fn write_config(&mut self, offset: u32, value: u8) {
        match offset {
            0 => {
                self.config.select = value;
                self.update_config_data();
            }
            1 => {
                self.config.subsel = value;
                self.update_config_data();
            }
            _ => {}
        }
    }

    fn num_queues(&self) -> u16 {
        2 // eventq, statusq
    }

    fn queue_notify(&mut self, queue_idx: u16, queues: &mut [Virtqueue]) {
        if queue_idx == 0 {
            self.deliver_events(queues);
        }
    }

    fn reset(&mut self) {
        self.event_queue.clear();
        self.pending_irq = false;
    }

    fn name(&self) -> &str {
        "virtio-input"
    }
}

impl VirtioInput {
    fn update_config_data(&mut self) {
        self.config.data = [0u8; 128];
        match self.config.select {
            VIRTIO_INPUT_CFG_ID_NAME => {
                let name = self.device_name.as_bytes();
                let len = name.len().min(128);
                self.config.data[..len].copy_from_slice(&name[..len]);
                self.config.size = len as u8;
            }
            VIRTIO_INPUT_CFG_ID_SERIAL => {
                let serial = b"HELM-INPUT-001";
                let len = serial.len().min(128);
                self.config.data[..len].copy_from_slice(&serial[..len]);
                self.config.size = len as u8;
            }
            VIRTIO_INPUT_CFG_ID_DEVIDS => {
                // bustype, vendor, product, version (each u16 LE)
                self.config.data[0..2].copy_from_slice(&6u16.to_le_bytes()); // BUS_VIRTUAL
                self.config.data[2..4].copy_from_slice(&0x484Cu16.to_le_bytes()); // "HL"
                let product = match self.kind {
                    InputDeviceKind::Keyboard => 1u16,
                    InputDeviceKind::Mouse => 2u16,
                    InputDeviceKind::Tablet => 3u16,
                };
                self.config.data[4..6].copy_from_slice(&product.to_le_bytes());
                self.config.data[6..8].copy_from_slice(&1u16.to_le_bytes()); // version
                self.config.size = 8;
            }
            VIRTIO_INPUT_CFG_EV_BITS => {
                // Report supported event types
                match self.kind {
                    InputDeviceKind::Keyboard => {
                        self.config.data[0] = 0x03; // SYN + KEY
                        self.config.size = 1;
                    }
                    InputDeviceKind::Mouse => {
                        self.config.data[0] = 0x07; // SYN + KEY + REL
                        self.config.size = 1;
                    }
                    InputDeviceKind::Tablet => {
                        self.config.data[0] = 0x0B; // SYN + KEY + ABS
                        self.config.size = 1;
                    }
                }
            }
            _ => {
                self.config.size = 0;
            }
        }
        self.config_bytes = input_config_to_bytes(&self.config);
    }
}

fn input_config_to_bytes(config: &VirtioInputConfig) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(131);
    bytes.push(config.select);
    bytes.push(config.subsel);
    bytes.push(config.size);
    bytes.extend_from_slice(&config.data);
    bytes
}
