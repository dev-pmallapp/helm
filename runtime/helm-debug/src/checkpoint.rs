//! Checkpoint manager — serialize and restore architectural state.
//!
//! Uses a length-prefixed JSON format. The header contains a magic number
//! and version for format identification and migration support.

use std::collections::HashMap;

use crate::{
    breakpoint::BreakpointIntent, watchpoint::WatchpointIntent, BreakAction, BreakpointEngine,
    DebugError, WatchAction, WatchKind, WatchpointEngine,
};

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

/// Checkpointable debug intent for native breakpoint/watchpoint engines.
#[derive(Debug, Clone, Default)]
pub struct DebugIntentCheckpoint {
    pub breakpoints: Option<Vec<BreakpointIntent>>,
    pub watchpoints: Option<Vec<WatchpointIntent>>,
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

impl DebugIntentCheckpoint {
    pub fn capture(
        breakpoints: Option<&BreakpointEngine>,
        watchpoints: Option<&WatchpointEngine>,
    ) -> Self {
        Self {
            breakpoints: breakpoints.map(BreakpointEngine::snapshot_intent),
            watchpoints: watchpoints.map(WatchpointEngine::snapshot_intent),
        }
    }

    pub fn append_values(&self, values: &mut Vec<(String, u64)>) {
        if let Some(breakpoints) = &self.breakpoints {
            values.push((
                "debug.breakpoints.count".to_string(),
                breakpoints.len() as u64,
            ));
            for (idx, bp) in breakpoints.iter().enumerate() {
                let prefix = format!("debug.breakpoints.{idx}");
                values.push((format!("{prefix}.addr"), bp.addr));
                values.push((format!("{prefix}.enabled"), u64::from(bp.enabled)));
                values.push((format!("{prefix}.hit_count"), bp.hit_count));
                let (kind, arg) = bp.action.checkpoint_fields();
                values.push((format!("{prefix}.action_kind"), kind));
                values.push((format!("{prefix}.action_arg"), arg));
            }
        }

        if let Some(watchpoints) = &self.watchpoints {
            values.push((
                "debug.watchpoints.count".to_string(),
                watchpoints.len() as u64,
            ));
            for (idx, wp) in watchpoints.iter().enumerate() {
                let prefix = format!("debug.watchpoints.{idx}");
                values.push((format!("{prefix}.start"), wp.start));
                values.push((format!("{prefix}.size"), wp.size));
                values.push((format!("{prefix}.enabled"), u64::from(wp.enabled)));
                values.push((format!("{prefix}.kind"), wp.kind.checkpoint_value()));
                let (kind, arg) = wp.action.checkpoint_fields();
                values.push((format!("{prefix}.action_kind"), kind));
                values.push((format!("{prefix}.action_arg"), arg));
            }
        }
    }

    pub fn from_restored_values(restored: &[(String, u64)]) -> Self {
        let map: HashMap<&str, u64> = restored.iter().map(|(k, v)| (k.as_str(), *v)).collect();

        let breakpoints = map.get("debug.breakpoints.count").copied().map(|count| {
            let mut out = Vec::with_capacity(count as usize);
            for idx in 0..count {
                let prefix = format!("debug.breakpoints.{idx}");
                let Some(addr) = map.get(format!("{prefix}.addr").as_str()).copied() else {
                    continue;
                };
                let enabled = map
                    .get(format!("{prefix}.enabled").as_str())
                    .copied()
                    .unwrap_or(1)
                    != 0;
                let hit_count = map
                    .get(format!("{prefix}.hit_count").as_str())
                    .copied()
                    .unwrap_or(0);
                let action_kind = map
                    .get(format!("{prefix}.action_kind").as_str())
                    .copied()
                    .unwrap_or(0);
                let action_arg = map
                    .get(format!("{prefix}.action_arg").as_str())
                    .copied()
                    .unwrap_or(0);
                out.push(BreakpointIntent {
                    addr,
                    action: BreakAction::from_checkpoint_fields(action_kind, action_arg),
                    enabled,
                    hit_count,
                });
            }
            out
        });

        let watchpoints = map.get("debug.watchpoints.count").copied().map(|count| {
            let mut out = Vec::with_capacity(count as usize);
            for idx in 0..count {
                let prefix = format!("debug.watchpoints.{idx}");
                let Some(start) = map.get(format!("{prefix}.start").as_str()).copied() else {
                    continue;
                };
                let size = map
                    .get(format!("{prefix}.size").as_str())
                    .copied()
                    .unwrap_or(0);
                let enabled = map
                    .get(format!("{prefix}.enabled").as_str())
                    .copied()
                    .unwrap_or(1)
                    != 0;
                let kind = map
                    .get(format!("{prefix}.kind").as_str())
                    .copied()
                    .map(WatchKind::from_checkpoint_value)
                    .unwrap_or(WatchKind::Write);
                let action_kind = map
                    .get(format!("{prefix}.action_kind").as_str())
                    .copied()
                    .unwrap_or(0);
                let action_arg = map
                    .get(format!("{prefix}.action_arg").as_str())
                    .copied()
                    .unwrap_or(0);
                out.push(WatchpointIntent {
                    start,
                    size,
                    kind,
                    action: WatchAction::from_checkpoint_fields(action_kind, action_arg),
                    enabled,
                });
            }
            out
        });

        Self {
            breakpoints,
            watchpoints,
        }
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

    #[test]
    fn debug_intent_roundtrip_preserves_breakpoints_and_watchpoints() {
        let mut breakpoints = BreakpointEngine::new();
        let bp_id = breakpoints.add(0x1000, BreakAction::Log);
        breakpoints.check(0x1000);
        breakpoints.set_enabled(bp_id, false);

        let mut watchpoints = WatchpointEngine::new();
        let wp_id = watchpoints.add(0x2000, 16, WatchKind::ReadWrite, WatchAction::Break);
        watchpoints.set_enabled(wp_id, false);

        let mut values = Vec::new();
        DebugIntentCheckpoint::capture(Some(&breakpoints), Some(&watchpoints))
            .append_values(&mut values);
        let refs: Vec<(&str, u64)> = values.iter().map(|(k, v)| (k.as_str(), *v)).collect();

        let mgr = CheckpointManager::new();
        let restored = mgr.restore_values(&mgr.save_values(&refs)).unwrap();
        let intent = DebugIntentCheckpoint::from_restored_values(&restored);

        let breakpoints = intent.breakpoints.expect("breakpoints intent");
        assert_eq!(breakpoints.len(), 1);
        assert_eq!(breakpoints[0].addr, 0x1000);
        assert!(!breakpoints[0].enabled);
        assert_eq!(breakpoints[0].hit_count, 1);
        assert!(matches!(breakpoints[0].action, BreakAction::Log));

        let watchpoints = intent.watchpoints.expect("watchpoints intent");
        assert_eq!(watchpoints.len(), 1);
        assert_eq!(watchpoints[0].start, 0x2000);
        assert_eq!(watchpoints[0].size, 16);
        assert!(!watchpoints[0].enabled);
        assert!(matches!(watchpoints[0].kind, WatchKind::ReadWrite));
        assert!(matches!(watchpoints[0].action, WatchAction::Break));
    }
}
