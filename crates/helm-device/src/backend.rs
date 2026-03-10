//! Device backends — data sources/sinks that plug into device frontends.
//!
//! Follows the QEMU pattern:
//! - **CharBackend**: character device (serial, console, PTY, socket, file)
//! - **BlockBackend**: block device (disk image, raw file, memory)
//! - **NetBackend**: network device (TAP, user-mode, socket)
//!
//! Frontends (PL011, VirtioBlk, etc.) own a `Box<dyn Backend>` and call
//! its methods to move data. The backend is injected at construction time,
//! making devices testable with in-memory buffers.

use std::collections::VecDeque;
use std::io;

// ═══════════════════════════════════════════════════════════════════════════
// Character backend
// ═══════════════════════════════════════════════════════════════════════════

/// Backend for character-oriented devices (UART, virtio-console, etc.).
pub trait CharBackend: Send + Sync {
    /// Write bytes from the device to the outside world (guest → host).
    fn write(&mut self, data: &[u8]) -> io::Result<usize>;

    /// Read bytes from the outside world into the device (host → guest).
    /// Returns 0 if no data is available (non-blocking).
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize>;

    /// Check if input data is available without consuming it.
    fn can_read(&self) -> bool;

    /// Flush output.
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }

    /// Backend name for diagnostics.
    fn backend_name(&self) -> &str;
}

/// In-memory character backend — useful for testing and programmatic I/O.
///
/// ```text
/// guest writes → output buffer (drain with output())
/// input buffer → guest reads   (fill with inject())
/// ```
pub struct BufferCharBackend {
    input: VecDeque<u8>,
    output: Vec<u8>,
}

impl BufferCharBackend {
    pub fn new() -> Self {
        Self {
            input: VecDeque::new(),
            output: Vec::new(),
        }
    }

    /// Inject data that the guest will read.
    pub fn inject(&mut self, data: &[u8]) {
        self.input.extend(data);
    }

    /// Drain output written by the guest.
    pub fn output(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.output)
    }

    /// Peek at output without draining.
    pub fn output_ref(&self) -> &[u8] {
        &self.output
    }

    /// Output as UTF-8 string (lossy).
    pub fn output_string(&self) -> String {
        String::from_utf8_lossy(&self.output).into_owned()
    }
}

impl Default for BufferCharBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl CharBackend for BufferCharBackend {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        self.output.extend_from_slice(data);
        Ok(data.len())
    }

    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = buf.len().min(self.input.len());
        for b in buf.iter_mut().take(n) {
            *b = self.input.pop_front().unwrap();
        }
        Ok(n)
    }

    fn can_read(&self) -> bool {
        !self.input.is_empty()
    }

    fn backend_name(&self) -> &str {
        "buffer"
    }
}

/// Null character backend — discards output, never has input.
pub struct NullCharBackend;

impl CharBackend for NullCharBackend {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        Ok(data.len())
    }
    fn read(&mut self, _buf: &mut [u8]) -> io::Result<usize> {
        Ok(0)
    }
    fn can_read(&self) -> bool {
        false
    }
    fn backend_name(&self) -> &str {
        "null"
    }
}

/// Stdio character backend — connects to the host's stdin/stdout.
pub struct StdioCharBackend;

impl CharBackend for StdioCharBackend {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        use std::io::Write;
        let mut stdout = io::stdout().lock();
        stdout.write_all(data)?;
        stdout.flush()?;
        Ok(data.len())
    }

    fn read(&mut self, _buf: &mut [u8]) -> io::Result<usize> {
        // Non-blocking stdin is platform-specific; return 0 for now.
        Ok(0)
    }

    fn can_read(&self) -> bool {
        false
    }

    fn backend_name(&self) -> &str {
        "stdio"
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Block backend
// ═══════════════════════════════════════════════════════════════════════════

/// Backend for block-oriented devices (virtio-blk, SCSI, etc.).
pub trait BlockBackend: Send + Sync {
    /// Read `buf.len()` bytes starting at byte offset `offset`.
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<usize>;

    /// Write `data` starting at byte offset `offset`.
    fn write_at(&mut self, offset: u64, data: &[u8]) -> io::Result<usize>;

    /// Total size in bytes.
    fn size(&self) -> u64;

    /// Whether the backend is read-only.
    fn is_readonly(&self) -> bool {
        false
    }

    /// Flush writes to stable storage.
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }

    fn backend_name(&self) -> &str;
}

/// In-memory block backend.
pub struct MemoryBlockBackend {
    data: Vec<u8>,
    readonly: bool,
}

impl MemoryBlockBackend {
    pub fn new(size: usize) -> Self {
        Self {
            data: vec![0u8; size],
            readonly: false,
        }
    }

    pub fn from_data(data: Vec<u8>) -> Self {
        Self {
            data,
            readonly: false,
        }
    }

    pub fn readonly(mut self) -> Self {
        self.readonly = true;
        self
    }
}

impl BlockBackend for MemoryBlockBackend {
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
        let start = offset as usize;
        let end = (start + buf.len()).min(self.data.len());
        if start >= self.data.len() {
            return Ok(0);
        }
        let n = end - start;
        buf[..n].copy_from_slice(&self.data[start..end]);
        Ok(n)
    }

    fn write_at(&mut self, offset: u64, data: &[u8]) -> io::Result<usize> {
        if self.readonly {
            return Err(io::Error::new(io::ErrorKind::PermissionDenied, "read-only"));
        }
        let start = offset as usize;
        let end = (start + data.len()).min(self.data.len());
        if start >= self.data.len() {
            return Ok(0);
        }
        let n = end - start;
        self.data[start..end].copy_from_slice(&data[..n]);
        Ok(n)
    }

    fn size(&self) -> u64 {
        self.data.len() as u64
    }

    fn is_readonly(&self) -> bool {
        self.readonly
    }

    fn backend_name(&self) -> &str {
        "memory"
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Network backend
// ═══════════════════════════════════════════════════════════════════════════

/// Backend for network devices (virtio-net, e1000, etc.).
pub trait NetBackend: Send + Sync {
    /// Send a packet from guest to the network.
    fn send(&mut self, packet: &[u8]) -> io::Result<usize>;

    /// Receive a packet from the network into `buf`.
    /// Returns 0 if no packet is available.
    fn recv(&mut self, buf: &mut [u8]) -> io::Result<usize>;

    /// Check if a packet is available.
    fn can_recv(&self) -> bool;

    fn backend_name(&self) -> &str;
}

/// In-memory network backend — loopback or packet capture.
pub struct BufferNetBackend {
    tx_queue: VecDeque<Vec<u8>>,
    rx_queue: VecDeque<Vec<u8>>,
}

impl BufferNetBackend {
    pub fn new() -> Self {
        Self {
            tx_queue: VecDeque::new(),
            rx_queue: VecDeque::new(),
        }
    }

    /// Inject a packet for the guest to receive.
    pub fn inject_rx(&mut self, packet: Vec<u8>) {
        self.rx_queue.push_back(packet);
    }

    /// Drain packets sent by the guest.
    pub fn drain_tx(&mut self) -> Vec<Vec<u8>> {
        self.tx_queue.drain(..).collect()
    }
}

impl Default for BufferNetBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl NetBackend for BufferNetBackend {
    fn send(&mut self, packet: &[u8]) -> io::Result<usize> {
        self.tx_queue.push_back(packet.to_vec());
        Ok(packet.len())
    }

    fn recv(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if let Some(packet) = self.rx_queue.pop_front() {
            let n = buf.len().min(packet.len());
            buf[..n].copy_from_slice(&packet[..n]);
            Ok(n)
        } else {
            Ok(0)
        }
    }

    fn can_recv(&self) -> bool {
        !self.rx_queue.is_empty()
    }

    fn backend_name(&self) -> &str {
        "buffer"
    }
}

/// Null network backend — drops all packets.
pub struct NullNetBackend;

impl NetBackend for NullNetBackend {
    fn send(&mut self, packet: &[u8]) -> io::Result<usize> {
        Ok(packet.len())
    }
    fn recv(&mut self, _buf: &mut [u8]) -> io::Result<usize> {
        Ok(0)
    }
    fn can_recv(&self) -> bool {
        false
    }
    fn backend_name(&self) -> &str {
        "null"
    }
}
