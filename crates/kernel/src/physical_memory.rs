use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use microsystem_abi::Status;
use microsystem_kernel::memory::{FrameAllocator, PAGE_SIZE};

use crate::kernel_heap;

const KERNEL_HEAP_BYTES: u64 = 128 * 1024 * 1024;

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

pub fn initialize(ram_base: u64, ram_bytes: u64) -> Result<MemoryLayout, Status> {
    let _guard = lock_allocator();
    if ALLOCATOR_READY.load(Ordering::Acquire) {
        return Err(Status::Busy);
    }
    let ram_end = ram_base.checked_add(ram_bytes).ok_or(Status::Invalid)?;
    let kernel_end = crate::arch::virt_to_phys(core::ptr::addr_of!(__kernel_end) as usize as u64)
        .ok_or(Status::Invalid)?;
    let kernel_heap_base = align_up(kernel_end.max(ram_base), PAGE_SIZE).ok_or(Status::Invalid)?;
    let allocator_base = kernel_heap_base
        .checked_add(KERNEL_HEAP_BYTES)
        .ok_or(Status::Invalid)?;
    if allocator_base >= ram_end {
        return Err(Status::NoMemory);
    }
    kernel_heap::initialize(kernel_heap_base, KERNEL_HEAP_BYTES as usize)?;
    let allocator = unsafe { &mut *ALLOCATOR.0.get() };
    allocator.initialize(allocator_base, ram_end - allocator_base)?;
    let free = allocator.free_frames();
    ALLOCATOR_READY.store(true, Ordering::Release);
    Ok(MemoryLayout {
        kernel_heap_base,
        kernel_heap_bytes: KERNEL_HEAP_BYTES as usize,
        frame_base: allocator_base,
        free_frames: free,
    })
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
