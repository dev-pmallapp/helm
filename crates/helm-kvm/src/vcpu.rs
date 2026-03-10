//! KVM virtual CPU management.
//!
//! [`KvmVcpu`] wraps a single KVM vCPU file descriptor and provides
//! register access, the run loop, and MMIO response helpers.

use crate::error::{KvmError, Result};
use crate::exit::{self, VmExit};
use crate::kvm_sys::{self, kvm_one_reg, kvm_vcpu_init};
use std::os::unix::io::RawFd;
use std::ptr;

/// A KVM virtual CPU.
///
/// Created via [`KvmVm::create_vcpu`](crate::vm::KvmVm::create_vcpu).
pub struct KvmVcpu {
    /// vCPU file descriptor.
    fd: RawFd,
    /// Pointer to the mmap'd `kvm_run` region.
    kvm_run: *mut u8,
    /// Size of the mmap'd region (needed for `munmap`).
    mmap_size: usize,
}

// Safety: the vCPU fd and mmap region are not shared across threads.
// KVM requires that a vCPU fd is used from a single thread, which
// the caller must ensure.
unsafe impl Send for KvmVcpu {}

impl KvmVcpu {
    /// Create a vCPU from a pre-existing fd.
    ///
    /// `vcpu_fd` must be obtained from `KVM_CREATE_VCPU`.
    /// `mmap_size` is from `KVM_GET_VCPU_MMAP_SIZE`.
    pub(crate) fn from_fd(vcpu_fd: RawFd, mmap_size: usize) -> Result<Self> {
        let kvm_run = unsafe {
            libc::mmap(
                ptr::null_mut(),
                mmap_size,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                vcpu_fd,
                0,
            )
        };
        if kvm_run == libc::MAP_FAILED {
            return Err(KvmError::Mmap(std::io::Error::last_os_error()));
        }
        Ok(Self {
            fd: vcpu_fd,
            kvm_run: kvm_run as *mut u8,
            mmap_size,
        })
    }

    /// Initialise the vCPU with the preferred target and optional features.
    ///
    /// Must be called before the first `run()`.  `preferred` is
    /// obtained from [`KvmVm::preferred_target`](crate::vm::KvmVm::preferred_target).
    pub fn init(&self, preferred: &kvm_vcpu_init) -> Result<()> {
        unsafe {
            kvm_sys::kvm_ioctl(
                self.fd,
                kvm_sys::KVM_ARM_VCPU_INIT,
                preferred as *const _ as u64,
            )
        }
        .map_err(|e| KvmError::Ioctl {
            name: "KVM_ARM_VCPU_INIT",
            source: e,
        })?;
        Ok(())
    }

    // ── Register access ─────────────────────────────────────────────────

    /// Read a single 64-bit register.
    pub fn get_reg(&self, reg_id: u64) -> Result<u64> {
        let mut value: u64 = 0;
        let reg = kvm_one_reg {
            id: reg_id,
            addr: &mut value as *mut u64 as u64,
        };
        unsafe { kvm_sys::kvm_ioctl(self.fd, kvm_sys::KVM_GET_ONE_REG, &reg as *const _ as u64) }
            .map_err(|e| KvmError::Ioctl {
            name: "KVM_GET_ONE_REG",
            source: e,
        })?;
        Ok(value)
    }

    /// Write a single 64-bit register.
    pub fn set_reg(&self, reg_id: u64, value: u64) -> Result<()> {
        let val = value;
        let reg = kvm_one_reg {
            id: reg_id,
            addr: &val as *const u64 as u64,
        };
        unsafe { kvm_sys::kvm_ioctl(self.fd, kvm_sys::KVM_SET_ONE_REG, &reg as *const _ as u64) }
            .map_err(|e| KvmError::Ioctl {
            name: "KVM_SET_ONE_REG",
            source: e,
        })?;
        Ok(())
    }

    /// Read general-purpose register `Xn` (n = 0..30).
    pub fn get_xn(&self, n: u32) -> Result<u64> {
        if n > 30 {
            return Err(KvmError::InvalidParameter(format!("Xn: n={n} > 30")));
        }
        self.get_reg(kvm_sys::reg_id_xn(n as u64))
    }

    /// Write general-purpose register `Xn` (n = 0..30).
    pub fn set_xn(&self, n: u32, value: u64) -> Result<()> {
        if n > 30 {
            return Err(KvmError::InvalidParameter(format!("Xn: n={n} > 30")));
        }
        self.set_reg(kvm_sys::reg_id_xn(n as u64), value)
    }

    /// Read the program counter.
    pub fn get_pc(&self) -> Result<u64> {
        self.get_reg(kvm_sys::REG_ID_PC)
    }

    /// Write the program counter.
    pub fn set_pc(&self, value: u64) -> Result<()> {
        self.set_reg(kvm_sys::REG_ID_PC, value)
    }

    /// Read the stack pointer (SP_EL0 visible at current EL).
    pub fn get_sp(&self) -> Result<u64> {
        self.get_reg(kvm_sys::REG_ID_SP)
    }

    /// Write the stack pointer.
    pub fn set_sp(&self, value: u64) -> Result<()> {
        self.set_reg(kvm_sys::REG_ID_SP, value)
    }

    /// Read PSTATE.
    pub fn get_pstate(&self) -> Result<u64> {
        self.get_reg(kvm_sys::REG_ID_PSTATE)
    }

    /// Write PSTATE.
    pub fn set_pstate(&self, value: u64) -> Result<()> {
        self.set_reg(kvm_sys::REG_ID_PSTATE, value)
    }

    /// Snapshot all general-purpose registers (X0–X30, SP, PC, PSTATE).
    pub fn get_core_regs(&self) -> Result<CoreRegs> {
        let mut regs = CoreRegs::default();
        for i in 0..31u32 {
            regs.xn[i as usize] = self.get_xn(i)?;
        }
        regs.sp = self.get_sp()?;
        regs.pc = self.get_pc()?;
        regs.pstate = self.get_pstate()?;
        Ok(regs)
    }

    /// Restore all general-purpose registers from a snapshot.
    pub fn set_core_regs(&self, regs: &CoreRegs) -> Result<()> {
        for i in 0..31u32 {
            self.set_xn(i, regs.xn[i as usize])?;
        }
        self.set_sp(regs.sp)?;
        self.set_pc(regs.pc)?;
        self.set_pstate(regs.pstate)?;
        Ok(())
    }

    // ── System register access ──────────────────────────────────────────

    /// Read a system register by its KVM register ID.
    pub fn get_sys_reg(&self, reg_id: u64) -> Result<u64> {
        self.get_reg(reg_id)
    }

    /// Write a system register by its KVM register ID.
    pub fn set_sys_reg(&self, reg_id: u64, value: u64) -> Result<()> {
        self.set_reg(reg_id, value)
    }

    /// Snapshot commonly-used EL1 system registers.
    pub fn get_sys_regs(&self) -> Result<SysRegs> {
        Ok(SysRegs {
            sctlr_el1: self.get_reg(kvm_sys::SYSREG_SCTLR_EL1)?,
            tcr_el1: self.get_reg(kvm_sys::SYSREG_TCR_EL1)?,
            ttbr0_el1: self.get_reg(kvm_sys::SYSREG_TTBR0_EL1)?,
            ttbr1_el1: self.get_reg(kvm_sys::SYSREG_TTBR1_EL1)?,
            mair_el1: self.get_reg(kvm_sys::SYSREG_MAIR_EL1)?,
            vbar_el1: self.get_reg(kvm_sys::SYSREG_VBAR_EL1)?,
            elr_el1: self.get_reg(kvm_sys::SYSREG_ELR_EL1)?,
            spsr_el1: self.get_reg(kvm_sys::SYSREG_SPSR_EL1)?,
            esr_el1: self.get_reg(kvm_sys::SYSREG_ESR_EL1)?,
            far_el1: self.get_reg(kvm_sys::SYSREG_FAR_EL1)?,
        })
    }

    /// Restore commonly-used EL1 system registers.
    pub fn set_sys_regs(&self, regs: &SysRegs) -> Result<()> {
        self.set_reg(kvm_sys::SYSREG_SCTLR_EL1, regs.sctlr_el1)?;
        self.set_reg(kvm_sys::SYSREG_TCR_EL1, regs.tcr_el1)?;
        self.set_reg(kvm_sys::SYSREG_TTBR0_EL1, regs.ttbr0_el1)?;
        self.set_reg(kvm_sys::SYSREG_TTBR1_EL1, regs.ttbr1_el1)?;
        self.set_reg(kvm_sys::SYSREG_MAIR_EL1, regs.mair_el1)?;
        self.set_reg(kvm_sys::SYSREG_VBAR_EL1, regs.vbar_el1)?;
        self.set_reg(kvm_sys::SYSREG_ELR_EL1, regs.elr_el1)?;
        self.set_reg(kvm_sys::SYSREG_SPSR_EL1, regs.spsr_el1)?;
        self.set_reg(kvm_sys::SYSREG_ESR_EL1, regs.esr_el1)?;
        self.set_reg(kvm_sys::SYSREG_FAR_EL1, regs.far_el1)?;
        Ok(())
    }

    // ── Execution ───────────────────────────────────────────────────────

    /// Execute the vCPU until a VM exit occurs.
    pub fn run(&mut self) -> Result<VmExit> {
        let ret = unsafe { libc::ioctl(self.fd, kvm_sys::KVM_RUN, 0) };
        if ret < 0 {
            let err = std::io::Error::last_os_error();
            if err.raw_os_error() == Some(libc::EINTR) {
                return Ok(VmExit::Intr);
            }
            return Err(KvmError::Ioctl {
                name: "KVM_RUN",
                source: err,
            });
        }
        Ok(unsafe { exit::decode_exit(self.kvm_run) })
    }

    /// Write the MMIO read-response data back into `kvm_run`.
    ///
    /// Must be called after a `VmExit::Mmio { is_write: false }` and
    /// before the next `run()`.
    pub fn set_mmio_response(&mut self, value: u64, len: u32) {
        unsafe { exit::set_mmio_response(self.kvm_run, value, len) };
    }

    /// Return the raw vCPU file descriptor.
    pub fn fd(&self) -> RawFd {
        self.fd
    }
}

impl Drop for KvmVcpu {
    fn drop(&mut self) {
        if !self.kvm_run.is_null() {
            unsafe {
                libc::munmap(self.kvm_run as *mut libc::c_void, self.mmap_size);
            }
        }
        unsafe { libc::close(self.fd) };
    }
}

/// Snapshot of the AArch64 core (general-purpose) registers.
#[derive(Debug, Clone, Default)]
pub struct CoreRegs {
    /// X0–X30.
    pub xn: [u64; 31],
    /// Stack pointer.
    pub sp: u64,
    /// Program counter.
    pub pc: u64,
    /// Processor state (NZCV, EL, etc.).
    pub pstate: u64,
}

/// Snapshot of commonly-used EL1 system registers.
#[derive(Debug, Clone, Default)]
pub struct SysRegs {
    pub sctlr_el1: u64,
    pub tcr_el1: u64,
    pub ttbr0_el1: u64,
    pub ttbr1_el1: u64,
    pub mair_el1: u64,
    pub vbar_el1: u64,
    pub elr_el1: u64,
    pub spsr_el1: u64,
    pub esr_el1: u64,
    pub far_el1: u64,
}
