//! Cooperative thread scheduler for SE mode.
//!
//! Provides lightweight "green threads" so that guest binaries using
//! `clone(CLONE_THREAD)` can make progress.  Threads share the same
//! `AddressSpace` (CLONE_VM) and are scheduled cooperatively: the
//! engine switches context whenever a thread blocks on a syscall
//! (futex-wait, read on empty pipe, ppoll with no ready fds).

use helm_core::types::Addr;
use helm_isa::arm::regs::Aarch64Regs;
use helm_memory::address_space::MemSnapshot;

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
    /// If this thread was created by a fork-style clone (no CLONE_VM),
    /// this holds the parent TID so we know to restore its snapshot
    /// when this child exits.
    pub fork_parent: Option<Tid>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThreadState {
    Runnable,
    /// Blocked on FUTEX_WAIT at the given address with expected value.
    FutexWait {
        uaddr: Addr,
        val: u32,
    },
    /// Blocked on read() — fd returned EAGAIN.
    BlockedRead,
    /// Blocked on ppoll/pselect with no ready fds.
    BlockedPoll,
    /// Blocked on wait4() — waiting for any child thread to exit.
    WaitChild,
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
    /// Memory snapshots saved before fork-style child threads run.
    /// Keyed by the *parent* TID.
    fork_snapshots: Vec<(Tid, MemSnapshot)>,
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
                fork_parent: None,
            }],
            current: 0,
            next_tid: main_tid + 1,
            fork_snapshots: Vec::new(),
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
        self.threads
            .iter()
            .filter(|t| t.state != ThreadState::Exited)
            .count()
    }

    /// Returns true if any thread is runnable.
    pub fn any_runnable(&self) -> bool {
        self.threads
            .iter()
            .any(|t| t.state == ThreadState::Runnable)
    }

    /// Spawn a new thread. Returns (parent_ret=new_tid, new_tid).
    pub fn spawn(&mut self, req: CloneRequest) -> Tid {
        let tid = self.next_tid;
        self.next_tid += 1;

        let mut child_regs = self.threads[self.current].regs.clone();
        // When child_stack is 0 (fork-style clone) the child inherits
        // the parent's stack pointer, matching kernel behaviour.
        if req.child_stack != 0 {
            child_regs.sp = req.child_stack;
        }
        child_regs.x[0] = 0; // clone returns 0 in child
        child_regs.pc += 4; // advance past the SVC

        // Only override TPIDR_EL0 when CLONE_SETTLS is requested;
        // otherwise the child inherits the parent's value.
        const CLONE_SETTLS: u64 = 0x0008_0000;
        if req.flags & CLONE_SETTLS != 0 {
            child_regs.tpidr_el0 = req.tls;
        }

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
            fork_parent: None,
        });
        tid
    }

    /// Mark the most recently spawned child as a fork child of the
    /// current (parent) thread.
    pub fn mark_last_as_fork(&mut self) {
        let parent_tid = self.threads[self.current].tid;
        if let Some(last) = self.threads.last_mut() {
            last.fork_parent = Some(parent_tid);
        }
    }

    /// Store a memory snapshot for a parent thread (before fork child runs).
    pub fn push_fork_snapshot(&mut self, parent_tid: Tid, snap: MemSnapshot) {
        self.fork_snapshots.push((parent_tid, snap));
    }

    /// Pop the memory snapshot for a parent thread (after fork child exits).
    pub fn pop_fork_snapshot(&mut self, parent_tid: Tid) -> Option<MemSnapshot> {
        if let Some(pos) = self
            .fork_snapshots
            .iter()
            .rposition(|(t, _)| *t == parent_tid)
        {
            Some(self.fork_snapshots.remove(pos).1)
        } else {
            None
        }
    }

    /// Return the fork_parent TID of the current thread, if any.
    pub fn current_fork_parent(&self) -> Option<Tid> {
        self.threads[self.current].fork_parent
    }

    /// Override TPIDR_EL0 for the most recently spawned thread.
    pub fn set_last_spawned_tpidr(&mut self, tpidr: u64) {
        if let Some(last) = self.threads.last_mut() {
            last.regs.tpidr_el0 = tpidr;
        }
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
        self.exit_current_with_code(0)
    }

    /// Mark the current thread as exited with a specific exit code.
    /// Returns the clear_tid_addr if CLONE_CHILD_CLEARTID was set.
    pub fn exit_current_with_code(&mut self, code: u64) -> Option<Addr> {
        let t = &mut self.threads[self.current];
        t.state = ThreadState::Exited;
        t.regs.x[0] = code;
        t.clear_tid_addr.take()
    }

    /// Wake up to `count` threads blocked on FUTEX_WAIT at `uaddr`.
    /// Returns the number of threads woken.
    pub fn futex_wake(&mut self, uaddr: Addr, count: u32) -> u32 {
        let mut woken = 0u32;
        for t in &mut self.threads {
            if woken >= count {
                break;
            }
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
                if matches!(
                    t.state,
                    ThreadState::BlockedRead | ThreadState::BlockedPoll | ThreadState::WaitChild
                ) {
                    t.state = ThreadState::Runnable;
                    unblocked = true;
                }
            }
        }
        unblocked
    }

    /// Try to reap an exited child thread.
    ///
    /// Returns `Some((tid, exit_code))` if a child has exited, removing
    /// it from the thread list.  Returns `None` if no exited child is
    /// available.
    pub fn try_reap_child(&mut self) -> Option<(Tid, u64)> {
        let pos = self.threads.iter().position(|t| {
            t.state == ThreadState::Exited && t.tid != self.threads[self.current].tid
        })?;
        let child = self.threads.remove(pos);
        // Adjust current index if removal shifted it.
        if pos < self.current {
            self.current -= 1;
        }
        // exit code was stored in x0 by the exit handler
        let code = child.regs.x[0];
        Some((child.tid, code))
    }

    /// Wake all threads blocked on `WaitChild`.
    pub fn wake_wait_child(&mut self) {
        for t in &mut self.threads {
            if t.state == ThreadState::WaitChild {
                t.state = ThreadState::Runnable;
            }
        }
    }
}
