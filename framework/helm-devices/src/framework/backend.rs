//! Backend abstractions -- `CharBackend` (serial I/O) and `BlockBackend` (disk I/O).
//!
//! Devices use these traits to communicate with the host without knowing
//! the concrete transport (PTY, socket, file, buffer, etc.).

use std::collections::VecDeque;

// ── Character Backend ───────────────────────────────────────────────────────

/// Character (byte-stream) backend -- used by UART-like devices.
///
/// The backend is the host-side transport for serial data. A UART device
/// writes bytes to its `CharBackend` (TX path) and reads bytes from it
/// (RX path). The backend decides where those bytes actually go: a PTY,
/// a TCP socket, a file, or an in-memory buffer for testing.
pub trait CharBackend: Send {
    /// Write bytes to the backend. Returns number of bytes accepted.
    fn write(&mut self, data: &[u8]) -> usize;

    /// Read a single byte from the backend, if available.
    ///
    /// Returns `None` if no data is ready.
    fn read(&mut self) -> Option<u8>;

    /// Check if the backend can accept at least one byte for writing.
    fn can_write(&self) -> bool;

    /// Check if the backend has at least one byte available for reading.
    fn can_read(&self) -> bool;
}

// ── Block Backend ───────────────────────────────────────────────────────────

/// Block (byte-addressed) backend -- used by disk-like devices.
///
/// Provides random-access reads and writes at arbitrary byte offsets.
/// The backend decides where blocks are actually stored: a file, a
/// memory buffer, or a network block device.
pub trait BlockBackend: Send {
    /// Read `buf.len()` bytes starting at byte `offset`.
    fn read_block(&mut self, offset: u64, buf: &mut [u8]);

    /// Write `buf.len()` bytes starting at byte `offset`.
    fn write_block(&mut self, offset: u64, buf: &[u8]);

    /// Total capacity in bytes.
    fn capacity(&self) -> u64;
}

// ── NullCharBackend ─────────────────────────────────────────────────────────

/// A character backend that discards all writes and never has data to read.
///
/// Useful as a default backend when no host transport is connected, or in
/// tests where serial output is irrelevant.
pub struct NullCharBackend;

impl CharBackend for NullCharBackend {
    fn write(&mut self, data: &[u8]) -> usize {
        // Accept all bytes and discard them.
        data.len()
    }

    fn read(&mut self) -> Option<u8> {
        None
    }

    fn can_write(&self) -> bool {
        true
    }

    fn can_read(&self) -> bool {
        false
    }
}

// ── BufferCharBackend ───────────────────────────────────────────────────────

/// An in-memory character backend backed by two `VecDeque` FIFOs.
///
/// Written bytes accumulate in the TX buffer and can be drained by test
/// code. Bytes injected into the RX buffer are returned by `read()`.
/// Primarily used in tests and headless operation.
pub struct BufferCharBackend {
    /// Bytes written by the device (TX path). Test code reads from here.
    tx: VecDeque<u8>,
    /// Bytes available for the device to read (RX path). Test code writes here.
    rx: VecDeque<u8>,
}

impl BufferCharBackend {
    /// Create a new buffer backend with empty TX and RX FIFOs.
    pub fn new() -> Self {
        Self {
            tx: VecDeque::new(),
            rx: VecDeque::new(),
        }
    }

    /// Inject bytes into the RX path (simulates host sending data to device).
    pub fn inject_rx(&mut self, data: &[u8]) {
        self.rx.extend(data);
    }

    /// Drain all bytes from the TX path (device output).
    pub fn drain_tx(&mut self) -> Vec<u8> {
        self.tx.drain(..).collect()
    }

    /// Peek at the TX buffer without draining.
    pub fn tx_bytes(&self) -> &VecDeque<u8> {
        &self.tx
    }
}

impl Default for BufferCharBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl CharBackend for BufferCharBackend {
    fn write(&mut self, data: &[u8]) -> usize {
        self.tx.extend(data);
        data.len()
    }

    fn read(&mut self) -> Option<u8> {
        self.rx.pop_front()
    }

    fn can_write(&self) -> bool {
        true
    }

    fn can_read(&self) -> bool {
        !self.rx.is_empty()
    }
}
