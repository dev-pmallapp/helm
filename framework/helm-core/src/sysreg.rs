//! System register map for `AArch64` system register dispatch.
//!
//! Supports two entry types:
//! - `Inline`: zero-cost field offset into `ArchState` (direct read/write)
//! - `Handler`: dynamic handler closure for side-effectful registers

use std::collections::HashMap;

/// System register encoding key: (op0, op1, crn, crm, op2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SysRegKey {
    /// Op0 field (2 bits).
    pub op0: u8,
    /// Op1 field (3 bits).
    pub op1: u8,
    /// `CRn` field (4 bits).
    pub crn: u8,
    /// `CRm` field (4 bits).
    pub crm: u8,
    /// Op2 field (3 bits).
    pub op2: u8,
}

impl SysRegKey {
    /// Construct from individual fields.
    pub const fn new(op0: u8, op1: u8, crn: u8, crm: u8, op2: u8) -> Self {
        Self { op0, op1, crn, crm, op2 }
    }

    /// Pack into a single u16 for compact storage.
    /// Layout: op0[1:0] | op1[2:0] | crn[3:0] | crm[3:0] | op2[2:0]
    pub const fn packed(&self) -> u16 {
        ((self.op0 as u16 & 0x3) << 14)
            | ((self.op1 as u16 & 0x7) << 11)
            | ((self.crn as u16 & 0xF) << 7)
            | ((self.crm as u16 & 0xF) << 3)
            | (self.op2 as u16 & 0x7)
    }

    /// Unpack from a packed u16.
    pub const fn from_packed(p: u16) -> Self {
        Self {
            op0: ((p >> 14) & 0x3) as u8,
            op1: ((p >> 11) & 0x7) as u8,
            crn: ((p >> 7) & 0xF) as u8,
            crm: ((p >> 3) & 0xF) as u8,
            op2: (p & 0x7) as u8,
        }
    }
}

/// A handler for system registers with side effects.
pub trait SysRegHandler: Send + Sync {
    /// Read the system register value.
    fn read(&self) -> u64;
    /// Write a value to the system register.
    fn write(&self, val: u64);
}

/// An entry in the system register map.
pub enum SysRegEntry {
    /// Direct field access -- zero overhead. The `offset` is the byte offset
    /// into the `ArchState` struct for this register's u64 field.
    Inline {
        /// Byte offset into `ArchState`.
        offset: usize,
    },
    /// Dynamic handler for registers with side effects (e.g., `CNTPCT_EL0`).
    Handler(Box<dyn SysRegHandler>),
}

/// Maps `AArch64` system register encodings to entries.
///
/// Built at `elaborate()` time; immutable during RUN.
pub struct SysRegMap {
    entries: HashMap<u16, SysRegEntry>,
}

impl SysRegMap {
    /// Create an empty map.
    pub fn new() -> Self {
        Self {
            entries: HashMap::new(),
        }
    }

    /// Register an inline (field-offset) system register.
    pub fn add_inline(&mut self, key: SysRegKey, offset: usize) {
        self.entries.insert(key.packed(), SysRegEntry::Inline { offset });
    }

    /// Register a handler-based system register.
    pub fn add_handler(&mut self, key: SysRegKey, handler: Box<dyn SysRegHandler>) {
        self.entries.insert(key.packed(), SysRegEntry::Handler(handler));
    }

    /// Look up an entry by encoding.
    pub fn lookup(&self, key: SysRegKey) -> Option<&SysRegEntry> {
        self.entries.get(&key.packed())
    }

    /// Look up by packed key.
    pub fn lookup_packed(&self, packed: u16) -> Option<&SysRegEntry> {
        self.entries.get(&packed)
    }

    /// Number of registered entries.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the map is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

impl Default for SysRegMap {
    fn default() -> Self {
        Self::new()
    }
}

// Well-known system register keys (commonly used in AArch64).
// Encodings from ARM Architecture Reference Manual.

/// `MPIDR_EL1`: op0=3, op1=0, crn=0, crm=0, op2=5
pub const SYSREG_MPIDR_EL1: SysRegKey = SysRegKey::new(3, 0, 0, 0, 5);

/// `SCTLR_EL1`: op0=3, op1=0, crn=1, crm=0, op2=0
pub const SYSREG_SCTLR_EL1: SysRegKey = SysRegKey::new(3, 0, 1, 0, 0);

/// `TTBR0_EL1`: op0=3, op1=0, crn=2, crm=0, op2=0
pub const SYSREG_TTBR0_EL1: SysRegKey = SysRegKey::new(3, 0, 2, 0, 0);

/// `CNTPCT_EL0`: op0=3, op1=3, crn=14, crm=0, op2=1
pub const SYSREG_CNTPCT_EL0: SysRegKey = SysRegKey::new(3, 3, 14, 0, 1);

/// `ICC_IAR1_EL1`: op0=3, op1=0, crn=12, crm=12, op2=0
pub const SYSREG_ICC_IAR1_EL1: SysRegKey = SysRegKey::new(3, 0, 12, 12, 0);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pack_unpack_roundtrip() {
        let keys = [
            SYSREG_MPIDR_EL1,
            SYSREG_SCTLR_EL1,
            SYSREG_TTBR0_EL1,
            SYSREG_CNTPCT_EL0,
            SYSREG_ICC_IAR1_EL1,
        ];
        for key in keys {
            let packed = key.packed();
            let unpacked = SysRegKey::from_packed(packed);
            assert_eq!(key, unpacked, "roundtrip failed for {key:?}");
        }
    }

    #[test]
    fn inline_entry() {
        let mut map = SysRegMap::new();
        map.add_inline(SYSREG_MPIDR_EL1, 42);
        assert!(matches!(
            map.lookup(SYSREG_MPIDR_EL1),
            Some(SysRegEntry::Inline { offset: 42 })
        ));
    }

    #[test]
    fn handler_entry() {
        struct TestHandler;
        impl SysRegHandler for TestHandler {
            fn read(&self) -> u64 { 0xDEAD }
            fn write(&self, _val: u64) {}
        }
        let mut map = SysRegMap::new();
        map.add_handler(SYSREG_CNTPCT_EL0, Box::new(TestHandler));
        assert!(matches!(
            map.lookup(SYSREG_CNTPCT_EL0),
            Some(SysRegEntry::Handler(_))
        ));
    }

    #[test]
    fn missing_entry_returns_none() {
        let map = SysRegMap::new();
        assert!(map.lookup(SYSREG_MPIDR_EL1).is_none());
    }
}
