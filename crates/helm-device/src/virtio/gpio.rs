//! VirtIO GPIO device (type 43) — spec 5.17.

use crate::virtio::features::*;
use crate::virtio::queue::Virtqueue;
use crate::virtio::transport::VirtioDeviceBackend;
use serde::{Deserialize, Serialize};

/// GPIO message types (spec 5.17.6.1).
pub const VIRTIO_GPIO_MSG_GET_NAMES: u16 = 0x0001;
pub const VIRTIO_GPIO_MSG_GET_DIRECTION: u16 = 0x0002;
pub const VIRTIO_GPIO_MSG_SET_DIRECTION: u16 = 0x0003;
pub const VIRTIO_GPIO_MSG_GET_VALUE: u16 = 0x0004;
pub const VIRTIO_GPIO_MSG_SET_VALUE: u16 = 0x0005;
pub const VIRTIO_GPIO_MSG_IRQ_TYPE: u16 = 0x0006;

/// GPIO directions.
pub const VIRTIO_GPIO_DIRECTION_NONE: u8 = 0;
pub const VIRTIO_GPIO_DIRECTION_IN: u8 = 1;
pub const VIRTIO_GPIO_DIRECTION_OUT: u8 = 2;

/// Config space (spec 5.17.4).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct VirtioGpioConfig {
    pub ngpio: u16,
    pub _padding: u16,
    pub gpio_names_size: u32,
}

/// GPIO pin state.
#[derive(Debug, Clone, Default)]
pub struct GpioPin {
    pub direction: u8,
    pub value: u8,
    pub irq_type: u32,
}

pub struct VirtioGpio {
    config: VirtioGpioConfig,
    config_bytes: Vec<u8>,
    pub pins: Vec<GpioPin>,
    pub pending_irq: bool,
}

impl VirtioGpio {
    pub fn new(num_pins: u16) -> Self {
        let config = VirtioGpioConfig {
            ngpio: num_pins,
            _padding: 0,
            gpio_names_size: 0,
        };
        let config_bytes = gpio_config_to_bytes(&config);
        let pins = (0..num_pins).map(|_| GpioPin::default()).collect();
        Self {
            config,
            config_bytes,
            pins,
            pending_irq: false,
        }
    }

    pub fn set_pin(&mut self, pin: u16, value: u8) {
        if let Some(p) = self.pins.get_mut(pin as usize) {
            p.value = value;
        }
    }

    pub fn get_pin(&self, pin: u16) -> u8 {
        self.pins.get(pin as usize).map_or(0, |p| p.value)
    }

    /// Return the device configuration.
    pub fn config(&self) -> &VirtioGpioConfig {
        &self.config
    }
}

impl VirtioDeviceBackend for VirtioGpio {
    fn device_id(&self) -> u32 {
        VIRTIO_DEV_GPIO
    }
    fn device_features(&self) -> u64 {
        VIRTIO_F_VERSION_1 | VIRTIO_GPIO_F_IRQ
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
    } // requestq, eventq

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
        for p in &mut self.pins {
            *p = GpioPin::default();
        }
        self.pending_irq = false;
    }

    fn name(&self) -> &str {
        "virtio-gpio"
    }
}

fn gpio_config_to_bytes(c: &VirtioGpioConfig) -> Vec<u8> {
    let mut b = Vec::with_capacity(8);
    b.extend_from_slice(&c.ngpio.to_le_bytes());
    b.extend_from_slice(&c._padding.to_le_bytes());
    b.extend_from_slice(&c.gpio_names_size.to_le_bytes());
    b
}
