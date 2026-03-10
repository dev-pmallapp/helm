use crate::pci::BarDecl;

#[test]
fn unused_size_is_zero() {
    assert_eq!(BarDecl::Unused.size(), 0);
}

#[test]
fn unused_is_unused() {
    assert!(BarDecl::Unused.is_unused());
}

#[test]
fn mmio32_not_unused() {
    assert!(!BarDecl::Mmio32 { size: 0x1000 }.is_unused());
}

#[test]
fn mmio32_size() {
    assert_eq!(BarDecl::Mmio32 { size: 0x4000 }.size(), 0x4000);
}

#[test]
fn mmio32_not_64bit() {
    assert!(!BarDecl::Mmio32 { size: 0x1000 }.is_64bit());
}

#[test]
fn mmio32_not_io() {
    assert!(!BarDecl::Mmio32 { size: 0x1000 }.is_io());
}

#[test]
fn mmio64_is_64bit() {
    assert!(BarDecl::Mmio64 { size: 0x10_0000 }.is_64bit());
}

#[test]
fn mmio64_not_unused() {
    assert!(!BarDecl::Mmio64 { size: 0x1000 }.is_unused());
}

#[test]
fn mmio64_not_io() {
    assert!(!BarDecl::Mmio64 { size: 0x1000 }.is_io());
}

#[test]
fn mmio64_size() {
    assert_eq!(BarDecl::Mmio64 { size: 0x10_0000 }.size(), 0x10_0000);
}

#[test]
fn io_is_io() {
    assert!(BarDecl::Io { size: 0x100 }.is_io());
}

#[test]
fn io_not_64bit() {
    assert!(!BarDecl::Io { size: 0x100 }.is_64bit());
}

#[test]
fn io_not_unused() {
    assert!(!BarDecl::Io { size: 0x100 }.is_unused());
}

#[test]
fn io_size() {
    assert_eq!(BarDecl::Io { size: 0x100 }.size(), 0x100);
}

#[test]
fn copy_semantics() {
    let original = BarDecl::Mmio32 { size: 0x2000 };
    let copy = original;
    assert_eq!(copy.size(), 0x2000);
}

#[test]
fn eq_semantics() {
    assert_eq!(BarDecl::Unused, BarDecl::Unused);
    assert_ne!(
        BarDecl::Mmio32 { size: 0x1000 },
        BarDecl::Mmio32 { size: 0x2000 }
    );
    assert_ne!(BarDecl::Mmio32 { size: 0x1000 }, BarDecl::Unused);
}
