# helm-debug — LLD: CheckpointManager

> **Module:** `helm-debug::checkpoint`
> **Format:** CBOR (primary) + JSON (fallback/inspection)
> **Strategy:** Full-state snapshot (Phase 2); differential deferred
> **Serialization mechanism:** `HelmAttr` system — sole source of truth

---

## Table of Contents

1. [Overview](#1-overview)
2. [Public API](#2-public-api)
3. [Checkpoint File Format](#3-checkpoint-file-format)
4. [Serialization Protocol — HelmAttr](#4-serialization-protocol--helmattr)
5. [World Serialization Sequence](#5-world-serialization-sequence)
6. [Restore Sequence](#6-restore-sequence)
7. [Post-Restore HelmEventBus Re-subscription](#7-post-restore-helmeventbus-re-subscription)
8. [Error Types](#8-error-types)
9. [Implementation Notes](#9-implementation-notes)

---

## 1. Overview

The `CheckpointManager` saves and restores the complete architectural state of a simulation. It is the only checkpoint mechanism in helm-ng; individual `SimObject` implementations do not write their own serialization logic. Instead, each component declares its state as typed `HelmAttr` attributes, and `CheckpointManager` iterates all `HelmObject` instances in the `World`, collecting and serializing all attributes.

### Design Constraints

- **CBOR primary format** — compact binary, efficient to write and read. Inspectable with `ciborium` or any CBOR tool.
- **JSON fallback** — for human inspection, enabled by a CLI flag or Python API call.
- **Full-state (Phase 2)** — every attribute of every object is serialized on every `save()`. No differential tracking.
- **No manual serialization** — components never implement `checkpoint_save()` directly. The `HelmAttr` system handles all state.
- **Versioned header** — the first object in every checkpoint file is a version header that gates compatibility checks.

---

## 2. Public API

```rust
pub struct CheckpointManager;

impl CheckpointManager {
    /// Save a full checkpoint of `world` to `path` in CBOR format.
    ///
    /// Preconditions:
    /// - The simulation must be paused (all harts quiesced).
    /// - `world` must have completed the `startup` lifecycle phase.
    ///
    /// The file is written atomically: first to a `.tmp` sibling, then renamed.
    pub fn save(world: &World, path: &Path) -> Result<(), CheckpointError>;

    /// Save in human-readable JSON format (for inspection/debugging).
    /// Same semantics as `save()`; larger output file.
    pub fn save_json(world: &World, path: &Path) -> Result<(), CheckpointError>;

    /// Restore a `World` from a checkpoint at `path`.
    ///
    /// Steps:
    /// 1. Parse and validate the version header.
    /// 2. Reconstruct all `HelmObject` instances from the `World` blueprint
    ///    (same blueprint used to build the original world; supplied by `WorldBuilder`).
    /// 3. Deserialize each object's attributes from the checkpoint blob.
    /// 4. Run `init()` on each component so that HelmEventBus subscriptions
    ///    are re-established (startup() is skipped).
    ///
    /// Returns `Err` if the file is corrupt, version-incompatible, or ISA/mode mismatches.
    pub fn restore(path: &Path, builder: &WorldBuilder) -> Result<World, CheckpointError>;

    /// Restore from a JSON checkpoint.
    pub fn restore_json(path: &Path, builder: &WorldBuilder) -> Result<World, CheckpointError>;

    /// Read and return only the checkpoint header (fast path for compatibility checks).
    pub fn read_header(path: &Path) -> Result<CheckpointHeader, CheckpointError>;
}
```

---

## 3. Checkpoint File Format

A checkpoint file is a sequence of CBOR items:

```
[ header: CheckpointHeader, objects: [ObjectBlob, ...] ]
```

### CheckpointHeader

```rust
/// Appears as the first CBOR value in every checkpoint file.
///
/// Q86: `schema_version` is a monotonically increasing `u32`. Breaking format
/// changes increment it. On load: exact match → load normally; version ahead →
/// load with warning; version behind → refuse with `CheckpointError::IncompatibleVersion`.
/// A `helm checkpoint-upgrade` CLI command applies migration scripts for N→N+1 upgrades.
#[derive(Debug, Serialize, Deserialize)]
pub struct CheckpointHeader {
    /// Monotonically increasing checkpoint format version (Q86).
    /// Breaking schema changes increment this. Never decremented.
    pub schema_version: u32,             // e.g. 1
    /// Helm simulator version that wrote this checkpoint (informational only).
    pub helm_version: String,            // e.g. "0.1.0"
    /// ISA of the simulated system.
    pub isa: String,                     // e.g. "riscv64", "aarch64"
    /// Execution mode at checkpoint time.
    pub mode: String,                    // e.g. "se", "fs"
    /// Unix timestamp (seconds since epoch) when the checkpoint was written.
    pub created_at: u64,
    /// Number of `ObjectBlob` entries that follow.
    pub object_count: u32,
    /// Total simulated cycles at checkpoint time.
    pub cycle: u64,
    /// Checksum of all object blobs (CRC32 of the concatenated CBOR bytes).
    pub blob_checksum: u32,
}
```

Current version constant:

```rust
/// Monotonically increasing. Increment on every breaking checkpoint format change (Q86).
pub const CHECKPOINT_SCHEMA_VERSION: u32 = 1;
```

### ObjectBlob

```rust
/// Serialized state of one HelmObject.
#[derive(Debug, Serialize, Deserialize)]
pub struct ObjectBlob {
    /// Full dot-path of the object (e.g. "system.cpu0.icache").
    pub path: String,
    /// Opaque CBOR bytes of the object's attribute state.
    /// Produced by `AttrStore::serialize_all()`.
    pub attrs: serde_bytes::ByteBuf,
}
```

### File Layout Diagram

```
┌─────────────────────────────────────────────────┐
│  CBOR array [2 items]                           │
│                                                 │
│  Item 0: CheckpointHeader (CBOR map)            │
│    schema_version: 1                            │
│    helm_version: "0.1.0"                        │
│    isa:          "riscv64"                      │
│    mode:         "se"                           │
│    created_at:   1741824000                     │
│    object_count: 14                             │
│    cycle:        1000000000                     │
│    blob_checksum: 0xDEADBEEF                    │
│                                                 │
│  Item 1: array of ObjectBlob (CBOR array)       │
│    [0] path="system.cpu0"  attrs=<cbor bytes>   │
│    [1] path="system.cpu0.icache" attrs=<...>    │
│    [2] path="system.cpu0.dcache" attrs=<...>    │
│    [3] path="system.dram" attrs=<cbor bytes>    │
│    ...                                          │
└─────────────────────────────────────────────────┘
```

---

## 4. Serialization Protocol — HelmAttr

The `HelmAttr` system is inspired by SIMICS's attribute/interface separation. Each `HelmObject` exposes its state through an `AttrStore` — a typed, named key-value map. `CheckpointManager` calls `AttrStore::serialize_all()` per object to get the CBOR bytes for that object's blob.

### AttrStore

```rust
/// Holds all serializable attributes of one HelmObject.
pub struct AttrStore {
    attrs: HashMap<String, AttrValue>,
}

/// A typed attribute value.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum AttrValue {
    Bool(bool),
    Int(i64),
    Uint(u64),
    Float(f64),
    Bytes(Vec<u8>),
    String(String),
    List(Vec<AttrValue>),
    Map(HashMap<String, AttrValue>),
    Nil,
}

impl AttrStore {
    /// Register a required attribute. Required attributes must be present in
    /// any checkpoint blob; their absence is a restore error.
    pub fn def_required(&mut self, name: &str, value: AttrValue);

    /// Register an optional attribute. Missing from checkpoint → use default.
    pub fn def_optional(&mut self, name: &str, default: AttrValue);

    /// Serialize all attributes to CBOR bytes.
    pub fn serialize_all(&self) -> Result<Vec<u8>, CheckpointError>;

    /// Deserialize from CBOR bytes, populating all registered attributes.
    pub fn deserialize_all(&mut self, data: &[u8]) -> Result<(), CheckpointError>;

    /// Get an attribute value by name.
    pub fn get(&self, name: &str) -> Option<&AttrValue>;

    /// Set an attribute value by name.
    pub fn set(&mut self, name: &str, value: AttrValue);
}
```

### HelmObject

```rust
/// Every simulation component that participates in checkpointing implements this.
/// It supersedes the `checkpoint_save()`/`checkpoint_restore()` methods on `SimObject`.
pub trait HelmObject: SimObject {
    /// Called by CheckpointManager before `save()`.
    /// The component writes its current state into `store`.
    fn write_attrs(&self, store: &mut AttrStore);

    /// Called by CheckpointManager during `restore()`.
    /// The component reads its state from `store`.
    fn read_attrs(&mut self, store: &AttrStore);
}
```

### Example: CPU Core

```rust
impl HelmObject for RiscVCpu {
    fn write_attrs(&self, store: &mut AttrStore) {
        // Integer registers
        for i in 0..32u64 {
            store.set(&format!("x{i}"), AttrValue::Uint(self.arch.int_reg(i as usize)));
        }
        store.set("pc", AttrValue::Uint(self.arch.pc()));

        // Floating-point registers
        for i in 0..32u64 {
            store.set(&format!("f{i}"), AttrValue::Uint(self.arch.float_reg_raw(i as usize)));
        }

        // Key CSRs
        store.set("mstatus", AttrValue::Uint(self.arch.csr(0x300)));
        store.set("mepc",    AttrValue::Uint(self.arch.csr(0x341)));
        store.set("mcause",  AttrValue::Uint(self.arch.csr(0x342)));
        store.set("mtval",   AttrValue::Uint(self.arch.csr(0x343)));
        store.set("satp",    AttrValue::Uint(self.arch.csr(0x180)));
    }

    fn read_attrs(&mut self, store: &AttrStore) {
        for i in 0..32usize {
            if let Some(AttrValue::Uint(v)) = store.get(&format!("x{i}")) {
                self.arch.set_int_reg(i, *v);
            }
        }
        if let Some(AttrValue::Uint(pc)) = store.get("pc") {
            self.arch.set_pc(*pc);
        }
        // ... CSRs ...
    }
}
```

---

## 5. World Serialization Sequence

`CheckpointManager::save()` iterates all registered `HelmObject` instances in the `World` in registration order (depth-first, matching `System::register()` order). This guarantees deterministic file layout.

```rust
impl CheckpointManager {
    pub fn save(world: &World, path: &Path) -> Result<(), CheckpointError> {
        let tmp = path.with_extension("tmp");
        let file = std::fs::File::create(&tmp)?;
        let mut writer = std::io::BufWriter::new(file);

        let objects: Vec<ObjectBlob> = world
            .iter_objects()
            .map(|(obj_path, obj)| {
                let mut store = AttrStore::new();
                obj.write_attrs(&mut store);
                let attrs = store.serialize_all()?;
                Ok(ObjectBlob { path: obj_path.to_string(), attrs: attrs.into() })
            })
            .collect::<Result<_, CheckpointError>>()?;

        let header = CheckpointHeader {
            version:      CHECKPOINT_FORMAT_VERSION,
            helm_version: env!("CARGO_PKG_VERSION").to_string(),
            isa:          world.isa().to_string(),
            mode:         world.exec_mode().to_string(),
            created_at:   unix_now(),
            object_count: objects.len() as u32,
            cycle:        world.current_cycle(),
            blob_checksum: crc32_of_blobs(&objects),
        };

        ciborium::ser::into_writer(&(header, objects), &mut writer)?;
        writer.flush()?;
        drop(writer);

        std::fs::rename(tmp, path)?;
        Ok(())
    }
}
```

---

## 6. Restore Sequence

Restore does not call `startup()`. Instead, after `init()` re-establishes subscriptions, the component reads its saved state from the checkpoint and begins exactly where the simulation left off.

```
CONSTRUCT → INIT → ELABORATE → [checkpoint_restore via read_attrs] → RUN
```

```rust
impl CheckpointManager {
    pub fn restore(path: &Path, builder: &WorldBuilder) -> Result<World, CheckpointError> {
        let file = std::fs::File::open(path)?;
        let reader = std::io::BufReader::new(file);
        let (header, blobs): (CheckpointHeader, Vec<ObjectBlob>) =
            ciborium::de::from_reader(reader)?;

        // Validate header
        if header.version != CHECKPOINT_FORMAT_VERSION {
            return Err(CheckpointError::VersionMismatch {
                found: header.version.to_string(),
                expected: CHECKPOINT_FORMAT_VERSION.to_string(),
            });
        }
        if header.isa != builder.isa().to_string() {
            return Err(CheckpointError::IsaMismatch {
                checkpoint: header.isa,
                builder: builder.isa().to_string(),
            });
        }
        if header.mode != builder.exec_mode().to_string() {
            return Err(CheckpointError::ModeMismatch {
                checkpoint: header.mode,
                builder: builder.exec_mode().to_string(),
            });
        }

        // Rebuild world skeleton (no startup())
        let mut world = builder.build_without_startup()?;

        // Restore each object's attributes
        let blob_map: HashMap<String, &ObjectBlob> =
            blobs.iter().map(|b| (b.path.clone(), b)).collect();

        for (obj_path, obj) in world.iter_objects_mut() {
            match blob_map.get(obj_path) {
                Some(blob) => {
                    let mut store = AttrStore::new();
                    store.deserialize_all(&blob.attrs)?;
                    obj.read_attrs(&store);
                }
                None => {
                    return Err(CheckpointError::MissingObject { path: obj_path.to_string() });
                }
            }
        }

        Ok(world)
    }
}
```

---

## 7. Post-Restore HelmEventBus Re-subscription

`HelmEventBus` subscriptions are registered during `init()`. On a normal simulation startup, the call order is `init → elaborate → startup`. On restore, the call order is `init → elaborate → [restore attrs]` — `startup()` is skipped because the checkpoint already encodes the post-startup state.

The `init()` call ensures that every component re-subscribes to `HelmEventBus` during restore, exactly as it does during a normal startup. This means:

- `TraceLogger` re-registers its `HelmEventBus` callbacks in `init()`.
- `GdbServer` re-registers its pause/resume hooks in `init()`.
- No special "restore mode" flag is needed; `init()` is idempotent with respect to subscriptions.

```rust
// Pseudocode in WorldBuilder::build_without_startup()
fn build_without_startup(&self) -> Result<World, CheckpointError> {
    let mut world = self.construct_world();
    for obj in world.iter_objects_mut() {
        obj.init();           // re-establishes subscriptions
    }
    for obj in world.iter_objects_mut() {
        obj.elaborate(&mut world.system);  // re-establishes cross-object wiring
    }
    // startup() is intentionally NOT called here
    Ok(world)
}
```

---

## 8. Error Types

```rust
#[derive(Debug, thiserror::Error)]
pub enum CheckpointError {
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),

    #[error("CBOR encode error: {0}")]
    CborEncode(#[from] ciborium::ser::Error<io::Error>),

    #[error("CBOR decode error: {0}")]
    CborDecode(#[from] ciborium::de::Error<io::Error>),

    #[error("checkpoint format version mismatch: file has '{found}', current is '{expected}'")]
    VersionMismatch { found: String, expected: String },

    #[error("ISA mismatch: checkpoint has '{checkpoint}', builder has '{builder}'")]
    IsaMismatch { checkpoint: String, builder: String },

    #[error("execution mode mismatch: checkpoint has '{checkpoint}', builder has '{builder}'")]
    ModeMismatch { checkpoint: String, builder: String },

    #[error("checkpoint references object '{path}' which is not in the current World")]
    MissingObject { path: String },

    #[error("required attribute '{attr}' missing from object '{obj}'")]
    MissingAttribute { obj: String, attr: String },

    #[error("blob checksum mismatch: computed {computed:#010x}, stored {stored:#010x}")]
    ChecksumMismatch { computed: u32, stored: u32 },

    #[error("attribute type mismatch on '{obj}.{attr}': expected {expected}, got {got}")]
    AttributeTypeMismatch { obj: String, attr: String, expected: &'static str, got: &'static str },
}
```

---

## 9. Implementation Notes

### Atomic Write

`save()` always writes to a `.tmp` sibling first, then `rename()`s it into place. This prevents a partially-written file from being seen as a valid checkpoint if the process is interrupted.

### JSON Fallback

`save_json()` uses `serde_json` instead of `ciborium`. The `CheckpointHeader` and `ObjectBlob` types derive both `serde::Serialize` and `serde::Deserialize`, so switching serializer requires only changing the writer call.

```rust
pub fn save_json(world: &World, path: &Path) -> Result<(), CheckpointError> {
    // Same logic as save(), but:
    serde_json::to_writer_pretty(&mut writer, &(header, objects))?;
    Ok(())
}
```

### Memory Usage for Large Checkpoints

For simulations with large RAM (e.g. 4 GiB), serializing `AttrValue::Bytes(ram_data)` can require significant memory. Stream the CBOR encoding directly to the file rather than collecting all blobs into a `Vec<ObjectBlob>` first.

```rust
// Streaming pattern for large checkpoints:
let mut ser = ciborium::ser::into_writer_with_buffer(&mut writer, &header, buf)?;
for (path, obj) in world.iter_objects() {
    let mut store = AttrStore::new();
    obj.write_attrs(&mut store);
    let blob = ObjectBlob { path: path.to_string(), attrs: store.serialize_all()?.into() };
    ciborium::ser::into_writer(&blob, &mut writer)?;
}
```

### CRC32 Checksum

```rust
fn crc32_of_blobs(blobs: &[ObjectBlob]) -> u32 {
    use crc32fast::Hasher;
    let mut h = Hasher::new();
    for blob in blobs {
        h.update(blob.attrs.as_ref());
    }
    h.finalize()
}
```


---

## Design Decisions from Q&A

### Design Decision: schema_version is a monotonic u32 (Q86)

The checkpoint header contains `schema_version: u32` (monotonically increasing integer). Breaking format changes increment it. On load: exact match → load normally; same major with minor difference → load with warning; version behind current → refuse with `CheckpointError::IncompatibleVersion { checkpoint: X, simulator: Y }`. A `helm checkpoint-upgrade` CLI command applies migration scripts for N→N+1 upgrades. The monotonic `u32` integer is simpler than semantic versioning and unambiguous about ordering. Silent corruption from version mismatch is unacceptable for research reproducibility.

### Design Decision: CBOR for attribute state, raw binary for memory (Q88)

The checkpoint format uses CBOR (via `ciborium`) for attribute state (device registers, CPU state, configuration), concatenated with a raw binary memory image for RAM. The file structure: `[CBOR header][CBOR attribute map][raw memory bytes]`. The entire file is zstd-compressed. Rationale: CBOR is binary (compact, fast), handles all Rust primitive types without loss, is a published standard (RFC 8949), and has good Rust library support. The raw binary memory blob is necessary — encoding 256 MiB as CBOR byte strings would work but adds unnecessary framing overhead. A `helm dump-checkpoint` tool decodes the CBOR to JSON for human inspection.

### Design Decision: Full-state checkpoints for Phase 0 and 1 (Q87)

Full-state checkpoints are self-contained — restoring does not require any other file. Memory contents are written as a binary blob. Register and device state use the `HelmAttr` attribute format. Differential checkpoints are a Phase 2 feature. Rationale: the gem5 full-state model is simpler to implement correctly and produces self-contained checkpoints that researchers can share without worrying about base checkpoint availability.
