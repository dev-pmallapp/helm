//! GICv3 ICC (Interrupt Controller CPU) system register emulation.
//!
//! In GICv3 the CPU interface is accessed through system registers
//! (`MRS` / `MSR`) rather than MMIO.  Each PE holds its own
//! [`IccState`], and the engine dispatches sysreg accesses via the
//! [`IccReg`] enum.

use super::common::SPURIOUS_IRQ;

/// Well-known ICC system register identifiers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum IccReg {
    /// Interrupt Acknowledge Register (Group 1).
    Iar1 = 0,
    /// End of Interrupt Register (Group 1).
    Eoir1 = 1,
    /// Priority Mask Register.
    Pmr = 2,
    /// Control Register.
    Ctlr = 3,
    /// System Register Enable Register (read-as-one).
    Sre = 4,
    /// Interrupt Group 1 Enable Register.
    Igrpen1 = 5,
    /// Binary Point Register (Group 1).
    Bpr1 = 6,
    /// SGI Generation Register (Group 1).
    Sgi1r = 7,
    /// Interrupt Acknowledge Register (Group 0).
    Iar0 = 8,
    /// End of Interrupt Register (Group 0).
    Eoir0 = 9,
    /// Interrupt Group 0 Enable Register.
    Igrpen0 = 10,
}

impl IccReg {
    /// Decode from AArch64 system-register encoding.
    pub fn from_sysreg(op0: u8, op1: u8, crn: u8, crm: u8, op2: u8) -> Option<Self> {
        match (op0, op1, crn, crm, op2) {
            (3, 0, 12, 12, 0) => Some(Self::Iar1),
            (3, 0, 12, 12, 1) => Some(Self::Eoir1),
            (3, 0, 4, 6, 0) => Some(Self::Pmr),
            (3, 0, 12, 12, 4) => Some(Self::Ctlr),
            (3, 0, 12, 12, 5) => Some(Self::Sre),
            (3, 0, 12, 12, 7) => Some(Self::Igrpen1),
            (3, 0, 12, 12, 3) => Some(Self::Bpr1),
            (3, 0, 12, 11, 5) => Some(Self::Sgi1r),
            (3, 0, 12, 8, 0) => Some(Self::Iar0),
            (3, 0, 12, 8, 1) => Some(Self::Eoir0),
            (3, 0, 12, 12, 6) => Some(Self::Igrpen0),
            _ => None,
        }
    }

    /// Convert to the `u32` key used by `InterruptController` sysreg helpers.
    pub fn as_u32(self) -> u32 {
        self as u32
    }

    /// Construct from a `u32` key.
    pub fn from_u32(val: u32) -> Option<Self> {
        match val {
            0 => Some(Self::Iar1),
            1 => Some(Self::Eoir1),
            2 => Some(Self::Pmr),
            3 => Some(Self::Ctlr),
            4 => Some(Self::Sre),
            5 => Some(Self::Igrpen1),
            6 => Some(Self::Bpr1),
            7 => Some(Self::Sgi1r),
            8 => Some(Self::Iar0),
            9 => Some(Self::Eoir0),
            10 => Some(Self::Igrpen0),
            _ => None,
        }
    }
}

/// Per-PE ICC register state.
pub struct IccState {
    /// Priority mask (`ICC_PMR_EL1`).
    pub pmr: u8,
    /// Control register (`ICC_CTLR_EL1`).
    pub ctlr: u32,
    /// Group 1 interrupt enable (`ICC_IGRPEN1_EL1`).
    pub igrpen1: u32,
    /// Group 0 interrupt enable (`ICC_IGRPEN0_EL1`).
    pub igrpen0: u32,
    /// Binary point register (`ICC_BPR1_EL1`).
    pub bpr1: u32,
    /// Stack of active interrupt priorities (for preemption tracking).
    pub active_priorities: Vec<u8>,
    /// Current running priority (idle = 0xFF).
    pub running_priority: u8,
}

impl IccState {
    /// Create a fresh ICC state (power-on defaults).
    pub fn new() -> Self {
        Self {
            pmr: 0,
            ctlr: 0,
            igrpen1: 0,
            igrpen0: 0,
            bpr1: 0,
            active_priorities: Vec::new(),
            running_priority: 0xFF,
        }
    }

    /// Whether `EOImode` is set (priority drop separate from deactivate).
    pub fn eoi_mode(&self) -> bool {
        self.ctlr & (1 << 1) != 0
    }

    /// Whether Group 1 interrupts are enabled.
    pub fn group1_enabled(&self) -> bool {
        self.igrpen1 & 1 != 0
    }

    /// Record an interrupt acknowledge: push its priority onto the
    /// active stack and update the running priority.
    pub fn priority_drop(&mut self, priority: u8) {
        self.active_priorities.push(priority);
        self.running_priority = priority;
    }

    /// Record an end-of-interrupt: pop the most recent active priority
    /// and restore the previous running priority.
    pub fn deactivate(&mut self) {
        self.active_priorities.pop();
        self.running_priority = self.active_priorities.last().copied().unwrap_or(0xFF);
    }

    /// Reset to power-on state.
    pub fn reset(&mut self) {
        self.pmr = 0;
        self.ctlr = 0;
        self.igrpen1 = 0;
        self.igrpen0 = 0;
        self.bpr1 = 0;
        self.active_priorities.clear();
        self.running_priority = 0xFF;
    }

    /// Read a simple ICC register (not IAR/EOIR/SGI — those require
    /// external state and are handled by `GicV3` directly).
    pub fn read_simple(&self, reg: IccReg) -> u64 {
        match reg {
            IccReg::Pmr => self.pmr as u64,
            IccReg::Ctlr => self.ctlr as u64,
            IccReg::Sre => 0x7,
            IccReg::Igrpen1 => self.igrpen1 as u64,
            IccReg::Igrpen0 => self.igrpen0 as u64,
            IccReg::Bpr1 => self.bpr1 as u64,
            IccReg::Iar1 | IccReg::Iar0 => SPURIOUS_IRQ as u64,
            _ => 0,
        }
    }

    /// Write a simple ICC register.
    pub fn write_simple(&mut self, reg: IccReg, val: u64) {
        match reg {
            IccReg::Pmr => self.pmr = (val & 0xFF) as u8,
            IccReg::Ctlr => self.ctlr = val as u32 & 0x3,
            IccReg::Igrpen1 => self.igrpen1 = val as u32 & 1,
            IccReg::Igrpen0 => self.igrpen0 = val as u32 & 1,
            IccReg::Bpr1 => self.bpr1 = val as u32 & 0x7,
            _ => {}
        }
    }
}

impl Default for IccState {
    fn default() -> Self {
        Self::new()
    }
}
