#![no_std]
#![no_main]

use core::panic::PanicInfo;

#[unsafe(no_mangle)]
pub extern "C" fn _start(pid: u64) -> ! {
    let _ = microsystem_user_rt::debug_write_u64(
        b"[app] page-fault probe pid=",
        pid,
        b" entered EL0\n",
    );
    unsafe {
        core::arch::asm!("ldr xzr, [{address}]", address = in(reg) 0x0060_0000u64, options(nostack));
    }
    microsystem_user_rt::exit(1)
}

#[panic_handler]
fn panic(_info: &PanicInfo<'_>) -> ! {
    microsystem_user_rt::exit(2)
}
