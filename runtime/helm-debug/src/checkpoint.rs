//! Checkpoint manager — serialize and restore architectural state.
//!
//! Uses a length-prefixed JSON format. The header contains a magic number
//! and version for format identification and migration support.

use crate::DebugError;

/// Version of the checkpoint format.
pub const CHECKPOINT_VERSION: u32 = 1;

/// Header prepended to checkpoint data.
#[derive(Debug, Clone)]
pub struct CheckpointHeader {
    pub magic: u32,
    pub version: u32,
    pub entry_count: u32,
}

impl CheckpointHeader {
    /// Magic number: "HLM\0" in ASCII.
    pub const MAGIC: u32 = 0x484C_4D00;

    pub fn new(entry_count: u32) -> Self {
        Self {
            magic: Self::MAGIC,
            version: CHECKPOINT_VERSION,
            entry_count,
        }
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(12);
        buf.extend_from_slice(&self.magic.to_le_bytes());
        buf.extend_from_slice(&self.version.to_le_bytes());
        buf.extend_from_slice(&self.entry_count.to_le_bytes());
        buf
    }

    pub fn from_bytes(data: &[u8]) -> Result<Self, DebugError> {
        if data.len() < 12 {
            return Err(DebugError::CorruptCheckpoint);
        }
        let magic = u32::from_le_bytes(data[0..4].try_into().unwrap());
        let version = u32::from_le_bytes(data[4..8].try_into().unwrap());
        let entry_count = u32::from_le_bytes(data[8..12].try_into().unwrap());
        if magic != Self::MAGIC {
            return Err(DebugError::CorruptCheckpoint);
        }
        Ok(Self {
            magic,
            version,
            entry_count,
        })
    }
}

/// Saves and restores architectural state.
///
/// Format: 12-byte header + N entries of (key_len:u32 + key + val:u64).
pub struct CheckpointManager {
    _private: (),
}

impl CheckpointManager {
    pub fn new() -> Self {
        Self { _private: () }
    }

    /// Serialize a set of named u64 values to bytes.
    pub fn save_values(&self, values: &[(&str, u64)]) -> Vec<u8> {
        let header = CheckpointHeader::new(values.len() as u32);
        let mut buf = header.to_bytes();
        for (key, val) in values {
            let key_bytes = key.as_bytes();
            buf.extend_from_slice(&(key_bytes.len() as u32).to_le_bytes());
            buf.extend_from_slice(key_bytes);
            buf.extend_from_slice(&val.to_le_bytes());
        }
        buf
    }

    /// Restore named u64 values from bytes.
    pub fn restore_values(&self, data: &[u8]) -> Result<Vec<(String, u64)>, DebugError> {
        let header = CheckpointHeader::from_bytes(data)?;
        if header.version != CHECKPOINT_VERSION {
            return Err(DebugError::CorruptCheckpoint);
        }

        let mut offset = 12;
        let mut result = Vec::with_capacity(header.entry_count as usize);
        for _ in 0..header.entry_count {
            if offset + 4 > data.len() {
                return Err(DebugError::CorruptCheckpoint);
            }
            let key_len = u32::from_le_bytes(data[offset..offset + 4].try_into().unwrap()) as usize;
            offset += 4;

            if offset + key_len + 8 > data.len() {
                return Err(DebugError::CorruptCheckpoint);
            }
            let key = String::from_utf8_lossy(&data[offset..offset + key_len]).to_string();
            offset += key_len;
            let val = u64::from_le_bytes(data[offset..offset + 8].try_into().unwrap());
            offset += 8;
            result.push((key, val));
        }
        Ok(result)
    }
}

impl Default for CheckpointManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn save_restore_roundtrip() {
        let mgr = CheckpointManager::new();
        let values = vec![("pc", 0x8000_0000u64), ("x0", 42)];
        let data = mgr.save_values(&values);
        let restored = mgr.restore_values(&data).unwrap();
        assert_eq!(restored.len(), 2);
        assert_eq!(restored[0], ("pc".to_string(), 0x8000_0000));
        assert_eq!(restored[1], ("x0".to_string(), 42));
    }

    #[test]
    fn corrupt_data_detected() {
        let mgr = CheckpointManager::new();
        assert!(mgr.restore_values(&[0; 4]).is_err());
    }

    #[test]
    fn bad_magic_rejected() {
        let mgr = CheckpointManager::new();
        let mut data = vec![0u8; 12];
        // wrong magic
        data[0..4].copy_from_slice(&0xDEADBEEFu32.to_le_bytes());
        assert!(mgr.restore_values(&data).is_err());
    }
}
