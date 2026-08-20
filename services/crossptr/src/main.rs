#![no_std]
#![no_main]

use core::panic::PanicInfo;
use microsystem_abi::{Status, Syscall};

const STACK_END: u64 = 0x0080_0000;

#[unsafe(no_mangle)]
pub extern "C" fn _start(pid: u64) -> ! {
    let _ = microsystem_user_rt::debug_write_u64(
        b"[app] cross-page pointer probe pid=",
        pid,
        b" entered EL0\n",
    );
    let pointer = STACK_END - 4;
    unsafe {
        (pointer as *mut u32).write_volatile(u32::from_le_bytes(*b"XPG!"));
    }
    let result =
        unsafe { microsystem_user_rt::syscall(Syscall::DebugWrite, [pointer, 8, 0, 0, 0, 0]) };
    if result != Status::Fault as i64 {
        microsystem_user_rt::exit(1);
    }
    let _ = microsystem_user_rt::debug_write(
        b"[isolation] cross-page user buffer rejected-before-copy status=-8 task-survived=true\n",
    );
    microsystem_user_rt::exit(0)
}

#[panic_handler]
fn panic(_info: &PanicInfo<'_>) -> ! {
    microsystem_user_rt::exit(2)
}
