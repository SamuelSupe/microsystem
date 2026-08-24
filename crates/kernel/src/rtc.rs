#[cfg(target_arch = "x86_64")]
use core::sync::atomic::AtomicBool;
use core::sync::atomic::{AtomicUsize, Ordering};

const MINIMUM_VALID_UNIX_TIME: u32 = 1_577_836_800;

static BASE: AtomicUsize = AtomicUsize::new(0);
#[cfg(target_arch = "x86_64")]
static CMOS_LOCK: AtomicBool = AtomicBool::new(false);

#[cfg(target_arch = "x86_64")]
struct CmosGuard;

#[cfg(target_arch = "x86_64")]
impl Drop for CmosGuard {
    fn drop(&mut self) {
        CMOS_LOCK.store(false, Ordering::Release);
    }
}

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

#[cfg(target_arch = "x86_64")]
pub fn realtime() -> Option<u64> {
    let _guard = lock_cmos();
    for _ in 0..100_000 {
        if cmos(0x0a) & 0x80 != 0 {
            core::hint::spin_loop();
            continue;
        }
        let first = rtc_fields();
        if cmos(0x0a) & 0x80 == 0 && first == rtc_fields() {
            return unix_time(first).filter(|seconds| *seconds >= MINIMUM_VALID_UNIX_TIME as u64);
        }
    }
    None
}

#[cfg(target_arch = "x86_64")]
fn lock_cmos() -> CmosGuard {
    // Port 0x70 selects the register consumed by port 0x71, so the complete
    // RTC sample must be serialized across CPUs rather than each byte alone.
    while CMOS_LOCK
        .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
        .is_err()
    {
        core::hint::spin_loop();
    }
    CmosGuard
}

#[cfg(target_arch = "x86_64")]
fn rtc_fields() -> [u8; 7] {
    [
        cmos(0x00),
        cmos(0x02),
        cmos(0x04),
        cmos(0x07),
        cmos(0x08),
        cmos(0x09),
        cmos(0x32),
    ]
}

#[cfg(target_arch = "x86_64")]
fn unix_time(mut fields: [u8; 7]) -> Option<u64> {
    let status_b = cmos(0x0b);
    let pm = fields[2] & 0x80 != 0;
    fields[2] &= 0x7f;
    if status_b & 0x04 == 0 {
        for field in &mut fields {
            *field = (*field & 0x0f) + ((*field >> 4) * 10);
        }
    }
    let mut hour = u32::from(fields[2]);
    if status_b & 0x02 == 0 {
        hour %= 12;
        if pm {
            hour += 12;
        }
    }
    let year = if fields[6] != 0 {
        u32::from(fields[6]) * 100 + u32::from(fields[5])
    } else if fields[5] >= 70 {
        1900 + u32::from(fields[5])
    } else {
        2000 + u32::from(fields[5])
    };
    let month = u32::from(fields[4]);
    let day = u32::from(fields[3]);
    let minute = u32::from(fields[1]);
    let second = u32::from(fields[0]);
    if !(1970..=9999).contains(&year)
        || !(1..=12).contains(&month)
        || day == 0
        || day > days_in_month(year, month)
        || hour > 23
        || minute > 59
        || second > 59
    {
        return None;
    }
    let days = days_before_year(year) + days_before_month(year, month) + u64::from(day - 1);
    Some(days * 86_400 + u64::from(hour * 3600 + minute * 60 + second))
}

#[cfg(target_arch = "x86_64")]
const fn leap_year(year: u32) -> bool {
    year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400))
}

#[cfg(target_arch = "x86_64")]
const fn days_before_year(year: u32) -> u64 {
    let previous = year - 1;
    let leap_days = previous / 4 - previous / 100 + previous / 400;
    let base_leap_days = 1969 / 4 - 1969 / 100 + 1969 / 400;
    ((year - 1970) * 365 + leap_days - base_leap_days) as u64
}

#[cfg(target_arch = "x86_64")]
fn days_before_month(year: u32, month: u32) -> u64 {
    const DAYS: [u16; 12] = [0, 31, 59, 90, 120, 151, 181, 212, 243, 273, 304, 334];
    u64::from(DAYS[(month - 1) as usize]) + u64::from(month > 2 && leap_year(year))
}

#[cfg(target_arch = "x86_64")]
const fn days_in_month(year: u32, month: u32) -> u32 {
    match month {
        2 if leap_year(year) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

#[cfg(target_arch = "x86_64")]
fn cmos(register: u8) -> u8 {
    let value: u8;
    unsafe {
        core::arch::asm!(
            "out dx, al",
            in("dx") 0x70u16,
            in("al") register,
            options(nomem, nostack, preserves_flags)
        );
        core::arch::asm!(
            "in al, dx",
            in("dx") 0x71u16,
            out("al") value,
            options(nomem, nostack, preserves_flags)
        );
    }
    value
}

#[cfg(not(target_arch = "x86_64"))]
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
