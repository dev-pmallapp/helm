use crate::fd_table::FdTable;

#[test]
fn new_has_stdio_fds() {
    let ft = FdTable::new();
    assert!(ft.get_host_fd(0).is_some());
    assert_eq!(ft.get_host_fd(1), Some(1));
    assert_eq!(ft.get_host_fd(2), Some(2));
}

#[test]
fn alloc_returns_ascending_guest_fds() {
    let mut ft = FdTable::new();
    let fd1 = ft.alloc(100);
    let fd2 = ft.alloc(200);
    assert_eq!(fd1, 3);
    assert_eq!(fd2, 4);
    assert_ne!(fd1, fd2);
}

#[test]
fn get_host_fd_returns_inserted_fd() {
    let mut ft = FdTable::new();
    let guest = ft.alloc(42);
    assert_eq!(ft.get_host_fd(guest), Some(42));
}

#[test]
fn close_removes_fd() {
    let mut ft = FdTable::new();
    let guest = ft.alloc(42);
    assert!(ft.close(guest));
    assert_eq!(ft.get_host_fd(guest), None);
}

#[test]
fn close_nonexistent_returns_false() {
    let mut ft = FdTable::new();
    assert!(!ft.close(99));
}

#[test]
fn get_host_fd_missing_returns_none() {
    let ft = FdTable::new();
    assert_eq!(ft.get_host_fd(99), None);
}

#[test]
fn dup_copies_mapping() {
    let mut ft = FdTable::new();
    let orig = ft.alloc(77);
    let duped = ft.dup(orig).unwrap();
    assert_ne!(orig, duped);
    assert_eq!(ft.get_host_fd(duped), Some(77));
}

#[test]
fn dup_nonexistent_returns_none() {
    let mut ft = FdTable::new();
    assert!(ft.dup(99).is_none());
}

#[test]
fn dup_to_maps_to_specific_guest_fd() {
    let mut ft = FdTable::new();
    let orig = ft.alloc(55);
    let result = ft.dup_to(orig, 10).unwrap();
    assert_eq!(result, 10);
    assert_eq!(ft.get_host_fd(10), Some(55));
}

#[test]
fn dup_to_nonexistent_returns_none() {
    let mut ft = FdTable::new();
    assert!(ft.dup_to(99, 10).is_none());
}

#[test]
fn default_is_same_as_new() {
    let ft = FdTable::default();
    assert!(ft.get_host_fd(0).is_some());
    assert_eq!(ft.get_host_fd(1), Some(1));
    assert_eq!(ft.get_host_fd(2), Some(2));
    assert_eq!(ft.get_host_fd(3), None);
}
