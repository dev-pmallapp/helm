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

            // Pattern line — expand @format references before parsing
            let expanded = expand_format_refs(trimmed, &tree.format_defs);
            if let Some(dl) = pattern::parse_decode_line(&expanded) {
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

/// Expand `@format` references in a pattern line.
///
/// When a pattern references a format, the format's field definitions
/// are appended to the pattern. The pattern keeps its own fixed bits;
/// the format provides field NAMES for the don't-care positions.
///
/// Example: `ADD_rrri .... 000 0000 . .... .... ..... .. 0 .... @s_rrr_shi`
/// The format `@s_rrr_shi` has `s:1 rn:4 rd:4 shim:5 shty:2 rm:4` which
/// map to the `....` positions in the pattern.
fn expand_format_refs(line: &str, formats: &HashMap<String, FormatDef>) -> String {
    let trimmed = line.trim();
    let parts: Vec<&str> = trimmed.split_whitespace().collect();

    // Find @format reference
    let format_ref = parts.iter().find(|p| p.starts_with('@'));
    let format_ref = match format_ref {
        Some(r) => r.trim_start_matches('@'),
        None => return line.to_string(),
    };

    let fmt = match formats.get(format_ref) {
        Some(f) => f,
        None => return line.to_string(),
    };

    // The format defines the FULL 32-bit layout (fields + fixed bits + don't-cares).
    // The pattern overrides some of the format's bits with its own fixed values.
    //
    // Strategy: merge the pattern's fixed bits into the format's template
    // character by character. Where the pattern has '0'/'1', use the pattern.
    // Where the pattern has '.', use whatever the format has (field or don't-care).
    //
    // Build a combined bit template from the format, then overlay the pattern's
    // fixed bits.

    let mnemonic = parts[0];

    // Collect all non-meta tokens from pattern and format as raw bit characters
    fn bit_chars(tokens: &[&str]) -> Vec<char> {
        let mut chars = Vec::new();
        for t in tokens {
            if t.starts_with('@')
                || t.starts_with('&')
                || t.starts_with('!')
                || t.starts_with('%')
                || t.starts_with('\\')
                || t.contains('=')
            {
                continue;
            }
            if t.contains(':') {
                // Field token: expand to '.' chars for its width
                if let Some(w) = t.split(':').nth(1) {
                    let w = w.trim_start_matches('s');
                    if let Ok(width) = w.parse::<usize>() {
                        for _ in 0..width {
                            chars.push('f');
                        } // 'f' = field placeholder
                    }
                }
                continue;
            }
            for c in t.chars() {
                if c != '_' {
                    chars.push(c);
                }
            }
        }
        chars
    }

    let pat_tokens: Vec<&str> = parts[1..].iter().copied().collect();
    let fmt_tokens: Vec<&str> = fmt.tokens.iter().map(|s| s.as_str()).collect();

    let pat_bits = bit_chars(&pat_tokens);
    let fmt_bits = bit_chars(&fmt_tokens);

    if pat_bits.len() != 32 || fmt_bits.len() != 32 {
        // Can't merge — dimensions don't match, return original
        return line.to_string();
    }

    // Merge: pattern fixed bits override format
    // Rebuild with format's field tokens at the right positions
    let mut merged_bits = String::with_capacity(32);
    for i in 0..32 {
        let pc = pat_bits[i];
        let fc = fmt_bits[i];
        match (pc, fc) {
            ('0' | '1', _) => merged_bits.push(pc), // pattern overrides
            ('.', 'f') => merged_bits.push('.'),    // format has field here
            ('.', c) => merged_bits.push(c),        // format has fixed bit
            (_, _) => merged_bits.push(pc),
        }
    }

    // Now reconstruct: walk the merged bits and the format's field tokens
    // to produce the final line
    let mut bit_idx = 0;
    let mut result = format!("{} ", mnemonic);

    for tok in &fmt.tokens {
        if tok == "\\" {
            continue;
        }
        if tok.starts_with('!') || tok.starts_with('%') || tok.starts_with('&') {
            continue;
        }
        if tok.contains(':') && !tok.contains('=') {
            // Field token — check if the pattern has fixed bits here
            let w_str = tok.split(':').nth(1).unwrap_or("0").trim_start_matches('s');
            let width: usize = w_str.parse().unwrap_or(0);
            let all_fixed = (bit_idx..bit_idx + width).all(|i| {
                i < 32 && matches!(merged_bits.as_bytes().get(i), Some(b'0') | Some(b'1'))
            });
            if all_fixed {
                // Pattern overrides this field with fixed bits
                result.push_str(&merged_bits[bit_idx..bit_idx + width]);
                result.push(' ');
            } else {
                result.push_str(tok);
                result.push(' ');
            }
            bit_idx += width;
        } else if tok.contains('=') {
            // Constraint — pass through
            result.push_str(tok);
            result.push(' ');
        } else {
            // Fixed bits from format
            for c in tok.chars() {
                if c == '_' {
                    continue;
                }
                if bit_idx < 32 {
                    result.push(merged_bits.as_bytes()[bit_idx] as char);
                }
                bit_idx += 1;
            }
            result.push(' ');
        }
    }

    // Also append pattern-specific constraints and extra fields
    for p in &parts[1..] {
        if p.starts_with('@') || p.starts_with('&') {
            continue;
        }
        if p.contains('=')
            && !p
                .chars()
                .all(|c| c == '0' || c == '1' || c == '.' || c == '-' || c == '_')
        {
            result.push_str(p);
            result.push(' ');
        }
    }

    result.trim().to_string()
}
