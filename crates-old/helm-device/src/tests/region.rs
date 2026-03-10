use crate::region::*;

#[test]
fn empty_tree() {
    let tree = MemRegionTree::new();
    assert!(tree.lookup(0).is_none());
    assert!(!tree.contains(0));
}

#[test]
fn single_region_lookup() {
    let mut tree = MemRegionTree::new();
    tree.add(MemRegion {
        name: "ram".into(),
        base: 0x1000,
        size: 0x100,
        kind: RegionKind::Io,
        priority: 0,
    });
    assert!(tree.contains(0x1000));
    assert!(tree.contains(0x10FF));
    assert!(!tree.contains(0x1100));
    assert!(!tree.contains(0x0FFF));
}

#[test]
fn multiple_non_overlapping_regions() {
    let mut tree = MemRegionTree::new();
    tree.add(MemRegion {
        name: "a".into(),
        base: 0x1000,
        size: 0x100,
        kind: RegionKind::Io,
        priority: 0,
    });
    tree.add(MemRegion {
        name: "b".into(),
        base: 0x2000,
        size: 0x200,
        kind: RegionKind::Io,
        priority: 0,
    });
    let a = tree.lookup(0x1050).unwrap();
    assert_eq!(a.region_idx, 0);
    let b = tree.lookup(0x2100).unwrap();
    assert_eq!(b.region_idx, 1);
    assert!(tree.lookup(0x1500).is_none());
}

#[test]
fn priority_overlap_higher_wins() {
    let mut tree = MemRegionTree::new();
    // Low-priority region covering 0x0000..0x2000
    tree.add(MemRegion {
        name: "low".into(),
        base: 0x0,
        size: 0x2000,
        kind: RegionKind::Io,
        priority: 0,
    });
    // High-priority region covering 0x1000..0x1100
    tree.add(MemRegion {
        name: "high".into(),
        base: 0x1000,
        size: 0x100,
        kind: RegionKind::Io,
        priority: 10,
    });

    // Address in high-prio region should resolve to region 1
    let entry = tree.lookup(0x1050).unwrap();
    assert_eq!(entry.region_idx, 1);

    // Address outside high-prio but inside low-prio should resolve to region 0
    let entry = tree.lookup(0x0500).unwrap();
    assert_eq!(entry.region_idx, 0);
}

#[test]
fn remove_region() {
    let mut tree = MemRegionTree::new();
    tree.add(MemRegion {
        name: "r".into(),
        base: 0x1000,
        size: 0x100,
        kind: RegionKind::Io,
        priority: 0,
    });
    assert!(tree.contains(0x1000));
    tree.remove(0);
    assert!(!tree.contains(0x1000));
}

#[test]
fn flat_view_sorted() {
    let mut tree = MemRegionTree::new();
    tree.add(MemRegion {
        name: "b".into(),
        base: 0x2000,
        size: 0x100,
        kind: RegionKind::Io,
        priority: 0,
    });
    tree.add(MemRegion {
        name: "a".into(),
        base: 0x1000,
        size: 0x100,
        kind: RegionKind::Io,
        priority: 0,
    });
    let fv = tree.flat_view();
    assert!(fv.len() >= 2);
    // Flat view should be sorted by start address
    for w in fv.windows(2) {
        assert!(w[0].start <= w[1].start);
    }
}

#[test]
fn default_creates_empty() {
    let tree = MemRegionTree::default();
    assert!(tree.regions().is_empty());
    assert!(tree.flat_view().is_empty());
}
