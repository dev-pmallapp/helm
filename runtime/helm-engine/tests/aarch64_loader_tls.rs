use helm_engine::{loader::load_elf, FlatMem};

const FISH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/aarch64/binaries/fish");

#[test]
fn fish_loader_exposes_tls_metadata() {
    let mut mem = FlatMem::new(0, 1 << 20);
    let loaded = load_elf(
        FISH,
        &["fish", "--no-config", "-c", "echo hello"],
        &["HOME=/tmp", "LANG=C"],
        &mut mem,
    )
    .expect("load fish elf");

    let tls = loaded.tls_info.expect("expected PT_TLS metadata for fish");
    assert_eq!(tls.template_vaddr, 0x0000_0000_00f4_d6f0);
    assert_eq!(tls.file_size, 0x20);
    assert_eq!(tls.mem_size, 0x98);
    assert_eq!(tls.align, 0x8);
}
