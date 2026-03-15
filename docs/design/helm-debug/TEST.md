# helm-debug — Test Plan

> **Crate:** `helm-debug`
> **Test targets:** GDB server integration, TraceLogger unit, CheckpointManager round-trip

---

## 1. GDB Server — Integration Test

**Goal:** Connect a real `gdb` binary to the simulation, set a software breakpoint, step over it, and verify register state.

### Test: `test_gdb_breakpoint_step_register`

```rust
// tests/gdb_integration.rs
use helm_debug::gdb::{GdbServer, GdbReg, StopReason};
use helm_test::SimFixture;  // test helper that builds a minimal RISC-V SE sim

#[test]
fn test_gdb_breakpoint_step_register() {
    // 1. Build a minimal RV64 SE simulation preloaded with a hello-world ELF.
    let mut fixture = SimFixture::riscv64_se("tests/fixtures/hello_world.elf");
    let entry_pc = fixture.entry_pc();

    // 2. Bind GDB server on an ephemeral port.
    let mut server = GdbServer::bind(0).expect("bind failed");
    let port = server.local_addr().unwrap().port();

    // 3. Spawn simulation thread.
    let (cmd_tx, cmd_rx) = std::sync::mpsc::channel();
    let sim_thread = std::thread::spawn(move || {
        fixture.run_with_gdb(cmd_rx);
    });

    // 4. Spawn GDB client thread using a subprocess.
    let gdb_thread = std::thread::spawn(move || {
        let output = std::process::Command::new("gdb")
            .args([
                "--batch",
                "--ex", &format!("target remote :{}", port),
                "--ex", &format!("break *{:#x}", entry_pc),
                "--ex", "continue",
                "--ex", "stepi",
                "--ex", "info registers pc",
                "--ex", "detach",
                "--ex", "quit",
            ])
            .output()
            .expect("gdb not found");
        String::from_utf8_lossy(&output.stdout).to_string()
    });

    // 5. Serve one GDB session.
    let mut target = fixture.gdb_target_mut();
    server.accept_and_serve(&mut target).expect("GDB session failed");

    let gdb_output = gdb_thread.join().unwrap();
    cmd_tx.send(()).ok();
    sim_thread.join().unwrap();

    // 6. Verify GDB output contains the expected PC value.
    let expected_after_step = format!("{:#x}", entry_pc + 4);
    assert!(
        gdb_output.contains(&expected_after_step),
        "Expected PC {expected_after_step} in GDB output, got:\n{gdb_output}"
    );
}
```

### Test: `test_gdb_memory_read_write`

```rust
#[test]
fn test_gdb_memory_read_write() {
    let mut fixture = SimFixture::riscv64_se("tests/fixtures/hello_world.elf");
    let load_addr = fixture.load_addr();

    let mut server = GdbServer::bind(0).unwrap();
    let port = server.local_addr().unwrap().port();

    // GDB: read 4 bytes at load addr, write 4 bytes, read back.
    let gdb_thread = std::thread::spawn(move || {
        std::process::Command::new("gdb")
            .args([
                "--batch",
                "--ex", &format!("target remote :{}", port),
                "--ex", &format!("x/4xb {:#x}", load_addr),
                "--ex", &format!("set {{unsigned char[4]}}{:#x} = {{0xDE,0xAD,0xBE,0xEF}}", load_addr),
                "--ex", &format!("x/4xb {:#x}", load_addr),
                "--ex", "detach",
            ])
            .output()
            .expect("gdb not found")
    });

    let mut target = fixture.gdb_target_mut();
    server.accept_and_serve(&mut target).unwrap();
    let result = gdb_thread.join().unwrap();
    let stdout = String::from_utf8_lossy(&result.stdout);
    assert!(stdout.contains("0xde"), "expected 0xde in memory read-back: {stdout}");
}
```

### Test: `test_gdb_multi_hart_vcont`

```rust
#[test]
fn test_gdb_multi_hart_vcont() {
    // Verify that vCont;s:1 steps hart 0 and vCont;s:2 steps hart 1.
    let mut fixture = SimFixture::riscv64_se_multi("tests/fixtures/mt_hello.elf", 2);
    let mut server = GdbServer::bind(0).unwrap();
    let port = server.local_addr().unwrap().port();

    let gdb_thread = std::thread::spawn(move || {
        std::process::Command::new("gdb")
            .args([
                "--batch",
                "--ex", &format!("target remote :{}", port),
                "--ex", "set scheduler-locking on",
                "--ex", "thread 1",
                "--ex", "stepi",
                "--ex", "thread 2",
                "--ex", "stepi",
                "--ex", "detach",
            ])
            .output()
            .expect("gdb not found")
    });

    let mut target = fixture.gdb_target_mut();
    server.accept_and_serve(&mut target).unwrap();
    gdb_thread.join().unwrap();
    // If no panic, the test passes. Multi-hart vCont is exercised.
}
```

---

## 2. TraceLogger — Unit Tests

### Test: `test_ring_buffer_wrap_around`

```rust
// tests/trace_unit.rs
use helm_debug::trace::{TraceLogger, TraceEvent};

#[test]
fn test_ring_buffer_wrap_around() {
    // Capacity of 4 (next power of two >= 4).
    let logger = TraceLogger::new(4);

    // Insert 6 events — last 4 should survive.
    for i in 0u64..6 {
        logger.log(TraceEvent::InsnFetch { cycle: i, hart: 0, pc: 0x1000 + i * 4, bytes: 0x13 });
    }

    let recent = logger.recent(4);
    assert_eq!(recent.len(), 4);

    // Verify the surviving events are the last four (pc = 0x1008..0x1018).
    for (j, event) in recent.iter().enumerate() {
        if let TraceEvent::InsnFetch { pc, .. } = event {
            let expected_pc = 0x1008 + j as u64 * 4;
            assert_eq!(*pc, expected_pc, "event {j}: expected pc={expected_pc:#x}, got {pc:#x}");
        } else {
            panic!("unexpected variant");
        }
    }
}
```

### Test: `test_flush_to_file`

```rust
#[test]
fn test_flush_to_file() {
    use std::io::BufRead;

    let logger = TraceLogger::new(16);
    logger.log(TraceEvent::Syscall {
        cycle: 42, hart: 0, nr: 64, args: [1, 0, 14, 0, 0, 0], ret: 14,
    });
    logger.log(TraceEvent::MemWrite {
        cycle: 43, hart: 0, addr: 0xDEAD, size: 4, value: 0xCAFE,
    });

    let tmp = tempfile::NamedTempFile::new().unwrap();
    logger.flush_to_file(tmp.path()).unwrap();

    let file = std::fs::File::open(tmp.path()).unwrap();
    let lines: Vec<String> = std::io::BufReader::new(file)
        .lines()
        .map(|l| l.unwrap())
        .collect();

    assert_eq!(lines.len(), 2, "expected 2 JSONL lines");

    let first: serde_json::Value = serde_json::from_str(&lines[0]).unwrap();
    assert_eq!(first["type"], "syscall");
    assert_eq!(first["nr"], 64);
    assert_eq!(first["ret"], 14);

    let second: serde_json::Value = serde_json::from_str(&lines[1]).unwrap();
    assert_eq!(second["type"], "mem_write");
    assert_eq!(second["addr"], 0xDEADu64);
}
```

### Test: `test_subscriber_callback`

```rust
#[test]
fn test_subscriber_callback() {
    use std::sync::{Arc, Mutex};

    let logger = TraceLogger::new(16);
    let seen: Arc<Mutex<Vec<u64>>> = Arc::new(Mutex::new(Vec::new()));

    let seen2 = Arc::clone(&seen);
    logger.subscribe(Box::new(move |event| {
        if let TraceEvent::InsnFetch { pc, .. } = event {
            seen2.lock().unwrap().push(*pc);
        }
    }));

    logger.log(TraceEvent::InsnFetch { cycle: 0, hart: 0, pc: 0x1000, bytes: 0x13 });
    logger.log(TraceEvent::InsnFetch { cycle: 1, hart: 0, pc: 0x1004, bytes: 0x93 });

    let captured = seen.lock().unwrap().clone();
    assert_eq!(captured, vec![0x1000u64, 0x1004u64]);
}
```

### Test: `test_recent_empty`

```rust
#[test]
fn test_recent_empty() {
    let logger = TraceLogger::new(16);
    let events = logger.recent(10);
    assert!(events.is_empty(), "expected empty recent on fresh logger");
}
```

### Test: `test_total_logged_counter`

```rust
#[test]
fn test_total_logged_counter() {
    let logger = TraceLogger::new(4);  // capacity 4
    for i in 0u64..10 {
        logger.log(TraceEvent::InsnFetch { cycle: i, hart: 0, pc: 0x1000, bytes: 0 });
    }
    assert_eq!(logger.total_logged(), 10, "total_logged must count all events including overwritten");
    assert_eq!(logger.buffered_count(), 4, "only capacity events remain in buffer");
}
```

---

## 3. CheckpointManager — Round-Trip Tests

### Test: `test_checkpoint_save_restore_registers`

```rust
// tests/checkpoint_roundtrip.rs
use helm_debug::checkpoint::{CheckpointManager, CheckpointHeader, CHECKPOINT_FORMAT_VERSION};
use helm_test::WorldBuilder;

#[test]
fn test_checkpoint_save_restore_registers() {
    let builder = WorldBuilder::riscv64_se();
    let mut world = builder.build().unwrap();

    // Set some known register values.
    world.set_int_reg(0, "cpu0", 10, 0xDEADBEEF_CAFEBABE);
    world.set_pc("cpu0", 0x8000_1000);

    let tmp = tempfile::NamedTempFile::new().unwrap();
    CheckpointManager::save(&world, tmp.path()).unwrap();

    // Restore into a fresh world.
    let restored = CheckpointManager::restore(tmp.path(), &builder).unwrap();

    assert_eq!(
        restored.get_int_reg("cpu0", 10),
        0xDEADBEEF_CAFEBABE,
        "integer register a0 must survive checkpoint round-trip"
    );
    assert_eq!(
        restored.get_pc("cpu0"),
        0x8000_1000,
        "PC must survive checkpoint round-trip"
    );
}
```

### Test: `test_checkpoint_header_validation`

```rust
#[test]
fn test_checkpoint_header_validation() {
    let builder = WorldBuilder::riscv64_se();
    let world = builder.build().unwrap();
    let tmp = tempfile::NamedTempFile::new().unwrap();
    CheckpointManager::save(&world, tmp.path()).unwrap();

    let header = CheckpointManager::read_header(tmp.path()).unwrap();
    assert_eq!(header.version, CHECKPOINT_FORMAT_VERSION);
    assert_eq!(header.isa, "riscv64");
    assert_eq!(header.mode, "se");
    assert!(header.created_at > 0);
}
```

### Test: `test_checkpoint_isa_mismatch_rejected`

```rust
#[test]
fn test_checkpoint_isa_mismatch_rejected() {
    let riscv_builder = WorldBuilder::riscv64_se();
    let world = riscv_builder.build().unwrap();
    let tmp = tempfile::NamedTempFile::new().unwrap();
    CheckpointManager::save(&world, tmp.path()).unwrap();

    let aarch64_builder = WorldBuilder::aarch64_se();
    let result = CheckpointManager::restore(tmp.path(), &aarch64_builder);

    assert!(
        matches!(result, Err(helm_debug::checkpoint::CheckpointError::IsaMismatch { .. })),
        "restoring RISC-V checkpoint into AArch64 world must fail"
    );
}
```

### Test: `test_checkpoint_memory_contents`

```rust
#[test]
fn test_checkpoint_memory_contents() {
    let builder = WorldBuilder::riscv64_se();
    let mut world = builder.build().unwrap();

    // Write a pattern into simulated RAM.
    let addr = 0x8000_0000u64;
    let data = b"hello checkpoint";
    world.mem_write_functional(addr, data);

    let tmp = tempfile::NamedTempFile::new().unwrap();
    CheckpointManager::save(&world, tmp.path()).unwrap();

    let restored = CheckpointManager::restore(tmp.path(), &builder).unwrap();
    let readback = restored.mem_read_functional(addr, data.len());
    assert_eq!(&readback, data, "RAM contents must survive checkpoint round-trip");
}
```

---

## Test Matrix

| Test | File | Type | Phase |
|------|------|------|-------|
| GDB breakpoint + step + register read | `tests/gdb_integration.rs` | Integration (real `gdb`) | Phase 1 |
| GDB memory read/write | `tests/gdb_integration.rs` | Integration | Phase 1 |
| GDB multi-hart vCont | `tests/gdb_integration.rs` | Integration | Phase 1 |
| Ring buffer wrap-around | `tests/trace_unit.rs` | Unit | Phase 2 |
| Flush to JSONL file | `tests/trace_unit.rs` | Unit | Phase 2 |
| Subscriber callback fires | `tests/trace_unit.rs` | Unit | Phase 2 |
| `recent()` on empty logger | `tests/trace_unit.rs` | Unit | Phase 2 |
| `total_logged` counter | `tests/trace_unit.rs` | Unit | Phase 2 |
| Register round-trip | `tests/checkpoint_roundtrip.rs` | Unit | Phase 2 |
| Header validation | `tests/checkpoint_roundtrip.rs` | Unit | Phase 2 |
| ISA mismatch rejected | `tests/checkpoint_roundtrip.rs` | Unit | Phase 2 |
| Memory contents round-trip | `tests/checkpoint_roundtrip.rs` | Unit | Phase 2 |

### Running the Tests

```bash
# All helm-debug tests (unit only, no GDB required)
cargo test -p helm-debug

# Integration tests (requires `gdb` on PATH)
cargo test -p helm-debug --test gdb_integration -- --nocapture

# With verbose trace output
RUST_LOG=helm_debug=trace cargo test -p helm-debug
```
