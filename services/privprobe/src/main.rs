#![no_std]
#![no_main]

use core::panic::PanicInfo;
use microsystem_abi::{Status, boot_cap, service};

#[unsafe(no_mangle)]
pub extern "C" fn _start(pid: u64) -> ! {
    let _ = microsystem_user_rt::debug_write_u64(
        b"[app] privileged probe pid=",
        pid,
        b" entered EL0\n",
    );
    let mut info = service::InfoV1::EMPTY;
    if microsystem_user_rt::service_status(3, &mut info) != Err(Status::AccessDenied)
        || microsystem_user_rt::service_stop(3, -15) != Err(Status::AccessDenied)
        || microsystem_user_rt::console_write_bytes(b"") != Err(Status::AccessDenied)
    {
        microsystem_user_rt::exit(3);
    }
    let _ = microsystem_user_rt::debug_write(
        b"[isolation] application service-management syscalls denied=true\n",
    );
    if !matches!(microsystem_user_rt::network_interface_info(boot_cap::NETWORK_DEVICE, 0), Err(Status::AccessDenied))
        || microsystem_user_rt::ipc_peer() != Err(Status::Busy)
    {
        microsystem_user_rt::exit(4);
    }
    let _ = microsystem_user_rt::debug_write(
        b"[isolation] application raw-network denied=true IPC peer without caller=Busy\n",
    );
    unsafe { microsystem_user_rt::privileged_probe() };
    microsystem_user_rt::exit(1)
}

#[panic_handler]
fn panic(_info: &PanicInfo<'_>) -> ! {
    microsystem_user_rt::exit(2)
}
