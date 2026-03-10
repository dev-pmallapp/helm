//! KVM virtual machine management.
//!
//! [`KvmVm`] is the top-level handle: it owns the VM file descriptor,
//! creates vCPUs, registers memory regions, and sets up the in-kernel
//! GIC.

use crate::capability::{self, KvmCaps};
use crate::error::{KvmError, Result};
use crate::gic::{GicConfig, KvmGic};
use crate::kvm_sys::{self, kvm_vcpu_init};
use crate::memory::GuestMemory;
use crate::vcpu::KvmVcpu;
use std::os::unix::io::RawFd;

/// Path to the KVM device node.
const KVM_DEV: &str = "/dev/kvm";

/// A KVM virtual machine.
///
/// # Lifecycle
///
/// ```text
/// KvmVm::new()            open /dev/kvm, KVM_CREATE_VM
///   ├── add_memory()       KVM_SET_USER_MEMORY_REGION × N
///   ├── create_vcpu()      KVM_CREATE_VCPU, mmap kvm_run
///   ├── setup_gic()        KVM_CREATE_DEVICE (after vCPUs)
///   └── vcpu.run() loop    KVM_RUN ↔ MMIO dispatch
/// ```
pub struct KvmVm {
    /// `/dev/kvm` file descriptor.
    kvm_fd: RawFd,
    /// VM file descriptor.
    vm_fd: RawFd,
    /// Probed capabilities.
    caps: KvmCaps,
    /// Guest physical memory.
    pub memory: GuestMemory,
    /// In-kernel GIC (if created).
    gic: Option<KvmGic>,
    /// Number of vCPUs created so far.
    vcpu_count: u32,
}

impl KvmVm {
    /// Open `/dev/kvm` and create a new VM.
    pub fn new() -> Result<Self> {
        let kvm_fd = Self::open_kvm()?;

        let caps = capability::probe(kvm_fd)?;
        if !caps.user_memory {
            return Err(KvmError::CapabilityMissing("KVM_CAP_USER_MEMORY".into()));
        }
        if !caps.one_reg {
            return Err(KvmError::CapabilityMissing("KVM_CAP_ONE_REG".into()));
        }

        let vm_fd =
            unsafe { kvm_sys::kvm_ioctl(kvm_fd, kvm_sys::KVM_CREATE_VM, 0) }.map_err(|e| {
                KvmError::Ioctl {
                    name: "KVM_CREATE_VM",
                    source: e,
                }
            })? as RawFd;

        log::info!(
            "KVM VM created: api={}, vcpu_mmap={}",
            caps.api_version,
            caps.vcpu_mmap_size
        );

        Ok(Self {
            kvm_fd,
            vm_fd,
            caps,
            memory: GuestMemory::new(),
            gic: None,
            vcpu_count: 0,
        })
    }

    /// Open `/dev/kvm`.
    fn open_kvm() -> Result<RawFd> {
        let fd = unsafe {
            libc::open(
                KVM_DEV.as_ptr() as *const libc::c_char,
                libc::O_RDWR | libc::O_CLOEXEC,
            )
        };
        if fd < 0 {
            return Err(KvmError::Unavailable(format!(
                "cannot open {KVM_DEV}: {}",
                std::io::Error::last_os_error()
            )));
        }
        Ok(fd)
    }

    // ── Memory ──────────────────────────────────────────────────────────

    /// Add a guest physical memory region and register it with KVM.
    ///
    /// Returns the index of the new region in [`GuestMemory::regions`].
    pub fn add_memory(&mut self, guest_phys_addr: u64, size: u64) -> Result<usize> {
        self.memory.add_region(self.vm_fd, guest_phys_addr, size)
    }

    // ── vCPU ────────────────────────────────────────────────────────────

    /// Query the preferred AArch64 vCPU target from KVM.
    pub fn preferred_target(&self) -> Result<kvm_vcpu_init> {
        let mut init = kvm_vcpu_init {
            target: 0,
            features: [0; 7],
        };
        unsafe {
            kvm_sys::kvm_ioctl(
                self.vm_fd,
                kvm_sys::KVM_ARM_PREFERRED_TARGET,
                &mut init as *mut _ as u64,
            )
        }
        .map_err(|e| KvmError::Ioctl {
            name: "KVM_ARM_PREFERRED_TARGET",
            source: e,
        })?;
        Ok(init)
    }

    /// Create a new vCPU.
    ///
    /// The vCPU is **not** initialised — call
    /// [`KvmVcpu::init`] with a [`kvm_vcpu_init`] from
    /// [`preferred_target`](KvmVm::preferred_target) before running.
    pub fn create_vcpu(&mut self) -> Result<KvmVcpu> {
        let vcpu_id = self.vcpu_count;
        self.vcpu_count += 1;

        let vcpu_fd =
            unsafe { kvm_sys::kvm_ioctl(self.vm_fd, kvm_sys::KVM_CREATE_VCPU, vcpu_id as u64) }
                .map_err(|e| KvmError::Ioctl {
                    name: "KVM_CREATE_VCPU",
                    source: e,
                })? as RawFd;

        log::debug!("created vCPU {vcpu_id} (fd={vcpu_fd})");
        KvmVcpu::from_fd(vcpu_fd, self.caps.vcpu_mmap_size)
    }

    // ── GIC ─────────────────────────────────────────────────────────────

    /// Set up the in-kernel GIC.
    ///
    /// Must be called **after** all vCPUs are created.
    pub fn setup_gic(&mut self, config: GicConfig) -> Result<()> {
        let gic = KvmGic::new(self.vm_fd, config)?;
        self.gic = Some(gic);
        Ok(())
    }

    /// Return a reference to the in-kernel GIC, if created.
    pub fn gic(&self) -> Option<&KvmGic> {
        self.gic.as_ref()
    }

    // ── IRQ ─────────────────────────────────────────────────────────────

    /// Assert or de-assert an IRQ line on the in-kernel GIC.
    ///
    /// This is a convenience wrapper around [`crate::irq::irq_line`].
    pub fn irq_line(&self, irq: u32, level: bool) -> Result<()> {
        crate::irq::irq_line(self.vm_fd, irq, level)
    }

    /// Assert an SPI (Shared Peripheral Interrupt).
    pub fn assert_spi(&self, spi_num: u32) -> Result<()> {
        crate::irq::assert_spi(self.vm_fd, spi_num)
    }

    /// De-assert an SPI.
    pub fn deassert_spi(&self, spi_num: u32) -> Result<()> {
        crate::irq::deassert_spi(self.vm_fd, spi_num)
    }

    // ── Accessors ───────────────────────────────────────────────────────

    /// Return the probed KVM capabilities.
    pub fn caps(&self) -> &KvmCaps {
        &self.caps
    }

    /// Return the VM file descriptor.
    pub fn vm_fd(&self) -> RawFd {
        self.vm_fd
    }

    /// Return the `/dev/kvm` file descriptor.
    pub fn kvm_fd(&self) -> RawFd {
        self.kvm_fd
    }

    /// Number of vCPUs created.
    pub fn vcpu_count(&self) -> u32 {
        self.vcpu_count
    }
}

impl Drop for KvmVm {
    fn drop(&mut self) {
        // GIC is dropped automatically (its Drop closes the device fd).
        // Close VM fd and /dev/kvm fd.
        unsafe {
            libc::close(self.vm_fd);
            libc::close(self.kvm_fd);
        }
    }
}
