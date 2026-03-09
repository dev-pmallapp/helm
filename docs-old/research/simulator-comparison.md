# Simulator Comparison: QEMU, Simics, gem5, and HELM

**Date:** March 4, 2026  
**Version:** 1.0

## Executive Summary

This document compares four major computer architecture simulators based on their timing models, accuracy levels, and intended use cases. Understanding these differences is crucial for positioning HELM in the simulator ecosystem.

---

## Quick Comparison Table

| Simulator | Primary Focus | Timing Model | Default Mode | Speed Range | Accuracy Range |
|-----------|--------------|--------------|--------------|-------------|----------------|
| **QEMU** | Functional emulation | None (functional only) | Fast functional | 1000+ MIPS | Functional only |
| **Simics** | Functional-first, timing optional | Event-driven, pluggable | Fast functional | 10-1000 MIPS | Functional → Approximate |
| **gem5** | Cycle-accurate research | Cycle/Event hybrid | Detailed timing | 0.1-100 MIPS | Approximate → Cycle-accurate |
| **HELM** | Hybrid research platform | Event-driven, pluggable | Configurable | 0.1-1000+ MIPS | Functional → Cycle-accurate |

---

## 1. QEMU: Pure Functional Emulation

### 1.1 Core Architecture

**Type**: Dynamic Binary Translator (DBT)  
**Timing Model**: **None**  
**Primary Goal**: Fast functional emulation for software development

```
┌─────────────────────────────────┐
│         QEMU Core               │
│  ┌──────────────────────────┐   │
│  │  Dynamic Translator      │   │
│  │  (TCG - Tiny Code Gen)   │   │
│  └──────────────────────────┘   │
│  ┌──────────────────────────┐   │
│  │  Device Emulation        │   │
│  │  (Functional only)       │   │
│  └──────────────────────────┘   │
└─────────────────────────────────┘
        │
        ▼
  Guest code executes
  No cycle counts
  No timing information
```

### 1.2 Characteristics

**Strengths:**
- ✓ Extremely fast (100-1000 MIPS typical)
- ✓ Supports many ISAs (x86, ARM, RISC-V, MIPS, etc.)
- ✓ Full system emulation (runs unmodified OS)
- ✓ Mature, stable, widely used
- ✓ Great for software development and testing

**Limitations:**
- ✗ **No timing model whatsoever**
- ✗ Cannot measure performance (IPC, cache miss rates, etc.)
- ✗ Not suitable for architecture research
- ✗ All operations complete "instantly" from guest perspective
- ✗ No cycle-accurate device models

**Timing Category**: **Level 0 - Pure Functional**

**Use Cases:**
- Operating system development
- Software testing and CI/CD
- Cross-platform development
- Quick boot and test cycles
- When performance metrics don't matter

**Example Performance:**
```
Booting Linux on x86:     ~30 seconds (real time)
Running SPEC CPU2017:     Fast, but no meaningful performance data
Memory access:            Instant (no cache simulation)
Device I/O:               Functional only, no latency modeling

Typical speed:            1000+ MIPS (can exceed on modern hosts)
```

---

## 2. Simics: Functional-First with Optional Timing

### 2.1 Core Architecture

**Type**: Event-Driven Simulator with Pluggable Timing  
**Timing Model**: **Optional, layered on top of functional core**  
**Primary Goal**: Fast functional execution with opt-in timing detail

```
┌──────────────────────────────────────────────┐
│            Simics Core                       │
│  ┌────────────────────────────────────────┐  │
│  │  Fast Functional Execution             │  │
│  │  (Event-driven, IPC=1 by default)      │  │
│  └────────────────────────────────────────┘  │
│           ▲                                   │
│           │ (optional attachment)             │
│  ┌────────┴────────────────────────────────┐ │
│  │  Timing Models (pluggable)              │ │
│  │  • timing_model interface               │ │
│  │  • micro_architecture interface         │ │
│  │  • Cache simulation (g-cache)           │ │
│  │  • Custom timing modules                │ │
│  └─────────────────────────────────────────┘ │
└──────────────────────────────────────────────┘
```

### 2.2 Characteristics

**Strengths:**
- ✓ Fast by default (100-1000 MIPS functional)
- ✓ Can add timing detail where needed (1-100 MIPS)
- ✓ Event-driven architecture (skip idle time)
- ✓ Temporal decoupling for multi-core
- ✓ Dynamic switching between fast/detailed modes
- ✓ Deterministic execution and replay
- ✓ Checkpointing and reverse execution

**Timing Philosophy:**
- Default: Functional execution, IPC=1, no cache effects
- Opt-in: Attach timing models for specific components
- Stall-cycle injection: Return latency values, not cycle-by-cycle simulation

**Limitations:**
- ✗ Not cycle-accurate by default
- ✗ Timing models are approximations (stall-based, not true pipeline)
- ✗ No detailed OOO pipeline in default mode
- ✗ Accuracy depends on quality of attached timing models
- Typical error: 10-50% vs real hardware even with timing models

**Timing Category**: **Level 1-2 - Functional to Timing-Approximate**

**Use Cases:**
- Firmware and OS development (fast mode)
- Performance estimation (with timing models)
- Large-scale system simulation
- Hardware/software co-design
- Education and training

**Example Performance:**
```
Functional mode:          100-1000 MIPS
+ Cache timing model:     10-100 MIPS (cache hit/miss latency)
+ Microarch model:        1-10 MIPS (basic OOO approximation)
+ Full timing:            0.1-1 MIPS (detailed but still approximate)

Accuracy:
  Functional:             N/A (no timing)
  With timing models:     ~50% error (performance trends only)
  Best case:              ~10-20% error (with custom calibrated models)
```

---

## 3. gem5: Cycle-Accurate Research Simulator

### 3.1 Core Architecture

**Type**: Detailed Architectural Simulator  
**Timing Model**: **Built-in, cycle-accurate by default**  
**Primary Goal**: Precise microarchitectural modeling for research

```
┌────────────────────────────────────────────────┐
│              gem5 Core                         │
│  ┌──────────────────────────────────────────┐  │
│  │  CPU Models (choose one):                │  │
│  │  • AtomicSimpleCPU (functional, fast)    │  │
│  │  • TimingSimpleCPU (in-order, timing)    │  │
│  │  │  └─> models pipeline stages          │  │
│  │  • O3CPU (out-of-order, detailed)        │  │
│  │  │  └─> ROB, IQ, LSQ, exec units        │  │
│  │  • MinorCPU (in-order, 4-stage pipe)     │  │
│  └──────────────────────────────────────────┘  │
│  ┌──────────────────────────────────────────┐  │
│  │  Memory System (detailed)                │  │
│  │  • Classic (simple hierarchy)            │  │
│  │  • Ruby (detailed coherence protocols)   │  │
│  └──────────────────────────────────────────┘  │
└────────────────────────────────────────────────┘
```

### 3.2 Characteristics

**Strengths:**
- ✓ True cycle-accurate simulation
- ✓ Detailed OOO pipeline modeling (O3CPU)
- ✓ Accurate cache and memory system (Ruby)
- ✓ Models pipeline stages, reservation stations, ROB
- ✓ Configurable to match specific processors
- ✓ Academic standard for architecture research
- ✓ Extensive documentation and validation

**Architecture:**
- Models each pipeline stage explicitly
- Tracks every instruction through fetch → execute → commit
- Simulates resource contention, dependencies, stalls
- Ruby: detailed cache coherence protocols (MESI, MOESI, etc.)

**Limitations:**
- ✗ Very slow (0.1-10 MIPS typical)
- ✗ Complex configuration
- ✗ Long setup time for new platforms
- ✗ Not suitable for full OS boot (too slow)
- ✗ Primarily research-oriented, not production

**Timing Category**: **Level 2-3 - Timing-Approximate to Cycle-Accurate**

**CPU Model Comparison:**

| gem5 CPU Model | Speed | Accuracy | Use Case |
|----------------|-------|----------|----------|
| **AtomicSimpleCPU** | 10-100 MIPS | Functional only | Fast testing, no timing |
| **TimingSimpleCPU** | 1-10 MIPS | Basic timing | Simple in-order modeling |
| **MinorCPU** | 0.5-5 MIPS | Good timing | In-order pipeline research |
| **O3CPU** | 0.1-1 MIPS | Cycle-accurate | OOO architecture research |

**Use Cases:**
- Microarchitecture research
- Cache hierarchy studies
- Memory system research
- Branch predictor evaluation
- Comparing design alternatives

**Example Performance:**
```
AtomicSimpleCPU:       10-100 MIPS (functional, no timing)
TimingSimpleCPU:       1-10 MIPS (basic in-order timing)
O3CPU:                 0.1-1 MIPS (detailed OOO)

Accuracy (O3CPU):
  vs real hardware:    2-10% error (carefully configured)
  IPC prediction:      Very good (within 5-10%)
  Cache behavior:      Excellent (Ruby models)
  
Typical simulation:
  Boot Linux:          Hours to days (often impractical)
  SPEC benchmark:      Minutes to hours per test
  1M instructions:     Seconds to minutes (O3CPU)
```

---

## 4. HELM: Hybrid Approach

### 4.1 Design Philosophy

**Type**: Multi-Mode Simulator (Functional → Cycle-Accurate)  
**Timing Model**: **Pluggable, user-selectable**  
**Primary Goal**: Combine QEMU speed, Simics flexibility, gem5 accuracy

```
┌─────────────────────────────────────────────────┐
│              HELM Core                          │
│  ┌───────────────────────────────────────────┐  │
│  │  Fast Functional Core (like QEMU)         │  │
│  │  • Dynamic translation                    │  │
│  │  • Syscall emulation                      │  │
│  └───────────────────────────────────────────┘  │
│           ▲                                      │
│           │ (runtime switchable)                 │
│  ┌────────┴──────────────────────────────────┐  │
│  │  Timing Models (like Simics)              │  │
│  │  • Event-driven                           │  │
│  │  • Temporal decoupling                    │  │
│  │  • Pluggable timing modules               │  │
│  └───────────────────────────────────────────┘  │
│  ┌───────────────────────────────────────────┐  │
│  │  Detailed Models (like gem5)              │  │
│  │  • OOO pipeline simulation                │  │
│  │  • Cycle-accurate cache                   │  │
│  │  • Branch prediction                      │  │
│  └───────────────────────────────────────────┘  │
└─────────────────────────────────────────────────┘
```

### 4.2 Multi-Level Design

HELM supports 4 accuracy levels (see detailed docs):

**Level 0: QEMU-like Functional**
- Speed: 1000+ MIPS (matches or exceeds QEMU)
- No timing information
- Use: Software development, testing

**Level 1: Simics-like Stall-Annotated**
- Speed: 10-100 MIPS  
- Cache latency, device delays
- Use: Performance trends

**Level 2: Simics+gem5-like Microarchitectural**
- Speed: 1-10 MIPS
- OOO execution, branch prediction
- Use: Architecture research

**Level 3: gem5-like Cycle-Accurate**
- Speed: 0.1-1 MIPS
- Detailed pipeline, precise timing
- Use: Architecture validation

**Timing Category**: **Level 0-3 - Configurable across full spectrum**

---

## 5. Detailed Categorization

### 5.1 By Timing Accuracy

```
Functional Only          Timing-Approximate       Cycle-Accurate
─────────────────────────────────────────────────────────────────>
QEMU                    Simics (w/ timing)       gem5 (O3CPU)
HELM (Level 0)          HELM (Level 1-2)         HELM (Level 3)

Speed: 1000+ MIPS       Speed: 1-100 MIPS        Speed: 0.1-1 MIPS
Error: N/A              Error: 10-50%            Error: 2-10%
```

### 5.2 By Primary Use Case

**Software Development (Need Speed):**
1. QEMU (fastest, no timing)
2. Simics (fast functional mode)
3. HELM Level 0 (QEMU-like)
4. gem5 AtomicSimpleCPU (still slow)

**Performance Estimation (Need Trends):**
1. Simics with timing models (good speed/accuracy balance)
2. HELM Level 1-2 (similar approach)
3. gem5 TimingSimpleCPU (slower but more accurate)

**Architecture Research (Need Accuracy):**
1. gem5 O3CPU (academic standard)
2. HELM Level 3 (aims for similar accuracy)
3. Simics with detailed models (limited accuracy)

**System-Level Research (Need Scale):**
1. Simics (can simulate large systems)
2. HELM (designed for flexibility)
3. gem5 (limited to smaller systems)

### 5.3 By Simulation Methodology

**Event-Driven:**
- Simics: Yes (core architecture)
- HELM: Yes (core architecture)
- gem5: Partial (hybrid event/cycle)
- QEMU: No (pure DBT)

**Cycle-Driven:**
- gem5: Yes (for detailed models)
- HELM: Optional (Level 3)
- Simics: No (stall-based approximation)
- QEMU: No

**Temporal Decoupling:**
- Simics: Yes (time quanta)
- HELM: Yes (configurable)
- gem5: Limited
- QEMU: N/A

### 5.4 By Flexibility

**Runtime Mode Switching:**
1. HELM: Yes (design goal)
2. Simics: Yes (attach/detach timing models)
3. gem5: Limited (requires restart)
4. QEMU: No (functional only)

**Pluggable Components:**
1. Simics: Excellent (dynamic modules)
2. HELM: Excellent (design goal)
3. gem5: Good (Python configs, requires rebuild)
4. QEMU: Limited (static compilation)

---

## 6. Timing Model Deep Dive

### 6.1 QEMU Timing Model

```
Instruction → Execute → Done (instant)

No concept of:
  - Cycles
  - Pipeline stages  
  - Cache hits/misses
  - Device latency
  
Everything completes in "zero time"
```

### 6.2 Simics Timing Model

```
Instruction → Execute functionally → Query timing_model → Advance virtual time

timing_model returns stall cycles:
  - Base: 1 cycle (IPC=1)
  - + Cache miss: X cycles (from cache simulator)
  - + Branch mispredict: Y cycles (from predictor)
  
Stalls are added up, virtual time advances
No actual pipeline simulation
```

**Key Insight**: Simics doesn't model the pipeline, it models the **effects** of the pipeline through stall cycles.

### 6.3 gem5 Timing Model (O3CPU)

```
Instruction → Fetch stage (cycle N)
           → Decode stage (cycle N+1)
           → Rename stage (cycle N+2)
           → Issue queue (cycles N+3...N+K, depends on dependencies)
           → Execute (cycle N+K+1)
           → Writeback (cycle N+K+2)
           → Commit from ROB (cycle N+K+3)

Every stage simulated explicitly
Real pipeline state maintained
True cycle-by-cycle execution
```

**Key Insight**: gem5 actually simulates the pipeline structure, not just its timing effects.

### 6.4 HELM Timing Model (Configurable)

```
Level 0 (Functional):
  Instruction → Execute → Done (QEMU-style)

Level 1 (Stall-Annotated):
  Instruction → Execute → Query timing → Add stalls (Simics-style)

Level 2 (Microarchitectural):
  Instruction → Decode → ROB allocation → Issue → Execute → Commit
  (Simplified pipeline, event-driven)

Level 3 (Cycle-Accurate):
  Instruction → Full pipeline simulation (gem5-style)
  (Each stage explicitly modeled)
```

---

## 7. Summary Comparison

### 7.1 Quick Reference

| Aspect | QEMU | Simics | gem5 | HELM |
|--------|------|--------|------|------|
| **Primary goal** | Software dev | Flexible platform | Research | Hybrid research |
| **Speed (max)** | 1000+ MIPS | 1000 MIPS | 100 MIPS | 1000+ MIPS |
| **Speed (detailed)** | N/A | 1-10 MIPS | 0.1-1 MIPS | 0.1-10 MIPS |
| **Timing model** | None | Optional | Built-in | Configurable |
| **Pipeline model** | No | No (stalls) | Yes (detailed) | Yes (configurable) |
| **Cache simulation** | No | Yes (latency) | Yes (detailed) | Yes (detailed) |
| **Branch prediction** | No | Optional | Yes | Yes |
| **OOO execution** | No | Approximate | Yes (O3CPU) | Yes (Level 2-3) |
| **Accuracy** | N/A | ~20-50% | ~2-10% | ~2-50% (depends) |
| **Flexibility** | Low | High | Medium | High |
| **Boot OS** | Fast | Fast | Slow | Fast → Slow |
| **Dynamic modules** | No | Yes | No | Yes |
| **Learning curve** | Low | Medium | High | Medium |

### 7.2 Positioning Statement

**QEMU**: "Fast functional emulation, no timing information"  
- Best for: Software development, testing, CI/CD

**Simics**: "Functional-first with optional timing detail"  
- Best for: System simulation, performance trends, flexible research

**gem5**: "Cycle-accurate architectural simulator"  
- Best for: Detailed architecture research, cache studies, academic papers

**HELM**: "Hybrid platform: QEMU speed + Simics flexibility + gem5 accuracy"  
- Best for: Research requiring both speed and accuracy, design space exploration

---

## 8. When to Use Each Simulator

### 8.1 Decision Tree

```
Do you need timing information?
├─ No → Use QEMU
│   └─ Fastest option, perfect for functional testing
│
└─ Yes → Do you need cycle-accurate details?
    ├─ No, just performance trends → Use Simics or HELM Level 1-2
    │   └─ Fast enough to boot OS, get reasonable performance estimates
    │
    └─ Yes, need precise accuracy → Use gem5 O3CPU or HELM Level 3
        └─ Slow but accurate, for detailed architecture research
```

### 8.2 Use Case Matrix

| Use Case | First Choice | Alternative | Why |
|----------|-------------|-------------|-----|
| Boot Linux, test drivers | QEMU | Simics | Speed matters, no timing needed |
| Firmware development | QEMU | HELM L0 | Fast iteration cycle |
| Performance estimation | Simics | HELM L1 | Good speed/accuracy balance |
| Cache hierarchy research | gem5 Ruby | HELM L2-3 | Need detailed coherence |
| Branch predictor study | gem5 O3CPU | HELM L2-3 | Need accurate branch behavior |
| OOO design exploration | gem5 O3CPU | HELM L2-3 | Need pipeline details |
| Large system simulation | Simics | HELM L1 | Need to handle scale |
| HW/SW co-design | Simics | HELM L1-2 | Need flexibility |

---

## Conclusion

The four simulators occupy different niches:

1. **QEMU**: Purely functional, extremely fast, no timing
2. **Simics**: Functional-first with opt-in timing approximation
3. **gem5**: Cycle-accurate by default, research-focused
4. **HELM**: Configurable across the entire spectrum

HELM's unique value proposition is **flexibility**: it can operate like QEMU when you need speed, like Simics when you need reasonable timing, and like gem5 when you need cycle-accuracy—all within the same framework without rebuilding or reconfiguring.
