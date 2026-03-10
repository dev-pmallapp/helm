//! Inflate parity test: interp vs TCG backend.
//!
//! Runs a standalone gzip inflate binary through both backends and
//! asserts both produce the same exit code (0 = decompression correct).

use crate::se::backend::ExecBackend;
use crate::se::session::{SeSession, StopReason};

const INFLATE_BIN: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../assets/binaries/inflate_test"
);

const MAX_INSNS: u64 = 50_000_000;

#[test]
fn inflate_interp_passes() {
    let mut s = SeSession::new(INFLATE_BIN, &["inflate_test"], &[]).unwrap();
    let reason = s.run(MAX_INSNS);
    assert!(
        s.has_exited(),
        "interp should exit, got {reason:?} at PC={:#x} after {} insns",
        s.pc(),
        s.insn_count()
    );
    assert_eq!(
        s.exit_code(),
        0,
        "interp inflate should pass (exit 0), got exit code {}",
        s.exit_code()
    );
}

#[test]
fn inflate_tcg_passes() {
    let mut s = SeSession::new(INFLATE_BIN, &["inflate_test"], &[]).unwrap();
    s.set_backend(ExecBackend::tcg());
    let reason = s.run(MAX_INSNS);
    assert!(
        s.has_exited(),
        "tcg should exit, got {reason:?} at PC={:#x} after {} insns",
        s.pc(),
        s.insn_count()
    );
    assert_eq!(
        s.exit_code(),
        0,
        "tcg inflate should pass (exit 0), got exit code {}",
        s.exit_code()
    );
}
