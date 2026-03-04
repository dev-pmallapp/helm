# Adapting QEMU's QOM and QMP for HELM

**Author:** Research Analysis  
**Date:** March 4, 2026  
**Version:** 1.0

## Executive Summary

This document analyzes QEMU's Object Model (QOM) and Machine Protocol (QMP) and proposes how their core concepts can be adapted for HELM (Hybrid Emulation Layer for Microarchitecture). While QEMU focuses on full-system virtualization, HELM targets microarchitectural exploration and multi-ISA simulation. The adaptation should preserve the introspection and control capabilities while aligning with HELM's Rust-centric architecture and research-oriented use cases.

---

## Table of Contents

1. [QEMU QOM Overview](#qemu-qom-overview)
2. [QEMU QMP Overview](#qemu-qmp-overview)
3. [HELM Architecture Analysis](#helm-architecture-analysis)
4. [QOM Adaptation for HELM](#qom-adaptation-for-helm)
5. [QMP Adaptation for HELM](#qmp-adaptation-for-helm)
6. [Implementation Roadmap](#implementation-roadmap)
7. [Benefits and Use Cases](#benefits-and-use-cases)
8. [References](#references)

---

## 1. QEMU QOM Overview

### 1.1 Core Concepts

**QEMU Object Model (QOM)** provides a framework for managing the complex object hierarchy in QEMU. Key features include:

- **Dynamic Type Registration:** Types are registered at runtime using `TypeInfo` structures
- **Single Inheritance:** Types inherit from parent types (root is `TYPE_OBJECT`)
- **Multiple Interface Inheritance:** Stateless interfaces for capability composition
- **Property System:** Internal state exposed as typed, introspectable properties
- **Composition Tree:** All objects organized in a hierarchical tree (the "QOM tree")

### 1.2 Key Components

```c
// Type registration pattern
static const TypeInfo my_device_info = {
    .name = TYPE_MY_DEVICE,
    .parent = TYPE_DEVICE,
    .instance_size = sizeof(MyDevice),
    .class_init = my_device_class_init,
};
```

**Object Lifecycle:**
1. **Class Initialization:** One-time setup, virtual function table population
2. **Instance Creation:** Object instantiation via `object_new()`
3. **Device Realization:** Two-phase initialization (realize/unrealize)
4. **Property Access:** Runtime get/set of object properties
5. **Destruction:** Cleanup and deallocation

### 1.3 QOM Tree Structure

In QEMU, the QOM tree represents the entire machine composition:

```
/machine
  /peripheral
    /cpu0
    /cpu1
  /unattached
    /device[0]
  /objects
    /memory-backend-ram
```

This tree is introspectable at runtime via the monitor interface.

---

## 2. QEMU QMP Overview

### 2.1 Protocol Architecture

**QEMU Machine Protocol (QMP)** is a JSON-based control and introspection interface for QEMU instances.

**Wire Format:** Newline-delimited JSON over Unix sockets or TCP

**Message Types:**
- **Commands:** `{"execute": "<cmd>", "arguments": {...}, "id": <correlator>}`
- **Responses:** `{"return": <result>}` or `{"error": {...}}`
- **Events:** `{"event": "<name>", "data": {...}, "timestamp": {...}}`

### 2.2 QAPI Schema System

QMP commands are defined in a typed schema language (QAPI):

```
{ 'command': 'blockdev-snapshot-sync',
  'data': { 'device': 'str', 'snapshot-file': 'str', '*format': 'str' } }
```

Code generation produces:
- C marshalling/unmarshalling code
- Dispatch tables
- Runtime introspection metadata

### 2.3 Key Command Families

| Domain | Example Commands |
|--------|------------------|
| **Lifecycle** | `quit`, `stop`, `cont`, `system_reset` |
| **Introspection** | `query-status`, `query-cpus`, `qom-list`, `qom-get` |
| **Device Management** | `device_add`, `device_del` |
| **Migration** | `migrate`, `query-migrate` |
| **Block Layer** | `blockdev-add`, `block-commit` |
| **Meta** | `query-qmp-schema` (self-describing API) |

### 2.4 QOM-QMP Integration

QMP exposes the QOM tree directly:
- **`qom-list`**: List children and properties of a QOM path
- **`qom-get`**: Read property values
- **`qom-set`**: Modify runtime-writable properties
- **`qom-list-types`**: Enumerate all registered types

This integration makes the entire QEMU object model accessible over the network.

---

## 3. HELM Architecture Analysis

### 3.1 Current Structure

HELM is organized as a Rust workspace with multiple crates:

```
helm/
├── crates/
│   ├── helm-core/          # Core types, IR, events
│   ├── helm-engine/        # Simulation engine
│   ├── helm-isa/           # ISA frontends (x86, RISC-V, ARM)
│   ├── helm-memory/        # Memory hierarchy (cache, TLB, coherence)
│   ├── helm-pipeline/      # OOO pipeline (ROB, scheduler, branch pred)
│   ├── helm-translate/     # Dynamic translation engine
│   ├── helm-syscall/       # Syscall emulation
│   ├── helm-stats/         # Statistics collection
│   └── helm-python/        # Python bindings (PyO3)
└── python/
    └── helm/               # Python API for configuration
```

### 3.2 Key Architectural Properties

1. **Rust Core, Python Configuration**
   - Simulation engine in Rust for performance and safety
   - Platform definitions in Python (gem5-like approach)
   - PyO3 bridges the two languages

2. **Modular ISA Support**
   - ISA frontends translate guest instructions to internal IR
   - Backend microarchitecture is largely ISA-agnostic
   - Enables cross-ISA comparisons

3. **Hybrid Execution Modes**
   - Fast syscall emulation mode (QEMU-like)
   - Detailed cycle-accurate microarchitectural mode
   - Can switch between modes or combine them

4. **Component-Based Design**
   - Cores, caches, predictors as composable components
   - Memory hierarchy with configurable cache levels
   - Pipeline stages with configurable widths/depths

### 3.3 Configuration Model (Python)

Current approach uses Python scripts for platform definition:

```python
from helm import Platform, Core, CacheHierarchy

platform = Platform()
platform.add_core(
    Core(
        isa='riscv64',
        rob_size=128,
        issue_width=4,
        branch_predictor='tage'
    )
)
platform.add_cache_hierarchy(
    CacheHierarchy(
        l1i_size='32KB',
        l1d_size='32KB',
        l2_size='256KB',
        l3_size='8MB'
    )
)
```

---

## 4. QOM Adaptation for HELM

### 4.1 Design Philosophy

A HELM Object Model (HOM) should provide:

1. **Type System for Microarchitectural Components**
   - Cores, caches, predictors, memory controllers as typed objects
   - Clear inheritance hierarchy (e.g., `BranchPredictor` → `TAGEPredictor`)

2. **Property-Based Configuration**
   - Component parameters as introspectable properties
   - Runtime querying of configuration state
   - Validation of parameter combinations

3. **Composition Tree**
   - Hierarchical organization of simulation components
   - Reflects the actual hardware structure being modeled

4. **Lifecycle Management**
   - Clean initialization and teardown
   - Support for design space exploration (many instantiations)

### 4.2 Proposed Rust Type System

Leverage Rust's trait system for object modeling:

```rust
// Core trait for all HELM objects
pub trait HelmObject: Any + Send + Sync {
    fn object_type(&self) -> &'static str;
    fn properties(&self) -> Vec<Property>;
    fn get_property(&self, name: &str) -> Result<PropertyValue>;
    fn set_property(&mut self, name: &str, value: PropertyValue) -> Result<()>;
}

// Type registration using inventory crate
inventory::submit! {
    TypeInfo {
        name: "core.ooo-core",
        parent: Some("core"),
        description: "Out-of-order core implementation",
        constructor: Box::new(|| Box::new(OoOCore::default())),
    }
}

// Property definition
#[derive(Clone, Debug)]
pub struct Property {
    pub name: String,
    pub type_info: PropertyType,
    pub description: String,
    pub default: Option<PropertyValue>,
    pub read_only: bool,
}

#[derive(Clone, Debug)]
pub enum PropertyType {
    Int { min: Option<i64>, max: Option<i64> },
    UInt { min: Option<u64>, max: Option<u64> },
    Float { min: Option<f64>, max: Option<f64> },
    String,
    Bool,
    Enum { variants: Vec<String> },
    Object { type_name: String },
}
```

### 4.3 Component Hierarchy

Proposed object type hierarchy for HELM:

```
HelmObject (root trait)
├── Platform
│   ├── properties: name, isa_support, num_cores
│   └── children: cores, memory_hierarchy, interconnect
├── Core
│   ├── InOrderCore
│   │   └── properties: pipeline_stages, forwarding_paths
│   └── OutOfOrderCore
│       └── properties: rob_size, issue_width, retire_width
├── BranchPredictor
│   ├── BimodalPredictor
│   ├── GSharePredictor
│   └── TAGEPredictor
│       └── properties: num_tables, history_lengths
├── Cache
│   ├── L1Cache
│   │   └── properties: size, associativity, replacement_policy
│   ├── L2Cache
│   └── L3Cache
├── MemoryController
│   └── properties: dram_type, frequency, channels
├── Interconnect
│   ├── Bus
│   └── NoC
└── ISAFrontend
    ├── X86Frontend
    ├── RISCVFrontend
    └── ARMFrontend
```

### 4.4 Object Composition Tree

HELM simulations would maintain a tree structure:

```
/platform
  /cores
    /core0 (OutOfOrderCore)
      /fetch-unit
      /decode-unit
      /rename-unit
      /rob
      /issue-queues
      /execution-units
        /alu0
        /alu1
        /fpu0
        /mem-unit
      /branch-predictor (TAGEPredictor)
    /core1
  /memory-hierarchy
    /l1i-core0 (L1Cache)
    /l1d-core0 (L1Cache)
    /l2-shared (L2Cache)
    /l3-shared (L3Cache)
    /memory-controller
  /interconnect (NoC)
  /stats-collectors
    /core-stats
    /cache-stats
    /power-model
```

### 4.5 Property System Implementation

```rust
// Example: OOO Core with properties
pub struct OoOCore {
    // Instance state
    rob: ReorderBuffer,
    issue_width: usize,
    retire_width: usize,
    
    // Property metadata
    properties: HashMap<String, Property>,
}

impl HelmObject for OoOCore {
    fn properties(&self) -> Vec<Property> {
        vec![
            Property {
                name: "rob_size".into(),
                type_info: PropertyType::UInt { 
                    min: Some(16), 
                    max: Some(512) 
                },
                description: "Reorder buffer size".into(),
                default: Some(PropertyValue::UInt(128)),
                read_only: false,
            },
            Property {
                name: "issue_width".into(),
                type_info: PropertyType::UInt { 
                    min: Some(1), 
                    max: Some(16) 
                },
                description: "Instruction issue width".into(),
                default: Some(PropertyValue::UInt(4)),
                read_only: false,
            },
            Property {
                name: "ipc".into(),
                type_info: PropertyType::Float { 
                    min: Some(0.0), 
                    max: None 
                },
                description: "Current instructions per cycle".into(),
                default: None,
                read_only: true,  // Runtime statistic
            },
        ]
    }
    
    fn get_property(&self, name: &str) -> Result<PropertyValue> {
        match name {
            "rob_size" => Ok(PropertyValue::UInt(self.rob.capacity() as u64)),
            "issue_width" => Ok(PropertyValue::UInt(self.issue_width as u64)),
            "ipc" => Ok(PropertyValue::Float(self.calculate_ipc())),
            _ => Err(anyhow!("Unknown property: {}", name)),
        }
    }
    
    fn set_property(&mut self, name: &str, value: PropertyValue) -> Result<()> {
        match (name, value) {
            ("rob_size", PropertyValue::UInt(size)) => {
                self.rob.resize(size as usize)?;
                Ok(())
            }
            ("issue_width", PropertyValue::UInt(width)) => {
                self.issue_width = width as usize;
                Ok(())
            }
            ("ipc", _) => Err(anyhow!("Property 'ipc' is read-only")),
            _ => Err(anyhow!("Unknown property: {}", name)),
        }
    }
}
```

### 4.6 Type Registration

Use the `inventory` crate for compile-time type registration:

```rust
use inventory;

pub struct TypeInfo {
    pub name: &'static str,
    pub parent: Option<&'static str>,
    pub description: &'static str,
    pub constructor: Box<dyn Fn() -> Box<dyn HelmObject>>,
}

inventory::collect!(TypeInfo);

// In each component crate:
inventory::submit! {
    TypeInfo {
        name: "core.ooo-core",
        parent: Some("core"),
        description: "Out-of-order superscalar core",
        constructor: Box::new(|| Box::new(OoOCore::default())),
    }
}

// Query all registered types at runtime
pub fn list_types() -> Vec<&'static TypeInfo> {
    inventory::iter::<TypeInfo>().collect()
}
```

### 4.7 Integration with Python

Expose the object model to Python via PyO3:

```rust
use pyo3::prelude::*;

#[pyclass]
pub struct PyHelmObject {
    inner: Arc<RwLock<Box<dyn HelmObject>>>,
}

#[pymethods]
impl PyHelmObject {
    fn get_property(&self, name: &str) -> PyResult<PyObject> {
        let obj = self.inner.read().unwrap();
        let value = obj.get_property(name)
            .map_err(|e| PyErr::new::<pyo3::exceptions::PyValueError, _>(e.to_string()))?;
        
        Python::with_gil(|py| {
            Ok(match value {
                PropertyValue::Int(v) => v.into_py(py),
                PropertyValue::UInt(v) => v.into_py(py),
                PropertyValue::Float(v) => v.into_py(py),
                PropertyValue::String(v) => v.into_py(py),
                PropertyValue::Bool(v) => v.into_py(py),
            })
        })
    }
    
    fn set_property(&mut self, name: &str, value: &PyAny) -> PyResult<()> {
        let mut obj = self.inner.write().unwrap();
        
        let prop_value = if let Ok(v) = value.extract::<i64>() {
            PropertyValue::Int(v)
        } else if let Ok(v) = value.extract::<u64>() {
            PropertyValue::UInt(v)
        } else if let Ok(v) = value.extract::<f64>() {
            PropertyValue::Float(v)
        } else if let Ok(v) = value.extract::<String>() {
            PropertyValue::String(v)
        } else if let Ok(v) = value.extract::<bool>() {
            PropertyValue::Bool(v)
        } else {
            return Err(PyErr::new::<pyo3::exceptions::PyTypeError, _>("Unsupported type"));
        };
        
        obj.set_property(name, prop_value)
            .map_err(|e| PyErr::new::<pyo3::exceptions::PyValueError, _>(e.to_string()))
    }
    
    fn list_properties(&self) -> PyResult<Vec<String>> {
        let obj = self.inner.read().unwrap();
        Ok(obj.properties().iter().map(|p| p.name.clone()).collect())
    }
}
```

---

## 5. QMP Adaptation for HELM

### 5.1 Design Philosophy

A HELM Management Protocol (HMP) should provide:

1. **Runtime Introspection**
   - Query simulation state (cycle count, IPC, cache statistics)
   - Inspect object hierarchy and properties
   - Monitor microarchitectural events

2. **Control Interface**
   - Start/stop/step simulation
   - Checkpoint/restore state
   - Hot-reconfigure parameters (within limits)

3. **Event Streaming**
   - Asynchronous notifications of important events
   - Branch mispredictions, cache misses, pipeline stalls
   - Custom event filtering for analysis

4. **Self-Describing API**
   - Schema introspection for tool generation
   - Version negotiation for compatibility

### 5.2 Protocol Design

**Transport:** JSON over Unix sockets, TCP, or WebSockets (for browser integration)

**Message Format:** Similar to QMP but adapted for simulation:

```json
// Command
{
  "execute": "sim-run",
  "arguments": {
    "cycles": 1000000,
    "until": "syscall"
  },
  "id": "cmd-001"
}

// Response
{
  "return": {
    "cycles_executed": 1000000,
    "instructions_retired": 850000,
    "stop_reason": "cycle-limit"
  },
  "id": "cmd-001"
}

// Event
{
  "event": "BRANCH_MISPREDICTION",
  "data": {
    "core": 0,
    "pc": "0x401234",
    "predicted_target": "0x401240",
    "actual_target": "0x401500",
    "cycle": 12345678
  },
  "timestamp": {
    "sim_cycle": 12345678,
    "wall_time_us": 123456
  }
}
```

### 5.3 Command Categories

#### 5.3.1 Simulation Control

```rust
pub enum SimulationCommand {
    /// Start or resume simulation
    SimRun {
        cycles: Option<u64>,
        instructions: Option<u64>,
        until: Option<StopCondition>,
    },
    
    /// Pause simulation
    SimStop,
    
    /// Step simulation by one cycle
    SimStep { count: u64 },
    
    /// Reset simulation to initial state
    SimReset,
    
    /// Terminate simulation
    Quit,
}

#[derive(Serialize, Deserialize)]
pub enum StopCondition {
    CycleLimit,
    InstructionLimit,
    Syscall,
    Breakpoint,
    Event(String),
}
```

#### 5.3.2 Introspection Commands

```rust
pub enum IntrospectionCommand {
    /// Query current simulation status
    QueryStatus,
    
    /// Get statistics for a component
    QueryStats {
        path: String,
        reset: bool,
    },
    
    /// List children of an object
    ObjectList { path: String },
    
    /// Get property value
    ObjectGet {
        path: String,
        property: String,
    },
    
    /// Set property value
    ObjectSet {
        path: String,
        property: String,
        value: serde_json::Value,
    },
    
    /// List all registered types
    QueryTypes,
    
    /// Get the command schema
    QuerySchema,
}
```

#### 5.3.3 Checkpoint Commands

```rust
pub enum CheckpointCommand {
    /// Save simulation state
    CheckpointSave { path: String },
    
    /// Restore simulation state
    CheckpointLoad { path: String },
    
    /// List available checkpoints
    CheckpointList,
}
```

#### 5.3.4 Analysis Commands

```rust
pub enum AnalysisCommand {
    /// Subscribe to specific events
    EventSubscribe {
        events: Vec<String>,
        filter: Option<EventFilter>,
    },
    
    /// Unsubscribe from events
    EventUnsubscribe { events: Vec<String> },
    
    /// Set breakpoint (PC, cycle, instruction count)
    BreakpointAdd {
        location: BreakpointLocation,
        condition: Option<String>,
    },
    
    /// Remove breakpoint
    BreakpointRemove { id: u64 },
    
    /// Enable/disable tracing
    TraceControl {
        enable: bool,
        components: Vec<String>,
    },
}
```

### 5.4 Event System

Define events for microarchitectural phenomena:

```rust
#[derive(Serialize, Deserialize, Debug)]
pub enum SimulationEvent {
    // Core events
    BranchMisprediction {
        core_id: usize,
        pc: u64,
        predicted: u64,
        actual: u64,
        penalty_cycles: u64,
    },
    
    ROBFull {
        core_id: usize,
        stall_cycles: u64,
    },
    
    // Memory events
    CacheMiss {
        cache_level: usize,
        address: u64,
        miss_type: String, // "read", "write", "fetch"
        penalty_cycles: u64,
    },
    
    TLBMiss {
        address: u64,
        penalty_cycles: u64,
    },
    
    MemoryOrder Violation {
        load_pc: u64,
        store_pc: u64,
    },
    
    // Syscall events
    SyscallEntry {
        number: i64,
        args: Vec<u64>,
    },
    
    SyscallExit {
        number: i64,
        result: i64,
    },
    
    // Lifecycle events
    SimulationStarted,
    SimulationStopped { reason: String },
    CheckpointCreated { path: String },
    
    // Custom events
    Custom {
        name: String,
        data: serde_json::Value,
    },
}
```

### 5.5 Implementation Architecture

```rust
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::mpsc;
use serde::{Deserialize, Serialize};

pub struct HmpServer {
    listener: UnixListener,
    command_tx: mpsc::Sender<HmpCommand>,
    event_rx: mpsc::Receiver<SimulationEvent>,
    clients: Vec<HmpClient>,
}

impl HmpServer {
    pub async fn run(&mut self) -> Result<()> {
        loop {
            tokio::select! {
                // Accept new clients
                Ok((stream, _)) = self.listener.accept() => {
                    let client = HmpClient::new(stream, self.event_rx.resubscribe());
                    self.clients.push(client);
                }
                
                // Broadcast events to all clients
                Some(event) = self.event_rx.recv() => {
                    for client in &mut self.clients {
                        client.send_event(&event).await?;
                    }
                }
            }
        }
    }
}

pub struct HmpClient {
    stream: UnixStream,
    event_filter: EventFilter,
}

impl HmpClient {
    async fn handle_command(&mut self, cmd: HmpCommand) -> Result<HmpResponse> {
        match cmd {
            HmpCommand::SimRun { cycles, .. } => {
                // Send to simulation engine
                self.command_tx.send(cmd).await?;
                // Wait for response
                Ok(HmpResponse::Success { data: json!({}) })
            }
            
            HmpCommand::ObjectGet { path, property } => {
                let tree = object_tree();
                let obj = tree.get(&path)?;
                let value = obj.get_property(&property)?;
                Ok(HmpResponse::Success { data: value.to_json() })
            }
            
            // ... handle other commands
        }
    }
    
    async fn send_event(&mut self, event: &SimulationEvent) -> Result<()> {
        if self.event_filter.matches(event) {
            let json = serde_json::to_string(&json!({
                "event": event.event_name(),
                "data": event,
                "timestamp": get_timestamp(),
            }))?;
            self.stream.write_all(json.as_bytes()).await?;
            self.stream.write_all(b"\n").await?;
        }
        Ok(())
    }
}
```

### 5.6 Schema Generation

Provide self-describing API similar to `query-qmp-schema`:

```rust
use schemars::{schema_for, JsonSchema};

#[derive(JsonSchema, Serialize, Deserialize)]
pub enum HmpCommand {
    #[serde(rename = "sim-run")]
    SimRun {
        /// Number of cycles to execute
        cycles: Option<u64>,
        /// Stop condition
        until: Option<StopCondition>,
    },
    // ... other commands
}

pub fn get_schema() -> serde_json::Value {
    let schema = schema_for!(HmpCommand);
    serde_json::to_value(schema).unwrap()
}

// Command: {"execute": "query-schema"}
// Response: { "return": <full JSON schema> }
```

### 5.7 Python Integration

Expose HMP client in Python:

```python
import helm
import json

# Create simulation
sim = helm.Simulation()
sim.configure(...)

# Start HMP server
hmp_server = sim.start_hmp_server("/tmp/helm-sim.sock")

# Python client
client = helm.HmpClient("/tmp/helm-sim.sock")

# Execute commands
result = client.execute("sim-run", {"cycles": 1000000})
print(f"Executed {result['cycles_executed']} cycles")

# Subscribe to events
def on_cache_miss(event):
    print(f"Cache miss at address {event['address']}")

client.subscribe("CACHE_MISS", callback=on_cache_miss)

# Query object properties
ipc = client.get_property("/platform/cores/core0", "ipc")
print(f"Current IPC: {ipc}")
```

---

## 6. Implementation Roadmap

### Phase 1: Core Object Model (4-6 weeks)

**Objectives:**
- Implement `HelmObject` trait and property system
- Create base types for cores, caches, predictors
- Build object composition tree
- Add type registration mechanism

**Deliverables:**
- `helm-object` crate with core abstractions
- Properties for major component types
- Python bindings for object access
- Unit tests for property get/set

### Phase 2: Introspection API (3-4 weeks)

**Objectives:**
- Implement object tree navigation
- Add statistics as read-only properties
- Create schema generation
- Build command infrastructure

**Deliverables:**
- Object listing and property queries
- Statistics integration with property system
- JSON schema generation
- Basic command handler framework

### Phase 3: HMP Protocol (4-6 weeks)

**Objectives:**
- Implement JSON protocol over Unix sockets
- Add simulation control commands
- Build event system
- Create Python client library

**Deliverables:**
- HMP server in `helm-hmp` crate
- Core commands (run, stop, step, query)
- Event subscription and filtering
- Python `HmpClient` class

### Phase 4: Advanced Features (6-8 weeks)

**Objectives:**
- Checkpointing integration
- Breakpoint support
- Hot-reconfiguration (where safe)
- Performance monitoring events

**Deliverables:**
- Checkpoint save/restore via HMP
- Conditional breakpoints
- Runtime parameter adjustment
- Rich event streaming (branch mispredict, cache miss, etc.)

### Phase 5: Tooling & Integration (4-6 weeks)

**Objectives:**
- CLI tool for HMP interaction
- Web dashboard (WebSocket support)
- Integration with analysis tools
- Documentation and examples

**Deliverables:**
- `helm-ctl` command-line tool
- Web-based monitoring dashboard
- Example scripts for common workflows
- Comprehensive documentation

**Total Timeline:** ~6 months for full implementation

---

## 7. Benefits and Use Cases

### 7.1 Benefits

#### For Researchers
- **Interactive Exploration:** Query and modify simulation state without restarting
- **Automated Experiments:** Script design space sweeps using HMP commands
- **Real-Time Monitoring:** Track microarchitectural events as they occur
- **Reproducibility:** Checkpoints and command logs for reproducing results

#### For Tool Developers
- **Standard Interface:** Consistent API for building analysis tools
- **Self-Describing:** Query schema at runtime, no version lock-in
- **Event-Driven Analysis:** React to specific microarchitectural events
- **Language Agnostic:** JSON protocol accessible from any language

#### For Educators
- **Visualization:** Build interactive visualizations of pipeline behavior
- **Step-Through Debugging:** Single-step through microarchitectural execution
- **Parameter Exploration:** Students can experiment with different configurations
- **Live Dashboards:** Real-time display of performance metrics

### 7.2 Use Case Examples

#### Use Case 1: Design Space Exploration

```python
import helm
import itertools

# Parameter sweep
rob_sizes = [64, 128, 256]
issue_widths = [2, 4, 6, 8]
predictor_types = ['bimodal', 'gshare', 'tage']

results = []
for rob, width, pred in itertools.product(rob_sizes, issue_widths, predictor_types):
    # Configure via HMP
    client = helm.HmpClient(create_new_simulation())
    client.set_property("/platform/cores/core0", "rob_size", rob)
    client.set_property("/platform/cores/core0", "issue_width", width)
    client.set_property("/platform/cores/core0/branch-predictor", "type", pred)
    
    # Run simulation
    client.execute("sim-run", {"instructions": 1_000_000_000})
    
    # Collect statistics
    stats = client.execute("query-stats", {"path": "/platform/cores/core0"})
    results.append({
        'config': (rob, width, pred),
        'ipc': stats['ipc'],
        'mpki': stats['branch_mpki'],
    })

# Analyze results
best_config = max(results, key=lambda r: r['ipc'])
print(f"Best configuration: {best_config}")
```

#### Use Case 2: Real-Time Visualization

```python
import helm
import matplotlib.pyplot as plt
from matplotlib.animation import FuncAnimation

client = helm.HmpClient("/tmp/helm-sim.sock")

# Track IPC over time
ipc_history = []

def update_plot(frame):
    stats = client.execute("query-stats", {"path": "/platform/cores/core0"})
    ipc_history.append(stats['ipc'])
    plt.clf()
    plt.plot(ipc_history)
    plt.ylabel('IPC')
    plt.xlabel('Sample')
    plt.title('Instructions Per Cycle Over Time')

# Start simulation in background
client.execute("sim-run", {"cycles": None})  # Run indefinitely

# Animate
fig = plt.figure()
ani = FuncAnimation(fig, update_plot, interval=100)
plt.show()
```

#### Use Case 3: Event-Driven Analysis

```python
import helm

client = helm.HmpClient("/tmp/helm-sim.sock")

# Track branch prediction accuracy
mispredictions = 0
predictions = 0

def on_branch(event):
    global mispredictions, predictions
    predictions += 1
    if event['event'] == 'BRANCH_MISPREDICTION':
        mispredictions += 1
        print(f"Misprediction at PC {hex(event['data']['pc'])}")
        print(f"Current accuracy: {100 * (1 - mispredictions/predictions):.2f}%")

# Subscribe to branch events
client.subscribe("BRANCH_MISPREDICTION", callback=on_branch)
client.execute("sim-run", {"instructions": 100_000})

print(f"Final accuracy: {100 * (1 - mispredictions/predictions):.2f}%")
```

#### Use Case 4: Interactive Debugging

```python
import helm

client = helm.HmpClient("/tmp/helm-sim.sock")

# Set breakpoint at specific PC
client.execute("breakpoint-add", {
    "location": {"pc": 0x401234},
    "condition": "core0.rob_occupancy > 100"
})

# Run until breakpoint
result = client.execute("sim-run", {})

if result['stop_reason'] == 'breakpoint':
    # Inspect state
    rob_state = client.get_property("/platform/cores/core0/rob", "occupancy")
    pc = client.get_property("/platform/cores/core0", "pc")
    
    print(f"Stopped at PC {hex(pc)}")
    print(f"ROB occupancy: {rob_state}")
    
    # Examine pipeline
    pipeline = client.execute("object-list", {"path": "/platform/cores/core0"})
    for stage in pipeline['children']:
        state = client.get_property(f"/platform/cores/core0/{stage}", "state")
        print(f"{stage}: {state}")
```

---

## 8. References

### 8.1 QEMU Documentation

- [QEMU Object Model (QOM)](https://www.qemu.org/docs/master/devel/qom.html)
- [QMP Protocol Specification](https://wiki.qemu.org/Documentation/QMP)
- [QAPI Code Generator](https://www.qemu.org/docs/master/devel/qapi-code-gen.html)

### 8.2 Related Projects

- **gem5:** Python-based simulation configuration, similar hybrid approach
- **Simics:** Checkpoint-restart and scriptable debugging
- **Spike (RISC-V ISA Simulator):** Interactive debugging features
- **Sniper:** Multi-core simulation with statistics API

### 8.3 Rust Ecosystem

- **PyO3:** Rust-Python bindings
- **serde:** Serialization framework
- **tokio:** Async runtime for network services
- **inventory:** Compile-time type registration
- **schemars:** JSON Schema generation from Rust types

---

## Appendix A: Example Command Reference

### Simulation Control

| Command | Arguments | Description |
|---------|-----------|-------------|
| `sim-run` | `cycles`, `instructions`, `until` | Execute simulation |
| `sim-stop` | - | Pause simulation |
| `sim-step` | `count` | Step by N cycles |
| `sim-reset` | - | Reset to initial state |
| `quit` | - | Terminate simulation |

### Introspection

| Command | Arguments | Description |
|---------|-----------|-------------|
| `query-status` | - | Get simulation status |
| `query-stats` | `path`, `reset` | Get component statistics |
| `query-types` | - | List registered types |
| `query-schema` | - | Get command schema |
| `object-list` | `path` | List object children |
| `object-get` | `path`, `property` | Get property value |
| `object-set` | `path`, `property`, `value` | Set property value |

### Events

| Event | Data Fields | Description |
|-------|-------------|-------------|
| `BRANCH_MISPREDICTION` | `core_id`, `pc`, `predicted`, `actual` | Branch predictor miss |
| `CACHE_MISS` | `level`, `address`, `type` | Cache miss occurred |
| `ROB_FULL` | `core_id`, `stall_cycles` | ROB capacity exhausted |
| `SYSCALL_ENTRY` | `number`, `args` | Syscall invoked |
| `SIMULATION_STOPPED` | `reason` | Simulation halted |

---

## Appendix B: Code Examples

### B.1 Full Object Implementation

```rust
use helm_object::{HelmObject, Property, PropertyType, PropertyValue};
use std::collections::HashMap;

pub struct TAGEPredictor {
    num_tables: usize,
    history_lengths: Vec<usize>,
    tables: Vec<PredictionTable>,
    stats: PredictorStats,
}

impl HelmObject for TAGEPredictor {
    fn object_type(&self) -> &'static str {
        "branch-predictor.tage"
    }
    
    fn properties(&self) -> Vec<Property> {
        vec![
            Property {
                name: "num_tables".into(),
                type_info: PropertyType::UInt { min: Some(1), max: Some(16) },
                description: "Number of prediction tables".into(),
                default: Some(PropertyValue::UInt(4)),
                read_only: false,
            },
            Property {
                name: "accuracy".into(),
                type_info: PropertyType::Float { min: Some(0.0), max: Some(1.0) },
                description: "Prediction accuracy rate".into(),
                default: None,
                read_only: true,
            },
            Property {
                name: "mpki".into(),
                type_info: PropertyType::Float { min: Some(0.0), max: None },
                description: "Mispredictions per thousand instructions".into(),
                default: None,
                read_only: true,
            },
        ]
    }
    
    fn get_property(&self, name: &str) -> Result<PropertyValue> {
        match name {
            "num_tables" => Ok(PropertyValue::UInt(self.num_tables as u64)),
            "accuracy" => {
                let total = self.stats.predictions as f64;
                let correct = total - self.stats.mispredictions as f64;
                Ok(PropertyValue::Float(if total > 0.0 { correct / total } else { 0.0 }))
            }
            "mpki" => {
                let insns = self.stats.instructions as f64;
                let mpki = if insns > 0.0 {
                    (self.stats.mispredictions as f64 / insns) * 1000.0
                } else {
                    0.0
                };
                Ok(PropertyValue::Float(mpki))
            }
            _ => Err(anyhow!("Unknown property: {}", name)),
        }
    }
    
    fn set_property(&mut self, name: &str, value: PropertyValue) -> Result<()> {
        match (name, value) {
            ("num_tables", PropertyValue::UInt(n)) => {
                self.resize_tables(n as usize)?;
                Ok(())
            }
            (prop, _) if ["accuracy", "mpki"].contains(&prop) => {
                Err(anyhow!("Property '{}' is read-only", prop))
            }
            _ => Err(anyhow!("Unknown property: {}", name)),
        }
    }
}
```

### B.2 HMP Server Setup

```rust
use helm_hmp::{HmpServer, HmpConfig};
use tokio;

#[tokio::main]
async fn main() -> Result<()> {
    // Create simulation
    let mut sim = Simulation::new()?;
    sim.configure_from_file("platform.toml")?;
    
    // Start HMP server
    let config = HmpConfig {
        socket_path: "/tmp/helm-sim.sock".into(),
        enable_tcp: Some(("127.0.0.1", 9999)),
        event_buffer_size: 1000,
    };
    
    let (mut server, command_rx, event_tx) = HmpServer::new(config)?;
    
    // Spawn server task
    tokio::spawn(async move {
        server.run().await.unwrap();
    });
    
    // Main simulation loop
    loop {
        // Check for commands
        if let Ok(cmd) = command_rx.try_recv() {
            match cmd {
                HmpCommand::SimRun { cycles, .. } => {
                    sim.run(cycles.unwrap_or(u64::MAX))?;
                }
                HmpCommand::SimStop => {
                    sim.pause();
                }
                // ... handle other commands
            }
        }
        
        // Execute simulation step
        sim.step()?;
        
        // Send events
        for event in sim.collect_events() {
            event_tx.send(event)?;
        }
    }
}
```

---

## Conclusion

Adapting QEMU's QOM and QMP concepts for HELM provides a powerful foundation for introspection, control, and automation of microarchitectural simulations. The proposed design:

1. **Leverages Rust's strengths** while maintaining Python's configurability
2. **Preserves QOM/QMP's proven patterns** adapted to simulation domain
3. **Enables rich tooling ecosystem** through standardized interfaces
4. **Supports research workflows** with interactive and automated exploration
5. **Maintains performance** through careful async design and event filtering

Implementation should proceed incrementally, with early focus on core object model and basic introspection, followed by protocol implementation and advanced features. The result will be a significantly more powerful and accessible simulation framework for microarchitectural research.
