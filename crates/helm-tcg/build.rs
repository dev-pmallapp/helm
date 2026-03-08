//! Build script: generate handler traits from `.decode` files for TCG emission.
//!
//! Reuses the same `.decode` files as `helm-isa` to ensure the decoder and
//! TCG emitter stay in sync.

use std::path::Path;

fn main() {
    let decode_dir = Path::new("../helm-isa/src/arm/decode_files");
    let out_dir = std::env::var("OUT_DIR").unwrap();
    let out_dir = Path::new(&out_dir);

    println!("cargo:rerun-if-changed={}", decode_dir.display());

    // Generate handler traits from the per-group decode files.
    for name in &[
        "aarch64-branch",
        "aarch64-dp-imm",
        "aarch64-dp-reg",
        "aarch64-fp",
        "aarch64-ldst",
        "aarch64-simd",
    ] {
        let path = decode_dir.join(format!("{name}.decode"));
        if path.exists() {
            println!("cargo:rerun-if-changed={}", path.display());
            generate_handler(&path, out_dir);
        }
    }
}

fn generate_handler(decode_path: &Path, out_dir: &Path) {
    let stem = decode_path.file_stem().unwrap().to_str().unwrap();
    let text = std::fs::read_to_string(decode_path).unwrap();
    let tree = helm_decode::DecodeTree::from_decode_text(&text);

    if tree.is_empty() {
        return;
    }

    let fn_name = format!("decode_{}", stem.replace('-', "_"));

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

    let full_code = helm_decode::generate_decoder(
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

    // Split generated code into trait definition and dispatch function.
    // The trait goes at module level; the dispatch fn inside an `impl` block.
    let dispatch_marker = "/// Auto-generated decoder";
    let split_pos = full_code.find(dispatch_marker).unwrap_or(full_code.len());
    let trait_code = &full_code[..split_pos];
    let dispatch_code = &full_code[split_pos..];

    let trait_path = out_dir.join(format!("{fn_name}_trait.rs"));
    std::fs::write(&trait_path, trait_code).unwrap();

    // Wrap the dispatch function in a complete impl block so it can
    // be include!()'d at module level.
    let wrapped = format!(
        "impl A64TcgEmitter<'_> {{\n{}\n}}\n",
        dispatch_code
    );
    let dispatch_path = out_dir.join(format!("{fn_name}_dispatch.rs"));
    std::fs::write(&dispatch_path, &wrapped).unwrap();

    let handler_path = out_dir.join(format!("{fn_name}_handler.rs"));
    std::fs::write(&handler_path, &full_code).unwrap();

    eprintln!(
        "helm-tcg build.rs: {} → {} ({} patterns)",
        decode_path.display(),
        handler_path.display(),
        tree.len()
    );
}
