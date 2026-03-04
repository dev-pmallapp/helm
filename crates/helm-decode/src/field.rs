//! Bit-field extraction from a 32-bit instruction word.

/// A named bit-field within an instruction encoding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BitField {
    /// Field name (e.g. `"rd"`, `"imm12"`).
    pub name: String,
    /// Least-significant bit position in the instruction word.
    pub lsb: u8,
    /// Width in bits.
    pub width: u8,
    /// If true, the extracted value is sign-extended.
    pub sext: bool,
}

impl BitField {
    pub fn new(name: impl Into<String>, lsb: u8, width: u8) -> Self {
        Self {
            name: name.into(),
            lsb,
            width,
            sext: false,
        }
    }

    pub fn signed(mut self) -> Self {
        self.sext = true;
        self
    }

    /// Extract the field value from a 32-bit instruction.
    pub fn extract(&self, insn: u32) -> u32 {
        let raw = (insn >> self.lsb) & ((1u32 << self.width) - 1);
        if self.sext {
            let sign_bit = 1u32 << (self.width - 1);
            if raw & sign_bit != 0 {
                raw | !((1u32 << self.width) - 1)
            } else {
                raw
            }
        } else {
            raw
        }
    }

    /// Mask covering this field's bit positions.
    pub fn mask(&self) -> u32 {
        ((1u32 << self.width) - 1) << self.lsb
    }
}

/// A `%name` field definition line from a `.decode` file.
///
/// QEMU syntax: `%name pos:len [pos:len ...]`
///
/// Multi-segment fields are concatenated (e.g. split immediate).
#[derive(Debug, Clone)]
pub struct FieldDef {
    pub name: String,
    /// Segments, from most-significant to least-significant in the
    /// concatenated result.
    pub segments: Vec<(u8, u8)>, // (lsb, width)
    pub sext: bool,
}

impl FieldDef {
    /// Extract and concatenate all segments.
    pub fn extract(&self, insn: u32) -> u32 {
        let mut result: u32 = 0;
        for &(lsb, width) in &self.segments {
            result <<= width;
            result |= (insn >> lsb) & ((1u32 << width) - 1);
        }
        if self.sext {
            let total_bits: u8 = self.segments.iter().map(|s| s.1).sum();
            let sign_bit = 1u32 << (total_bits - 1);
            if result & sign_bit != 0 {
                result | !((1u32 << total_bits) - 1)
            } else {
                result
            }
        } else {
            result
        }
    }
}

/// Parse a `%name pos:len [pos:len ...]` line.
pub fn parse_field_def(line: &str) -> Option<FieldDef> {
    let line = line.trim();
    if !line.starts_with('%') {
        return None;
    }
    let mut parts = line.split_whitespace();
    let name = parts.next()?.trim_start_matches('%').to_string();
    let mut segments = Vec::new();
    let mut sext = false;

    for token in parts {
        if token == "!function=..." {
            continue; // skip function annotations for now
        }
        if token.starts_with("s") && token.contains(':') {
            // Signed segment: s12:1 means sign-extend, pos 12, width 1
            sext = true;
            let token = &token[1..];
            let mut split = token.split(':');
            let pos: u8 = split.next()?.parse().ok()?;
            let len: u8 = split.next()?.parse().ok()?;
            segments.push((pos, len));
        } else if token.contains(':') {
            let mut split = token.split(':');
            let pos: u8 = split.next()?.parse().ok()?;
            let len: u8 = split.next()?.parse().ok()?;
            segments.push((pos, len));
        }
    }

    if segments.is_empty() {
        return None;
    }

    Some(FieldDef {
        name,
        segments,
        sext,
    })
}
