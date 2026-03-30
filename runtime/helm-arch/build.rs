//! Build script for helm-arch.
//!
//! Drives helm-decode's code-generation pipeline over the RISC-V `.decode`
//! files, producing Rust source files in `OUT_DIR` that are then `include!`-ed
//! from `src/riscv/generated.rs`.

use helm_decode::codegen::{generate_decoder, CodegenOpts};
use helm_decode::DecodeTree;
use std::{env, fs, path::PathBuf};

fn main() {
    let out = PathBuf::from(env::var("OUT_DIR").unwrap());
    let src = PathBuf::from("src/riscv/decode_files");

    // Each entry: (file stem, source file, generated function name)
    let decode_files: &[(&str, &str)] = &[
        ("riscv64_base", "riscv64-base.decode"),
        ("riscv64_m", "riscv64-m.decode"),
        ("riscv64_a", "riscv64-a.decode"),
        ("riscv64_f", "riscv64-f.decode"),
        ("riscv64_zicsr", "riscv64-zicsr.decode"),
        ("riscv64_zb", "riscv64-zb.decode"),
        ("riscv64_v", "riscv64-v.decode"),
        ("riscv64_zvk", "riscv64-zvk.decode"),
    ];
    let vendor_files: &[(&str, &str)] = &[("riscv64_xthead", "vendor/riscv64-xthead.decode")];

    let all_files = decode_files.iter().chain(vendor_files.iter());

    for (stem, rel_path) in all_files {
        let input = src.join(rel_path);
        let output = out.join(format!("{stem}.rs"));

        println!("cargo:rerun-if-changed=src/riscv/decode_files/{rel_path}");

        let text = match fs::read_to_string(&input) {
            Ok(t) => t,
            Err(e) => {
                // Emit a compile error pointing at the missing file.
                eprintln!(
                    "cargo:warning=helm-arch build.rs: cannot read {}: {e}",
                    input.display()
                );
                continue;
            }
        };

        let tree = DecodeTree::from_decode_text(&text);

        let fn_name = format!("decode_{stem}");
        let code = generate_decoder(
            &tree,
            &CodegenOpts {
                fn_name: &fn_name,
                return_type: "&'static str",
                fallthrough: "\"UNKNOWN\"",
                visibility: "pub(crate)",
                extract_fields: false,
                nested_match: false,
                ..Default::default()
            },
        );

        fs::write(&output, &code)
            .unwrap_or_else(|e| panic!("failed to write {}: {e}", output.display()));
    }

    println!("cargo:rerun-if-changed=build.rs");
}
