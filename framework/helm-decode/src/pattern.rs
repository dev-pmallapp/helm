//! Decode pattern: fixed-bit mask + value, plus named fields.

use super::field::{BitField, FieldDef};

/// An `&name` argument-set definition.
///
/// QEMU syntax: `&name field1 field2 ...`
#[derive(Debug, Clone)]
pub struct ArgSet {
    pub name: String,
    pub fields: Vec<String>,
}

/// A field slot in a decode pattern.
///
/// Simple contiguous fields (e.g. `%rd 7:5`) are stored as [`BitField`].
/// Multi-segment or transform-bearing fields (e.g. `%imm_b 31:s1 7:1 25:6 8:4`)
/// are stored as [`FieldDef`].
#[derive(Debug, Clone)]
pub enum FieldSlot {
    /// Single contiguous field.
    Simple(BitField),
    /// Multi-segment or transform-bearing field.
    Multi(FieldDef),
}

impl FieldSlot {
    /// Field name.
    pub fn name(&self) -> &str {
        match self {
            Self::Simple(f) => &f.name,
            Self::Multi(f) => &f.name,
        }
    }

    /// Extract the field value from an instruction word.
    pub fn extract(&self, insn: u32) -> u32 {
        match self {
            Self::Simple(f) => f.extract(insn),
            Self::Multi(f) => f.extract(insn),
        }
    }
}

/// Parse an `&name field1 field2 ...` line.
pub fn parse_arg_set(line: &str) -> Option<ArgSet> {
    let line = line.trim();
    if !line.starts_with('&') {
        return None;
    }
    let mut parts = line.split_whitespace();
    let name = parts.next()?.trim_start_matches('&').to_string();
    let fields: Vec<String> = parts.map(String::from).collect();
    if fields.is_empty() {
        return None;
    }
    Some(ArgSet { name, fields })
}

/// A single decoded instruction pattern line.
#[derive(Debug, Clone)]
pub struct DecodeLine {
    pub mnemonic: String,
    pub pattern: DecodePattern,
    /// Unresolved `%field_name` references (standalone or via `name=%field`).
    /// Resolved to `FieldSlot::Multi` by `DecodeTree::from_decode_text` after
    /// all `%field` definitions have been parsed.
    pub field_refs: Vec<String>,
}

/// Fixed-bit mask/value pair plus extracted fields.
///
/// An instruction matches when `(insn & mask) == value` AND all
/// field constraints are satisfied.
#[derive(Debug, Clone)]
pub struct DecodePattern {
    pub mask: u32,
    pub value: u32,
    /// Ordered list of fields to extract when this pattern matches.
    /// May contain simple single-segment fields or complex multi-segment ones.
    pub fields: Vec<FieldSlot>,
    /// Field-value constraints: `(field_name, required_value)`.
    pub constraints: Vec<(String, u32)>,
}

impl DecodePattern {
    /// Test whether an instruction word matches this pattern.
    pub fn matches(&self, insn: u32) -> bool {
        if (insn & self.mask) != self.value {
            return false;
        }
        // Check constraints
        for (name, expected) in &self.constraints {
            if let Some(f) = self.fields.iter().find(|f| f.name() == *name) {
                if f.extract(insn) != *expected {
                    return false;
                }
            }
        }
        true
    }

    /// Extract all fields from a matching instruction.
    pub fn extract_fields(&self, insn: u32) -> Vec<(&str, u32)> {
        self.fields
            .iter()
            .map(|f| (f.name(), f.extract(insn)))
            .collect()
    }
}

/// Parse a single decode-tree line into a `DecodeLine`.
///
/// Supports both HELM's simple format and QEMU's format:
/// - Fixed bits: `0`, `1`, `.` (don't-care), `-` (must-be-zero)
/// - Fields: `name:N` (N-bit field at current position)
/// - Constraints: `name=value`
/// - Separators: `_` and spaces (ignored)
pub fn parse_decode_line(line: &str) -> Option<DecodeLine> {
    let line = line.trim();
    if line.is_empty()
        || line.starts_with('#')
        || line.starts_with('%')
        || line.starts_with('&')
        || line.starts_with('@')
        || line.starts_with('{')
        || line.starts_with('}')
    {
        return None;
    }

    let mut parts = line.split_whitespace();
    let mnemonic = parts.next()?.to_string();

    let mut mask: u32 = 0;
    let mut value: u32 = 0;
    let mut fields = Vec::new();
    let mut constraints = Vec::new();
    let mut field_refs = Vec::new();
    let mut bit_pos: i32 = 31;

    for token in parts {
        // Skip format/argset references (handled at a higher level)
        if token.starts_with('@') || token.starts_with('&') {
            continue;
        }

        // Standalone %field_name reference — record for later resolution
        if token.starts_with('%') {
            let name = token.trim_start_matches('%').to_string();
            field_refs.push(name);
            continue;
        }

        // Field reference: name=%field_def (doesn't consume bits)
        if token.contains("=%") {
            let field_name = token.split("=%").nth(1).unwrap_or("").to_string();
            if !field_name.is_empty() {
                field_refs.push(field_name);
            }
            continue;
        }

        // !function= annotation (skip)
        if token.starts_with('!') {
            continue;
        }

        // Constraint: name=value
        if token.contains('=') && !token.contains(':') {
            let mut split = token.split('=');
            let name = split.next().unwrap().to_string();
            let val: u32 = split.next().and_then(|v| v.parse().ok()).unwrap_or(0);
            constraints.push((name, val));
            continue;
        }

        if token.contains(':') {
            // Field: "name:width" or "name:swidth" (signed)
            let mut split = token.split(':');
            let name = split.next().unwrap().to_string();
            let width_str = split.next().unwrap_or("0");
            let (signed, width_s) = if width_str.starts_with('s') {
                (true, width_str.strip_prefix('s').unwrap())
            } else {
                (false, width_str)
            };
            let width: u8 = width_s.parse().ok()?;
            let lsb = (bit_pos - width as i32 + 1) as u8;
            let mut f = BitField::new(name, lsb, width);
            if signed {
                f.sext = true;
            }
            fields.push(FieldSlot::Simple(f));
            bit_pos -= width as i32;
        } else {
            // Fixed bits
            for ch in token.chars() {
                match ch {
                    '0' | '-' => {
                        if bit_pos >= 0 {
                            mask |= 1u32 << bit_pos as u32;
                        }
                        bit_pos -= 1;
                    }
                    '1' => {
                        if bit_pos >= 0 {
                            mask |= 1u32 << bit_pos as u32;
                            value |= 1u32 << bit_pos as u32;
                        }
                        bit_pos -= 1;
                    }
                    '.' => {
                        bit_pos -= 1;
                    }
                    '_' => {} // cosmetic separator
                    _ => return None,
                }
            }
        }
    }

    // bit_pos must reach -1 for a fully-specified pattern without %field refs.
    // With %field refs, some bits are defined by the field def, so allow any
    // non-negative remainder (the fields cover the remaining bit positions).
    // Reject only if we over-consumed bits (bit_pos went below -1).
    if bit_pos < -1 {
        return None;
    }
    // For purely fixed-bit patterns (no field refs), require exact 32 bits.
    if field_refs.is_empty() && bit_pos != -1 {
        return None;
    }

    Some(DecodeLine {
        mnemonic,
        pattern: DecodePattern {
            mask,
            value,
            fields,
            constraints,
        },
        field_refs,
    })
}
