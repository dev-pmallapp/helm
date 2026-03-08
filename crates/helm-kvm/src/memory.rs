//! Guest physical memory management.
//!
//! Provides [`GuestMemory`] — a collection of [`GuestMemoryRegion`]s
//! backed by `mmap`'d host memory, registered with KVM via
//! `KVM_SET_USER_MEMORY_REGION`.

use crate::error::{KvmError, Result};
use crate::kvm_sys::{self, kvm_userspace_memory_region};
use std::os::unix::io::RawFd;
use std::ptr;

/// A single contiguous region of guest physical memory.
///
/// The backing host memory is allocated via `mmap(MAP_ANONYMOUS |
/// MAP_PRIVATE)` so that KVM's stage-2 page tables can map it.
#[derive(Debug)]
pub struct GuestMemoryRegion {
    /// KVM memory slot number.
    pub slot: u32,
    /// Guest physical base address.
    pub guest_phys_addr: u64,
    /// Region size in bytes.
    pub size: u64,
    /// Host virtual address (from `mmap`).
    host_addr: *mut u8,
}

// Safety: the mmap'd region is process-private and not shared across
// threads without synchronisation provided by KVM.
unsafe impl Send for GuestMemoryRegion {}
unsafe impl Sync for GuestMemoryRegion {}

impl GuestMemoryRegion {
    /// Allocate a new memory region backed by anonymous `mmap`.
    ///
    /// The region is **not** registered with KVM until
    /// [`register`](GuestMemoryRegion::register) is called.
    pub fn new(slot: u32, guest_phys_addr: u64, size: u64) -> Result<Self> {
        if size == 0 {
            return Err(KvmError::InvalidParameter("region size must be > 0".into()));
        }
        let host_addr = unsafe {
            libc::mmap(
                ptr::null_mut(),
                size as libc::size_t,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_ANONYMOUS | libc::MAP_PRIVATE | libc::MAP_NORESERVE,
                -1,
                0,
            )
        };
        if host_addr == libc::MAP_FAILED {
            return Err(KvmError::Mmap(std::io::Error::last_os_error()));
        }
        Ok(Self {
            slot,
            guest_phys_addr,
            size,
            host_addr: host_addr as *mut u8,
        })
    }

    /// Register this region with KVM via `KVM_SET_USER_MEMORY_REGION`.
    pub fn register(&self, vm_fd: RawFd) -> Result<()> {
        let region = kvm_userspace_memory_region {
            slot: self.slot,
            flags: 0,
            guest_phys_addr: self.guest_phys_addr,
            memory_size: self.size,
            userspace_addr: self.host_addr as u64,
        };
        unsafe {
            kvm_sys::kvm_ioctl(
                vm_fd,
                kvm_sys::KVM_SET_USER_MEMORY_REGION,
                &region as *const _ as u64,
            )
        }
        .map_err(|e| KvmError::Ioctl {
            name: "KVM_SET_USER_MEMORY_REGION",
            source: e,
        })?;
        Ok(())
    }

    /// Unregister this region from KVM (sets `memory_size = 0`).
    pub fn unregister(&self, vm_fd: RawFd) -> Result<()> {
        let region = kvm_userspace_memory_region {
            slot: self.slot,
            flags: 0,
            guest_phys_addr: self.guest_phys_addr,
            memory_size: 0,
            userspace_addr: self.host_addr as u64,
        };
        unsafe {
            kvm_sys::kvm_ioctl(
                vm_fd,
                kvm_sys::KVM_SET_USER_MEMORY_REGION,
                &region as *const _ as u64,
            )
        }
        .map_err(|e| KvmError::Ioctl {
            name: "KVM_SET_USER_MEMORY_REGION(unregister)",
            source: e,
        })?;
        Ok(())
    }

    /// Raw host pointer to the start of this region.
    pub fn host_ptr(&self) -> *mut u8 {
        self.host_addr
    }

    /// Translate a guest physical address to a host pointer.
    ///
    /// Returns `None` if `gpa` is outside this region.
    pub fn translate(&self, gpa: u64) -> Option<*mut u8> {
        if gpa >= self.guest_phys_addr && gpa < self.guest_phys_addr + self.size {
            let offset = (gpa - self.guest_phys_addr) as usize;
            Some(unsafe { self.host_addr.add(offset) })
        } else {
            None
        }
    }

    /// Write `data` into the region at the given guest physical address.
    pub fn write(&self, gpa: u64, data: &[u8]) -> Result<()> {
        let end = gpa + data.len() as u64;
        if gpa < self.guest_phys_addr || end > self.guest_phys_addr + self.size {
            return Err(KvmError::InvalidParameter(format!(
                "write {:#x}..{:#x} out of region {:#x}..{:#x}",
                gpa,
                end,
                self.guest_phys_addr,
                self.guest_phys_addr + self.size
            )));
        }
        let offset = (gpa - self.guest_phys_addr) as usize;
        unsafe {
            ptr::copy_nonoverlapping(data.as_ptr(), self.host_addr.add(offset), data.len());
        }
        Ok(())
    }

    /// Read `len` bytes from the region at the given guest physical address.
    pub fn read(&self, gpa: u64, len: usize) -> Result<Vec<u8>> {
        let end = gpa + len as u64;
        if gpa < self.guest_phys_addr || end > self.guest_phys_addr + self.size {
            return Err(KvmError::InvalidParameter(format!(
                "read {:#x}..{:#x} out of region {:#x}..{:#x}",
                gpa,
                end,
                self.guest_phys_addr,
                self.guest_phys_addr + self.size
            )));
        }
        let offset = (gpa - self.guest_phys_addr) as usize;
        let mut buf = vec![0u8; len];
        unsafe {
            ptr::copy_nonoverlapping(self.host_addr.add(offset), buf.as_mut_ptr(), len);
        }
        Ok(buf)
    }

    /// Return a slice view of the entire region.
    ///
    /// # Safety
    /// The caller must ensure no concurrent mutation of the region.
    pub unsafe fn as_slice(&self) -> &[u8] {
        std::slice::from_raw_parts(self.host_addr, self.size as usize)
    }

    /// Return a mutable slice view of the entire region.
    ///
    /// # Safety
    /// The caller must ensure exclusive access.
    pub unsafe fn as_mut_slice(&mut self) -> &mut [u8] {
        std::slice::from_raw_parts_mut(self.host_addr, self.size as usize)
    }
}

impl Drop for GuestMemoryRegion {
    fn drop(&mut self) {
        if !self.host_addr.is_null() {
            unsafe {
                libc::munmap(
                    self.host_addr as *mut libc::c_void,
                    self.size as libc::size_t,
                );
            }
        }
    }
}

/// Collection of guest memory regions.
///
/// Manages multiple [`GuestMemoryRegion`]s and provides whole-address-space
/// translation.
pub struct GuestMemory {
    regions: Vec<GuestMemoryRegion>,
    next_slot: u32,
}

impl GuestMemory {
    /// Create an empty guest memory map.
    pub fn new() -> Self {
        Self {
            regions: Vec::new(),
            next_slot: 0,
        }
    }

    /// Allocate a new region and register it with KVM.
    pub fn add_region(&mut self, vm_fd: RawFd, guest_phys_addr: u64, size: u64) -> Result<usize> {
        let slot = self.next_slot;
        self.next_slot += 1;
        let region = GuestMemoryRegion::new(slot, guest_phys_addr, size)?;
        region.register(vm_fd)?;
        let idx = self.regions.len();
        self.regions.push(region);
        Ok(idx)
    }

    /// Translate a guest physical address to a host pointer.
    pub fn translate(&self, gpa: u64) -> Option<*mut u8> {
        for region in &self.regions {
            if let Some(ptr) = region.translate(gpa) {
                return Some(ptr);
            }
        }
        None
    }

    /// Write bytes to guest physical memory.
    pub fn write(&self, gpa: u64, data: &[u8]) -> Result<()> {
        for region in &self.regions {
            let start = region.guest_phys_addr;
            let end = start + region.size;
            if gpa >= start && gpa + data.len() as u64 <= end {
                return region.write(gpa, data);
            }
        }
        Err(KvmError::InvalidParameter(format!(
            "GPA {gpa:#x} not in any memory region"
        )))
    }

    /// Read bytes from guest physical memory.
    pub fn read(&self, gpa: u64, len: usize) -> Result<Vec<u8>> {
        for region in &self.regions {
            let start = region.guest_phys_addr;
            let end = start + region.size;
            if gpa >= start && gpa + len as u64 <= end {
                return region.read(gpa, len);
            }
        }
        Err(KvmError::InvalidParameter(format!(
            "GPA {gpa:#x} not in any memory region"
        )))
    }

    /// Return all regions.
    pub fn regions(&self) -> &[GuestMemoryRegion] {
        &self.regions
    }
}

impl Default for GuestMemory {
    fn default() -> Self {
        Self::new()
    }
}
