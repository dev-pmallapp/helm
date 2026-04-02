//! Device topology tree — printable device map.
//!
//! [`DeviceTopology`] builds a tree of [`DeviceNode`]s that can be
//! pretty-printed for debugging or exposed to Python.

use std::fmt;

/// A tree of devices in the system.
#[derive(Debug, Clone)]
pub struct DeviceTopology {
    /// Root node (typically the system bus or `SoC`).
    pub root: DeviceNode,
}

/// A single device in the topology tree.
#[derive(Debug, Clone)]
pub struct DeviceNode {
    /// Device instance name (e.g. "uart0", "gic-dist").
    pub name: String,
    /// Device type/class (e.g. "PL011", "`GICv2Distributor`").
    pub device_type: String,
    /// Base MMIO address, if memory-mapped.
    pub base: Option<u64>,
    /// Region size in bytes, if memory-mapped.
    pub size: Option<u64>,
    /// IRQ number, if wired to an interrupt controller.
    pub irq: Option<u32>,
    /// Child devices (sub-buses, peripherals behind a bridge, etc.).
    pub children: Vec<DeviceNode>,
}

impl DeviceTopology {
    /// Create a new topology with the given root node.
    pub fn new(root: DeviceNode) -> Self {
        Self { root }
    }

    /// Render the topology as a tree-formatted string.
    pub fn print(&self) -> String {
        let mut out = String::new();
        print_node(&self.root, &mut out, "", true);
        out
    }
}

impl DeviceNode {
    /// Create a new device node.
    pub fn new(name: impl Into<String>, device_type: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            device_type: device_type.into(),
            base: None,
            size: None,
            irq: None,
            children: Vec::new(),
        }
    }

    /// Builder: set base address.
    #[must_use]
    pub fn with_base(mut self, base: u64) -> Self {
        self.base = Some(base);
        self
    }

    /// Builder: set region size.
    #[must_use]
    pub fn with_size(mut self, size: u64) -> Self {
        self.size = Some(size);
        self
    }

    /// Builder: set IRQ number.
    #[must_use]
    pub fn with_irq(mut self, irq: u32) -> Self {
        self.irq = Some(irq);
        self
    }

    /// Builder: add a child node.
    #[must_use]
    pub fn with_child(mut self, child: DeviceNode) -> Self {
        self.children.push(child);
        self
    }
}

fn print_node(node: &DeviceNode, out: &mut String, prefix: &str, is_last: bool) {
    use std::fmt::Write;

    let connector = if prefix.is_empty() {
        ""
    } else if is_last {
        "└── "
    } else {
        "├── "
    };

    out.push_str(prefix);
    out.push_str(connector);
    out.push_str(&node.name);
    out.push_str(" [");
    out.push_str(&node.device_type);
    out.push(']');

    if let Some(base) = node.base {
        let _ = write!(out, " @ {base:#010x}");
    }
    if let Some(size) = node.size {
        let _ = write!(out, " ({size:#x} bytes)");
    }
    if let Some(irq) = node.irq {
        let _ = write!(out, " IRQ {irq}");
    }
    out.push('\n');

    let child_prefix = if prefix.is_empty() {
        String::new()
    } else if is_last {
        format!("{prefix}    ")
    } else {
        format!("{prefix}│   ")
    };

    for (i, child) in node.children.iter().enumerate() {
        let last = i == node.children.len() - 1;
        print_node(child, out, &child_prefix, last);
    }
}

impl fmt::Display for DeviceTopology {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.print())
    }
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn topology_print_basic() {
        let topo = DeviceTopology::new(
            DeviceNode::new("soc", "ArmVirt")
                .with_child(
                    DeviceNode::new("gic-dist", "GICv2Distributor")
                        .with_base(0x0800_0000)
                        .with_size(0x1000),
                )
                .with_child(
                    DeviceNode::new("gic-cpu", "GICv2CpuInterface")
                        .with_base(0x0801_0000)
                        .with_size(0x1000),
                )
                .with_child(
                    DeviceNode::new("uart0", "PL011")
                        .with_base(0x0900_0000)
                        .with_size(0x1000)
                        .with_irq(33),
                ),
        );

        let output = topo.print();
        assert!(output.contains("soc [ArmVirt]"));
        assert!(output.contains("gic-dist [GICv2Distributor]"));
        assert!(output.contains("uart0 [PL011]"));
        assert!(output.contains("0x09000000"));
        assert!(output.contains("IRQ 33"));
    }

    #[test]
    fn empty_topology() {
        let topo = DeviceTopology::new(DeviceNode::new("soc", "Empty"));
        let output = topo.print();
        assert!(output.contains("soc [Empty]"));
    }
}
