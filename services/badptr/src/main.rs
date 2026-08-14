#![no_std]
#![no_main]

use core::panic::PanicInfo;
use microsystem_abi::Status;

#[unsafe(no_mangle)]
pub extern "C" fn _start(pid: u64) -> ! {
    let _ = microsystem_user_rt::debug_write_u64(
        b"[app] invalid-pointer probe pid=",
        pid,
        b" entered EL0\n",
    );
    let result: i64;
    unsafe {
        core::arch::asm!(
            "svc #0",
            inlateout("x0") u64::MAX => result,
            in("x1") 4u64,
            in("x8") 19u64,
            options(nostack)
        );
    }
    if result != Status::Fault as i64 {
        microsystem_user_rt::exit(1);
    }
    let _ = microsystem_user_rt::debug_write(
        b"[isolation] invalid user pointer rejected status=-8 task-survived=true\n",
    );
    microsystem_user_rt::exit(0)
}

#[panic_handler]
fn panic(_info: &PanicInfo<'_>) -> ! {
    microsystem_user_rt::exit(2)
}
