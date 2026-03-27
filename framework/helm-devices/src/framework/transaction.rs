//! Bus transaction types for full-system simulation with timing.
//!
//! [`Transaction`] carries the full bus context through the bus hierarchy:
//! address, data, access attributes, and accumulated stall cycles.
//!
//! In functional-emulation (FE/SE) mode, the simplified
//! [`Device::read()`](super::device::Device::read) /
//! [`Device::write()`](super::device::Device::write) path is used instead.
//! In full-system (FS) mode with timing, transactions flow through bus
//! bridges, accumulate latency, and arrive at the target device.

/// A bus transaction carrying full context through the bus hierarchy.
///
/// Created by the CPU or DMA engine, flows through bus bridges,
/// accumulates latency, and arrives at the target device.
#[repr(C)]
#[derive(Debug, Clone)]
pub struct Transaction {
    /// Absolute address on the originating bus segment.
    pub addr: u64,

    /// Offset relative to the target device's mapped base.
    /// Set by the address map dispatch before calling the device.
    pub offset: u64,

    /// Access size in bytes: 1, 2, 4, 8, or 16 (for SIMD/LDP/STP).
    pub size: usize,

    /// Data buffer -- up to 128 bits for SIMD paired load/store.
    /// For reads, the device fills this buffer.
    /// For writes, the initiator fills this buffer.
    pub data: [u8; 16],

    /// `true` = write, `false` = read.
    pub is_write: bool,

    /// Initiator and access attributes.
    pub attrs: TransactionAttrs,

    /// Accumulated stall cycles through the bus hierarchy.
    /// Each bus bridge and device adds its latency contribution.
    /// The timing model reads this after the transaction completes.
    pub stall_cycles: u64,
}

/// Attributes carried by every transaction.
///
/// These describe the initiator and access properties. Bus bridges
/// and devices inspect these to make routing and access-control decisions.
#[repr(C)]
#[derive(Debug, Clone, Default)]
pub struct TransactionAttrs {
    /// Initiator ID -- CPU core index or DMA engine ID.
    pub initiator_id: u32,

    /// TrustZone secure bit -- `true` for Secure world accesses.
    pub secure: bool,

    /// Cacheability -- `true` for cacheable accesses.
    pub cacheable: bool,

    /// Privilege level -- `true` for privileged (EL1+) accesses.
    pub privileged: bool,

    /// SMMU stream ID for DMA transactions.
    /// `None` = CPU-initiated (not subject to SMMU translation).
    /// `Some(sid)` = device DMA with the given stream ID.
    pub stream_id: Option<u32>,

    /// SMMU sub-stream ID (SubstreamID) for multi-context devices.
    /// Only meaningful when `stream_id` is `Some`.
    pub sub_stream_id: Option<u32>,
}

impl Transaction {
    /// Create a read transaction for the given address and size.
    pub fn read(addr: u64, size: usize) -> Self {
        Self {
            addr,
            offset: 0,
            size,
            data: [0u8; 16],
            is_write: false,
            attrs: TransactionAttrs::default(),
            stall_cycles: 0,
        }
    }

    /// Create a write transaction with the given data.
    ///
    /// Copies up to `size` bytes (max 16) from `data` into the transaction
    /// buffer.
    pub fn write(addr: u64, size: usize, data: &[u8]) -> Self {
        let mut txn = Self {
            addr,
            offset: 0,
            size,
            data: [0u8; 16],
            is_write: true,
            attrs: TransactionAttrs::default(),
            stall_cycles: 0,
        };
        let copy_len = size.min(16).min(data.len());
        txn.data[..copy_len].copy_from_slice(&data[..copy_len]);
        txn
    }

    /// Read the transaction data as a `u64` (little-endian).
    ///
    /// For accesses smaller than 8 bytes, only the low bytes are meaningful.
    pub fn data_u64(&self) -> u64 {
        let mut buf = [0u8; 8];
        let copy_len = self.size.min(8);
        buf[..copy_len].copy_from_slice(&self.data[..copy_len]);
        u64::from_le_bytes(buf)
    }

    /// Set the transaction data from a `u64` (little-endian).
    pub fn set_data_u64(&mut self, val: u64) {
        self.data[..8].copy_from_slice(&val.to_le_bytes());
    }
}
