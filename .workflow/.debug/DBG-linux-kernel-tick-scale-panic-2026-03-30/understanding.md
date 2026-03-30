# Understanding Document

**Session ID**: DBG-linux-kernel-tick-scale-panic-2026-03-30
**Bug Description**: `--tick-scale=100` causes a Linux kernel panic during FS boot on `arm-virt`
**Started**: 2026-03-30

---

## Exploration Timeline

### Iteration 1 - Resume and Reproduction

#### Current Understanding

- The old failure reproduced reliably with `--tick-scale=100`.
- The guest panic was:
  - `Unable to handle kernel paging request at virtual address ffffffbfbeaf0000`
  - `Internal error: Oops: 0000000086000005 [#1] PREEMPT SMP`
  - `pc == lr == 0xffffffbfbeaf0000`
- The non-`tick-scale` boot path did not hit this panic and instead progressed further before stalling in later boot code.

#### Evidence

- Existing logs in `tmp/boot-release-tick100-60s.log` and `tmp/boot-tick100.log` showed the panic near guest time `22.4s`.
- A trace window around the failure showed the last stable control-flow sequence entering the timer/scheduler IRQ path:
  - `sched_tick -> task_tick_idle`
  - No traced return from `task_tick_idle` before the bogus `pc/lr` fault.
- Symbolization of the last traced branches:
  - `0xffffffc0800db8a8` = `sched_tick`
  - `0xffffffc0800e93c8` = `task_tick_idle`
  - `0xffffffc080d0fe40` = `arch_counter_get_cntvct`

#### Corrected Understanding

- Initial suspicion: a bad indirect callback target in `sched_tick`.
- Updated conclusion: the stronger signal is a corrupted interrupt return path after entering the timer tick handler.
- The regression is tied to global `fs.tick += tick_scale`, not to the base boot path.

### Iteration 2 - Fix Direction

#### Current Understanding

- Global tick scaling is safe for early delay loops.
- It becomes unsafe once the guest has an active, unmasked generic timer:
  - simulated time advances much faster than retired instructions
  - the timer IRQ path can be re-driven before the kernel finishes handling the current tick
  - that correlates with the observed return-path corruption

#### Implemented Change

- Added `effective_tick_step()` in `runtime/helm-engine/src/fs.rs`.
- Behavior:
  - use `fs.tick_scale` while no generic timer IRQ is live
  - fall back to `1` tick per instruction once `CNTP` or `CNTV` is enabled and unmasked

#### Verification Evidence

- `cargo build -p helm-cli --release` succeeded in `.worktrees/debug-boot`.
- A rebuilt release run of:
  - `target/release/helm-system-aarch64 --sim-trace=null: examples/fs/boot_rpi_full.py --tick-scale 100 --max-insns 120000000`
  no longer emitted the old panic markers (`Kernel panic`, `Oops`, `Unable to handle`).
- The run progressed past the old crash point into:
  - `clocksource: Switched to clocksource arch_sys_counter`
  - `hrtimer: interrupt took 12512000 ns`

---

## Current Consolidated Understanding

The `tick_scale` crash was caused by globally accelerating the virtual counter even after Linux had enabled the generic timer interrupt path. That distorted the relationship between kernel work and timer expiry enough to corrupt the interrupt return sequence. The current mitigation preserves early delay-loop acceleration but disables scaled ticking once a live generic timer IRQ exists.
