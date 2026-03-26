//! ARM core model table — maps core name to ID registers and feature bits.
//!
//! Usage:
//! ```no_run
//! use helm_arch::{Aarch64ArchState, ArmCoreModel};
//!
//! let model = ArmCoreModel::from_name("cortex-a55").unwrap_or_default();
//! let mut arch_state = Aarch64ArchState::new();
//! model.apply(&mut arch_state);
//! ```

use super::arch_state::Aarch64ArchState;

/// Known ARM core models.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ArmCoreModel {
    /// Generic ARMv8.0 baseline (Cortex-A53 compatible).
    #[default]
    Generic,
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
            "cortex-a53" | "cortexa53" | "a53" => Some(Self::Generic),
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

    /// Apply this core model's ID registers to `arch_state`.
    ///
    /// Sets MIDR_EL1, ID_AA64ISAR0/1_EL1, ID_AA64PFR0/1_EL1, ID_AA64MMFR0_EL1.
    pub fn apply(&self, a: &mut Aarch64ArchState) {
        match self {
            Self::Generic => {
                // ARMv8.0 baseline — Cortex-A53 compatible
                a.midr_el1 = 0x410F_D034; // Cortex-A53 r0p4
                // SHA1=1, SHA2=1, AES=1, CRC32=1, ATOMIC=0, RDM=0
                a.id_aa64isar0_el1 = 0x0000_0000_0001_1120;
                a.id_aa64isar1_el1 = 0x0000_0000_0000_0000;
                // EL0=AArch64, EL1=AArch64, EL2/EL3=not impl, FP+AdvSIMD=present, GIC=1 (GICv3 sysreg)
                a.id_aa64pfr0_el1 = 0x0000_0001_0000_0011;
                a.id_aa64pfr1_el1 = 0;
                // PARange=0 (32-bit PA), TGran4=0
                a.id_aa64mmfr0_el1 = 0x0000_0000_0000_1122;
            }
            Self::CortexA55 => {
                // Cortex-A55 r1p0 — ARMv8.2 + LRCPC + DotProd
                a.midr_el1 = 0x4110_D050; // r1p0
                // SHA1=1, SHA2=2 (512), AES=2, CRC32=1, ATOMIC=2, RDM=1, DP=1
                a.id_aa64isar0_el1 = 0x0000_0000_1011_1120;
                // LRCPC=1, DPB=1, JSCVT=1, FCMA=1
                a.id_aa64isar1_el1 = 0x0000_0000_0000_1111;
                // EL0=AArch64, EL1=AArch64, EL2/EL3=not impl, FP+AdvSIMD=present, GIC=1 (GICv3 sysreg)
                a.id_aa64pfr0_el1 = 0x0000_0001_0000_0011;
                a.id_aa64pfr1_el1 = 0;
                // PARange=5 (48-bit PA), TGran4=0
                a.id_aa64mmfr0_el1 = 0x0000_0000_0000_1125;
            }
            Self::CortexA73 => {
                // Cortex-A73 r0p2 — ARMv8.0 + CRC32 + Atomics
                a.midr_el1 = 0x4100_D092; // r0p2
                // SHA1=1, SHA2=1, AES=2, CRC32=1, ATOMIC=2
                a.id_aa64isar0_el1 = 0x0000_0000_0001_1122;
                a.id_aa64isar1_el1 = 0x0000_0000_0000_0000;
                // EL0=AArch64, EL1=AArch64, EL2/EL3=not impl, FP+AdvSIMD=present, GIC=1 (GICv3 sysreg)
                a.id_aa64pfr0_el1 = 0x0000_0001_0000_0011;
                a.id_aa64pfr1_el1 = 0;
                a.id_aa64mmfr0_el1 = 0x0000_0000_0000_1124;
            }
            Self::NeoverseN1 | Self::CortexA78 | Self::CortexX1 => {
                // ARMv8.4 — full v8.4 feature set
                let midr = match self {
                    Self::NeoverseN1 => 0x4100_D0C1, // Neoverse N1 r3p1
                    Self::CortexA78 => 0x4100_D410,  // Cortex-A78 r0p0
                    Self::CortexX1 => 0x4100_D440,   // Cortex-X1 r0p0
                    _ => unreachable!(),
                };
                a.midr_el1 = midr;
                // SHA1=1, SHA2=2, AES=2, CRC32=1, ATOMIC=2, RDM=1, DP=1, SM3=1, SM4=1
                a.id_aa64isar0_el1 = 0x0000_0000_1011_1120;
                // LRCPC=2, DPB=2, JSCVT=1, FCMA=1, SB=1
                a.id_aa64isar1_el1 = 0x0000_0000_0001_1212;
                // EL0=AArch64, EL1=AArch64, EL2/EL3=not impl, FP+AdvSIMD=present, GIC=1 (GICv3 sysreg)
                a.id_aa64pfr0_el1 = 0x0000_0001_0000_0011;
                // BT=1 (BTI), SSBS=1
                a.id_aa64pfr1_el1 = 0x0000_0000_0000_0011;
                // PARange=5, TGran4=0, TGran16=6
                a.id_aa64mmfr0_el1 = 0x0000_0000_0000_1125;
            }
            Self::CortexA510 | Self::CortexA710 => {
                // ARMv9.0 — simulated as ARMv8.5 feature set
                let midr = match self {
                    Self::CortexA510 => 0x4100_D460, // Cortex-A510 r0p0
                    Self::CortexA710 => 0x4100_D470, // Cortex-A710 r0p0
                    _ => unreachable!(),
                };
                a.midr_el1 = midr;
                a.id_aa64isar0_el1 = 0x0000_0000_1011_1120;
                a.id_aa64isar1_el1 = 0x0000_0000_0001_1212;
                // EL0=AArch64, EL1=AArch64, EL2/EL3=not impl, FP+AdvSIMD=present, GIC=1 (GICv3 sysreg)
                a.id_aa64pfr0_el1 = 0x0000_0001_0000_0011;
                a.id_aa64pfr1_el1 = 0x0000_0000_0000_0021; // BT=2 (BTI-c), SSBS=1
                a.id_aa64mmfr0_el1 = 0x0000_0000_0000_1125;
            }
        }
    }
}
