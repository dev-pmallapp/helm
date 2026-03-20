//! `helm-engine` — simulation kernel.
//!
//! # Key types
//! - [`HelmEngine<T>`] — generic simulation kernel; `T` is the `TimingModel`
//! - [`HelmSim`]       — enum wrapping all timing variants; the PyO3 boundary
//! - [`Isa`]           — which ISA is active (dispatch once per `run()` call)
//! - [`ExecMode`]      — functional / syscall-emulation / full-system
//! - [`FlatMem`]       — Phase 0 flat memory (replaced by `MemoryMap` in Phase 1)
//! - [`StopReason`]    — why `run()` returned
//!
//! # Inner loop contract
//! The inner loop (`step_*`) is hot. No allocations, no trait objects, no
//! dynamic dispatch. All cross-component refs are stored during `elaborate()`.

#![allow(missing_docs)]

pub mod fs;
pub mod loader;
pub mod platform;
pub mod se;
pub mod system_mem;

use helm_arch::{
    aarch64_decode, aarch64_execute, Aarch64ArchState,
    riscv_decode, riscv_execute, DecodeError,
};
pub use helm_core::{AccessType, MemFault, MemInterface};
use helm_core::{ExecContext, HartException};
use helm_event::EventQueue;
use helm_timing::{Accurate, InsnInfo, Interval, TimingModel, Virtual};

use helm_plugin::PluginRegistry;
pub use helm_plugin;

use helm_probe::{probe, BranchEvent, BranchKind as ProbeBranchKind, CpuProbes, CpuStepEvent, MemAccessEvent};

use crate::fs::FsState;
use helm_diag;
use helm_hw_intc::GicState;
use crate::platform::arm_virt::{self, ArmVirtDevices};
use crate::system_mem::SystemMem;
use se::{LinuxAarch64SyscallHandler, SyscallArgs, SyscallHandler};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct UnimplementedInstructionSite {
    pc: u64,
    raw: u32,
    opcode_name: &'static str,
}

struct Aarch64FsMachine {
    sys_mem: SystemMem,
    fs: FsState,
    #[allow(dead_code)]
    devs: ArmVirtDevices,
    /// IRQ line from the GIC — raised when any enabled SPI/PPI is pending.
    irq_line: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    /// Shared GIC state — used to assert device interrupts (e.g. timer PPI 30).
    #[allow(dead_code)]
    gic: Option<std::sync::Arc<std::sync::Mutex<GicState>>>,
}

// ── Isa ───────────────────────────────────────────────────────────────────────

/// Which ISA the engine is running.
///
/// Dispatched once per `run()` call via `match self.isa { ... }`.
/// Zero dispatch inside the per-instruction step functions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Isa {
    RiscV,
    AArch64,
    AArch32,
}

// ── ExecMode ──────────────────────────────────────────────────────────────────

/// Simulation execution mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecMode {
    /// Pure instruction execution — no syscall interception, no interrupts.
    Functional,
    /// Intercept `ecall` / `svc` and forward to the Linux ABI emulator.
    Syscall,
    /// Full-system: boot a real kernel, deliver interrupts, emulate MMU.
    System,
}

// ── StopReason ────────────────────────────────────────────────────────────────

/// Why `HelmEngine::run()` returned.
#[derive(Debug, Clone, PartialEq)]
pub enum StopReason {
    /// Ran the requested number of instructions.
    Quantum,
    /// Guest called `exit()`.
    Exit { code: i32 },
    /// Unhandled exception — simulation cannot continue.
    Exception(HartException),
    /// ISA not yet implemented.
    Unsupported,
}

// ── FlatMem ───────────────────────────────────────────────────────────────────

/// Sparse memory backend using contiguous mapped regions and a flat page table.
///
/// Ported from the reference implementation (`helm.git/crates/helm-memory/src/
/// address_space.rs`).  The key design points:
///
/// - `map(base, size)` allocates one contiguous `Vec<u8>` per region.
/// - A flat page table (`Vec<*mut u8>`, indexed by `(PA - base) >> 12`) gives
///   O(1) host-pointer lookups for single-page accesses — no HashMap, no hash.
/// - Page table is rebuilt after each `map()` call. Fast path is a two-compare,
///   one-null-check, one unsafe `copy_nonoverlapping` — the entire hot path fits
///   in a cache line.
/// - Reads to unmapped addresses return 0 (consistent with zero-initialized RAM).
/// - Safe: raw pointers point into heap-allocated `Vec<u8>` owned by the struct;
///   all access goes through `&mut self` so no aliasing is possible.
pub struct FlatMem {
    regions: Vec<FlatMemRegion>,
    /// Flat page table: each entry is a host pointer to the start of that 4KB
    /// page within the owning region's data buffer.  Null = unmapped.
    page_table: Vec<*mut u8>,
    /// Lowest PA covered by the page table.
    page_table_base: u64,
    /// Number of 4KB entries in the page table.
    page_table_pages: usize,
    /// Preserved for SystemMem RAM fast-path (`addr.wrapping_sub(base) < size`).
    pub base: u64,
    pub size_bytes: u64,
}

struct FlatMemRegion {
    base: u64,
    size: u64,
    data: Vec<u8>,
}

// Safety: raw pointers in page_table point into FlatMemRegion::data Vec<u8>
// buffers. They are valid for the lifetime of this FlatMem. All access goes
// through &mut self, so no concurrent aliasing is possible.
#[allow(unsafe_code)]
unsafe impl Send for FlatMem {}

const FM_PAGE_SHIFT: u32 = 12;
const FM_PAGE_SIZE:  u64 = 1 << FM_PAGE_SHIFT;
const FM_PAGE_MASK:  u64 = FM_PAGE_SIZE - 1;
/// 1M pages = 4 GiB coverage. Stays as 8 MiB table. SE-mode scattered segments
/// may exceed this and fall through to the region-scan slow path.
const FM_MAX_PAGES: usize = 1 << 20;

#[allow(unsafe_code)]
impl FlatMem {
    pub fn new(base: u64, size: usize) -> Self {
        let mut fm = Self {
            regions: Vec::new(),
            page_table: Vec::new(),
            page_table_base: 0,
            page_table_pages: 0,
            base,
            size_bytes: size as u64,
        };
        if size > 0 {
            fm.map(base, size as u64);
        }
        fm
    }

    /// Map a contiguous region. Existing regions are preserved (grows the space).
    pub fn map(&mut self, base: u64, size: u64) {
        self.regions.push(FlatMemRegion { base, size, data: vec![0u8; size as usize] });
        self.rebuild_page_table();
    }

    fn rebuild_page_table(&mut self) {
        use std::ptr;
        if self.regions.is_empty() {
            self.page_table.clear();
            self.page_table_base = 0;
            self.page_table_pages = 0;
            return;
        }
        let min_base = self.regions.iter().map(|r| r.base).min().unwrap();
        let max_end  = self.regions.iter().map(|r| r.base + r.size).max().unwrap();
        let base_page = min_base >> FM_PAGE_SHIFT;
        let end_page  = (max_end + FM_PAGE_MASK) >> FM_PAGE_SHIFT;
        let num_pages = (end_page - base_page) as usize;

        if num_pages > FM_MAX_PAGES {
            // PA range too large — disable page table, fall through to region scan.
            self.page_table.clear();
            self.page_table_base = 0;
            self.page_table_pages = 0;
            return;
        }
        self.page_table_base  = base_page << FM_PAGE_SHIFT;
        self.page_table_pages = num_pages;
        self.page_table = vec![ptr::null_mut(); num_pages];

        for region in &self.regions {
            // Require page-aligned base; size must cover at least one full page.
            if region.base & FM_PAGE_MASK != 0 || region.size < FM_PAGE_SIZE {
                continue; // sub-page region — leave for slow path
            }
            let pages     = (region.size >> FM_PAGE_SHIFT) as usize;
            let data_ptr  = region.data.as_ptr() as *mut u8;
            let start_idx = ((region.base >> FM_PAGE_SHIFT) - base_page) as usize;
            for p in 0..pages {
                let idx = start_idx + p;
                if idx < num_pages {
                    // SAFETY: p * PAGE_SIZE < region.data.len() because pages = size / PAGE_SIZE.
                    self.page_table[idx] = unsafe { data_ptr.add(p << FM_PAGE_SHIFT as usize) };
                }
            }
        }
    }

    /// Load bytes into a mapped region (e.g. from ELF loader).
    pub fn load_bytes(&mut self, addr: u64, bytes: &[u8]) {
        // Try fast path via page table first.
        let mut off: usize = 0;
        let mut va = addr;
        while off < bytes.len() {
            let page_off = (va & FM_PAGE_MASK) as usize;
            let chunk = (bytes.len() - off).min(FM_PAGE_SIZE as usize - page_off);
            if va >= self.page_table_base {
                let idx = ((va - self.page_table_base) >> FM_PAGE_SHIFT) as usize;
                if idx < self.page_table_pages {
                    let host = self.page_table[idx];
                    if !host.is_null() {
                        unsafe {
                            std::ptr::copy_nonoverlapping(
                                bytes[off..].as_ptr(),
                                host.add(page_off),
                                chunk,
                            );
                        }
                        off += chunk;
                        va  += chunk as u64;
                        continue;
                    }
                }
            }
            // Slow path: find region and write directly.
            let written = self.write_region(va, &bytes[off..off + chunk]);
            if !written {
                // Address not mapped — map on demand (SE mode scattered segments).
                let page_base = va & !FM_PAGE_MASK;
                self.map(page_base, FM_PAGE_SIZE);
                self.write_region(va, &bytes[off..off + chunk]);
            }
            off += chunk;
            va  += chunk as u64;
        }
    }

    fn write_region(&mut self, addr: u64, data: &[u8]) -> bool {
        for region in &mut self.regions {
            if addr >= region.base && addr + data.len() as u64 <= region.base + region.size {
                let off = (addr - region.base) as usize;
                region.data[off..off + data.len()].copy_from_slice(data);
                return true;
            }
        }
        false
    }

    /// O(1) read of up to 8 bytes. Falls back to region scan on page-table miss.
    #[inline]
    fn read_inner(&self, addr: u64, size: usize) -> u64 {
        let page_off = (addr & FM_PAGE_MASK) as usize;
        // Fast path: single-page access via page table.
        if page_off + size <= FM_PAGE_SIZE as usize && addr >= self.page_table_base {
            let idx = ((addr - self.page_table_base) >> FM_PAGE_SHIFT) as usize;
            if idx < self.page_table_pages {
                let host = self.page_table[idx];
                if !host.is_null() {
                    let mut buf = [0u8; 8];
                    unsafe {
                        std::ptr::copy_nonoverlapping(host.add(page_off), buf.as_mut_ptr(), size);
                    }
                    return u64::from_le_bytes(buf);
                }
            }
        }
        // Slow path: region scan.
        for region in &self.regions {
            if addr >= region.base && addr + size as u64 <= region.base + region.size {
                let off = (addr - region.base) as usize;
                let mut buf = [0u8; 8];
                buf[..size].copy_from_slice(&region.data[off..off + size]);
                return u64::from_le_bytes(buf);
            }
        }
        0 // unmapped reads as zero
    }

    /// O(1) write of up to 8 bytes.
    #[inline]
    fn write_inner(&mut self, addr: u64, size: usize, val: u64) {
        let bytes = val.to_le_bytes();
        let page_off = (addr & FM_PAGE_MASK) as usize;
        // Fast path: single-page access via page table.
        if page_off + size <= FM_PAGE_SIZE as usize && addr >= self.page_table_base {
            let idx = ((addr - self.page_table_base) >> FM_PAGE_SHIFT) as usize;
            if idx < self.page_table_pages {
                let host = self.page_table[idx];
                if !host.is_null() {
                    unsafe {
                        std::ptr::copy_nonoverlapping(bytes.as_ptr(), host.add(page_off), size);
                    }
                    return;
                }
            }
        }
        // Slow path: region scan.
        for region in &mut self.regions {
            if addr >= region.base && addr + size as u64 <= region.base + region.size {
                let off = (addr - region.base) as usize;
                region.data[off..off + size].copy_from_slice(&bytes[..size]);
                return;
            }
        }
        // Unmapped write: allocate on demand (SE mode).
        let page_base = addr & !FM_PAGE_MASK;
        self.map(page_base, FM_PAGE_SIZE);
        self.write_inner(addr, size, val);
    }
}

impl MemInterface for FlatMem {
    #[inline]
    fn read(&mut self, addr: u64, size: usize, _ty: AccessType) -> Result<u64, MemFault> {
        debug_assert!(size <= 8);
        Ok(self.read_inner(addr, size))
    }

    #[inline]
    fn write(&mut self, addr: u64, size: usize, val: u64, _ty: AccessType) -> Result<(), MemFault> {
        debug_assert!(size <= 8);
        self.write_inner(addr, size, val);
        Ok(())
    }
}

// ── InstrumentedMem ──────────────────────────────────────────────────────────

/// Stack-allocated memory access recorder for the plugin system.
///
/// Wraps `&mut FlatMem`, delegates all accesses, and records up to 8 entries
/// for post-execute callback dispatch.
struct InstrumentedMem<'a> {
    inner: &'a mut FlatMem,
    records: [MemAccessRecord; 8],
    count: usize,
}

#[derive(Clone, Copy)]
struct MemAccessRecord {
    vaddr: u64,
    size: u8,
    is_store: bool,
    is_atomic: bool,
}

impl Default for MemAccessRecord {
    fn default() -> Self {
        Self { vaddr: 0, size: 0, is_store: false, is_atomic: false }
    }
}

impl<'a> InstrumentedMem<'a> {
    fn new(inner: &'a mut FlatMem) -> Self {
        Self { inner, records: [MemAccessRecord::default(); 8], count: 0 }
    }

    fn push(&mut self, vaddr: u64, size: u8, is_store: bool, is_atomic: bool) {
        if self.count < 8 {
            self.records[self.count] = MemAccessRecord { vaddr, size, is_store, is_atomic };
            self.count += 1;
        }
    }

    fn recorded(&self) -> &[MemAccessRecord] {
        &self.records[..self.count]
    }
}

impl<'a> MemInterface for InstrumentedMem<'a> {
    fn read(&mut self, addr: u64, size: usize, ty: AccessType) -> Result<u64, MemFault> {
        let is_atomic = ty == AccessType::Atomic;
        self.push(addr, size as u8, false, is_atomic);
        self.inner.read(addr, size, ty)
    }

    fn write(&mut self, addr: u64, size: usize, val: u64, ty: AccessType) -> Result<(), MemFault> {
        let is_atomic = ty == AccessType::Atomic;
        self.push(addr, size as u8, true, is_atomic);
        self.inner.write(addr, size, val, ty)
    }
}

// ── HelmEngine<T> ─────────────────────────────────────────────────────────────

/// The simulation kernel, generic over timing model `T`.
///
/// Monomorphized at compile time — one binary specialization per timing variant.
/// The `HelmSim` enum selects which specialization to construct.
pub struct HelmEngine<T: TimingModel> {
    pub isa:  Isa,
    pub mode: ExecMode,
    pub timing: T,

    // RISC-V arch state (Phase 0 — will become an enum in Phase 2 for multi-ISA)
    pub iregs: [u64; 32],
    pub fregs: [u64; 32],
    pub csrs:  Box<[u64; 4096]>,
    pub pc:    u64,

    /// Reservation address for LR/SC atomics.
    pub lr_addr: Option<u64>,

    /// AArch64 architectural state (populated when isa == AArch64).
    pub a64_state: Option<Aarch64ArchState>,
    /// AArch64 Linux syscall handler (populated when isa == AArch64, mode == Syscall).
    pub a64_handler: Option<LinuxAarch64SyscallHandler>,
    /// AArch64 full-system machine state (populated when mode == System).
    a64_fs: Option<Aarch64FsMachine>,

    mem_size: usize,
    pub memory: FlatMem,
    pub events: EventQueue,

    pub syscall_handler: Option<Box<dyn SyscallHandler>>,

    /// Total instructions retired.
    pub insns_retired: u64,

    /// Countdown for the FS-mode periodic timer check (fires at 0, resets to TIMER_CHECK_INTERVAL).
    timer_countdown: u32,
    /// Countdown for IRQ line polling (poll every 16 instructions instead of every instruction).
    irq_poll_countdown: u8,

    /// Plugin callback registry.
    pub plugins: PluginRegistry,

    /// Typed probe bundle — zero-cost in release builds.
    pub probes: CpuProbes,

    /// ELF symbol table (populated after load_aarch64_elf).
    pub symbols: Vec<loader::ElfSymbol>,

    /// Unique stubbed instruction sites encountered during execution.
    unimplemented_instruction_sites: std::collections::HashSet<UnimplementedInstructionSite>,
}

impl<T: TimingModel> HelmEngine<T> {
    pub fn new(isa: Isa, mode: ExecMode, timing: T, mem_base: u64, mem_size: usize) -> Self {
        Self {
            isa,
            mode,
            timing,
            iregs: [0u64; 32],
            fregs: [0u64; 32],
            csrs:  Box::new([0u64; 4096]),
            pc:    0,
            lr_addr: None,
            a64_state: None,
            a64_handler: None,
            a64_fs: None,
            mem_size,
            memory: FlatMem::new(mem_base, mem_size),
            events: EventQueue::new(),
            syscall_handler: None,
            insns_retired: 0,
            timer_countdown: 1024,
            irq_poll_countdown: 16,
            plugins: PluginRegistry::new(),
            probes: CpuProbes::default(),
            symbols: Vec::new(),
            unimplemented_instruction_sites: std::collections::HashSet::new(),
        }
    }

    /// Set the program counter (reset vector).
    pub fn set_pc(&mut self, pc: u64) { self.pc = pc; }

    /// Load bytes into memory (e.g. from ELF loader).
    pub fn load_bytes(&mut self, addr: u64, bytes: &[u8]) {
        self.memory.load_bytes(addr, bytes);
    }

    /// Attach a syscall handler (required for `ExecMode::Syscall`).
    pub fn set_syscall_handler(&mut self, h: Box<dyn SyscallHandler>) {
        self.syscall_handler = Some(h);
    }

    fn note_unimplemented_instruction(&mut self, pc: u64, raw: u32, opcode_name: &'static str) -> bool {
        let site = UnimplementedInstructionSite { pc, raw, opcode_name };
        if self.unimplemented_instruction_sites.insert(site) {
            log::warn!(
                "unimplemented instruction executed at pc={pc:#x} raw={raw:#010x} kind={opcode_name}; future encounters of this site will be ignored"
            );
            true
        } else {
            false
        }
    }

    pub fn has_unimplemented_instructions(&self) -> bool {
        !self.unimplemented_instruction_sites.is_empty()
    }

    pub fn unimplemented_instruction_count(&self) -> usize {
        self.unimplemented_instruction_sites.len()
    }

    /// Run up to `max_insns` instructions. Returns the reason for stopping.
    pub fn run(&mut self, max_insns: u64) -> StopReason {
        for _ in 0..max_insns {
            let result = match self.isa {
                Isa::RiscV   => self.step_riscv(),
                Isa::AArch64 => {
                    if self.mode == ExecMode::System {
                        self.step_aarch64_system()
                    } else {
                        self.step_aarch64()
                    }
                }
                Isa::AArch32 => return StopReason::Unsupported,
            };
            match result {
                Ok(()) => {
                    self.insns_retired += 1;
                    // Only update probe insn count when probe subscribers exist.
                    // In release builds probe!() is zero-sized; this guard also
                    // eliminates the thread-local write on the common no-probe path.
                    if self.probes.any_active() {
                        helm_probe::update_probe_insn_count(self.insns_retired);
                    }
                    // Update sim-trace context only when a MonitorSink is active.
                    if helm_diag::is_monitor_active() {
                        helm_diag::update_sim_ctx(self.insns_retired, 1_000_000_000);
                    }
                    if self.plugins.has_any_callbacks() && self.plugins.has_timer_callbacks() {
                        self.plugins.fire_timer(0, self.insns_retired);
                    }
                }
                Err(exc) => {
                    let stop = self.handle_exception(exc);
                    // Check if AArch64 handler requested exit
                    if let Some(h) = &self.a64_handler {
                        if h.should_exit {
                            return StopReason::Exit { code: h.exit_code };
                        }
                    }
                    match stop {
                        // Syscall handled OK — count it and keep running.
                        StopReason::Quantum => { self.insns_retired += 1; }
                        ref s @ StopReason::Exit { .. } | ref s @ StopReason::Exception(_) => {
                            self.plugins.fire_vcpu_exit(0);
                            return s.clone();
                        }
                        other => return other,
                    }
                }
            }
        }
        StopReason::Quantum
    }

    /// Single-step one AArch64 instruction via decode() → execute().
    fn step_aarch64(&mut self) -> Result<(), HartException> {
        let pc = self.a64_state.as_ref().ok_or(HartException::Unsupported)?.pc;

        probe!(self.probes.pre_step, CpuStepEvent {
            pc,
            raw: 0,
            insn_class: helm_probe::InsnClass::Unknown,
            is_stub: false,
        });

        // 1. Fetch
        let raw = self.memory.fetch32(pc).map_err(|_| {
            HartException::InstructionAccessFault { addr: pc }
        })?;

        // 2. Decode
        let insn = match aarch64_decode(raw, pc) {
            Ok(insn) => insn,
            Err(DecodeError::Unknown { raw, pc }) => {
                return Err(HartException::IllegalInstruction { pc, raw });
            }
            Err(DecodeError::Unimplemented) => {
                self.note_unimplemented_instruction(pc, raw, "DecodeUnimplemented");
                return Err(HartException::Unsupported);
            }
        };

        // 3. Execute — use InstrumentedMem when mem callbacks are registered
        let use_mem_instrumentation = self.plugins.has_mem_callbacks();
        let pc_written;

        if use_mem_instrumentation {
            // Destructure to satisfy borrow checker: borrow a64_state and memory separately.
            let HelmEngine { ref mut a64_state, ref mut memory, ref plugins, ref probes, .. } = *self;
            let a64 = a64_state.as_mut().unwrap();
            let mut imem = InstrumentedMem::new(memory);
            pc_written = aarch64_execute(&insn, a64, &mut imem)?;
            if !pc_written {
                a64.pc = a64.pc.wrapping_add(4);
            }
            // Fire mem_access callbacks for each recorded access
            for rec in imem.recorded() {
                plugins.fire_mem_access(0, &helm_plugin::runtime::MemInfo {
                    vaddr: rec.vaddr, size: rec.size, is_store: rec.is_store, is_atomic: rec.is_atomic,
                });
            }
            for rec in imem.recorded() {
                probe!(probes.mem, MemAccessEvent {
                    addr: rec.vaddr,
                    size: rec.size,
                    is_store: rec.is_store,
                    pc,
                });
            }
        } else {
            let a64 = self.a64_state.as_mut().unwrap();
            pc_written = aarch64_execute(&insn, a64, &mut self.memory)?;
            if !pc_written {
                a64.pc = a64.pc.wrapping_add(4);
            }
        }

        // Classify opcode (used by both probe and plugin callbacks)
        let (class, opcode_name, is_stub) = classify_aarch64_opcode(insn.opcode);

        // Probe: post-step
        probe!(self.probes.post_step, CpuStepEvent {
            pc,
            raw,
            insn_class: to_probe_class(class),
            is_stub,
        });

        // Probe: branch
        if insn.is_branch() {
            let target = self.a64_state.as_ref().map(|s| s.pc).unwrap_or(pc.wrapping_add(4));
            probe!(self.probes.branch, BranchEvent {
                pc,
                target,
                taken: pc_written,
                kind: probe_branch_kind(insn.opcode),
            });
        }

        // 4. Timing
        let tinfo = InsnInfo {
            pc,
            is_branch: insn.is_branch(),
            is_load:   insn.is_mem_access(),
            is_store:  insn.is_mem_access(),
            is_fp:     false,
        };
        self.timing.on_insn(&tinfo);

        // 5. Plugin callbacks
        if is_stub {
            self.note_unimplemented_instruction(pc, raw, opcode_name);
        }
        if self.plugins.has_insn_callbacks() {
            self.plugins.fire_insn_exec(0, &helm_plugin::runtime::InsnInfo {
                pc, raw, size: 4, class, opcode_name, is_stub,
                context: if let Some(a) = &self.a64_state {
                    helm_plugin::runtime::ArchContext::Aarch64 {
                        x: a.x, sp: a.sp, pc: a.pc, nzcv: a.nzcv,
                    }
                } else {
                    helm_plugin::runtime::ArchContext::None
                },
            });
        }

        // 6. Branch callback
        if self.plugins.has_branch_callbacks() && insn.is_branch() {
            let target = self.a64_state.as_ref().unwrap().pc;
            self.plugins.fire_branch(0, &helm_plugin::runtime::BranchInfo {
                pc,
                target,
                taken: pc_written,
                kind: classify_branch_kind(insn.opcode),
            });
        }

        Ok(())
    }

    fn step_aarch64_system(&mut self) -> Result<(), HartException> {
        let HelmEngine { ref mut a64_state, ref mut a64_fs, ref probes, .. } = *self;
        let a64 = a64_state.as_mut().ok_or(HartException::Unsupported)?;
        let machine = a64_fs.as_mut().ok_or(HartException::Unsupported)?;

        // Record PC before step so we can detect and log branches afterwards.
        // pc_before is retained for future branch probe wiring (Phase 2).
        let _pc_before = a64.pc;

        // Physical timer (PPI 30, INTID 30) — level-triggered signal.
        // Countdown replaces the previous `% 1024` modulo (avoids integer division).
        self.timer_countdown -= 1;
        if self.timer_countdown == 0 {
            self.timer_countdown = 1024;
            arm_virt::inject_timers(a64, &mut machine.fs, &mut machine.sys_mem);
        }

        // Sync irq_pending from the GIC IRQ line (level-triggered, not edge).
        // Polled every 16 instructions instead of every instruction to avoid
        // the AtomicBool load overhead on the critical path. IRQ latency is at
        // most 16 instructions (~3 ns at current speed — negligible for Linux).
        //
        // Critical: ASSIGN (not OR) so irq_pending tracks the line exactly.
        // When the kernel reads GICC_IAR the line drops; on the next poll
        // irq_pending becomes false, preventing spurious re-interrupts after ERET.
        self.irq_poll_countdown -= 1;
        if self.irq_poll_countdown == 0 {
            self.irq_poll_countdown = 16;
            machine.fs.irq_pending = machine.irq_line
                .as_ref()
                .map_or(false, |l| l.load(std::sync::atomic::Ordering::Relaxed));
        }

       let result = fs::step_aarch64_fs(a64, &mut machine.sys_mem, &mut machine.fs, probes);

        // WFI fast-forward: when the kernel executes WFI, fs.tick stops advancing
        // (step returned early before tick += 1). Jump tick forward to the nearest
        // armed timer deadline so the next timer-check interval fires immediately
        // rather than waiting for real instruction-count progress to catch up.
        // WFI fast-forward: advance the virtual tick to the nearest timer deadline
        // so the next inject_timers() call fires immediately.
        if matches!(result, Err(helm_core::HartException::WaitForInterrupt)) {
            let mut nearest = u64::MAX;
            if a64.cntp_ctl_el0 & 1 != 0 { nearest = nearest.min(a64.cntp_cval_el0); }
            if a64.cntv_ctl_el0 & 1 != 0  { nearest = nearest.min(a64.cntv_cval_el0); }
            if nearest != u64::MAX && nearest > machine.fs.tick {
                machine.fs.tick = nearest;
                a64.cntvct_el0 = nearest;
            }
            arm_virt::inject_timers(a64, &mut machine.fs, &mut machine.sys_mem);
        }

       result
    }

    /// Load a static AArch64 ELF binary and set up the engine for SE mode.
    ///
    /// Initialises `a64_state`, sets PC and SP, and configures the syscall handler.
    pub fn load_aarch64_elf(&mut self, path: &str, argv: &[&str], envp: &[&str]) -> Result<(), String> {
        use loader::load_elf;

        let loaded = load_elf(path, argv, envp, &mut self.memory)?;

        // Pre-map the initial brk page so musl can access it before calling brk()
        // Linux always has at least one page mapped at the brk base
        {
            let zeros = vec![0u8; 0x1000];
            self.memory.load_bytes(loaded.brk_base, &zeros);
        }

        let mut state = Aarch64ArchState::new();
        state.pc = loaded.entry_point;
        state.sp = loaded.initial_sp;

        let mut handler = LinuxAarch64SyscallHandler::new(loaded.brk_base);
        handler.binary_path = path.to_string();

        self.a64_state   = Some(state);
        self.a64_handler = Some(handler);
        self.a64_fs      = None;
        self.mode        = ExecMode::Syscall;
        self.symbols     = loaded.symbols;

        self.plugins.fire_vcpu_init(0);
        Ok(())
    }

    /// Load an ARM64 Linux Image and configure the engine for FS mode on arm-virt.
    ///
    /// `append` overrides the DTB `/chosen/bootargs` (highest precedence).
    pub fn load_aarch64_kernel(
        &mut self,
        kernel_path: &str,
        dtb_path: &str,
        initrd_path: Option<&str>,
        append: Option<&str>,
    ) -> Result<(), String> {
        let (state, sys_mem, fs, devs, irq_line, gic_state) = arm_virt::setup_arm_virt_boot(
            kernel_path,
            dtb_path,
            initrd_path,
            append,
            self.mem_size / (1024 * 1024),
            Box::new(arm_virt::StdioCharBackend),
        )?;

        self.a64_state = Some(state);
        self.a64_handler = None;
        self.a64_fs = Some(Aarch64FsMachine { sys_mem, fs, devs, irq_line: Some(irq_line), gic: Some(gic_state) });
        self.mode = ExecMode::System;
        self.symbols.clear();

        self.plugins.fire_vcpu_init(0);
        Ok(())
    }

    fn step_riscv(&mut self) -> Result<(), HartException> {
        let pc = self.pc;

        // 1. Fetch
        let raw = self.memory.fetch32(pc).map_err(|_| {
            HartException::InstructionAccessFault { addr: pc }
        })?;

        // 2. Decode
        let insn = riscv_decode(raw, pc).map_err(|e| match e {
            DecodeError::Unknown { raw, pc } => HartException::IllegalInstruction { pc, raw },
            DecodeError::Unimplemented      => HartException::Unsupported,
        })?;

        // 3. Execute (writes PC itself)
        riscv_execute(insn, self)?;

        // 4. Timing
        let info = InsnInfo {
            pc,
            is_branch: insn.is_control_flow(),
            is_load:   insn.is_mem_access(),
            is_store:  insn.is_mem_access(),
            is_fp:     false, // TODO: add is_fp() to Instruction
        };
        self.timing.on_insn(&info);

        Ok(())
    }

    fn handle_exception(&mut self, exc: HartException) -> StopReason {
        match exc {
            HartException::EnvironmentCall { pc: _, nr } => {
                if self.mode == ExecMode::Syscall {
                    // AArch64: syscall number from X8 (passed in `nr`), args from X0-X5
                    if self.isa == Isa::AArch64 {
                        return self.dispatch_aarch64_syscall(nr);
                    }
                    // RISC-V: forward to generic SyscallHandler
                    if let Some(handler) = &mut self.syscall_handler {
                        let args = SyscallArgs {
                            a0: self.iregs[10], a1: self.iregs[11],
                            a2: self.iregs[12], a3: self.iregs[13],
                            a4: self.iregs[14], a5: self.iregs[15],
                        };
                        match handler.handle(nr, args) {
                            Ok(ret) => { self.iregs[10] = ret as u64; }
                            Err(e)  => return self.handle_exception(e),
                        }
                    }
                    return StopReason::Quantum;
                }
                StopReason::Exception(HartException::EnvironmentCall { pc: 0, nr })
            }
            HartException::Exit { code } => StopReason::Exit { code },
            HartException::WaitForInterrupt => {
                // In FS mode, WFI is a hint; resume on next run() call.
                StopReason::Quantum
            }
            HartException::DataAbort { .. } | HartException::InstructionAbort { .. } => {
                // In FS mode, these are delivered as exceptions by the step function.
                // If we get them here, the FS handler didn't catch them.
                StopReason::Exception(exc)
            }
            HartException::Unsupported => StopReason::Unsupported,
            other => {
                // Fire plugin fault callback before returning.
                let (pc, raw_insn, kind, message) = match &other {
                    HartException::IllegalInstruction { pc, raw } => {
                        (*pc, *raw, helm_plugin::runtime::FaultKind::IllegalInstruction,
                         format!("illegal instruction at {pc:#x} (raw={raw:#010x})"))
                    }
                    HartException::Breakpoint { pc } => {
                        let raw = self.memory.fetch32(*pc).unwrap_or(0);
                        (*pc, raw, helm_plugin::runtime::FaultKind::Breakpoint,
                         format!("breakpoint at {pc:#x}"))
                    }
                    HartException::InstructionAddressMisaligned { addr } => {
                        (*addr, 0, helm_plugin::runtime::FaultKind::WildJump,
                         format!("instruction address misaligned: {addr:#x}"))
                    }
                    HartException::LoadAccessFault { addr } => {
                        (0, 0, helm_plugin::runtime::FaultKind::MemoryFault,
                         format!("load access fault at {addr:#x}"))
                    }
                    HartException::StoreAccessFault { addr } => {
                        (0, 0, helm_plugin::runtime::FaultKind::MemoryFault,
                         format!("store/AMO access fault at {addr:#x}"))
                    }
                    HartException::InstructionAccessFault { addr } => {
                        (*addr, 0, helm_plugin::runtime::FaultKind::MemoryFault,
                         format!("instruction access fault at {addr:#x}"))
                    }
                    _ => (0, 0, helm_plugin::runtime::FaultKind::IllegalInstruction,
                          format!("{other}"))
                };
                let context = if let Some(a) = &self.a64_state {
                    helm_plugin::runtime::ArchContext::Aarch64 {
                        x: a.x, sp: a.sp, pc: a.pc, nzcv: a.nzcv,
                    }
                } else {
                    helm_plugin::runtime::ArchContext::RiscV {
                        x: self.iregs, pc: self.pc,
                    }
                };
                self.plugins.fire_fault(&helm_plugin::runtime::FaultInfo {
                    vcpu_idx: 0, pc, raw: raw_insn, kind, message,
                    insn_count: self.insns_retired, context,
                });
                StopReason::Exception(other)
            }
        }
    }

    /// Dispatch one AArch64 SVC syscall to `LinuxAarch64SyscallHandler`.
    fn dispatch_aarch64_syscall(&mut self, nr: u64) -> StopReason {
        // Borrow arch state and handler separately — can't borrow self twice.
        let (x0, x1, x2, x3, x4, x5) = {
            let a = self.a64_state.as_ref().expect("a64_state missing");
            (a.x[0], a.x[1], a.x[2], a.x[3], a.x[4], a.x[5])
        };
        let args = SyscallArgs { a0: x0, a1: x1, a2: x2, a3: x3, a4: x4, a5: x5 };

        // Fire plugin pre-syscall event
        self.plugins.fire_syscall(&helm_plugin::runtime::SyscallInfo {
            vcpu_idx: 0, number: nr, args: [x0, x1, x2, x3, x4, x5],
        });

        let result = if let Some(h) = &mut self.a64_handler {
            h.handle(nr, args, &mut self.memory)
        } else {
            Ok(-38) // -ENOSYS if no handler
        };

        match result {
            Ok(ret) => {
                if let Some(a) = &mut self.a64_state {
                    a.x[0] = ret as u64;
                    // Advance PC past the SVC instruction
                    a.pc = a.pc.wrapping_add(4);
                }
                // Fire plugin post-syscall event
                self.plugins.fire_syscall_ret(&helm_plugin::runtime::SyscallRetInfo {
                    vcpu_idx: 0, number: nr, ret_value: ret as u64,
                });
                StopReason::Quantum
            }
            Err(HartException::Exit { code }) => StopReason::Exit { code },
            Err(e) => StopReason::Exception(e),
        }
    }
}

// ── ExecContext impl for HelmEngine<T> ───────────────────────────────────────

impl<T: TimingModel> ExecContext for HelmEngine<T> {
    #[inline(always)]
    fn read_int_reg(&self, idx: usize) -> u64 { self.iregs[idx] }

    #[inline(always)]
    fn write_int_reg(&mut self, idx: usize, val: u64) {
        if idx != 0 { self.iregs[idx] = val; }
    }

    #[inline(always)]
    fn read_float_reg_bits(&self, idx: usize) -> u64 { self.fregs[idx] }

    #[inline(always)]
    fn write_float_reg_bits(&mut self, idx: usize, val: u64) { self.fregs[idx] = val; }

    #[inline(always)]
    fn read_csr(&self, addr: u16) -> u64 { self.csrs[addr as usize] }

    #[inline(always)]
    fn write_csr(&mut self, addr: u16, val: u64) { self.csrs[addr as usize] = val; }

    #[inline(always)]
    fn read_pc(&self) -> u64 { self.pc }

    #[inline(always)]
    fn write_pc(&mut self, val: u64) { self.pc = val; }

    #[inline(always)]
    fn read_mem(&mut self, addr: u64, size: usize, ty: AccessType) -> Result<u64, MemFault> {
        self.memory.read(addr, size, ty)
    }

    #[inline(always)]
    fn write_mem(&mut self, addr: u64, size: usize, val: u64, ty: AccessType) -> Result<(), MemFault> {
        self.memory.write(addr, size, val, ty)
    }
}

// ── HelmSim ───────────────────────────────────────────────────────────────────

/// The PyO3 boundary — one enum variant per timing model.
///
/// Python calls `build_simulator()` which returns a `HelmSim`.
/// All Python-facing methods dispatch into the appropriate arm.
pub enum HelmSim {
    Virtual(HelmEngine<Virtual>),
    Interval(HelmEngine<Interval>),
    Accurate(HelmEngine<Accurate>),
}

impl HelmSim {
    pub fn run(&mut self, max_insns: u64) -> StopReason {
        match self {
            Self::Virtual(e)  => e.run(max_insns),
            Self::Interval(e) => e.run(max_insns),
            Self::Accurate(e) => e.run(max_insns),
        }
    }

    pub fn insns_retired(&self) -> u64 {
        match self {
            Self::Virtual(e)  => e.insns_retired,
            Self::Interval(e) => e.insns_retired,
            Self::Accurate(e) => e.insns_retired,
        }
    }

    pub fn set_pc(&mut self, pc: u64) {
        match self {
            Self::Virtual(e)  => e.set_pc(pc),
            Self::Interval(e) => e.set_pc(pc),
            Self::Accurate(e) => e.set_pc(pc),
        }
    }

    pub fn load_bytes(&mut self, addr: u64, bytes: &[u8]) {
        match self {
            Self::Virtual(e)  => e.load_bytes(addr, bytes),
            Self::Interval(e) => e.load_bytes(addr, bytes),
            Self::Accurate(e) => e.load_bytes(addr, bytes),
        }
    }

    /// Load an AArch64 ELF binary and configure the engine for SE mode.
    pub fn load_aarch64_elf(&mut self, path: &str, argv: &[&str], envp: &[&str]) -> Result<(), String> {
        match self {
            Self::Virtual(e)  => e.load_aarch64_elf(path, argv, envp),
            Self::Interval(e) => e.load_aarch64_elf(path, argv, envp),
            Self::Accurate(e) => e.load_aarch64_elf(path, argv, envp),
        }
    }

    /// Load an ARM64 Linux Image and configure the simulator for FS mode.
    ///
    /// `append` overrides the DTB `/chosen/bootargs` (highest precedence).
    pub fn load_aarch64_kernel(
        &mut self,
        kernel_path: &str,
        dtb_path: &str,
        initrd_path: Option<&str>,
        append: Option<&str>,
    ) -> Result<(), String> {
        match self {
            Self::Virtual(e)  => e.load_aarch64_kernel(kernel_path, dtb_path, initrd_path, append),
            Self::Interval(e) => e.load_aarch64_kernel(kernel_path, dtb_path, initrd_path, append),
            Self::Accurate(e) => e.load_aarch64_kernel(kernel_path, dtb_path, initrd_path, append),
        }
    }

    /// Get mutable reference to the plugin registry.
    pub fn plugins_mut(&mut self) -> &mut PluginRegistry {
        match self {
            Self::Virtual(e)  => &mut e.plugins,
            Self::Interval(e) => &mut e.plugins,
            Self::Accurate(e) => &mut e.plugins,
        }
    }

    /// Get the loaded ELF symbol table.
    pub fn symbols(&self) -> &[loader::ElfSymbol] {
        match self {
            Self::Virtual(e)  => &e.symbols,
            Self::Interval(e) => &e.symbols,
            Self::Accurate(e) => &e.symbols,
        }
    }

    /// Resolve a symbol name to its address. Returns None if not found.
    pub fn resolve_symbol(&self, name: &str) -> Option<u64> {
        self.symbols().iter().find(|s| s.name == name).map(|s| s.addr)
    }

    pub fn has_unimplemented_instructions(&self) -> bool {
        match self {
            Self::Virtual(e)  => e.has_unimplemented_instructions(),
            Self::Interval(e) => e.has_unimplemented_instructions(),
            Self::Accurate(e) => e.has_unimplemented_instructions(),
        }
    }

    pub fn unimplemented_instruction_count(&self) -> usize {
        match self {
            Self::Virtual(e)  => e.unimplemented_instruction_count(),
            Self::Interval(e) => e.unimplemented_instruction_count(),
            Self::Accurate(e) => e.unimplemented_instruction_count(),
        }
    }

    /// Immutable reference to the AArch64 architectural state (if ISA == AArch64).
    pub fn a64_state(&self) -> Option<&Aarch64ArchState> {
        match self {
            Self::Virtual(e)  => e.a64_state.as_ref(),
            Self::Interval(e) => e.a64_state.as_ref(),
            Self::Accurate(e) => e.a64_state.as_ref(),
        }
    }

    /// Current program counter.
    pub fn pc(&self) -> u64 {
        match self {
            Self::Virtual(e)  => e.a64_state.as_ref().map_or(e.pc, |s| s.pc),
            Self::Interval(e) => e.a64_state.as_ref().map_or(e.pc, |s| s.pc),
            Self::Accurate(e) => e.a64_state.as_ref().map_or(e.pc, |s| s.pc),
        }
    }

    /// Mutable reference to the CPU probe bundle.
    pub fn probes_mut(&mut self) -> &mut CpuProbes {
        match self {
            Self::Virtual(e)  => &mut e.probes,
            Self::Interval(e) => &mut e.probes,
            Self::Accurate(e) => &mut e.probes,
        }
    }

    /// Read `size` bytes from guest memory (for debugging).
    pub fn read_mem(&mut self, addr: u64, size: usize) -> u64 {
        use helm_core::{AccessType, MemInterface};
        match self {
            Self::Virtual(e)  => e.memory.read(addr, size, AccessType::Load).unwrap_or(0xDEAD),
            Self::Interval(e) => e.memory.read(addr, size, AccessType::Load).unwrap_or(0xDEAD),
            Self::Accurate(e) => e.memory.read(addr, size, AccessType::Load).unwrap_or(0xDEAD),
        }
    }

    /// Apply an ARM core model, setting MIDR_EL1 and ID registers.
    ///
    /// `model_name` is case-insensitive: `"cortex-a55"`, `"neoverse-n1"`, etc.
    /// Returns `Err(String)` for unknown names.
    pub fn set_cpu_model(&mut self, model_name: &str) -> Result<(), String> {
        let m = helm_arch::ArmCoreModel::from_name(model_name)
            .ok_or_else(|| format!("Unknown ARM core model '{model_name}'"))?;
        match self {
            Self::Virtual(e)  => { if let Some(s) = e.a64_state.as_mut() { m.apply(s); } }
            Self::Interval(e) => { if let Some(s) = e.a64_state.as_mut() { m.apply(s); } }
            Self::Accurate(e) => { if let Some(s) = e.a64_state.as_mut() { m.apply(s); } }
        }
        Ok(())
    }
}

// ── build_simulator ───────────────────────────────────────────────────────────

/// Constructor called from Python config (or Rust tests).
///
/// `mem_base` and `mem_size` define the flat guest-physical memory window.
pub fn build_simulator(
    isa: Isa,
    mode: ExecMode,
    timing: TimingChoice,
    mem_base: u64,
    mem_size: usize,
) -> HelmSim {
    match timing {
        TimingChoice::Virtual { ipc } => {
            HelmSim::Virtual(HelmEngine::new(isa, mode, Virtual::new(ipc), mem_base, mem_size))
        }
        TimingChoice::Interval { ipc, interval_len } => HelmSim::Interval(HelmEngine::new(
            isa,
            mode,
            Interval::new(ipc, interval_len),
            mem_base,
            mem_size,
        )),
        TimingChoice::Accurate => {
            HelmSim::Accurate(HelmEngine::new(isa, mode, Accurate::default(), mem_base, mem_size))
        }
    }
}

/// Classify an AArch64 opcode for the plugin system.
/// Returns (InsnClass, opcode_name, is_stub).
pub(crate) fn classify_aarch64_opcode(
    op: helm_arch::aarch64::insn::Opcode,
) -> (helm_plugin::runtime::InsnClass, &'static str, bool) {
    use helm_arch::aarch64::insn::Opcode::*;
    use helm_plugin::runtime::InsnClass;

    match op {
        // Data processing
        Adr | Adrp | AddImm | SubImm | AddsImm | SubsImm
        | AndImm | OrrImm | EorImm | AndsImm
        | Movn | Movz | Movk | Sbfm | Bfm | Ubfm | Extr
        | AddReg | SubReg | AddsReg | SubsReg
        | AddExt | SubExt | AddsExt | SubsExt
        | AndReg | OrrReg | EorReg | AndsReg | BicReg | OrnReg | EonReg | BicsReg
        | Adc | Adcs | Sbc | Sbcs
        | Lsl | Lsr | Asr | Ror
        | Cls | Clz | Rev | Rev16 | Rev32 | Rbit
        | Csel | Csinc | Csinv | Csneg | Ccmn | Ccmp
            => (InsnClass::IntAlu, "IntAlu", false),

        Mul | Madd | Msub | Mneg | Smulh | Umulh | Sdiv | Udiv
        | Smaddl | Smsubl | Umaddl | Umsubl
            => (InsnClass::IntMul, "IntMul", false),

        Crc32 | Crc32c => (InsnClass::IntAlu, "Crc32", true), // stub

        // Branch
        B | Bl | Br | Blr | Ret | BCond | Cbz | Cbnz | Tbz | Tbnz
        | Svc | Hvc | Smc | Eret
            => (InsnClass::Branch, "Branch", false),

        // Load/Store
        Ldr | Ldrb | Ldrh | Ldrsb | Ldrsh | Ldrsw | LdrLit | LdrswLit
        | Ldp | Ldur | Ldurb | Ldurh | Ldursb | Ldursh | Ldursw
        | Ldxr | Ldaxr | Ldxp | Ldaxp | Ldar
        | LdrSimd | LdpSimd | LdurSimd
            => (InsnClass::Load, "Load", false),

        Str | Strb | Strh | Stp | Stur | Sturb | Sturh
        | Stxr | Stlxr | Stxp | Stlxp | Stlr
        | StrSimd | StpSimd | SturSimd
            => (InsnClass::Store, "Store", false),

        // Atomics
        Ldadd | Ldclr | Ldeor | Ldset | LdSmax | LdSmin | LdUmax | LdUmin | Swp | Cas | Casp
            => (InsnClass::Atomic, "Atomic", false),

        // FP
        FmovImm | FmovReg | FmovGpr
        | Fadd | Fsub | Fmul | Fdiv | Fsqrt | Fabs | Fneg | Fnmul
        | Fmax | Fmin | Fmaxnm | Fminnm
        | Fmadd | Fmsub | Fnmadd | Fnmsub
        | Fcmp | Fcmpe | Fccmp | Fccmpe | Fcvt | Fsel
        | FcvtzsGpr | FcvtzuGpr | ScvtfGpr | UcvtfGpr
        | FcvtnsGpr | FcvtnuGpr | FcvtmsGpr | FcvtmuGpr
        | FcvtpsGpr | FcvtpuGpr | FcvtasGpr | FcvtauGpr
            => (InsnClass::FpAlu, "FpAlu", false),

        // System
        Nop | Wfi | Wfe | Sev | Sevl | Yield | Dmb | Dsb | Isb
        | Brk | Mrs | Msr | MsrImm | Sys | Clrex | Prfm
            => (InsnClass::System, "System", false),

        DcZva => (InsnClass::System, "DcZva", false),

        // SIMD — implemented
        SimdDup | SimdIns | SimdUmov | SimdSmov | SimdMovi
        | SimdAdd | SimdSub | SimdMul
        | SimdAnd | SimdOrr | SimdEor | SimdBic
        | SimdNot | SimdNeg | SimdAbs
        | SimdCmeq | SimdCmgt | SimdCmge | SimdCmhi | SimdCmhs
        | SimdUmaxv | SimdUminv
            => (InsnClass::SimdAlu, "SimdImpl", false),

        // SIMD — stubs (silently skipped)
        SimdOther => (InsnClass::SimdAlu, "SimdOther", true),
        SimdBif | SimdBit | SimdBsl | SimdOrrImm
            => (InsnClass::SimdAlu, "SimdLogic", true),
        SimdCmgt0 | SimdCmeq0 | SimdCmlt0 | SimdCmge0 | SimdCmle0
            => (InsnClass::SimdAlu, "SimdCmpZero", false),
        SimdCmtst
            => (InsnClass::SimdAlu, "SimdCmp", true),
        SimdAddp | SimdAddv
            => (InsnClass::SimdAlu, "SimdReduce", true),
        SimdSshl | SimdUshl | SimdSshr | SimdUshr | SimdShl
            => (InsnClass::SimdAlu, "SimdShift", true),
        SimdTbl | SimdTbx => (InsnClass::SimdAlu, "SimdTbl", true),
        SimdZip1 | SimdZip2 | SimdUzp1 | SimdUzp2 | SimdTrn1 | SimdTrn2
            => (InsnClass::SimdAlu, "SimdPermute", true),
        SimdExt => (InsnClass::SimdAlu, "SimdExt", true),
        SimdRev64 | SimdRev32 | SimdRev16 => (InsnClass::SimdAlu, "SimdRev", true),
        SimdCnt | SimdClz => (InsnClass::SimdAlu, "SimdBitCount", true),
        SimdSxtl | SimdUxtl => (InsnClass::SimdAlu, "SimdExtend", true),
        SimdSmin | SimdUmin | SimdSmax | SimdUmax => (InsnClass::SimdAlu, "SimdMinMax", true),
        SimdFadd | SimdFsub | SimdFmul | SimdFdiv
        | SimdFabs | SimdFneg | SimdFsqrt
        | SimdFcmeq | SimdFcmgt | SimdFcmge
        | SimdFcvtzs | SimdFcvtzu | SimdScvtf | SimdUcvtf
        | SimdFrintm | SimdFrintn | SimdFrintp | SimdFrintz
            => (InsnClass::SimdAlu, "SimdFp", true),
        SimdMvni | SimdFmov => (InsnClass::SimdAlu, "SimdMov", true),
        SimdLd1 | SimdSt1 | SimdLd2 | SimdSt2 | SimdLd3 | SimdSt3 | SimdLd4 | SimdSt4
        | SimdLd1r
            => (InsnClass::SimdAlu, "SimdMultiStruct", true),

        FcvtzsVec | FcvtzuVec => (InsnClass::SimdAlu, "SimdVecCvt", true),

        ScalarAddp => (InsnClass::SimdAlu, "ScalarAddp", false),
        // v8.3/v8.4 new opcodes
        Ldapr | Ldaprh | Ldaprb => (InsnClass::Load, "LdaprRcpc", false),
        LdapurB | LdapurH | Ldapur => (InsnClass::Load, "LdapurRcpc2", false),
        StlurB | StlurH | Stlur => (InsnClass::Store, "StlurRcpc2", false),
        Fjcvtzs => (InsnClass::FpAlu, "Fjcvtzs", false),
        Fcadd | Fcmla => (InsnClass::SimdAlu, "FcmaComplex", false),
        Sdot | Udot => (InsnClass::SimdAlu, "DotProduct", false),
        Setf8 | Setf16 | Cfinv | Rmif => (InsnClass::IntAlu, "FlagM", false),
        Bti => (InsnClass::System, "Bti", false),
        Sha3 | Sha512 | Sm3 | Sm4 => (InsnClass::SimdAlu, "CryptoStub", true),

        Undefined => (InsnClass::Unknown, "Undefined", false),
    }
}

/// Convert an AArch64 opcode to a `helm_probe::BranchKind`.
fn probe_branch_kind(op: helm_arch::aarch64::insn::Opcode) -> ProbeBranchKind {
    use helm_arch::aarch64::insn::Opcode::*;
    match op {
        B                                => ProbeBranchKind::DirectUncond,
        Bl                               => ProbeBranchKind::Call,
        Ret | Eret                       => ProbeBranchKind::Return,
        Br                               => ProbeBranchKind::IndirectJump,
        Blr                              => ProbeBranchKind::IndirectCall,
        BCond | Cbz | Cbnz | Tbz | Tbnz => ProbeBranchKind::DirectCond,
        _                                => ProbeBranchKind::DirectUncond,
    }
}

/// Map a helm-plugin InsnClass to helm-probe InsnClass.
pub(crate) fn to_probe_class(c: helm_plugin::runtime::InsnClass) -> helm_probe::InsnClass {
    use helm_plugin::runtime::InsnClass as P;
    use helm_probe::InsnClass as H;
    match c {
        P::IntAlu  => H::IntAlu,
        P::IntMul  => H::IntMul,
        P::Branch  => H::Branch,
        P::Load    => H::Load,
        P::Store   => H::Store,
        P::FpAlu   => H::FpAlu,
        P::SimdAlu => H::SimdAlu,
        P::System  => H::System,
        P::Nop     => H::Nop,
        P::Atomic  => H::Atomic,
        P::Unknown => H::Unknown,
    }
}

/// Classify an AArch64 branch opcode into a `BranchKind`.
fn classify_branch_kind(
    op: helm_arch::aarch64::insn::Opcode,
) -> helm_plugin::runtime::BranchKind {
    use helm_arch::aarch64::insn::Opcode::*;
    use helm_plugin::runtime::BranchKind;

    match op {
        B                                       => BranchKind::DirectUncond,
        Bl                                      => BranchKind::Call,
        Ret                                     => BranchKind::Return,
        Br                                      => BranchKind::IndirectJump,
        Blr                                     => BranchKind::IndirectCall,
        BCond | Cbz | Cbnz | Tbz | Tbnz        => BranchKind::DirectCond,
        _                                       => BranchKind::DirectUncond,
    }
}

/// Timing configuration passed to `build_simulator`.
pub enum TimingChoice {
    Virtual  { ipc: f64 },
    Interval { ipc: f64, interval_len: u64 },
    Accurate,
}

#[cfg(test)]
mod tests {
    use super::classify_aarch64_opcode;
    use crate::{ExecMode, HelmEngine, Isa, Virtual};
    use helm_arch::aarch64::insn::Opcode;

    #[test]
    fn classify_implemented_simd_ops_as_non_stub() {
        for opcode in [
            Opcode::SimdAdd,
            Opcode::SimdSub,
            Opcode::SimdMul,
            Opcode::SimdAnd,
            Opcode::SimdOrr,
            Opcode::SimdEor,
            Opcode::SimdBic,
            Opcode::SimdNot,
            Opcode::SimdNeg,
            Opcode::SimdAbs,
            Opcode::SimdCmeq,
            Opcode::SimdCmgt,
            Opcode::SimdCmge,
            Opcode::SimdCmhi,
            Opcode::SimdCmhs,
            Opcode::SimdUmaxv,
            Opcode::SimdUminv,
        ] {
            let (_, _, is_stub) = classify_aarch64_opcode(opcode);
            assert!(!is_stub, "{opcode:?} should not be classified as a stub");
        }
    }

    #[test]
    fn unimplemented_instruction_tracking_deduplicates_by_site() {
        let mut engine = HelmEngine::new(
            Isa::AArch64,
            ExecMode::Syscall,
            Virtual::new(1.0),
            0,
            1 << 20,
        );

        assert!(engine.note_unimplemented_instruction(0x1000, 0xDEADBEEF, "SimdOther"));
        assert!(!engine.note_unimplemented_instruction(0x1000, 0xDEADBEEF, "SimdOther"));
        assert!(engine.note_unimplemented_instruction(0x1004, 0xDEADBEEF, "SimdOther"));
        assert_eq!(engine.unimplemented_instruction_count(), 2);
        assert!(engine.has_unimplemented_instructions());
    }
}
