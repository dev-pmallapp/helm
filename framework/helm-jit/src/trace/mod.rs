//! LuaJIT-style trace recorder and compiler (Phase 2-D).
//!
//! Detects hot backward branches and records the execution path through
//! multiple basic blocks. Compiles the hot path as a single x86-64 function
//! with a direct backward `jmp` for the loop back-edge and guard exits for
//! conditional branches that leave the hot path.
//!
//! # Trace lifecycle
//!
//! 1. `TraceRecorder::on_backward_branch(pc)` — increments a per-PC counter.
//!    When the count reaches `TRACE_THRESHOLD`, recording begins.
//! 2. `TraceRecorder::record(pc, insns)` — appends instructions to the active
//!    recording. Detects loop closure (back to `start_pc`).
//! 3. `compiler::compile_trace(arena, insns, start_pc)` — emits the trace into
//!    the `CodeArena`. Returns a `CompiledTrace`.
//! 4. `TraceCache` — simple vec of up to `MAX_LIVE_TRACES` compiled traces,
//!    checked before the block JIT cache in the hot loop.
//! 5. On guard exit, `exit::handle_guard_exit` is called, which either falls
//!    back to block JIT / interpreter, or retires the trace after too many misses.

pub mod compiler;
pub mod exit;
pub mod recorder;

use helm_arch::aarch64::insn::Instruction;

/// Number of backward-branch executions before recording begins.
pub const TRACE_THRESHOLD: u32 = 64;

/// Maximum guest instructions in a single trace.
pub const TRACE_MAX_INSNS: usize = 512;

/// Maximum number of basic blocks inlined into a single trace.
pub const TRACE_MAX_DEPTH: u32 = 8;

/// Maximum number of guard exits in a single trace.
pub const MAX_GUARD_EXITS: usize = 32;

/// Maximum number of live compiled traces.
pub const MAX_LIVE_TRACES: usize = 256;

/// Guard-miss count above which a trace is retired.
pub const GUARD_MISS_THRESHOLD: u32 = 16;

/// Recording state machine for a single trace session.
#[derive(Default)]
#[allow(missing_docs)]
pub enum RecordState {
    /// Not currently recording.
    #[default]
    Idle,
    /// Actively recording a trace starting at `start_pc`.
    Recording {
        start_pc: u64,
        insns: Vec<Instruction>,
        pcs: Vec<u64>,
        depth: u32,
    },
    /// Recording complete — ready to compile.
    Complete {
        start_pc: u64,
        insns: Vec<Instruction>,
    },
}
