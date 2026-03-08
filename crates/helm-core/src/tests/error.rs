use crate::error::*;

#[test]
fn decode_error_displays_address() {
    let err = HelmError::Decode {
        addr: 0xDEAD,
        reason: "bad opcode".into(),
    };
    let msg = format!("{}", err);
    assert!(msg.contains("0xdead"), "should contain hex address: {msg}");
    assert!(msg.contains("bad opcode"));
}

#[test]
fn io_error_converts() {
    let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "gone");
    let helm_err: HelmError = io_err.into();
    assert!(matches!(helm_err, HelmError::Io(_)));
}

#[test]
fn all_error_variants_produce_non_empty_display() {
    let variants: Vec<HelmError> = vec![
        HelmError::Isa("bad isa".into()),
        HelmError::Translation("bad translation".into()),
        HelmError::Syscall {
            number: 42,
            reason: "denied".into(),
        },
        HelmError::Memory {
            addr: 0xDEAD,
            reason: "fault".into(),
        },
        HelmError::Pipeline("stall".into()),
        HelmError::Config("bad config".into()),
    ];
    for err in &variants {
        let msg = format!("{}", err);
        assert!(!msg.is_empty(), "display should not be empty for: {err:?}");
    }
}

#[test]
fn syscall_error_includes_syscall_number() {
    let err = HelmError::Syscall {
        number: 99,
        reason: "not allowed".into(),
    };
    let msg = format!("{}", err);
    assert!(msg.contains("99"), "should contain syscall number: {msg}");
    assert!(msg.contains("not allowed"));
}

#[test]
fn memory_error_includes_hex_address() {
    let err = HelmError::Memory {
        addr: 0xBEEF_CAFE,
        reason: "segfault".into(),
    };
    let msg = format!("{}", err);
    assert!(
        msg.to_lowercase().contains("beef"),
        "should contain hex address: {msg}"
    );
}

#[test]
fn helm_result_ok_unwraps() {
    let r: HelmResult<u64> = Ok(42);
    assert_eq!(r.unwrap(), 42);
}

#[test]
fn helm_result_err_is_error() {
    let r: HelmResult<u64> = Err(HelmError::Config("oops".into()));
    assert!(r.is_err());
    assert!(matches!(r.unwrap_err(), HelmError::Config(_)));
}
