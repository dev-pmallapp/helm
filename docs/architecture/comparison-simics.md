# Comparison with Simics

Architectural parallels and differences between helm-ng and Wind River
Simics.

## Overview

Simics is the gold standard for deterministic platform simulation.
helm-ng borrows Simics's clean separation between events and haps
(observable notifications), its attribute-based state exposure, and its
determinism-first philosophy. The main divergence is language (Rust vs
C/DML) and licensing (open vs commercial).

## Design Philosophy

| Principle | Simics | helm-ng |
|-----------|--------|---------|
| Determinism | First-class guarantee | First-class guarantee |
| State exposure | Attribute system (every field visible) | AttrRegistry (no dark state) |
| Config language | Python + DML | Python + Rust traits |
| Device description | DML (domain-specific language) | Rust `Device` trait |
| Simulation modes | Transaction-level + cycle-accurate | VirtualTiming + IntervalTiming + AccurateTiming |

Both enforce the principle that all simulation state must be explicitly
registered and inspectable. Simics uses its attribute system; helm-ng
uses `AttrRegistry` with the "no dark state" rule.

## Event Systems

| Aspect | Simics | helm-ng |
|--------|--------|---------|
| Scheduled events | `SIM_event_post()` → future time | `EventQueue::post_at()` → future tick |
| Observable events | HAP system (`SIM_hap_add_callback`) | `HelmEventBus::emit()` |
| Separation | Clean (events ≠ haps) | Clean (EventQueue ≠ HelmEventBus) |
| Checkpoint | Events saved, haps re-register | EventQueue saved, EventBus re-registers |

helm-ng's dual event system directly follows the Simics model. The
distinction between "what should happen at time T" (events) and "who
wants to know that X happened" (haps/EventBus) is preserved.

## Device Model

| Aspect | Simics | helm-ng |
|--------|--------|---------|
| Device language | DML (compiled to C) | Rust (`Device` trait) |
| Register banks | DML `register` / `field` declarations | Future `register_bank!` macro |
| MMIO dispatch | Interface-based callbacks | `Device::transact()` |
| IRQ model | Wire interfaces | `InterruptPin` → `InterruptSink` |
| Address ownership | Config-driven (device is oblivious) | Platform-driven (device is oblivious) |
| Dynamic loading | `.so` module loading | DLD `.so` loading via `DeviceRegistry` |

Both follow the principle that devices should not know their base
address or IRQ number. The platform/configuration layer handles all
wiring.

## Attribute System

| Aspect | Simics | helm-ng |
|--------|--------|---------|
| Registration | `SIM_register_attribute()` | `AttrRegistry::register()` |
| Types | Integer, string, list, dict, object | `AttrValue` enum |
| Inspection | CLI + Python (`SIM_get_attribute`) | Debugger + Python |
| Serialization | Built-in checkpoint format | `checkpoint_save/restore` via AttrRegistry |
| Dark state | Rare (attribute system catches most) | Forbidden (all fields must be registered) |

## Checkpoint

| Aspect | Simics | helm-ng |
|--------|--------|---------|
| Scope | All registered attributes | All registered attributes |
| Format | Binary + text metadata | Binary blob |
| Differential | Supported | Planned |
| Reverse execution | Supported (with micro-checkpoints) | Not planned |

## Timing

| Aspect | Simics | helm-ng |
|--------|--------|---------|
| Primary mode | Transaction-level (instruction-level timing) | IntervalTiming (instruction-class latencies) |
| Cycle-accurate | Available (with detailed models) | AccurateTiming (planned) |
| Functional | Available | VirtualTiming (IPC=1) |
| Dispatch | Callback-based | Monomorphized generic |

## Platform Construction

| Aspect | Simics | helm-ng |
|--------|--------|---------|
| Config language | Python + DML components | Python (gem5-style) |
| Component model | Namespace tree | SimObject tree |
| Config freeze | At `continue` command | At `instantiate()` call |
| Platform description | Python scripts + DML | `Platform` trait + Python |

## Instrumentation

| Aspect | Simics | helm-ng |
|--------|--------|---------|
| Probes | HAP callbacks (C) | `Probe<T>` (zero-cost) + `HelmPlugin` |
| Logging | `SIM_log()` levels | `DiagMonitor` + `DiagLevel` |
| Statistics | Custom per-model | `StatsRegistry` + `PerfCounter` |
| Analysis | External tools | Built-in `helm-spy` + `helm-report` |

## Key Differences

1. **Open source** — helm-ng is fully open; Simics is commercial
   (Wind River / Intel).

2. **Language safety** — Rust's ownership system prevents many
   categories of bugs that Simics's C/DML code must guard against
   manually.

3. **No DML** — helm-ng uses plain Rust traits instead of a
   domain-specific device description language. This reduces the
   learning curve but loses DML's register bank automation.

4. **Timing dispatch** — Simics uses callbacks; helm-ng uses
   monomorphized generics. helm-ng's approach eliminates function-call
   overhead in the hot loop at the cost of binary size (one copy of
   the engine per timing model).
