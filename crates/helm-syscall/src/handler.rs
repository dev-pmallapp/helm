//! Syscall dispatch and emulation logic.

use super::table::{self, Syscall};
use helm_core::types::{Addr, IsaKind};
use helm_core::HelmResult;
use helm_memory::address_space::AddressSpace;

/// Emulates Linux syscalls against the guest address space.
pub struct SyscallHandler {
    isa: IsaKind,
    brk_addr: Addr,
}

impl SyscallHandler {
    pub fn new(isa: IsaKind) -> Self {
        Self {
            isa,
            brk_addr: 0x1000_0000, // default heap start
        }
    }

    /// Dispatch a syscall. `args` are the register-passed arguments.
    /// Returns the syscall return value.
    pub fn handle(
        &mut self,
        number: u64,
        args: &[u64; 6],
        address_space: &mut AddressSpace,
    ) -> HelmResult<u64> {
        let sc = table::lookup(self.isa, number);
        match sc {
            Syscall::Write => self.sys_write(args, address_space),
            Syscall::Exit | Syscall::ExitGroup => Ok(args[0]),
            Syscall::Brk => Ok(self.sys_brk(args[0])),
            _ => {
                log::warn!("Unimplemented syscall {:?} (number {})", sc, number);
                Ok(u64::MAX) // -ENOSYS
            }
        }
    }

    fn sys_write(&self, args: &[u64; 6], address_space: &AddressSpace) -> HelmResult<u64> {
        let _fd = args[0];
        let buf_addr = args[1];
        let count = args[2] as usize;
        let mut buf = vec![0u8; count];
        address_space.read(buf_addr, &mut buf)?;
        // In a real implementation this would write to the host fd.
        Ok(count as u64)
    }

    fn sys_brk(&mut self, addr: u64) -> u64 {
        if addr == 0 {
            return self.brk_addr;
        }
        if addr > self.brk_addr {
            self.brk_addr = addr;
        }
        self.brk_addr
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn brk_returns_current_when_zero() {
        let mut handler = SyscallHandler::new(IsaKind::X86_64);
        let mut addr_space = AddressSpace::new();
        let args = [0u64; 6];
        let result = handler.handle(12, &args, &mut addr_space).unwrap(); // brk(0)
        assert!(result > 0, "brk(0) should return current break address");
    }

    #[test]
    fn brk_advances() {
        let mut handler = SyscallHandler::new(IsaKind::X86_64);
        let mut addr_space = AddressSpace::new();
        let current = handler.handle(12, &[0; 6], &mut addr_space).unwrap();
        let new_brk = current + 0x1000;
        let mut args = [0u64; 6];
        args[0] = new_brk;
        let result = handler.handle(12, &args, &mut addr_space).unwrap();
        assert_eq!(result, new_brk);
    }

    #[test]
    fn exit_returns_status_code() {
        let mut handler = SyscallHandler::new(IsaKind::X86_64);
        let mut addr_space = AddressSpace::new();
        let mut args = [0u64; 6];
        args[0] = 42;
        let result = handler.handle(60, &args, &mut addr_space).unwrap(); // exit(42)
        assert_eq!(result, 42);
    }
}
