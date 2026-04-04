#![allow(missing_docs)]
use std::sync::mpsc;
use std::time::Duration;

use helm_engine::se::threading::{
    HostThreadRuntime, CLONE_FILES, CLONE_FS, CLONE_SETTLS, CLONE_SIGHAND, CLONE_SYSVSEM,
    CLONE_THREAD, CLONE_VM,
};
use helm_engine::{
    se::{LinuxAarch64SyscallHandler, SyscallArgs},
    FlatMem,
};

#[test]
fn thread_style_clone_spawns_host_thread() {
    let runtime = HostThreadRuntime::new(1);
    let flags = CLONE_VM | CLONE_FS | CLONE_FILES | CLONE_SIGHAND | CLONE_THREAD | CLONE_SYSVSEM;
    let (tx, rx) = mpsc::channel();
    let parent_thread = std::thread::current().id();

    let tid = runtime
        .spawn_thread_for_clone(flags, move || {
            tx.send(std::thread::current().id())
                .expect("send child thread id");
        })
        .expect("spawn host thread");

    let child_thread = rx
        .recv_timeout(Duration::from_secs(1))
        .expect("child thread should run");

    assert_eq!(tid, 2);
    assert_ne!(child_thread, parent_thread);
    runtime.join_all().expect("join spawned threads");
}

#[test]
fn host_thread_runtime_records_child_thread_pointer() {
    let runtime = HostThreadRuntime::new(1);
    let parent_tp = 0xAAAA_0000u64;
    let requested_tls = 0xBBBB_0000u64;
    let flags = CLONE_VM
        | CLONE_FS
        | CLONE_FILES
        | CLONE_SIGHAND
        | CLONE_THREAD
        | CLONE_SYSVSEM
        | CLONE_SETTLS;

    let tid = runtime
        .spawn_thread_for_clone_with_tp(flags, parent_tp, requested_tls, || {})
        .expect("spawn host thread with thread pointer");

    assert_eq!(runtime.thread_pointer(tid), Some(requested_tls));
    runtime.join_all().expect("join spawned threads");
}

#[test]
fn aarch64_clone_thread_path_returns_einval_until_guest_thread_exec_exists() {
    let mut handler = LinuxAarch64SyscallHandler::new(0x2000_0000);
    let mut mem = FlatMem::new(0, 1 << 20);
    let flags = CLONE_VM | CLONE_FS | CLONE_FILES | CLONE_SIGHAND | CLONE_THREAD | CLONE_SYSVSEM;
    let args = SyscallArgs {
        a0: flags,
        a1: 0x8000_0000,
        a2: 0,
        a3: 0,
        a4: 0,
        a5: 0,
    };

    let ret = handler
        .handle(220, args, &mut mem)
        .expect("clone syscall should return");

    assert_eq!(
        ret, -22,
        "thread-style clone must fail honestly with -EINVAL"
    );
}

#[test]
fn aarch64_clone_settls_thread_path_is_rejected() {
    let mut handler = LinuxAarch64SyscallHandler::new(0x2000_0000);
    let mut mem = FlatMem::new(0, 1 << 20);
    let requested_tls = 0xBBBB_0000u64;
    let flags = CLONE_VM
        | CLONE_FS
        | CLONE_FILES
        | CLONE_SIGHAND
        | CLONE_THREAD
        | CLONE_SYSVSEM
        | CLONE_SETTLS;
    let args = SyscallArgs {
        a0: flags,
        a1: 0x8000_0000,
        a2: 0,
        a3: requested_tls,
        a4: 0,
        a5: 0,
    };

    let ret = handler
        .handle(220, args, &mut mem)
        .expect("clone syscall should return");
    assert_eq!(ret, -22);
    assert_eq!(handler.thread_pointer_for_tid(1001), None);
}

#[test]
fn aarch64_clone_without_settls_thread_path_is_rejected() {
    let mut handler = LinuxAarch64SyscallHandler::new(0x2000_0000);
    let mut mem = FlatMem::new(0, 1 << 20);
    let parent_tp = 0xAAAA_0000u64;
    let flags = CLONE_VM | CLONE_FS | CLONE_FILES | CLONE_SIGHAND | CLONE_THREAD | CLONE_SYSVSEM;
    let args = SyscallArgs {
        a0: flags,
        a1: 0x8000_0000,
        a2: 0,
        a3: 0xDEAD_BEEFu64,
        a4: 0,
        a5: 0,
    };

    handler.set_thread_pointer(parent_tp);
    let ret = handler
        .handle(220, args, &mut mem)
        .expect("clone syscall should return");

    assert_eq!(ret, -22);
    assert_eq!(handler.thread_pointer_for_tid(1001), None);
}
