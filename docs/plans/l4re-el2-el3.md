# Plan: L4Re EL2/EL3 Bring-Up and Linux VM Support

> **Status:** Design pass started — 2026-04-09
> **Goal:** Define a concrete implementation plan for the AArch64 privilege, virtualization, and machine-model work required for Helm to boot an L4Re/Fiasco.OC-style microkernel payload at EL2 and then run a Linux guest VM beneath it
> **Primary references:** `runtime/helm-arch/src/aarch64/*`, `runtime/helm-engine/src/fs.rs`, `runtime/helm-engine/src/platform/arm_virt.rs`, and `../helm.git`
> **External behavioral target:** QEMU `virt` semantics where `virtualization=on` causes direct kernel entry at EL2 and `secure=on` causes direct kernel entry at EL3
> **Completion gate:** Helm can boot an EL2 payload on `arm-virt`, that payload can configure second-stage translation and guest trap routing, and a Linux guest running below it can make forward progress with correct exception, MMU, timer, and interrupt behavior

---

## Implementation Progress

### 2026-04-09 — First cut implemented

The following first-cut EL2 virtualization support is now in tree:

- `VTTBR_EL2`, `VTCR_EL2`, and `HPFAR_EL2` were added to the live AArch64 architectural state
- EL2 sysreg read/write support for those registers was added
- the live AArch64 MMU path now performs stage-2 translation for EL0/EL1 accesses when `HCR_EL2.VM=1`
- stage-2 fetch and data faults now route to EL2 in the FS execution path
- focused regression tests now cover:
  - EL2 virtualization sysregs
  - stage-2 translation success/failure in `helm-arch`
  - stage-2 fetch/data fault routing in `helm-engine`

The following small EL3 boot-state improvement is also now in tree:

- `arm-virt` boot-vCPU construction now has an explicit `boot_el == 3` path
- EL3 boot tests now verify `SP_EL3` initialization and EL2/EL3 feature visibility bits for the boot CPU

The following second-slice work is now also in tree:

- `AT S1E1*` and `AT S12E1*` are now distinguished in the FS executor
- plain `S1E1*` probes remain stage-1 only even when `HCR_EL2.VM=1`
- `S12E1*` probes now report the combined stage-1 + stage-2 result
- a direct-kernel boot EL override now threads through:
  - `arm-virt` system construction
  - the Rust engine load-kernel APIs
  - the Python `load_kernel()` binding
  - the FS example scripts
- the example scripts now expose QEMU-like user-facing shorthands:
  - `--virtualization` -> EL2 direct entry
  - `--secure` -> EL3 direct entry

### 2026-04-09 — First EL2 payload run result

Using:

```bash
target/debug/helm-system-aarch64 examples/fs/virt.py \
  --kernel assets/aarch64/boot/l4re_hello-2_arm_virt.elf \
  --boot-el 2 \
  --max-insns 2000000
```

Helm now successfully reaches:

- L4 bootstrapper output
- module relocation and loading
- Fiasco/L4Re microkernel welcome banner
- `Hello from Startup::stage2`

This is an important milestone because it shows:

- direct ELF entry at EL2 is working
- the first-cut EL2 virtualization substrate does not immediately crash the payload
- the next blockers are now likely deeper kernel/hypervisor runtime dependencies rather than basic EL2 entry, stage-2 fetch, or obvious sysreg absence

### 2026-04-09 — Trace-driven EL2 sysreg follow-up

The first L4Re / Fiasco.OC VM-hosting trace exposed an additional early-boot EL2 sysreg gap beyond the original first cut:

- `MDCR_EL2`, `CPTR_EL2`, and `HSTR_EL2` are now part of the live AArch64 architectural state
- EL2 `MRS`/`MSR` support for those registers now exists in the main sysreg helpers
- focused `helm-arch` regression tests now cover read/write persistence for all three registers
- after rebuilding `helm-system-aarch64`, the earlier `s3_4_c1_c1_{1,2,3}` stub traffic disappears from the `l4re_vm-basic_arm_virt.elf` boot trace
- a follow-up GICv3 compatibility fix stops the redistributor from advertising LPI capability bits before ITS/LPI support exists
- with that capability fix in place, the simulator no longer falsely advertises redistributor LPI support, although the EL2 payload still warns that the current GICv3 model does not implement real LPI / ITS support
- a later trace-driven slice added `CNTHCTL_EL2`, `CNTHP_CTL_EL2`, `CNTHP_CVAL_EL2`, `CNTHP_TVAL_EL2`, and `CNTVOFF_EL2` to the live architectural state and EL2 sysreg path
- after rebuilding the simulator, the earlier EL2 hypervisor-timer sysreg stubs disappear from the `vm-basic` boot trace
- the next visible blocker after that timer cleanup was traced to `sigma0` reading `TPIDRRO_EL0` and storing through it while Helm still hardwired that sysreg to zero
- extracting the embedded `sigma0` ELF and disassembling the faulting PC showed the exact sequence `mrs x0, tpidrro_el0; str q31, [x0]`, confirming the null boot-state source rather than random memory corruption
- `TPIDRRO_EL0` is now modeled as live architectural state, writable by privileged code, and seeded during `arm-virt` boot-vCPU construction with a small per-vCPU scratch page above the initial stack
- with that boot-state fix in place, the previous `sigma0` low-address abort disappears and the same `vm-basic` EL2 run reaches `SIGMA0: Hello!` and the resource-map dump
- the next post-`sigma0` stall was traced to an EL2 kernel waiting in a `WFI` loop after programming `CNTHP_CVAL_EL2`, while Helm still only delivered `CNTP` and `CNTV` timer PPIs
- `CNTHP` timer expiry is now evaluated in the FS loop, included in timer countdown / WFI fast-forward logic, and injected into GICv2/GICv3 as hypervisor-timer PPI 10 / INTID 26
- with that timer-delivery fix in place, CPU0 no longer parks at the old `WFI` PC after `sigma0`; the run advances into later kernel timer / exception paths instead of stalling immediately

This keeps the work aligned with the plan's Phase 8 guidance: use real EL2 payload traces to close the next concrete architectural hole instead of guessing at larger virtualization features.

### 2026-04-23 — Virtual GIC hypervisor sysregs become live state

Disassembly of the embedded Fiasco/L4Re modules in
`l4re_vm-basic_arm_virt.elf` confirmed that `Fiasco` actively reads and
writes the GICv3 hypervisor virtual interface (`ICH_AP*R*_EL2`,
`ICH_HCR_EL2`, `ICH_VMCR_EL2`, `ICH_LR0..15_EL2`) as part of vCPU context
save/restore, while Helm previously dropped every write and returned zero
for every read. Any vCPU resumed after a context switch would silently
lose its virtual interrupt state, so this slice converts those registers
into live architectural state:

- `Aarch64ArchState` now carries `ich_ap0r_el2[4]`, `ich_ap1r_el2[4]`,
  `ich_hcr_el2`, `ich_vmcr_el2`, and `ich_lr_el2[16]`
- the EL2 sysreg helpers now read/write those fields directly
- `ICH_VTR_EL2` now reports a 16-LR / 5-bit-priority / A3V=1 layout
  consistent with the new LR storage instead of always returning zero
- `ICH_ELRSR_EL2` now derives the per-LR empty bit from each LR's State
  field instead of always reporting "all LRs occupied"
- `ICH_MISR_EL2` and `ICH_EISR_EL2` continue to report quiescent state
  because Helm does not yet evaluate maintenance interrupts; this is an
  intentional next-slice boundary
- focused `helm-arch` regression tests now cover read/write persistence
  for `ICH_HCR_EL2`, `ICH_VMCR_EL2`, both `ICH_AP0R0` / `ICH_AP1R0`,
  the low and high `ICH_LR` banks, the `ICH_VTR_EL2` feature constant,
  and the `ICH_ELRSR_EL2` derivation
- the existing `vm-basic` and `hello-2` boot traces remain unchanged in
  observable progress, confirming no regression on currently-working
  payloads; the wired ICH state will become observable once a future
  trace-driven slice exercises actual vCPU context switches

### Still outstanding after the first cut

The current implementation is intentionally conservative and does **not** yet include:

- VMID-aware TLB behavior or virtualization-specific fast paths
- full machine-property-style `secure=on` / `virtualization=on` platform configuration beyond the current direct-kernel boot EL override
- virtual interrupt evaluation, maintenance interrupt generation, and
  guest-side delivery against the now-live `ICH_LR*` / `ICH_VMCR_EL2`
  state
- L4Re payload integration and trace-driven guest validation

The next slices should build on the first-cut substrate rather than reopening the basic stage-2 design.

---

## Executive Summary

Helm already has a meaningful portion of the AArch64 privilege model:

- EL1/EL2/EL3 architectural state exists
- exception routing across EL1/EL2/EL3 exists
- `ERET`, `HVC`, `SMC`, and trapped sysreg flows exist
- EL2 boot entry is already partly supported by the current kernel loader and `arm-virt` boot path
- stage-1 MMU translation exists for EL1, EL2, and EL3 execution regimes

What Helm does **not** yet have is a full EL2 hypervisor execution model.
The dominant gap is the absence of **second-stage translation and EL2 virtualization fault handling**.
Without that, Helm can enter EL2, but it cannot run a real EL2 hypervisor that owns an EL1 guest.

For the requested L4Re path, the required work falls into three layers:

1. **Architectural EL2/EL3 correctness**
   Helm must fully model the registers, translation regimes, and fault routing required by EL2 hypervisor software and EL3 firmware/monitor entry.

2. **Machine-model compatibility**
   Helm's `arm-virt` machine must expose QEMU-like boot semantics for EL1/EL2/EL3 startup, especially direct `-kernel` entry at EL2 for `virtualization=on` and at EL3 for `secure=on`.

3. **L4Re guest-hosting capability**
   Helm must be good enough for an EL2 microkernel/hypervisor payload to own an EL1 guest Linux VM, which means second-stage MMU, virtual interrupt handling, timer behavior, and enough device/machine contract fidelity for the guest stack to boot.

This document defines the missing pieces, the implementation order, the tests that must gate each phase, and the practical boundaries between "architecturally complete enough" and "L4Re Linux VM complete enough".

---

## Scope

## In Scope

- AArch64 EL1/EL2/EL3 execution support in full-system mode
- EL2 virtualization state and second-stage translation
- EL2 fault routing for stage-2 translation and permission failures
- EL3 startup and secure-world boot entry state sufficient for `secure=on` semantics
- `arm-virt` machine configuration and loader behavior needed to start a payload at EL1, EL2, or EL3
- hypervisor-relevant timer, interrupt, and trap plumbing needed by an EL2 payload hosting an EL1 Linux guest
- implementation planning for the L4Re-to-Linux-VM path

## Out of Scope for the First Complete Slice

- a full TrustZone security architecture
- a full secure monitor firmware implementation
- ACPI/UEFI firmware boot flows
- KVM acceleration or host-assisted nested virtualization
- full device pass-through or IOMMU assignment for guests
- a complete L4Re userspace distro integration

The first objective is **functional EL2 hypervisor capability** on the current software CPU model, not every optional ARMv8 security or firmware feature.

---

## Current State

## Already Implemented

### Privilege and exception model

The following already exist in the current tree:

- `Aarch64ArchState` includes EL1, EL2, and EL3 register state in `runtime/helm-arch/src/aarch64/arch_state.rs`
- synchronous exception routing across EL1/EL2/EL3 exists in `runtime/helm-arch/src/aarch64/exception.rs`
- `ERET` return flow exists and has tests
- `HVC`, `SMC`, and trapped sysreg flows exist
- EL2/EL3 regression coverage exists in `runtime/helm-arch/src/aarch64/tests/exec_el2_el3.rs`

### Stage-1 MMU

Helm already supports:

- EL1 stage-1 translation
- EL2 stage-1 translation, including non-VHE and VHE split/single-space handling
- EL3 stage-1 translation
- software TLB handling
- TLB invalidation on sysreg changes and TLBI-like flows

This is implemented in `runtime/helm-arch/src/aarch64/mmu.rs` and consumed by `runtime/helm-engine/src/fs.rs`.

### EL2 boot entry

The loader and machine boot path already understand the kernel's requested boot EL:

- `runtime/helm-engine/src/loader/arm64_image.rs` carries `LoadedKernel.boot_el`
- `runtime/helm-engine/src/platform/arm_virt.rs` already has `build_boot_vcpu(..., boot_el, ...)`
- current tests already verify that a boot vCPU can start at EL2

This is an important distinction:
Helm already knows how to **start at EL2**.
It does **not** yet know how to **behave like a hypervisor once there**.

---

## Missing Pieces

## 1. EL2 virtualization state is incomplete

The current `Aarch64ArchState` includes:

- `HCR_EL2`
- `SCTLR_EL2`
- `TCR_EL2`
- `TTBR0_EL2`
- `TTBR1_EL2`
- `VBAR_EL2`
- `ELR_EL2`
- `SPSR_EL2`
- `ESR_EL2`
- `FAR_EL2`

But it does **not** yet include the core second-stage virtualization state:

- `VTTBR_EL2`
- `VTCR_EL2`
- `HPFAR_EL2`

Without these registers, Helm cannot represent:

- the stage-2 translation table root
- the stage-2 translation configuration
- the IPA-side fault reporting required when a guest translation fails

This is the central architectural gap.

## 2. Second-stage translation is absent from the live MMU path

The current MMU implementation in `runtime/helm-arch/src/aarch64/mmu.rs` is stage-1 only.
It can translate:

- VA -> PA for EL1
- VA -> PA for EL2
- VA -> PA for EL3

It cannot yet perform:

- EL1 VA -> IPA through stage-1
- IPA -> PA through stage-2 when `HCR_EL2.VM=1`

This means the current FS runtime has no real virtualization MMU regime.
An EL2 payload can program sysregs, but the guest memory model below it does not exist.

## 3. Stage-2 faults are not routed to EL2

The FS execution path currently treats memory faults as ordinary EL0/EL1 instruction or data aborts.
For virtualization, when an EL1 guest access fails at stage-2, Helm must:

- report the fault to EL2
- populate `FAR_EL2`
- populate `HPFAR_EL2`
- set the correct abort syndrome
- resume execution at the EL2 vector, not the guest EL1 vector

That routing is not currently modeled.

## 4. EL2 virtualization sysregs are not fully readable/writable

The sysreg helpers in `runtime/helm-arch/src/aarch64/execute/helpers.rs` do not yet expose:

- `VTTBR_EL2`
- `VTCR_EL2`
- `HPFAR_EL2`

As a result, even once MMU work exists, EL2 software would not be able to manage or observe second-stage state.

## 5. Combined translation probe instructions are incomplete

Hypervisor software often uses `AT` instructions to probe translation state.
Current Helm supports useful `AT` behavior for stage-1, but not the combined virtualization-sensitive cases such as:

- `AT S12E1R`
- `AT S12E1W`

For an EL2 payload, these instructions matter because they test the combined guest translation regime.

## 6. EL3 boot-state initialization is incomplete

The current boot vCPU initialization in `runtime/helm-engine/src/platform/arm_virt.rs` has explicit setup for:

- EL1
- EL2

But not a distinct EL3 startup path.
If the machine starts a payload at EL3, Helm must initialize:

- `SP_EL3`
- `SCTLR_EL3`
- EL3-visible feature state
- secure monitor-facing boot context, as far as the direct-kernel path requires

Today EL3 is present in architectural state and exception flow, but not fully in boot construction.

## 7. Secure vs non-secure execution is not a complete machine model yet

Helm has some framework-level notion of secure transactions, but the FS CPU and memory path do not yet implement a real TrustZone-style secure/non-secure split.

For the immediate target, this matters less than EL2 virtualization.
However, it does matter for claiming compatibility with `secure=on`.

The realistic first milestone is:

- **EL3 entry semantics compatible with QEMU's direct kernel entry**

not:

- **a full TrustZone platform implementation**

## 8. Virtual interrupt handling is only partly sufficient

Physical GICv2/GICv3 device models exist.
That is necessary, but not sufficient, for a guest-hosting EL2 payload.

The likely next hypervisor-facing interrupt gaps after stage-2 MMU are:

- virtual interrupt controller state
- ICC/ICH/ICV behavior for virtualized delivery paths
- enough correctness for an EL2 payload to inject and service guest-visible interrupts

The current code already has some GICv3 sysreg plumbing, but it is not yet equivalent to a complete EL2 virtual interrupt model.

## 9. QEMU-compatible boot policy is implicit, not explicit

The user-visible compatibility goal is:

- `virtualization=on` -> direct kernel entry at EL2
- `secure=on` -> direct kernel entry at EL3

Helm today mostly derives boot EL from the loaded image metadata and internal machine logic.
It does not yet expose a clean machine policy layer that maps:

- default `virt`
- `virtualization=on`
- `secure=on`

to boot-level and feature enablement decisions.

That policy should live in the machine/loader path, not in ad hoc example logic.

---

## Behavioral Target

Helm should converge on the following `arm-virt` direct-kernel startup model:

| Machine mode | Boot EL | Required behavior |
|--------------|---------|-------------------|
| plain virt | EL1 | current Linux direct-boot behavior |
| `virtualization=on` | EL2 | kernel or hypervisor payload sees EL2 implemented and can manage EL1 guests |
| `secure=on` | EL3 | payload sees EL3 implemented and enters with valid EL3 startup state |
| `virtualization=on,secure=on` | EL3 | secure monitor / EL3 entry first, with EL2 implemented beneath it |

This should be interpreted as:

- **boot-entry compatibility**
- **architectural feature visibility**
- **correct register initialization**

and not merely "set `CurrentEL` to a different number".

---

## Design Principles

## 1. Functional correctness before optimization

The first implementation can be conservative:

- software-only
- no VMID-aware fast TLB
- full-flush where needed
- limited virtualization-specialized caching

The first gate is correct boot and trap behavior, not peak performance.

## 2. Reuse the previous-generation reference

`../helm.git` already carries:

- EL2 virtualization state shapes
- stage-2 walk helpers
- combined translation probe behavior

Helm-ng should port the working concepts from that code rather than invent new abstractions without evidence.

## 3. Keep the current FS execution model intact

The current `step_aarch64_fs()` structure in `runtime/helm-engine/src/fs.rs` is sound:

1. IRQ check
2. fetch translation
3. decode
4. execute using translating memory
5. post-step TLB and timing work

The implementation should extend this path, not replace it.

## 4. Separate machine boot policy from architectural execution

The architectural CPU model should not need to know about a user-facing string like `virtualization=on`.
That policy belongs in:

- loader interpretation
- machine configuration
- vCPU boot construction

The CPU model should simply observe the register state it is given.

---

## Proposed Phases

## Phase 1: Complete EL2 virtualization architectural state

### Goal

Add the missing EL2 virtualization registers and expose them through the live AArch64 sysreg path.

### Required changes

#### `runtime/helm-arch/src/aarch64/arch_state.rs`

Add:

- `vttbr_el2: u64`
- `vtcr_el2: u64`
- `hpfar_el2: u64`

Initialize them in `Default`.

#### `runtime/helm-arch/src/aarch64/execute/helpers.rs`

Add `MRS`/`MSR` support for:

- `VTTBR_EL2`
- `VTCR_EL2`
- `HPFAR_EL2`

Ensure TLB flush behavior is correct on writes to:

- `VTTBR_EL2`
- `VTCR_EL2`

### Completion gate

- EL2 software can read/write second-stage control registers
- unit tests cover sysreg visibility and write persistence

---

## Phase 2: Add second-stage MMU support

### Goal

Extend the live AArch64 MMU path so EL1/EL0 accesses run through stage-2 translation when virtualization is enabled.

### Required behavior

When:

- current execution is at EL0 or EL1
- `HCR_EL2.VM == 1`

Helm must perform:

1. stage-1 VA -> IPA
2. stage-2 IPA -> PA

for:

- instruction fetches
- data loads
- data stores
- atomics

### Required changes

#### `runtime/helm-arch/src/aarch64/mmu.rs`

Port or adapt the stage-2 support concepts from `../helm.git`:

- stage-2 configuration parsing
- stage-2 page-table walk
- stage-2 permission checking
- stage-2 fault classification

This should include:

- a stage-2 config type
- stage-2 walk result / fault mapping
- logic to compose stage-1 and stage-2 into a single translation outcome

The code should stay aligned with the current helm-ng MMU style rather than importing the old API wholesale.

### Design note

The first implementation can treat VMID conservatively:

- no VMID-aware live TLB entries
- flush on relevant EL2 virtualization changes

That is acceptable for a first correct functional implementation.

### Completion gate

- EL1 fetch/load/store paths translate successfully through stage-2
- stage-2 permission failures and translation failures are distinguishable
- regression tests exist for success and failure paths

---

## Phase 3: Route stage-2 faults to EL2 correctly

### Goal

Ensure that second-stage translation and permission failures trap to EL2 as guest-visible hypervisor faults.

### Required behavior

For EL1/EL0 accesses under `HCR_EL2.VM=1`, a stage-2 failure must:

- enter EL2, not EL1
- update `FAR_EL2`
- update `HPFAR_EL2`
- generate correct instruction-abort or data-abort syndrome
- preserve the guest restart model expected by EL2 software

### Required changes

#### `runtime/helm-engine/src/fs.rs`

The FS step path must distinguish:

- normal stage-1 guest aborts
- stage-2 virtualization aborts

This likely requires one of:

- richer `MemFault` metadata
- richer `HartException` metadata
- a local FS-layer fault structure for AArch64 MMU results

The key requirement is that the FS loop know whether the fault target is:

- guest EL1
- host EL2

#### `runtime/helm-arch/src/aarch64/exception.rs`

The existing exception entry helper is already close to usable.
The main requirement is that the caller select EL2 as the target when the fault source is second-stage virtualization.

### Completion gate

- failing stage-2 instruction fetch enters EL2 vector space
- failing stage-2 data access enters EL2 vector space
- EL2 fault registers are updated correctly
- tests cover both instruction and data abort cases

---

## Phase 4: Add combined translation probe support

### Goal

Support the translation-probe instructions EL2 software is likely to use when managing guest address spaces.

### Required behavior

Implement the combined translation semantics for:

- `AT S12E1R`
- `AT S12E1W`

and any adjacent variants that are practically required by the target software stack.

### Required changes

#### `runtime/helm-engine/src/fs.rs`

Extend `try_exec_at_instruction()` so the result can represent:

- stage-1 only
- stage-1 + stage-2 combined translation
- correct `PAR_EL1` fault reporting for combined translation failure

### Completion gate

- `AT S12E1*` works under EL2 virtualization
- `PAR_EL1` reflects success/failure appropriately
- tests cover the combined walk behavior

---

## Phase 5: Complete EL3 boot-state initialization

### Goal

Support direct EL3 kernel/payload entry with machine state that is meaningfully consistent and testable.

### Required changes

#### `runtime/helm-engine/src/platform/arm_virt.rs`

Extend `build_boot_vcpu()` to handle `boot_el == 3` explicitly:

- initialize `SP_EL3`
- initialize `SCTLR_EL3`
- preserve EL2 implemented state beneath EL3 when appropriate
- set feature registers consistently

#### `runtime/helm-engine/src/loader/arm64_image.rs`

Preserve the current image-driven boot EL detection, but ensure it composes cleanly with machine policy overrides in later phases.

### Expected first milestone

This phase targets:

- **EL3 direct-entry correctness**

not:

- **full secure monitor implementation**

### Completion gate

- tests verify boot vCPU construction at EL3
- EL3 payload can execute and return via `ERET`

---

## Phase 6: Add explicit machine boot policy for EL1/EL2/EL3

### Goal

Make Helm's `arm-virt` machine expose explicit QEMU-like startup policy instead of relying only on implicit image metadata.

### Required behavior

The machine configuration layer should be able to express:

- plain virt boot
- virtualization-enabled boot
- secure-world boot

with clear mapping to:

- entry EL
- implemented feature visibility
- any machine-specific reserved behavior

### Candidate implementation approach

Add an explicit boot policy enum in the `arm-virt` machine layer, for example:

- `Normal`
- `VirtualizationOn`
- `SecureOn`
- `SecureAndVirtualizationOn`

Then define how this combines with:

- image metadata
- platform defaults
- Python/CLI configuration

### Completion gate

- machine startup policy is explicit and testable
- EL1/EL2/EL3 startup no longer depends on hidden behavior

---

## Phase 7: Hypervisor-visible interrupt and timer completeness

### Goal

Provide enough interrupt/timer correctness for an EL2 payload to host a Linux guest VM.

### Required areas

#### Generic timers

Verify and extend timer behavior for virtualization-sensitive cases:

- physical timer
- virtual timer
- EL2 control visibility
- trap/ownership behavior expected by EL2

#### GIC virtualization-facing behavior

After second-stage MMU is working, evaluate the next required GIC surface:

- virtual CPU interface behavior
- ICH/ICV/ICC sysregs that an EL2 payload actually touches
- guest interrupt injection paths

This phase should be guided by the first failing L4Re/Linux traces rather than speculative over-implementation.

### Completion gate

- Linux guest below an EL2 payload can receive timer interrupts
- guest interrupt paths no longer stall early boot

---

## Phase 8: L4Re-specific boot path and Linux guest validation

### Goal

Move from "Helm has EL2 support" to "Helm can boot the target EL2 payload and then a Linux VM beneath it".

### Required work

#### Asset integration

Add or document the payloads needed for:

- EL2 L4Re/Fiasco.OC image
- any companion loader or userspace components
- Linux guest image and DTB expectations

#### Machine contract validation

Confirm that the EL2 payload expects the current `arm-virt` contract:

- GIC version
- timer topology
- UART
- RAM layout
- DTB content
- PSCI behavior

#### Trace-driven completion

Use simulator traces to close the remaining gaps:

- missing sysregs
- missing traps
- missing device behavior
- missing MMU corner cases

### Completion gate

- EL2 payload boots
- EL2 payload reaches the point of guest VM creation
- Linux guest VM begins execution and reaches observable forward progress

---

## Detailed Implementation Inventory

## Files expected to change first

### Architectural state and sysregs

- `runtime/helm-arch/src/aarch64/arch_state.rs`
- `runtime/helm-arch/src/aarch64/execute/helpers.rs`
- `runtime/helm-arch/src/aarch64/execute/sysreg.rs`

### MMU and exception model

- `runtime/helm-arch/src/aarch64/mmu.rs`
- `runtime/helm-arch/src/aarch64/exception.rs`

### FS execution path

- `runtime/helm-engine/src/fs.rs`

### Boot and machine policy

- `runtime/helm-engine/src/platform/arm_virt.rs`
- `runtime/helm-engine/src/loader/arm64_image.rs`
- possibly Python/config plumbing if explicit user-facing machine options are added

### Tests

- `runtime/helm-arch/src/aarch64/tests/exec_el2_el3.rs`
- `runtime/helm-arch/src/aarch64/tests/exec_sysreg.rs`
- `runtime/helm-arch/src/aarch64/mmu.rs` unit tests
- `runtime/helm-engine/src/platform/arm_virt.rs` tests
- `runtime/helm-engine/src/fs.rs` tests

---

## Testing Strategy

## Unit tests

### EL2 virtualization state

Add tests for:

- `MRS/MSR VTTBR_EL2`
- `MRS/MSR VTCR_EL2`
- `MRS HPFAR_EL2`
- TLB flush side effects on `VTTBR_EL2` / `VTCR_EL2` writes

### Stage-2 walk

Add MMU tests covering:

- valid stage-2 4K page translation
- invalid descriptor fault
- access-flag fault
- stage-2 permission fault
- execute-never fault

### Combined stage-1 + stage-2

Add tests covering:

- EL1 fetch through stage-1 + stage-2
- EL1 load/store through stage-1 + stage-2
- stage-1 success followed by stage-2 failure

### Fault routing

Add FS tests confirming:

- stage-2 instruction abort targets EL2
- stage-2 data abort targets EL2
- `FAR_EL2` and `HPFAR_EL2` are updated

### Boot-state tests

Add machine tests covering:

- EL1 boot vCPU initialization
- EL2 boot vCPU initialization
- EL3 boot vCPU initialization

## Integration tests

Create focused integration flows in increasing order:

1. minimal EL2 test payload that writes virtualization sysregs and exercises a guest access
2. EL2 payload that deliberately triggers stage-2 faults and validates vectors
3. EL2 payload that enters an EL1 guest stub
4. Linux direct boot below EL2
5. full L4Re-to-Linux-VM path

## Runtime validation

Use the existing simulator trace infrastructure to capture:

- fault entry
- boot EL
- trap registers
- MMU translation failures
- guest progress milestones

Where needed, add targeted `sim_info!`, `sim_warn!`, or `sim_stub!` instrumentation using the existing diagnostics conventions.

---

## Risks and Mitigations

## Risk 1: Fault routing becomes ambiguous

The current `MemFault` / `HartException` path is intentionally simple.
Adding stage-2 awareness may tempt a broad exception refactor.

### Mitigation

Keep the first implementation narrow:

- only enrich the metadata needed to distinguish stage-2 from stage-1
- avoid redesigning the whole exception subsystem

## Risk 2: Over-implementing EL3 before EL2 works

The requested end state mentions both EL2 and EL3, but the practical blocker for a Linux VM beneath L4Re is EL2 virtualization, not full secure-world machinery.

### Mitigation

Do EL3 boot-entry correctness first, but defer deep secure-world modeling until there is evidence it is required by the target stack.

## Risk 3: GIC virtualization gaps become the next blocker immediately

Once stage-2 MMU exists, the first EL2 payload may fail on virtual interrupt expectations.

### Mitigation

Treat stage-2 MMU as the first vertical slice, then use trace-guided completion on GIC virtualization rather than speculative bulk implementation.

## Risk 4: Machine policy and image policy conflict

There may be disagreement between:

- image-requested boot EL
- machine-requested boot EL
- user-requested startup mode

### Mitigation

Define one precedence policy explicitly in the machine/loader layer and test it.

---

## Recommended Execution Order

This is the practical order to implement the work:

1. Add `VTTBR_EL2`, `VTCR_EL2`, `HPFAR_EL2` to architectural state and sysreg handling.
2. Add stage-2 walk and composition into the live MMU path.
3. Route stage-2 instruction/data aborts to EL2 with correct fault registers.
4. Add combined `AT S12E1*` support.
5. Add explicit EL3 boot-state initialization.
6. Add explicit machine boot policy for EL1/EL2/EL3 startup modes.
7. Validate timer and interrupt behavior under an EL2 guest-hosting flow.
8. Integrate L4Re payloads and close remaining gaps from real traces.

This order maximizes the chance that each phase produces a testable improvement.

---

## Definition of Done

This plan is complete when all of the following are true:

- Helm can explicitly and correctly start `arm-virt` payloads at EL1, EL2, or EL3
- EL2 payloads can program second-stage translation through `VTTBR_EL2` and `VTCR_EL2`
- EL1 guest accesses beneath EL2 are translated through stage-2 when virtualization is enabled
- stage-2 translation and permission faults trap to EL2 correctly
- EL3 startup has dedicated boot-state handling
- a real EL2 hypervisor-style payload can host an EL1 Linux guest with observable forward progress

For the specific project goal, the final practical completion gate is:

- **L4Re boots at EL2 and reaches a Linux VM that boots far enough to demonstrate real guest execution**

---

## Immediate Next Slice

The first implementation slice should be:

1. add `VTTBR_EL2`, `VTCR_EL2`, `HPFAR_EL2`
2. add stage-2 translation to the live MMU path
3. add EL2 fault routing for second-stage failures
4. add regression tests for fetch/load/store success and failure beneath EL2

That slice is the minimum useful substrate for every later L4Re-specific step.
