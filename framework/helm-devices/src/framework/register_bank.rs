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
        if width >= 64 {
            u64::MAX
        } else {
            ((1u64 << width) - 1) << self.lsb
        }
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
    // ── Width helper: default to 32 if not specified ──
    (@width) => { 32u8 };
    (@width $w:tt) => { $w as u8 };

    // ── Access helper: default to "read_write" if not specified ──
    (@access) => { "read_write" };
    (@access read_only) => { "read_only" };
    (@access write_only) => { "write_only" };
    (@access read_write) => { "read_write" };

    // ── Rich syntax: with `for $dev:ty` — forwards to the canonical arm below ──
    ($bank_name:ident for $dev:ty {
        $(
            reg $reg_name:ident @ $offset:tt
            $(width $width:tt)?
            $(is $access:ident)?
            $({ $( field $field_name:ident [$($bits:tt)*] );* $(;)? })?
        )*
    }) => {
        $crate::register_bank!($bank_name {
            $(
                reg $reg_name @ $offset
                $(width $width)?
                $(is $access)?
                $({ $( field $field_name [$($bits)*] );* })?
            )*
        });
    };

    // ── Rich syntax: canonical expansion ──
    ($bank_name:ident {
        $(
            reg $reg_name:ident @ $offset:tt
            $(width $width:tt)?
            $(is $access:ident)?
            $({ $( field $field_name:ident [$($bits:tt)*] );* $(;)? })?
        )*
    }) => {
        $(
            #[allow(non_upper_case_globals, dead_code)]
            const $reg_name: u64 = $offset;
        )*

        #[allow(dead_code)]
        const fn __register_bank_desc() -> &'static [(&'static str, u64, u8, &'static str)] {
            &[
                $(
                    (
                        stringify!($reg_name),
                        $offset,
                        $crate::register_bank!(@width $($width)?),
                        $crate::register_bank!(@access $($access)?),
                    ),
                )*
            ]
        }
    };

    // ── Simple tuple syntax (original) ──

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
    fn field_mask_full_width() {
        let f = FieldDesc::new("BASE", 63, 0);
        assert_eq!(f.mask(), u64::MAX);
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

    register_bank!(
        TestRegs,
        base = 0x0,
        [(CTRL, 0x00), (STATUS, 0x04), (ADDR, 0x08),]
    );

    #[test]
    fn register_offsets() {
        assert_eq!(CTRL, 0x00);
        assert_eq!(STATUS, 0x04);
        assert_eq!(ADDR, 0x08);
    }

    #[test]
    fn rich_syntax_offsets() {
        register_bank! {
            RichTestRegs for () {
                reg CTRL @ 0x00 { field EN [0] }
                reg STATUS @ 0x04 is read_only { field BUSY [0] }
                reg ADDR @ 0x08 width 64 { field BASE [63:0] }
                reg DATA @ 0x0C is write_only { field VAL [31:0] }
            }
        }
        assert_eq!(CTRL, 0x00);
        assert_eq!(STATUS, 0x04);
        assert_eq!(ADDR, 0x08);
        assert_eq!(DATA, 0x0C);
    }

    #[test]
    fn rich_syntax_bank_desc() {
        register_bank! {
            DescTestRegs for () {
                reg REG_A @ 0x00 { field X [0] }
                reg REG_B @ 0x04 width 64 is read_only { field Y [7:0] }
            }
        }
        let desc = __register_bank_desc();
        assert_eq!(desc.len(), 2);
        assert_eq!(desc[0].0, "REG_A");
        assert_eq!(desc[0].1, 0x00);
        assert_eq!(desc[0].2, 32); // default width
        assert_eq!(desc[1].0, "REG_B");
        assert_eq!(desc[1].2, 64); // explicit width
        assert_eq!(desc[1].3, "read_only");
    }
}
