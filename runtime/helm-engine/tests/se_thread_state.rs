#![allow(missing_docs)]
use helm_engine::se::threading::{child_thread_pointer, CLONE_SETTLS};

#[test]
fn clone_settls_sets_child_thread_pointer() {
    let parent_tp = 0xAAAA_0000u64;
    let requested_tls = 0xBBBB_0000u64;

    let child_tp = child_thread_pointer(parent_tp, CLONE_SETTLS, requested_tls);

    assert_eq!(child_tp, requested_tls);
}

#[test]
fn clone_without_settls_inherits_parent_thread_pointer() {
    let parent_tp = 0xAAAA_0000u64;
    let requested_tls = 0xDEAD_BEEFu64;

    let child_tp = child_thread_pointer(parent_tp, 0, requested_tls);

    assert_eq!(child_tp, parent_tp);
}
