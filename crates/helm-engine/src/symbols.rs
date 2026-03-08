//! Symbol table — maps names ↔ addresses for breakpoints and display.
//!
//! Two sources:
//! - **System.map** (FS mode): `addr type name` per line from Linux kernel build
//! - **ELF .symtab** (SE mode): symbol table section from the guest binary

use std::collections::BTreeMap;
use std::collections::HashMap;

/// Bidirectional symbol table for address ↔ name lookups.
pub struct SymbolTable {
    by_name: HashMap<String, u64>,
    by_addr: BTreeMap<u64, String>,
}

impl SymbolTable {
    pub fn new() -> Self {
        Self {
            by_name: HashMap::new(),
            by_addr: BTreeMap::new(),
        }
    }

    /// Parse a Linux `System.map` file.
    ///
    /// Format: `<hex_addr> <type> <name>` per line.
    /// Example: `ffffffc080080000 T _stext`
    pub fn from_system_map(path: &str) -> std::io::Result<Self> {
        let content = std::fs::read_to_string(path)?;
        let mut table = Self::new();
        for line in content.lines() {
            let parts: Vec<&str> = line.splitn(3, ' ').collect();
            if parts.len() < 3 {
                continue;
            }
            if let Ok(addr) = u64::from_str_radix(parts[0], 16) {
                let name = parts[2].to_string();
                table.by_name.insert(name.clone(), addr);
                table.by_addr.insert(addr, name);
            }
        }
        Ok(table)
    }

    /// Extract symbols from ELF binary data (.symtab section).
    pub fn from_elf(data: &[u8]) -> Self {
        let mut table = Self::new();

        if data.len() < 64 {
            return table;
        }

        // Verify ELF magic
        if &data[0..4] != b"\x7fELF" {
            return table;
        }

        let is_64 = data[4] == 2;
        let is_le = data[5] == 1;
        if !is_64 || !is_le {
            return table; // only support 64-bit LE
        }

        let e_shoff = u64::from_le_bytes(data[0x28..0x30].try_into().unwrap_or([0; 8])) as usize;
        let e_shentsize = u16::from_le_bytes(data[0x3A..0x3C].try_into().unwrap_or([0; 2])) as usize;
        let e_shnum = u16::from_le_bytes(data[0x3C..0x3E].try_into().unwrap_or([0; 2])) as usize;
        let e_shstrndx = u16::from_le_bytes(data[0x3E..0x40].try_into().unwrap_or([0; 2])) as usize;

        if e_shoff == 0 || e_shentsize < 64 || e_shnum == 0 {
            return table;
        }

        // Find .symtab and .strtab sections
        let mut symtab_off = 0usize;
        let mut symtab_size = 0usize;
        let mut symtab_link = 0usize; // index of associated string table
        let mut strtab_off = 0usize;
        let mut strtab_size = 0usize;

        for i in 0..e_shnum {
            let sh = e_shoff + i * e_shentsize;
            if sh + e_shentsize > data.len() {
                break;
            }
            let sh_type = u32::from_le_bytes(data[sh + 4..sh + 8].try_into().unwrap_or([0; 4]));
            let sh_offset = u64::from_le_bytes(data[sh + 24..sh + 32].try_into().unwrap_or([0; 8])) as usize;
            let sh_size = u64::from_le_bytes(data[sh + 32..sh + 40].try_into().unwrap_or([0; 8])) as usize;
            let sh_link = u32::from_le_bytes(data[sh + 40..sh + 44].try_into().unwrap_or([0; 4])) as usize;

            // SHT_SYMTAB = 2
            if sh_type == 2 && symtab_off == 0 {
                symtab_off = sh_offset;
                symtab_size = sh_size;
                symtab_link = sh_link;
            }
        }

        if symtab_off == 0 || symtab_link >= e_shnum {
            return table;
        }

        // Get the linked string table
        let strtab_sh = e_shoff + symtab_link * e_shentsize;
        if strtab_sh + e_shentsize <= data.len() {
            strtab_off = u64::from_le_bytes(data[strtab_sh + 24..strtab_sh + 32].try_into().unwrap_or([0; 8])) as usize;
            strtab_size = u64::from_le_bytes(data[strtab_sh + 32..strtab_sh + 40].try_into().unwrap_or([0; 8])) as usize;
        }

        if strtab_off == 0 {
            return table;
        }

        // Parse symbol entries (Elf64_Sym = 24 bytes)
        const SYM_SIZE: usize = 24;
        let sym_count = symtab_size / SYM_SIZE;
        for i in 0..sym_count {
            let off = symtab_off + i * SYM_SIZE;
            if off + SYM_SIZE > data.len() {
                break;
            }
            let st_name = u32::from_le_bytes(data[off..off + 4].try_into().unwrap_or([0; 4])) as usize;
            let st_info = data[off + 4];
            let st_value = u64::from_le_bytes(data[off + 8..off + 16].try_into().unwrap_or([0; 8]));

            // Only include FUNC and OBJECT symbols with nonzero address
            let st_type = st_info & 0xf;
            if st_value == 0 || (st_type != 1 && st_type != 2) {
                // 1=FUNC, 2=OBJECT
                continue;
            }

            // Read name from string table
            let name_start = strtab_off + st_name;
            if name_start >= data.len() || name_start >= strtab_off + strtab_size {
                continue;
            }
            let name_end = data[name_start..]
                .iter()
                .position(|&b| b == 0)
                .map(|p| name_start + p)
                .unwrap_or(data.len());
            let name = String::from_utf8_lossy(&data[name_start..name_end]).to_string();
            if name.is_empty() {
                continue;
            }

            table.by_name.insert(name.clone(), st_value);
            table.by_addr.insert(st_value, name);
        }

        table
    }

    /// Insert a symbol manually.
    pub fn insert(&mut self, name: String, addr: u64) {
        self.by_name.insert(name.clone(), addr);
        self.by_addr.insert(addr, name);
    }

    /// Look up an address by symbol name.
    pub fn lookup(&self, name: &str) -> Option<u64> {
        self.by_name.get(name).copied()
    }

    /// Resolve an address to the nearest symbol (name + offset).
    pub fn resolve(&self, addr: u64) -> Option<(&str, u64)> {
        // Find the greatest key ≤ addr
        self.by_addr
            .range(..=addr)
            .next_back()
            .map(|(sym_addr, name)| (name.as_str(), addr - sym_addr))
    }

    /// Number of symbols loaded.
    pub fn len(&self) -> usize {
        self.by_name.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_name.is_empty()
    }
}

impl Default for SymbolTable {
    fn default() -> Self {
        Self::new()
    }
}

/// Try to find a System.map file alongside a kernel image.
///
/// Given `/path/to/vmlinuz-rpi`, looks for `/path/to/System.map-6.12.67-0-rpi`
/// or similar patterns.
pub fn find_system_map(kernel_path: &str) -> Option<String> {
    let path = std::path::Path::new(kernel_path);
    let dir = path.parent()?;
    let stem = path.file_name()?.to_str()?;

    // Try vmlinuz-XXX → System.map-XXX
    if let Some(suffix) = stem.strip_prefix("vmlinuz-") {
        let candidate = dir.join(format!("System.map-{suffix}"));
        if candidate.exists() {
            return Some(candidate.to_string_lossy().into_owned());
        }
    }

    // Try vmlinuz → System.map (same directory)
    let candidate = dir.join("System.map");
    if candidate.exists() {
        return Some(candidate.to_string_lossy().into_owned());
    }

    // Scan for any System.map* in the directory
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            if name.to_str().map_or(false, |n| n.starts_with("System.map")) {
                return Some(entry.path().to_string_lossy().into_owned());
            }
        }
    }

    None
}
