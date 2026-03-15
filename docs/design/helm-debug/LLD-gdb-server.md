# helm-debug — LLD: GDB Server

> **Module:** `helm-debug::gdb`
> **Protocol:** GDB Remote Serial Protocol (RSP) over TCP or Unix domain socket
> **Thread model:** Dedicated server thread; simulation thread quiesces on request

---

## Table of Contents

1. [RSP Packet Framing](#1-rsp-packet-framing)
2. [Public API](#2-public-api)
3. [GdbTarget Trait](#3-gdbtarget-trait)
4. [Type Definitions](#4-type-definitions)
5. [RSP Packet Handlers](#5-rsp-packet-handlers)
6. [Thread Model](#6-thread-model)
7. [LLDB Compatibility — target.xml](#7-lldb-compatibility--targetxml)
8. [Error Handling](#8-error-handling)

---

## 1. RSP Packet Framing

All RSP messages conform to the standard GDB packet format:

```
$packet-data#checksum
```

- `$` — start character
- `packet-data` — ASCII payload (may contain run-length encoded repetitions via `*`)
- `#` — end-of-data marker
- `checksum` — two lowercase hex digits; sum of all bytes in `packet-data` modulo 256

Acknowledgment protocol:
- `+` — packet received and checksum valid
- `-` — checksum mismatch; sender must retransmit

### Checksum Computation

```rust
fn rsp_checksum(data: &[u8]) -> u8 {
    data.iter().fold(0u8, |acc, &b| acc.wrapping_add(b))
}
```

### Packet Serialization

```rust
fn rsp_encode(payload: &str) -> String {
    let cksum = rsp_checksum(payload.as_bytes());
    format!("${}#{:02x}", payload, cksum)
}
```

### Packet Parsing

```rust
/// Returns the inner payload string if framing and checksum are valid.
fn rsp_decode(raw: &[u8]) -> Result<&str, RspError> {
    // Expects raw to be exactly one packet: $data#xx
    let start = raw.iter().position(|&b| b == b'$').ok_or(RspError::NoStart)?;
    let end   = raw.iter().position(|&b| b == b'#').ok_or(RspError::NoEnd)?;
    let data  = &raw[start + 1..end];
    let cksum_bytes = raw.get(end + 1..end + 3).ok_or(RspError::TruncatedChecksum)?;
    let expected = u8::from_str_radix(std::str::from_utf8(cksum_bytes)?, 16)?;
    let actual   = rsp_checksum(data);
    if actual != expected { return Err(RspError::ChecksumMismatch { actual, expected }); }
    Ok(std::str::from_utf8(data)?)
}
```

---

## 2. Public API

### `GdbServer`

```rust
pub struct GdbServer {
    listener: GdbListener,           // TCP or Unix socket
    breakpoints: HashMap<u64, BreakpointKind>,
}

pub enum GdbListener {
    Tcp(TcpListener),
    Unix(UnixListener),
}

impl GdbServer {
    /// Bind a TCP port. Port 0 binds to an OS-assigned ephemeral port.
    pub fn bind(port: u16) -> io::Result<Self>;

    /// Bind a Unix domain socket path.
    pub fn bind_unix(path: &Path) -> io::Result<Self>;

    /// Block until one GDB client connects, then serve the RSP session
    /// until the client detaches or sends a `k` (kill) packet.
    ///
    /// `target` must remain valid for the duration of the call.
    /// The simulation is paused before any register/memory access and
    /// resumed when the client sends `c` or `vCont;c`.
    pub fn accept_and_serve(&mut self, target: &mut dyn GdbTarget) -> io::Result<()>;

    /// Return the bound local address (useful for ephemeral port discovery).
    pub fn local_addr(&self) -> io::Result<std::net::SocketAddr>;
}
```

### Spawning as a background thread

```rust
/// Spawn the GDB server in a background thread.
/// Returns a `JoinHandle` and a channel sender that can be used
/// to shut down the server from the main thread.
pub fn spawn_gdb_server(
    port: u16,
    target: Arc<Mutex<dyn GdbTarget + Send>>,
) -> (JoinHandle<io::Result<()>>, Sender<()>);
```

---

## 3. GdbTarget Trait

`GdbTarget` is the interface between the RSP server and the simulation engine. It is implemented by `HelmEngine<T>`. All methods are called from the GDB server thread **only after** the simulation has been paused via `HelmEventBus`.

```rust
pub trait GdbTarget: Send {
    // ── Register access ───────────────────────────────────────────────────────

    /// Read one register by GDB register number.
    fn read_register(&self, reg: GdbReg) -> u64;

    /// Write one register by GDB register number.
    fn write_register(&mut self, reg: GdbReg, val: u64);

    /// Read all registers in GDB order (for `g` packet).
    /// Returns a flat byte array of little-endian 64-bit values.
    fn read_all_registers(&self) -> Vec<u8>;

    /// Write all registers from a flat byte array (for `G` packet).
    fn write_all_registers(&mut self, data: &[u8]);

    // ── Memory access ─────────────────────────────────────────────────────────

    /// Read `len` bytes from the simulated address space starting at `addr`.
    /// Uses functional access mode (no timing side effects).
    fn read_memory(&self, addr: u64, len: usize) -> Result<Vec<u8>, GdbError>;

    /// Write `data` bytes into the simulated address space starting at `addr`.
    fn write_memory(&mut self, addr: u64, data: &[u8]) -> Result<(), GdbError>;

    // ── Execution control ─────────────────────────────────────────────────────

    /// Execute exactly one instruction on the given hart and return the stop reason.
    /// `hart_id` of `None` means "the current hart".
    fn step(&mut self, hart_id: Option<usize>) -> StopReason;

    /// Resume execution until a breakpoint, watchpoint, or external stop.
    fn r#continue(&mut self, hart_id: Option<usize>) -> StopReason;

    // ── Breakpoints ───────────────────────────────────────────────────────────

    /// Insert a software breakpoint at `addr`.
    fn insert_breakpoint(&mut self, addr: u64, kind: BreakpointKind) -> Result<(), GdbError>;

    /// Remove a previously inserted breakpoint.
    fn remove_breakpoint(&mut self, addr: u64, kind: BreakpointKind) -> Result<(), GdbError>;

    // ── Session control ───────────────────────────────────────────────────────

    /// Return the stop reason at the current simulation state.
    /// Called on initial `?` packet from GDB.
    fn stop_reason(&self) -> StopReason;

    /// Called when GDB sends `D` (detach). Simulation should resume.
    fn detach(&mut self);

    /// Called when GDB sends `k` (kill). Simulation should terminate.
    fn kill(&mut self);

    // ── Target description ────────────────────────────────────────────────────

    /// Return the LLDB/GDB target XML for `qXfer:features:read:target.xml`.
    /// Return `None` to indicate the feature is unsupported.
    fn target_xml(&self) -> Option<String>;

    /// Return the number of harts (threads in GDB's view).
    fn hart_count(&self) -> usize;

    /// Return the ISA of the given hart.
    fn hart_isa(&self, hart_id: usize) -> crate::Isa;
}
```

---

## 4. Type Definitions

### `GdbReg`

```rust
/// ISA-neutral GDB register identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GdbReg {
    /// Program counter.
    Pc,
    /// Integer register by index (x0–x31 for RISC-V; x0–x30 for AArch64).
    Int(u8),
    /// Floating-point register by index (f0–f31).
    Float(u8),
    /// RISC-V CSR by CSR number (e.g. `GdbReg::Csr(0x300)` for `mstatus`).
    Csr(u16),
    /// AArch64 system register (encoded as the GDB regnum past the GP set).
    SysReg(u16),
}
```

### `StopReason`

```rust
/// Why the simulation stopped; sent in RSP `?` and `W`/`S`/`T` replies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopReason {
    /// Stopped by POSIX signal. SIGTRAP (5) for breakpoint/step.
    Signal(u8),
    /// Software breakpoint (`z0`/`Z0`) was hit.
    Breakpoint,
    /// Hardware watchpoint was triggered.
    Watchpoint { addr: u64 },
    /// The simulated process exited with the given status code.
    Exited(u8),
    /// The simulation was explicitly paused by the GDB server (for `?` on connect).
    Halted,
}

impl StopReason {
    /// Format as an RSP stop-reply packet payload (e.g. `"S05"`, `"T05"`, `"W00"`).
    pub fn to_rsp_reply(self) -> String;
}
```

### `BreakpointKind`

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BreakpointKind {
    /// Replace the target instruction with a breakpoint trap opcode.
    Software,
    /// Track the PC in a set; no instruction patching.
    Hardware,
}
```

### `GdbError`

```rust
#[derive(Debug, thiserror::Error)]
pub enum GdbError {
    #[error("address {addr:#x} is not mapped")]
    UnmappedAddress { addr: u64 },
    #[error("breakpoint table full")]
    BreakpointTableFull,
    #[error("no breakpoint at {addr:#x}")]
    NoBreakpointAt { addr: u64 },
    #[error("register index {0} out of range")]
    InvalidRegister(u8),
}
```

---

## 5. RSP Packet Handlers

Each handler is a private function called by the main `accept_and_serve` dispatch loop.

### Minimum RSP Packet Set (Phase 1)

| Packet | Direction | Handler |
|--------|-----------|---------|
| `?` | GDB → sim | `handle_halt_reason` |
| `g` | GDB → sim | `handle_read_all_regs` |
| `G XX...` | GDB → sim | `handle_write_all_regs` |
| `m addr,len` | GDB → sim | `handle_read_mem` |
| `M addr,len:data` | GDB → sim | `handle_write_mem` |
| `c [addr]` | GDB → sim | `handle_continue` |
| `s [addr]` | GDB → sim | `handle_step` |
| `z0,addr,kind` | GDB → sim | `handle_remove_breakpoint` |
| `Z0,addr,kind` | GDB → sim | `handle_insert_breakpoint` |
| `k` | GDB → sim | `handle_kill` |
| `D` | GDB → sim | `handle_detach` |
| `vCont?` | GDB → sim | `handle_vcont_query` |
| `vCont;action[:tid]...` | GDB → sim | `handle_vcont` |
| `qXfer:features:read:target.xml:0,FFFF` | GDB → sim | `handle_qxfer_features` |
| `qSupported` | GDB → sim | `handle_qsupported` |
| `qC` | GDB → sim | `handle_current_thread` |

### Handler Signatures (internal)

```rust
fn handle_halt_reason(target: &dyn GdbTarget) -> String;
fn handle_read_all_regs(target: &dyn GdbTarget) -> String;
fn handle_write_all_regs(target: &mut dyn GdbTarget, hex: &str) -> String;
fn handle_read_mem(target: &dyn GdbTarget, addr: u64, len: usize) -> String;
fn handle_write_mem(target: &mut dyn GdbTarget, addr: u64, hex: &str) -> String;
fn handle_continue(target: &mut dyn GdbTarget, addr: Option<u64>) -> String;
fn handle_step(target: &mut dyn GdbTarget, addr: Option<u64>) -> String;
fn handle_insert_breakpoint(target: &mut dyn GdbTarget, addr: u64, kind: BreakpointKind) -> String;
fn handle_remove_breakpoint(target: &mut dyn GdbTarget, addr: u64, kind: BreakpointKind) -> String;
fn handle_kill(target: &mut dyn GdbTarget) -> String;
fn handle_detach(target: &mut dyn GdbTarget) -> String;
fn handle_vcont(target: &mut dyn GdbTarget, actions: &str) -> String;
fn handle_qxfer_features(target: &dyn GdbTarget, offset: usize, length: usize) -> String;
fn handle_qsupported(features: &[&str]) -> String;
```

### Dispatch Loop Sketch

```rust
fn serve_session(
    stream: TcpStream,
    target: &mut dyn GdbTarget,
) -> io::Result<()> {
    let mut buf = Vec::with_capacity(4096);
    loop {
        let packet = read_packet(&stream, &mut buf)?;
        let reply = match packet.as_str() {
            "?" => handle_halt_reason(target),
            p if p.starts_with('g') => handle_read_all_regs(target),
            p if p.starts_with('G') => handle_write_all_regs(target, &p[1..]),
            p if p.starts_with('m') => {
                let (addr, len) = parse_addr_len(&p[1..])?;
                handle_read_mem(target, addr, len)
            }
            p if p.starts_with('M') => {
                let (addr, data) = parse_mem_write(&p[1..])?;
                handle_write_mem(target, addr, data)
            }
            "c" | p if p.starts_with('c') => handle_continue(target, None),
            "s" | p if p.starts_with('s') => handle_step(target, None),
            p if p.starts_with('Z') => { /* ... */ String::from("OK") }
            p if p.starts_with('z') => { /* ... */ String::from("OK") }
            "k" => { handle_kill(target); break; }
            "D" => { handle_detach(target); break; }
            p if p.starts_with("vCont") => handle_vcont(target, p),
            p if p.starts_with("qXfer:features") => handle_qxfer_features(target, 0, 0xFFFF),
            _ => String::new(), // empty reply = "unsupported"
        };
        send_packet(&stream, &reply)?;
    }
    Ok(())
}
```

---

## 6. Thread Model

```
Main Thread (Simulation)                 GDB Server Thread
─────────────────────────                ────────────────────────────
HelmEngine::run()                        GdbServer::accept_and_serve()
  │                                        │
  │◄── HelmEventBus::subscribe ────────────┤  (GdbServer subscribes at elaborate())
  │                                        │
  │                                        │ TCP accept() → GDB client connects
  │                                        │
  │◄─── channel: SimCmd::Pause ───────────┤  GDB sends `?` or `s` or `c`
  │                                        │
  │ pauses, quiesces, sends                │
  │ SimEvent::Paused ──────────────────────►│
  │                                        │
  │                                        │ reads/writes registers/memory via GdbTarget
  │                                        │
  │◄─── channel: SimCmd::Resume ──────────┤  GDB sends `c`
  │                                        │
  │ resumes hot loop                        │ waits for next stop event
```

**Channel types:**

```rust
pub enum SimCmd {
    Pause,
    Resume,
    Kill,
}

pub enum SimEvent {
    Paused(StopReason),
    Running,
    Exited(u8),
}
```

The `GdbServer` thread holds a `Sender<SimCmd>` and a `Receiver<SimEvent>`. The simulation thread holds the complementary ends. When the simulation receives `SimCmd::Pause`, it completes the current instruction, posts `SimEvent::Paused`, and blocks on `Receiver<SimCmd>` until `Resume` or `Kill` arrives.

---

## 7. LLDB Compatibility — target.xml

LLDB (and newer GDB) request `target.xml` via the `qXfer:features:read:target.xml` packet. The server generates this XML from the `GdbTarget::target_xml()` method.

### RISC-V RV64GC target.xml

```xml
<?xml version="1.0"?>
<!DOCTYPE target SYSTEM "gdb-target.dtd">
<target version="1.0">
  <architecture>riscv:rv64</architecture>
  <feature name="org.gnu.gdb.riscv.cpu">
    <!-- x0–x31: GDB regnum 0–31 -->
    <reg name="zero" bitsize="64" regnum="0"  type="int"   group="general"/>
    <reg name="ra"   bitsize="64" regnum="1"  type="code_ptr" group="general"/>
    <reg name="sp"   bitsize="64" regnum="2"  type="data_ptr" group="general"/>
    <!-- ... x3–x31 ... -->
    <reg name="pc"   bitsize="64" regnum="32" type="code_ptr" group="general"/>
  </feature>
  <feature name="org.gnu.gdb.riscv.fpu">
    <!-- f0–f31: GDB regnum 33–64 -->
    <reg name="ft0"  bitsize="64" regnum="33" type="ieee_double" group="float"/>
    <!-- ... f1–f31 ... -->
  </feature>
  <feature name="org.gnu.gdb.riscv.csr">
    <!-- Selected CSRs: GDB regnum 65+ -->
    <reg name="mstatus"  bitsize="64" regnum="65"  type="int" group="csr"/>
    <reg name="misa"     bitsize="64" regnum="66"  type="int" group="csr"/>
    <reg name="mtvec"    bitsize="64" regnum="67"  type="int" group="csr"/>
    <reg name="mepc"     bitsize="64" regnum="68"  type="int" group="csr"/>
    <reg name="mcause"   bitsize="64" regnum="69"  type="int" group="csr"/>
    <reg name="mtval"    bitsize="64" regnum="70"  type="int" group="csr"/>
    <reg name="mip"      bitsize="64" regnum="71"  type="int" group="csr"/>
    <reg name="mie"      bitsize="64" regnum="72"  type="int" group="csr"/>
    <reg name="satp"     bitsize="64" regnum="73"  type="int" group="csr"/>
    <reg name="sstatus"  bitsize="64" regnum="74"  type="int" group="csr"/>
    <reg name="sepc"     bitsize="64" regnum="75"  type="int" group="csr"/>
    <reg name="scause"   bitsize="64" regnum="76"  type="int" group="csr"/>
  </feature>
</target>
```

### AArch64 target.xml

```xml
<?xml version="1.0"?>
<!DOCTYPE target SYSTEM "gdb-target.dtd">
<target version="1.0">
  <architecture>aarch64</architecture>
  <feature name="org.gnu.gdb.aarch64.core">
    <!-- x0–x30: GDB regnum 0–30 -->
    <reg name="x0"  bitsize="64" regnum="0"  type="int"      group="general"/>
    <!-- ... x1–x28 ... -->
    <reg name="x29" bitsize="64" regnum="29" type="data_ptr" group="general"/>
    <reg name="x30" bitsize="64" regnum="30" type="code_ptr" group="general"/>
    <reg name="sp"  bitsize="64" regnum="31" type="data_ptr" group="general"/>
    <reg name="pc"  bitsize="64" regnum="32" type="code_ptr" group="general"/>
    <reg name="cpsr" bitsize="32" regnum="33" type="int"     group="general"/>
  </feature>
  <feature name="org.gnu.gdb.aarch64.fpu">
    <!-- v0–v31: GDB regnum 34–65, 128-bit vector registers -->
    <reg name="v0" bitsize="128" regnum="34" type="vec128" group="float"/>
    <!-- ... v1–v31 ... -->
  </feature>
</target>
```

---

## 8. Error Handling

```rust
#[derive(Debug, thiserror::Error)]
pub enum RspError {
    #[error("no '$' start byte in packet")]
    NoStart,
    #[error("no '#' end byte in packet")]
    NoEnd,
    #[error("checksum bytes truncated")]
    TruncatedChecksum,
    #[error("checksum mismatch: actual={actual:#04x} expected={expected:#04x}")]
    ChecksumMismatch { actual: u8, expected: u8 },
    #[error("non-UTF-8 packet data")]
    Utf8(#[from] std::str::Utf8Error),
    #[error("hex parse error")]
    HexParse(#[from] std::num::ParseIntError),
    #[error("I/O error")]
    Io(#[from] io::Error),
}
```

Packet parsing errors do not crash the server; the loop sends a `-` NAK and retries. An `io::Error` closes the connection and lets `accept_and_serve` return.
