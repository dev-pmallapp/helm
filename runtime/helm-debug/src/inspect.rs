//! Inspection API — dump arch state, memory ranges, device state on demand.

use std::collections::BTreeMap;

/// A symbol visible to the current debug target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SymbolView {
    pub name: String,
    pub addr: u64,
    pub size: u64,
}

/// A lightweight structured device-state dump.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceView {
    pub name: String,
    pub fields: BTreeMap<String, String>,
}

/// A snapshot of inspectable state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InspectionResult {
    pub arch: Option<String>,
    pub int_regs: Vec<(String, u64)>,
    pub pc: u64,
    pub symbols: Vec<SymbolView>,
    pub devices: Vec<DeviceView>,
    pub extras: BTreeMap<String, String>,
}

impl InspectionResult {
    pub fn new(pc: u64) -> Self {
        Self {
            arch: None,
            int_regs: Vec::new(),
            pc,
            symbols: Vec::new(),
            devices: Vec::new(),
            extras: BTreeMap::new(),
        }
    }

    pub fn set_arch(&mut self, arch: impl Into<String>) {
        self.arch = Some(arch.into());
    }

    pub fn add_reg(&mut self, name: impl Into<String>, val: u64) {
        self.int_regs.push((name.into(), val));
    }

    pub fn add_symbol(&mut self, name: impl Into<String>, addr: u64, size: u64) {
        self.symbols.push(SymbolView {
            name: name.into(),
            addr,
            size,
        });
    }

    pub fn add_device_field(
        &mut self,
        device_name: impl Into<String>,
        field_name: impl Into<String>,
        value: impl Into<String>,
    ) {
        let device_name = device_name.into();
        if let Some(device) = self.devices.iter_mut().find(|device| device.name == device_name) {
            device.fields.insert(field_name.into(), value.into());
            return;
        }
        let mut fields = BTreeMap::new();
        fields.insert(field_name.into(), value.into());
        self.devices.push(DeviceView {
            name: device_name,
            fields,
        });
    }

    pub fn add_extra(&mut self, key: impl Into<String>, val: impl Into<String>) {
        self.extras.insert(key.into(), val.into());
    }

    pub fn to_text(&self) -> String {
        let mut out = String::new();
        if let Some(arch) = &self.arch {
            out.push_str(&format!("ARCH = {arch}\n"));
        }
        out.push_str(&format!("PC = {:#018x}\n", self.pc));
        for (name, val) in &self.int_regs {
            out.push_str(&format!("{name:>4} = {val:#018x}\n"));
        }
        for symbol in &self.symbols {
            out.push_str(&format!(
                "symbol {:#018x} {:#x} {}\n",
                symbol.addr, symbol.size, symbol.name
            ));
        }
        for device in &self.devices {
            out.push_str(&format!("[device:{}]\n", device.name));
            for (key, value) in &device.fields {
                out.push_str(&format!("{key}: {value}\n"));
            }
        }
        for (key, val) in &self.extras {
            out.push_str(&format!("{key}: {val}\n"));
        }
        out
    }
}

/// Trait for components that support inspection.
pub trait Inspectable {
    fn inspect(&self) -> InspectionResult;
    fn inspect_memory(&mut self, addr: u64, len: usize) -> Option<Vec<u8>>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formatting() {
        let mut r = InspectionResult::new(0x8000_0000);
        r.set_arch("aarch64");
        r.add_reg("x0", 42);
        r.add_symbol("_start", 0x8000_0000, 0x40);
        r.add_device_field("uart", "tx_count", "0");
        r.add_extra("NZCV", "0b0100");
        let text = r.to_text();
        assert!(text.contains("ARCH = aarch64"));
        assert!(text.contains("PC = 0x0000000080000000"));
        assert!(text.contains("symbol 0x0000000080000000 0x40 _start"));
        assert!(text.contains("[device:uart]"));
        assert!(text.contains("tx_count: 0"));
        assert!(text.contains("NZCV: 0b0100"));
    }
}
