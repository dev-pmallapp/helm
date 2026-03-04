//! The main translator that ties ISA frontend decoding with block caching.

use super::block::TranslatedBlock;
use super::cache::TranslationCache;
use helm_core::types::Addr;
use helm_core::HelmResult;
use helm_isa::IsaFrontend;

pub struct Translator {
    cache: TranslationCache,
}

impl Default for Translator {
    fn default() -> Self {
        Self::new()
    }
}

impl Translator {
    pub fn new() -> Self {
        Self {
            cache: TranslationCache::new(),
        }
    }

    /// Translate a block starting at `pc`. Uses the cache when possible.
    pub fn translate(
        &mut self,
        frontend: &dyn IsaFrontend,
        pc: Addr,
        memory: &[u8],
    ) -> HelmResult<&TranslatedBlock> {
        if self.cache.lookup(pc).is_some() {
            return Ok(self.cache.lookup(pc).unwrap());
        }

        let mut uops = Vec::new();
        let mut offset = 0usize;
        let max_block_insns = 64;

        // Translate until we hit a branch, end of data, or the block limit.
        while offset < memory.len() && uops.len() < max_block_insns {
            let (decoded, consumed) = frontend.decode(pc + offset as u64, &memory[offset..])?;
            let has_branch = decoded.iter().any(|u| u.flags.is_branch);
            uops.extend(decoded);
            offset += consumed;
            if has_branch {
                break;
            }
        }

        let block = TranslatedBlock {
            start_pc: pc,
            guest_size: offset,
            uops,
        };
        self.cache.insert(block);
        Ok(self.cache.lookup(pc).unwrap())
    }
}
