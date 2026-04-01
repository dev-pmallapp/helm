//! GDB Remote Serial Protocol implementation.

pub mod rsp;
pub mod target;

pub use rsp::RspServer;
pub use target::{GdbTarget, StopReason};
