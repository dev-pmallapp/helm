use crate::memory::GuestMemoryRegion;

#[test]
fn region_new_allocates_memory() {
    let region = GuestMemoryRegion::new(0, 0x4000_0000, 4096).unwrap();
    assert_eq!(region.slot, 0);
    assert_eq!(region.guest_phys_addr, 0x4000_0000);
    assert_eq!(region.size, 4096);
    assert!(!region.host_ptr().is_null());
}

#[test]
fn region_zero_size_is_error() {
    let res = GuestMemoryRegion::new(0, 0, 0);
    assert!(res.is_err());
}

#[test]
fn region_write_read_roundtrip() {
    let region = GuestMemoryRegion::new(0, 0x1000, 0x1000).unwrap();
    let data = b"hello kvm";
    region.write(0x1000, data).unwrap();
    let out = region.read(0x1000, data.len()).unwrap();
    assert_eq!(&out, data);
}

#[test]
fn region_write_out_of_bounds() {
    let region = GuestMemoryRegion::new(0, 0x1000, 0x100).unwrap();
    let data = [0u8; 0x200];
    assert!(region.write(0x1000, &data).is_err());
}

#[test]
fn region_read_out_of_bounds() {
    let region = GuestMemoryRegion::new(0, 0x1000, 0x100).unwrap();
    assert!(region.read(0x1000, 0x200).is_err());
}

#[test]
fn region_translate_in_range() {
    let region = GuestMemoryRegion::new(0, 0x2000, 0x1000).unwrap();
    let ptr = region.translate(0x2800);
    assert!(ptr.is_some());
}

#[test]
fn region_translate_out_of_range() {
    let region = GuestMemoryRegion::new(0, 0x2000, 0x1000).unwrap();
    assert!(region.translate(0x4000).is_none());
    assert!(region.translate(0x1000).is_none());
}

#[test]
fn region_translate_boundary() {
    let region = GuestMemoryRegion::new(0, 0x2000, 0x1000).unwrap();
    assert!(region.translate(0x2000).is_some());
    assert!(region.translate(0x2FFF).is_some());
    assert!(region.translate(0x3000).is_none());
}

#[test]
fn region_as_slice() {
    let region = GuestMemoryRegion::new(0, 0x0, 256).unwrap();
    region.write(0x0, &[0xAA; 8]).unwrap();
    let slice = unsafe { region.as_slice() };
    assert_eq!(slice.len(), 256);
    assert_eq!(&slice[..8], &[0xAA; 8]);
}

#[test]
fn guest_memory_translate_multiple_regions() {
    let gm = crate::memory::GuestMemory::new();
    let r0 = GuestMemoryRegion::new(0, 0x0, 0x1000).unwrap();
    let r1 = GuestMemoryRegion::new(1, 0x4000_0000, 0x1000).unwrap();
    // We can't register without a VM fd, but we can test translate
    // by using the region directly.
    assert!(r0.translate(0x800).is_some());
    assert!(r1.translate(0x4000_0800).is_some());
    assert!(r0.translate(0x4000_0000).is_none());
    // GuestMemory without registration:
    assert!(gm.translate(0x0).is_none()); // empty
    let _ = (r0, r1); // prevent unused warnings
}
