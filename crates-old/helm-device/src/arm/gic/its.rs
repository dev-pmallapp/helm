//! GICv3 ITS (Interrupt Translation Service).
//!
//! The ITS translates MSI doorbells — `(DeviceID, EventID)` — into
//! physical LPI injections — `(target PE, INTID)` — using three
//! in-memory tables:
//!
//! | Table | Indexed by | Contains |
//! |-------|-----------|----------|
//! | Device Table | DeviceID | Pointer to ITT |
//! | ITT | EventID | INTID + CollectionID |
//! | Collection Table | CollectionID | Target PE |
//!
//! Software populates these via a command queue (`GITS_CBASER` /
//! `GITS_CWRITER` / `GITS_CREADR`).

use std::collections::BTreeMap;

// ── ITS register offsets ────────────────────────────────────────────────────

/// ITS Control Register.
pub const GITS_CTLR: u64 = 0x000;
/// ITS Implementer Identification Register.
pub const GITS_IIDR: u64 = 0x004;
/// ITS Type Register (64-bit, low word).
pub const GITS_TYPER_LO: u64 = 0x008;
/// ITS Type Register (64-bit, high word).
pub const GITS_TYPER_HI: u64 = 0x00C;
/// Command Queue Base Address Register (64-bit).
pub const GITS_CBASER: u64 = 0x080;
/// Command Queue Write Pointer (64-bit).
pub const GITS_CWRITER: u64 = 0x088;
/// Command Queue Read Pointer (64-bit).
pub const GITS_CREADR: u64 = 0x090;
/// Table Base Address Registers (8 × 64-bit).
pub const GITS_BASER_BASE: u64 = 0x100;

/// ITS command opcodes (8-bit field in command packets).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItsCommand {
    /// Map a DeviceID to an ITT.
    Mapd,
    /// Map `(DeviceID, EventID)` → `(INTID, CollectionID)`.
    Mapti,
    /// Shorthand `MAPTI` where `INTID = EventID`.
    Mapi,
    /// Map `CollectionID` → target PE.
    Mapc,
    /// Translate and inject an interrupt.
    Int,
    /// Invalidate cached LPI configuration.
    Inv,
    /// Invalidate all cached configuration for a collection.
    Invall,
    /// Ensure commands for a PE have completed.
    Sync,
    /// Remove an ITT entry.
    Discard,
    /// Move all collections from one PE to another.
    Movall,
    /// Map a virtual PE (GICv4).
    Vmapp,
    /// Map a virtual LPI (GICv4).
    Vmapti,
    /// Move a virtual LPI (GICv4).
    Vmovi,
    /// Invalidate all virtual LPIs for a vPE (GICv4).
    Vinvall,
    /// Synchronise virtual PE state (GICv4).
    Vsync,
    /// Virtual SGI (GICv4.1).
    Vsgi,
}

impl ItsCommand {
    /// Decode from the 8-bit opcode field.
    pub fn from_opcode(op: u8) -> Option<Self> {
        match op {
            0x08 => Some(Self::Mapd),
            0x0A => Some(Self::Mapti),
            0x0B => Some(Self::Mapi),
            0x09 => Some(Self::Mapc),
            0x03 => Some(Self::Int),
            0x0C => Some(Self::Inv),
            0x0D => Some(Self::Invall),
            0x05 => Some(Self::Sync),
            0x0F => Some(Self::Discard),
            0x0E => Some(Self::Movall),
            0x29 => Some(Self::Vmapp),
            0x2A => Some(Self::Vmapti),
            0x2B => Some(Self::Vmovi),
            0x2D => Some(Self::Vinvall),
            0x25 => Some(Self::Vsync),
            0x23 => Some(Self::Vsgi),
            _ => None,
        }
    }
}

/// Entry in an Interrupt Translation Table.
#[derive(Debug, Clone)]
pub struct IttEntry {
    /// Event ID.
    pub event_id: u32,
    /// Physical INTID (LPI) to generate.
    pub intid: u32,
    /// Collection ID for target-PE routing.
    pub collection: u16,
}

/// Per-device state held by the ITS.
#[derive(Debug, Clone)]
pub struct DeviceEntry {
    /// Interrupt Translation Table entries for this device.
    pub itt: Vec<IttEntry>,
}

/// GIC ITS state.
pub struct GicIts {
    /// Control register (bit 0 = Enabled).
    pub ctrl: u32,
    /// Type register (capabilities).
    pub typer: u64,
    /// Command queue base address.
    pub cmd_base: u64,
    /// Command queue write pointer.
    pub cmd_write: u64,
    /// Command queue read pointer.
    pub cmd_read: u64,
    /// `GITS_BASER0`–`GITS_BASER7`.
    pub table_bases: [u64; 8],
    /// Device table: `DeviceID` → ITT.
    pub devices: BTreeMap<u32, DeviceEntry>,
    /// Collection table: `CollectionID` → target PE index.
    pub collections: BTreeMap<u16, u32>,
    /// Pending injections produced by `INT` commands: `(target_pe, intid)`.
    pub pending_injections: Vec<(u32, u32)>,
}

impl GicIts {
    /// Create an empty ITS.
    pub fn new() -> Self {
        Self {
            ctrl: 0,
            typer: 0,
            cmd_base: 0,
            cmd_write: 0,
            cmd_read: 0,
            table_bases: [0; 8],
            devices: BTreeMap::new(),
            collections: BTreeMap::new(),
            pending_injections: Vec::new(),
        }
    }

    /// Whether the ITS is enabled.
    pub fn is_enabled(&self) -> bool {
        self.ctrl & 1 != 0
    }

    // ── Register access ─────────────────────────────────────────────────

    /// Handle an ITS register read.
    pub fn read(&self, offset: u64) -> u32 {
        match offset {
            GITS_CTLR => self.ctrl,
            GITS_IIDR => 0x0300_043B,
            GITS_TYPER_LO => self.typer as u32,
            GITS_TYPER_HI => (self.typer >> 32) as u32,
            GITS_CBASER => self.cmd_base as u32,
            0x084 => (self.cmd_base >> 32) as u32,
            GITS_CWRITER => self.cmd_write as u32,
            0x08C => (self.cmd_write >> 32) as u32,
            GITS_CREADR => self.cmd_read as u32,
            0x094 => (self.cmd_read >> 32) as u32,
            off if (GITS_BASER_BASE..GITS_BASER_BASE + 64).contains(&off) => {
                let idx = ((off - GITS_BASER_BASE) / 8) as usize;
                let low = (off - GITS_BASER_BASE).is_multiple_of(8);
                if idx < 8 {
                    if low {
                        self.table_bases[idx] as u32
                    } else {
                        (self.table_bases[idx] >> 32) as u32
                    }
                } else {
                    0
                }
            }
            _ => 0,
        }
    }

    /// Handle an ITS register write.
    pub fn write(&mut self, offset: u64, value: u32) {
        match offset {
            GITS_CTLR => self.ctrl = value & 1,
            GITS_CBASER => {
                self.cmd_base = (self.cmd_base & !0xFFFF_FFFF) | value as u64;
            }
            0x084 => {
                self.cmd_base = (self.cmd_base & 0xFFFF_FFFF) | ((value as u64) << 32);
            }
            GITS_CWRITER => {
                self.cmd_write = (self.cmd_write & !0xFFFF_FFFF) | value as u64;
                self.process_commands();
            }
            0x08C => {
                self.cmd_write = (self.cmd_write & 0xFFFF_FFFF) | ((value as u64) << 32);
            }
            off if (GITS_BASER_BASE..GITS_BASER_BASE + 64).contains(&off) => {
                let idx = ((off - GITS_BASER_BASE) / 8) as usize;
                let low = (off - GITS_BASER_BASE).is_multiple_of(8);
                if idx < 8 {
                    if low {
                        self.table_bases[idx] =
                            (self.table_bases[idx] & !0xFFFF_FFFF) | value as u64;
                    } else {
                        self.table_bases[idx] =
                            (self.table_bases[idx] & 0xFFFF_FFFF) | ((value as u64) << 32);
                    }
                }
            }
            _ => {}
        }
    }

    // ── Command execution ───────────────────────────────────────────────

    /// `MAPD` — map a DeviceID to an (initially empty) ITT.
    pub fn cmd_mapd(&mut self, device_id: u32, valid: bool) {
        if valid {
            self.devices
                .entry(device_id)
                .or_insert_with(|| DeviceEntry { itt: Vec::new() });
        } else {
            self.devices.remove(&device_id);
        }
    }

    /// `MAPTI` — map `(DeviceID, EventID)` → `(INTID, CollectionID)`.
    pub fn cmd_mapti(&mut self, device_id: u32, event_id: u32, intid: u32, collection: u16) {
        if let Some(dev) = self.devices.get_mut(&device_id) {
            dev.itt.retain(|e| e.event_id != event_id);
            dev.itt.push(IttEntry {
                event_id,
                intid,
                collection,
            });
        }
    }

    /// `MAPI` — shorthand `MAPTI` where `INTID = EventID`.
    pub fn cmd_mapi(&mut self, device_id: u32, event_id: u32, collection: u16) {
        self.cmd_mapti(device_id, event_id, event_id, collection);
    }

    /// `MAPC` — map `CollectionID` → target PE.
    pub fn cmd_mapc(&mut self, collection: u16, target_pe: u32, valid: bool) {
        if valid {
            self.collections.insert(collection, target_pe);
        } else {
            self.collections.remove(&collection);
        }
    }

    /// `INT` — translate and inject an interrupt.
    ///
    /// Returns `Some((target_pe, intid))` on success.
    pub fn cmd_int(&mut self, device_id: u32, event_id: u32) -> Option<(u32, u32)> {
        let dev = self.devices.get(&device_id)?;
        let entry = dev.itt.iter().find(|e| e.event_id == event_id)?;
        let target_pe = self.collections.get(&entry.collection).copied()?;
        let result = (target_pe, entry.intid);
        self.pending_injections.push(result);
        Some(result)
    }

    /// `DISCARD` — remove an ITT entry.
    pub fn cmd_discard(&mut self, device_id: u32, event_id: u32) {
        if let Some(dev) = self.devices.get_mut(&device_id) {
            dev.itt.retain(|e| e.event_id != event_id);
        }
    }

    /// Drain pending injections for delivery to redistributors.
    pub fn drain_pending(&mut self) -> Vec<(u32, u32)> {
        std::mem::take(&mut self.pending_injections)
    }

    /// Reset the ITS to power-on state.
    pub fn reset(&mut self) {
        self.ctrl = 0;
        self.cmd_base = 0;
        self.cmd_write = 0;
        self.cmd_read = 0;
        self.table_bases = [0; 8];
        self.devices.clear();
        self.collections.clear();
        self.pending_injections.clear();
    }

    /// Advance the read pointer to match the write pointer.
    ///
    /// A full implementation would read command packets from guest
    /// memory; this stub assumes commands are submitted via the
    /// direct `cmd_*` methods.
    fn process_commands(&mut self) {
        self.cmd_read = self.cmd_write;
    }
}

impl Default for GicIts {
    fn default() -> Self {
        Self::new()
    }
}
