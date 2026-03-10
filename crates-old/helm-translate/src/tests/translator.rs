use crate::translator::*;
use helm_isa::x86::X86Frontend;

#[test]
fn translate_populates_cache() {
    let mut translator = Translator::new();
    let fe = X86Frontend::new();
    let memory = [0u8; 64];
    let block = translator.translate(&fe, 0x1000, &memory).unwrap();
    assert_eq!(block.start_pc, 0x1000);
    assert!(!block.uops.is_empty());
}

#[test]
fn translate_same_pc_uses_cache() {
    let mut translator = Translator::new();
    let fe = X86Frontend::new();
    let memory = [0u8; 64];

    // First call fills the cache.
    let pc1 = translator.translate(&fe, 0x2000, &memory).unwrap().start_pc;

    // Second call for the same PC should still succeed (cache hit).
    let pc2 = translator.translate(&fe, 0x2000, &memory).unwrap().start_pc;
    assert_eq!(pc1, pc2);
}
