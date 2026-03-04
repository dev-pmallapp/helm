//! Decode tree: ordered patterns with support for %field, &argset,
//! @format definitions, and {} groups.

use super::field::{self, FieldDef};
use super::format::{self, FormatDef};
use super::pattern::{self, ArgSet, DecodeLine, DecodePattern};
use std::collections::HashMap;

/// A node in the decode tree.
#[derive(Debug, Clone)]
pub struct DecodeNode {
    pub mnemonic: String,
    pub pattern: DecodePattern,
}

/// Collection of patterns, field definitions, formats, and argument sets.
///
/// The tree is built once from `.decode` text and then used for
/// read-only lookups from multiple threads (`Arc<DecodeTree>`).
#[derive(Debug, Clone, Default)]
pub struct DecodeTree {
    pub nodes: Vec<DecodeNode>,
    /// `%name` field definitions.
    pub field_defs: HashMap<String, FieldDef>,
    /// `&name` argument sets.
    pub arg_sets: HashMap<String, ArgSet>,
    /// `@name` format definitions.
    pub format_defs: HashMap<String, FormatDef>,
}

impl DecodeTree {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a pattern.
    pub fn add(&mut self, line: DecodeLine) {
        self.nodes.push(DecodeNode {
            mnemonic: line.mnemonic,
            pattern: line.pattern,
        });
    }

    /// Build a tree from `.decode` text.  Handles `%`, `&`, `@`, `#`,
    /// `{`/`}`, and pattern lines.
    pub fn from_decode_text(text: &str) -> Self {
        let mut tree = Self::new();

        for line in text.lines() {
            let trimmed = line.trim();

            // Field definition
            if trimmed.starts_with('%') {
                if let Some(fd) = field::parse_field_def(trimmed) {
                    tree.field_defs.insert(fd.name.clone(), fd);
                }
                continue;
            }

            // Argument set
            if trimmed.starts_with('&') {
                if let Some(aset) = pattern::parse_arg_set(trimmed) {
                    tree.arg_sets.insert(aset.name.clone(), aset);
                }
                continue;
            }

            // Format definition
            if trimmed.starts_with('@') {
                if let Some(fmt) = format::parse_format_def(trimmed) {
                    tree.format_defs.insert(fmt.name.clone(), fmt);
                }
                continue;
            }

            // Group delimiters (parsed but groups are flattened for now)
            if trimmed.starts_with('{') || trimmed.starts_with('}') {
                continue;
            }

            // Pattern line
            if let Some(dl) = pattern::parse_decode_line(trimmed) {
                tree.add(dl);
            }
        }

        tree
    }

    /// Look up the first matching pattern.
    pub fn lookup(&self, insn: u32) -> Option<(&str, Vec<(&str, u32)>)> {
        for node in &self.nodes {
            if node.pattern.matches(insn) {
                let fields = node.pattern.extract_fields(insn);
                return Some((&node.mnemonic, fields));
            }
        }
        None
    }

    /// Number of instruction patterns (excludes %field, &arg, @format defs).
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }
}
