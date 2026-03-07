//! Cooperative thread scheduler for SE mode.
//!
//! Provides lightweight "green threads" so that guest binaries using
//! `clone(CLONE_THREAD)` can make progress.  Threads share the same
//! `AddressSpace` (CLONE_VM) and are scheduled cooperatively: the
//! engine switches context whenever a thread blocks on a syscall
//! (futex-wait, read on empty pipe, ppoll with no ready fds).

use helm_core::types::Addr;
use helm_isa::arm::regs::Aarch64Regs;
use std::collections::{HashMap, VecDeque};

/// Unique thread identifier.
pub type Tid = u64;

/// Per-thread saved state.
#[derive(Clone)]
pub struct Thread {
    pub tid: Tid,
    pub regs: Aarch64Regs,
    pub state: ThreadState,
    /// Address to clear + futex-wake on thread exit (CLONE_CHILD_CLEARTID).
    pub clear_tid_addr: Option<Addr>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThreadState {
    Runnable,
    /// Blocked on FUTEX_WAIT at the given address with expected value.
    FutexWait { uaddr: Addr, val: u32 },
    /// Blocked on read() — fd returned EAGAIN.
    BlockedRead,
    /// Blocked on ppoll/pselect with no ready fds.
    BlockedPoll,
    Exited,
}

/// Action returned by the syscall layer telling the scheduler what to do.
#[derive(Debug)]
pub enum SchedAction {
    /// Syscall handled normally — continue running the same thread.
    Continue(u64),
    /// Current thread should block on FUTEX_WAIT.
    FutexWait { uaddr: Addr, val: u32 },
    /// Wake up to `count` threads waiting on `uaddr`.  Returns wake count in x0.
    FutexWake { uaddr: Addr, count: u32 },
    /// Spawn a new thread.
    Clone(CloneRequest),
    /// Current thread calls exit (not exit_group).
    ThreadExit { code: u64 },
    /// Current thread should block (generic — read/ppoll).
    Block(ThreadState),
}

/// Parameters for a new thread.
#[derive(Debug)]
pub struct CloneRequest {
    pub flags: u64,
    pub child_stack: Addr,
    pub parent_tid_ptr: Addr,
    pub child_tid_ptr: Addr,
    pub tls: u64,
}

/// Cooperative scheduler managing multiple threads.
pub struct Scheduler {
    threads: Vec<Thread>,
    current: usize,
    next_tid: Tid,
}

impl Scheduler {
    /// Create a scheduler with the initial (main) thread.
    pub fn new(main_regs: Aarch64Regs, main_tid: Tid) -> Self {
        Self {
            threads: vec![Thread {
                tid: main_tid,
                regs: main_regs,
                state: ThreadState::Runnable,
                clear_tid_addr: None,
            }],
            current: 0,
            next_tid: main_tid + 1,
        }
    }

    pub fn current_tid(&self) -> Tid {
        self.threads[self.current].tid
    }

    pub fn current_regs(&self) -> &Aarch64Regs {
        &self.threads[self.current].regs
    }

    pub fn current_regs_mut(&mut self) -> &mut Aarch64Regs {
        &mut self.threads[self.current].regs
    }

    /// Total number of live (non-exited) threads.
    pub fn live_count(&self) -> usize {
        self.threads.iter().filter(|t| t.state != ThreadState::Exited).count()
    }

    /// Returns true if any thread is runnable.
    pub fn any_runnable(&self) -> bool {
        self.threads.iter().any(|t| t.state == ThreadState::Runnable)
    }

    /// Spawn a new thread. Returns (parent_ret=new_tid, new_tid).
    pub fn spawn(&mut self, req: CloneRequest) -> Tid {
        let tid = self.next_tid;
        self.next_tid += 1;

        let mut child_regs = self.threads[self.current].regs.clone();
        child_regs.sp = req.child_stack;
        child_regs.x[0] = 0; // clone returns 0 in child
        child_regs.pc += 4;   // advance past the SVC
        child_regs.tpidr_el0 = req.tls;

        let clear_tid = if req.flags & 0x200000 != 0 {
            // CLONE_CHILD_CLEARTID
            Some(req.child_tid_ptr)
        } else {
            None
        };

        self.threads.push(Thread {
            tid,
            regs: child_regs,
            state: ThreadState::Runnable,
            clear_tid_addr: clear_tid,
        });
        tid
    }

    /// Block the current thread with the given state and switch to
    /// the next runnable thread.  Returns `true` if a switch happened.
    pub fn block_current(&mut self, state: ThreadState) -> bool {
        self.threads[self.current].state = state;
        self.try_switch()
    }

    /// Mark the current thread as exited.  Returns the clear_tid_addr
    /// if CLONE_CHILD_CLEARTID was set.
    pub fn exit_current(&mut self) -> Option<Addr> {
        let t = &mut self.threads[self.current];
        t.state = ThreadState::Exited;
        t.clear_tid_addr.take()
    }

    /// Wake up to `count` threads blocked on FUTEX_WAIT at `uaddr`.
    /// Returns the number of threads woken.
    pub fn futex_wake(&mut self, uaddr: Addr, count: u32) -> u32 {
        let mut woken = 0u32;
        for t in &mut self.threads {
            if woken >= count { break; }
            if let ThreadState::FutexWait { uaddr: wa, .. } = t.state {
                if wa == uaddr {
                    t.state = ThreadState::Runnable;
                    t.regs.x[0] = 0; // futex returns 0 on wake
                    woken += 1;
                }
            }
        }
        woken
    }

    /// Wake all threads in BlockedRead or BlockedPoll state.
    /// Called when a write to a pipe might unblock a reader.
    pub fn wake_io_waiters(&mut self) {
        for t in &mut self.threads {
            if matches!(t.state, ThreadState::BlockedRead | ThreadState::BlockedPoll) {
                t.state = ThreadState::Runnable;
            }
        }
    }

    /// Try to switch to the next runnable thread (round-robin).
    /// Returns true if we switched.
    pub fn try_switch(&mut self) -> bool {
        let n = self.threads.len();
        for offset in 1..=n {
            let idx = (self.current + offset) % n;
            if self.threads[idx].state == ThreadState::Runnable {
                self.current = idx;
                return true;
            }
        }
        false
    }

    /// Save CPU registers into the current thread's slot.
    pub fn save_regs(&mut self, regs: &Aarch64Regs) {
        self.threads[self.current].regs = regs.clone();
    }

    /// Load the current thread's registers into the CPU.
    pub fn load_regs(&self, regs: &mut Aarch64Regs) {
        *regs = self.threads[self.current].regs.clone();
    }

    /// Check for deadlock: all live threads blocked, none runnable.
    pub fn is_deadlocked(&self) -> bool {
        self.live_count() > 0 && !self.any_runnable()
    }

    /// Force-unblock all futex-waiters (deadlock breaker).
    /// Returns true if any thread was unblocked.
    pub fn break_deadlock(&mut self) -> bool {
        let mut unblocked = false;
        for t in &mut self.threads {
            if let ThreadState::FutexWait { .. } = t.state {
                t.state = ThreadState::Runnable;
                t.regs.x[0] = 0;
                unblocked = true;
            }
        }
        if !unblocked {
            // Unblock IO waiters too
            for t in &mut self.threads {
                if matches!(t.state, ThreadState::BlockedRead | ThreadState::BlockedPoll) {
                    t.state = ThreadState::Runnable;
                    unblocked = true;
                }
            }
        }
        unblocked
    }
}
