#![no_std]
#![no_main]

use core::panic::PanicInfo;
use microsystem_abi::{CapHandle, Message, Rights, boot_cap, process, protocol};

const FIRST_FRAME_VA: u64 = 0x0058_0000;
const SECOND_FRAME_VA: u64 = 0x0058_1000;
const FAULT_VA: u64 = 0x0060_0000;

#[unsafe(no_mangle)]
pub extern "C" fn _start(pid: u64) -> ! {
    let _ = microsystem_user_rt::debug_write_u64(
        b"[app] resource fault probe pid=",
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
    let frame_rights = Rights(Rights::READ.0 | Rights::WRITE.0 | Rights::MAP.0);
    let frames = [
        microsystem_user_rt::frame_create(reply.caps[0], frame_rights)
            .unwrap_or_else(|_| microsystem_user_rt::exit(2)),
        microsystem_user_rt::frame_create(reply.caps[0], frame_rights)
            .unwrap_or_else(|_| microsystem_user_rt::exit(2)),
    ];
    for (frame, address) in frames.into_iter().zip([FIRST_FRAME_VA, SECOND_FRAME_VA]) {
        if microsystem_user_rt::frame_map(frame, address, Rights(Rights::READ.0 | Rights::WRITE.0))
            .is_err()
        {
            microsystem_user_rt::exit(3);
        }
        unsafe { core::ptr::write_volatile(address as *mut u64, address ^ pid) };
    }
    let _ = microsystem_user_rt::debug_write_u64(
        b"[app] resource fault probe pid=",
        pid,
        b" triggering page fault with live frames=2 mappings=2\n",
    );
    unsafe {
        core::arch::asm!("ldr xzr, [{address}]", address = in(reg) FAULT_VA, options(nostack));
    }
    microsystem_user_rt::exit(4)
}

#[panic_handler]
fn panic(_info: &PanicInfo<'_>) -> ! {
    microsystem_user_rt::exit(5)
}
