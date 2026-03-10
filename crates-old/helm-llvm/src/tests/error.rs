//! Tests for the Error type and its variants

use crate::error::Error;

#[test]
fn test_parse_error_display() {
    let e = Error::ParseError("unexpected token".to_string());
    let msg = e.to_string();
    assert!(msg.contains("unexpected token"), "got: {msg}");
    assert!(msg.contains("parse") || msg.contains("Parse"), "got: {msg}");
}

#[test]
fn test_invalid_instruction_display() {
    let e = Error::InvalidInstruction("bad opcode".to_string());
    let msg = e.to_string();
    assert!(msg.contains("bad opcode"), "got: {msg}");
}

#[test]
fn test_unsupported_operation_display() {
    let e = Error::UnsupportedOperation("atomic".to_string());
    let msg = e.to_string();
    assert!(msg.contains("atomic"), "got: {msg}");
}

#[test]
fn test_resource_exhausted_display() {
    let e = Error::ResourceExhausted("no ports".to_string());
    let msg = e.to_string();
    assert!(msg.contains("no ports"), "got: {msg}");
}

#[test]
fn test_scheduling_error_display() {
    let e = Error::SchedulingError("deadlock".to_string());
    let msg = e.to_string();
    assert!(msg.contains("deadlock"), "got: {msg}");
}

#[test]
fn test_type_mismatch_display() {
    let e = Error::TypeMismatch {
        expected: "i32".to_string(),
        actual: "float".to_string(),
    };
    let msg = e.to_string();
    assert!(msg.contains("i32"), "got: {msg}");
    assert!(msg.contains("float"), "got: {msg}");
}

#[test]
fn test_llvm_error_display() {
    let e = Error::LLVMError("module verification failed".to_string());
    let msg = e.to_string();
    assert!(msg.contains("module verification failed"), "got: {msg}");
}

#[test]
fn test_other_error_display() {
    let e = Error::Other("something went wrong".to_string());
    let msg = e.to_string();
    assert!(msg.contains("something went wrong"), "got: {msg}");
}

#[test]
fn test_io_error_from_conversion() {
    let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "file missing");
    let e: Error = io_err.into();
    let msg = e.to_string();
    assert!(
        msg.contains("file missing") || msg.contains("IO") || msg.contains("io"),
        "got: {msg}"
    );
}

#[test]
fn test_result_type_is_result_of_error() {
    // Ensure crate::error::Result<T> is equivalent to std::result::Result<T, Error>
    let ok: crate::error::Result<u32> = Ok(42);
    assert_eq!(ok.unwrap(), 42);

    let err: crate::error::Result<u32> = Err(Error::Other("oops".to_string()));
    assert!(err.is_err());
}
