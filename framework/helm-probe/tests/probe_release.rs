// Works in BOTH profiles -- in release Probe<T> is ZST
#[test]
fn probe_zst_in_release() {
    // This assertion is only meaningful in release.
    // In dev, the Vec makes it non-zero-sized. So gate it:
    #[cfg(not(debug_assertions))]
    {
        assert_eq!(
            std::mem::size_of::<helm_probe::Probe<u64>>(),
            0,
            "Probe<T> must be ZST in release"
        );
        assert_eq!(
            std::mem::size_of::<helm_probe::CpuProbes>(),
            0,
            "CpuProbes must be ZST in release"
        );
    }
    // In dev just check it compiles
    let _: helm_probe::Probe<u64> = helm_probe::Probe::new();
}

#[test]
fn cpu_probes_default_and_branch_event() {
    use helm_probe::{BranchEvent, BranchKind, CpuProbes};
    let probes = CpuProbes::default();
    assert!(!probes.branch.has_listeners());
    let ev = BranchEvent {
        pc: 0x4000,
        target: 0x5000,
        taken: true,
        kind: BranchKind::Call,
    };
    assert_eq!(ev.pc, 0x4000);
    assert!(ev.taken);
}
