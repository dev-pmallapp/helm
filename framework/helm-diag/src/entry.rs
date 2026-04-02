// src/entry.rs — DiagLevel enum and DiagEntry struct

use std::fmt::Write as FmtWrite;

/// Severity level of a diagnostic entry.
///
/// Ordered from lowest to highest severity:
/// `Info < Stub < Warn < Error`
///
/// Note: `Branch` is intentionally absent. Branch events are emitted through
/// `probe!(probes.branch, BranchEvent { ... })` at Layer 1 (helm-probe), not
/// through the diagnostic channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum DiagLevel {
    /// Normal informational message (loader, boot progress).
    Info,
    /// Unimplemented feature -- stub was executed, returned a default value.
    Stub,
    /// Something unexpected but recoverable.
    Warn,
    /// Fatal or hard error (unhandled trap, assertion failure, unrecoverable state).
    Error,
}

impl DiagLevel {
    /// Four-character tag used in formatted output. Space-padded to 4 chars.
    pub fn as_tag(self) -> &'static str {
        match self {
            DiagLevel::Info => "INFO",
            DiagLevel::Stub => "STUB",
            DiagLevel::Warn => "WARN",
            DiagLevel::Error => "ERR ",
        }
    }
}

/// Per-thread simulation context updated by the engine before each instruction step.
///
/// The engine calls [`update_sim_ctx`](crate::update_sim_ctx) once per step (or
/// once per quantum for bulk-step modes) to keep the context approximately accurate.
#[derive(Clone, Copy, Default)]
pub struct DiagContext {
    /// Simulated nanoseconds since simulation start.
    pub sim_ns: u64,
    /// Total instructions retired since simulation start.
    pub sim_insns: u64,
}

/// One structured diagnostic record emitted by the simulator.
///
/// # Wire format
/// ```text
/// [STUB] sim_ns=000001234 insns=000025750 gicv2-dist       pc=0x0000000040201234 | MRS ID_AA64MMFR4_EL1 -> 0
/// ```
#[derive(Debug, Clone)]
pub struct DiagEntry {
    /// Simulated nanoseconds elapsed (derived from `insns * 1_000_000_000 / freq_hz`).
    pub sim_ns: u64,
    /// Total instructions retired at the time of the message.
    pub sim_insns: u64,
    /// Short, stable identifier for the emitting component (e.g. `"gicv2-dist"`).
    /// Must be `'static` so no allocation is required at the call site.
    pub component: &'static str,
    /// Severity level.
    pub level: DiagLevel,
    /// Guest program counter, if known at the call site.
    pub pc: Option<u64>,
    /// Free-form human-readable message string.
    pub message: String,
}

impl DiagEntry {
    /// Format into the canonical single-line representation.
    ///
    /// The format is stable -- downstream log parsers may rely on it.
    ///
    /// Column layout:
    /// - `[LEVL]`              -- 6 chars (bracket + 4-char tag + bracket)
    /// - `sim_ns=NNNNNNNNNNNN` -- 18 chars (label + 12-digit number)
    /// - `insns=NNNNNNNNNNNN`  -- 18 chars
    /// - `<component>`         -- left-justified, padded to 16 chars
    /// - `pc=0xHHHHHHHHHHHHHHHH` or `pc=?                 ` -- 20 chars
    /// - `| <message>`
    pub fn format(&self) -> String {
        let mut s = String::with_capacity(128);
        let pc_str = match self.pc {
            Some(p) => format!("{p:#018x}"),
            None => "?                 ".to_string(),
        };
        let _ = write!(
            s,
            "[{}] sim_ns={:012} insns={:012} {:<16} pc={} | {}",
            self.level.as_tag(),
            self.sim_ns,
            self.sim_insns,
            self.component,
            pc_str,
            self.message,
        );
        s
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod level_tests {
    use super::DiagLevel;

    // T-LEVEL-01
    /// Ordering: Info < Stub < Warn < Error.
    #[test]
    fn level_ordering_is_correct() {
        assert!(DiagLevel::Info < DiagLevel::Stub);
        assert!(DiagLevel::Stub < DiagLevel::Warn);
        assert!(DiagLevel::Warn < DiagLevel::Error);
        assert!(DiagLevel::Info < DiagLevel::Error);
    }

    // T-LEVEL-02
    /// DiagLevel::Info is the minimum (sentinel for "pass all").
    #[test]
    fn info_is_minimum_level() {
        for &lvl in &[
            DiagLevel::Info,
            DiagLevel::Stub,
            DiagLevel::Warn,
            DiagLevel::Error,
        ] {
            assert!(lvl >= DiagLevel::Info, "{lvl:?} must be >= Info");
        }
    }

    // T-LEVEL-03
    /// as_tag() returns the correct 4-character string for every variant.
    #[test]
    fn as_tag_returns_correct_strings() {
        assert_eq!(DiagLevel::Info.as_tag(), "INFO");
        assert_eq!(DiagLevel::Stub.as_tag(), "STUB");
        assert_eq!(DiagLevel::Warn.as_tag(), "WARN");
        assert_eq!(DiagLevel::Error.as_tag(), "ERR ");
    }

    // T-LEVEL-04
    /// All as_tag() strings are exactly 4 characters.
    #[test]
    fn as_tag_is_always_four_chars() {
        for &lvl in &[
            DiagLevel::Info,
            DiagLevel::Stub,
            DiagLevel::Warn,
            DiagLevel::Error,
        ] {
            assert_eq!(lvl.as_tag().len(), 4, "{lvl:?}.as_tag() must be 4 chars");
        }
    }

    // T-LEVEL-05
    /// DiagLevel derives Clone and Copy -- can be used by value.
    #[test]
    fn level_is_copy() {
        let a = DiagLevel::Warn;
        let b = a; // copy
        let c = a; // copy again
        assert_eq!(b, c);
    }

    // T-LEVEL-06
    /// There are exactly four DiagLevel variants (no Branch).
    #[test]
    fn diaglevel_has_four_variants() {
        let count = [
            DiagLevel::Info,
            DiagLevel::Stub,
            DiagLevel::Warn,
            DiagLevel::Error,
        ]
        .len();
        assert_eq!(count, 4);
    }
}

#[cfg(test)]
mod entry_tests {
    use super::{DiagEntry, DiagLevel};

    fn make(level: DiagLevel, component: &'static str, pc: Option<u64>, msg: &str) -> DiagEntry {
        DiagEntry {
            sim_ns: 1234,
            sim_insns: 5678,
            component,
            level,
            pc,
            message: msg.to_string(),
        }
    }

    // T-ENTRY-01
    /// format() starts with the correct level tag in brackets.
    #[test]
    fn format_starts_with_level_tag() {
        assert!(make(DiagLevel::Info, "c", None, "m")
            .format()
            .starts_with("[INFO]"));
        assert!(make(DiagLevel::Stub, "c", None, "m")
            .format()
            .starts_with("[STUB]"));
        assert!(make(DiagLevel::Warn, "c", None, "m")
            .format()
            .starts_with("[WARN]"));
        assert!(make(DiagLevel::Error, "c", None, "m")
            .format()
            .starts_with("[ERR ]"));
    }

    // T-ENTRY-02
    /// format() contains sim_ns zero-padded to 12 digits.
    #[test]
    fn format_contains_sim_ns_zero_padded() {
        let entry = make(DiagLevel::Info, "c", None, "m");
        assert!(
            entry.format().contains("sim_ns=000000001234"),
            "got: {}",
            entry.format()
        );
    }

    // T-ENTRY-03
    /// format() contains sim_insns zero-padded to 12 digits.
    #[test]
    fn format_contains_sim_insns_zero_padded() {
        let entry = make(DiagLevel::Info, "c", None, "m");
        assert!(
            entry.format().contains("insns=000000005678"),
            "got: {}",
            entry.format()
        );
    }

    // T-ENTRY-04
    /// format() contains the component string.
    #[test]
    fn format_contains_component() {
        let entry = make(DiagLevel::Stub, "gicv2-dist", None, "msg");
        assert!(
            entry.format().contains("gicv2-dist"),
            "got: {}",
            entry.format()
        );
    }

    // T-ENTRY-05
    /// format() with pc=Some(addr) renders the address as 0x-prefixed 18-char hex.
    #[test]
    fn format_pc_some_renders_hex() {
        let entry = make(DiagLevel::Stub, "c", Some(0x4020_1234), "m");
        let s = entry.format();
        assert!(s.contains("pc=0x0000000040201234"), "got: {s}");
    }

    // T-ENTRY-06
    /// format() with pc=None renders "pc=?".
    #[test]
    fn format_pc_none_renders_question_mark() {
        let entry = make(DiagLevel::Info, "c", None, "m");
        let s = entry.format();
        assert!(s.contains("pc=?"), "got: {s}");
    }

    // T-ENTRY-07
    /// format() contains the message after the " | " separator.
    #[test]
    fn format_contains_message_after_separator() {
        let entry = make(DiagLevel::Warn, "c", None, "write to read-only reg");
        let s = entry.format();
        assert!(s.contains("| write to read-only reg"), "got: {s}");
    }

    // T-ENTRY-08
    /// format() output is a single line (no embedded newlines).
    #[test]
    fn format_is_single_line() {
        let entry = make(DiagLevel::Info, "c", None, "no newlines here");
        assert!(
            !entry.format().contains('\n'),
            "format must not contain newlines"
        );
    }

    // T-ENTRY-09
    /// DiagEntry derives Clone -- can be sent through the channel.
    #[test]
    fn entry_is_clone() {
        let entry = make(DiagLevel::Stub, "test", Some(0x1000), "hello");
        let clone = entry.clone();
        assert_eq!(entry.format(), clone.format());
    }

    // T-ENTRY-10
    /// sim_ns = 0 and sim_insns = 0 renders as twelve zeros each.
    #[test]
    fn format_zero_timestamps() {
        let entry = DiagEntry {
            sim_ns: 0,
            sim_insns: 0,
            component: "c",
            level: DiagLevel::Info,
            pc: None,
            message: "m".to_string(),
        };
        let s = entry.format();
        assert!(s.contains("sim_ns=000000000000"), "got: {s}");
        assert!(s.contains("insns=000000000000"), "got: {s}");
    }
}
