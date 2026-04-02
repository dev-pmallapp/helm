//! Build script for helm-jit: compiles C stencil source files and extracts
//! stencil bytes + relocations into generated Rust source files.
//!
//! Only runs when the `backend-stencil` feature is enabled.

#[cfg(feature = "backend-stencil")]
#[allow(
    clippy::doc_markdown,
    clippy::cast_possible_truncation,
    clippy::manual_let_else,
    clippy::uninlined_format_args
)]
mod stencil_build {
    use object::{
        Object, ObjectSection, ObjectSymbol, RelocationFlags, RelocationTarget, SymbolKind,
    };
    use std::collections::{BTreeMap, HashMap};
    use std::fmt::Write as FmtWrite;
    use std::fs;
    use std::path::{Path, PathBuf};

    /// A relocation found in the .text section referencing a HOLE_* symbol.
    struct HoleReloc {
        /// Byte offset within the function's code.
        offset: u32,
        /// The HOLE_* symbol name (e.g. "HOLE_RD_OFF").
        symbol_name: String,
        /// Whether this is a PC-relative relocation (R_X86_64_PLT32).
        is_pc_rel: bool,
    }

    /// Extracted stencil from an object file.
    struct ExtractedStencil {
        /// Function name (e.g. "stencil_add_imm").
        name: String,
        /// Raw x86-64 bytes.
        bytes: Vec<u8>,
        /// Relocations referencing HOLE_* symbols.
        relocs: Vec<HoleReloc>,
    }

    /// Map a HOLE_* symbol name to its Rust HoleKind representation.
    fn hole_kind_rust(name: &str) -> Option<String> {
        Some(match name {
            "HOLE_RD_OFF" => "HoleKind::RegOffset(RegField::Rd)".to_string(),
            "HOLE_RN_OFF" => "HoleKind::RegOffset(RegField::Rn)".to_string(),
            "HOLE_RM_OFF" => "HoleKind::RegOffset(RegField::Rm)".to_string(),
            "HOLE_RA_OFF" => "HoleKind::RegOffset(RegField::Ra)".to_string(),
            "HOLE_RT_OFF" => "HoleKind::RegOffset(RegField::Rt)".to_string(),
            "HOLE_RT2_OFF" => "HoleKind::RegOffset(RegField::Rt2)".to_string(),
            "HOLE_IMM" => "HoleKind::ImmZext".to_string(),
            "HOLE_SIMM" => "HoleKind::Simm".to_string(),
            "HOLE_SHAMT" => "HoleKind::Shamt".to_string(),
            "HOLE_TARGET" => "HoleKind::BranchTarget".to_string(),
            "HOLE_NEXT_PC" => "HoleKind::NextPc".to_string(),
            "HOLE_MEM_READ" => "HoleKind::Helper(HelperFn::MemRead)".to_string(),
            "HOLE_MEM_WRITE" => "HoleKind::Helper(HelperFn::MemWrite)".to_string(),
            _ => return None,
        })
    }

    /// Whether a stencil is a pure leaf function (no push/call/jmp to helpers).
    /// Detected by checking if first byte is NOT push (0x53=push rbx, 0x41=REX prefix for push r1x)
    /// and the function contains no `call` (0xFF with modrm) or `jmp *reg` instructions.
    fn is_leaf(bytes: &[u8]) -> bool {
        if bytes.is_empty() {
            return true;
        }
        // Non-leaf indicators: push rbx (0x53), push r12-r15 (0x41 0x54-0x57)
        if bytes[0] == 0x53 {
            return false;
        }
        if bytes.len() >= 2 && bytes[0] == 0x41 && (0x54..=0x57).contains(&bytes[1]) {
            return false;
        }
        // Also check for indirect jmp *rax (0xFF 0xE0) which stores use for tail-call
        for i in 0..bytes.len().saturating_sub(1) {
            if bytes[i] == 0xFF && bytes[i + 1] == 0xE0 {
                return false; // jmp *rax (tail call)
            }
        }
        true
    }

    /// Whether a stencil function is a terminator (returns exit code).
    fn is_terminator(name: &str) -> bool {
        let terminators = [
            "stencil_b",
            "stencil_bl",
            "stencil_br",
            "stencil_blr",
            "stencil_ret",
            "stencil_cbz",
            "stencil_cbnz",
            "stencil_bcond",
            "stencil_tbz",
            "stencil_tbnz",
            "stencil_svc",
            "stencil_rv_beq",
            "stencil_rv_bne",
            "stencil_rv_blt",
            "stencil_rv_bge",
            "stencil_rv_bltu",
            "stencil_rv_bgeu",
            "stencil_rv_jal",
            "stencil_rv_jalr",
            "stencil_rv_ecall",
        ];
        terminators.contains(&name)
    }

    /// Compile a C file to an object file using the cc crate's compiler detection
    /// but with manual invocation for full control over output location.
    fn compile_c_manual(src: &Path, out_dir: &Path) -> PathBuf {
        let stem = src.file_stem().unwrap().to_str().unwrap();
        let obj_path = out_dir.join(format!("{stem}.o"));
        let include_dir = src.parent().unwrap();

        let status = std::process::Command::new("cc")
            .arg("-c")
            .arg("-O2")
            .arg("-fno-pic")
            .arg("-fno-PIC")
            .arg("-fomit-frame-pointer")
            .arg("-fno-stack-protector")
            .arg("-fno-asynchronous-unwind-tables")
            .arg("-fno-exceptions")
            .arg("-fno-jump-tables")
            .arg(format!("-I{}", include_dir.display()))
            .arg("-o")
            .arg(&obj_path)
            .arg(src)
            .status()
            .expect("failed to invoke C compiler");

        assert!(status.success(), "C compiler failed for {}", src.display());

        obj_path
    }

    /// Extract stencils from an ELF object file.
    fn extract_stencils(obj_path: &Path) -> Vec<ExtractedStencil> {
        let data = fs::read(obj_path).expect("failed to read object file");
        let obj = object::File::parse(&*data).expect("failed to parse object file");

        // Build a map from symbol index (as usize) to symbol name for HOLE_* symbols.
        let mut hole_symbols: HashMap<usize, String> = HashMap::new();
        for sym in obj.symbols() {
            let name = match sym.name() {
                Ok(n) => n,
                Err(_) => continue,
            };
            if name.starts_with("HOLE_") {
                hole_symbols.insert(sym.index().0, name.to_string());
            }
        }

        // Build a map from symbol name to (section_index, offset, size) for functions.
        let mut functions: BTreeMap<String, (object::SectionIndex, u64, u64)> = BTreeMap::new();
        for sym in obj.symbols() {
            if sym.kind() != SymbolKind::Text {
                continue;
            }
            let name = match sym.name() {
                Ok(n) if n.starts_with("stencil_") => n.to_string(),
                _ => continue,
            };
            if let Some(section_idx) = sym.section_index() {
                functions.insert(name, (section_idx, sym.address(), sym.size()));
            }
        }

        // Get .text section data and relocations.
        let text_section = obj
            .section_by_name(".text")
            .expect("no .text section in object file");
        let text_data = text_section.data().expect("failed to read .text data");
        let text_addr = text_section.address();

        // Collect relocations by offset. Store (sym_idx, addend, is_pc_rel).
        // R_X86_64_PLT32 (type 4) and R_X86_64_PC32 (type 2) are PC-relative.
        // R_X86_64_32S (type 11) and R_X86_64_32 (type 10) are absolute.
        let mut relocs_by_offset: BTreeMap<u64, (object::SymbolIndex, i64, bool)> = BTreeMap::new();
        for (offset, reloc) in text_section.relocations() {
            if let RelocationTarget::Symbol(sym_idx) = reloc.target() {
                let is_pc_rel = match reloc.flags() {
                    RelocationFlags::Elf { r_type } => {
                        r_type == 2 || r_type == 4 // R_X86_64_PC32=2, R_X86_64_PLT32=4
                    }
                    _ => false,
                };
                relocs_by_offset.insert(offset, (sym_idx, reloc.addend(), is_pc_rel));
            }
        }

        // Extract each function.
        let mut stencils = Vec::new();
        for (name, (_, sym_addr, sym_size)) in &functions {
            let func_offset = (*sym_addr - text_addr) as usize;
            let func_size = *sym_size as usize;

            if func_size == 0 || func_offset + func_size > text_data.len() {
                continue;
            }

            let bytes = text_data[func_offset..func_offset + func_size].to_vec();

            // Find relocations within this function's range.
            let mut func_relocs = Vec::new();
            for (&offset, &(sym_idx, _addend, is_pc_rel)) in &relocs_by_offset {
                if offset >= *sym_addr && offset < sym_addr + sym_size {
                    if let Some(hole_name) = hole_symbols.get(&sym_idx.0) {
                        func_relocs.push(HoleReloc {
                            offset: (offset - sym_addr) as u32,
                            symbol_name: hole_name.clone(),
                            is_pc_rel,
                        });
                    }
                }
            }

            stencils.push(ExtractedStencil {
                name: name.clone(),
                bytes,
                relocs: func_relocs,
            });
        }

        stencils
    }

    /// Generate a Rust source file from extracted stencils.
    fn generate_rust(stencils: &[ExtractedStencil], prefix: &str) -> String {
        let mut out = String::new();
        // NOTE: No `use` statements here — the host file (data/aarch64.rs or
        // data/riscv64.rs) provides all necessary imports before `include!()`.
        writeln!(
            out,
            "// Auto-generated stencil data for {prefix}. DO NOT EDIT."
        )
        .unwrap();
        writeln!(out, "// Generated by build.rs from stencil_gen/{prefix}.c").unwrap();
        writeln!(out).unwrap();

        for s in stencils {
            let upper = s.name.to_uppercase();

            // Emit bytes array.
            write!(out, "static BYTES_{upper}: [u8; {}] = [", s.bytes.len()).unwrap();
            for (i, b) in s.bytes.iter().enumerate() {
                if i % 16 == 0 {
                    write!(out, "\n    ").unwrap();
                }
                write!(out, "0x{b:02x}, ").unwrap();
            }
            writeln!(out, "\n];").unwrap();
            writeln!(out).unwrap();

            // Emit relocs array.
            writeln!(
                out,
                "static RELOCS_{upper}: [StencilReloc; {}] = [",
                s.relocs.len()
            )
            .unwrap();
            for r in &s.relocs {
                if let Some(kind) = hole_kind_rust(&r.symbol_name) {
                    let reloc_kind = if r.is_pc_rel {
                        "RelocKind::PcRel32"
                    } else {
                        "RelocKind::Abs32"
                    };
                    writeln!(
                        out,
                        "    StencilReloc {{ byte_offset: {}, hole: {kind}, kind: {reloc_kind} }},",
                        r.offset
                    )
                    .unwrap();
                }
            }
            writeln!(out, "];").unwrap();
            writeln!(out).unwrap();

            // Emit Stencil struct.
            // Non-terminators: strip trailing ret (0xC3) so stencils chain
            // without premature return. The block compiler appends its own
            // epilogue (PC update + mov rax,0 + ret) after the last stencil.
            let is_term = is_terminator(&s.name);
            let leaf = is_leaf(&s.bytes);
            // Leaf non-terminators: strip trailing ret so they can chain.
            let body_len = if leaf && !is_term && s.bytes.last() == Some(&0xC3) {
                s.bytes.len() - 1
            } else {
                s.bytes.len()
            };
            writeln!(out, "pub static {upper}: Stencil = Stencil {{").unwrap();
            writeln!(out, "    bytes: &BYTES_{upper},").unwrap();
            writeln!(out, "    body_len: {body_len},").unwrap();
            writeln!(out, "    relocs: &RELOCS_{upper},").unwrap();
            writeln!(out, "    is_terminator: {is_term},").unwrap();
            writeln!(out, "    is_leaf: {leaf},").unwrap();
            writeln!(out, "}};").unwrap();
            writeln!(out).unwrap();
        }

        out
    }

    pub fn run() {
        let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
        let out_dir = PathBuf::from(std::env::var("OUT_DIR").unwrap());

        let stencil_dir = manifest_dir.join("stencil_gen");
        let a64_src = stencil_dir.join("aarch64.c");
        let rv64_src = stencil_dir.join("riscv64.c");

        // Rerun if C sources change.
        println!("cargo:rerun-if-changed=stencil_gen/aarch64.c");
        println!("cargo:rerun-if-changed=stencil_gen/riscv64.c");
        println!("cargo:rerun-if-changed=stencil_gen/common.h");

        // Compile C files.
        let a64_obj = compile_c_manual(&a64_src, &out_dir);
        let rv64_obj = compile_c_manual(&rv64_src, &out_dir);

        // Extract stencils.
        let a64_stencils = extract_stencils(&a64_obj);
        let rv64_stencils = extract_stencils(&rv64_obj);

        // Generate Rust source files.
        let a64_rust = generate_rust(&a64_stencils, "aarch64");
        let rv64_rust = generate_rust(&rv64_stencils, "riscv64");

        let gen_a64_path = out_dir.join("generated_a64.rs");
        let gen_rv64_path = out_dir.join("generated_rv64.rs");

        fs::write(&gen_a64_path, a64_rust).expect("failed to write generated_a64.rs");
        fs::write(&gen_rv64_path, rv64_rust).expect("failed to write generated_rv64.rs");
    }
}

fn main() {
    #[cfg(feature = "backend-stencil")]
    stencil_build::run();
}
