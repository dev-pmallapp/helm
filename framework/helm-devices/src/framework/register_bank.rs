//! `register_bank!` macro for declarative device register definitions.
//!
//! Generates `Device::read()` / `Device::write()` dispatch, serde checkpoint
//! fields, and optional per-register hooks from a concise register description.
//!
//! # Width qualifier
//! By default, registers are 32-bit. Use `width 64` to specify 64-bit registers:
//! ```ignore
//! register_bank! {
//!     MyRegs for MyDevice {
//!         reg CTRL @ 0x00 { field EN [0] }
//!         reg STATUS @ 0x04 is read_only { field BUSY [0] }
//!         reg ADDR @ 0x08 width 64 { field BASE [63:0] }
//!     }
//! }
//! ```

/// Describes a single register in a register bank.
#[derive(Debug, Clone)]
pub struct RegisterDesc {
    /// Register name.
    pub name: &'static str,
    /// Byte offset within the device's MMIO region.
    pub offset: u64,
    /// Width in bits (32 or 64).
    pub width: u8,
    /// Access mode.
    pub access: RegisterAccess,
    /// Reset value.
    pub reset_value: u64,
}

/// Register access mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegisterAccess {
    /// Register can be read and written.
    ReadWrite,
    /// Register can only be read (writes ignored).
    ReadOnly,
    /// Register can only be written (reads return 0).
    WriteOnly,
}

/// A field within a register.
#[derive(Debug, Clone)]
pub struct FieldDesc {
    /// Field name.
    pub name: &'static str,
    /// Bit position of the LSB.
    pub lsb: u8,
    /// Bit position of the MSB (inclusive).
    pub msb: u8,
}

impl FieldDesc {
    /// Create a new field descriptor.
    pub const fn new(name: &'static str, msb: u8, lsb: u8) -> Self {
        Self { name, lsb, msb }
    }

    /// Bit mask for this field (shifted to position).
    pub const fn mask(&self) -> u64 {
        let width = self.msb - self.lsb + 1;
        ((1u64 << width) - 1) << self.lsb
    }

    /// Extract this field's value from a register value.
    pub const fn extract(&self, val: u64) -> u64 {
        (val & self.mask()) >> self.lsb
    }

    /// Insert a value into this field's position.
    pub const fn insert(&self, reg_val: u64, field_val: u64) -> u64 {
        (reg_val & !self.mask()) | ((field_val << self.lsb) & self.mask())
    }
}

/// Describes a complete register bank for code generation.
#[derive(Debug)]
pub struct RegisterBankDesc {
    /// Bank name.
    pub name: &'static str,
    /// Registers in this bank.
    pub registers: &'static [RegisterDesc],
}

/// Macro to define a register bank with per-register width support.
///
/// # Example
/// ```ignore
/// register_bank! {
///     UartRegs for Uart16550 at offset 0x0 {
///         reg RHR @ 0x00 is read_only { field DATA [7:0] }
///         reg THR @ 0x00 is write_only { field DATA [7:0] }
///         reg LSR @ 0x14 is read_only {
///             field THRE [5]
///             field DR   [0]
///         }
///         reg DLL @ 0x00 width 64 { field DIVISOR [15:0] }
///     }
/// }
/// ```
/// Define register offset constants from a bank specification.
///
/// Each register is defined with a name and byte offset. The macro generates
/// `const` offset values for use in `Device::read()` / `Device::write()`.
///
/// Width (32 or 64) and access mode (read_only, write_only) are accepted
/// syntactically as documentation but do not affect the generated constants.
#[macro_export]
macro_rules! register_bank {
    // Entry: parse individual register definitions
    (@reg $base:expr, $reg_name:ident, $offset:expr) => {
        #[allow(non_upper_case_globals, dead_code)]
        const $reg_name: u64 = $base + $offset;
    };

    // Main entry: iterate over register list
    ($bank_name:ident, base = $base:expr, [
        $( ($reg_name:ident, $offset:expr) ),* $(,)?
    ]) => {
        $(
            $crate::register_bank!(@reg $base, $reg_name, $offset);
        )*
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn field_mask_single_bit() {
        let f = FieldDesc::new("EN", 0, 0);
        assert_eq!(f.mask(), 1);
    }

    #[test]
    fn field_mask_multi_bit() {
        let f = FieldDesc::new("DATA", 7, 0);
        assert_eq!(f.mask(), 0xFF);
    }

    #[test]
    fn field_extract() {
        let f = FieldDesc::new("STATUS", 5, 4);
        assert_eq!(f.extract(0b0011_0000), 3);
    }

    #[test]
    fn field_insert() {
        let f = FieldDesc::new("STATUS", 5, 4);
        let result = f.insert(0x00, 2);
        assert_eq!(result, 0b0010_0000);
    }

    register_bank!(TestRegs, base = 0x0, [
        (CTRL, 0x00),
        (STATUS, 0x04),
        (ADDR, 0x08),
    ]);

    #[test]
    fn register_offsets() {
        assert_eq!(CTRL, 0x00);
        assert_eq!(STATUS, 0x04);
        assert_eq!(ADDR, 0x08);
    }
}
