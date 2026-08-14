#![no_std]
#![no_main]

use core::panic::PanicInfo;
use microsystem_abi::{Message, Status, SystemStats, boot_cap, gui, protocol};

#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    let _ = microsystem_user_rt::debug_write(b"[user] monitor service ELF entered EL0\n");
    let mut request = Message::new(protocol::GUI, gui::Operation::CreateWindow as u16);
    request.words[..4].copy_from_slice(&[704, 48, 280, 360]);
    let mut reply = Message::new(protocol::GUI, 0);
    if microsystem_user_rt::ipc_call(boot_cap::GUI_MONITOR_ENDPOINT, &request, &mut reply, 0)
        .is_err()
        || reply.words[0] == 0
    {
        microsystem_user_rt::exit(2);
    }
    let mut stats = SystemStats::default();
    if microsystem_user_rt::system_stats(boot_cap::SYSTEM_INFO, &mut stats).is_err()
        || stats.version != 1
        || stats.cpu_count != 2
    {
        microsystem_user_rt::exit(3);
    }
    let _ = microsystem_user_rt::debug_write(
        b"[gui] monitor window ready cpus=2 memory=true irq=true ipc=true refresh=1s\n",
    );
    let _ = microsystem_user_rt::service_online();
    let _ = microsystem_user_rt::debug_write(b"[gui] monitor service wait=event-blocked\n");
    wait_for_events(boot_cap::GUI_MONITOR_EVENTS)
}

fn wait_for_events(endpoint: microsystem_abi::CapHandle) -> ! {
    let mut request = Message::new(0, 0);
    if microsystem_user_rt::ipc_recv(endpoint, &mut request, 0).is_err() {
        microsystem_user_rt::exit(4);
    }
    loop {
        let mut reply = Message::new(request.protocol, request.opcode);
        reply.words[5] = Status::Ok as i64 as u64;
        let mut next = Message::new(0, 0);
        if microsystem_user_rt::ipc_reply_recv(endpoint, &reply, &mut next, 0).is_err() {
            microsystem_user_rt::exit(4);
        }
        request = next;
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo<'_>) -> ! { microsystem_user_rt::exit(1) }
