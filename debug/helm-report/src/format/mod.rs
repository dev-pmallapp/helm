// src/format/mod.rs -- ReportFormatter trait and submodule re-exports.

pub mod csv;
pub mod helmstats;
pub mod json;
pub mod text;

use crate::snapshot::HelmSpySnapshot;

/// Trait for formatting an HelmSpySnapshot into a byte buffer.
///
/// Implementations must be `Send + Sync` -- the engine may format from
/// a background thread.
pub trait ReportFormatter: Send + Sync {
    /// Format the entire snapshot into a byte buffer.
    fn format_session(&self, session: &HelmSpySnapshot) -> Vec<u8>;

    /// Format a single named counter value (for incremental delivery).
    fn format_counter(&self, name: &str, value: u64, unit: &str) -> Vec<u8>;

    /// Format a named histogram as (bin_label, count) pairs.
    fn format_histogram(&self, name: &str, bins: &[(&str, u64)]) -> Vec<u8>;

    /// MIME type for the output of this formatter.
    fn content_type(&self) -> &'static str;
}

pub use self::csv::CsvFormatter;
pub use self::helmstats::HelmstatsFormatter;
pub use self::json::JsonFormatter;
pub use self::text::TextFormatter;
