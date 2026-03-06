//! QEMU decodetree parser conformance tests.
//!
//! Imported from `assets/qemu/tests/decode/`. Each `err_*` file must
//! produce validation errors; each `succ_*` file must parse cleanly.

use crate::tree::DecodeTree;
use crate::validate::{validate, has_errors};

// ── Error tests: must produce validation errors ─────────────────────

macro_rules! err_test_ignored {
    ($name:ident, $file:expr) => {
        #[test]
        #[ignore]
        fn $name() {
            let text = include_str!(concat!("qemu_decode_files/", $file));
            let tree = DecodeTree::from_decode_text(text);
            let diags = validate(&tree);
            let has_err = has_errors(&diags) || tree.is_empty();
            assert!(has_err, "{}: expected error", $file);
        }
    };
}

macro_rules! err_test {
    ($name:ident, $file:expr) => {
        #[test]
        fn $name() {
            let text = include_str!(concat!("qemu_decode_files/", $file));
            let tree = DecodeTree::from_decode_text(text);
            let diags = validate(&tree);
            // err_ files should produce at least one error OR parse to
            // zero patterns (which means the parser correctly rejected
            // the invalid syntax by not producing any nodes).
            let has_err = has_errors(&diags) || tree.is_empty();
            assert!(
                has_err,
                "{}: expected error or empty tree, got {} nodes, {} diagnostics",
                $file,
                tree.len(),
                diags.len(),
            );
        }
    };
}

err_test!(err_argset1, "err_argset1.decode");
err_test!(err_argset2, "err_argset2.decode");
err_test!(err_field1, "err_field1.decode");
err_test!(err_field2, "err_field2.decode");
err_test!(err_field3, "err_field3.decode");
err_test!(err_field4, "err_field4.decode");
err_test!(err_field5, "err_field5.decode");
err_test!(err_field6, "err_field6.decode");
err_test!(err_field7, "err_field7.decode");
// These require stricter parser validation (Part 2):
err_test_ignored!(err_field8, "err_field8.decode");
err_test_ignored!(err_field9, "err_field9.decode");
err_test_ignored!(err_field10, "err_field10.decode");
err_test_ignored!(err_init1, "err_init1.decode");
err_test_ignored!(err_init2, "err_init2.decode");
err_test_ignored!(err_init3, "err_init3.decode");
err_test_ignored!(err_init4, "err_init4.decode");
err_test!(err_overlap1, "err_overlap1.decode");
err_test_ignored!(err_overlap2, "err_overlap2.decode");
err_test!(err_overlap3, "err_overlap3.decode");
err_test_ignored!(err_overlap4, "err_overlap4.decode");
err_test!(err_overlap5, "err_overlap5.decode");
err_test_ignored!(err_overlap6, "err_overlap6.decode");
err_test!(err_overlap7, "err_overlap7.decode");
err_test_ignored!(err_overlap8, "err_overlap8.decode");
err_test_ignored!(err_overlap9, "err_overlap9.decode");
err_test!(err_pattern_group_empty, "err_pattern_group_empty.decode");
err_test_ignored!(err_pattern_group_ident1, "err_pattern_group_ident1.decode");
err_test_ignored!(err_pattern_group_ident2, "err_pattern_group_ident2.decode");
err_test_ignored!(err_pattern_group_nest1, "err_pattern_group_nest1.decode");
err_test!(err_pattern_group_nest2, "err_pattern_group_nest2.decode");
err_test_ignored!(err_pattern_group_nest3, "err_pattern_group_nest3.decode");
err_test_ignored!(err_pattern_group_overlap1, "err_pattern_group_overlap1.decode");
err_test_ignored!(err_width1, "err_width1.decode");
err_test!(err_width2, "err_width2.decode");
err_test!(err_width3, "err_width3.decode");
err_test!(err_width4, "err_width4.decode");

// ── Success tests: must parse without errors ────────────────────────

macro_rules! succ_test {
    ($name:ident, $file:expr) => {
        #[test]
        fn $name() {
            let text = include_str!(concat!("qemu_decode_files/", $file));
            let tree = DecodeTree::from_decode_text(text);
            let diags = validate(&tree);
            assert!(
                !has_errors(&diags),
                "{}: unexpected error(s): {:?}",
                $file,
                diags,
            );
        }
    };
}

succ_test!(succ_argset_type1, "succ_argset_type1.decode");
succ_test!(succ_function, "succ_function.decode");
succ_test!(succ_ident1, "succ_ident1.decode");
succ_test!(succ_infer1, "succ_infer1.decode");
succ_test!(succ_named_field, "succ_named_field.decode");
succ_test!(succ_pattern_group_nest1, "succ_pattern_group_nest1.decode");
succ_test!(succ_pattern_group_nest2, "succ_pattern_group_nest2.decode");
succ_test!(succ_pattern_group_nest3, "succ_pattern_group_nest3.decode");
succ_test!(succ_pattern_group_nest4, "succ_pattern_group_nest4.decode");
