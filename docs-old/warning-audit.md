# Workspace Warning Audit

Actionable fixes for every compiler warning in the workspace (excluding
`helm-python`).  Each entry explains **why** the warning exists and the
correct resolution — not just `_`-prefixing or line deletion.

---

## helm-decode

### `tree.rs:187` — `result_tokens` unused + needlessly mutable

`result_tokens` is allocated and initialised with the mnemonic but
never consumed; the reconstruction loop below it builds `result`
directly via `format!` and string appends.  The original intent was
to collect tokens into a vec and join them, but the code took a
different path.

**Fix:** Finish the token-based reconstruction.  Replace the manual
`result = format!(…)` + string-append loop (lines ~207–230) with
pushes into `result_tokens`, then return `result_tokens.join(" ")`.
This makes the function produce identical output but through the
structured token path the author started.

---

## helm-device

### `arm/gic.rs:6` — unused import `DeviceEvent`

`Gic` implements the `Device` trait, whose `tick()` method returns
`Vec<DeviceEvent>`.  The current `tick()` impl returns an empty vec
literal, so the compiler doesn't see a use of the type name.

**Fix:** Implement IRQ-pending event emission in `Gic::tick()`.  When
the GIC has a pending-and-enabled interrupt, `tick()` should return
`vec![DeviceEvent::Irq { line: intid, assert: true }]`.  This is
the intended event path for the FS-mode IRQ delivery loop and will
naturally use `DeviceEvent`.

### `arm/bcm_gpio.rs:6`, `arm/bcm_mailbox.rs:6`, `arm/bcm_mini_uart.rs:7` — unused `DeviceEvent`

Same pattern as GIC: `Device::tick()` returns `Ok(vec![])` without
naming `DeviceEvent`.

**Fix:** Add interrupt-assertion logic to each device's `tick()`:
- **BCM GPIO:** Return `DeviceEvent::Irq` when an edge is detected on
  a pin whose interrupt is enabled (check `gpeds` rising/falling edge
  detect status).
- **BCM Mailbox:** Return `DeviceEvent::Irq` when the response FIFO
  transitions from empty to non-empty and the mailbox IRQ enable bit
  is set.
- **BCM Mini UART:** Return `DeviceEvent::Irq` when the RX FIFO has
  data and `AUX_MU_IER` has RX-interrupt enabled (bit 0), or when the
  TX FIFO is empty with TX-interrupt enabled (bit 1).

### `arm/gic.rs:23` — `GICD_ICFGR_BASE` never used

The GIC interrupt-configuration registers (`ICFGR`, 0xC00–0xCFF)
control edge vs. level triggering per IRQ.  The constant is defined
but the `transact()` handler doesn't match on the ICFGR range.

**Fix:** Add an `ICFGR` handler in the GIC `transact()` match block.
On read, return the current trigger configuration for the 16-IRQ bank.
On write, update a `config: Vec<u8>` field (2 bits per IRQ:
edge/level).  This is needed for Linux GIC driver probe (`irq_set_type`).

### `arm/pl061.rs:12` — `GPIODATA` never used

PL061 data register accesses use address-line masking (bits [9:2]
select which GPIO bits are affected).  The constant is defined but
the `transact()` handler uses a raw `offset < 0x400` range check
instead.

**Fix:** Use `GPIODATA` as the range start in the match arm.  Replace
the magic `0x400` with `GPIODATA..GPIODIR` range pattern and use the
offset bits [9:2] as the access mask, matching the PL061 spec (DDI0190
§3.3.1).

### `arm/sp804.rs:27` — `CTRL_32BIT` never used

The SP804 timer control register bit 1 selects 16-bit vs 32-bit
counter mode.  The current implementation always runs in 32-bit mode.

**Fix:** Check `CTRL_32BIT` when processing timer ticks: if the bit is
clear, mask the counter to 16 bits (`value & 0xFFFF`) on each
decrement and wrap at 0xFFFF instead of 0xFFFF_FFFF.  This makes the
timer model faithful to the spec for 16-bit mode.

### `arm/sp805.rs:21` — `CTRL_RESEN` never used

The SP805 watchdog reset-enable bit controls whether a watchdog bark
causes a system reset or just an interrupt.

**Fix:** In the watchdog expiry path (inside `tick()`), check
`CTRL_RESEN`.  If set, emit `DeviceEvent::Log { level: Error,
message: "watchdog reset" }` (or a new `DeviceEvent::SystemReset`
variant).  If clear, emit only `DeviceEvent::Irq`.

### `arm/sysregs.rs:29,33,34` — `SYS_PCICTL`, `SYS_CLCDSER`, `SYS_BOOTCS` never used

These are RealView system-register offsets for PCI control, CLCD
serial interface, and boot chip-select.  The `transact()` handler
doesn't match on them.

**Fix:** Add stub read/write handlers that return sensible defaults:
- `SYS_PCICTL` → read returns 0 (no PCI); writes are no-ops.
- `SYS_CLCDSER` → read returns 0; writes are no-ops.
- `SYS_BOOTCS` → read returns 0 (default boot source); writes are
  no-ops.

These enable the RealView Linux BSP to probe without hitting the
unknown-register fallback.

### `loader.rs:54–55` — `DeviceFactory.name` and `version` never read

`DeviceFactory` stores `name` and `version` but they are only written
during `register()` and never queried.

**Fix:** Expose them through the `DynamicDeviceLoader` public API.
Add `pub fn list_factories(&self) -> Vec<(&str, &str)>` that returns
`(name, version)` pairs for each registered factory.  This is useful
for the `helm` CLI `--list-devices` subcommand and for debug logging.

### `proto/amba.rs:33,122` — `AhbBus.window_size` and `ApbBus.window_size` never read

Both bus types store `window_size` but only use it during construction
to set `region.size`.  After that the field is dead.

**Fix:** Add a `pub fn window_size(&self) -> u64` accessor on both
`AhbBus` and `ApbBus`.  The device-tree generator and platform debug
dumps need to query the bus window size.  Alternatively, make the
`region` field pub and access `.region.size` directly — but an
accessor is cleaner.

### `virtio/blk.rs:90,97` — `VirtioBlk.config` and `serial` never read

`VirtioBlk` stores the typed `VirtioBlkConfig` struct and the
`serial` byte array, but the VirtIO config-space read path uses
`config_bytes: Vec<u8>` (a pre-serialised copy) instead.

**Fix:** Remove the separate `config_bytes` field and serialize
`config` on demand:
```rust
fn config_read(&self, offset: u32) -> u8 {
    let bytes = self.config.to_bytes(); // new method
    bytes.get(offset as usize).copied().unwrap_or(0)
}
```
Add `VirtioBlkConfig::to_bytes(&self) -> Vec<u8>` that serialises the
struct in virtio-spec byte order.  This makes `config` the single
source of truth and allows `serial` to be read via the config space.

### `virtio/blk.rs:186` — `status` assigned but never read

The `status` variable computes the virtio completion status for each
request but is never written back to the guest descriptor chain.

**Fix:** Write the status byte to the last descriptor in the chain
(virtio-blk spec §5.2.6: the device writes a 1-byte status to the
final descriptor).  After the match:
```rust
let status_byte = status as u8;
// Write status to last descriptor's buffer
if let Some(last) = chain.last() {
    guest_mem.write(last.addr, &[status_byte]);
}
```

### `virtio/net.rs:142` — `packet` unused in `while let`

The RX drain loop peeks at the front of `rx_queue` but never reads
the packet data to copy it into the guest descriptor.

**Fix:** Use `packet` to copy the received frame into the guest
virtqueue buffer.  Inside the loop body, memcpy `packet` into the
descriptor chain obtained from `q.pop_avail()`, then report the
correct byte count to `q.push_used()`.

### `virtio/{crypto,gpio,gpu,iommu,pmem,sound,video,vsock}.rs` — `config` never read

All eight VirtIO device stubs store a typed config struct that is
never accessed.  The pattern is identical across all of them.

**Fix:** Add the VirtIO config-space read/write interface to each
device (matching the `VirtioDeviceBackend` trait's
`config_read`/`config_write` methods).  Serialize the `config` struct
to a byte array in `config_read()`.  This makes each device return its
spec-defined configuration to the guest driver during probe.

### `virtio/can.rs:6`, `virtio/fs.rs:8` — unused `Serialize`/`Deserialize`

The serde derives were imported for the config structs but the structs
don't derive `Serialize`/`Deserialize` — they use manual construction.

**Fix:** Add `#[derive(Serialize, Deserialize)]` to the config structs
(`VirtioCanConfig`, `VirtioFsConfig`).  This enables checkpoint/restore
of device state via serde, which is needed for the FS-mode checkpoint
workflow.

---

## helm-syscall

### `handler.rs:211` — `sys_statfs` never called

The method is fully implemented but the `handle()` match dispatches
`nr::STATFS` to a hard-coded stub (`Ok(neg(2))`) at line 90 instead.

**Fix:** Replace the stub arm with a call to `self.sys_statfs(args,
mem)`.  Change `nr::STATFS => Ok(neg(2))` to
`nr::STATFS => self.sys_statfs(args, mem)`.

### `handler.rs:215–216` — `flags` and `mode` unused in `sys_statfs`

`statfs(2)` takes only a path and a buffer pointer — it has no
`flags` or `mode` parameters.  These were likely copy-pasted from
`sys_openat`.

**Fix:** Remove the `flags` and `mode` bindings from `sys_statfs`.
The syscall only uses `args[0]` (path) and `args[1]` (buf).

### `handler.rs:235–236` — `flags` and `mode` unused in `sys_readlinkat`

`readlinkat(2)` takes `(dirfd, pathname, buf, bufsiz)` — no flags or
mode.  These were copy-pasted from another handler.

**Fix:** Remove the `flags` and `mode` bindings.  The function already
uses `args[1]` (path), `args[2]` (buf), and `args[3]` (bufsiz)
correctly, but the `flags`/`mode` re-bindings of args[2]/args[3] are
redundant and misleading.

### `handler.rs:424–425` — `mem` and `uaddr` unused in `sys_futex`

`sys_futex` currently returns stub values for FUTEX_WAIT/WAKE without
actually reading the futex word from guest memory.

**Fix:** Implement the FUTEX_WAIT path properly: read the 32-bit
value at `uaddr` from `mem`, compare it with `args[2]` (expected
value).  If they match, return `-EAGAIN` (in single-threaded SE mode,
no one will wake us) or `-ETIMEDOUT` if a timeout was provided.  If
they don't match, return `-EAGAIN` immediately.  This uses both `mem`
and `uaddr`.

---

## helm-llvm

### `accelerator.rs:7` — unused import `FunctionalUnitPool`

`AcceleratorBuilder::build()` constructs a `FunctionalUnitPool` via
the builder and passes it to `InstructionScheduler::new()`, but the
pool type itself is never named directly.

**Fix:** Add a `pub fn functional_units(&self) -> &FunctionalUnitPool`
accessor to `Accelerator` so users can inspect the pool configuration
at runtime (e.g. for stats reporting or Python bindings).  This
naturally uses the import.

### `accelerator.rs:47` — `Accelerator.config` never read

The `AcceleratorConfig` is stored but never queried after construction.

**Fix:** Add `pub fn config(&self) -> &AcceleratorConfig` accessor.
Expose clock period and scratchpad size for statistics reporting and
for the `AcceleratorDevice` register map (`REG_FU_CONFIG`).

### `ir.rs:7` — unused import `Error`

The `Error` type is imported but all error construction uses `?`
propagation from sub-calls that already return `Result`.

**Fix:** Add explicit error returns in the parser validation paths.
For example, `LLVMModule::validate()` (to be added) should return
`Error::Parse(...)` when a module has no functions, uses unsupported
types, or has unresolved references.  This validates IR before
scheduling.

### `scheduler.rs:7–9` — unused imports `Error`, `FunctionalUnitType`, `LLVMInstruction`, `LLVMValue`

The scheduler currently operates on pre-lowered `MicroOp`s and never
touches the LLVM IR types or fine-grained FU types directly.

**Fix:** Implement `InstructionScheduler::schedule_function()` which
accepts an `&LLVMFunction`, calls `llvm_to_micro_ops()` on each basic
block, maps each MicroOp to a `FunctionalUnitType`, and reports errors
via `Error`.  This replaces the current pattern where the caller must
manually lower IR before calling `schedule_basic_block()`.

### `functional_units.rs:6` — unused `VecDeque`

`VecDeque` was imported for a pipelined issue-queue model that uses
`Vec` instead.

**Fix:** Replace the `Vec`-based `compute_queue` in
`FunctionalUnitPool` with a `VecDeque` for O(1) front-pop semantics.
The current `Vec::remove(0)` is O(n) on every completed unit;
`VecDeque::pop_front()` is the correct data structure for a FIFO
completion queue.

### `micro_op.rs:303` — `ty` unused in `Add` pattern

The LLVM type information (`ty`: i32, i64, float, etc.) is available
in the `Add` instruction but not propagated to the `MicroOp`.

**Fix:** Use `ty` to select the correct `FunctionalUnitType`.  Map
integer types to `IntAdder`, float types to `FPAdder`.  Also use the
bit width to set the `MicroOp`'s operand size, which affects latency
in width-dependent functional-unit models.

### `micro_op.rs:483` — `value` unused in `Ret` pattern

The return value of the LLVM `ret` instruction is discarded.

**Fix:** If `value` is `Some(val)`, emit a `MicroOp::Move` that
copies the return value to a designated output register (e.g. reg 0).
This allows the caller (`AcceleratorDevice`) to read the accelerator's
computed result from the output register after `run()` completes.

### `micro_op.rs:558` — `args` unused in `Call` pattern

Function call arguments are not lowered to micro-ops.

**Fix:** Emit `MicroOp::Move` instructions to copy each argument into
the callee's parameter registers (following a simple calling
convention: arg0 → reg0, arg1 → reg1, etc.).  For inlined accelerator
functions, this sets up the data flow for the callee's basic blocks.

---

## helm-isa

### `exec.rs:15` — unused import `TtbrSelect`

`TtbrSelect` discriminates TTBR0 vs TTBR1 in translation results but
is not used in the current page-table-walk integration.

**Fix:** Use `TtbrSelect` in `exec_at()` (Address Translate
instruction) to record which TTBR was used for the walk.  Write the
result into PAR_EL1 bit [11] (the "S" bit: 0=TTBR0, 1=TTBR1).  The
current AT implementation does not set this bit.

### `exec.rs:26–27,31,34` — unused params in `MmuDebugHook` default methods

All three `MmuDebugHook` trait methods have default empty impls whose
parameters are unused.

**Fix:** These are **trait default method bodies** — the parameters
are intentionally unused because implementors override them.  The
correct fix is a crate-level attribute:
```rust
#![allow(unused_variables)]  // trait default bodies
```
Or, more surgically, add `#[allow(unused_variables)]` on each default
method.  This is the standard Rust pattern for trait hooks (cf.
`serde::Visitor`).

### `exec.rs:1237,1373` — unreachable pattern `SP_EL3`

`sysreg::SP_EL2` and `sysreg::SP_EL3` are both defined as
`sysreg(3, 6, 4, 1, 0)` in `sysreg.rs` (lines 128 and 157).  They
are **identical values**, so the `SP_EL3` match arm is unreachable.

**Fix:** Correct the `SP_EL3` encoding.  ARMv8 does not define a
user-accessible `SP_EL3` system register (EL3 stack pointer is not
directly addressable as a sysreg in the standard encoding space).
If the simulator needs to model EL3 SP access, use a synthetic
register ID that doesn't collide with `SP_EL2`.  Alternatively, if
EL3 is not modelled, remove the `SP_EL3` constant and the
corresponding match arms.

### `exec.rs:1493` — `scr` unused in `route_sync_exception`

`scr_el3` (Secure Configuration Register) is read but never used to
influence exception routing.  In ARMv8, SCR_EL3 bits determine whether
exceptions from EL1/EL2 are routed to EL3.

**Fix:** Implement SCR_EL3-based routing.  In the `from_el == 1`
branch, check `scr & SCR_EA` (bit 3) for SError routing to EL3 and
`scr & SCR_IRQ` (bit 1) / `scr & SCR_FIQ` (bit 2) for interrupt
routing.  In the `from_el == 2` branch, check whether `scr &
SCR_EEL2` allows EL2 exceptions to be trapped to EL3.

### `exec.rs:1800` — unreachable TLBI pattern

The TLBI dispatch has overlapping arms: `(4, 3, 4) | (4, 7, 4)` is
matched at line 1771 (ALLE1(IS)), and `(4, 3, 5) | (4, 7, 5)` is
matched at line 1785 (VAE2(IS)), so the combined arm at line 1800
(IPAS2E1(IS)) is fully shadowed.

**Fix:** Reorder the TLBI match arms so IPAS2E1(IS) (`(4, 0, 4) |
(4, 4, 4)`) uses its correct op1/crm/op2 encoding from the ARM spec
(ARM DDI 0487, §D19.2.119).  The current encoding `(4, 3, 4)` is
ALLE1IS, not IPAS2E1IS.  The correct IPAS2E1 encoding is
`op1=4, CRm=0, op2=1` → `(4, 0, 1)`.

### `exec.rs:1821` — `is_write` unused in `exec_at`

The AT instruction direction bit is decoded but not forwarded to the
translation walk.

**Fix:** Pass `is_write` to the `mmu::translate()` call so the walker
can check AP (Access Permission) bits for write faults.  The current
code always performs a read-side permission check regardless of
whether the AT was S1E1R or S1E1W.

### `exec.rs:2363` — `elem_size` unused (SIMD LD/ST)

The SIMD structure load/store decoder extracts `elem_size` (bits
[11:10]) but doesn't use it to select the element width.

**Fix:** Use `elem_size` to determine the per-element byte width
(`1 << elem_size`) for structure load/store operations.  Currently
the code assumes 8-byte elements; the correct interpretation is
byte/half/word/double per the encoding.

### `exec.rs:3077` — `esize` unused (UMOV/SMOV)

The DUP/MOV element-size is computed but not used.

**Fix:** Use `esize` to perform sign-extension (for SMOV) or
zero-extension (for UMOV) to the correct destination width.  Currently
the code zero-extends to 64 bits regardless of `esize`.  For SMOV with
`esize=1` (byte), the value should be sign-extended from 8 bits.

---

## helm-engine

### `se/linux.rs:9` — unused import `SchedAction`

`SchedAction` is the return type of `Scheduler::tick()` (run next
thread / block / exit).  It is imported but the SE runner doesn't
call `Scheduler::tick()` yet — multi-thread scheduling is stubbed.

**Fix:** Implement the multi-thread scheduling loop in `exec_interp` /
`exec_tcg`.  After each syscall that affects thread state (clone,
exit, futex), call `sched.tick()` and match on `SchedAction::Switch`,
`SchedAction::Block`, `SchedAction::Exit` to context-switch between
threads.

### `se/thread.rs:11` — unused `HashMap` and `VecDeque`

The thread scheduler module imports these collections but uses a
`Vec<ThreadState>` internally.

**Fix:** Migrate the scheduler internals:
- Use `HashMap<Tid, ThreadState>` for O(1) thread lookup by ID
  (currently linear scan).
- Use `VecDeque<Tid>` as the run queue for O(1) round-robin
  scheduling (pop front, push back).

### `se/session.rs:20` — unused import `handle_sc`

`handle_sc` (syscall handler) is imported but `SeSession`'s
`run_inner` dispatches syscalls inline instead of calling the shared
handler.

**Fix:** Refactor `SeSession::run_inner` to call `handle_sc()` on
`HelmError::Syscall` instead of duplicating the inline syscall
dispatch.  This unifies the syscall path between the standalone runner
(`run_aarch64_se`) and the session-based runner (`SeSession`).

### `se/linux.rs:198` — `devices` unused in `exec_tcg`

The `devices` parameter is threaded through for MMIO-aware execution
but the TCG backend does not perform device bus lookups.

**Fix:** Wire device-bus MMIO dispatch into the TCG execution loop.
When a translated block contains a memory access to an MMIO region,
call `devices.read_fast()` / `write_fast()` instead of the normal
memory path.  This makes TCG mode functional for FS-mode simulation
with devices.

### `symbols.rs:67` — `e_shstrndx` unused

The ELF section-header string-table index is parsed from the ELF
header but never used to resolve section names.

**Fix:** Use `e_shstrndx` to locate the `.shstrtab` section.  Then
use it to look up section names by index, enabling the symbol loader
to find `.symtab` and `.strtab` by name rather than by type heuristic.
This makes the loader robust against ELF files with unusual section
ordering.

---

## helm-cli

### `helm_system_aarch64.rs:784` — `build_plugin_registry` never used

The function is fully implemented but the `main()` function constructs
the plugin registry inline instead of calling it.

**Fix:** Refactor `main()` to call `build_plugin_registry(&plugin_names)`
instead of the inline plugin setup.  This consolidates the plugin
creation logic in one place and makes the `--plugin` CLI flag work
through the shared path.

### `helm_system_aarch64.rs:818` — `count_nodes` never used

A utility function for counting FDT nodes, intended for debug logging.

**Fix:** Call `count_nodes()` after DTB resolution to log the total
node count: `eprintln!("HELM: DTB has {} nodes", count_nodes(&root))`.
This provides useful diagnostics during platform bring-up.

### `helm_arm.rs:81` — `PlatformCfg.isa` never read

The ISA field is deserialized from YAML/JSON config but never used to
select the ISA frontend.

**Fix:** Use `platform_cfg.isa` to select the `IsaKind` when
constructing `PlatformConfig`.  Map `"aarch64"` → `IsaKind::Arm64`,
`"riscv64"` → `IsaKind::RiscV64`, `"x86_64"` → `IsaKind::X86_64`.
Error on unrecognised ISA strings.

### `helm_arm.rs:101–103` — `CoreCfg.lq_size` and `sq_size` never read

Load-queue and store-queue sizes are deserialized but not mapped to
`CoreConfig`.

**Fix:** Add `lq_size` and `sq_size` fields to `helm_core::CoreConfig`
and populate them from `CoreCfg` during config conversion.  The
pipeline model (`helm-pipeline`) needs these to size the load/store
queues in the OoO scheduler.

### `helm_arm.rs:161–167` — `CacheCfg` fields never read

All four fields (`size`, `associativity`, `latency_cycles`,
`line_size`) are deserialized but never mapped to
`helm_core::CacheConfig`.

**Fix:** Implement the `CacheCfg` → `CacheConfig` conversion:
```rust
fn to_cache_config(cfg: &CacheCfg) -> CacheConfig {
    CacheConfig {
        size_bytes: parse_size(&cfg.size),
        associativity: cfg.associativity,
        latency_cycles: cfg.latency_cycles,
        line_size: cfg.line_size,
    }
}
```
Call this when building the `MemoryConfig` from `MemoryCfg`.
