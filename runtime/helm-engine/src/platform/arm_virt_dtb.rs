use std::collections::HashMap;

use crate::platform::arm_virt::ArmVirtGicVersion;
use helm_platform::aarch64::virt::{
    FLASH_BASE, FLASH_SIZE, FW_CFG_BASE, GICC_BASE, GICC_REGION_SIZE, GICD_BASE, GICD_REGION_SIZE,
    GICR_BASE, GICR_STRIDE, GPIO_BASE, GPIO_IRQ, PCIE_BUS_COUNT, PCIE_ECAM_BASE, PCIE_ECAM_SIZE,
    PCIE_INTX_IRQ_BASE, PCIE_INTX_IRQ_COUNT, PCIE_MMIO_BASE, PCIE_MMIO_SIZE, PCIE_PIO_BASE,
    PCIE_PIO_SIZE, RAM_BASE, RTC_BASE, SECURE_UART_BASE, SECURE_UART_IRQ, UART_BASE,
};

const FDT_MAGIC: u32 = 0xD00D_FEED;
const FDT_BEGIN_NODE: u32 = 1;
const FDT_END_NODE: u32 = 2;
const FDT_PROP: u32 = 3;
const FDT_END: u32 = 9;
const FDT_VERSION: u32 = 17;
const FDT_LAST_COMP_VERSION: u32 = 16;
const APB_CLOCK_PHANDLE: u32 = 1;
const GIC_PHANDLE: u32 = 2;
const GPIO_PHANDLE: u32 = 3;
const INITRD_OFFSET: u64 = 0x0400_0000;
const FDT_PCI_RANGE_IOPORT: u32 = 0x0100_0000;
const FDT_PCI_RANGE_MMIO: u32 = 0x0200_0000;
const GIC_FDT_IRQ_TYPE_SPI: u32 = 0;
const GIC_FDT_IRQ_FLAGS_LEVEL_HI: u32 = 4;

fn push_be32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_be_bytes());
}

fn pci_devfn(slot: u32, function: u32) -> u32 {
    (slot << 3) | function
}

fn arm_virt_pcie_interrupt_map() -> Vec<u8> {
    let mut out = Vec::with_capacity(4 * PCIE_INTX_IRQ_COUNT as usize * 10 * 4);
    for devfn in (0u32..=0x18).step_by(0x8) {
        let slot = devfn >> 3;
        for pin in 0..PCIE_INTX_IRQ_COUNT {
            let irq_nr = (PCIE_INTX_IRQ_BASE - 32) + ((pin + slot) % PCIE_INTX_IRQ_COUNT);
            let cells = [
                devfn << 8,
                0,
                0,
                pin + 1,
                GIC_PHANDLE,
                0,
                0,
                GIC_FDT_IRQ_TYPE_SPI,
                irq_nr,
                GIC_FDT_IRQ_FLAGS_LEVEL_HI,
            ];
            for cell in cells {
                push_be32(&mut out, cell);
            }
        }
    }
    out
}

struct FdtWriter {
    struct_block: Vec<u8>,
    strings: Vec<u8>,
    string_offsets: HashMap<String, u32>,
}

impl FdtWriter {
    fn new() -> Self {
        Self {
            struct_block: Vec::new(),
            strings: Vec::new(),
            string_offsets: HashMap::new(),
        }
    }

    fn intern_name(&mut self, name: &str) -> u32 {
        if let Some(offset) = self.string_offsets.get(name) {
            return *offset;
        }
        let offset = self.strings.len() as u32;
        self.strings.extend_from_slice(name.as_bytes());
        self.strings.push(0);
        self.string_offsets.insert(name.to_string(), offset);
        offset
    }

    fn begin_node(&mut self, name: &str) {
        push_be32(&mut self.struct_block, FDT_BEGIN_NODE);
        self.struct_block.extend_from_slice(name.as_bytes());
        self.struct_block.push(0);
        while self.struct_block.len() % 4 != 0 {
            self.struct_block.push(0);
        }
    }

    fn end_node(&mut self) {
        push_be32(&mut self.struct_block, FDT_END_NODE);
    }

    fn property(&mut self, name: &str, value: &[u8]) {
        push_be32(&mut self.struct_block, FDT_PROP);
        push_be32(&mut self.struct_block, value.len() as u32);
        let nameoff = self.intern_name(name);
        push_be32(&mut self.struct_block, nameoff);
        self.struct_block.extend_from_slice(value);
        while self.struct_block.len() % 4 != 0 {
            self.struct_block.push(0);
        }
    }

    fn property_str(&mut self, name: &str, value: &str) {
        let mut bytes = value.as_bytes().to_vec();
        bytes.push(0);
        self.property(name, &bytes);
    }

    fn property_strlist(&mut self, name: &str, values: &[&str]) {
        let mut bytes = Vec::new();
        for value in values {
            bytes.extend_from_slice(value.as_bytes());
            bytes.push(0);
        }
        self.property(name, &bytes);
    }

    fn property_empty(&mut self, name: &str) {
        self.property(name, &[]);
    }

    fn property_cells(&mut self, name: &str, cells: &[u32]) {
        let mut bytes = Vec::with_capacity(cells.len() * 4);
        for cell in cells {
            bytes.extend_from_slice(&cell.to_be_bytes());
        }
        self.property(name, &bytes);
    }

    fn finish(mut self) -> Vec<u8> {
        push_be32(&mut self.struct_block, FDT_END);

        let header_size = 40usize;
        let reserve_size = 16usize;
        let off_mem_rsvmap = header_size as u32;
        let off_dt_struct = (header_size + reserve_size) as u32;
        let off_dt_strings = off_dt_struct + self.struct_block.len() as u32;
        let totalsize = off_dt_strings + self.strings.len() as u32;

        let mut out = Vec::with_capacity(totalsize as usize);
        push_be32(&mut out, FDT_MAGIC);
        push_be32(&mut out, totalsize);
        push_be32(&mut out, off_dt_struct);
        push_be32(&mut out, off_dt_strings);
        push_be32(&mut out, off_mem_rsvmap);
        push_be32(&mut out, FDT_VERSION);
        push_be32(&mut out, FDT_LAST_COMP_VERSION);
        push_be32(&mut out, 0);
        push_be32(&mut out, self.strings.len() as u32);
        push_be32(&mut out, self.struct_block.len() as u32);
        out.extend_from_slice(&[0u8; 16]);
        out.append(&mut self.struct_block);
        out.append(&mut self.strings);
        out
    }
}

pub(crate) fn build_baseline_arm_virt_dtb(
    mem_mib: usize,
    num_cpus: usize,
    gic_version: ArmVirtGicVersion,
    bootargs: &str,
    initrd_size: Option<u64>,
    include_rtc: bool,
    include_second_uart: bool,
) -> Vec<u8> {
    let cpu_count = num_cpus.max(1);
    let mem_size = (mem_mib as u64) * 1024 * 1024;
    let timer_irq_flags = 4u32 | (((1u32 << cpu_count.min(8)) - 1) << 8);
    let mut fdt = FdtWriter::new();

    fdt.begin_node("");
    fdt.property_str("compatible", "linux,dummy-virt");
    fdt.property_cells("#address-cells", &[2]);
    fdt.property_cells("#size-cells", &[2]);
    fdt.property_cells("interrupt-parent", &[GIC_PHANDLE]);

    fdt.begin_node("chosen");
    fdt.property_str("stdout-path", &format!("/pl011@{UART_BASE:x}"));
    fdt.property_str("bootargs", bootargs);
    if let Some(size) = initrd_size {
        let start = RAM_BASE + INITRD_OFFSET;
        let end = start + size;
        fdt.property_cells("linux,initrd-start", &[(start >> 32) as u32, start as u32]);
        fdt.property_cells("linux,initrd-end", &[(end >> 32) as u32, end as u32]);
    }
    fdt.end_node();

    fdt.begin_node("aliases");
    fdt.property_str("serial0", &format!("/pl011@{UART_BASE:x}"));
    if include_second_uart {
        fdt.property_str("serial1", &format!("/pl011@{SECURE_UART_BASE:x}"));
    }
    fdt.end_node();

    fdt.begin_node("cpus");
    fdt.property_cells("#address-cells", &[2]);
    fdt.property_cells("#size-cells", &[0]);
    for cpu_idx in 0..cpu_count {
        fdt.begin_node(&format!("cpu@{cpu_idx:x}"));
        fdt.property_str("device_type", "cpu");
        fdt.property_str("compatible", "arm,cortex-a53");
        fdt.property_cells("reg", &[0, cpu_idx as u32]);
        fdt.property_str("enable-method", "psci");
        fdt.end_node();
    }
    fdt.end_node();

    fdt.begin_node(&format!("memory@{RAM_BASE:x}"));
    fdt.property_str("device_type", "memory");
    fdt.property_cells("reg", &[0, RAM_BASE as u32, 0, mem_size as u32]);
    fdt.end_node();

    fdt.begin_node("psci");
    fdt.property_str("compatible", "arm,psci-0.2");
    fdt.property_str("method", "smc");
    fdt.end_node();

    fdt.begin_node("timer");
    fdt.property_str("compatible", "arm,armv8-timer");
    fdt.property_cells(
        "interrupts",
        &[
            1,
            13,
            timer_irq_flags,
            1,
            14,
            timer_irq_flags,
            1,
            11,
            timer_irq_flags,
            1,
            10,
            timer_irq_flags,
        ],
    );
    fdt.property_empty("always-on");
    fdt.end_node();

    fdt.begin_node("apb-pclk");
    fdt.property_str("compatible", "fixed-clock");
    fdt.property_cells("#clock-cells", &[0]);
    fdt.property_cells("clock-frequency", &[24_000_000]);
    fdt.property_str("clock-output-names", "clk24mhz");
    fdt.property_cells("phandle", &[APB_CLOCK_PHANDLE]);
    fdt.end_node();

    fdt.begin_node(&format!("interrupt-controller@{GICD_BASE:x}"));
    fdt.property_cells("#address-cells", &[2]);
    fdt.property_cells("#size-cells", &[2]);
    fdt.property_empty("ranges");
    match gic_version {
        ArmVirtGicVersion::V2 => {
            fdt.property_str("compatible", "arm,cortex-a15-gic");
            fdt.property_cells(
                "reg",
                &[
                    0,
                    GICD_BASE as u32,
                    0,
                    GICD_REGION_SIZE as u32,
                    0,
                    GICC_BASE as u32,
                    0,
                    GICC_REGION_SIZE as u32,
                ],
            );
        }
        ArmVirtGicVersion::V3 => {
            fdt.property_str("compatible", "arm,gic-v3");
            fdt.property_cells("#redistributor-regions", &[1]);
            fdt.property_cells(
                "reg",
                &[
                    0,
                    GICD_BASE as u32,
                    0,
                    GICD_REGION_SIZE as u32,
                    0,
                    GICR_BASE as u32,
                    0,
                    (cpu_count as u32) * GICR_STRIDE as u32,
                ],
            );
        }
    }
    fdt.property_cells("#interrupt-cells", &[3]);
    fdt.property_empty("interrupt-controller");
    fdt.property_cells("phandle", &[GIC_PHANDLE]);
    fdt.end_node();

    fdt.begin_node(&format!("pl011@{UART_BASE:x}"));
    fdt.property_strlist("compatible", &["arm,pl011", "arm,primecell"]);
    fdt.property_cells("reg", &[0, UART_BASE as u32, 0, 0x1000]);
    fdt.property_cells("interrupts", &[0, 1, 4]);
    fdt.property_cells("clocks", &[APB_CLOCK_PHANDLE, APB_CLOCK_PHANDLE]);
    fdt.property_strlist("clock-names", &["uartclk", "apb_pclk"]);
    fdt.end_node();

    if include_rtc {
        fdt.begin_node(&format!("pl031@{RTC_BASE:x}"));
        fdt.property_strlist("compatible", &["arm,pl031", "arm,primecell"]);
        fdt.property_cells("reg", &[0, RTC_BASE as u32, 0, 0x1000]);
        fdt.property_cells("interrupts", &[0, 2, 4]);
        fdt.property_cells("clocks", &[APB_CLOCK_PHANDLE]);
        fdt.property_str("clock-names", "apb_pclk");
        fdt.end_node();
    }

    fdt.begin_node(&format!("fw-cfg@{FW_CFG_BASE:x}"));
    fdt.property_str("compatible", "qemu,fw-cfg-mmio");
    fdt.property_cells("reg", &[0, FW_CFG_BASE as u32, 0, 0x18]);
    fdt.end_node();

    fdt.begin_node(&format!("pl061@{GPIO_BASE:x}"));
    fdt.property_strlist("compatible", &["arm,pl061", "arm,primecell"]);
    fdt.property_cells("reg", &[0, GPIO_BASE as u32, 0, 0x1000]);
    fdt.property_cells("interrupts", &[0, GPIO_IRQ - 32, 4]);
    fdt.property_cells("#gpio-cells", &[2]);
    fdt.property_empty("gpio-controller");
    fdt.property_cells("clocks", &[APB_CLOCK_PHANDLE]);
    fdt.property_str("clock-names", "apb_pclk");
    fdt.property_cells("phandle", &[GPIO_PHANDLE]);
    fdt.property_str("status", "disabled");
    fdt.end_node();

    fdt.begin_node("gpio-poweroff");
    fdt.property_str("compatible", "gpio-poweroff");
    fdt.property_cells("gpios", &[GPIO_PHANDLE, 0, 0]);
    fdt.property_str("status", "disabled");
    fdt.end_node();

    fdt.begin_node("gpio-restart");
    fdt.property_str("compatible", "gpio-restart");
    fdt.property_cells("gpios", &[GPIO_PHANDLE, 1, 0]);
    fdt.property_str("status", "disabled");
    fdt.end_node();

    fdt.begin_node(&format!("pl011@{SECURE_UART_BASE:x}"));
    fdt.property_strlist("compatible", &["arm,pl011", "arm,primecell"]);
    fdt.property_cells("reg", &[0, SECURE_UART_BASE as u32, 0, 0x1000]);
    fdt.property_cells("interrupts", &[0, SECURE_UART_IRQ - 32, 4]);
    fdt.property_cells("clocks", &[APB_CLOCK_PHANDLE, APB_CLOCK_PHANDLE]);
    fdt.property_strlist("clock-names", &["uartclk", "apb_pclk"]);
    if !include_second_uart {
        fdt.property_str("status", "disabled");
    }
    fdt.end_node();

    fdt.begin_node(&format!("flash@{FLASH_BASE:x}"));
    fdt.property_str("compatible", "cfi-flash");
    fdt.property_cells("reg", &[0, FLASH_BASE as u32, 0, FLASH_SIZE as u32]);
    fdt.property_str("status", "disabled");
    fdt.end_node();

    fdt.begin_node(&format!("pcie@{PCIE_ECAM_BASE:x}"));
    fdt.property_str("compatible", "pci-host-ecam-generic");
    fdt.property_str("device_type", "pci");
    fdt.property_cells("#address-cells", &[3]);
    fdt.property_cells("#size-cells", &[2]);
    fdt.property_cells("linux,pci-domain", &[0]);
    fdt.property_cells("bus-range", &[0, PCIE_BUS_COUNT - 1]);
    fdt.property_empty("dma-coherent");
    fdt.property_cells("reg", &[0, PCIE_ECAM_BASE as u32, 0, PCIE_ECAM_SIZE as u32]);
    fdt.property_cells(
        "ranges",
        &[
            FDT_PCI_RANGE_IOPORT,
            0,
            0,
            0,
            PCIE_PIO_BASE as u32,
            0,
            PCIE_PIO_SIZE as u32,
            FDT_PCI_RANGE_MMIO,
            0,
            PCIE_MMIO_BASE as u32,
            0,
            PCIE_MMIO_BASE as u32,
            0,
            PCIE_MMIO_SIZE as u32,
        ],
    );
    fdt.property_cells("#interrupt-cells", &[1]);
    fdt.property("interrupt-map", &arm_virt_pcie_interrupt_map());
    fdt.property_cells("interrupt-map-mask", &[(pci_devfn(3, 0)) << 8, 0, 0, 0x7]);
    fdt.end_node();

    fdt.end_node();
    fdt.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn contains_subslice(haystack: &[u8], needle: &[u8]) -> bool {
        haystack
            .windows(needle.len())
            .any(|window| window == needle)
    }

    fn be_cells(cells: &[u32]) -> Vec<u8> {
        let mut out = Vec::with_capacity(cells.len() * 4);
        for cell in cells {
            out.extend_from_slice(&cell.to_be_bytes());
        }
        out
    }

    #[test]
    fn baseline_dtb_has_fdt_magic_and_expected_strings() {
        let dtb = build_baseline_arm_virt_dtb(
            512,
            2,
            ArmVirtGicVersion::V3,
            "console=ttyAMA0",
            None,
            true,
            false,
        );
        assert_eq!(u32::from_be_bytes(dtb[0..4].try_into().unwrap()), FDT_MAGIC);
        let as_text = String::from_utf8_lossy(&dtb);
        assert!(as_text.contains("arm,armv8-timer"));
        assert!(as_text.contains("arm,pl011"));
        assert!(as_text.contains("arm,pl031"));
        assert!(as_text.contains("qemu,fw-cfg-mmio"));
        assert!(as_text.contains("cfi-flash"));
        assert!(as_text.contains("bootargs"));
        assert!(as_text.contains("arm,gic-v3"));
        assert!(as_text.contains("pci-host-ecam-generic"));
    }

    #[test]
    fn baseline_dtb_uses_gicv2_specific_reg_layout() {
        let dtb = build_baseline_arm_virt_dtb(
            512,
            1,
            ArmVirtGicVersion::V2,
            "console=ttyAMA0",
            None,
            true,
            false,
        );

        let as_text = String::from_utf8_lossy(&dtb);
        assert!(as_text.contains("arm,cortex-a15-gic"));
        assert!(!as_text.contains("arm,gic-v3"));
        assert!(contains_subslice(
            &dtb,
            &be_cells(&[
                0,
                GICD_BASE as u32,
                0,
                GICD_REGION_SIZE as u32,
                0,
                GICC_BASE as u32,
                0,
                GICC_REGION_SIZE as u32,
            ]),
        ));
    }

    #[test]
    fn baseline_dtb_uses_gicv3_specific_reg_layout() {
        let dtb = build_baseline_arm_virt_dtb(
            512,
            3,
            ArmVirtGicVersion::V3,
            "console=ttyAMA0",
            None,
            true,
            false,
        );

        let as_text = String::from_utf8_lossy(&dtb);
        assert!(as_text.contains("arm,gic-v3"));
        assert!(as_text.contains("#redistributor-regions"));
        assert!(contains_subslice(
            &dtb,
            &be_cells(&[
                0,
                GICD_BASE as u32,
                0,
                GICD_REGION_SIZE as u32,
                0,
                GICR_BASE as u32,
                0,
                3u32 * GICR_STRIDE as u32,
            ]),
        ));
    }

    #[test]
    fn baseline_dtb_describes_pcie_host_bridge_windows_and_irq_map() {
        let dtb = build_baseline_arm_virt_dtb(
            512,
            2,
            ArmVirtGicVersion::V3,
            "console=ttyAMA0",
            None,
            true,
            false,
        );

        assert!(contains_subslice(
            &dtb,
            &be_cells(&[0, PCIE_ECAM_BASE as u32, 0, PCIE_ECAM_SIZE as u32]),
        ));
        assert!(contains_subslice(
            &dtb,
            &be_cells(&[
                FDT_PCI_RANGE_IOPORT,
                0,
                0,
                0,
                PCIE_PIO_BASE as u32,
                0,
                PCIE_PIO_SIZE as u32,
                FDT_PCI_RANGE_MMIO,
                0,
                PCIE_MMIO_BASE as u32,
                0,
                PCIE_MMIO_BASE as u32,
                0,
                PCIE_MMIO_SIZE as u32,
            ]),
        ));
        assert!(contains_subslice(
            &dtb,
            &be_cells(&[(pci_devfn(3, 0)) << 8, 0, 0, 0x7]),
        ));
        let irq_map = arm_virt_pcie_interrupt_map();
        assert!(contains_subslice(&dtb, &irq_map));
    }

    #[test]
    fn baseline_dtb_describes_disabled_gpio_and_second_uart_by_default() {
        let dtb = build_baseline_arm_virt_dtb(
            512,
            2,
            ArmVirtGicVersion::V3,
            "console=ttyAMA0",
            None,
            true,
            false,
        );

        let as_text = String::from_utf8_lossy(&dtb);
        assert!(as_text.contains("arm,pl061"));
        assert!(as_text.contains("gpio-poweroff"));
        assert!(as_text.contains("gpio-restart"));
        assert!(as_text.contains(&format!("pl011@{SECURE_UART_BASE:x}")));
        assert!(!as_text.contains("serial1"));
    }

    #[test]
    fn baseline_dtb_can_enable_second_uart_alias() {
        let dtb = build_baseline_arm_virt_dtb(
            512,
            2,
            ArmVirtGicVersion::V3,
            "console=ttyAMA0",
            None,
            true,
            true,
        );

        let as_text = String::from_utf8_lossy(&dtb);
        assert!(as_text.contains("serial1"));
        assert!(as_text.contains(&format!("pl011@{SECURE_UART_BASE:x}")));
    }
}
