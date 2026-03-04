# HELM: Hybrid Emulation Layer for Microarchitecture

HELM is a high-performance microarchitectural exploration tool and multi-ISA simulator written in Rust. Platform and system configurations are defined using Python scripts (similar to gem5), while the simulation engine itself is implemented in Rust. Together, they bridge the gap between fast functional emulation and detailed cycle-accurate analysis, providing a unified framework for system-level research and hardware–software co-design.

Like QEMU, HELM uses a fast dynamic translation engine in Syscall Emulation (SE) mode to execute real binaries efficiently. Unlike pure functional simulators, HELM can also drive those executions through a detailed out-of-order (OOO) microarchitectural model, allowing you to study pipeline behavior, speculation, and the memory hierarchy without giving up usability.

---

## Overview

HELM is designed primarily for architecture researchers and advanced practitioners who need to:

- Explore new microarchitectural designs (pipelines, branch predictors, memory hierarchies).
- Compare ISA and microarchitecture interactions across multiple instruction sets.
- Prototype hardware–software co-design ideas at the system level.

Rather than forcing a choice between speed and detail, HELM provides:

- **Fast functional execution** via dynamic translation and syscall emulation, similar in spirit to `qemu-user`.
- **Cycle-accurate OOO simulation** for in-depth microarchitectural analysis when and where you need it.

The core is a modular, multithreaded Rust engine with a clean separation between **ISA frontends** and a shared **microarchitectural backend**, making it straightforward to add new instruction sets or hardware models. High-level platforms are composed and configured in Python, giving users a familiar, scriptable interface for building experiments.

---

## Motivation & Positioning

Traditional simulation tools tend to occupy one of two extremes:

- **High-speed functional emulators** (e.g., QEMU) that execute binaries quickly but expose little microarchitectural detail.
- **Detailed architectural simulators** (e.g., gem5) that model pipelines and memory systems with high fidelity, often at the cost of significantly lower throughput and higher complexity.

HELM is explicitly designed to sit **between** these extremes:

- It aims for **QEMU-like speed in SE mode** by using dynamic translation to run unmodified user-space binaries while emulating syscalls.
- It exposes a **cycle-accurate OOO pipeline model** and a configurable memory hierarchy for microarchitectural studies.
- It maintains the **ease of use of user-mode emulators**, so you can bring up experiments quickly without requiring full-system OS images.

This hybrid approach makes HELM particularly suitable for modern research on microarchitecture, speculation, and memory systems, where you often need both realistic workloads and detailed hardware behavior.

---

## Key Features

- **Hybrid Execution Modes**  
  Switch between or combine fast functional emulation and detailed cycle-accurate simulation.

- **Syscall Emulation (SE) Mode**  
  - Executes real user-space binaries using a dynamic translation engine.  
  - Emulates system calls in a manner similar to `qemu-user`.  
  - Provides high throughput for workload bring-up, profiling, and large-scale experiments.

- **Microarchitectural Mode**  
  - Drives execution through a detailed cycle-accurate OOO core model.  
  - Enables deep inspection of pipeline stages, speculation, and memory interactions.  
  - Intended for microarchitectural design-space exploration and what-if studies.

- **Multi-ISA Support**  
  - Architected from the ground up to support multiple instruction sets through modular ISA frontends.  
  - Targets **x86**, **RISC-V**, and **ARM** families, enabling cross-ISA comparison and co-design.

- **Parallel Rust Engine**  
  - Implemented in Rust with a highly concurrent architecture.  
  - Scales simulations across modern multi-core hosts for better throughput.  
  - Designed to support parallel exploration of cores, threads, or independent workloads.

- **Detailed Out-of-Order Modeling**  
  - Register renaming and **Reorder Buffer (ROB)** modeling.  
  - **Branch prediction** and **speculative execution**, including misprediction and recovery behavior.  
  - **Memory hierarchy** with caches and basic coherence support for multi-core scenarios.  
  - Conceptually configurable structures (e.g., ROB depth, issue width, cache organization) to match target designs.

- **Frontend/Backend Separation**  
  - ISA frontends focus on decoding, lifting, and translating guest instructions.  
  - The microarchitectural backend models core and memory behavior in a largely ISA-agnostic way.  
  - This separation simplifies adding new architectures and hardware variants.

---

## Execution Modes

### Syscall Emulation (SE) Mode

In SE mode, HELM functions as a high-speed functional emulator:

- Guest code is executed via dynamic translation, similar to QEMU.  
- System calls are intercepted and emulated in user mode, avoiding the complexity of full-system virtualization.  
- The focus is on **speed and convenience**: rapidly bring up workloads, run large test suites, and collect coarse-grained performance or behavioral data.

This mode is ideal when you:

- Need to run unmodified binaries quickly.  
- Are primarily studying functional behavior or high-level performance trends.  
- Want to prepare workloads or regions of interest for deeper microarchitectural analysis.

### Microarchitectural Mode

In microarchitectural mode, HELM exposes a detailed OOO pipeline and memory model:

- Instructions flow through realistic pipeline stages (fetch, decode, rename, issue, execute, commit).  
- The simulator tracks dependencies, speculation, and resource usage.  
- Branch mispredictions, cache misses, and other microarchitectural events are modeled explicitly.

This mode is intended for:

- Evaluating pipeline designs and parameter choices (width, depths, queues).  
- Studying branch prediction and speculative execution behavior.  
- Exploring cache and memory hierarchy tradeoffs, including basic coherence.

Depending on your experiment, you can use SE mode for fast exploration and then switch to microarchitectural mode for detailed runs on selected workloads or regions.

---

## Supported ISAs & Multi-ISA Design

HELM is built around a **modular multi-ISA architecture**:

- **ISA Frontends**  
  - Each ISA (e.g., x86-64, RISC-V, ARM) has a dedicated frontend responsible for decoding and lifting guest instructions into an internal representation.  
  - Frontends can leverage the same dynamic translation infrastructure used in SE mode.

- **Shared Microarchitectural Backend**  
  - A largely ISA-agnostic backend models cores, pipelines, and memory.  
  - The same microarchitectural model can be driven by different ISA frontends, enabling direct comparisons.

This structure allows you to:

- Prototype new ISA variants without rewriting the microarchitectural core.  
- Study how different ISAs interact with similar hardware structures.  
- Share tooling, analysis, and infrastructure across multiple ISAs.

> **Note:** The exact set of fully supported ISAs and their maturity may evolve. Consult project documentation and release notes for up-to-date status.

---

## Microarchitecture Model

HELM’s microarchitectural backend focuses on capturing key behaviors of modern out-of-order processors:

- **Register Renaming & ROB**  
  - Renames architectural registers to physical registers.  
  - Uses a configurable Reorder Buffer (ROB) to track in-flight instructions and enforce precise exceptions.

- **Issue and Execution**  
  - Models instruction queues, functional units, and issue/execute/complete timing.  
  - Captures contention for shared resources and pipeline bottlenecks.

- **Branch Prediction & Speculation**  
  - Evaluates prediction accuracy and misprediction penalties.  
  - Models speculative execution and recovery, making it suitable for studies of speculation-related vulnerabilities or mitigation strategies.

- **Memory Hierarchy & Coherency**  
  - Represents a multi-level cache hierarchy (e.g., L1/L2/L3) and shared memory.  
  - Supports basic coherence mechanisms for multi-core configurations.  
  - Enables experiments on cache sizes, associativity, and placement/prefetch strategies.

Configuration knobs are designed to allow researchers to approximate a range of microarchitectures while reusing the same core modeling infrastructure.

---

## Parallel Engine & Scalability

HELM’s simulation engine is implemented in Rust and designed to take advantage of modern multi-core hosts:

- **Multithreaded Core**  
  - Simulation tasks can be partitioned across host threads, enabling higher throughput on multi-core machines.

- **Parallel Workloads**  
  - Intended to support parallel simulation of multiple cores, threads, or independent workloads.  
  - Useful for experiments that require running many configurations or input sets.

- **Rust Safety & Performance**  
  - Rust’s ownership and type system help maintain correctness in a highly concurrent codebase.  
  - Low-level control enables efficient implementation of dynamic translation and microarchitectural models.

---

## Extensibility & Research Use Cases

The frontend/backend split and modular design make HELM an extensible platform for research.

### Extending HELM

- **New ISAs**  
  - Add a new ISA frontend that decodes instructions and integrates with the existing translation and execution pipeline.

- **New Microarchitectural Variants**  
  - Plug in alternative branch predictors, cache hierarchies, or core configurations.  
  - Adjust pipeline parameters (width, depths, queues) to approximate different designs.

- **Custom Analysis & Instrumentation**  
  - Attach analysis hooks or statistics collection to pipeline events, memory accesses, or syscall interactions.  
  - Build experiment-specific tooling on top of HELM’s execution engine.

### Example Research Questions

HELM is intended to support studies such as:

- How do different branch predictor designs impact performance and energy across ISAs?  
- What are the tradeoffs among ROB size, issue width, and memory latency for a given workload mix?  
- How do cache hierarchy choices (sizes, levels, associativity) affect performance and scalability?  
- How do ISA features (e.g., compressed encodings, vector instructions) interact with a given microarchitectural design?  
- What is the impact of speculation policies or mitigations on real-world workloads?

---

## Project Status & Audience

HELM is **usable by others but evolving**:

- The core concepts—hybrid execution modes, multi-ISA support, and OOO modeling—are in place and under active refinement.  
- Interfaces, configuration formats, and supported ISAs may change as the project matures.

The primary audience is **architecture researchers**, with secondary applicability to:

- Systems and compiler researchers exploring HW/SW co-design.  
- Performance engineers interested in microarchitectural sensitivity studies.  
- Advanced students learning about modern OOO processors and memory systems.

---

## Roadmap (High-Level)

Planned and potential directions include:

- Broadening and hardening ISA support (x86, RISC-V, ARM and variants).  
- Enriching the memory system model with more detailed coherence and interconnects.  
- Expanding configuration and scripting support for large-scale design-space exploration.  
- Integrating additional analysis tools, statistics, and visualization hooks.  
- Improving documentation, examples, and teaching materials for new users.

Details of the roadmap and implementation status will be reflected in future releases and documentation updates.

---

## Getting Involved

HELM is an active project. As the codebase and APIs solidify, the project aims to provide:

- Clear build and usage documentation.  
- Example workloads and experiment recipes.  
- Contribution guidelines for adding new ISAs or microarchitectural models.

If you are interested in using HELM for your research or contributing new ideas, please watch this repository for updates and future documentation.
