//! Typed quirk identifiers for platform and board integration.
//!
//! The initial scope is platform and board quirks. Additional scopes
//! (SoC, CPU core, device) can extend [`QuirkKey`] later without
//! changing how plans and runtime state store selected quirks.

use std::collections::BTreeSet;

/// Platform-level quirks that affect fixed wiring or address layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PlatformQuirk {
    /// `arm-virt` carries a PL031 RTC at `0x0901_0000`, wired to SPI 34.
    ArmVirtPl031Rtc,
}

impl PlatformQuirk {
    /// Stable string identifier for diagnostics and tooling.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ArmVirtPl031Rtc => "arm-virt-pl031-rtc",
        }
    }
}

/// Board-level quirks that affect runtime setup or boot behavior.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum BoardQuirk {
    /// Handle PSCI power-management calls in the engine.
    PsciViaEngine,
}

impl BoardQuirk {
    /// Stable string identifier for diagnostics and tooling.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::PsciViaEngine => "psci-via-engine",
        }
    }
}

/// One typed quirk key stored in plans and runtime state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum QuirkKey {
    /// Platform-scoped quirk.
    Platform(PlatformQuirk),
    /// Board-scoped quirk.
    Board(BoardQuirk),
}

impl QuirkKey {
    /// Quirk scope label for diagnostics.
    pub const fn scope(self) -> &'static str {
        match self {
            Self::Platform(_) => "platform",
            Self::Board(_) => "board",
        }
    }

    /// Stable string identifier for diagnostics and tooling.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Platform(quirk) => quirk.as_str(),
            Self::Board(quirk) => quirk.as_str(),
        }
    }
}

/// One quirk supported by a platform build plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QuirkSpec {
    /// Typed quirk identifier.
    pub key: QuirkKey,
    /// Short human-readable description.
    pub summary: &'static str,
    /// Whether the quirk is enabled in the platform's default selection.
    pub default_enabled: bool,
}

/// Selected quirks carried into runtime state.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct QuirkSet {
    enabled: BTreeSet<QuirkKey>,
}

impl QuirkSet {
    /// Create an empty quirk selection.
    pub fn new() -> Self {
        Self::default()
    }

    /// Build the default-enabled quirk set from plan metadata.
    pub fn from_specs(specs: &[QuirkSpec]) -> Self {
        let mut quirks = Self::default();
        for spec in specs {
            if spec.default_enabled {
                quirks.enable(spec.key);
            }
        }
        quirks
    }

    /// Enable one quirk.
    pub fn enable(&mut self, key: QuirkKey) -> bool {
        self.enabled.insert(key)
    }

    /// Disable one quirk.
    pub fn disable(&mut self, key: QuirkKey) -> bool {
        self.enabled.remove(&key)
    }

    /// Check whether a quirk is enabled.
    pub fn contains(&self, key: QuirkKey) -> bool {
        self.enabled.contains(&key)
    }

    /// Iterate over enabled quirks in a stable order.
    pub fn iter(&self) -> impl Iterator<Item = QuirkKey> + '_ {
        self.enabled.iter().copied()
    }

    /// Return the number of enabled quirks.
    pub fn len(&self) -> usize {
        self.enabled.len()
    }

    /// Return true when no quirks are enabled.
    pub fn is_empty(&self) -> bool {
        self.enabled.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_specs_collects_only_default_enabled_quirks() {
        let specs = [
            QuirkSpec {
                key: QuirkKey::Platform(PlatformQuirk::ArmVirtPl031Rtc),
                summary: "rtc",
                default_enabled: true,
            },
            QuirkSpec {
                key: QuirkKey::Board(BoardQuirk::PsciViaEngine),
                summary: "psci",
                default_enabled: false,
            },
        ];

        let quirks = QuirkSet::from_specs(&specs);

        assert!(quirks.contains(QuirkKey::Platform(PlatformQuirk::ArmVirtPl031Rtc)));
        assert!(!quirks.contains(QuirkKey::Board(BoardQuirk::PsciViaEngine)));
        assert_eq!(quirks.len(), 1);
    }

    #[test]
    fn enable_and_disable_update_membership() {
        let mut quirks = QuirkSet::new();
        let key = QuirkKey::Board(BoardQuirk::PsciViaEngine);

        assert!(quirks.enable(key));
        assert!(quirks.contains(key));
        assert!(quirks.disable(key));
        assert!(!quirks.contains(key));
    }
}
