//! Built-in plugins.

pub mod cache_sim;
pub mod execlog;
pub mod hotblocks;
pub mod howvec;
pub mod insn_count;
pub mod syscall_trace;

pub use cache_sim::CacheSim;
pub use execlog::ExecLog;
pub use hotblocks::HotBlocks;
pub use howvec::HowVec;
pub use insn_count::InsnCount;
pub use syscall_trace::SyscallTrace;
