//! In-kernel GIC (Generic Interrupt Controller) setup.
//!
//! Uses `KVM_CREATE_DEVICE` to instantiate a GICv2 or GICv3 inside
//! KVM.  Once created, IRQs are injected via [`irq::irq_line`](crate::irq::irq_line)
//! and the GIC state is maintained entirely in the kernel — no
//! user-space MMIO traps for GIC distributor or CPU-interface accesses.

use crate::error::{KvmError, Result};
use crate::kvm_sys::{self, kvm_create_device, kvm_device_attr};
use std::os::unix::io::RawFd;

/// Which GIC version to use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GicVersion {
    /// GICv2 — simpler, single-cluster, up to 8 CPUs.
    V2,
    /// GICv3 — scalable, affinity routing, required for > 8 CPUs.
    V3,
}

/// Configuration for the in-kernel GIC.
#[derive(Debug, Clone)]
pub struct GicConfig {
    /// GIC version.
    pub version: GicVersion,
    /// Number of SPIs to support (rounded up to multiple of 32).
    pub num_irqs: u32,
    /// GIC distributor base address in guest physical memory.
    pub dist_addr: u64,
    /// GICv2 CPU interface base address, or GICv3 redistributor base.
    pub cpu_or_redist_addr: u64,
    /// Number of vCPUs (needed for GICv3 redistributor sizing).
    pub num_cpus: u32,
}

impl Default for GicConfig {
    fn default() -> Self {
        Self {
            version: GicVersion::V3,
            num_irqs: 128,
            dist_addr: 0x0800_0000,
            cpu_or_redist_addr: 0x080A_0000,
            num_cpus: 1,
        }
    }
}

/// An in-kernel GIC instance.
///
/// The GIC device fd is kept open for the lifetime of the VM.
/// Dropping this struct closes the device fd.
pub struct KvmGic {
    device_fd: RawFd,
    config: GicConfig,
}

impl KvmGic {
    /// Create and initialise an in-kernel GIC.
    ///
    /// Must be called **after** all vCPUs are created but **before**
    /// the first `KVM_RUN`.
    pub fn new(vm_fd: RawFd, config: GicConfig) -> Result<Self> {
        let dev_type = match config.version {
            GicVersion::V2 => kvm_sys::KVM_DEV_TYPE_ARM_VGIC_V2,
            GicVersion::V3 => kvm_sys::KVM_DEV_TYPE_ARM_VGIC_V3,
        };

        let mut create = kvm_create_device {
            type_: dev_type,
            fd: 0,
            flags: 0,
        };

        unsafe {
            kvm_sys::kvm_ioctl(
                vm_fd,
                kvm_sys::KVM_CREATE_DEVICE,
                &mut create as *mut _ as u64,
            )
        }
        .map_err(|e| KvmError::Ioctl {
            name: "KVM_CREATE_DEVICE",
            source: e,
        })?;

        let device_fd = create.fd as RawFd;

        let gic = Self {
            device_fd,
            config: config.clone(),
        };

        gic.set_nr_irqs(config.num_irqs)?;
        gic.set_addresses(&config)?;
        gic.init_device()?;

        log::info!(
            "KVM GIC{:?} created: dist={:#x}, {}_addr={:#x}, irqs={}",
            config.version,
            config.dist_addr,
            match config.version {
                GicVersion::V2 => "cpu",
                GicVersion::V3 => "redist",
            },
            config.cpu_or_redist_addr,
            config.num_irqs,
        );

        Ok(gic)
    }

    /// Set the number of supported IRQs.
    fn set_nr_irqs(&self, num_irqs: u32) -> Result<()> {
        let nr = num_irqs;
        let attr = kvm_device_attr {
            flags: 0,
            group: kvm_sys::KVM_DEV_ARM_VGIC_GRP_NR_IRQS,
            attr: 0,
            addr: &nr as *const u32 as u64,
        };
        self.set_attr(&attr, "GRP_NR_IRQS")
    }

    /// Set distributor and CPU-interface / redistributor addresses.
    fn set_addresses(&self, config: &GicConfig) -> Result<()> {
        match config.version {
            GicVersion::V2 => {
                self.set_addr(
                    kvm_sys::KVM_VGIC_V2_ADDR_TYPE_DIST,
                    config.dist_addr,
                    "V2_DIST",
                )?;
                self.set_addr(
                    kvm_sys::KVM_VGIC_V2_ADDR_TYPE_CPU,
                    config.cpu_or_redist_addr,
                    "V2_CPU",
                )?;
            }
            GicVersion::V3 => {
                self.set_addr(
                    kvm_sys::KVM_VGIC_V3_ADDR_TYPE_DIST,
                    config.dist_addr,
                    "V3_DIST",
                )?;
                self.set_addr(
                    kvm_sys::KVM_VGIC_V3_ADDR_TYPE_REDIST,
                    config.cpu_or_redist_addr,
                    "V3_REDIST",
                )?;
            }
        }
        Ok(())
    }

    /// Set a GIC address attribute.
    fn set_addr(&self, attr_val: u64, addr: u64, label: &str) -> Result<()> {
        let val = addr;
        let attr = kvm_device_attr {
            flags: 0,
            group: kvm_sys::KVM_DEV_ARM_VGIC_GRP_ADDR,
            attr: attr_val,
            addr: &val as *const u64 as u64,
        };
        self.set_attr(&attr, label)
    }

    /// Send `KVM_DEV_ARM_VGIC_CTRL_INIT` to finalise device creation.
    fn init_device(&self) -> Result<()> {
        let attr = kvm_device_attr {
            flags: 0,
            group: kvm_sys::KVM_DEV_ARM_VGIC_GRP_CTRL,
            attr: kvm_sys::KVM_DEV_ARM_VGIC_CTRL_INIT,
            addr: 0,
        };
        self.set_attr(&attr, "CTRL_INIT")
    }

    /// Call `KVM_SET_DEVICE_ATTR` on the GIC fd.
    fn set_attr(&self, attr: &kvm_device_attr, label: &str) -> Result<()> {
        unsafe {
            kvm_sys::kvm_ioctl(
                self.device_fd,
                kvm_sys::KVM_SET_DEVICE_ATTR,
                attr as *const _ as u64,
            )
        }
        .map_err(|e| KvmError::Ioctl {
            name: "KVM_SET_DEVICE_ATTR",
            source: std::io::Error::new(e.kind(), format!("{label}: {e}")),
        })?;
        Ok(())
    }

    /// Check whether a device attribute is supported.
    pub fn has_attr(&self, group: u32, attr_val: u64) -> bool {
        let attr = kvm_device_attr {
            flags: 0,
            group,
            attr: attr_val,
            addr: 0,
        };
        unsafe {
            kvm_sys::kvm_ioctl(
                self.device_fd,
                kvm_sys::KVM_HAS_DEVICE_ATTR,
                &attr as *const _ as u64,
            )
        }
        .is_ok()
    }

    /// Return the GIC configuration.
    pub fn config(&self) -> &GicConfig {
        &self.config
    }

    /// Return the GIC device fd.
    pub fn fd(&self) -> RawFd {
        self.device_fd
    }
}

impl Drop for KvmGic {
    fn drop(&mut self) {
        unsafe { libc::close(self.device_fd) };
    }
}
