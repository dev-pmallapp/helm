//! ARM ISA frontend — supports AArch64 (ARMv8/v9) and AArch32 (ARMv7-A).

pub mod aarch32;
pub mod aarch64;
pub mod regs;

#[cfg(test)]
mod tests;

use crate::frontend::IsaFrontend;
use helm_core::ir::MicroOp;
use helm_core::types::Addr;
use helm_core::HelmResult;

/// ARM frontend that dispatches to the AArch64 decoder.
/// AArch32 support is added in stage 1.
#[derive(Default)]
pub struct ArmFrontend {
    a64: aarch64::Aarch64Decoder,
}

impl ArmFrontend {
    pub fn new() -> Self {
        Self::default()
    }
}

impl IsaFrontend for ArmFrontend {
    fn name(&self) -> &str {
        "aarch64"
    }

    fn decode(&self, pc: Addr, bytes: &[u8]) -> HelmResult<(Vec<MicroOp>, usize)> {
        if bytes.len() < 4 {
            return Err(helm_core::HelmError::Decode {
                addr: pc,
                reason: "need at least 4 bytes".into(),
            });
        }
        let insn = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
        let uops = self.a64.decode_insn(pc, insn)?;
        Ok((uops, 4))
    }

    fn min_insn_align(&self) -> usize {
        4
    }
}
