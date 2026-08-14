#![no_std]
#![no_main]

use core::panic::PanicInfo;

#[unsafe(no_mangle)]
pub extern "C" fn _start(pid: u64) -> ! {
    let _ = microsystem_user_rt::debug_write_u64(b"[app] counter pid=", pid, b" started at EL0\n");
    let mut counter = 0u64;
    while counter < 20_000_000 {
        counter = core::hint::black_box(counter.wrapping_add(1));
    }
    let _ = microsystem_user_rt::debug_write_u64(b"[app] counter pid=", pid, b" completed\n");
    microsystem_user_rt::exit(0)
}

#[panic_handler]
fn panic(_info: &PanicInfo<'_>) -> ! {
    microsystem_user_rt::exit(1)
}
