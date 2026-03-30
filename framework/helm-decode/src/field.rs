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
    /// Create an unsigned bit field.
    pub fn new(name: impl Into<String>, lsb: u8, width: u8) -> Self {
        Self {
            name: name.into(),
            lsb,
            width,
            sext: false,
        }
    }

    /// Mark this field as sign-extended.
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

/// Post-processing transform applied to an extracted field value.
///
/// Corresponds to QEMU's `!function=name` annotations on `%field` definitions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldTransform {
    /// Left-shift result by 1 (branch/JAL offsets; always even).
    Shift1,
    /// Left-shift result by 2.
    Shift2,
    /// Left-shift result by 3.
    Shift3,
    /// Left-shift result by 4.
    Shift4,
    /// Left-shift result by 12 (U-type immediate).
    Shift12,
    /// Add 1 to result.
    PlusOne,
    /// Add 8 to result (RVC register mapping: x8–x15).
    RvcRegister,
    /// Add 8 or 16 depending on bit pattern (s0/s1 or s2).
    SregRegister,
}

impl FieldTransform {
    /// Parse a QEMU `!function=name` string into a transform variant.
    pub fn from_function_name(name: &str) -> Option<Self> {
        match name {
            "ex_shift_1" => Some(Self::Shift1),
            "ex_shift_2" => Some(Self::Shift2),
            "ex_shift_3" => Some(Self::Shift3),
            "ex_shift_4" => Some(Self::Shift4),
            "ex_shift_12" => Some(Self::Shift12),
            "ex_plus_1" => Some(Self::PlusOne),
            "ex_rvc_register" => Some(Self::RvcRegister),
            "ex_sreg_register" => Some(Self::SregRegister),
            _ => None,
        }
    }

    /// Emit the Rust expression that applies this transform to `val_expr`.
    pub fn emit_rust(&self, val_expr: &str) -> String {
        match self {
            Self::Shift1 => format!("({val_expr}) << 1"),
            Self::Shift2 => format!("({val_expr}) << 2"),
            Self::Shift3 => format!("({val_expr}) << 3"),
            Self::Shift4 => format!("({val_expr}) << 4"),
            Self::Shift12 => format!("({val_expr}) << 12"),
            Self::PlusOne => format!("({val_expr}).wrapping_add(1)"),
            Self::RvcRegister => format!("({val_expr}).wrapping_add(8)"),
            Self::SregRegister => format!("({val_expr}).wrapping_add(8)"),
        }
    }
}

/// A `%name` field definition line from a `.decode` file.
///
/// QEMU syntax: `%name pos:len [pos:len ...]`
///
/// Multi-segment fields are concatenated (e.g. split immediate).
#[derive(Debug, Clone)]
pub struct FieldDef {
    /// Field name (matches the `%name` declaration in the `.decode` file).
    pub name: String,
    /// Segments, from most-significant to least-significant in the
    /// concatenated result.
    pub segments: Vec<(u8, u8)>, // (lsb, width)
    /// If true, the concatenated value is sign-extended to 32 bits.
    pub sext: bool,
    /// Optional post-processing transform (`!function=name`).
    pub transform: Option<FieldTransform>,
}

impl FieldDef {
    /// Extract and concatenate all segments, apply sign-extension, then transform.
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
                result |= !((1u32 << total_bits) - 1);
            }
        }
        // Apply transform (operates on the sign-extended result)
        if let Some(t) = self.transform {
            result = match t {
                FieldTransform::Shift1 => result << 1,
                FieldTransform::Shift2 => result << 2,
                FieldTransform::Shift3 => result << 3,
                FieldTransform::Shift4 => result << 4,
                FieldTransform::Shift12 => result << 12,
                FieldTransform::PlusOne => result.wrapping_add(1),
                FieldTransform::RvcRegister => result.wrapping_add(8),
                FieldTransform::SregRegister => result.wrapping_add(8),
            };
        }
        result
    }

    /// Total bit width of this field (sum of all segment widths).
    pub fn total_width(&self) -> u8 {
        self.segments.iter().map(|s| s.1).sum()
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
    let mut transform = None;

    for token in parts {
        if let Some(fn_name) = token.strip_prefix("!function=") {
            transform = FieldTransform::from_function_name(fn_name);
            continue;
        }
        if token.starts_with('!') {
            continue; // unknown annotation, skip
        }
        if token.contains(':') {
            let t = token.trim_start_matches('s');
            let is_signed = token.starts_with('s') && t != token;
            if is_signed {
                sext = true;
            }
            let mut split = t.split(':');
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
        transform,
    })
}
