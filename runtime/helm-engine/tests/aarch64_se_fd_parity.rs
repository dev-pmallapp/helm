#![allow(missing_docs)]
use helm_engine::{
    se::{LinuxAarch64SyscallHandler, SyscallArgs},
    FlatMem,
};

fn open_dev_null(handler: &mut LinuxAarch64SyscallHandler, mem: &mut FlatMem) -> i64 {
    let path_addr = 0x1000;
    mem.load_bytes(path_addr, b"/dev/null\0");
    handler
        .handle(
            56, // openat
            SyscallArgs {
                a0: libc::AT_FDCWD as u64,
                a1: path_addr,
                a2: libc::O_RDONLY as u64,
                a3: 0,
                a4: 0,
                a5: 0,
            },
            mem,
        )
        .expect("openat should succeed")
}

#[test]
fn fcntl_dupfd_cloexec_returns_guest_fd_and_keeps_mapping() {
    let mut handler = LinuxAarch64SyscallHandler::new(0x2000_0000);
    let mut mem = FlatMem::new(0, 1 << 20);

    let guest_fd = open_dev_null(&mut handler, &mut mem);
    assert!(guest_fd >= 3);

    let dup_fd = handler
        .handle(
            25, // fcntl
            SyscallArgs {
                a0: guest_fd as u64,
                a1: libc::F_DUPFD_CLOEXEC as u64,
                a2: 10,
                a3: 0,
                a4: 0,
                a5: 0,
            },
            &mut mem,
        )
        .expect("fcntl(F_DUPFD_CLOEXEC) should succeed");

    assert!(dup_fd >= 10, "guest dup fd should honor requested minimum");

    let stat_addr = 0x2000;
    let r = handler
        .handle(
            80, // fstat
            SyscallArgs {
                a0: dup_fd as u64,
                a1: stat_addr,
                a2: 0,
                a3: 0,
                a4: 0,
                a5: 0,
            },
            &mut mem,
        )
        .expect("fstat on duplicated guest fd should succeed");

    assert_eq!(r, 0);
}

#[test]
fn dup3_uses_requested_guest_fd() {
    let mut handler = LinuxAarch64SyscallHandler::new(0x2000_0000);
    let mut mem = FlatMem::new(0, 1 << 20);

    let guest_fd = open_dev_null(&mut handler, &mut mem);
    let new_guest_fd = 17;

    let ret = handler
        .handle(
            24, // dup3
            SyscallArgs {
                a0: guest_fd as u64,
                a1: new_guest_fd as u64,
                a2: libc::O_CLOEXEC as u64,
                a3: 0,
                a4: 0,
                a5: 0,
            },
            &mut mem,
        )
        .expect("dup3 should succeed");

    assert_eq!(ret, new_guest_fd as i64);

    let stat_addr = 0x3000;
    let r = handler
        .handle(
            80, // fstat
            SyscallArgs {
                a0: new_guest_fd as u64,
                a1: stat_addr,
                a2: 0,
                a3: 0,
                a4: 0,
                a5: 0,
            },
            &mut mem,
        )
        .expect("fstat on dup3 guest fd should succeed");

    assert_eq!(r, 0);
}
