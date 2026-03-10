//! Pipeline stage trait and common stage definitions.

use helm_core::types::Cycle;

/// Each pipeline stage advances by one cycle at a time.
pub trait Stage: Send + Sync {
    fn name(&self) -> &str;
    fn tick(&mut self, cycle: Cycle);
    fn is_stalled(&self) -> bool;
}

/// Enum of the canonical OOO pipeline stages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StageName {
    Fetch,
    Decode,
    Rename,
    Dispatch,
    Issue,
    Execute,
    Complete,
    Commit,
}
