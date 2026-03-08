//! Full-System mode session.
//!
//! ```text
//! fs/
//!   session.rs  — Suspendable FS-mode session (FsSession)
//! ```

pub mod session;

pub use session::{FsOpts, FsSession};
