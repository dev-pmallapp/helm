//! RISC-V CSR address constants.

/// Commonly-used CSR addresses (12-bit, stored as u16).
#[allow(dead_code)]
pub mod addr {
    // Machine information
    pub const MVENDORID: u16 = 0xF11;
    pub const MARCHID:   u16 = 0xF12;
    pub const MIMPID:    u16 = 0xF13;
    pub const MHARTID:   u16 = 0xF14;
    pub const MCONFIGPTR: u16 = 0xF15;

    // Machine trap setup
    pub const MSTATUS:  u16 = 0x300;
    pub const MISA:     u16 = 0x301;
    pub const MEDELEG:  u16 = 0x302;
    pub const MIDELEG:  u16 = 0x303;
    pub const MIE:      u16 = 0x304;
    pub const MTVEC:    u16 = 0x305;
    pub const MCOUNTEREN: u16 = 0x306;
    pub const MSTATUSH: u16 = 0x310;

    // Machine trap handling
    pub const MSCRATCH: u16 = 0x340;
    pub const MEPC:     u16 = 0x341;
    pub const MCAUSE:   u16 = 0x342;
    pub const MTVAL:    u16 = 0x343;
    pub const MIP:      u16 = 0x344;
    pub const MTINST:   u16 = 0x34A;
    pub const MTVAL2:   u16 = 0x34B;

    // Machine counters
    pub const MCYCLE:      u16 = 0xB00;
    pub const MINSTRET:    u16 = 0xB02;

    // Supervisor trap setup
    pub const SSTATUS:  u16 = 0x100;
    pub const SIE:      u16 = 0x104;
    pub const STVEC:    u16 = 0x105;
    pub const SCOUNTEREN: u16 = 0x106;

    // Supervisor trap handling
    pub const SSCRATCH: u16 = 0x140;
    pub const SEPC:     u16 = 0x141;
    pub const SCAUSE:   u16 = 0x142;
    pub const STVAL:    u16 = 0x143;
    pub const SIP:      u16 = 0x144;

    // Supervisor address translation
    pub const SATP:     u16 = 0x180;

    // User-mode
    pub const CYCLE:    u16 = 0xC00;
    pub const TIME:     u16 = 0xC01;
    pub const INSTRET:  u16 = 0xC02;
}
