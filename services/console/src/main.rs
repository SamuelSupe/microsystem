#![no_std]
#![no_main]

use core::panic::PanicInfo;
use microsystem_abi::{
    CapHandle, Message, Rights, Status, Syscall, boot_cap, message_cap_move, protocol,
};
use microsystem_console::{INLINE_BYTES, Operation};

const DATA: usize = 0x00;
#[cfg(target_arch = "aarch64")]
const FLAGS: usize = 0x18;
#[cfg(target_arch = "aarch64")]
const RX_EMPTY: u32 = 1 << 4;
#[cfg(target_arch = "riscv64")]
const LINE_STATUS: usize = 0x05;
#[cfg(target_arch = "riscv64")]
const RX_READY: u8 = 1 << 0;
const REVOCATION_PROBE_VA: u64 = 0x0058_0000;

#[unsafe(no_mangle)]
pub extern "C" fn _start(uart: usize) -> ! {
    let _ = microsystem_user_rt::debug_write(b"[user] console service ELF entered EL0\n");
    if uart == 0 {
        microsystem_user_rt::exit(2);
    }
    let mut request = Message::new(protocol::CONSOLE, 0);
    let _ = microsystem_user_rt::debug_write(b"[ipc] resident console endpoint=1 ready\n");
    let _ = microsystem_user_rt::service_online();
    if microsystem_user_rt::ipc_recv(boot_cap::CONSOLE_ENDPOINT, &mut request, 0).is_err() {
        microsystem_user_rt::exit(3);
    }
    let mut deferred_signal = None;
    let mut revocation_probe = None;
    loop {
        let mut reply = Message::new(protocol::CONSOLE, request.opcode);
        reply.words[5] = handle(
            uart,
            &request,
            &mut reply,
            &mut deferred_signal,
            &mut revocation_probe,
        ) as i64 as u64;
        let deadline = deferred_signal
            .map(|(_, deadline, _)| deadline)
            .unwrap_or(0);
        match microsystem_user_rt::ipc_reply_recv(
            boot_cap::CONSOLE_ENDPOINT,
            &reply,
            &mut request,
            deadline,
        ) {
            Ok(()) => {}
            Err(Status::TimedOut) => {
                let Some((notification, _, bits)) = deferred_signal.take() else {
                    microsystem_user_rt::exit(4);
                };
                if microsystem_user_rt::notification_signal(notification, bits).is_err()
                    || microsystem_user_rt::cap_delete(notification).is_err()
                    || microsystem_user_rt::ipc_recv(boot_cap::CONSOLE_ENDPOINT, &mut request, 0)
                        .is_err()
                {
                    microsystem_user_rt::exit(4);
                }
            }
            Err(_) => microsystem_user_rt::exit(4),
        }
    }
}

fn handle(
    uart: usize,
    request: &Message,
    reply: &mut Message,
    deferred_signal: &mut Option<(CapHandle, u64, u64)>,
    revocation_probe: &mut Option<CapHandle>,
) -> Status {
    match request.opcode {
        value if value == Operation::Ping as u16 => {
            if request.words[0] == 0x40 && request.caps[0] != CapHandle::INVALID {
                if let Err(status) = microsystem_user_rt::notification_signal(request.caps[0], 0x1)
                {
                    return status;
                }
                if let Err(status) = microsystem_user_rt::notification_signal(request.caps[0], 0x4)
                {
                    return status;
                }
                reply.words[0] = 0x41;
                reply.caps[0] = request.caps[0];
                reply.flags |= message_cap_move(0);
                return Status::Ok;
            }
            if request.words[0] == 0x42 && request.caps[0] != CapHandle::INVALID {
                if deferred_signal.is_some() {
                    reply.caps[0] = request.caps[0];
                    reply.flags |= message_cap_move(0);
                    return Status::Busy;
                }
                let Ok(now) = microsystem_user_rt::clock_now() else {
                    reply.caps[0] = request.caps[0];
                    reply.flags |= message_cap_move(0);
                    return Status::NotSupported;
                };
                *deferred_signal = Some((request.caps[0], now.saturating_add(50_000_000), 0x8));
                reply.words[0] = 0x43;
                return Status::Ok;
            }
            if request.words[0] == 0x50 && request.caps[0] != CapHandle::INVALID {
                if revocation_probe.is_some() {
                    let _ = microsystem_user_rt::cap_delete(request.caps[0]);
                    return Status::Busy;
                }
                if let Err(status) = microsystem_user_rt::frame_map(
                    request.caps[0],
                    REVOCATION_PROBE_VA,
                    Rights(Rights::READ.0 | Rights::WRITE.0),
                ) {
                    let _ = microsystem_user_rt::cap_delete(request.caps[0]);
                    return status;
                }
                unsafe {
                    (REVOCATION_PROBE_VA as *mut u32).write_volatile(u32::from_le_bytes(*b"RVK!"));
                }
                *revocation_probe = Some(request.caps[0]);
                reply.words[0] = 0x51;
                return Status::Ok;
            }
            if request.words[0] == 0x52 {
                let Some(frame) = revocation_probe.take() else {
                    return Status::Invalid;
                };
                let debug_result = unsafe {
                    microsystem_user_rt::syscall(
                        Syscall::DebugWrite,
                        [REVOCATION_PROBE_VA, 4, 0, 0, 0, 0],
                    )
                };
                if debug_result != Status::Fault as i64
                    || microsystem_user_rt::cap_delete(frame) != Err(Status::BadCapability)
                {
                    return Status::Fault;
                }
                reply.words[0] = 0x53;
                return Status::Ok;
            }
            if request.words[0] == 0x30 && request.caps[0] != CapHandle::INVALID {
                let rights = Rights(Rights::READ.0 | Rights::MAP.0 | Rights::GRANT.0);
                let frame = match microsystem_user_rt::frame_create(request.caps[0], rights) {
                    Ok(frame) => frame,
                    Err(status) => return status,
                };
                reply.words[0] = 0x31;
                reply.caps[0] = request.caps[0];
                reply.caps[1] = frame;
                reply.flags |= message_cap_move(0) | message_cap_move(1);
                return Status::Ok;
            }
            reply.words[0] = request.words[0].wrapping_add(1);
            if request.caps[0] != CapHandle::INVALID {
                reply.caps[0] = request.caps[0];
                reply.flags |= message_cap_move(0);
            }
            Status::Ok
        }
        value if value == Operation::Write as u16 => {
            let length = request.words[0] as usize;
            if length > INLINE_BYTES {
                return Status::Invalid;
            }
            let bytes = words_as_bytes(&request.words[1..5]);
            microsystem_user_rt::console_write_bytes(&bytes[..length])
                .err()
                .unwrap_or(Status::Ok)
        }
        value if value == Operation::Read as u16 => match get(uart) {
            Some(byte) => {
                reply.words[0] = byte as u64;
                Status::Ok
            }
            None => Status::Busy,
        },
        _ => Status::Invalid,
    }
}

fn words_as_bytes(words: &[u64]) -> &[u8] {
    unsafe { core::slice::from_raw_parts(words.as_ptr().cast::<u8>(), words.len() * 8) }
}

fn get(uart: usize) -> Option<u8> {
    #[cfg(target_arch = "aarch64")]
    if read32(uart + FLAGS) & RX_EMPTY != 0 {
        None
    } else {
        Some(unsafe { core::ptr::read_volatile((uart + DATA) as *const u8) })
    }
    #[cfg(target_arch = "riscv64")]
    if read8(uart + LINE_STATUS) & RX_READY == 0 {
        None
    } else {
        Some(read8(uart + DATA))
    }
    #[cfg(target_arch = "x86_64")]
    {
        let _ = uart;
        microsystem_user_rt::debug_read()
    }
}

#[cfg(target_arch = "aarch64")]
fn read32(address: usize) -> u32 {
    unsafe { core::ptr::read_volatile(address as *const u32) }
}

#[cfg(target_arch = "riscv64")]
fn read8(address: usize) -> u8 {
    unsafe { core::ptr::read_volatile(address as *const u8) }
}

#[panic_handler]
fn panic(_info: &PanicInfo<'_>) -> ! {
    microsystem_user_rt::exit(1)
}
