//! Flattened Device Tree (FDT/DTB) — runtime builder with overlay support.
//!
//! The DTB handling strategy is derived from the **platform** and the
//! CLI arguments actually present — just like QEMU and gem5, the user
//! never has to think about "boot methods" or "DTB policies".
//!
//! | Situation | What happens |
//! |-----------|--------------|
//! | `-M virt -kernel Image` | DTB generated from platform + devices |
//! | `-M virt -kernel Image --dtb base.dtb` | User DTB patched with `-device` extras |
//! | `-M virt -kernel Image --dtb base.dtb` (no extras) | User DTB passed through verbatim |
//! | `-M virt -bios edk2.fd` | UEFI boot — no DTB |
//! | `-M virt -drive file=hd.img` (no -kernel) | Firmware/bootloader boot — no DTB |
//!
//! # FDT binary layout (spec: devicetree.org v0.4)
//!
//! ```text
//! ┌──────────────────────────┐  offset 0
//! │  fdt_header (40 bytes)   │
//! ├──────────────────────────┤  off_mem_rsvmap
//! │  memory reservation map  │
//! ├──────────────────────────┤  off_dt_struct
//! │  structure block         │
//! ├──────────────────────────┤  off_dt_strings
//! │  strings block           │
//! └──────────────────────────┘  totalsize
//! ```

use crate::platform::Platform;
use std::collections::HashMap;

// ── DTB policy & boot method ────────────────────────────────────────────────

/// Controls how the DTB is produced (or not) for the guest.
///
/// Users never set this directly — it is inferred by [`DtbPolicy::infer`]
/// from the platform and the CLI arguments present (like QEMU and gem5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DtbPolicy {
    Generate,
    Patch,
    Passthrough,
    None,
}

impl DtbPolicy {
    /// Infer the DTB policy from the platform and what the user supplied.
    ///
    /// Mirrors the logic of QEMU's machine-class `init` and gem5's
    /// `generateDtb()`: the platform knows whether it uses DTB, and the
    /// presence/absence of `-kernel`, `--dtb`, `-bios`, and `-device`
    /// determines the exact strategy.
    ///
    /// # Arguments
    /// * `platform_uses_dtb` — does this machine type use device trees?
    ///   (e.g. `virt`=true, an x86 ACPI platform=false)
    /// * `has_kernel` — was `-kernel` passed? (direct kernel boot)
    /// * `has_bios` — was `-bios` passed? (firmware boot, e.g. EDK2)
    /// * `has_drive_no_kernel` — was `-drive` passed without `-kernel`?
    ///   (disk boot — firmware/bootloader provides DTB)
    /// * `user_dtb` — was `--dtb` passed?
    /// * `has_extra_devices` — were any `-device`/`-driver` args given?
    pub fn infer(ctx: &InferCtx) -> Self {
        if !ctx.platform_uses_dtb {
            return DtbPolicy::None;
        }
        if ctx.has_bios {
            return DtbPolicy::None;
        }
        if !ctx.has_kernel && ctx.has_drive {
            return if ctx.user_dtb { DtbPolicy::Passthrough } else { DtbPolicy::None };
        }
        if ctx.user_dtb && ctx.has_extra_devices {
            return DtbPolicy::Patch;
        }
        if ctx.user_dtb {
            return DtbPolicy::Passthrough;
        }
        DtbPolicy::Generate
    }
}

/// Context for [`DtbPolicy::infer`] — gathered from the platform and
/// CLI args so the user never has to specify boot methods explicitly.
pub struct InferCtx {
    pub platform_uses_dtb: bool,
    pub has_kernel: bool,
    pub has_bios: bool,
    pub has_drive: bool,
    pub user_dtb: bool,
    pub has_extra_devices: bool,
}

impl InferCtx {
    /// Build from a Platform and CLI presence flags.
    pub fn from_platform(
        platform: &Platform,
        has_kernel: bool,
        has_bios: bool,
        has_drive: bool,
        user_dtb: bool,
        has_extra_devices: bool,
    ) -> Self {
        Self {
            platform_uses_dtb: platform.uses_dtb,
            has_kernel,
            has_bios,
            has_drive,
            user_dtb,
            has_extra_devices,
        }
    }
}

impl std::fmt::Display for DtbPolicy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Generate => write!(f, "generate"),
            Self::Patch => write!(f, "patch"),
            Self::Passthrough => write!(f, "passthrough"),
            Self::None => write!(f, "none"),
        }
    }
}


// ── FDT constants ───────────────────────────────────────────────────────────

const FDT_MAGIC: u32 = 0xD00DFEED;
const FDT_VERSION: u32 = 17;
const FDT_LAST_COMP_VERSION: u32 = 16;
const FDT_HEADER_SIZE: u32 = 40;

const FDT_BEGIN_NODE: u32 = 0x0000_0001;
const FDT_END_NODE: u32 = 0x0000_0002;
const FDT_PROP: u32 = 0x0000_0003;
const FDT_END: u32 = 0x0000_0009;

// ── Public types ────────────────────────────────────────────────────────────

/// A property value in the device tree.
#[derive(Debug, Clone)]
pub enum FdtValue {
    Empty,
    U32(u32),
    U64(u64),
    String(String),
    StringList(Vec<String>),
    Bytes(Vec<u8>),
    Reg(Vec<(u64, u64)>),
    U32List(Vec<u32>),
    Phandle(u32),
}

/// A single node in the device tree.
#[derive(Debug, Clone)]
pub struct FdtNode {
    pub name: String,
    pub properties: Vec<(String, FdtValue)>,
    pub children: Vec<FdtNode>,
}

impl FdtNode {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            properties: Vec::new(),
            children: Vec::new(),
        }
    }

    pub fn with_prop(mut self, key: impl Into<String>, val: FdtValue) -> Self {
        self.properties.push((key.into(), val));
        self
    }

    pub fn with_child(mut self, child: FdtNode) -> Self {
        self.children.push(child);
        self
    }

    pub fn add_prop(&mut self, key: impl Into<String>, val: FdtValue) {
        self.properties.push((key.into(), val));
    }

    pub fn add_child(&mut self, child: FdtNode) {
        self.children.push(child);
    }

    /// Find a direct child by name prefix (e.g. "uart" matches "uart@9000000").
    pub fn find_child(&self, prefix: &str) -> Option<&FdtNode> {
        self.children.iter().find(|c| c.name.starts_with(prefix))
    }

    /// Remove a direct child by exact name.
    pub fn remove_child(&mut self, name: &str) -> bool {
        let before = self.children.len();
        self.children.retain(|c| c.name != name);
        self.children.len() != before
    }
}

/// Trait for devices that can describe themselves in the device tree.
pub trait FdtDescriptor {
    fn fdt_node(&self, base_addr: u64, irq_start: u32) -> Option<FdtNode>;
    fn fdt_compatible(&self) -> Vec<&str> {
        vec![]
    }
}

// ── DeviceSpec ──────────────────────────────────────────────────────────────

/// Parsed `-device` / `-driver` specification.
///
/// Format: `TYPE[,key=val,...]`
///
/// ```text
/// virtio-net-device,netdev=net0,mac=52:54:00:12:34:56
/// pl011,addr=0x9040000
/// ```
#[derive(Debug, Clone)]
pub struct DeviceSpec {
    pub type_name: String,
    pub properties: HashMap<String, String>,
}

impl DeviceSpec {
    pub fn parse(spec: &str) -> Self {
        let mut parts = spec.split(',');
        let type_name = parts.next().unwrap_or("").to_string();
        let mut properties = HashMap::new();
        for part in parts {
            if let Some((k, v)) = part.split_once('=') {
                properties.insert(k.to_string(), v.to_string());
            }
        }
        Self { type_name, properties }
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.properties.get(key).map(|s| s.as_str())
    }

    pub fn get_u64(&self, key: &str) -> Option<u64> {
        self.get(key).and_then(|v| {
            if let Some(hex) = v.strip_prefix("0x") {
                u64::from_str_radix(hex, 16).ok()
            } else {
                v.parse().ok()
            }
        })
    }

    pub fn get_u32(&self, key: &str) -> Option<u32> {
        self.get_u64(key).map(|v| v as u32)
    }
}

// ── FDT binary builder ──────────────────────────────────────────────────────

/// Low-level DTB blob builder from a tree of [`FdtNode`]s.
pub struct FdtBuilder {
    root: FdtNode,
    boot_cpuid: u32,
    mem_reservations: Vec<(u64, u64)>,
}

impl FdtBuilder {
    pub fn new() -> Self {
        Self {
            root: FdtNode::new(""),
            boot_cpuid: 0,
            mem_reservations: Vec::new(),
        }
    }

    pub fn set_root(&mut self, root: FdtNode) {
        self.root = root;
        self.root.name = String::new();
    }

    pub fn root_mut(&mut self) -> &mut FdtNode {
        &mut self.root
    }

    pub fn add_mem_reservation(&mut self, addr: u64, size: u64) {
        self.mem_reservations.push((addr, size));
    }

    pub fn set_boot_cpuid(&mut self, cpuid: u32) {
        self.boot_cpuid = cpuid;
    }

    pub fn build(&self) -> Vec<u8> {
        let mut strings = StringTable::new();
        let struct_bytes = self.emit_node(&self.root, &mut strings);
        let strings_bytes = strings.into_bytes();

        let mem_rsv_size = (self.mem_reservations.len() + 1) * 16;
        let off_mem_rsvmap = FDT_HEADER_SIZE;
        let off_dt_struct = off_mem_rsvmap + mem_rsv_size as u32;
        let off_dt_strings = off_dt_struct + struct_bytes.len() as u32;
        let totalsize = off_dt_strings + strings_bytes.len() as u32;

        let mut blob = Vec::with_capacity(totalsize as usize);

        blob.extend_from_slice(&FDT_MAGIC.to_be_bytes());
        blob.extend_from_slice(&totalsize.to_be_bytes());
        blob.extend_from_slice(&off_dt_struct.to_be_bytes());
        blob.extend_from_slice(&off_dt_strings.to_be_bytes());
        blob.extend_from_slice(&off_mem_rsvmap.to_be_bytes());
        blob.extend_from_slice(&FDT_VERSION.to_be_bytes());
        blob.extend_from_slice(&FDT_LAST_COMP_VERSION.to_be_bytes());
        blob.extend_from_slice(&self.boot_cpuid.to_be_bytes());
        blob.extend_from_slice(&(strings_bytes.len() as u32).to_be_bytes());
        blob.extend_from_slice(&(struct_bytes.len() as u32).to_be_bytes());

        for &(addr, size) in &self.mem_reservations {
            blob.extend_from_slice(&addr.to_be_bytes());
            blob.extend_from_slice(&size.to_be_bytes());
        }
        blob.extend_from_slice(&0u64.to_be_bytes());
        blob.extend_from_slice(&0u64.to_be_bytes());

        blob.extend_from_slice(&struct_bytes);
        blob.extend_from_slice(&strings_bytes);

        blob
    }

    fn emit_node(&self, node: &FdtNode, strings: &mut StringTable) -> Vec<u8> {
        let mut buf = Vec::new();

        buf.extend_from_slice(&FDT_BEGIN_NODE.to_be_bytes());
        buf.extend_from_slice(node.name.as_bytes());
        buf.push(0);
        pad_to_4(&mut buf);

        for (key, val) in &node.properties {
            let val_bytes = value_to_bytes(val);
            let name_off = strings.intern(key);
            buf.extend_from_slice(&FDT_PROP.to_be_bytes());
            buf.extend_from_slice(&(val_bytes.len() as u32).to_be_bytes());
            buf.extend_from_slice(&name_off.to_be_bytes());
            buf.extend_from_slice(&val_bytes);
            pad_to_4(&mut buf);
        }

        for child in &node.children {
            buf.extend_from_slice(&self.emit_node(child, strings));
        }

        buf.extend_from_slice(&FDT_END_NODE.to_be_bytes());

        if node.name.is_empty() {
            buf.extend_from_slice(&FDT_END.to_be_bytes());
        }

        buf
    }
}

impl Default for FdtBuilder {
    fn default() -> Self {
        Self::new()
    }
}

// ── DTB parser (blob → FdtNode tree) ────────────────────────────────────────

/// Parse a DTB binary blob into an [`FdtNode`] tree.
///
/// Returns `None` if the blob is invalid (bad magic, truncated, etc.).
pub fn parse_dtb(blob: &[u8]) -> Option<FdtNode> {
    if blob.len() < FDT_HEADER_SIZE as usize {
        return None;
    }
    let magic = read_be_u32(blob, 0);
    if magic != FDT_MAGIC {
        return None;
    }
    let totalsize = read_be_u32(blob, 4) as usize;
    if blob.len() < totalsize {
        return None;
    }
    let off_dt_struct = read_be_u32(blob, 8) as usize;
    let off_dt_strings = read_be_u32(blob, 12) as usize;

    let struct_block = &blob[off_dt_struct..off_dt_strings.min(totalsize)];
    let strings_block = &blob[off_dt_strings..totalsize];

    let mut cursor = 0;
    parse_node(struct_block, strings_block, &mut cursor)
}

fn parse_node(structs: &[u8], strings: &[u8], cursor: &mut usize) -> Option<FdtNode> {
    if *cursor + 4 > structs.len() {
        return None;
    }
    let token = read_be_u32(structs, *cursor);
    if token != FDT_BEGIN_NODE {
        return None;
    }
    *cursor += 4;

    let name_start = *cursor;
    while *cursor < structs.len() && structs[*cursor] != 0 {
        *cursor += 1;
    }
    let name = String::from_utf8_lossy(&structs[name_start..*cursor]).into_owned();
    *cursor += 1; // skip NUL
    align_up_4(cursor);

    let mut node = FdtNode::new(name);

    loop {
        if *cursor + 4 > structs.len() {
            break;
        }
        let token = read_be_u32(structs, *cursor);
        match token {
            FDT_PROP => {
                *cursor += 4;
                let val_len = read_be_u32(structs, *cursor) as usize;
                *cursor += 4;
                let name_off = read_be_u32(structs, *cursor) as usize;
                *cursor += 4;
                let val_data = if val_len > 0 && *cursor + val_len <= structs.len() {
                    structs[*cursor..*cursor + val_len].to_vec()
                } else {
                    vec![]
                };
                *cursor += val_len;
                align_up_4(cursor);

                let prop_name = read_cstr(strings, name_off);
                node.properties.push((prop_name, FdtValue::Bytes(val_data)));
            }
            FDT_BEGIN_NODE => {
                if let Some(child) = parse_node(structs, strings, cursor) {
                    node.children.push(child);
                }
            }
            FDT_END_NODE => {
                *cursor += 4;
                break;
            }
            FDT_END => {
                *cursor += 4;
                break;
            }
            _ => {
                *cursor += 4; // skip unknown / FDT_NOP
            }
        }
    }

    Some(node)
}

fn read_cstr(buf: &[u8], off: usize) -> String {
    let mut end = off;
    while end < buf.len() && buf[end] != 0 {
        end += 1;
    }
    String::from_utf8_lossy(&buf[off..end]).into_owned()
}

fn read_be_u32(buf: &[u8], off: usize) -> u32 {
    u32::from_be_bytes([buf[off], buf[off + 1], buf[off + 2], buf[off + 3]])
}

fn align_up_4(v: &mut usize) {
    *v = (*v + 3) & !3;
}

// ── RuntimeDtb — live DTB that can be patched at any time ───────────────────

/// A runtime DTB that keeps the node tree in memory and can be
/// re-serialized after mutations (hot-plug, Python device_add, etc.).
///
/// # Lifecycle
///
/// ```text
///  ┌─────────────────────────┐
///  │  Optional --dtb file    │──parse──►┐
///  └─────────────────────────┘          │   merge
///  ┌─────────────────────────┐          ├─────────►  RuntimeDtb
///  │  Platform skeleton      │──build──►┘              │
///  └─────────────────────────┘                         │
///  ┌─────────────────────────┐                         │ add_device()
///  │  -device / -driver CLI  │──────────────────────►──┤ add_node()
///  └─────────────────────────┘                         │
///  ┌─────────────────────────┐                         │ (anytime)
///  │  Python device_add()    │──────────────────────►──┘
///  └─────────────────────────┘
///                                           ▼
///                                    to_blob() → guest RAM
/// ```
pub struct RuntimeDtb {
    root: FdtNode,
    next_phandle: u32,
    next_spi: u32,
    mem_reservations: Vec<(u64, u64)>,
}

impl RuntimeDtb {
    /// Create from a platform and config, optionally merging a base DTB.
    ///
    /// If `base_blob` is `Some`, its nodes are parsed and used as the
    /// starting tree.  Platform-generated nodes are added for any devices
    /// not already present.  CLI / Python devices are then overlaid on top.
    pub fn new(
        platform: &Platform,
        config: &DtbConfig,
        base_blob: Option<&[u8]>,
    ) -> Self {
        let mut root = if let Some(blob) = base_blob {
            parse_dtb(blob).unwrap_or_else(|| build_skeleton_root(platform, config))
        } else {
            build_skeleton_root(platform, config)
        };

        let mut next_phandle = scan_max_phandle(&root) + 1;
        let mut next_spi: u32 = 32;

        // If the base DTB was user-supplied, overlay platform devices
        // that are not already present.
        for (name, base) in platform.device_map() {
            let node_prefix = device_node_prefix(name);
            if root.find_child(&node_prefix).is_some() {
                continue; // already described in the base DTB
            }
            if let Some(node) = device_to_fdt_node(name, *base, &mut next_spi) {
                root.add_child(node);
            }
        }

        // Overlay CLI extra devices.
        for spec in &config.extra_devices {
            if let Some(node) = device_spec_to_fdt_node(spec, &mut next_spi) {
                root.add_child(node);
            }
        }

        // Ensure the GIC phandle is present when we built the skeleton.
        if base_blob.is_none() {
            let gic_ph = next_phandle;
            next_phandle += 1;
            inject_gic_phandle(&mut root, gic_ph);
        }

        Self {
            root,
            next_phandle,
            next_spi,
            mem_reservations: Vec::new(),
        }
    }

    /// Add a device at runtime (hot-plug / Python `device_add()`).
    ///
    /// Inserts a new node into the tree.  Call [`to_blob`] afterwards to
    /// get the updated binary and write it into guest RAM.
    pub fn add_device(&mut self, spec: &DeviceSpec) {
        if let Some(node) = device_spec_to_fdt_node(spec, &mut self.next_spi) {
            self.root.add_child(node);
        }
    }

    /// Insert an arbitrary pre-built node.
    pub fn add_node(&mut self, node: FdtNode) {
        self.root.add_child(node);
    }

    /// Remove a device node by name (e.g. `"virtio_mmio@a000000"`).
    pub fn remove_device(&mut self, node_name: &str) -> bool {
        self.root.remove_child(node_name)
    }

    /// Allocate a fresh phandle.
    pub fn alloc_phandle(&mut self) -> u32 {
        let ph = self.next_phandle;
        self.next_phandle += 1;
        ph
    }

    /// Allocate a fresh SPI IRQ number.
    pub fn alloc_spi(&mut self) -> u32 {
        let irq = self.next_spi;
        self.next_spi += 1;
        irq
    }

    /// Read-only access to the current node tree.
    pub fn root(&self) -> &FdtNode {
        &self.root
    }

    /// Mutable access to the current node tree.
    pub fn root_mut(&mut self) -> &mut FdtNode {
        &mut self.root
    }

    /// Add a memory reservation.
    pub fn add_mem_reservation(&mut self, addr: u64, size: u64) {
        self.mem_reservations.push((addr, size));
    }

    /// Serialize the current tree to a DTB blob.
    pub fn to_blob(&self) -> Vec<u8> {
        let mut builder = FdtBuilder::new();
        builder.set_root(self.root.clone());
        for &(addr, size) in &self.mem_reservations {
            builder.add_mem_reservation(addr, size);
        }
        builder.build()
    }
}

/// Scan the tree for the highest phandle value.
fn scan_max_phandle(node: &FdtNode) -> u32 {
    let mut max = 0u32;
    for (key, val) in &node.properties {
        if key == "phandle" || key == "linux,phandle" {
            if let FdtValue::Phandle(ph) = val {
                max = max.max(*ph);
            }
            if let FdtValue::U32(ph) = val {
                max = max.max(*ph);
            }
            if let FdtValue::Bytes(b) = val {
                if b.len() == 4 {
                    max = max.max(u32::from_be_bytes([b[0], b[1], b[2], b[3]]));
                }
            }
        }
    }
    for child in &node.children {
        max = max.max(scan_max_phandle(child));
    }
    max
}

fn device_node_prefix(name: &str) -> String {
    let lower = name.to_lowercase();
    if lower.contains("uart") || lower.contains("pl011") {
        return "pl011@".to_string();
    }
    if lower.contains("gic") {
        return "intc@".to_string();
    }
    if lower.contains("virtio") {
        return "virtio_mmio@".to_string();
    }
    format!("{name}@")
}

fn inject_gic_phandle(root: &mut FdtNode, gic_phandle: u32) {
    for child in &mut root.children {
        if child.name.starts_with("intc@") {
            let has_phandle = child.properties.iter().any(|(k, _)| k == "phandle");
            if !has_phandle {
                child.add_prop("phandle", FdtValue::Phandle(gic_phandle));
            }
            break;
        }
    }
    let has_irq_parent = root.properties.iter().any(|(k, _)| k == "interrupt-parent");
    if !has_irq_parent {
        root.add_prop("interrupt-parent", FdtValue::Phandle(gic_phandle));
    }
}

// ── DtbConfig ───────────────────────────────────────────────────────────────

/// Configuration for DTB generation.
pub struct DtbConfig {
    pub ram_base: u64,
    pub ram_size: u64,
    pub num_cpus: u32,
    pub bootargs: String,
    pub initrd: Option<(u64, u64)>,
    pub extra_devices: Vec<DeviceSpec>,
    pub gic_dist_base: u64,
    pub gic_cpu_base: u64,
    pub gic_version: u32,
    pub psci_method: String,
}

impl Default for DtbConfig {
    fn default() -> Self {
        Self {
            ram_base: 0x4000_0000,
            ram_size: 256 * 1024 * 1024,
            num_cpus: 1,
            bootargs: String::new(),
            initrd: None,
            extra_devices: Vec::new(),
            gic_dist_base: 0x0800_0000,
            gic_cpu_base: 0x0801_0000,
            gic_version: 2,
            psci_method: "hvc".to_string(),
        }
    }
}

// ── Skeleton builder ────────────────────────────────────────────────────────

fn build_skeleton_root(platform: &Platform, config: &DtbConfig) -> FdtNode {
    let mut root = FdtNode::new("");
    root.add_prop("compatible", FdtValue::StringList(vec![
        "linux,dummy-virt".to_string(),
    ]));
    root.add_prop("#address-cells", FdtValue::U32(2));
    root.add_prop("#size-cells", FdtValue::U32(2));
    root.add_prop("model", FdtValue::String(format!("HELM {}", platform.name)));

    // /chosen
    let mut chosen = FdtNode::new("chosen");
    if !config.bootargs.is_empty() {
        chosen.add_prop("bootargs", FdtValue::String(config.bootargs.clone()));
    }
    chosen.add_prop("stdout-path", FdtValue::String("/pl011@9000000".to_string()));
    if let Some((start, end)) = config.initrd {
        chosen.add_prop("linux,initrd-start", FdtValue::U64(start));
        chosen.add_prop("linux,initrd-end", FdtValue::U64(end));
    }
    root.add_child(chosen);

    // /memory
    let mut memory = FdtNode::new(format!("memory@{:x}", config.ram_base));
    memory.add_prop("device_type", FdtValue::String("memory".to_string()));
    memory.add_prop("reg", FdtValue::Reg(vec![(config.ram_base, config.ram_size)]));
    root.add_child(memory);

    // /cpus
    let mut cpus = FdtNode::new("cpus");
    cpus.add_prop("#address-cells", FdtValue::U32(1));
    cpus.add_prop("#size-cells", FdtValue::U32(0));
    for i in 0..config.num_cpus {
        let mut cpu = FdtNode::new(format!("cpu@{i}"));
        cpu.add_prop("device_type", FdtValue::String("cpu".to_string()));
        cpu.add_prop("compatible", FdtValue::String("arm,cortex-a53".to_string()));
        cpu.add_prop("reg", FdtValue::U32(i));
        cpu.add_prop("enable-method", FdtValue::String("psci".to_string()));
        cpus.add_child(cpu);
    }
    root.add_child(cpus);

    // /psci
    let mut psci = FdtNode::new("psci");
    psci.add_prop("compatible", FdtValue::StringList(vec![
        "arm,psci-1.0".to_string(),
        "arm,psci-0.2".to_string(),
        "arm,psci".to_string(),
    ]));
    psci.add_prop("method", FdtValue::String(config.psci_method.clone()));
    root.add_child(psci);

    // /timer
    let mut timer = FdtNode::new("timer");
    timer.add_prop("compatible", FdtValue::String("arm,armv8-timer".to_string()));
    timer.add_prop("interrupts", FdtValue::U32List(vec![
        1, 13, 0xf04, 1, 14, 0xf04, 1, 11, 0xf04, 1, 10, 0xf04,
    ]));
    timer.add_prop("always-on", FdtValue::Empty);
    root.add_child(timer);

    // /intc (GIC)
    let mut intc = FdtNode::new(format!("intc@{:x}", config.gic_dist_base));
    if config.gic_version == 3 {
        intc.add_prop("compatible", FdtValue::String("arm,gic-v3".to_string()));
        intc.add_prop("reg", FdtValue::Reg(vec![
            (config.gic_dist_base, 0x10000),
            (config.gic_cpu_base, 0x10000),
        ]));
    } else {
        intc.add_prop("compatible", FdtValue::String("arm,cortex-a15-gic".to_string()));
        intc.add_prop("reg", FdtValue::Reg(vec![
            (config.gic_dist_base, 0x1000),
            (config.gic_cpu_base, 0x2000),
        ]));
    }
    intc.add_prop("#interrupt-cells", FdtValue::U32(3));
    intc.add_prop("interrupt-controller", FdtValue::Empty);
    intc.add_prop("#address-cells", FdtValue::U32(0));
    root.add_child(intc);

    // /pl011@9000000 — UART0 (serial console)
    let mut uart0 = FdtNode::new("pl011@9000000");
    uart0.add_prop("compatible", FdtValue::StringList(vec![
        "arm,pl011".to_string(), "arm,primecell".to_string(),
    ]));
    uart0.add_prop("reg", FdtValue::Reg(vec![(0x0900_0000, 0x1000)]));
    uart0.add_prop("interrupts", FdtValue::U32List(vec![0, 1, 4])); // SPI #1
    uart0.add_prop("clock-names", FdtValue::StringList(vec![
        "uartclk".to_string(), "apb_pclk".to_string(),
    ]));
    uart0.add_prop("clocks", FdtValue::U32List(vec![0x8000, 0x8000])); // dummy clock phandles
    root.add_child(uart0);

    // /pl011@9001000 — UART1
    let mut uart1 = FdtNode::new("pl011@9001000");
    uart1.add_prop("compatible", FdtValue::StringList(vec![
        "arm,pl011".to_string(), "arm,primecell".to_string(),
    ]));
    uart1.add_prop("reg", FdtValue::Reg(vec![(0x0900_1000, 0x1000)]));
    uart1.add_prop("interrupts", FdtValue::U32List(vec![0, 2, 4])); // SPI #2
    root.add_child(uart1);

    root
}

// ── Top-level convenience (backward compat) ─────────────────────────────────

/// Generate a complete DTB for an ARM virt platform.
///
/// Convenience wrapper around [`RuntimeDtb`].  For runtime hot-plug,
/// use `RuntimeDtb` directly.
pub fn generate_virt_dtb(platform: &Platform, config: &DtbConfig) -> Vec<u8> {
    RuntimeDtb::new(platform, config, None).to_blob()
}

/// Parse a user-supplied DTB, merge platform + CLI devices on top.
pub fn patch_dtb(base_blob: &[u8], platform: &Platform, config: &DtbConfig) -> Vec<u8> {
    RuntimeDtb::new(platform, config, Some(base_blob)).to_blob()
}

/// The resolved DTB outcome after applying the [`DtbPolicy`].
#[derive(Debug)]
pub enum ResolvedDtb {
    /// A DTB blob to place in guest RAM.
    Blob(Vec<u8>),
    /// No DTB — the platform uses ACPI or the bootloader provides one.
    None,
}

/// Resolve the final DTB according to the [`DtbPolicy`].
///
/// Main entry point — the policy is derived from the platform and CLI
/// context automatically, matching QEMU/gem5 conventions.
///
/// # Arguments
/// * `platform` — the simulated machine
/// * `config` — RAM/CPU/device configuration
/// * `user_dtb_blob` — contents of `--dtb <file>`, if any
/// * `ctx` — inference context (platform + CLI flags)
pub fn resolve_dtb(
    platform: &Platform,
    config: &DtbConfig,
    user_dtb_blob: Option<&[u8]>,
    ctx: &InferCtx,
) -> ResolvedDtb {
    let policy = DtbPolicy::infer(ctx);

    match policy {
        DtbPolicy::None => ResolvedDtb::None,
        DtbPolicy::Passthrough => {
            match user_dtb_blob {
                Some(blob) => ResolvedDtb::Blob(blob.to_vec()),
                // Nothing to pass through — fall back to generating.
                None => ResolvedDtb::Blob(generate_virt_dtb(platform, config)),
            }
        }
        DtbPolicy::Patch => {
            match user_dtb_blob {
                Some(blob) => ResolvedDtb::Blob(patch_dtb(blob, platform, config)),
                None => ResolvedDtb::Blob(generate_virt_dtb(platform, config)),
            }
        }
        DtbPolicy::Generate => {
            ResolvedDtb::Blob(generate_virt_dtb(platform, config))
        }
    }
}

// ── Device → FDT node mapping ───────────────────────────────────────────────

fn device_to_fdt_node(name: &str, base: u64, irq_num: &mut u32) -> Option<FdtNode> {
    let lower = name.to_lowercase();

    if lower.contains("uart") || lower.contains("pl011") {
        let irq = alloc_spi(irq_num);
        let mut node = FdtNode::new(format!("pl011@{base:x}"));
        node.add_prop("compatible", FdtValue::StringList(vec![
            "arm,pl011".to_string(), "arm,primecell".to_string(),
        ]));
        node.add_prop("reg", FdtValue::Reg(vec![(base, 0x1000)]));
        node.add_prop("interrupts", FdtValue::U32List(vec![0, irq, 4]));
        node.add_prop("clock-names", FdtValue::StringList(vec![
            "uartclk".to_string(), "apb_pclk".to_string(),
        ]));
        return Some(node);
    }
    if lower.contains("gic") { return None; }
    if lower.contains("rtc") || lower.contains("pl031") {
        let irq = alloc_spi(irq_num);
        let mut node = FdtNode::new(format!("pl031@{base:x}"));
        node.add_prop("compatible", FdtValue::StringList(vec![
            "arm,pl031".to_string(), "arm,primecell".to_string(),
        ]));
        node.add_prop("reg", FdtValue::Reg(vec![(base, 0x1000)]));
        node.add_prop("interrupts", FdtValue::U32List(vec![0, irq, 4]));
        return Some(node);
    }
    if lower.contains("timer") || lower.contains("sp804") {
        let irq = alloc_spi(irq_num);
        let mut node = FdtNode::new(format!("timer@{base:x}"));
        node.add_prop("compatible", FdtValue::StringList(vec![
            "arm,sp804".to_string(), "arm,primecell".to_string(),
        ]));
        node.add_prop("reg", FdtValue::Reg(vec![(base, 0x1000)]));
        node.add_prop("interrupts", FdtValue::U32List(vec![0, irq, 4]));
        return Some(node);
    }
    if lower.contains("gpio") || lower.contains("pl061") {
        let irq = alloc_spi(irq_num);
        let mut node = FdtNode::new(format!("gpio@{base:x}"));
        node.add_prop("compatible", FdtValue::StringList(vec![
            "arm,pl061".to_string(), "arm,primecell".to_string(),
        ]));
        node.add_prop("reg", FdtValue::Reg(vec![(base, 0x1000)]));
        node.add_prop("interrupts", FdtValue::U32List(vec![0, irq, 4]));
        node.add_prop("gpio-controller", FdtValue::Empty);
        node.add_prop("#gpio-cells", FdtValue::U32(2));
        return Some(node);
    }
    if lower.contains("watchdog") || lower.contains("sp805") {
        let irq = alloc_spi(irq_num);
        let mut node = FdtNode::new(format!("watchdog@{base:x}"));
        node.add_prop("compatible", FdtValue::StringList(vec![
            "arm,sp805".to_string(), "arm,primecell".to_string(),
        ]));
        node.add_prop("reg", FdtValue::Reg(vec![(base, 0x1000)]));
        node.add_prop("interrupts", FdtValue::U32List(vec![0, irq, 4]));
        return Some(node);
    }
    if lower.contains("sysregs") {
        let mut node = FdtNode::new(format!("sysregs@{base:x}"));
        node.add_prop("compatible", FdtValue::String("arm,realview-sysctl".to_string()));
        node.add_prop("reg", FdtValue::Reg(vec![(base, 0x1000)]));
        return Some(node);
    }
    if lower.contains("virtio") {
        let irq = alloc_spi(irq_num);
        let mut node = FdtNode::new(format!("virtio_mmio@{base:x}"));
        node.add_prop("compatible", FdtValue::String("virtio,mmio".to_string()));
        node.add_prop("reg", FdtValue::Reg(vec![(base, 0x200)]));
        node.add_prop("interrupts", FdtValue::U32List(vec![0, irq, 1]));
        node.add_prop("dma-coherent", FdtValue::Empty);
        return Some(node);
    }
    if lower.contains("apb") || lower.contains("ahb") { return None; }

    let mut node = FdtNode::new(format!("{name}@{base:x}"));
    node.add_prop("reg", FdtValue::Reg(vec![(base, 0x1000)]));
    Some(node)
}

fn device_spec_to_fdt_node(spec: &DeviceSpec, irq_num: &mut u32) -> Option<FdtNode> {
    let base = spec.get_u64("addr").or_else(|| spec.get_u64("base")).unwrap_or(0);
    let size = spec.get_u64("size").unwrap_or(0x1000);
    let compatible = spec.get("compatible")
        .map(|s| s.to_string())
        .unwrap_or_else(|| type_to_compatible(&spec.type_name));
    let ty = spec.type_name.as_str();

    match ty {
        "virtio-net-device" | "virtio-net" | "virtio-blk-device" | "virtio-blk"
        | "virtio-rng-device" | "virtio-rng" | "virtio-console-device" | "virtio-console"
        | "virtio-gpu-device" | "virtio-gpu" | "virtio-input-device" | "virtio-input"
        | "virtio-fs-device" | "virtio-fs" => {
            let irq = alloc_spi(irq_num);
            let mut node = FdtNode::new(format!("virtio_mmio@{base:x}"));
            node.add_prop("compatible", FdtValue::String("virtio,mmio".to_string()));
            node.add_prop("reg", FdtValue::Reg(vec![(base, 0x200)]));
            node.add_prop("interrupts", FdtValue::U32List(vec![0, irq, 1]));
            node.add_prop("dma-coherent", FdtValue::Empty);
            Some(node)
        }
        "pl011" | "uart" => {
            let irq = alloc_spi(irq_num);
            let mut node = FdtNode::new(format!("pl011@{base:x}"));
            node.add_prop("compatible", FdtValue::StringList(vec![
                "arm,pl011".to_string(), "arm,primecell".to_string(),
            ]));
            node.add_prop("reg", FdtValue::Reg(vec![(base, size)]));
            node.add_prop("interrupts", FdtValue::U32List(vec![0, irq, 4]));
            Some(node)
        }
        _ => {
            let irq = alloc_spi(irq_num);
            let mut node = FdtNode::new(format!("{ty}@{base:x}"));
            node.add_prop("compatible", FdtValue::String(compatible));
            node.add_prop("reg", FdtValue::Reg(vec![(base, size)]));
            node.add_prop("interrupts", FdtValue::U32List(vec![0, irq, 4]));
            for (k, v) in &spec.properties {
                if matches!(k.as_str(), "addr" | "base" | "size" | "compatible") {
                    continue;
                }
                node.add_prop(k.clone(), FdtValue::String(v.clone()));
            }
            Some(node)
        }
    }
}

fn type_to_compatible(type_name: &str) -> String {
    match type_name {
        "pl011" => "arm,pl011".to_string(),
        "sp804" => "arm,sp804".to_string(),
        "pl031" => "arm,pl031".to_string(),
        "pl061" => "arm,pl061".to_string(),
        "sp805" => "arm,sp805".to_string(),
        "gic" => "arm,cortex-a15-gic".to_string(),
        other => format!("helm,{other}"),
    }
}

fn alloc_spi(counter: &mut u32) -> u32 {
    let irq = *counter;
    *counter += 1;
    irq
}

// ── Strings table ───────────────────────────────────────────────────────────

struct StringTable {
    data: Vec<u8>,
    offsets: HashMap<String, u32>,
}

impl StringTable {
    fn new() -> Self {
        Self { data: Vec::new(), offsets: HashMap::new() }
    }
    fn intern(&mut self, s: &str) -> u32 {
        if let Some(&off) = self.offsets.get(s) { return off; }
        let off = self.data.len() as u32;
        self.data.extend_from_slice(s.as_bytes());
        self.data.push(0);
        self.offsets.insert(s.to_string(), off);
        off
    }
    fn into_bytes(self) -> Vec<u8> { self.data }
}

fn pad_to_4(buf: &mut Vec<u8>) {
    while buf.len() % 4 != 0 { buf.push(0); }
}

fn value_to_bytes(val: &FdtValue) -> Vec<u8> {
    match val {
        FdtValue::Empty => vec![],
        FdtValue::U32(v) => v.to_be_bytes().to_vec(),
        FdtValue::U64(v) => v.to_be_bytes().to_vec(),
        FdtValue::String(s) => { let mut b = s.as_bytes().to_vec(); b.push(0); b }
        FdtValue::StringList(list) => {
            let mut b = Vec::new();
            for s in list { b.extend_from_slice(s.as_bytes()); b.push(0); }
            b
        }
        FdtValue::Bytes(b) => b.clone(),
        FdtValue::Reg(pairs) => {
            let mut b = Vec::new();
            for &(a, s) in pairs { b.extend_from_slice(&a.to_be_bytes()); b.extend_from_slice(&s.to_be_bytes()); }
            b
        }
        FdtValue::U32List(list) => {
            let mut b = Vec::new();
            for &v in list { b.extend_from_slice(&v.to_be_bytes()); }
            b
        }
        FdtValue::Phandle(v) => v.to_be_bytes().to_vec(),
    }
}

/// Parse a human-readable RAM size string (e.g., "256M", "1G", "512K").
pub fn parse_ram_size(s: &str) -> Option<u64> {
    let s = s.trim();
    if s.is_empty() { return None; }
    let (num_str, mul) = if s.ends_with('G') || s.ends_with('g') {
        (&s[..s.len()-1], 1024*1024*1024u64)
    } else if s.ends_with('M') || s.ends_with('m') {
        (&s[..s.len()-1], 1024*1024u64)
    } else if s.ends_with('K') || s.ends_with('k') {
        (&s[..s.len()-1], 1024u64)
    } else { (s, 1u64) };
    num_str.parse::<u64>().ok().map(|n| n * mul)
}
