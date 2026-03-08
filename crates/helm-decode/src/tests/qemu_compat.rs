//! QEMU decode-tree compatibility tests.
//!
//! These mirror QEMU's `tests/decode/` suite.  Each `err_*.decode` file
//! must produce at least one Error diagnostic.  Each `succ_*.decode`
//! file must parse and validate without errors.

use crate::validate::{has_errors, parse_and_validate, Severity};

const QEMU_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/qemu_decode");

fn load(name: &str) -> String {
    let path = format!("{QEMU_DIR}/{name}");
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("cannot read {path}: {e}"))
}

macro_rules! err_test {
    ($name:ident) => {
        #[test]
        fn $name() {
            let text = load(concat!(stringify!($name), ".decode"));
            let (_, diags) = parse_and_validate(&text);
            assert!(
                has_errors(&diags),
                "{}: expected error but got none.\ndiags: {:?}",
                stringify!($name),
                diags,
            );
        }
    };
}

macro_rules! succ_test {
    ($name:ident) => {
        #[test]
        fn $name() {
            let text = load(concat!(stringify!($name), ".decode"));
            let (tree, diags) = parse_and_validate(&text);
            let errors: Vec<_> = diags
                .iter()
                .filter(|d| d.severity == Severity::Error)
                .collect();
            assert!(
                errors.is_empty(),
                "{}: unexpected errors:\n{}",
                stringify!($name),
                errors
                    .iter()
                    .map(|d| d.to_string())
                    .collect::<Vec<_>>()
                    .join("\n"),
            );
            assert!(tree.is_some(), "{}: parse returned None", stringify!($name));
        }
    };
}

// ── err_ tests (must produce errors) ───────────────────────────────
err_test!(err_overlap7);
err_test!(err_width1);
err_test!(err_width2);
err_test!(err_width3);
err_test!(err_width4);
err_test!(err_overlap5);

// err_overlap1: needs %field reference resolution in patterns (TODO)
// err_overlap8: needs unspecified-bit detection '. vs -' (TODO)
// These are marked as manual tests below:
#[test]
#[ignore = "needs %field reference resolution"]
fn err_overlap1_field_ref() {
    let text = load("err_overlap1.decode");
    let (_, diags) = parse_and_validate(&text);
    assert!(has_errors(&diags));
}

#[test]
#[ignore = "needs unspecified-bit detection"]
fn err_overlap8_unspecified_bit() {
    let text = load("err_overlap8.decode");
    let (_, diags) = parse_and_validate(&text);
    assert!(has_errors(&diags));
}

// ── succ_ tests (must parse without errors) ────────────────────────
succ_test!(succ_function);
succ_test!(succ_ident1);
succ_test!(succ_argset_type1);
succ_test!(succ_infer1);
succ_test!(succ_named_field);
succ_test!(succ_pattern_group_nest1);
succ_test!(succ_pattern_group_nest2);
succ_test!(succ_pattern_group_nest3);
succ_test!(succ_pattern_group_nest4);

// ── QEMU a64.decode: must parse without errors ─────────────────────
#[test]
fn qemu_a64_decode_parses_without_errors() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../crates/helm-isa/src/arm/decode_files/qemu/a64.decode"
    );
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(_) => return,
    };
    let (tree, diags) = parse_and_validate(&text);
    let errors: Vec<_> = diags
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .collect();
    assert!(
        errors.is_empty(),
        "a64.decode has {} errors:\n{}",
        errors.len(),
        errors
            .iter()
            .take(5)
            .map(|d| d.to_string())
            .collect::<Vec<_>>()
            .join("\n"),
    );
    let t = tree.unwrap();
    assert!(t.len() > 1000, "expected >1000 patterns, got {}", t.len());
}
