use crate::callback::MemFilter;

#[test]
fn mem_filter_all_matches_everything() {
    assert!(MemFilter::All.matches(true));
    assert!(MemFilter::All.matches(false));
}

#[test]
fn mem_filter_reads_only() {
    assert!(MemFilter::ReadsOnly.matches(false)); // not a store = read
    assert!(!MemFilter::ReadsOnly.matches(true));
}

#[test]
fn mem_filter_writes_only() {
    assert!(MemFilter::WritesOnly.matches(true));
    assert!(!MemFilter::WritesOnly.matches(false));
}
