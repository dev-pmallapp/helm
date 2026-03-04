use crate::rename::*;

#[test]
fn rename_dest_returns_unique_phys_regs() {
    let mut ru = RenameUnit::new();
    let p0 = ru.rename_dest(0);
    let p1 = ru.rename_dest(1);
    assert_ne!(p0, p1);
}

#[test]
fn lookup_src_reflects_latest_rename() {
    let mut ru = RenameUnit::new();
    let p = ru.rename_dest(5);
    assert_eq!(ru.lookup_src(5), p);

    let p2 = ru.rename_dest(5); // re-rename same arch reg
    assert_ne!(p, p2);
    assert_eq!(ru.lookup_src(5), p2);
}

#[test]
fn freed_regs_are_reused() {
    let mut ru = RenameUnit::new();
    let p0 = ru.rename_dest(0);
    ru.free(p0);
    let p1 = ru.rename_dest(1);
    assert_eq!(p0, p1, "freed phys reg should be reused");
}
