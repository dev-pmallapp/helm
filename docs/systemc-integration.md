# SystemC Integration for HELM

**Date:** March 4, 2026  
**Version:** 1.0

## Executive Summary

This document describes HELM's architecture for integrating SystemC/TLM-2.0 modules, enabling hardware/software co-simulation, RTL co-verification, and mixed-fidelity modeling. Drawing from proven approaches in QEMU, gem5, and Simics, HELM provides a flexible bridge architecture that balances performance with accuracy.

---

## Table of Contents

1. [SystemC and TLM Overview](#systemc-and-tlm-overview)
2. [Integration Approaches in Existing Simulators](#integration-approaches-in-existing-simulators)
3. [HELM SystemC Architecture](#helm-systemc-architecture)
4. [TLM Bridge Implementation](#tlm-bridge-implementation)
5. [Time Synchronization](#time-synchronization)
6. [Use Cases](#use-cases)
7. [Performance Considerations](#performance-considerations)
8. [Implementation Roadmap](#implementation-roadmap)

---

## 1. SystemC and TLM Overview

### 1.1 What is SystemC?

**SystemC** (IEEE 1666) is a C++ class library providing:
- Event-driven simulation kernel
- Hardware-oriented constructs (`sc_module`, `sc_signal`, `sc_clock`)
- Cooperative coroutine-based processes (`SC_THREAD`, `SC_METHOD`)
- Time-ordered event queue

```cpp
// Simple SystemC module
SC_MODULE(Counter) {
    sc_in<bool> clk;
    sc_out<int> count;
    
    void counter_process() {
        int val = 0;
        while (true) {
            wait();  // Wait for clock edge
            val++;
            count.write(val);
        }
    }
    
    SC_CTOR(Counter) {
        SC_THREAD(counter_process);
        sensitive << clk.pos();
    }
};
```

### 1.2 Transaction Level Modeling (TLM-2.0)

**TLM-2.0** provides standardized interoperability for memory-mapped transactions:

**Core Concepts:**
- **Transactions**: `tlm_generic_payload` encapsulates address, command, data, length
- **Sockets**: `tlm_initiator_socket` and `tlm_target_socket` for connections
- **Transport**: Blocking (`b_transport`) or non-blocking (`nb_transport_fw/bw`)

```cpp
// TLM-2.0 example
struct MemoryDevice : sc_module {
    tlm_target_socket<> socket;
    
    virtual void b_transport(tlm_generic_payload& trans, sc_time& delay) {
        // Read address, command, data
        uint64_t addr = trans.get_address();
        tlm_command cmd = trans.get_command();
        unsigned char* data = trans.get_data_ptr();
        unsigned int len = trans.get_data_length();
        
        // Process transaction
        if (cmd == TLM_READ_COMMAND) {
            // Fill data buffer
            memcpy(data, &memory[addr], len);
        } else {
            // Write to memory
            memcpy(&memory[addr], data, len);
        }
        
        // Annotate timing
        delay += sc_time(10, SC_NS);  // 10ns access latency
        trans.set_response_status(TLM_OK_RESPONSE);
    }
    
    SC_CTOR(MemoryDevice) {
        socket.register_b_transport(this, &MemoryDevice::b_transport);
    }
};
```

### 1.3 Coding Styles

**Loosely-Timed (LT):**
- Fast simulation via temporal decoupling
- `b_transport()` blocking calls
- Initiator runs ahead using quantum keeper
- Typical speed: 10-100 MIPS

**Approximately-Timed (AT):**
- Models transaction phases and timing points
- Non-blocking `nb_transport_fw/bw()`
- Captures pipelining and arbitration
- Typical speed: 0.1-10 MIPS

---

## 2. Integration Approaches in Existing Simulators

### 2.1 QEMU + SystemC

**Status**: No native support, requires external bridges

**Xilinx/AMD Approach (libsystemctlm-cosim):**
```
┌──────────────┐         ┌──────────────────┐
│     QEMU     │  Socket │    SystemC       │
│              │◄───────►│                  │
│  Bridge Dev  │   Wire  │  TLM Bridge      │
│  (SysBusDev) │ Protocol│  (sc_module)     │
└──────────────┘         └──────────────────┘
                              │
                              ▼
                         TLM Targets
                      (Custom IP models)
```

**Key Features:**
- Socket-based IPC (Unix domain or TCP)
- Custom wire protocol for transactions
- Requires `icount` mode for time tracking
- Quantum-based synchronization

**Limitations:**
- High overhead from serialization
- Not upstream in QEMU
- Limited to memory transactions

### 2.2 gem5 + SystemC

**Status**: First-party support in gem5 source tree

**Architecture:**
```
┌─────────────────────────────────────────────┐
│      gem5 Compiled as libgem5.so            │
│  ┌────────────────────────────────────────┐ │
│  │  Gem5SystemC::Module (sc_module)       │ │
│  │  • Wraps gem5 EventQueue               │ │
│  │  • Syncs curTick() ↔ sc_time_stamp()   │ │
│  └────────────────────────────────────────┘ │
│  ┌────────────────────────────────────────┐ │
│  │  TLM Bridges (in-process)              │ │
│  │  • Gem5ToTlmBridge: gem5 → SystemC     │ │
│  │  • TlmToGem5Bridge: SystemC → gem5     │ │
│  └────────────────────────────────────────┘ │
└─────────────────────────────────────────────┘
              │
              ▼
      SystemC sc_main() drives everything
```

**Key Features:**
- In-process integration (no serialization)
- Direct function calls through TLM sockets
- Both LT and AT mode support
- Shared time base (ticks ↔ sc_time)

**Benefits:**
- Low overhead (~1.5-2× slowdown)
- Clean integration
- Official support

### 2.3 Simics + SystemC

**Status**: Commercial SystemC Library module

**Architecture:**
```
┌──────────────────────────────────────────────┐
│        Simics Core                           │
│  ┌────────────────────────────────────────┐  │
│  │  SystemC Library Module                │  │
│  │  • Embedded OSCI kernel                │  │
│  │  • Gasket layer                        │  │
│  │  • Simics ↔ TLM conversion             │  │
│  └────────────────────────────────────────┘  │
│  ┌────────────────────────────────────────┐  │
│  │  SystemC Cell (temporal decoupling)    │  │
│  │  • Participates in cell sync           │  │
│  │  • Quantum-based execution             │  │
│  └────────────────────────────────────────┘  │
└──────────────────────────────────────────────┘
```

**Key Features:**
- Integrated with Simics cell architecture
- Gaskets handle bidirectional transactions
- TLM extensions for Simics-specific metadata
- Full checkpoint/reverse-execution support
- Most mature commercial solution

---

## 3. HELM SystemC Architecture

### 3.1 Design Goals

HELM's SystemC integration should:

1. **Support both in-process and out-of-process** co-simulation
2. **Leverage Rust's FFI** for clean C++ interop
3. **Minimize performance overhead** via direct calls when possible
4. **Align with HELM's timing levels** (LT for Level 0-1, AT for Level 2-3)
5. **Enable RTL co-simulation** via Verilator or commercial tools
6. **Maintain determinism** for replay and debugging

### 3.2 Proposed Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                 HELM Core (Rust)                            │
│  ┌───────────────────────────────────────────────────────┐  │
│  │  SystemC Bridge Manager                               │  │
│  │  • Manages SystemC library lifecycle                  │  │
│  │  • Synchronizes event queues                          │  │
│  │  • Routes transactions                                │  │
│  └───────────────────────────────────────────────────────┘  │
│                          │                                   │
│           ┌──────────────┼──────────────┐                   │
│           │              │              │                   │
│  ┌────────▼──────┐  ┌───▼─────────┐ ┌──▼────────────────┐  │
│  │ In-Process    │  │ Socket-Based│ │ Shared Memory     │  │
│  │ Bridge (fast) │  │ Bridge      │ │ Bridge (balanced) │  │
│  └────────┬──────┘  └───┬─────────┘ └──┬────────────────┘  │
│           │             │               │                   │
└───────────┼─────────────┼───────────────┼───────────────────┘
            │             │               │
            ▼             ▼               ▼
   ┌────────────────────────────────────────────┐
   │         SystemC Environment                 │
   │  ┌──────────────────────────────────────┐  │
   │  │  HELM-TLM Bridge (C++ sc_module)     │  │
   │  │  • tlm_target_socket (from HELM)     │  │
   │  │  • tlm_initiator_socket (to HELM)    │  │
   │  └──────────────────────────────────────┘  │
   │  ┌──────────────────────────────────────┐  │
   │  │  User SystemC Models                 │  │
   │  │  • Custom peripherals                │  │
   │  │  • RTL wrappers (Verilator)          │  │
   │  │  • Vendor IP (TLM models)            │  │
   │  └──────────────────────────────────────┘  │
   └────────────────────────────────────────────┘
```

### 3.3 Bridge Variants

#### Option 1: In-Process Bridge (Best Performance)

HELM links with `libsystemc.so` and calls SystemC kernel directly via FFI:

```rust
// helm-systemc/src/bridge.rs
use cxx::{CxxString, UniquePtr};

#[cxx::bridge]
mod ffi {
    unsafe extern "C++" {
        include!("helm-systemc/bridge.hpp");
        
        type SystemCKernel;
        type TlmTransaction;
        
        fn create_systemc_kernel() -> UniquePtr<SystemCKernel>;
        fn run_quantum(self: Pin<&mut SystemCKernel>, ns: f64) -> bool;
        fn create_transaction(
            addr: u64, 
            data: &[u8], 
            is_write: bool
        ) -> UniquePtr<TlmTransaction>;
        fn execute_transaction(
            self: Pin<&mut SystemCKernel>, 
            trans: &TlmTransaction
        ) -> Result<u64>;  // Returns delay in ns
    }
}

pub struct SystemCBridge {
    kernel: UniquePtr<SystemCKernel>,
    quantum_ns: f64,
}

impl SystemCBridge {
    pub fn new(quantum_ns: f64) -> Result<Self> {
        Ok(Self {
            kernel: ffi::create_systemc_kernel(),
            quantum_ns,
        })
    }
    
    pub fn memory_access(
        &mut self,
        addr: u64,
        data: &mut [u8],
        is_write: bool,
    ) -> Result<u64> {
        let trans = ffi::create_transaction(addr, data, is_write);
        let delay_ns = self.kernel.pin_mut().execute_transaction(&trans)?;
        
        // Convert ns to HELM cycles
        Ok(self.ns_to_cycles(delay_ns))
    }
    
    pub fn synchronize(&mut self) -> Result<()> {
        self.kernel.pin_mut().run_quantum(self.quantum_ns);
        Ok(())
    }
}
```

#### Option 2: Socket-Based Bridge (Flexibility)

For out-of-process SystemC (separate process, potentially different machine):

```rust
// helm-systemc/src/socket_bridge.rs
use tokio::net::UnixStream;

pub struct SystemCSocketBridge {
    stream: UnixStream,
    quantum_cycles: u64,
}

#[derive(Serialize, Deserialize)]
struct TransactionMessage {
    msg_type: MessageType,
    addr: u64,
    data: Vec<u8>,
    is_write: bool,
    timestamp: u64,
}

#[derive(Serialize, Deserialize)]
enum MessageType {
    MemoryRead,
    MemoryWrite,
    Synchronize,
    Response,
}

impl SystemCSocketBridge {
    pub async fn memory_access(
        &mut self,
        addr: u64,
        data: &mut [u8],
        is_write: bool,
    ) -> Result<u64> {
        // Send transaction
        let msg = TransactionMessage {
            msg_type: if is_write { MessageType::MemoryWrite } else { MessageType::MemoryRead },
            addr,
            data: data.to_vec(),
            is_write,
            timestamp: self.current_cycle,
        };
        
        let json = serde_json::to_string(&msg)?;
        self.stream.write_all(json.as_bytes()).await?;
        self.stream.write_all(b"\n").await?;
        
        // Receive response
        let mut buffer = String::new();
        self.stream.read_line(&mut buffer).await?;
        let response: TransactionMessage = serde_json::from_str(&buffer)?;
        
        // Update data for reads
        if !is_write {
            data.copy_from_slice(&response.data);
        }
        
        Ok(response.timestamp - self.current_cycle)  // Delay in cycles
    }
}
```

#### Option 3: Shared Memory Bridge (Balanced)

For low-latency inter-process communication:

```rust
// helm-systemc/src/shmem_bridge.rs
use shared_memory::ShmemConf;

pub struct SystemCShmemBridge {
    shmem: Shmem,
    transaction_ring: TransactionRingBuffer,
    response_ring: ResponseRingBuffer,
}

struct TransactionRingBuffer {
    // Lock-free ring buffer in shared memory
    // Producer: HELM, Consumer: SystemC
}

impl SystemCShmemBridge {
    pub fn memory_access(
        &mut self,
        addr: u64,
        data: &mut [u8],
        is_write: bool,
    ) -> Result<u64> {
        // Write to shared memory ring
        self.transaction_ring.push(Transaction {
            addr,
            data: data.to_vec(),
            is_write,
        })?;
        
        // Signal SystemC process (eventfd or futex)
        self.notify_systemc()?;
        
        // Wait for response
        let response = self.response_ring.pop_blocking()?;
        
        if !is_write {
            data.copy_from_slice(&response.data);
        }
        
        Ok(response.delay_cycles)
    }
}
```

---

## 4. TLM Bridge Implementation

### 4.1 C++ Bridge Module

The SystemC side implements a bridge module:

```cpp
// helm-systemc-bridge.hpp
#include <systemc>
#include <tlm>
#include <tlm_utils/simple_initiator_socket.h>
#include <tlm_utils/simple_target_socket.h>

using namespace sc_core;
using namespace tlm;

SC_MODULE(HelmTlmBridge) {
    // To HELM: receives transactions from SystemC, forwards to HELM
    tlm_utils::simple_target_socket<HelmTlmBridge> from_systemc_socket;
    
    // From HELM: receives HELM memory accesses, forwards to SystemC
    tlm_utils::simple_initiator_socket<HelmTlmBridge> to_systemc_socket;
    
    // Callback from SystemC → HELM
    virtual void b_transport(tlm_generic_payload& trans, sc_time& delay) {
        // Extract transaction details
        uint64_t addr = trans.get_address();
        tlm_command cmd = trans.get_command();
        unsigned char* data = trans.get_data_ptr();
        unsigned int len = trans.get_data_length();
        
        // Call into HELM via FFI
        helm_ffi::MemoryAccessResult result = 
            helm_ffi::helm_memory_access(addr, data, len, cmd == TLM_WRITE_COMMAND);
        
        // Update response
        if (cmd == TLM_READ_COMMAND) {
            memcpy(data, result.data, len);
        }
        
        delay += sc_time(result.delay_ns, SC_NS);
        trans.set_response_status(TLM_OK_RESPONSE);
    }
    
    // Forward HELM transaction to SystemC
    uint64_t helm_to_systemc_access(
        uint64_t addr, 
        unsigned char* data, 
        unsigned int len, 
        bool is_write
    ) {
        tlm_generic_payload trans;
        trans.set_address(addr);
        trans.set_command(is_write ? TLM_WRITE_COMMAND : TLM_READ_COMMAND);
        trans.set_data_ptr(data);
        trans.set_data_length(len);
        trans.set_streaming_width(len);
        trans.set_byte_enable_ptr(nullptr);
        trans.set_dmi_allowed(false);
        trans.set_response_status(TLM_INCOMPLETE_RESPONSE);
        
        sc_time delay = SC_ZERO_TIME;
        to_systemc_socket->b_transport(trans, delay);
        
        return delay.to_seconds() * 1e9;  // Convert to ns
    }
    
    SC_CTOR(HelmTlmBridge) {
        from_systemc_socket.register_b_transport(this, &HelmTlmBridge::b_transport);
    }
};
```

### 4.2 Rust FFI Layer

```rust
// helm-systemc/src/ffi.rs
use cxx::{UniquePtr, CxxVector};

#[cxx::bridge(namespace = "helm::systemc")]
mod ffi {
    struct MemoryAccessResult {
        data: Vec<u8>,
        delay_ns: f64,
    }
    
    unsafe extern "C++" {
        include!("helm-systemc/bridge.hpp");
        
        type HelmTlmBridge;
        
        fn create_bridge() -> UniquePtr<HelmTlmBridge>;
        fn run_until_sync(self: Pin<&mut HelmTlmBridge>, time_ns: f64);
        fn memory_access_from_helm(
            self: Pin<&mut HelmTlmBridge>,
            addr: u64,
            data: &mut [u8],
            is_write: bool,
        ) -> f64;  // Returns delay in ns
    }
    
    extern "Rust" {
        // Called by SystemC when it accesses HELM memory
        fn helm_memory_access(
            addr: u64,
            data: &mut [u8],
            is_write: bool,
        ) -> MemoryAccessResult;
    }
}

// Rust implementation of callback
fn helm_memory_access(
    addr: u64,
    data: &mut [u8],
    is_write: bool,
) -> MemoryAccessResult {
    // Access HELM's memory system
    let platform = GLOBAL_PLATFORM.lock().unwrap();
    
    if is_write {
        platform.memory_write(addr, data)?;
        MemoryAccessResult {
            data: Vec::new(),
            delay_ns: 0.0,  // No delay for writes in this mode
        }
    } else {
        let mut buffer = vec![0u8; data.len()];
        let delay_cycles = platform.memory_read(addr, &mut buffer)?;
        
        MemoryAccessResult {
            data: buffer,
            delay_ns: cycles_to_ns(delay_cycles),
        }
    }
}
```

### 4.3 Integration with HELM Components

```rust
// helm-engine/src/systemc_integration.rs

pub struct SystemCDevice {
    bridge: UniquePtr<HelmTlmBridge>,
    address_range: (u64, u64),
    interrupt_lines: Vec<u32>,
}

impl MemoryMappedDevice for SystemCDevice {
    fn read(&mut self, offset: u64, size: usize) -> Result<(u64, u64)> {
        let addr = self.address_range.0 + offset;
        let mut data = vec![0u8; size];
        
        // Call into SystemC via bridge
        let delay_ns = self.bridge
            .pin_mut()
            .memory_access_from_helm(addr, &mut data, false);
        
        // Convert bytes to u64
        let value = match size {
            1 => data[0] as u64,
            2 => u16::from_le_bytes([data[0], data[1]]) as u64,
            4 => u32::from_le_bytes([data[0], data[1], data[2], data[3]]) as u64,
            8 => u64::from_le_bytes(data.try_into().unwrap()),
            _ => return Err(anyhow!("Unsupported size")),
        };
        
        let delay_cycles = self.ns_to_cycles(delay_ns);
        Ok((value, delay_cycles))
    }
    
    fn write(&mut self, offset: u64, size: usize, value: u64) -> Result<u64> {
        let addr = self.address_range.0 + offset;
        let data = value.to_le_bytes()[..size].to_vec();
        
        let delay_ns = self.bridge
            .pin_mut()
            .memory_access_from_helm(addr, &mut data, true);
        
        Ok(self.ns_to_cycles(delay_ns))
    }
}
```

---

## 5. Time Synchronization

### 5.1 Quantum-Based Synchronization

```rust
pub struct SystemCTimeSynchronizer {
    helm_virtual_cycles: u64,
    systemc_time_ns: f64,
    quantum_cycles: u64,
    cpu_frequency: u64,  // Hz
}

impl SystemCTimeSynchronizer {
    pub fn run_quantum(&mut self, bridge: &mut HelmTlmBridge) -> Result<()> {
        // Calculate quantum duration
        let quantum_ns = self.cycles_to_ns(self.quantum_cycles);
        
        // Advance SystemC kernel
        bridge.pin_mut().run_until_sync(quantum_ns);
        
        // Update SystemC time
        self.systemc_time_ns += quantum_ns;
        
        // Update HELM time (may have executed fewer cycles due to stalls)
        self.helm_virtual_cycles += self.quantum_cycles;
        
        Ok(())
    }
    
    fn cycles_to_ns(&self, cycles: u64) -> f64 {
        (cycles as f64 / self.cpu_frequency as f64) * 1e9
    }
    
    fn ns_to_cycles(&self, ns: f64) -> u64 {
        ((ns / 1e9) * self.cpu_frequency as f64) as u64
    }
}
```

### 5.2 Synchronization Strategies

**Strategy 1: Fixed Quantum (LT Mode)**
- HELM runs for N cycles
- Synchronize with SystemC
- SystemC runs for equivalent time
- Repeat

```rust
loop {
    // HELM executes
    core.run_cycles(quantum_cycles)?;
    
    // Synchronize
    sync.run_quantum(&mut systemc_bridge)?;
    
    // Process SystemC-generated interrupts
    handle_interrupts()?;
}
```

**Strategy 2: Event-Driven (AT Mode)**
- Maintain unified event queue
- HELM and SystemC events interleaved
- Synchronize on every transaction

```rust
loop {
    match next_event() {
        Event::HelmCore => {
            core.step()?;
            if core.memory_access_needed() {
                let delay = systemc_bridge.transaction(...)?;
                schedule_event(current_time + delay);
            }
        }
        Event::SystemC => {
            systemc_bridge.run_until_next_event()?;
        }
    }
}
```

**Strategy 3: Hybrid (Configurable)**
- Use quantum sync for Level 0-1
- Use event-driven for Level 2-3
- User-selectable based on accuracy needs

### 5.3 Multi-Clock Domains

SystemC models often have multiple clocks:

```cpp
// SystemC side with multiple clocks
sc_clock cpu_clk("cpu_clk", 0.5, SC_NS);     // 2 GHz
sc_clock bus_clk("bus_clk", 10, SC_NS);      // 100 MHz
sc_clock periph_clk("periph_clk", 20, SC_NS); // 50 MHz

// Bridge must handle clock domain crossing
```

HELM handles this via time conversion:

```rust
pub struct ClockDomain {
    frequency_hz: u64,
    name: String,
}

pub fn convert_time(
    cycles: u64,
    from_domain: &ClockDomain,
    to_domain: &ClockDomain,
) -> u64 {
    let ns = (cycles as f64 / from_domain.frequency_hz as f64) * 1e9;
    ((ns / 1e9) * to_domain.frequency_hz as f64) as u64
}
```

---

## 6. Use Cases

### 6.1 RTL Co-Simulation

**Scenario**: Verify a custom DMA engine RTL against HELM's CPU and software stack

```python
# platforms/rpi3-with-custom-dma.py
from helm import Platform, SystemCDevice

platform = Platform.from_file("rpi3.py")

# Add SystemC/Verilator-wrapped RTL
platform.add_device(
    SystemCDevice(
        name="custom_dma",
        systemc_module="CustomDmaModule",  # Verilator-wrapped RTL
        base_address=0x40000000,
        size=0x1000,
        interrupts=[10],
        quantum_ns=1000,  # 1 μs quantum
    )
)

# Run with driver software
platform.load_kernel("vmlinux")
platform.run()
```

**SystemC/Verilator side:**
```cpp
// Custom DMA RTL wrapper
#include "VCustomDma.h"  // Verilator-generated

SC_MODULE(CustomDmaModule) {
    tlm_target_socket<> socket;
    VCustomDma* dma_rtl;
    sc_clock clk;
    
    void clock_thread() {
        while (true) {
            dma_rtl->clk = 1;
            dma_rtl->eval();
            wait(0.5, SC_NS);
            
            dma_rtl->clk = 0;
            dma_rtl->eval();
            wait(0.5, SC_NS);
        }
    }
    
    virtual void b_transport(tlm_generic_payload& trans, sc_time& delay) {
        // Map TLM to RTL register interface
        // ...
    }
    
    SC_CTOR(CustomDmaModule) : clk("clk", 1, SC_NS) {
        dma_rtl = new VCustomDma("dma");
        SC_THREAD(clock_thread);
    }
};
```

### 6.2 IP Verification

**Scenario**: Integrate vendor TLM IP models (e.g., ARM AMBA components)

```python
# Use ARM TLM models for interconnect
from helm import Platform, SystemCDevice

platform = Platform()

# ARM CoreLink CCI-400 (cache coherent interconnect)
platform.add_systemc_device(
    type="arm.corelink-cci400",
    systemc_library="libcci400_tlm.so",
    config={
        "num_ace_ports": 2,
        "num_ace_lite_ports": 3,
    }
)

# Connect HELM cores to SystemC interconnect
platform.cores[0].connect_to("systemc://cci400/ace_port0")
platform.cores[1].connect_to("systemc://cci400/ace_port1")
```

### 6.3 Mixed-Fidelity Simulation

**Scenario**: Most of SoC at HELM Level 1, critical IP at RTL accuracy

```python
platform = Platform()

# Fast HELM cores
for i in range(4):
    platform.add_core(type="cortex-a53", timing_level=1)

# SystemC-wrapped RTL for critical block
platform.add_device(
    SystemCDevice(
        name="crypto_engine",
        systemc_module="CryptoEngineRTL",  # Cycle-accurate RTL
        timing_mode="cycle-accurate",
    )
)

# Fast TLM models for peripherals
platform.add_device(
    SystemCDevice(
        name="ethernet",
        systemc_module="EthernetTLM",  # Loosely-timed TLM
        timing_mode="loosely-timed",
    )
)
```

### 6.4 NoC Modeling

**Scenario**: Network-on-Chip in SystemC, endpoints in HELM

```cpp
// SystemC NoC model (e.g., Noxim-based)
SC_MODULE(NocRouter) {
    tlm_target_socket<> input_ports[4];
    tlm_initiator_socket<> output_ports[4];
    
    // Routing logic, buffering, arbitration in SystemC
    // HELM cores connect as initiators/targets
};
```

```rust
// HELM connects cores through NoC
platform.add_interconnect(
    SystemCNoC {
        topology: "mesh",
        size: (4, 4),
        systemc_module: "NoximNoC",
    }
);

// Cores communicate via NoC instead of shared bus
```

---

## 7. Performance Considerations

### 7.1 Performance Matrix

| Integration Mode | Speed (MIPS) | Overhead | Best For |
|------------------|--------------|----------|----------|
| **In-process (cxx)** | 10-500 | 1.5-3× | Production, low-latency |
| **Shared memory** | 5-200 | 3-10× | Moderate coupling |
| **Unix socket** | 1-50 | 10-50× | Flexibility, debugging |
| **TCP socket** | 0.1-10 | 50-200× | Distributed simulation |

### 7.2 Quantum Size Impact

```rust
// Quantum tradeoff analysis
struct QuantumAnalysis {
    quantum_cycles: u64,
    synchronizations_per_second: f64,
    overhead_percentage: f64,
    timing_error_ns: f64,
}

fn analyze_quantum(quantum_cycles: u64, frequency: u64) -> QuantumAnalysis {
    let quantum_ns = (quantum_cycles as f64 / frequency as f64) * 1e9;
    let syncs_per_sec = 1e9 / quantum_ns;
    
    // Each sync costs ~1-10 μs depending on mode
    let overhead_ns = syncs_per_sec * 5.0;  // Assume 5 μs per sync
    let total_sim_time = 1e9;  // 1 second of virtual time
    let overhead_pct = (overhead_ns / total_sim_time) * 100.0;
    
    QuantumAnalysis {
        quantum_cycles,
        synchronizations_per_second: syncs_per_sec,
        overhead_percentage: overhead_pct,
        timing_error_ns: quantum_ns,  // Max timing skew
    }
}

// Example results:
// Quantum 1000 cycles (0.5 μs @ 2GHz):    2M syncs/sec, ~100% overhead, ±0.5μs error
// Quantum 10,000 cycles (5 μs):          200K syncs/sec, ~10% overhead, ±5μs error
// Quantum 100,000 cycles (50 μs):        20K syncs/sec, ~1% overhead, ±50μs error
```

**Recommendation:**
- Development: 10-100 μs quantum (1-5% overhead)
- Production: 1-10 μs quantum for tighter coupling
- Debugging: 100-1000 ns quantum for precise timing

### 7.3 Optimization Strategies

**1. Transaction Batching**
```rust
pub struct BatchedBridge {
    pending_transactions: Vec<Transaction>,
    batch_size: usize,
}

impl BatchedBridge {
    pub fn memory_access(&mut self, addr: u64, data: &[u8], is_write: bool) -> Result<u64> {
        self.pending_transactions.push(Transaction { addr, data, is_write });
        
        if self.pending_transactions.len() >= self.batch_size {
            self.flush_batch()?;
        }
        
        Ok(1)  // Approximate delay
    }
    
    fn flush_batch(&mut self) -> Result<()> {
        // Send all transactions at once
        // Process all responses
        // More efficient than one-by-one
        Ok(())
    }
}
```

**2. Direct Memory Interface (DMI)**
```cpp
// SystemC DMI for high-performance memory access
virtual bool get_direct_mem_ptr(tlm_generic_payload& trans, tlm_dmi& dmi_data) {
    // Return pointer to SystemC memory region
    // HELM can access directly without transaction overhead
    dmi_data.set_dmi_ptr(memory_ptr);
    dmi_data.set_start_address(0x0);
    dmi_data.set_end_address(0xFFFFFFFF);
    dmi_data.allow_read_write();
    return true;
}
```

**3. Asynchronous Transactions**
```rust
pub struct AsyncSystemCBridge {
    transaction_queue: mpsc::Sender<Transaction>,
    response_queue: mpsc::Receiver<Response>,
}

impl AsyncSystemCBridge {
    pub async fn memory_access(&mut self, trans: Transaction) -> Result<Response> {
        // Send transaction without blocking
        self.transaction_queue.send(trans).await?;
        
        // Continue HELM simulation
        // ...
        
        // Later, collect response
        let response = self.response_queue.recv().await?;
        Ok(response)
    }
}
```

---

## 8. Implementation Roadmap

### Phase 1: Basic FFI Bridge (4-6 weeks)

**Objectives:**
- Set up cxx/bindgen infrastructure
- Implement in-process bridge
- Basic TLM-2.0 LT mode support
- Single SystemC module integration

**Deliverables:**
- `helm-systemc` crate with FFI bindings
- C++ bridge module using cxx
- Example: Simple memory-mapped device in SystemC
- Unit tests for transaction conversion

### Phase 2: Multi-Module Support (3-4 weeks)

**Objectives:**
- Support multiple SystemC modules
- Interrupt line integration
- Clock domain conversion
- Python API for SystemC devices

**Deliverables:**
- Multi-socket support in bridge
- IRQ → SystemC signal mapping
- Python `SystemCDevice` class
- Examples: UART, timer in SystemC

### Phase 3: Advanced Synchronization (4-6 weeks)

**Objectives:**
- Implement AT mode support
- Fine-grained synchronization
- Performance optimization (DMI, batching)
- Timing level integration

**Deliverables:**
- Non-blocking transport support
- Configurable quantum sizes
- DMI fast-path
- Performance benchmarks

### Phase 4: RTL Co-Simulation (6-8 weeks)

**Objectives:**
- Verilator integration
- SystemC wrapper generation
- Full-system RTL co-sim examples
- Debugging support

**Deliverables:**
- Verilator-to-HELM bridge
- Auto-generate SystemC wrappers from RTL
- Example: DMA engine RTL + HELM CPU
- Waveform dumping (VCD/FST)

### Phase 5: Production Features (4-6 weeks)

**Objectives:**
- Shared memory bridge
- Socket-based bridge
- Checkpoint integration
- Commercial IP support

**Deliverables:**
- Zero-copy shared memory transport
- Remote SystemC support
- SystemC state in checkpoints
- Documentation for IP vendors

**Total Timeline:** ~6 months

---

## 9. Configuration Examples

### 9.1 TOML Configuration

```toml
# systemc-config.toml

[systemc]
enabled = true
library_path = "/usr/local/lib/libsystemc.so"
quantum_ns = 10000  # 10 μs

[[systemc.device]]
name = "custom_peripheral"
module = "CustomPeripheral"
library = "libcustom_periph.so"
base_address = 0x40000000
size = 0x1000
interrupts = [42]

[[systemc.device]]
name = "dma_engine"
module = "DmaEngineRTL"  # Verilator-wrapped
library = "libdma_rtl.so"
base_address = 0x50000000
timing_mode = "cycle-accurate"
```

### 9.2 Python API

```python
from helm import Platform, SystemCDevice

platform = Platform()

# Add SystemC device
dma = SystemCDevice(
    name="dma",
    module="DmaEngine",
    library="libdma_engine_tlm.so",
    properties={
        'base_address': 0x40000000,
        'num_channels': 8,
        'buffer_size': 4096,
    },
    interrupts=[32, 33, 34],
)

platform.add_device(dma)

# Configure synchronization
platform.systemc_config.quantum_ns = 5000  # 5 μs
platform.systemc_config.mode = 'loosely-timed'

# Run
platform.run()
```

### 9.3 Rust API

```rust
use helm_systemc::{SystemCBridge, SystemCDevice, BridgeConfig};

let mut platform = Platform::new();

// Configure SystemC bridge
let bridge_config = BridgeConfig {
    mode: BridgeMode::InProcess,
    quantum_cycles: 10_000,
    timing_mode: TlmTimingMode::LooselyTimed,
};

let mut bridge = SystemCBridge::new(bridge_config)?;

// Load SystemC module
let dma = SystemCDevice::new(
    "libdma_engine.so",
    "DmaEngine",
    0x40000000,
    0x1000,
)?;

bridge.register_device(dma)?;
platform.add_systemc_bridge(bridge);

// Run simulation
platform.run()?;
```

---

## 10. Advanced Features

### 10.1 Waveform Dumping

```cpp
// SystemC module with VCD tracing
SC_MODULE(TracedDevice) {
    sc_in<bool> clk;
    sc_signal<int> internal_state;
    
    SC_CTOR(TracedDevice) {
        // Enable VCD dump
        sc_trace_file* tf = sc_create_vcd_trace_file("waves");
        sc_trace(tf, clk, "clk");
        sc_trace(tf, internal_state, "internal_state");
    }
};
```

```rust
// HELM triggers trace dumps
platform.systemc_config.enable_tracing = true;
platform.systemc_config.trace_file = "simulation.vcd";
```

### 10.2 Checkpoint Integration

```rust
pub struct SystemCCheckpoint {
    systemc_time_ns: f64,
    module_states: HashMap<String, Vec<u8>>,
}

impl SystemCBridge {
    pub fn checkpoint(&self) -> Result<SystemCCheckpoint> {
        // Serialize SystemC module state
        let states = self.kernel.pin_mut().serialize_all_modules()?;
        
        Ok(SystemCCheckpoint {
            systemc_time_ns: self.get_current_time_ns(),
            module_states: states,
        })
    }
    
    pub fn restore(&mut self, checkpoint: SystemCCheckpoint) -> Result<()> {
        // Restore SystemC module state
        self.kernel.pin_mut().deserialize_all_modules(&checkpoint.module_states)?;
        self.set_time(checkpoint.systemc_time_ns)?;
        Ok(())
    }
}
```

### 10.3 Debug Interface

```rust
// Expose SystemC signals to HMP
pub fn query_systemc_signals(module: &str) -> Vec<SignalInfo> {
    bridge.get_signals(module)
}

// HMP command
{
    "execute": "systemc-query-signals",
    "arguments": {
        "module": "dma_engine"
    }
}

// Response
{
    "return": {
        "signals": [
            {"name": "clk", "type": "bool", "value": true},
            {"name": "state", "type": "int", "value": 42},
            {"name": "buffer_addr", "type": "uint64", "value": "0x80000000"}
        ]
    }
}
```

---

## 11. Comparison with Other Simulators

### 11.1 Feature Comparison

| Feature | QEMU + SystemC | gem5 + SystemC | Simics + SystemC | HELM + SystemC |
|---------|----------------|----------------|------------------|----------------|
| **Native support** | No (external) | Yes (upstream) | Yes (commercial) | Planned |
| **Integration mode** | Socket/QBOX | In-process lib | In-process lib | Multi-mode |
| **TLM version** | 2.0 | 2.0 | 2.0 | 2.0 |
| **LT mode** | Yes | Yes | Yes | Yes |
| **AT mode** | Limited | Yes | Yes | Yes |
| **Time sync** | Quantum | Unified queue | Cell-based | Configurable |
| **DMI support** | No | Yes | Yes | Planned |
| **Checkpointing** | No | Limited | Yes | Planned |
| **Performance** | Poor-Good | Good | Good-Excellent | Good (goal) |

### 11.2 Design Choice for HELM

Based on the research, HELM should adopt a **gem5-like approach** with **Simics-like flexibility**:

1. **In-process bridge** as primary mode (like gem5)
2. **Socket bridge** as fallback (like QEMU Xilinx)
3. **Quantum-based sync** aligned with HELM's timing levels
4. **Multi-mode support** (LT for Level 0-1, AT for Level 2-3)
5. **cxx for FFI** (type-safe, modern Rust↔C++ interop)

---

## 12. Example: Complete Integration

### 12.1 SystemC Module

```cpp
// custom_uart.cpp
#include <systemc>
#include <tlm>

using namespace sc_core;
using namespace tlm;

SC_MODULE(CustomUart) {
    tlm_target_socket<> socket;
    sc_out<bool> interrupt;
    
    sc_event tx_complete_event;
    uint8_t tx_fifo[256];
    int tx_head, tx_tail;
    
    virtual void b_transport(tlm_generic_payload& trans, sc_time& delay) {
        uint64_t addr = trans.get_address();
        unsigned char* data = trans.get_data_ptr();
        
        if (trans.get_command() == TLM_WRITE_COMMAND) {
            if (addr == 0x00) {  // TX_DATA register
                tx_fifo[tx_tail++] = *data;
                
                // Schedule transmission (115200 baud = ~87 μs/byte)
                tx_complete_event.notify(87, SC_US);
                
                // Annotate latency
                delay += sc_time(10, SC_NS);
            }
        } else {
            if (addr == 0x04) {  // STATUS register
                *data = (tx_tail == tx_head) ? 0x01 : 0x00;
                delay += sc_time(5, SC_NS);
            }
        }
        
        trans.set_response_status(TLM_OK_RESPONSE);
    }
    
    void tx_process() {
        while (true) {
            wait(tx_complete_event);
            
            // Transmit byte (simplified)
            if (tx_tail > tx_head) {
                transmit_byte(tx_fifo[tx_head++]);
                interrupt.write(true);
                wait(1, SC_US);
                interrupt.write(false);
            }
        }
    }
    
    SC_CTOR(CustomUart) : tx_head(0), tx_tail(0) {
        socket.register_b_transport(this, &CustomUart::b_transport);
        SC_THREAD(tx_process);
    }
};
```

### 12.2 HELM Integration

```rust
// helm platform configuration
use helm::Platform;
use helm_systemc::SystemCDevice;

let mut platform = Platform::new();

// Add SystemC UART
let uart = SystemCDevice::builder()
    .library("libcustom_uart.so")
    .module("CustomUart")
    .base_address(0x10000000)
    .size(0x100)
    .interrupt(57)
    .quantum_us(10.0)
    .build()?;

platform.add_device(uart);

// HELM CPU accesses UART registers
// → Triggers SystemC b_transport
// → UART processes transaction
// → Returns delay
// → HELM advances virtual time
```

### 12.3 Running the Simulation

```bash
# Build SystemC module
g++ -shared -fPIC custom_uart.cpp -o libcustom_uart.so -lsystemc

# Run HELM with SystemC
helm run --platform rpi3-systemc.py --systemc-quantum 10000
```

---

## Conclusion

SystemC integration enables HELM to:

1. **Co-simulate with RTL**: Verilator-wrapped Verilog/VHDL for critical blocks
2. **Use vendor IP models**: TLM-2.0 models from ARM, Synopsys, etc.
3. **Mixed-fidelity modeling**: Fast HELM + detailed SystemC where needed
4. **Research flexibility**: NoC, interconnect, custom accelerators

The proposed architecture balances:
- **Performance**: In-process bridge with quantum-based sync
- **Flexibility**: Multi-mode support (in-process, socket, shared memory)
- **Compatibility**: Standard TLM-2.0 for ecosystem access
- **Integration**: Aligns with HELM's 4-level timing model

Implementation priority: After core object model and timing infrastructure are in place (Phases 3-4 of main roadmap).
