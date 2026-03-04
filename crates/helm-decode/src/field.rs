//! Bit-field extraction from a 32-bit instruction word.

/// A named bit-field within an instruction encoding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BitField {
    /// Field name (e.g. `"rd"`, `"imm12"`, `"sf"`).
    pub name: String,
    /// Least-significant bit position in the instruction word.
    pub lsb: u8,
    /// Width in bits.
    pub width: u8,
}

impl BitField {
    pub fn new(name: impl Into<String>, lsb: u8, width: u8) -> Self {
        Self {
            name: name.into(),
            lsb,
            width,
        }
    }

    /// Extract the field value from a 32-bit instruction.
    pub fn extract(&self, insn: u32) -> u32 {
        (insn >> self.lsb) & ((1u32 << self.width) - 1)
    }

    /// Mask covering this field's bit positions.
    pub fn mask(&self) -> u32 {
        ((1u32 << self.width) - 1) << self.lsb
    }
}
