use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use microsystem_abi::Status;
use microsystem_kernel::memory::{FrameAllocator, MAX_MEMORY_REGIONS, PAGE_SIZE, PhysicalRange};

use crate::dtb::PlatformInfo;
use crate::kernel_heap;

const KERNEL_HEAP_BYTES: u64 = 64 * 1024 * 1024;

unsafe extern "C" {
    static __kernel_end: u8;
}

struct AllocatorCell(UnsafeCell<FrameAllocator>);

unsafe impl Sync for AllocatorCell {}

struct AllocatorGuard;

impl Drop for AllocatorGuard {
    fn drop(&mut self) {
        ALLOCATOR_LOCK_OWNER.fetch_add(1, Ordering::Release);
    }
}

static ALLOCATOR: AllocatorCell = AllocatorCell(UnsafeCell::new(FrameAllocator::empty()));
static ALLOCATOR_READY: AtomicBool = AtomicBool::new(false);
static ALLOCATOR_LOCK_NEXT: AtomicU32 = AtomicU32::new(0);
static ALLOCATOR_LOCK_OWNER: AtomicU32 = AtomicU32::new(0);

pub struct MemoryLayout {
    pub kernel_heap_base: u64,
    pub kernel_heap_bytes: usize,
    pub frame_base: u64,
    pub free_frames: usize,
}

fn lock_allocator() -> AllocatorGuard {
    let ticket = ALLOCATOR_LOCK_NEXT.fetch_add(1, Ordering::Relaxed);
    while ALLOCATOR_LOCK_OWNER.load(Ordering::Acquire) != ticket {
        core::hint::spin_loop();
    }
    AllocatorGuard
}

pub fn initialize(platform: &PlatformInfo) -> Result<MemoryLayout, Status> {
    let _guard = lock_allocator();
    if ALLOCATOR_READY.load(Ordering::Acquire) {
        return Err(Status::Busy);
    }
    if platform.memory_map_overflow || platform.ram_region_count == 0 {
        return Err(Status::Invalid);
    }
    let kernel_end = crate::arch::virt_to_phys(core::ptr::addr_of!(__kernel_end) as usize as u64)
        .ok_or(Status::Invalid)?;
    let ram_regions = &platform.ram_regions[..platform.ram_region_count];
    let (heap_region, kernel_heap_base, allocator_base) = find_heap_region(
        ram_regions,
        &platform.reserved_regions[..platform.reserved_region_count],
        kernel_end,
    ).inspect_err(|status| crate::kprintln!("[mm] heap region selection failed kernel-end={:#x} status={:?}", kernel_end, status))?;

    let mut allocator_regions = [PhysicalRange { base: 0, bytes: 0 }; MAX_MEMORY_REGIONS];
    let mut allocator_region_count = 0;
    let heap_region_end = heap_region.base + heap_region.bytes;
    if allocator_base < heap_region_end {
        allocator_regions[allocator_region_count] = PhysicalRange {
            base: allocator_base,
            bytes: heap_region_end - allocator_base,
        };
        allocator_region_count += 1;
    }
    for region in ram_regions {
        if region.base == heap_region.base && region.bytes == heap_region.bytes {
            continue;
        }
        allocator_regions[allocator_region_count] = *region;
        allocator_region_count += 1;
    }

    kernel_heap::initialize(kernel_heap_base, KERNEL_HEAP_BYTES as usize)?;
    let allocator = unsafe { &mut *ALLOCATOR.0.get() };
    allocator.initialize_regions(&allocator_regions[..allocator_region_count]).inspect_err(|status| {
        crate::kprintln!("[mm] frame ranges rejected count={} status={:?}", allocator_region_count, status);
        for range in &allocator_regions[..allocator_region_count] { crate::kprintln!("[mm] candidate={:#x}+{:#x}", range.base, range.bytes); }
    })?;
    // Other firmware RAM intervals may include low boot memory. They must not
    // make the kernel, trampoline or chosen heap allocatable again.
    allocator.reserve(0, allocator_base)?;
    for region in &platform.reserved_regions[..platform.reserved_region_count] {
        allocator.reserve(region.base, region.bytes)?;
    }
    let free = allocator.free_frames();
    ALLOCATOR_READY.store(true, Ordering::Release);
    Ok(MemoryLayout {
        kernel_heap_base,
        kernel_heap_bytes: KERNEL_HEAP_BYTES as usize,
        frame_base: allocator_base,
        free_frames: free,
    })
}

fn find_heap_region(
    ram_regions: &[PhysicalRange],
    reserved_regions: &[PhysicalRange],
    kernel_end: u64,
) -> Result<(PhysicalRange, u64, u64), Status> {
    for region in ram_regions {
        let region_end = region
            .base
            .checked_add(region.bytes)
            .ok_or(Status::Invalid)?;
        if kernel_end < region.base || kernel_end >= region_end {
            continue;
        }
        let mut heap_base = align_up(kernel_end, PAGE_SIZE).ok_or(Status::Invalid)?;
        loop {
            let heap_end = heap_base
                .checked_add(KERNEL_HEAP_BYTES)
                .ok_or(Status::Invalid)?;
            if heap_end > region_end {
                break;
            }
            let mut conflicting_end = None;
            for reserved in reserved_regions {
                if reserved.bytes == 0 {
                    continue;
                }
                let reserved_end = reserved
                    .base
                    .checked_add(reserved.bytes)
                    .ok_or(Status::Invalid)?;
                if heap_base < reserved_end && reserved.base < heap_end {
                    conflicting_end = Some(conflicting_end.unwrap_or(0u64).max(reserved_end));
                }
            }
            if let Some(end) = conflicting_end {
                heap_base = align_up(end, PAGE_SIZE).ok_or(Status::Invalid)?;
                continue;
            }
            return Ok((*region, heap_base, heap_end));
        }
    }
    Err(Status::NoMemory)
}

pub fn allocate_zeroed() -> Result<u64, Status> {
    let address = {
        let _guard = lock_allocator();
        if !ALLOCATOR_READY.load(Ordering::Acquire) {
            return Err(Status::Invalid);
        }
        unsafe { &mut *ALLOCATOR.0.get() }.allocate()?
    };
    zero_page(address);
    Ok(address)
}

pub fn release(address: u64) -> Result<(), Status> {
    let _guard = lock_allocator();
    if !ALLOCATOR_READY.load(Ordering::Acquire) {
        return Err(Status::Invalid);
    }
    unsafe { &mut *ALLOCATOR.0.get() }.release(address)?;
    zero_page(address);
    Ok(())
}

pub fn free_frames() -> usize {
    let _guard = lock_allocator();
    if !ALLOCATOR_READY.load(Ordering::Acquire) {
        return 0;
    }
    unsafe { &*ALLOCATOR.0.get() }.free_frames()
}

pub const fn kernel_heap_bytes() -> usize {
    KERNEL_HEAP_BYTES as usize
}

fn zero_page(address: u64) {
    unsafe {
        core::ptr::write_bytes(
            crate::arch::phys_to_virt(address) as *mut u8,
            0,
            PAGE_SIZE as usize,
        );
    }
}

fn align_up(value: u64, alignment: u64) -> Option<u64> {
    value
        .checked_add(alignment - 1)
        .map(|aligned| aligned & !(alignment - 1))
}
