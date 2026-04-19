//! ARM core model table — maps core name to ID registers and feature bits.
//!
//! Usage:
//! ```no_run
//! use helm_arch::{Aarch64ArchState, ArmCoreModel};
//!
//! let models = ArmCoreModel::list_models();
//! let model = ArmCoreModel::from_name("cortex-a55").unwrap_or_default();
//! let mut arch_state = Aarch64ArchState::new();
//! model.apply(&mut arch_state);
//! ```

use super::arch_state::Aarch64ArchState;

const ID_AA64PFR0_GIC_SHIFT: u64 = 24;
const ID_AA64PFR0_GIC_MASK: u64 = 0xF << ID_AA64PFR0_GIC_SHIFT;
const ID_AA64PFR0_EL2_EL3_MASK: u64 = 0xFF00;

// ID_AA64ISAR1_EL1 pointer-authentication fields.
// We implement PAC as an identity function (PAC bits = 0, AUT = NOP),
// which is a valid IMPDEF algorithm. Advertise API=1 (IMP DEF address
// auth) and GPI=1 (IMP DEF generic auth) so the kernel enables PAC.
const ID_AA64ISAR1_APA_SHIFT: u64 = 4;
const ID_AA64ISAR1_API_SHIFT: u64 = 8;
const ID_AA64ISAR1_GPA_SHIFT: u64 = 24;
const ID_AA64ISAR1_GPI_SHIFT: u64 = 28;

fn set_pauth_impdef(isar1: u64) -> u64 {
    let mask = (0xF << ID_AA64ISAR1_APA_SHIFT)
        | (0xF << ID_AA64ISAR1_API_SHIFT)
        | (0xF << ID_AA64ISAR1_GPA_SHIFT)
        | (0xF << ID_AA64ISAR1_GPI_SHIFT);
    // APA=0 (no QARMA5), API=1 (IMP DEF), GPA=0, GPI=1 (IMP DEF)
    (isar1 & !mask) | (1 << ID_AA64ISAR1_API_SHIFT) | (1 << ID_AA64ISAR1_GPI_SHIFT)
}

/// Known ARM core models.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ArmCoreModel {
    /// Generic ARMv8.0 baseline.
    #[default]
    Generic,
    /// Cortex-A53 — ARMv8.0 in-order.
    CortexA53,
    /// Cortex-A55 — ARMv8.2+LRCPC+DotProd.
    CortexA55,
    /// Cortex-A73 — ARMv8.0+CRC32+Atomics.
    CortexA73,
    /// Neoverse N1 — ARMv8.4+all extensions.
    NeoverseN1,
    /// Cortex-A78 — ARMv8.4+all extensions.
    CortexA78,
    /// Cortex-X1 — ARMv8.4+all extensions, high-performance.
    CortexX1,
    /// Cortex-A510 — ARMv9.0 (treated as v8.5 for simulation).
    CortexA510,
    /// Cortex-A710 — ARMv9.0 (treated as v8.5 for simulation).
    CortexA710,
}

impl ArmCoreModel {
    /// Parse a core model name (case-insensitive).
    ///
    /// Accepts names like `"cortex-a55"`, `"neoverse-n1"`, `"generic"`, etc.
    pub fn from_name(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "generic" | "arm" => Some(Self::Generic),
            "cortex-a53" | "cortexa53" | "a53" => Some(Self::CortexA53),
            "cortex-a55" | "cortexa55" | "a55" => Some(Self::CortexA55),
            "cortex-a73" | "cortexa73" | "a73" => Some(Self::CortexA73),
            "neoverse-n1" | "neoversen1" | "n1" => Some(Self::NeoverseN1),
            "cortex-a78" | "cortexa78" | "a78" => Some(Self::CortexA78),
            "cortex-x1" | "cortexx1" | "x1" => Some(Self::CortexX1),
            "cortex-a510" | "cortexa510" | "a510" => Some(Self::CortexA510),
            "cortex-a710" | "cortexa710" | "a710" => Some(Self::CortexA710),
            _ => None,
        }
    }

    /// Canonical CLI name for this model.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Generic => "generic",
            Self::CortexA53 => "cortex-a53",
            Self::CortexA55 => "cortex-a55",
            Self::CortexA73 => "cortex-a73",
            Self::NeoverseN1 => "neoverse-n1",
            Self::CortexA78 => "cortex-a78",
            Self::CortexX1 => "cortex-x1",
            Self::CortexA510 => "cortex-a510",
            Self::CortexA710 => "cortex-a710",
        }
    }

    /// Human-readable description.
    pub fn description(&self) -> &'static str {
        match self {
            Self::Generic => "ARMv8.0 baseline (generic)",
            Self::CortexA53 => "Cortex-A53 in-order (ARMv8.0)",
            Self::CortexA55 => "Cortex-A55 in-order (ARMv8.2)",
            Self::CortexA73 => "Cortex-A73 OoO (ARMv8.0)",
            Self::NeoverseN1 => "Neoverse N1 server-class OoO (ARMv8.2)",
            Self::CortexA78 => "Cortex-A78 OoO (ARMv8.2)",
            Self::CortexX1 => "Cortex-X1 high-perf OoO (ARMv8.2)",
            Self::CortexA510 => "Cortex-A510 in-order (ARMv9.0)",
            Self::CortexA710 => "Cortex-A710 OoO (ARMv9.0)",
        }
    }

    /// All known models as `(name, description)` pairs, suitable for `--cpu help`.
    pub fn list_models() -> Vec<(&'static str, &'static str)> {
        [
            Self::Generic,
            Self::CortexA53,
            Self::CortexA55,
            Self::CortexA73,
            Self::NeoverseN1,
            Self::CortexA78,
            Self::CortexX1,
            Self::CortexA510,
            Self::CortexA710,
        ]
        .iter()
        .map(|m| (m.name(), m.description()))
        .collect()
    }

    /// Apply this core model's ID registers to `arch_state`.
    ///
    /// Sets MIDR_EL1, ID_AA64ISAR0/1_EL1, ID_AA64PFR0/1_EL1, ID_AA64MMFR0_EL1.
    pub fn apply(&self, a: &mut Aarch64ArchState) {
        let existing_gic = a.id_aa64pfr0_el1 & ID_AA64PFR0_GIC_MASK;
        let existing_el2_el3 = a.id_aa64pfr0_el1 & ID_AA64PFR0_EL2_EL3_MASK;
        match self {
            Self::Generic => {
                // ARMv8.0 baseline — non-ARM implementer to avoid Spectre MIDR matching
                a.midr_el1 = 0x480F_D034; // implementer=0x48 ('H'), part=A53
                                          // ATOMIC=0 — pure baseline, no LSE
                a.id_aa64isar0_el1 = 0x0000_0000_0001_1120;
                a.id_aa64isar1_el1 = 0x0000_0000_0000_0000;
                // EL0=AArch64, EL1=AArch64, FP+AdvSIMD=present, GIC=1
                a.id_aa64pfr0_el1 = 0x0000_0000_0100_0011;
                a.id_aa64pfr1_el1 = 0;
                // PARange=5 (48-bit PA) so Linux 48-bit VA kernels work
                a.id_aa64mmfr0_el1 = 0x0000_0000_0000_1125;
            }
            Self::CortexA53 => {
                // Cortex-A53 feature set (ARMv8.0) — non-ARM implementer
                a.midr_el1 = 0x480F_D034;
                // CRC32=1, ATOMIC=2 (simulator implements LSE)
                a.id_aa64isar0_el1 = 0x0000_0000_0002_1120;
                a.id_aa64isar1_el1 = 0x0000_0000_0000_0000;
                // EL0=AArch64, EL1=AArch64, FP+AdvSIMD=present, GIC=1
                a.id_aa64pfr0_el1 = 0x0000_0000_0100_0011;
                a.id_aa64pfr1_el1 = 0;
                // PARange=5 (48-bit PA) so Linux 48-bit VA kernels work
                a.id_aa64mmfr0_el1 = 0x0000_0000_0000_1125;
            }
            Self::CortexA55 => {
                // Cortex-A55 feature set (ARMv8.2 + LRCPC + DotProd) with a
                // non-ARM implementer so Linux doesn't match our MIDR against
                // its Spectre-BHB vulnerability table. The simulator has no
                // speculative execution and cannot suffer BHB attacks.
                a.midr_el1 = 0x4810_D050; // implementer=0x48 ('H'), part=A55
                                          // SHA1=1, SHA2=2 (512), AES=2, CRC32=1, ATOMIC=2, RDM=1, DP=1
                a.id_aa64isar0_el1 = 0x0000_0000_1011_1120;
                // LRCPC=1, DPB=1, JSCVT=1, FCMA=1
                a.id_aa64isar1_el1 = 0x0000_0000_0000_1111;
                // EL0=AArch64, EL1=AArch64, EL2/EL3=not impl, FP+AdvSIMD=present, GIC=1 (GICv3 sysreg)
                a.id_aa64pfr0_el1 = 0x0000_0000_0100_0011;
                a.id_aa64pfr1_el1 = 0;
                // PARange=5 (48-bit PA), TGran4=0
                a.id_aa64mmfr0_el1 = 0x0000_0000_0000_1125;
            }
            Self::CortexA73 => {
                // Cortex-A73 r0p2 — ARMv8.0 + CRC32 + Atomics
                a.midr_el1 = 0x4800_D092; // non-ARM implementer
                                          // SHA1=1, SHA2=1, AES=2, CRC32=1, ATOMIC=2
                a.id_aa64isar0_el1 = 0x0000_0000_0001_1122;
                a.id_aa64isar1_el1 = 0x0000_0000_0000_0000;
                // EL0=AArch64, EL1=AArch64, EL2/EL3=not impl, FP+AdvSIMD=present, GIC=1 (GICv3 sysreg)
                a.id_aa64pfr0_el1 = 0x0000_0000_0100_0011;
                a.id_aa64pfr1_el1 = 0;
                a.id_aa64mmfr0_el1 = 0x0000_0000_0000_1124;
            }
            Self::NeoverseN1 | Self::CortexA78 | Self::CortexX1 => {
                // ARMv8.4 — full v8.4 feature set
                let midr = match self {
                    Self::NeoverseN1 => 0x4800_D0C1, // non-ARM implementer
                    Self::CortexA78 => 0x4800_D410,
                    Self::CortexX1 => 0x4800_D440,
                    _ => unreachable!(),
                };
                a.midr_el1 = midr;
                // SHA1=1, SHA2=2, AES=2, CRC32=1, ATOMIC=2, RDM=1, DP=1, SM3=1, SM4=1
                a.id_aa64isar0_el1 = 0x0000_0000_1011_1120;
                // LRCPC=2, DPB=2, JSCVT=1, FCMA=1, SB=1
                a.id_aa64isar1_el1 = 0x0000_0000_0001_1212;
                // EL0=AArch64, EL1=AArch64, EL2/EL3=not impl, FP+AdvSIMD=present, GIC=1 (GICv3 sysreg)
                a.id_aa64pfr0_el1 = 0x0000_0000_0100_0011;
                // BT=1 (BTI), SSBS=1
                a.id_aa64pfr1_el1 = 0x0000_0000_0000_0011;
                // PARange=5, TGran4=0, TGran16=6
                a.id_aa64mmfr0_el1 = 0x0000_0000_0000_1125;
            }
            Self::CortexA510 | Self::CortexA710 => {
                // ARMv9.0 — simulated as ARMv8.5 feature set
                let midr = match self {
                    Self::CortexA510 => 0x4800_D460, // non-ARM implementer
                    Self::CortexA710 => 0x4800_D470,
                    _ => unreachable!(),
                };
                a.midr_el1 = midr;
                a.id_aa64isar0_el1 = 0x0000_0000_1011_1120;
                a.id_aa64isar1_el1 = 0x0000_0000_0001_1212;
                // EL0=AArch64, EL1=AArch64, EL2/EL3=not impl, FP+AdvSIMD=present, GIC=1 (GICv3 sysreg)
                a.id_aa64pfr0_el1 = 0x0000_0000_0100_0011;
                a.id_aa64pfr1_el1 = 0x0000_0000_0000_0012; // BT=2 (BTI-c), SSBS=1
                a.id_aa64mmfr0_el1 = 0x0000_0000_0000_1125;
            }
        }
        a.id_aa64pfr0_el1 = (a.id_aa64pfr0_el1
            & !(ID_AA64PFR0_GIC_MASK | ID_AA64PFR0_EL2_EL3_MASK))
            | existing_gic
            | existing_el2_el3;
        // CSV2=2, CSV3=1 (PFR0 bits [59:56] and [63:60]): this simulator has
        // no speculative execution, so Spectre-v2/v3 are impossible.
        a.id_aa64pfr0_el1 = (a.id_aa64pfr0_el1 & !(0xFFu64 << 56)) | (2u64 << 56) | (1u64 << 60);
        // ECBHB=1 (MMFR1 bits [63:60]): advertise hardware BHB clearing so
        // the kernel skips the software BHB mitigation that patches exception
        // vectors with trampoline branches. Those trampolines compute offsets
        // relative to the real CPU's vector layout and produce incorrect
        // branches on our functional-only simulator.
        a.id_aa64mmfr1_el1 |= 1u64 << 60;
        a.id_aa64isar1_el1 = set_pauth_impdef(a.id_aa64isar1_el1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID_AA64ISAR1_FEAT_NONE: u64 = 0;
    const ID_AA64ISAR1_FEAT_IMPLDEF: u64 = 1;

    fn nibble(val: u64, shift: u64) -> u64 {
        (val >> shift) & 0xF
    }

    #[test]
    fn apply_preserves_existing_gic_field() {
        let mut a = Aarch64ArchState::new();
        a.id_aa64pfr0_el1 = 1 << ID_AA64PFR0_GIC_SHIFT;

        ArmCoreModel::CortexA55.apply(&mut a);

        assert_eq!(nibble(a.id_aa64pfr0_el1, ID_AA64PFR0_GIC_SHIFT), 1);
    }

    #[test]
    fn apply_preserves_existing_el2_field() {
        let mut a = Aarch64ArchState::new();
        a.id_aa64pfr0_el1 = 1 << 8;

        ArmCoreModel::CortexA55.apply(&mut a);

        assert_eq!(nibble(a.id_aa64pfr0_el1, 8), 1);
    }

    #[test]
    fn apply_programs_impdef_pauth_fields_for_all_models() {
        let models = [
            ArmCoreModel::Generic,
            ArmCoreModel::CortexA53,
            ArmCoreModel::CortexA55,
            ArmCoreModel::CortexA73,
            ArmCoreModel::NeoverseN1,
            ArmCoreModel::CortexA78,
            ArmCoreModel::CortexX1,
            ArmCoreModel::CortexA510,
            ArmCoreModel::CortexA710,
        ];

        for model in models {
            let mut a = Aarch64ArchState::new();
            model.apply(&mut a);

            assert_eq!(
                nibble(a.id_aa64isar1_el1, ID_AA64ISAR1_APA_SHIFT),
                ID_AA64ISAR1_FEAT_NONE
            );
            assert_eq!(
                nibble(a.id_aa64isar1_el1, ID_AA64ISAR1_API_SHIFT),
                ID_AA64ISAR1_FEAT_IMPLDEF
            );
            assert_eq!(
                nibble(a.id_aa64isar1_el1, ID_AA64ISAR1_GPA_SHIFT),
                ID_AA64ISAR1_FEAT_NONE
            );
            assert_eq!(
                nibble(a.id_aa64isar1_el1, ID_AA64ISAR1_GPI_SHIFT),
                ID_AA64ISAR1_FEAT_IMPLDEF
            );
        }
    }
}
