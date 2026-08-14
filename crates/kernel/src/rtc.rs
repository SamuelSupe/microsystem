use core::sync::atomic::{AtomicUsize, Ordering};

const KERNEL_OFFSET: usize = 0xffff_ff80_0000_0000;
const MINIMUM_VALID_UNIX_TIME: u32 = 1_577_836_800;

static BASE: AtomicUsize = AtomicUsize::new(0);

pub fn initialize(physical: usize) {
    BASE.store(
        if physical == 0 {
            0
        } else {
            KERNEL_OFFSET + physical
        },
        Ordering::Release,
    );
}

pub fn realtime() -> Option<u64> {
    let base = BASE.load(Ordering::Acquire);
    if base == 0 {
        return None;
    }
    let seconds = unsafe { core::ptr::read_volatile(base as *const u32) };
    (seconds >= MINIMUM_VALID_UNIX_TIME).then_some(seconds as u64)
}
