//! AArch64 convenience wrapper: decode one raw word, then execute it.
//!
//! The engine's primary path is explicit `decode()` then `execute()`.
//! `step()` is retained as a compatibility helper for tests and call sites
//! that want a single-entry-point API without maintaining a second executor.

use helm_core::{HartException, MemInterface};

use super::arch_state::Aarch64ArchState;
use super::decode::decode;
use super::execute::execute;
use crate::DecodeError;

/// Decode and execute one AArch64 instruction.
///
/// Returns `Ok(true)` if the instruction wrote PC, `Ok(false)` if the caller
/// should advance PC by 4, mirroring [`execute`].
pub fn step(
    a: &mut Aarch64ArchState,
    mem: &mut impl MemInterface,
    raw: u32,
) -> Result<bool, HartException> {
    let insn = match decode(raw, a.pc) {
        Ok(insn) => insn,
        Err(DecodeError::Unknown { raw, pc }) => {
            return Err(HartException::IllegalInstruction { pc, raw });
        }
        Err(DecodeError::Unimplemented) => {
            return Err(HartException::Unsupported);
        }
    };

    execute(&insn, a, mem, None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::aarch64::Aarch64ArchState;
    use helm_core::{AccessType, MemFault, MemInterface};

    struct TestMem {
        bytes: std::collections::HashMap<u64, u8>,
    }

    impl TestMem {
        fn new() -> Self {
            Self {
                bytes: std::collections::HashMap::new(),
            }
        }
    }

    impl MemInterface for TestMem {
        fn read(&mut self, addr: u64, size: usize, _ty: AccessType) -> Result<u64, MemFault> {
            let mut value = 0u64;
            for i in 0..size {
                value |= (*self.bytes.get(&(addr + i as u64)).unwrap_or(&0) as u64) << (i * 8);
            }
            Ok(value)
        }

        fn write(
            &mut self,
            addr: u64,
            size: usize,
            value: u64,
            _ty: AccessType,
        ) -> Result<(), MemFault> {
            for i in 0..size {
                self.bytes
                    .insert(addr + i as u64, ((value >> (i * 8)) & 0xff) as u8);
            }
            Ok(())
        }
    }

    #[test]
    fn step_decodes_and_executes_add_immediate() {
        let mut a = Aarch64ArchState::new();
        let mut mem = TestMem::new();
        a.write_x(0, 2);

        let pc_written = step(&mut a, &mut mem, 0x9100_1400).unwrap();

        assert!(!pc_written);
        assert_eq!(a.read_x(0), 7);
    }

    #[test]
    fn step_maps_decode_unknown_to_illegal_instruction() {
        let mut a = Aarch64ArchState::new();
        let mut mem = TestMem::new();
        a.pc = 0x4000;

        let err = step(&mut a, &mut mem, 0).unwrap_err();

        assert!(matches!(
            err,
            HartException::IllegalInstruction { pc: 0x4000, raw: 0 }
        ));
    }
}
