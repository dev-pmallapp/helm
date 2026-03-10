//! Generic session — ISA-independent execution loop.
//!
//! `Session<D, E, C>` composes a `Decoder`, `Executor`, and `CpuState`
//! with a `MemoryAccess` and optional `TimingBackend` to run guest code
//! without hardcoding any ISA-specific types.

use helm_core::cpu::CpuState;
use helm_core::decode::Decoder;
use helm_core::exec::Executor;
use helm_core::insn::InsnFlags;
use helm_core::mem::MemoryAccess;
use helm_core::syscall::{SyscallAction, SyscallHandler};
use helm_core::timing::TimingBackend;

/// Why execution stopped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GenericStopReason {
    /// Budget exhausted.
    BudgetExhausted,
    /// Process exited with code.
    Exit(u64),
    /// Unhandled exception.
    Exception { class: u32, iss: u32, pc: u64 },
    /// CPU halted (WFI with no pending IRQ).
    Halted,
    /// Decode error at PC.
    DecodeError(u64),
}

/// Generic ISA-independent execution session.
///
/// Composes traits from `helm-core` — no ISA-specific types appear here.
/// Concrete ISA types are injected by the caller (CLI or Python bindings).
pub struct GenericSession<D: Decoder, E: Executor, C: CpuState> {
    pub decoder: D,
    pub executor: E,
    pub cpu: C,
    pub mem: Box<dyn MemoryAccess>,
    pub timing: Box<dyn TimingBackend>,
    pub syscall: Option<Box<dyn SyscallHandler>>,
    pub insn_count: u64,
    pub cycle_count: u64,
}

impl<D: Decoder, E: Executor, C: CpuState> GenericSession<D, E, C> {
    pub fn new(
        decoder: D,
        executor: E,
        cpu: C,
        mem: Box<dyn MemoryAccess>,
        timing: Box<dyn TimingBackend>,
    ) -> Self {
        Self {
            decoder,
            executor,
            cpu,
            mem,
            timing,
            syscall: None,
            insn_count: 0,
            cycle_count: 0,
        }
    }

    /// Attach a syscall handler (SE mode).
    pub fn set_syscall_handler(&mut self, handler: Box<dyn SyscallHandler>) {
        self.syscall = Some(handler);
    }

    /// Run the interpreted execution loop for up to `budget` instructions.
    pub fn run_interpreted(&mut self, budget: u64) -> GenericStopReason {
        let min_size = self.decoder.min_insn_size();
        let mut fetch_buf = vec![0u8; min_size.max(16)]; // enough for any ISA

        for _ in 0..budget {
            let pc = self.cpu.pc();

            // Fetch
            if let Err(_) = self.mem.fetch(pc, &mut fetch_buf[..min_size.max(4)]) {
                return GenericStopReason::DecodeError(pc);
            }

            // Decode
            let insn = match self.decoder.decode(pc, &fetch_buf) {
                Ok(insn) => insn,
                Err(_) => return GenericStopReason::DecodeError(pc),
            };

            // Check for syscall — if handler attached and this is a syscall insn
            if insn.flags.contains(InsnFlags::SYSCALL) {
                if let Some(ref mut handler) = self.syscall {
                    // AArch64: syscall number in X8, args in X0-X5
                    let nr = self.cpu.gpr(8);
                    match handler.handle(nr, &mut self.cpu, &mut *self.mem) {
                        SyscallAction::Handled(ret) => {
                            self.cpu.set_gpr(0, ret);
                            self.cpu.set_pc(pc + insn.len as u64);
                            self.insn_count += 1;
                            continue;
                        }
                        SyscallAction::Exit { code } => {
                            return GenericStopReason::Exit(code);
                        }
                        _ => {
                            // Other actions (futex, clone, etc.) — advance PC
                            self.cpu.set_pc(pc + insn.len as u64);
                            self.insn_count += 1;
                            continue;
                        }
                    }
                }
            }

            // Execute
            let outcome = self.executor.execute(&insn, &mut self.cpu, &mut *self.mem);

            // Handle exception
            if let Some(ref exc) = outcome.exception {
                return GenericStopReason::Exception {
                    class: exc.class,
                    iss: exc.iss,
                    pc,
                };
            }

            // Timing
            let stall = self.timing.account(&insn, &outcome);
            self.cycle_count += 1 + stall;

            // Advance PC
            self.cpu.set_pc(outcome.next_pc);
            self.insn_count += 1;
        }

        GenericStopReason::BudgetExhausted
    }
}
