//! RealView Platform Baseboard system registers.
//!
//! Provides SYS_ID, SYS_SW, SYS_LED, SYS_FLAGS, and other
//! board-level configuration registers.

use crate::device::{Device, DeviceEvent};
use crate::region::MemRegion;
use crate::transaction::Transaction;
use helm_core::types::Addr;
use helm_core::HelmResult;

const SYS_ID: u64 = 0x000;
const SYS_SW: u64 = 0x004;
const SYS_LED: u64 = 0x008;
const SYS_OSC0: u64 = 0x00C;
const SYS_OSC1: u64 = 0x010;
const SYS_OSC2: u64 = 0x014;
const SYS_OSC3: u64 = 0x018;
const SYS_OSC4: u64 = 0x01C;
const SYS_LOCK: u64 = 0x020;
const SYS_100HZ: u64 = 0x024;
const SYS_FLAGS: u64 = 0x030;
const SYS_FLAGSSET: u64 = 0x030;
const SYS_FLAGSCLR: u64 = 0x034;
const SYS_NVFLAGS: u64 = 0x038;
const SYS_NVFLAGSSET: u64 = 0x038;
const SYS_NVFLAGSCLR: u64 = 0x03C;
const SYS_RESETCTL: u64 = 0x040;
const SYS_PCICTL: u64 = 0x044;
const SYS_MCI: u64 = 0x048;
const SYS_FLASH: u64 = 0x04C;
const SYS_CLCD: u64 = 0x050;
const SYS_CLCDSER: u64 = 0x054;
const SYS_BOOTCS: u64 = 0x058;
const SYS_24MHZ: u64 = 0x05C;
const SYS_MISC: u64 = 0x060;
const SYS_PROCID0: u64 = 0x084;
const SYS_PROCID1: u64 = 0x088;
const SYS_CFGDATA: u64 = 0x0A0;
const SYS_CFGCTRL: u64 = 0x0A4;
const SYS_CFGSTAT: u64 = 0x0A8;

const LOCK_MAGIC: u32 = 0xA05F;

pub struct RealViewSysRegs {
    dev_name: String,
    region: MemRegion,
    /// Board ID (SYS_ID). RealView-PB-A8 = 0x178_00000 + rev.
    pub sys_id: u32,
    pub sw: u32,
    pub led: u32,
    pub osc: [u32; 5],
    pub locked: bool,
    pub hz_100: u32,
    pub flags: u32,
    pub nvflags: u32,
    pub resetctl: u32,
    pub mci: u32,
    pub flash: u32,
    pub clcd: u32,
    pub misc: u32,
    pub proc_id: [u32; 2],
    pub cfgdata: u32,
    pub cfgctrl: u32,
    pub cfgstat: u32,
    pub mhz_24_counter: u32,
}

impl RealViewSysRegs {
    /// RealView Platform Baseboard for Cortex-A8 system registers.
    pub fn realview_pb_a8() -> Self {
        Self::new("sysregs", 0x0178_0000)
    }

    /// RealView Emulation Baseboard system registers.
    pub fn realview_eb() -> Self {
        Self::new("sysregs", 0x0140_0400)
    }

    pub fn new(name: impl Into<String>, sys_id: u32) -> Self {
        let n = name.into();
        Self {
            region: MemRegion {
                name: n.clone(), base: 0, size: 0x1000,
                kind: crate::region::RegionKind::Io, priority: 0,
            },
            dev_name: n,
            sys_id,
            sw: 0, led: 0, osc: [0; 5], locked: true,
            hz_100: 0, flags: 0, nvflags: 0, resetctl: 0,
            mci: 0, flash: 0, clcd: 0, misc: 0,
            proc_id: [0x0200_0000, 0], // Cortex-A8
            cfgdata: 0, cfgctrl: 0, cfgstat: 0,
            mhz_24_counter: 0,
        }
    }

    fn handle_read(&self, offset: u64) -> u32 {
        match offset {
            SYS_ID => self.sys_id,
            SYS_SW => self.sw,
            SYS_LED => self.led,
            SYS_OSC0 => self.osc[0],
            SYS_OSC1 => self.osc[1],
            SYS_OSC2 => self.osc[2],
            SYS_OSC3 => self.osc[3],
            SYS_OSC4 => self.osc[4],
            SYS_LOCK => self.locked as u32,
            SYS_100HZ => self.hz_100,
            SYS_FLAGS => self.flags,
            SYS_NVFLAGS => self.nvflags,
            SYS_RESETCTL => self.resetctl,
            SYS_MCI => self.mci,
            SYS_FLASH => self.flash,
            SYS_CLCD => self.clcd,
            SYS_24MHZ => self.mhz_24_counter,
            SYS_MISC => self.misc,
            SYS_PROCID0 => self.proc_id[0],
            SYS_PROCID1 => self.proc_id[1],
            SYS_CFGDATA => self.cfgdata,
            SYS_CFGCTRL => self.cfgctrl,
            SYS_CFGSTAT => self.cfgstat,
            _ => 0,
        }
    }

    fn handle_write(&mut self, offset: u64, value: u32) {
        if self.locked && offset != SYS_LOCK && offset != SYS_FLAGSCLR && offset != SYS_NVFLAGSCLR {
            return;
        }
        match offset {
            SYS_LED => self.led = value,
            SYS_OSC0 => self.osc[0] = value,
            SYS_OSC1 => self.osc[1] = value,
            SYS_OSC2 => self.osc[2] = value,
            SYS_OSC3 => self.osc[3] = value,
            SYS_OSC4 => self.osc[4] = value,
            SYS_LOCK => self.locked = (value & 0xFFFF) != LOCK_MAGIC,
            SYS_FLAGSSET => self.flags |= value,
            SYS_FLAGSCLR => self.flags &= !value,
            SYS_NVFLAGSSET => self.nvflags |= value,
            SYS_NVFLAGSCLR => self.nvflags &= !value,
            SYS_RESETCTL => self.resetctl = value,
            SYS_MCI => self.mci = value,
            SYS_FLASH => self.flash = value,
            SYS_CLCD => self.clcd = value,
            SYS_MISC => self.misc = value,
            SYS_CFGDATA => self.cfgdata = value,
            SYS_CFGCTRL => self.cfgctrl = value,
            SYS_CFGSTAT => self.cfgstat = value,
            _ => {}
        }
    }
}

impl Device for RealViewSysRegs {
    fn transact(&mut self, txn: &mut Transaction) -> HelmResult<()> {
        if txn.is_write { self.handle_write(txn.offset, txn.data_u32()); }
        else { txn.set_data_u32(self.handle_read(txn.offset)); }
        txn.stall_cycles += 1;
        Ok(())
    }

    fn regions(&self) -> &[MemRegion] { std::slice::from_ref(&self.region) }

    fn reset(&mut self) -> HelmResult<()> {
        self.led = 0; self.flags = 0; self.locked = true;
        self.hz_100 = 0; self.mhz_24_counter = 0;
        Ok(())
    }

    fn tick(&mut self, cycles: u64) -> HelmResult<Vec<DeviceEvent>> {
        self.mhz_24_counter = self.mhz_24_counter.wrapping_add(cycles as u32);
        self.hz_100 = self.hz_100.wrapping_add((cycles / 10000) as u32);
        Ok(vec![])
    }

    fn read_fast(&mut self, offset: Addr, _s: usize) -> HelmResult<u64> {
        Ok(self.handle_read(offset) as u64)
    }
    fn write_fast(&mut self, offset: Addr, _s: usize, v: u64) -> HelmResult<()> {
        self.handle_write(offset, v as u32); Ok(())
    }

    fn name(&self) -> &str { &self.dev_name }
}
