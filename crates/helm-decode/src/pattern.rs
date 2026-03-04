//! Decode pattern: a fixed-bit mask + value, plus named fields.

use super::field::BitField;

/// A single decoded instruction pattern line from a `.decode` file.
#[derive(Debug, Clone)]
pub struct DecodeLine {
    /// Mnemonic (e.g. `"ADD_imm"`).
    pub mnemonic: String,
    /// The pattern that matches this instruction.
    pub pattern: DecodePattern,
}

/// Fixed-bit mask/value pair plus extracted fields.
///
/// An instruction matches when `(insn & mask) == value`.
#[derive(Debug, Clone)]
pub struct DecodePattern {
    /// Bits that must be fixed (1 in mask).
    pub mask: u32,
    /// Expected value of the fixed bits.
    pub value: u32,
    /// Named variable fields.
    pub fields: Vec<BitField>,
}

impl DecodePattern {
    /// Test whether an instruction word matches this pattern.
    pub fn matches(&self, insn: u32) -> bool {
        (insn & self.mask) == self.value
    }

    /// Extract all fields from a matching instruction.
    pub fn extract_fields(&self, insn: u32) -> Vec<(&str, u32)> {
        self.fields
            .iter()
            .map(|f| (f.name.as_str(), f.extract(insn)))
            .collect()
    }
}

/// Parse a single decode-tree line into a `DecodeLine`.
///
/// Format: `MNEMONIC token token ...`
/// where each token is either:
/// - `0` or `1` — a fixed bit
/// - `.` — don't-care bit
/// - `name:N` — an N-bit field
///
/// Bits are written MSB-first and must total exactly 32.
pub fn parse_decode_line(line: &str) -> Option<DecodeLine> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return None;
    }

    let mut parts = line.split_whitespace();
    let mnemonic = parts.next()?.to_string();

    let mut mask: u32 = 0;
    let mut value: u32 = 0;
    let mut fields = Vec::new();
    let mut bit_pos: i32 = 31; // current MSB position (count down)

    for token in parts {
        if token.contains(':') {
            // Field: "name:width"
            let mut split = token.split(':');
            let name = split.next().unwrap().to_string();
            let width: u8 = split.next().unwrap().parse().ok()?;
            let lsb = (bit_pos - width as i32 + 1) as u8;
            fields.push(BitField::new(name, lsb, width));
            // Don't-care bits: mask stays 0 for these positions.
            bit_pos -= width as i32;
        } else {
            // Fixed bits: each char is '0', '1', or '.'
            for ch in token.chars() {
                match ch {
                    '0' => {
                        mask |= 1u32 << bit_pos as u32;
                        // value bit stays 0
                        bit_pos -= 1;
                    }
                    '1' => {
                        mask |= 1u32 << bit_pos as u32;
                        value |= 1u32 << bit_pos as u32;
                        bit_pos -= 1;
                    }
                    '.' => {
                        // Don't care
                        bit_pos -= 1;
                    }
                    '_' | ' ' => {} // cosmetic separator, ignore
                    _ => return None,
                }
            }
        }
    }

    if bit_pos != -1 {
        // Did not consume exactly 32 bits.
        return None;
    }

    Some(DecodeLine {
        mnemonic,
        pattern: DecodePattern {
            mask,
            value,
            fields,
        },
    })
}
