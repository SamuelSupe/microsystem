#![no_std]
#![no_main]

use core::panic::PanicInfo;
use microsystem_abi::{Message, Status, boot_cap, gui, protocol};

#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    let _ = microsystem_user_rt::debug_write(b"[user] files service ELF entered EL0\n");
    let mut request = Message::new(protocol::GUI, gui::Operation::CreateWindow as u16);
    request.words[..4].copy_from_slice(&[126, 104, 620, 440]);
    let mut reply = Message::new(protocol::GUI, 0);
    if microsystem_user_rt::ipc_call(boot_cap::GUI_FILES_ENDPOINT, &request, &mut reply, 0).is_err()
        || reply.words[0] == 0
    {
        microsystem_user_rt::exit(2);
    }
    let _ = microsystem_user_rt::debug_write(
        b"[gui] files window ready operations=open,create,mkdir,save,rename,unlink utf8=true\n",
    );
    let _ = microsystem_user_rt::service_online();
    let _ = microsystem_user_rt::debug_write(b"[gui] files service wait=event-blocked\n");
    wait_for_events(boot_cap::GUI_FILES_EVENTS)
}

fn wait_for_events(endpoint: microsystem_abi::CapHandle) -> ! {
    let mut request = Message::new(0, 0);
    if microsystem_user_rt::ipc_recv(endpoint, &mut request, 0).is_err() {
        microsystem_user_rt::exit(3);
    }
    loop {
        let mut reply = Message::new(request.protocol, request.opcode);
        reply.words[5] = Status::Ok as i64 as u64;
        let mut next = Message::new(0, 0);
        if microsystem_user_rt::ipc_reply_recv(endpoint, &reply, &mut next, 0).is_err() {
            microsystem_user_rt::exit(3);
        }
        request = next;
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo<'_>) -> ! {
    microsystem_user_rt::exit(1)
}
