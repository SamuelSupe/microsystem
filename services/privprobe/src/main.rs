#![no_std]
#![no_main]

use core::panic::PanicInfo;

#[unsafe(no_mangle)]
pub extern "C" fn _start(pid: u64) -> ! {
    let _ = microsystem_user_rt::debug_write_u64(
        b"[app] privileged probe pid=",
        pid,
        b" entered EL0\n",
    );
    unsafe {
        core::arch::asm!("msr ttbr0_el1, xzr", options(nostack));
    }
    microsystem_user_rt::exit(1)
}

#[panic_handler]
fn panic(_info: &PanicInfo<'_>) -> ! {
    microsystem_user_rt::exit(2)
}
