//! Bus transaction — carries all context for a device access.
//!
//! Inspired by Simics's transaction objects: a single struct flows through
//! bus bridges, IOMMU translation, and DMA engines without losing context.

use helm_core::types::Addr;

/// Attributes describing the initiator and memory properties of a transaction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TransactionAttrs {
    /// Which CPU or DMA engine initiated this access.
    pub initiator_id: u32,
    /// TrustZone NS bit — `true` means secure world.
    pub secure: bool,
    /// Whether the access is cacheable.
    pub cacheable: bool,
    /// Whether the access is privileged (e.g. EL1+).
    pub privileged: bool,
}

impl Default for TransactionAttrs {
    fn default() -> Self {
        Self {
            initiator_id: 0,
            secure: false,
            cacheable: true,
            privileged: false,
        }
    }
}

/// A bus transaction carrying all context for a device access.
///
/// Flows through the bus hierarchy: each bridge can inspect attributes,
/// translate the address, and accumulate stall cycles.
#[derive(Debug, Clone)]
pub struct Transaction {
    /// Absolute address on the originating bus.
    pub addr: Addr,
    /// Offset relative to the device's base address (filled in by the bus).
    pub offset: Addr,
    /// Access size in bytes: 1, 2, 4, 8, or 16.
    pub size: usize,
    /// Data buffer — up to 128 bits for SIMD/LDP/STP.
    pub data: [u8; 16],
    /// `true` for writes, `false` for reads.
    pub is_write: bool,
    /// Initiator and memory attributes.
    pub attrs: TransactionAttrs,
    /// Stall cycles accumulated as the transaction traverses the bus hierarchy.
    /// Each device and bus bridge adds to this value.
    pub stall_cycles: u64,
}

impl Transaction {
    /// Create a read transaction.
    pub fn read(addr: Addr, size: usize) -> Self {
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

    /// Create a write transaction with a u64 value (little-endian).
    pub fn write(addr: Addr, size: usize, value: u64) -> Self {
        let mut data = [0u8; 16];
        data[..8].copy_from_slice(&value.to_le_bytes());
        Self {
            addr,
            offset: 0,
            size,
            data,
            is_write: true,
            attrs: TransactionAttrs::default(),
            stall_cycles: 0,
        }
    }

    /// Create a write transaction with raw byte data.
    pub fn write_bytes(addr: Addr, bytes: &[u8]) -> Self {
        let mut data = [0u8; 16];
        let len = bytes.len().min(16);
        data[..len].copy_from_slice(&bytes[..len]);
        Self {
            addr,
            offset: 0,
            size: len,
            data,
            is_write: true,
            attrs: TransactionAttrs::default(),
            stall_cycles: 0,
        }
    }

    /// Read the data buffer as a little-endian u64.
    pub fn data_u64(&self) -> u64 {
        let mut buf = [0u8; 8];
        let len = self.size.min(8);
        buf[..len].copy_from_slice(&self.data[..len]);
        u64::from_le_bytes(buf)
    }

    /// Read the data buffer as a little-endian u32.
    pub fn data_u32(&self) -> u32 {
        let mut buf = [0u8; 4];
        let len = self.size.min(4);
        buf[..len].copy_from_slice(&self.data[..len]);
        u32::from_le_bytes(buf)
    }

    /// Set the data buffer from a u64 value (little-endian).
    pub fn set_data_u64(&mut self, value: u64) {
        self.data[..8].copy_from_slice(&value.to_le_bytes());
    }

    /// Set the data buffer from a u32 value (little-endian).
    pub fn set_data_u32(&mut self, value: u32) {
        self.data[..4].copy_from_slice(&value.to_le_bytes());
    }

    /// Builder: set transaction attributes.
    pub fn with_attrs(mut self, attrs: TransactionAttrs) -> Self {
        self.attrs = attrs;
        self
    }
}
