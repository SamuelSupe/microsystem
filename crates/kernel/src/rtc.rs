use core::sync::atomic::{AtomicUsize, Ordering};

const MINIMUM_VALID_UNIX_TIME: u32 = 1_577_836_800;

static BASE: AtomicUsize = AtomicUsize::new(0);

pub fn initialize(physical: usize) {
    BASE.store(
        if physical == 0 {
            0
        } else {
            crate::arch::phys_to_virt(physical as u64)
        },
        Ordering::Release,
    );
}

pub fn realtime() -> Option<u64> {
    let base = BASE.load(Ordering::Acquire);
    if base == 0 {
        return None;
    }
    #[cfg(target_arch = "aarch64")]
    let seconds = unsafe { core::ptr::read_volatile(base as *const u32) };
    #[cfg(target_arch = "riscv64")]
    let seconds = {
        let low = unsafe { core::ptr::read_volatile(base as *const u32) } as u64;
        let high = unsafe { core::ptr::read_volatile((base + 4) as *const u32) } as u64;
        ((high << 32) | low) / 1_000_000_000
    } as u32;
    (seconds >= MINIMUM_VALID_UNIX_TIME).then_some(seconds as u64)
}
