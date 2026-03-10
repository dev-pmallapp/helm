//! Memory bridge — re-exports ExecMem and provides TraitMemBridge.
//!
//! `ExecMem` trait is defined in `helm-core`. This module provides
//! `TraitMemBridge` which wraps `&mut dyn MemoryAccess` as an `ExecMem`.

pub use helm_core::exec_mem::ExecMem;

use helm_core::types::Addr;
use helm_core::HelmResult;

/// Wraps `&mut dyn MemoryAccess` to provide `ExecMem` for the generic session.
pub struct TraitMemBridge<'a>(pub &'a mut dyn helm_core::mem::MemoryAccess);

impl ExecMem for TraitMemBridge<'_> {
    #[inline]
    fn read_bytes(&mut self, addr: Addr, buf: &mut [u8]) -> HelmResult<()> {
        self.0.fetch(addr, buf).map_err(|f| helm_core::HelmError::Memory {
            addr: f.addr,
            reason: format!("{:?}", f.kind),
        })
    }

    #[inline]
    fn write_bytes(&mut self, addr: Addr, data: &[u8]) -> HelmResult<()> {
        if data.len() <= 8 {
            let mut val_bytes = [0u8; 8];
            val_bytes[..data.len()].copy_from_slice(data);
            let val = u64::from_le_bytes(val_bytes);
            self.0.write(addr, data.len(), val).map_err(|f| {
                helm_core::HelmError::Memory {
                    addr: f.addr,
                    reason: format!("{:?}", f.kind),
                }
            })
        } else {
            for (i, chunk) in data.chunks(8).enumerate() {
                let mut val_bytes = [0u8; 8];
                val_bytes[..chunk.len()].copy_from_slice(chunk);
                let val = u64::from_le_bytes(val_bytes);
                self.0
                    .write(addr + (i * 8) as u64, chunk.len(), val)
                    .map_err(|f| helm_core::HelmError::Memory {
                        addr: f.addr,
                        reason: format!("{:?}", f.kind),
                    })?;
            }
            Ok(())
        }
    }
}
