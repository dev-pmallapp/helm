use helm_engine::se::threading::{
    classify_clone_flags, CloneFlagsError, CloneStyle, CLONE_FILES, CLONE_FS, CLONE_SIGHAND,
    CLONE_SYSVSEM, CLONE_THREAD, CLONE_VM,
};

#[test]
fn thread_style_clone_flags_are_recognized() {
    let flags = CLONE_VM | CLONE_FS | CLONE_FILES | CLONE_SIGHAND | CLONE_THREAD | CLONE_SYSVSEM;
    let style = classify_clone_flags(flags).expect("thread-style flags should be accepted");
    assert_eq!(style, CloneStyle::Thread);
}

#[test]
fn invalid_thread_style_flags_are_rejected() {
    // QEMU linux-user rejects namespace-style flags on the thread path.
    let clone_newns = 0x0002_0000u64;
    let flags =
        CLONE_VM | CLONE_FS | CLONE_FILES | CLONE_SIGHAND | CLONE_THREAD | CLONE_SYSVSEM | clone_newns;
    let err = classify_clone_flags(flags).expect_err("unsupported thread-style flags must fail");
    assert_eq!(err, CloneFlagsError::InvalidThreadFlags);
}
