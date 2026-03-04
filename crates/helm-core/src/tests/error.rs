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
