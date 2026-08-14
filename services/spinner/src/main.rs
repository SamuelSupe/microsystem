#![no_std]
#![no_main]

use core::panic::PanicInfo;

#[unsafe(no_mangle)]
pub extern "C" fn _start(pid: u64) -> ! {
    let _ = microsystem_user_rt::debug_write_u64(b"[app] spinner pid=", pid, b" started at EL0\n");
    let mut progress = 0u64;
    loop {
        progress = core::hint::black_box(progress.wrapping_add(1));
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo<'_>) -> ! {
    microsystem_user_rt::exit(1)
}
