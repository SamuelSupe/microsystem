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
    unsafe { microsystem_user_rt::fault_probe(0x0060_0000) };
    microsystem_user_rt::exit(1)
}

#[panic_handler]
fn panic(_info: &PanicInfo<'_>) -> ! {
    microsystem_user_rt::exit(2)
}
