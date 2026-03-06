use crate::os::linux::aarch64::nr;

#[test]
fn syscall_numbers_match_linux_kernel() {
    assert_eq!(nr::READ, 63);
    assert_eq!(nr::WRITE, 64);
    assert_eq!(nr::OPENAT, 56);
    assert_eq!(nr::CLOSE, 57);
    assert_eq!(nr::EXIT, 93);
    assert_eq!(nr::EXIT_GROUP, 94);
    assert_eq!(nr::BRK, 214);
    assert_eq!(nr::MMAP, 222);
    assert_eq!(nr::IOCTL, 29);
    assert_eq!(nr::FCNTL, 25);
    assert_eq!(nr::PPOLL, 73);
    assert_eq!(nr::UNAME, 160);
    assert_eq!(nr::GETPID, 172);
    assert_eq!(nr::RT_SIGACTION, 134);
    assert_eq!(nr::CLOCK_GETTIME, 113);
    assert_eq!(nr::GETRANDOM, 278);
}

#[test]
fn all_syscall_numbers_are_nonzero_except_io_setup() {
    // Spot-check that no critical syscall number is accidentally zero
    assert_ne!(nr::READ, 0);
    assert_ne!(nr::WRITE, 0);
    assert_ne!(nr::EXIT, 0);
    assert_ne!(nr::BRK, 0);
    assert_ne!(nr::MMAP, 0);
}

#[test]
fn aarch64_exit_and_exit_group_are_different() {
    assert_ne!(nr::EXIT, nr::EXIT_GROUP);
}

#[test]
fn aarch64_read_and_write_are_different() {
    assert_ne!(nr::READ, nr::WRITE);
}
