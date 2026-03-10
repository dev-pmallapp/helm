use crate::exit::VmExit;

#[test]
fn mmio_exit_is_not_fatal() {
    let exit = VmExit::Mmio {
        addr: 0x0900_0000,
        data: [0x41, 0, 0, 0, 0, 0, 0, 0],
        len: 1,
        is_write: true,
    };
    assert!(!exit.is_fatal());
}

#[test]
fn shutdown_is_fatal() {
    assert!(VmExit::Shutdown.is_fatal());
}

#[test]
fn fail_entry_is_fatal() {
    let exit = VmExit::FailEntry {
        hardware_entry_failure_reason: 42,
    };
    assert!(exit.is_fatal());
}

#[test]
fn internal_error_is_fatal() {
    let exit = VmExit::InternalError { suberror: 1 };
    assert!(exit.is_fatal());
}

#[test]
fn hlt_is_not_fatal() {
    assert!(!VmExit::Hlt.is_fatal());
}

#[test]
fn intr_is_not_fatal() {
    assert!(!VmExit::Intr.is_fatal());
}

#[test]
fn debug_exit_is_not_fatal() {
    assert!(!VmExit::Debug.is_fatal());
}

#[test]
fn unknown_exit_is_not_fatal() {
    assert!(!VmExit::Unknown(255).is_fatal());
}
