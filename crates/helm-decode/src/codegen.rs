//! Code generation: emit Rust source from a [`DecodeTree`].
//!
//! The generated code is a single `match`-style function that decodes
//! a 32-bit instruction word and dispatches to a handler trait method.
//!
//! # Architecture
//!
//! ```text
//! .decode file
//!      │ (parse)
//!      ▼
//!  DecodeTree          ◄── helm-decode::tree
//!      │ (codegen)
//!      ▼
//!  generated Rust src  ◄── this module
//!      │ (include!)
//!      ▼
//!  exec.rs / tcg.rs    ◄── helm-isa / helm-tcg
//! ```
//!
//! ## Usage from build.rs
//!
//! ```rust,ignore
//! use helm_decode::{DecodeTree, codegen};
//!
//! let tree = DecodeTree::from_decode_text(
//!     &std::fs::read_to_string("aarch64-simd.decode").unwrap()
//! );
//! let rust = codegen::generate_decoder(&tree, &codegen::CodegenOpts {
//!     fn_name: "dispatch_simd",
//!     trait_name: "SimdHandler",
//!     insn_param: "insn",
//!     extra_params: "&mut self, mem: &mut AddressSpace",
//!     visibility: "pub(crate)",
//!     ..Default::default()
//! });
//! std::fs::write(out_dir.join("simd_decode.rs"), rust).unwrap();
//! ```

use crate::tree::DecodeTree;
use std::fmt::Write;

/// Options controlling the shape of generated code.
#[derive(Debug, Clone)]
pub struct CodegenOpts<'a> {
    /// Name of the generated dispatch function.
    pub fn_name: &'a str,
    /// If set, generate a trait with one method per mnemonic and
    /// dispatch to `self.<method>(fields...)`.
    pub trait_name: Option<&'a str>,
    /// Name of the instruction-word parameter (default `"insn"`).
    pub insn_param: &'a str,
    /// Return type of the dispatch function.
    pub return_type: &'a str,
    /// Expression used for the fallthrough / unmatched case.
    pub fallthrough: &'a str,
    /// Visibility qualifier (`pub`, `pub(crate)`, etc.).
    pub visibility: &'a str,
    /// If true, emit field extraction as local `let` bindings.
    pub extract_fields: bool,
    /// If true, group patterns by top-level opcode bits for faster
    /// dispatch (nested match).
    pub nested_match: bool,
}

impl<'a> Default for CodegenOpts<'a> {
    fn default() -> Self {
        Self {
            fn_name: "decode",
            trait_name: None,
            insn_param: "insn",
            return_type: "&'static str",
            fallthrough: "\"UNKNOWN\"",
            visibility: "pub",
            extract_fields: false,
            nested_match: false,
        }
    }
}

/// Generate Rust source for a decode dispatch function.
///
/// Each pattern becomes an `if (insn & MASK) == VALUE { ... }` arm.
/// Patterns are emitted in file order (first match wins), preserving
/// the same semantics as QEMU's decodetree.
pub fn generate_decoder(tree: &DecodeTree, opts: &CodegenOpts<'_>) -> String {
    let mut out = String::with_capacity(4096);

    // ── trait (optional) ────────────────────────────────────────────
    if let Some(trait_name) = opts.trait_name {
        writeln!(out, "/// Auto-generated handler trait from .decode file.").unwrap();
        writeln!(out, "{} trait {} {{", opts.visibility, trait_name).unwrap();
        let mut seen = std::collections::HashSet::new();
        for node in &tree.nodes {
            let method = node.mnemonic.to_lowercase();
            if seen.insert(method.clone()) {
                let field_params: String = node
                    .pattern
                    .fields
                    .iter()
                    .map(|f| format!(", {}: u32", f.name))
                    .collect();
                writeln!(
                    out,
                    "    fn handle_{method}(&mut self, {}: u32{field_params}) -> {};",
                    opts.insn_param, opts.return_type
                )
                .unwrap();
            }
        }
        writeln!(out, "}}").unwrap();
        writeln!(out).unwrap();
    }

    // ── dispatch function ───────────────────────────────────────────
    let self_param = if opts.trait_name.is_some() {
        "&mut self, "
    } else {
        ""
    };
    writeln!(
        out,
        "/// Auto-generated decoder — do not edit (regenerate from .decode file)."
    )
    .unwrap();
    writeln!(
        out,
        "#[allow(unused_variables, clippy::unusual_byte_groupings)]"
    )
    .unwrap();
    writeln!(
        out,
        "{vis} fn {name}({self_param}{insn}: u32) -> {ret} {{",
        vis = opts.visibility,
        name = opts.fn_name,
        insn = opts.insn_param,
        ret = opts.return_type,
    )
    .unwrap();

    if opts.nested_match {
        emit_nested(tree, opts, &mut out);
    } else {
        emit_linear(tree, opts, &mut out);
    }

    writeln!(out, "    {}", opts.fallthrough).unwrap();
    writeln!(out, "}}").unwrap();
    out
}

/// Emit linear if-else chain (simple, preserves file order).
fn emit_linear(tree: &DecodeTree, opts: &CodegenOpts<'_>, out: &mut String) {
    let insn = opts.insn_param;
    for node in &tree.nodes {
        let p = &node.pattern;
        emit_pattern_arm(out, &node.mnemonic, p, insn, opts);
    }
}

/// Emit nested match on top-level opcode bits [28:25] then linear
/// scan within each group.  ~4× fewer comparisons for large trees.
fn emit_nested(tree: &DecodeTree, opts: &CodegenOpts<'_>, out: &mut String) {
    let insn = opts.insn_param;

    // Group by bits [28:25] (the A64 top-level encoding group).
    let mut groups: std::collections::BTreeMap<u32, Vec<usize>> = std::collections::BTreeMap::new();
    for (i, node) in tree.nodes.iter().enumerate() {
        let p = &node.pattern;
        // If the pattern fixes bits 28:25, group by those bits.
        let group_mask = 0x1E00_0000u32; // bits [28:25]
        if p.mask & group_mask == group_mask {
            let key = (p.value & group_mask) >> 25;
            groups.entry(key).or_default().push(i);
        } else {
            // Pattern doesn't fix all 4 bits — put in every possible group
            for key in 0..16u32 {
                if (key << 25) & p.mask == p.value & p.mask & group_mask {
                    groups.entry(key).or_default().push(i);
                }
            }
        }
    }

    writeln!(out, "    match ({insn} >> 25) & 0xF {{").unwrap();
    for (key, indices) in &groups {
        writeln!(out, "        {key:#06b} => {{").unwrap();
        for &i in indices {
            let node = &tree.nodes[i];
            emit_pattern_arm(out, &node.mnemonic, &node.pattern, insn, opts);
        }
        writeln!(out, "        }}").unwrap();
    }
    writeln!(out, "        _ => {{}}").unwrap();
    writeln!(out, "    }}").unwrap();
}

/// Emit a single if-arm for one pattern.
fn emit_pattern_arm(
    out: &mut String,
    mnemonic: &str,
    p: &crate::pattern::DecodePattern,
    insn: &str,
    opts: &CodegenOpts<'_>,
) {
    // Constraint check
    let mut constraint_cond = String::new();
    for (name, val) in &p.constraints {
        if let Some(f) = p.fields.iter().find(|f| f.name == *name) {
            write!(
                constraint_cond,
                " && ({insn} >> {}) & {:#x} == {val}",
                f.lsb,
                (1u32 << f.width) - 1
            )
            .unwrap();
        }
    }

    writeln!(
        out,
        "    if {insn} & {mask:#010x} == {value:#010x}{cond} {{",
        mask = p.mask,
        value = p.value,
        cond = constraint_cond,
    )
    .unwrap();

    // Field extraction
    if opts.extract_fields {
        for f in &p.fields {
            let fmask = (1u32 << f.width) - 1;
            writeln!(
                out,
                "        let {name} = ({insn} >> {lsb}) & {fmask:#x};",
                name = f.name,
                lsb = f.lsb,
            )
            .unwrap();
        }
    }

    // Body: return mnemonic or dispatch to handler
    if let Some(trait_name) = opts.trait_name {
        let method = mnemonic.to_lowercase();
        let args: String = if opts.extract_fields {
            p.fields.iter().map(|f| format!(", {}", f.name)).collect()
        } else {
            p.fields
                .iter()
                .map(|f| {
                    let fmask = (1u32 << f.width) - 1;
                    format!(", ({insn} >> {}) & {fmask:#x}", f.lsb)
                })
                .collect()
        };
        let _ = trait_name;
        writeln!(out, "        return self.handle_{method}({insn}{args});",).unwrap();
    } else {
        writeln!(out, "        return \"{mnemonic}\";").unwrap();
    }

    writeln!(out, "    }}").unwrap();
}

/// Convenience: generate a name-only decoder (returns `&'static str`).
pub fn generate_name_decoder(tree: &DecodeTree, fn_name: &str) -> String {
    generate_decoder(
        tree,
        &CodegenOpts {
            fn_name,
            ..Default::default()
        },
    )
}
