#![no_std]
#![no_main]

use core::panic::PanicInfo;
use microsystem_abi::{CapHandle, Message, Rights, boot_cap, process, protocol};

const FRAME_VA: u64 = 0x0058_0000;

#[unsafe(no_mangle)]
pub extern "C" fn _start(pid: u64) -> ! {
    let _ = microsystem_user_rt::debug_write_u64(
        b"[app] resource cleanup probe pid=",
        pid,
        b" entered EL0\n",
    );
    let mut request = Message::new(protocol::PROCESS, process::Operation::MemoryPool as u16);
    request.words[0] = 2;
    let mut reply = Message::new(protocol::PROCESS, 0);
    if microsystem_user_rt::ipc_call(boot_cap::PROCESS_ENDPOINT, &request, &mut reply, 0).is_err()
        || reply.words[5] as i64 != 0
        || reply.caps[0] == CapHandle::INVALID
    {
        microsystem_user_rt::exit(1);
    }
    let rights = Rights(Rights::READ.0 | Rights::WRITE.0 | Rights::MAP.0);
    let frame = microsystem_user_rt::frame_create(reply.caps[0], rights)
        .unwrap_or_else(|_| microsystem_user_rt::exit(2));
    if microsystem_user_rt::frame_map(frame, FRAME_VA, Rights(Rights::READ.0 | Rights::WRITE.0))
        .is_err()
    {
        microsystem_user_rt::exit(3);
    }
    unsafe { core::ptr::write_volatile(FRAME_VA as *mut u64, 0x4d53_5953_5445_4d21) };
    let _ = microsystem_user_rt::debug_write_u64(
        b"[app] resource cleanup probe pid=",
        pid,
        b" exiting with live pool-frame-mapping=true\n",
    );
    microsystem_user_rt::exit(0)
}

#[panic_handler]
fn panic(_info: &PanicInfo<'_>) -> ! {
    microsystem_user_rt::exit(4)
}
