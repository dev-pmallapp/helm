//! VirtIO console device backend (device type 3, VirtIO spec §5.3).
//!
//! Implements a bidirectional byte-stream console using two virtqueues:
//! - **Queue 0** (receiveq port 0): device→driver (TX from the device's perspective).
//! - **Queue 1** (transmitq port 0): driver→device (RX from the device's perspective).
//!
//! The console backend delegates actual I/O to a [`CharBackend`], making it
//! composable with the same backends used by the PL011 UART
//! (`NullCharBackend`, `BufferCharBackend`, `StdioCharBackend`, …).
//!
//! # Limitations
//!
//! - Multiport (`VIRTIO_CONSOLE_F_MULTIPORT`) is not implemented.
//! - Console size (`VIRTIO_CONSOLE_F_SIZE`) reports fixed 80×24.
//! - Actual buffer processing requires caller to supply guest memory;
//!   `queue_notify` only sets a pending flag.

use helm_devices::CharBackend;

use crate::proto::features::{VIRTIO_CONSOLE_F_SIZE, VIRTIO_DEVICE_CONSOLE, VIRTIO_F_VERSION_1};
use crate::proto::virtqueue::VirtQueue;
use crate::{VirtioBackend, VirtioPendingEvents};

// ── Config space (VirtIO spec §5.3.4) ────────────────────────────────────────

/// Console config space: cols (2), rows (2).
#[derive(Debug, Clone, Copy)]
struct ConsoleConfig {
    cols: u16,
    rows: u16,
}

impl Default for ConsoleConfig {
    fn default() -> Self {
        Self { cols: 80, rows: 24 }
    }
}

// ── VirtioConsole ─────────────────────────────────────────────────────────────

/// VirtIO console device backend.
///
/// Wraps a [`CharBackend`] to provide a console device for guest OSes.
/// The transport exposes two queues: receiveq (0) and transmitq (1).
///
/// For functional simulation, attach to a [`StdioCharBackend`](helm_devices::CharBackend)
/// or [`BufferCharBackend`](helm_devices::BufferCharBackend).
pub struct VirtioConsole {
    backend: Box<dyn CharBackend>,
    config: ConsoleConfig,
    /// True if there is pending output in queue 0 (receiveq, device→driver).
    rx_notify_pending: bool,
    /// True if there is pending input in queue 1 (transmitq, driver→device).
    tx_notify_pending: bool,
}

impl VirtioConsole {
    /// Create a new console backend with default 80×24 terminal dimensions.
    pub fn new(backend: Box<dyn CharBackend>) -> Self {
        Self {
            backend,
            config: ConsoleConfig::default(),
            rx_notify_pending: false,
            tx_notify_pending: false,
        }
    }

    /// Create a console with a specific terminal size.
    pub fn with_size(backend: Box<dyn CharBackend>, cols: u16, rows: u16) -> Self {
        Self {
            backend,
            config: ConsoleConfig { cols, rows },
            rx_notify_pending: false,
            tx_notify_pending: false,
        }
    }

    /// Write a byte slice from the guest (driver→device, transmitq path).
    ///
    /// Called by the caller after walking the transmitq descriptor chain.
    pub fn write_from_guest(&mut self, data: &[u8]) -> usize {
        self.backend.write(data)
    }

    /// Read one byte from the backend for delivery to the guest (device→driver, receiveq).
    ///
    /// Returns `None` if the backend has no data.
    pub fn read_for_guest(&mut self) -> Option<u8> {
        self.backend.read()
    }

    /// Check if the backend has data ready to send to the guest.
    pub fn has_rx_data(&self) -> bool {
        self.backend.can_read()
    }

    /// Clear the transmit (driver→device) notify pending flag.
    pub fn take_tx_pending(&mut self) -> bool {
        let v = self.tx_notify_pending;
        self.tx_notify_pending = false;
        v
    }

    /// Clear the receive (device→driver) notify pending flag.
    pub fn take_rx_pending(&mut self) -> bool {
        let v = self.rx_notify_pending;
        self.rx_notify_pending = false;
        v
    }
}

impl VirtioBackend for VirtioConsole {
    fn device_type(&self) -> u32 {
        VIRTIO_DEVICE_CONSOLE
    }

    fn vendor_id(&self) -> u32 {
        crate::proto::features::VIRTIO_VENDOR_ID as u32
    }

    fn device_features(&self) -> u64 {
        VIRTIO_F_VERSION_1 | VIRTIO_CONSOLE_F_SIZE
    }

    fn queue_max_size(&self, queue: usize) -> u32 {
        match queue {
            0 | 1 => 64, // receiveq and transmitq
            _ => 0,
        }
    }

    fn queue_notify(&mut self, queue: usize, _mem: Option<&mut dyn helm_core::ByteMem>) {
        match queue {
            0 => self.rx_notify_pending = true,
            1 => self.tx_notify_pending = true,
            _ => {}
        }
    }

    fn read_config(&self, offset: u32) -> u32 {
        match offset {
            // cols (2 bytes) at offset 0, rows (2 bytes) at offset 2 — packed as one u32
            0 => (self.config.rows as u32) << 16 | self.config.cols as u32,
            _ => 0,
        }
    }

    fn write_config(&mut self, _offset: u32, _val: u32) {
        // Console config is read-only
    }

    fn reset(&mut self) {
        self.rx_notify_pending = false;
        self.tx_notify_pending = false;
    }

    fn process_pending(
        &mut self,
        mem: &mut dyn helm_core::ByteMem,
        queues: &mut [VirtQueue],
    ) -> VirtioPendingEvents {
        let mut queue_irq = false;

        if self.take_tx_pending() {
            if let Some(queue) = queues.get_mut(1) {
                while let Some(head) = queue.pop_chain(mem) {
                    let chain = queue.collect_chain(mem, head);
                    let mut bytes = Vec::new();
                    for (addr, len, is_write) in chain {
                        if is_write {
                            continue;
                        }
                        let mut buf = vec![0u8; len as usize];
                        let _ = mem.read_bytes(addr, &mut buf);
                        bytes.extend_from_slice(&buf);
                    }
                    let _ = self.write_from_guest(&bytes);
                    queue_irq |= queue.push_used(mem, head, 0);
                }
            }
        }

        if self.take_rx_pending() || self.has_rx_data() {
            if let Some(queue) = queues.get_mut(0) {
                while self.has_rx_data() {
                    let Some(head) = queue.pop_chain(mem) else {
                        break;
                    };
                    let chain = queue.collect_chain(mem, head);
                    let mut written = 0u32;
                    for (addr, len, is_write) in chain {
                        if !is_write {
                            continue;
                        }
                        let mut buf = Vec::new();
                        for _ in 0..len {
                            let Some(byte) = self.read_for_guest() else {
                                break;
                            };
                            buf.push(byte);
                        }
                        if !buf.is_empty() {
                            let _ = mem.write_bytes(addr, &buf);
                            written = written.saturating_add(buf.len() as u32);
                        }
                        if !self.has_rx_data() {
                            break;
                        }
                    }
                    queue_irq |= queue.push_used(mem, head, written);
                }
            }
        }

        VirtioPendingEvents {
            queue_irq,
            config_irq: false,
        }
    }

    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use helm_devices::BufferCharBackend;

    #[test]
    fn console_device_type() {
        let c = VirtioConsole::new(Box::new(BufferCharBackend::new()));
        assert_eq!(c.device_type(), VIRTIO_DEVICE_CONSOLE);
    }

    #[test]
    fn console_queue_max_size() {
        let c = VirtioConsole::new(Box::new(BufferCharBackend::new()));
        assert_eq!(c.queue_max_size(0), 64);
        assert_eq!(c.queue_max_size(1), 64);
        assert_eq!(c.queue_max_size(2), 0);
    }

    #[test]
    fn console_config_size() {
        let c = VirtioConsole::with_size(Box::new(BufferCharBackend::new()), 132, 50);
        // read_config(0) returns rows<<16 | cols
        let val = c.read_config(0);
        assert_eq!(val & 0xFFFF, 132); // cols
        assert_eq!(val >> 16, 50); // rows
    }

    #[test]
    fn console_write_read_roundtrip() {
        let backend = BufferCharBackend::new();
        let mut c = VirtioConsole::new(Box::new(backend));
        c.write_from_guest(b"hello");
        // BufferCharBackend TX drains what device writes
        // We can't easily drain from here, so just verify no panic.
    }

    #[test]
    fn console_notify_pending() {
        let mut c = VirtioConsole::new(Box::new(BufferCharBackend::new()));
        c.queue_notify(1, None); // transmitq
        assert!(c.take_tx_pending());
        assert!(!c.take_tx_pending()); // cleared

        c.queue_notify(0, None); // receiveq
        assert!(c.take_rx_pending());
        assert!(!c.take_rx_pending());
    }

    #[test]
    fn console_features_include_size() {
        let c = VirtioConsole::new(Box::new(BufferCharBackend::new()));
        assert!(c.device_features() & VIRTIO_CONSOLE_F_SIZE != 0);
    }

    #[test]
    fn console_reset_clears_pending() {
        let mut c = VirtioConsole::new(Box::new(BufferCharBackend::new()));
        c.queue_notify(0, None);
        c.queue_notify(1, None);
        c.reset();
        assert!(!c.take_rx_pending());
        assert!(!c.take_tx_pending());
    }
}
