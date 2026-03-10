//! Build script: generate AArch64 instruction decoders from `.decode` files.
//!
//! This reads every `.decode` file under `src/arm/decode_files/` and
//! generates Rust source in `$OUT_DIR/` that can be `include!`'d from
//! the crate.

use std::path::Path;

fn main() {
    let decode_dir = Path::new("src/arm/decode_files");
    let out_dir = std::env::var("OUT_DIR").unwrap();
    let out_dir = Path::new(&out_dir);

    // Re-run if any .decode file changes.
    println!("cargo:rerun-if-changed={}", decode_dir.display());
    for entry in std::fs::read_dir(decode_dir).unwrap().flatten() {
        let p = entry.path();
        if p.extension().map_or(false, |e| e == "decode") {
            println!("cargo:rerun-if-changed={}", p.display());
            generate_decoder_file(&p, out_dir);
        }
    }

    // Also generate from all QEMU decode files.
    let qemu_dir = decode_dir.join("qemu");
    if qemu_dir.exists() {
        println!("cargo:rerun-if-changed={}", qemu_dir.display());
        for entry in std::fs::read_dir(&qemu_dir).unwrap().flatten() {
            let p = entry.path();
            if p.extension().map_or(false, |e| e == "decode") {
                println!("cargo:rerun-if-changed={}", p.display());
                generate_decoder_file(&p, out_dir);
            }
        }
    }
}

fn generate_decoder_file(decode_path: &Path, out_dir: &Path) {
    let stem = decode_path.file_stem().unwrap().to_str().unwrap();
    let text = std::fs::read_to_string(decode_path).unwrap();
    let tree = helm_decode::DecodeTree::from_decode_text(&text);

    if tree.is_empty() {
        return;
    }

    // ── Validate: detect overlaps, shadows, duplicates ──────────
    let diags = helm_decode::validate(&tree);
    for d in &diags {
        let cargo_level = match d.severity {
            helm_decode::Severity::Info => "note",
            helm_decode::Severity::Warning => "warning",
            helm_decode::Severity::Error => "error",
        };
        println!("cargo:{cargo_level}={}: {}", decode_path.display(), d);
    }
    if helm_decode::has_errors(&diags) {
        let is_qemu = decode_path.to_str().map_or(false, |p| p.contains("qemu/"));
        if is_qemu {
            let err_count = diags
                .iter()
                .filter(|d| d.severity == helm_decode::Severity::Error)
                .count();
            println!(
                "cargo:warning={}: skipped ({} validation errors — needs parser extensions)",
                decode_path.display(),
                err_count,
            );
            return;
        }
        panic!(
            "{}: {} validation error(s) — fix the .decode file",
            decode_path.display(),
            diags
                .iter()
                .filter(|d| d.severity == helm_decode::Severity::Error)
                .count()
        );
    }

    // Sanitize stem for use as Rust identifier.
    let fn_name = format!("decode_{}", stem.replace('-', "_"));

    // 1. Name-only decoder (returns &'static str).
    let name_code = helm_decode::generate_name_decoder(&tree, &fn_name);
    let name_path = out_dir.join(format!("{fn_name}.rs"));
    std::fs::write(&name_path, &name_code).unwrap();

    // 2. Trait-based decoder for handler dispatch.
    let trait_name = fn_name
        .split('_')
        .map(|s| {
            let mut c = s.chars();
            match c.next() {
                None => String::new(),
                Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
            }
        })
        .collect::<String>()
        + "Handler";

    let handler_code = helm_decode::generate_decoder(
        &tree,
        &helm_decode::CodegenOpts {
            fn_name: &format!("{fn_name}_dispatch"),
            trait_name: Some(&trait_name),
            return_type: "Result<(), helm_core::HelmError>",
            fallthrough: "Err(helm_core::HelmError::Decode { addr: 0, reason: format!(\"unimplemented {}\", insn) })",
            extract_fields: true,
            ..Default::default()
        },
    );
    let handler_path = out_dir.join(format!("{fn_name}_handler.rs"));
    std::fs::write(&handler_path, &handler_code).unwrap();

    eprintln!(
        "helm-isa build.rs: {} → {} ({} patterns)",
        decode_path.display(),
        name_path.display(),
        tree.len()
    );
}
