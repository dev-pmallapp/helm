use crate::fdt::*;
use crate::platform::Platform;

// ── FdtBuilder low-level tests ──────────────────────────────────────────────

#[test]
fn empty_dtb_has_valid_header() {
    let builder = FdtBuilder::new();
    let blob = builder.build();
    assert!(blob.len() >= 40);
    assert_eq!(u32::from_be_bytes([blob[0], blob[1], blob[2], blob[3]]), 0xD00DFEED);
    assert_eq!(u32::from_be_bytes([blob[4], blob[5], blob[6], blob[7]]) as usize, blob.len());
}

#[test]
fn dtb_with_properties() {
    let root = FdtNode::new("")
        .with_prop("compatible", FdtValue::String("test,device".to_string()))
        .with_prop("#address-cells", FdtValue::U32(2));
    let mut b = FdtBuilder::new();
    b.set_root(root);
    let blob = b.build();
    assert!(blob.len() > 40);
}

#[test]
fn dtb_with_children() {
    let child = FdtNode::new("memory@40000000")
        .with_prop("device_type", FdtValue::String("memory".to_string()))
        .with_prop("reg", FdtValue::Reg(vec![(0x4000_0000, 0x1000_0000)]));
    let root = FdtNode::new("").with_prop("#address-cells", FdtValue::U32(2)).with_child(child);
    let mut b = FdtBuilder::new();
    b.set_root(root);
    let blob = b.build();
    let str_off = u32::from_be_bytes([blob[12], blob[13], blob[14], blob[15]]) as usize;
    let st = String::from_utf8_lossy(&blob[str_off..]);
    assert!(st.contains("device_type"));
    assert!(st.contains("reg"));
}

// ── DTB round-trip ──────────────────────────────────────────────────────────

#[test]
fn parse_dtb_roundtrip() {
    let root = FdtNode::new("")
        .with_child(FdtNode::new("memory@40000000"))
        .with_child(FdtNode::new("uart@9000000"));
    let mut b = FdtBuilder::new();
    b.set_root(root);
    let blob = b.build();
    let parsed = parse_dtb(&blob).expect("parse ok");
    assert_eq!(parsed.children.len(), 2);
    assert_eq!(parsed.children[0].name, "memory@40000000");
}

#[test]
fn parse_dtb_rejects_bad_magic() {
    let mut blob = vec![0u8; 60];
    blob[0..4].copy_from_slice(&0xDEADBEEFu32.to_be_bytes());
    assert!(parse_dtb(&blob).is_none());
}

// ── DeviceSpec parsing ──────────────────────────────────────────────────────

#[test]
fn device_spec_parse_type_only() {
    let s = DeviceSpec::parse("virtio-net-device");
    assert_eq!(s.type_name, "virtio-net-device");
    assert!(s.properties.is_empty());
}

#[test]
fn device_spec_parse_with_props() {
    let s = DeviceSpec::parse("pl011,addr=0x9040000,id=uart2");
    assert_eq!(s.get("addr"), Some("0x9040000"));
    assert_eq!(s.get_u64("addr"), Some(0x9040000));
}

// ── RuntimeDtb ──────────────────────────────────────────────────────────────

#[test]
fn runtime_dtb_generate_and_add() {
    let p = Platform::new("test-virt");
    let cfg = DtbConfig::default();
    let mut rt = RuntimeDtb::new(&p, &cfg, None);
    let before = rt.root().children.len();
    rt.add_device(&DeviceSpec::parse("virtio-net-device,addr=0xa000000"));
    assert_eq!(rt.root().children.len(), before + 1);
    let blob = rt.to_blob();
    assert_eq!(u32::from_be_bytes([blob[0], blob[1], blob[2], blob[3]]), 0xD00DFEED);
}

#[test]
fn runtime_dtb_remove() {
    let p = Platform::new("test");
    let cfg = DtbConfig {
        extra_devices: vec![DeviceSpec::parse("virtio-net,addr=0xa000000")],
        ..Default::default()
    };
    let mut rt = RuntimeDtb::new(&p, &cfg, None);
    assert!(rt.remove_device("virtio_mmio@a000000"));
}

#[test]
fn runtime_dtb_patch_preserves_base() {
    let base = FdtNode::new("")
        .with_prop("#address-cells", FdtValue::U32(2))
        .with_child(FdtNode::new("original@1000"));
    let mut b = FdtBuilder::new();
    b.set_root(base);
    let base_blob = b.build();

    let p = Platform::new("test");
    let cfg = DtbConfig {
        extra_devices: vec![DeviceSpec::parse("virtio-rng,addr=0xb000000")],
        ..Default::default()
    };
    let rt = RuntimeDtb::new(&p, &cfg, Some(&base_blob));
    let parsed = parse_dtb(&rt.to_blob()).unwrap();
    assert!(parsed.children.iter().any(|c| c.name == "original@1000"));
    assert!(parsed.children.iter().any(|c| c.name.contains("virtio")));
}

#[test]
fn runtime_dtb_multi_cpu() {
    let p = Platform::new("test");
    let cfg = DtbConfig { num_cpus: 4, ..Default::default() };
    let rt = RuntimeDtb::new(&p, &cfg, None);
    let blob = rt.to_blob();
    let so = u32::from_be_bytes([blob[8], blob[9], blob[10], blob[11]]) as usize;
    let ss = u32::from_be_bytes([blob[12], blob[13], blob[14], blob[15]]) as usize;
    let t = String::from_utf8_lossy(&blob[so..ss]);
    assert!(t.contains("cpu@0"));
    assert!(t.contains("cpu@3"));
}

// ── Platform convenience ────────────────────────────────────────────────────

#[test]
fn platform_create_dtb() {
    let p = Platform::new("test");
    let rt = p.create_dtb(&DtbConfig::default());
    let blob = rt.to_blob();
    assert_eq!(u32::from_be_bytes([blob[0], blob[1], blob[2], blob[3]]), 0xD00DFEED);
}

// ── DtbPolicy::infer (derived from platform + CLI context) ──────────────────

fn ctx(uses_dtb: bool, kernel: bool, bios: bool, drive: bool, dtb: bool, extras: bool) -> InferCtx {
    let mut p = Platform::new("t");
    p.uses_dtb = uses_dtb;
    InferCtx::from_platform(&p, kernel, bios, drive, dtb, extras)
}

#[test]
fn infer_virt_kernel_no_dtb_generates() {
    assert_eq!(DtbPolicy::infer(&ctx(true, true, false, false, false, false)), DtbPolicy::Generate);
}

#[test]
fn infer_virt_kernel_with_dtb_passthrough() {
    assert_eq!(DtbPolicy::infer(&ctx(true, true, false, false, true, false)), DtbPolicy::Passthrough);
}

#[test]
fn infer_virt_kernel_dtb_plus_extras_patches() {
    assert_eq!(DtbPolicy::infer(&ctx(true, true, false, false, true, true)), DtbPolicy::Patch);
}

#[test]
fn infer_bios_means_no_dtb() {
    assert_eq!(DtbPolicy::infer(&ctx(true, false, true, false, true, true)), DtbPolicy::None);
}

#[test]
fn infer_drive_only_no_kernel_no_dtb() {
    assert_eq!(DtbPolicy::infer(&ctx(true, false, false, true, false, false)), DtbPolicy::None);
}

#[test]
fn infer_drive_only_with_dtb_passthrough() {
    assert_eq!(DtbPolicy::infer(&ctx(true, false, false, true, true, false)), DtbPolicy::Passthrough);
}

#[test]
fn infer_non_dtb_platform_always_none() {
    assert_eq!(DtbPolicy::infer(&ctx(false, true, false, false, false, false)), DtbPolicy::None);
}

// ── resolve_dtb ─────────────────────────────────────────────────────────────

#[test]
fn resolve_none_for_bios_boot() {
    let p = Platform::new("test");
    let cfg = DtbConfig::default();
    let c = ctx(true, false, true, false, false, false);
    assert!(matches!(resolve_dtb(&p, &cfg, None, &c), ResolvedDtb::None));
}

#[test]
fn resolve_generate_for_kernel_boot() {
    let p = Platform::new("test");
    let cfg = DtbConfig::default();
    let c = ctx(true, true, false, false, false, false);
    match resolve_dtb(&p, &cfg, None, &c) {
        ResolvedDtb::Blob(b) => assert_eq!(u32::from_be_bytes([b[0],b[1],b[2],b[3]]), 0xD00DFEED),
        ResolvedDtb::None => panic!("expected blob"),
    }
}

#[test]
fn resolve_passthrough_returns_verbatim() {
    let p = Platform::new("test");
    let cfg = DtbConfig::default();
    let user = generate_virt_dtb(&p, &cfg);
    let c = ctx(true, true, false, false, true, false);
    match resolve_dtb(&p, &cfg, Some(&user), &c) {
        ResolvedDtb::Blob(b) => assert_eq!(b, user),
        ResolvedDtb::None => panic!("expected blob"),
    }
}

#[test]
fn resolve_patch_adds_extras() {
    let base_root = FdtNode::new("").with_child(FdtNode::new("orig@1000"));
    let mut bld = FdtBuilder::new();
    bld.set_root(base_root);
    let user = bld.build();

    let p = Platform::new("test");
    let cfg = DtbConfig {
        extra_devices: vec![DeviceSpec::parse("virtio-rng,addr=0xb000000")],
        ..Default::default()
    };
    let c = ctx(true, true, false, false, true, true);
    match resolve_dtb(&p, &cfg, Some(&user), &c) {
        ResolvedDtb::Blob(b) => {
            let parsed = parse_dtb(&b).unwrap();
            assert!(parsed.children.iter().any(|n| n.name == "orig@1000"));
            assert!(parsed.children.iter().any(|n| n.name.contains("virtio")));
        }
        ResolvedDtb::None => panic!("expected blob"),
    }
}

// ── parse_ram_size ──────────────────────────────────────────────────────────

#[test]
fn parse_ram_size_variants() {
    assert_eq!(parse_ram_size("256M"), Some(256 * 1024 * 1024));
    assert_eq!(parse_ram_size("1G"), Some(1024 * 1024 * 1024));
    assert_eq!(parse_ram_size("512K"), Some(512 * 1024));
    assert_eq!(parse_ram_size(""), None);
}

// ── backward compat ─────────────────────────────────────────────────────────

#[test]
fn generate_virt_dtb_still_works() {
    let p = Platform::new("test");
    let blob = generate_virt_dtb(&p, &DtbConfig::default());
    assert_eq!(u32::from_be_bytes([blob[0], blob[1], blob[2], blob[3]]), 0xD00DFEED);
}
