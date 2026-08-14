use core::alloc::Layout;
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use microsystem_abi::Status;

const KERNEL_OFFSET: usize = 0xffff_ff80_0000_0000;

static READY: AtomicBool = AtomicBool::new(false);
static START: AtomicUsize = AtomicUsize::new(0);
static END: AtomicUsize = AtomicUsize::new(0);
static NEXT: AtomicUsize = AtomicUsize::new(0);

pub fn initialize(physical: u64, bytes: usize) -> Result<(), Status> {
    if READY.load(Ordering::Acquire) || physical % 4096 != 0 || bytes < 4096 {
        return Err(Status::Invalid);
    }
    let start = KERNEL_OFFSET
        .checked_add(usize::try_from(physical).map_err(|_| Status::Invalid)?)
        .ok_or(Status::Invalid)?;
    let end = start.checked_add(bytes).ok_or(Status::Invalid)?;
    START.store(start, Ordering::Relaxed);
    END.store(end, Ordering::Relaxed);
    NEXT.store(start, Ordering::Relaxed);
    READY.store(true, Ordering::Release);
    Ok(())
}

pub fn allocate_zeroed(layout: Layout) -> Result<*mut u8, Status> {
    if !READY.load(Ordering::Acquire) || !layout.align().is_power_of_two() {
        return Err(Status::Invalid);
    }
    let mut current = NEXT.load(Ordering::Acquire);
    loop {
        let aligned = align_up(current, layout.align()).ok_or(Status::NoMemory)?;
        let next = aligned.checked_add(layout.size()).ok_or(Status::NoMemory)?;
        if next > END.load(Ordering::Acquire) {
            return Err(Status::NoMemory);
        }
        match NEXT.compare_exchange_weak(current, next, Ordering::AcqRel, Ordering::Acquire) {
            Ok(_) => {
                unsafe { core::ptr::write_bytes(aligned as *mut u8, 0, layout.size()) };
                return Ok(aligned as *mut u8);
            }
            Err(observed) => current = observed,
        }
    }
}

pub fn allocated_bytes() -> usize {
    NEXT.load(Ordering::Acquire)
        .saturating_sub(START.load(Ordering::Acquire))
}

fn align_up(value: usize, alignment: usize) -> Option<usize> {
    value
        .checked_add(alignment - 1)
        .map(|aligned| aligned & !(alignment - 1))
}
