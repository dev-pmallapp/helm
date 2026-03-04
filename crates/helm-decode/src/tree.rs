//! Decode tree: ordered list of patterns tested sequentially.
//!
//! A more sophisticated implementation would build a decision tree
//! keyed on discriminant bit-groups (like QEMU's decodetree.py).
//! This linear scan is correct and sufficient for bootstrapping;
//! the tree optimisation is a future enhancement.

use super::pattern::{DecodeLine, DecodePattern};

/// A node in the decode tree — currently just a flat pattern entry.
#[derive(Debug, Clone)]
pub struct DecodeNode {
    pub mnemonic: String,
    pub pattern: DecodePattern,
}

/// Collection of patterns tested in order.  First match wins.
#[derive(Debug, Clone, Default)]
pub struct DecodeTree {
    pub nodes: Vec<DecodeNode>,
}

impl DecodeTree {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a pattern to the tree.
    pub fn add(&mut self, line: DecodeLine) {
        self.nodes.push(DecodeNode {
            mnemonic: line.mnemonic,
            pattern: line.pattern,
        });
    }

    /// Build a tree from multiple `.decode` text blocks.
    pub fn from_decode_text(text: &str) -> Self {
        let mut tree = Self::new();
        for line in text.lines() {
            if let Some(dl) = super::pattern::parse_decode_line(line) {
                tree.add(dl);
            }
        }
        tree
    }

    /// Look up the first matching pattern. Returns mnemonic and fields.
    pub fn lookup(&self, insn: u32) -> Option<(&str, Vec<(&str, u32)>)> {
        for node in &self.nodes {
            if node.pattern.matches(insn) {
                let fields = node.pattern.extract_fields(insn);
                return Some((&node.mnemonic, fields));
            }
        }
        None
    }

    /// Number of patterns.
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }
}
