# Cycle-Accurate Simulation in HELM

**Date:** March 4, 2026  
**Version:** 1.0

## Executive Summary

This document describes HELM's timing model architecture for achieving cycle-accurate microarchitectural simulation. Inspired by Simics's event-driven approach with temporal decoupling, HELM provides a flexible framework that allows users to trade speed for accuracy based on their specific research needs.

---

## Table of Contents

1. [Overview](#overview)
2. [Event-Driven vs Cycle-Driven Simulation](#event-driven-vs-cycle-driven-simulation)
3. [Temporal Decoupling](#temporal-decoupling)
4. [Timing Models and Stall Cycles](#timing-models-and-stall-cycles)
5. [Virtual Time Management](#virtual-time-management)
6. [Fast-Forwarding and Sampling](#fast-forwarding-and-sampling)
7. [Accuracy Levels](#accuracy-levels)
8. [Implementation Details](#implementation-details)

---

## 1. Overview

### 1.1 Design Philosophy

HELM's timing model is built on these principles:

1. **Functional Correctness First**: The simulator always produces functionally correct results
2. **Opt-In Timing Detail**: Start fast (functional-only), add timing detail where needed
3. **Event-Driven Core**: Skip idle time, only simulate when something happens
4. **Temporal Decoupling**: Allow cores to run ahead within bounded intervals
5. **Pluggable Timing Models**: Attach/detach timing models at runtime

### 1.2 Speed vs Accuracy Spectrum

```
Functional Only        Timing-Approximate      Cycle-Accurate
─────────────────────────────────────────────────────────────>
1000+ MIPS            10-100 MIPS             0.1-10 MIPS
IPC=1, flat memory    Cache sim, stalls       Pipeline, OoO
Boot OS, test SW      Performance trends      Arch research
(QEMU-equivalent)     (Simics-like)           (gem5-like)
```

### 1.3 Key Concepts

- **Virtual Time**: Simulated time as seen by guest software (measured in cycles)
- **Real Time**: Wall-clock time on the host machine
- **Time Quantum**: Maximum allowed virtual time difference between cores
- **Synchronization Point**: Moment when all cores must reach same virtual time
- **Timing Model**: Component that annotates operations with stall cycles

---

## 2. Event-Driven vs Cycle-Driven Simulation

### 2.1 Cycle-Driven Simulation (Traditional)

In a cycle-driven simulator, all components advance one cycle at a time:

```rust
// Cycle-driven pseudocode
loop {
    for component in &mut all_components {
        component.tick();  // Advance by 1 cycle
    }
    global_cycle += 1;
}
```

**Problems:**
- Wasteful: Most components idle most of the time
- Slow: Must simulate every cycle even when nothing happens
- Clock domain complexity: Different components at different frequencies

**Example**: A 2 GHz CPU and 100 MHz bus both tick together. The bus does nothing 19 out of 20 cycles, but you still pay the simulation cost.

### 2.2 Event-Driven Simulation (HELM Approach)

HELM uses an event-driven architecture:

```rust
// Event-driven pseudocode
loop {
    let event = event_queue.pop_earliest();
    virtual_time = event.timestamp;
    event.handler(event.context);
}
```

**Advantages:**
- Only simulate when something actually happens
- Skip large idle periods automatically
- Natural handling of different clock frequencies
- Typical speedup: 10-100× over cycle-driven for OS-level workloads

**Example**: A UART that fires every 10ms doesn't consume any simulation cycles during the intervening 20 million cycles.

### 2.3 HELM Event Queue Implementation

```rust
// helm-core/src/event.rs
use std::collections::BinaryHeap;
use std::cmp::Reverse;

#[derive(Clone, Debug)]
pub struct SimulationEvent {
    pub timestamp: u64,        // Virtual cycle when event fires
    pub priority: u32,         // Tie-breaker for same timestamp
    pub handler: EventHandler, // What to execute
    pub context: EventContext, // Event-specific data
}

impl Ord for SimulationEvent {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        // Reverse to make BinaryHeap a min-heap
        (Reverse(self.timestamp), Reverse(self.priority))
            .cmp(&(Reverse(other.timestamp), Reverse(other.priority)))
    }
}

pub struct EventQueue {
    queue: BinaryHeap<SimulationEvent>,
    current_time: u64,
}

impl EventQueue {
    pub fn schedule(&mut self, event: SimulationEvent) {
        assert!(event.timestamp >= self.current_time, 
                "Cannot schedule event in the past");
        self.queue.push(event);
    }
    
    pub fn next_event(&mut self) -> Option<SimulationEvent> {
        self.queue.pop().map(|event| {
            self.current_time = event.timestamp;
            event
        })
    }
    
    pub fn peek_next_time(&self) -> Option<u64> {
        self.queue.peek().map(|e| e.timestamp)
    }
    
    pub fn current_time(&self) -> u64 {
        self.current_time
    }
}
```

### 2.4 Instruction-Driven CPU Core

The CPU itself is instruction-driven (a variant of event-driven):

```rust
// helm-engine/src/core_sim.rs
pub struct CoreSimulator {
    event_queue: Arc<Mutex<EventQueue>>,
    virtual_time: u64,
    instructions_executed: u64,
    timing_model: Option<Box<dyn TimingModel>>,
}

impl CoreSimulator {
    pub fn run_quantum(&mut self, max_cycles: u64) -> Result<QuantumResult> {
        let start_time = self.virtual_time;
        let end_time = start_time + max_cycles;
        
        while self.virtual_time < end_time {
            // Fetch and decode instruction
            let insn = self.fetch_instruction()?;
            
            // Execute functionally
            let result = self.execute_instruction(&insn)?;
            
            // Query timing model for stall cycles
            let stall_cycles = if let Some(ref timing) = self.timing_model {
                timing.compute_stall(&insn, &result)?
            } else {
                1  // Default IPC = 1
            };
            
            // Advance virtual time
            self.virtual_time += stall_cycles;
            self.instructions_executed += 1;
            
            // Check for interrupts, events, etc.
            if self.should_break()? {
                break;
            }
        }
        
        Ok(QuantumResult {
            cycles_executed: self.virtual_time - start_time,
            instructions_executed: self.instructions_executed,
        })
    }
}
```

---

## 3. Temporal Decoupling

### 3.1 Concept

Temporal decoupling allows cores to run ahead of each other by a bounded amount (the **time quantum**), then synchronize at barriers.

```
Core 0:  |=====quantum=====|barrier|=====quantum=====|barrier|
Core 1:  |=====quantum=====|barrier|=====quantum=====|barrier|
Core 2:  |=====quantum=====|barrier|=====quantum=====|barrier|
                            ^sync                     ^sync
         │←─── up to Q cycles of skew ───→│
```

**Benefits:**
- Cores execute independently within quantum → better host parallelism
- Fewer synchronization points → less overhead
- Configurable accuracy/speed tradeoff

**Quantum Size Trade-off:**
- **Large quantum** (10,000-100,000 cycles): Faster but more timing skew
- **Small quantum** (100-1,000 cycles): Slower but more accurate inter-core timing

### 3.2 Synchronization Points

Cores must synchronize at:

1. **I/O Device Access**: Memory-mapped device registers
2. **Inter-Processor Interrupts (IPIs)**: One core signals another
3. **Explicit Barriers**: Memory barriers, atomic operations (optional)
4. **Quantum Expiration**: Reached maximum quantum size

### 3.3 Implementation

```rust
// helm-engine/src/temporal_decoupling.rs

pub struct TemporalDecoupler {
    cores: Vec<CoreHandle>,
    quantum_size: u64,
    sync_barrier: Arc<Barrier>,
}

pub struct CoreHandle {
    core_id: usize,
    virtual_time: AtomicU64,
    quantum_start: AtomicU64,
    needs_sync: AtomicBool,
}

impl TemporalDecoupler {
    pub fn run_all_cores(&mut self) -> Result<()> {
        loop {
            // Phase 1: Run quantum on all cores in parallel
            self.cores.par_iter_mut().for_each(|core| {
                let quantum_end = core.quantum_start.load(Ordering::Relaxed) 
                                  + self.quantum_size;
                
                // Run until quantum expires or sync needed
                while core.virtual_time.load(Ordering::Relaxed) < quantum_end {
                    if core.needs_sync.load(Ordering::Acquire) {
                        break;
                    }
                    
                    // Execute instructions
                    core.step();
                }
            });
            
            // Phase 2: Synchronization barrier
            self.synchronize_all_cores()?;
            
            // Update quantum starts
            let global_time = self.cores.iter()
                .map(|c| c.virtual_time.load(Ordering::Relaxed))
                .min()
                .unwrap();
            
            for core in &self.cores {
                core.quantum_start.store(global_time, Ordering::Relaxed);
                core.needs_sync.store(false, Ordering::Release);
            }
        }
    }
    
    pub fn request_sync(&self, core_id: usize) {
        // Called when core hits I/O or IPI
        self.cores[core_id].needs_sync.store(true, Ordering::Release);
    }
    
    fn synchronize_all_cores(&mut self) -> Result<()> {
        // Wait for all cores to reach barrier
        self.sync_barrier.wait();
        
        // Process cross-core events (interrupts, memory operations)
        self.process_pending_events()?;
        
        Ok(())
    }
}
```

### 3.4 Memory Consistency

HELM provides configurable memory consistency modes:

```rust
pub enum MemoryConsistencyMode {
    /// No synchronization on memory accesses (fastest)
    /// Each core sees sequentially consistent view of own accesses
    Relaxed,
    
    /// Synchronize on shared memory writes
    /// Models cache coherence protocols
    Sequential,
    
    /// Synchronize on every memory access
    /// Strict ordering, very slow but most accurate
    Strict,
}
```

**Default**: `Relaxed` mode - only synchronize on I/O and IPIs

**Research mode**: `Sequential` or `Strict` for memory consistency studies

---

## 4. Timing Models and Stall Cycles

### 4.1 Timing Model Interface

Components inject timing by returning stall cycles:

```rust
// helm-core/src/timing.rs

pub trait TimingModel: Send + Sync {
    /// Compute stall cycles for an instruction
    fn compute_stall(
        &mut self,
        insn: &Instruction,
        result: &ExecutionResult,
    ) -> Result<u64>;
    
    /// Handle memory access timing
    fn memory_access(
        &mut self,
        addr: u64,
        size: usize,
        is_write: bool,
    ) -> Result<u64>;
    
    /// Handle branch misprediction
    fn branch_misprediction(
        &mut self,
        pc: u64,
        predicted: u64,
        actual: u64,
    ) -> Result<u64>;
}
```

### 4.2 Simple Timing Model: Fixed IPC

```rust
pub struct FixedIpcModel {
    cycles_per_instruction: u64,
}

impl TimingModel for FixedIpcModel {
    fn compute_stall(&mut self, _insn: &Instruction, _result: &ExecutionResult) 
        -> Result<u64> 
    {
        Ok(self.cycles_per_instruction)
    }
    
    fn memory_access(&mut self, _addr: u64, _size: usize, _is_write: bool) 
        -> Result<u64> 
    {
        Ok(0)  // Already accounted in CPI
    }
    
    fn branch_misprediction(&mut self, _pc: u64, _predicted: u64, _actual: u64) 
        -> Result<u64> 
    {
        Ok(0)  // Not modeled
    }
}
```

### 4.3 Cache Hierarchy Timing Model

```rust
pub struct CacheHierarchyModel {
    l1i: CacheSimulator,
    l1d: CacheSimulator,
    l2: CacheSimulator,
    l3: Option<CacheSimulator>,
    dram_latency: u64,
}

impl TimingModel for CacheHierarchyModel {
    fn memory_access(&mut self, addr: u64, size: usize, is_write: bool) 
        -> Result<u64> 
    {
        // Try L1D
        if self.l1d.access(addr, size, is_write) {
            return Ok(self.l1d.hit_latency);
        }
        
        // L1 miss, try L2
        if self.l2.access(addr, size, is_write) {
            return Ok(self.l2.hit_latency);
        }
        
        // L2 miss, try L3 if present
        if let Some(ref mut l3) = self.l3 {
            if l3.access(addr, size, is_write) {
                return Ok(l3.hit_latency);
            }
        }
        
        // All caches missed, go to DRAM
        Ok(self.dram_latency)
    }
    
    fn compute_stall(&mut self, insn: &Instruction, _result: &ExecutionResult) 
        -> Result<u64> 
    {
        let mut stall = 1;  // Base execution time
        
        // Add memory access latency
        if let Some(mem_op) = insn.memory_operation() {
            stall += self.memory_access(
                mem_op.address,
                mem_op.size,
                mem_op.is_write,
            )?;
        }
        
        Ok(stall)
    }
}
```

### 4.4 Out-of-Order Pipeline Model

For detailed microarchitectural studies:

```rust
pub struct OoOPipelineModel {
    rob: ReorderBuffer,
    issue_queues: IssueQueues,
    execution_units: Vec<ExecutionUnit>,
    branch_predictor: Box<dyn BranchPredictor>,
    register_file: RegisterFile,
    memory_model: Box<dyn MemoryTimingModel>,
}

impl TimingModel for OoOPipelineModel {
    fn compute_stall(&mut self, insn: &Instruction, result: &ExecutionResult) 
        -> Result<u64> 
    {
        // Decode and rename
        let uop = self.decode(insn)?;
        let renamed = self.rename(&uop)?;
        
        // Allocate ROB entry
        if self.rob.is_full() {
            return Ok(self.wait_for_rob_space());
        }
        let rob_entry = self.rob.allocate(renamed);
        
        // Schedule for execution
        let issue_cycles = self.schedule_for_issue(&rob_entry)?;
        
        // Execute
        let exec_cycles = self.execute(&rob_entry)?;
        
        // Handle memory operations
        let mem_cycles = if let Some(mem_op) = rob_entry.memory_op {
            self.memory_model.access(mem_op)?
        } else {
            0
        };
        
        // Retire
        let retire_cycles = self.retire(&rob_entry)?;
        
        Ok(issue_cycles + exec_cycles + mem_cycles + retire_cycles)
    }
    
    fn branch_misprediction(&mut self, pc: u64, predicted: u64, actual: u64) 
        -> Result<u64> 
    {
        // Flush pipeline
        self.rob.flush();
        self.issue_queues.flush();
        
        // Update branch predictor
        self.branch_predictor.update(pc, actual, false);
        
        // Return misprediction penalty
        Ok(self.rob.capacity() as u64)  // ROB depth = pipeline depth
    }
}
```

### 4.5 Device Timing

Devices return stall cycles via the memory interface:

```rust
pub trait MemoryMappedDevice {
    fn read(&mut self, offset: u64, size: usize) -> Result<(u64, u64)>;
    //                                                        ^data ^stall
    
    fn write(&mut self, offset: u64, size: usize, value: u64) -> Result<u64>;
    //                                                                   ^stall
}

// Example: UART with realistic timing
pub struct UartDevice {
    baud_rate: u32,
    cpu_frequency: u64,
}

impl MemoryMappedDevice for UartDevice {
    fn read(&mut self, offset: u64, _size: usize) -> Result<(u64, u64)> {
        match offset {
            TX_DATA => {
                let data = self.read_tx_fifo();
                // UART register access takes 10 cycles
                Ok((data, 10))
            }
            STATUS => {
                let status = self.compute_status();
                Ok((status, 5))
            }
            _ => Ok((0, 1)),
        }
    }
    
    fn write(&mut self, offset: u64, _size: usize, value: u64) -> Result<u64> {
        match offset {
            TX_DATA => {
                self.transmit_byte(value as u8);
                
                // Calculate transmission time in CPU cycles
                // At 115200 baud, 10 bits per byte = ~87 μs per byte
                let byte_time_us = 87;
                let cpu_cycles = (self.cpu_frequency / 1_000_000) * byte_time_us;
                
                // Schedule completion event
                self.schedule_tx_complete(cpu_cycles);
                
                Ok(cpu_cycles)
            }
            _ => Ok(1),
        }
    }
}
```

---

## 5. Virtual Time Management

### 5.1 Virtual Time vs Real Time

```rust
pub struct VirtualTimeManager {
    virtual_cycles: u64,        // Simulated cycles
    real_time_start: Instant,   // Host wall-clock start
    cpu_frequency: u64,         // Simulated CPU frequency (Hz)
}

impl VirtualTimeManager {
    pub fn virtual_to_real_seconds(&self, cycles: u64) -> f64 {
        cycles as f64 / self.cpu_frequency as f64
    }
    
    pub fn real_time_elapsed(&self) -> Duration {
        Instant::now() - self.real_time_start
    }
    
    pub fn simulation_speed(&self) -> f64 {
        let virtual_seconds = self.virtual_to_real_seconds(self.virtual_cycles);
        let real_seconds = self.real_time_elapsed().as_secs_f64();
        
        if real_seconds > 0.0 {
            virtual_seconds / real_seconds
        } else {
            0.0
        }
    }
    
    pub fn mips(&self, instructions: u64) -> f64 {
        let real_seconds = self.real_time_elapsed().as_secs_f64();
        if real_seconds > 0.0 {
            (instructions as f64 / 1_000_000.0) / real_seconds
        } else {
            0.0
        }
    }
}
```

### 5.2 Real-Time Mode (Optional)

For hardware-in-the-loop or interactive use:

```rust
pub struct RealTimeThrottler {
    virtual_time: VirtualTimeManager,
    sync_interval: u64,  // Sync every N cycles
}

impl RealTimeThrottler {
    pub fn throttle_if_needed(&mut self, current_cycle: u64) {
        if current_cycle % self.sync_interval == 0 {
            let virtual_seconds = self.virtual_time.virtual_to_real_seconds(current_cycle);
            let real_seconds = self.virtual_time.real_time_elapsed().as_secs_f64();
            
            if virtual_seconds > real_seconds {
                // Running too fast, sleep to catch up
                let sleep_time = Duration::from_secs_f64(virtual_seconds - real_seconds);
                std::thread::sleep(sleep_time);
            }
            // If running too slow, we can't do anything about it
        }
    }
}
```

### 5.3 Deterministic Execution

Virtual time decoupling from real time enables deterministic replay:

```rust
pub struct DeterministicExecutor {
    virtual_time: u64,
    event_log: Vec<TimestampedEvent>,
    random_seed: u64,
}

impl DeterministicExecutor {
    pub fn record_event(&mut self, event: Event) {
        self.event_log.push(TimestampedEvent {
            timestamp: self.virtual_time,
            event,
        });
    }
    
    pub fn replay_from_checkpoint(&mut self, checkpoint: Checkpoint) -> Result<()> {
        // Restore deterministic state
        self.virtual_time = checkpoint.virtual_time;
        self.restore_rng_state(checkpoint.random_seed);
        
        // Replay events in order
        for logged_event in &self.event_log {
            if logged_event.timestamp >= self.virtual_time {
                self.process_event(&logged_event.event)?;
            }
        }
        
        Ok(())
    }
}
```

---

## 6. Fast-Forwarding and Sampling

### 6.1 Sampling Methodology

For large workloads, simulate detailed only for regions of interest:

```
Boot & Init          Warmup              ROI              Cooldown
────────────────────────────────────────────────────────────────────>
Functional only     Cache warmup      Cycle-accurate    Drain pipeline
100+ MIPS           ~10 MIPS          1-10 MIPS         ~10 MIPS
Billions of insns   Millions          Millions          Thousands
```

### 6.2 Implementation

```rust
pub struct SamplingController {
    mode: SimulationMode,
    sample_size: u64,
    warmup_size: u64,
    fast_forward_target: u64,
}

pub enum SimulationMode {
    FastForward,    // Functional only, no timing
    Warmup,         // Cache sim but no stats
    Detailed,       // Full timing + statistics
}

impl SamplingController {
    pub fn run_sampled_simulation(&mut self, core: &mut CoreSimulator) 
        -> Result<Statistics> 
    {
        let mut stats = Statistics::new();
        
        // Phase 1: Fast-forward to region of interest
        core.detach_timing_model();
        core.run_instructions(self.fast_forward_target)?;
        
        // Phase 2: Warmup (populate caches, branch predictor)
        let timing = CacheHierarchyModel::new(/* config */);
        core.attach_timing_model(Box::new(timing));
        core.run_instructions(self.warmup_size)?;
        
        // Phase 3: Detailed simulation with statistics
        stats.start_collection();
        core.run_instructions(self.sample_size)?;
        stats.stop_collection();
        
        Ok(stats)
    }
}
```

### 6.3 Multi-Phase Execution

```python
# Python API for sampling
from helm import Simulation, SamplingConfig

sim = Simulation.from_file("rpi3.py")

# Configure sampling
config = SamplingConfig(
    fast_forward=1_000_000_000,  # Skip first 1B instructions
    warmup=10_000_000,           # 10M instruction warmup
    sample=100_000_000,          # 100M detailed sample
    repeat=10                    # 10 samples across execution
)

results = sim.run_sampled(
    workload="spec2017/gcc",
    config=config
)

print(f"Average IPC: {results.average_ipc()}")
print(f"L1D MPKI: {results.l1d_mpki()}")
```

---

## 7. Accuracy Levels

### 7.1 Level 0: Functional Only (QEMU-Equivalent)

**Speed**: 1000+ MIPS (matches or exceeds QEMU)  
**Use Case**: Software development, debugging, testing

```rust
let mut sim = Simulation::new();
sim.set_mode(SimulationMode::FunctionalOnly);
// No timing model attached
// IPC = 1, flat memory, instant I/O
// Uses same dynamic translation engine as QEMU
```

**Characteristics:**
- ✓ Functionally correct execution
- ✓ Fastest simulation speed (QEMU-class performance)
- ✓ Dynamic binary translation with JIT
- ✓ Can boot full OS quickly
- ✗ No timing information
- ✗ No cache effects
- ✗ No pipeline modeling

**Performance Target:**
- Match QEMU speed in syscall emulation mode
- 1000+ MIPS on modern hardware
- Boot Linux in under 1 minute
- Suitable for CI/CD and regression testing

### 7.2 Level 1: Stall-Annotated

**Speed**: 10-100 MIPS  
**Use Case**: Performance estimation, device latency studies

```rust
let timing = StallAnnotatedModel::new()
    .with_cache_latencies(/* L1: 3, L2: 12, L3: 40, DRAM: 200 */)
    .with_device_delays(/* UART: 50, SPI: 100 */)
    .build();

sim.attach_timing_model(timing);
```

**Characteristics:**
- ✓ Memory hierarchy timing (cache hit/miss latency)
- ✓ Device access delays
- ✓ Reasonable performance trends
- ✗ No instruction-level parallelism
- ✗ No branch prediction
- Accuracy: ~50-200% error vs real hardware

### 7.3 Level 2: Microarchitectural

**Speed**: 1-10 MIPS  
**Use Case**: Microarchitectural design space exploration

```rust
let timing = MicroarchitectureModel::new()
    .with_rob_size(128)
    .with_issue_width(4)
    .with_retire_width(4)
    .with_branch_predictor(TAGEPredictor::new())
    .with_cache_hierarchy(/* detailed cache config */)
    .build();

sim.attach_timing_model(timing);
```

**Characteristics:**
- ✓ Out-of-order execution
- ✓ Branch prediction and misprediction penalties
- ✓ ROB/scheduler/execution units modeled
- ✓ Instruction-level parallelism
- ✓ Realistic cache behavior
- Accuracy: ~5-20% error vs real hardware

### 7.4 Level 3: Cycle-Accurate

**Speed**: 0.1-1 MIPS  
**Use Case**: Precise architecture validation

```rust
let timing = CycleAccurateModel::new()
    .with_pipeline_stages(/* detailed stage config */)
    .with_bypass_network(/* forwarding paths */)
    .with_memory_order_buffer()
    .with_store_buffer()
    .build();

sim.attach_timing_model(timing);
```

**Characteristics:**
- ✓ Cycle-by-cycle pipeline state
- ✓ Precise modeling of all structures
- ✓ Memory ordering and disambiguation
- ✓ Can match hardware within a few percent
- ✗ Very slow simulation

### 7.5 Comparison Table

| Feature | Functional | Stall-Annotated | Microarch | Cycle-Accurate |
|---------|-----------|----------------|-----------|----------------|
| **Speed (MIPS)** | 1000+ | 10-100 | 1-10 | 0.1-1 |
| **IPC modeling** | Fixed (1.0) | Fixed (1.0) | Dynamic | Dynamic |
| **Cache hierarchy** | No | Yes (latency) | Yes (detailed) | Yes (detailed) |
| **Branch prediction** | No | No | Yes | Yes |
| **OoO execution** | No | No | Yes | Yes |
| **Pipeline stages** | No | No | Simplified | Detailed |
| **Accuracy vs HW** | N/A | ~100% error | ~10% error | ~2% error |
| **Use case** | SW dev | Perf trends | Arch research | HW validation |

---

## 8. Implementation Details

### 8.1 Core Engine Integration

```rust
// helm-engine/src/lib.rs

pub struct SimulationEngine {
    cores: Vec<CoreSimulator>,
    event_queue: EventQueue,
    decoupler: TemporalDecoupler,
    timing_mode: TimingMode,
}

impl SimulationEngine {
    pub fn run(&mut self, target_cycles: u64) -> Result<SimulationResult> {
        match self.timing_mode {
            TimingMode::EventDriven => self.run_event_driven(target_cycles),
            TimingMode::CycleDriven => self.run_cycle_driven(target_cycles),
        }
    }
    
    fn run_event_driven(&mut self, target_cycles: u64) -> Result<SimulationResult> {
        let end_time = self.event_queue.current_time() + target_cycles;
        
        while self.event_queue.current_time() < end_time {
            if let Some(event) = self.event_queue.next_event() {
                self.handle_event(event)?;
            } else {
                // No more events, advance to target
                self.event_queue.advance_time(end_time);
                break;
            }
        }
        
        Ok(self.collect_results())
    }
}
```

### 8.2 Python API

```python
from helm import Simulation, TimingModel

sim = Simulation()

# Start with functional-only (fast)
sim.mode = 'functional'
sim.run(instructions=1_000_000_000)  # Boot OS

# Switch to detailed timing for measurement
timing = TimingModel.from_config({
    'type': 'microarchitecture',
    'rob_size': 128,
    'issue_width': 4,
    'l1_cache': {'size': '32KB', 'associativity': 8},
    'l2_cache': {'size': '256KB', 'associativity': 8},
})

sim.attach_timing_model(timing)
stats = sim.run(instructions=100_000_000)  # Measure region of interest

print(f"IPC: {stats.ipc}")
print(f"L1D MPKI: {stats.l1d_miss_rate * 1000}")
```

### 8.3 Configuration File Format

```toml
# timing-config.toml

[simulation]
mode = "microarchitecture"
quantum_size = 10000  # cycles

[timing.core]
type = "out-of-order"
rob_size = 128
issue_width = 4
retire_width = 4
pipeline_depth = 14

[timing.branch_predictor]
type = "tage"
num_tables = 4
history_lengths = [8, 16, 32, 64]

[timing.cache.l1i]
size = "32KB"
associativity = 8
replacement = "lru"
hit_latency = 2

[timing.cache.l1d]
size = "32KB"
associativity = 8
replacement = "lru"
hit_latency = 3

[timing.cache.l2]
size = "256KB"
associativity = 8
replacement = "lru"
hit_latency = 12

[timing.memory]
dram_latency = 200
dram_bandwidth = "25.6GB/s"
```

---

## Conclusion

HELM's timing model architecture provides:

1. **Flexibility**: Choose the right accuracy level for your needs
2. **Speed**: Event-driven simulation with temporal decoupling
3. **Modularity**: Plug in different timing models at runtime
4. **Accuracy**: From functional-only to cycle-accurate
5. **Determinism**: Reproducible results for debugging and analysis

This design, inspired by Simics's proven approach, enables HELM to serve both software developers (who need speed) and architecture researchers (who need accuracy) with a single unified framework.
