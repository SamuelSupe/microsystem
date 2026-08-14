#![no_std]
#![no_main]

use core::fmt::{self, Write};
use core::panic::PanicInfo;
use microsystem_abi::{Message, Status, SystemStats, boot_cap, gui, protocol, time};
use microsystem_fs::Operation as FsOperation;
use microsystem_shell::{Command, parse};

const INLINE_TEXT_BYTES: usize = 40;
const SHARED_DATA: usize = 0x0061_0000;
const SHARED_BYTES: usize = 4096;
const COMMAND_BYTES: usize = 512;
const SHARED_TEXT_MAGIC: u64 = 0x5445_524d_5348_4152;
const CLEAR_REPLY: u16 = 1;

#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    let _ = microsystem_user_rt::debug_write(b"[user] terminal service ELF entered EL0\n");
    let mut request = Message::new(protocol::GUI, gui::Operation::CreateWindow as u16);
    request.words[..4].copy_from_slice(&[48, 56, 640, 430]);
    let mut reply = Message::new(protocol::GUI, 0);
    if microsystem_user_rt::ipc_call(boot_cap::GUI_TERMINAL_ENDPOINT, &request, &mut reply, 0)
        .is_err()
        || reply.words[0] == 0
    {
        microsystem_user_rt::exit(2);
    }
    let _ = microsystem_user_rt::debug_write(
        b"[gui] terminal window ready interactive=true filesystem=ls,cat,stat,write,mkdir,mv,rm,sync shared-bytes=4096\n",
    );
    let _ = microsystem_user_rt::service_online();

    let mut request = Message::new(0, 0);
    if microsystem_user_rt::ipc_recv(boot_cap::GUI_TERMINAL_EVENTS, &mut request, 0).is_err() {
        microsystem_user_rt::exit(3);
    }
    loop {
        let reply = execute(&request);
        let mut next = Message::new(0, 0);
        if microsystem_user_rt::ipc_reply_recv(
            boot_cap::GUI_TERMINAL_EVENTS,
            &reply,
            &mut next,
            0,
        )
        .is_err()
        {
            microsystem_user_rt::exit(4);
        }
        request = next;
    }
}

fn execute(request: &Message) -> Message {
    let mut reply = Message::new(protocol::GUI, gui::Operation::TerminalCommand as u16);
    if request.protocol != protocol::GUI
        || request.opcode != gui::Operation::TerminalCommand as u16
    {
        pack_text(&mut reply, b"invalid terminal request\n");
        return reply;
    }
    let mut command_bytes = [0u8; COMMAND_BYTES];
    let Some(command) = unpack_command(request, &mut command_bytes) else {
        pack_text(&mut reply, b"invalid command encoding\n");
        return reply;
    };
    let Ok(command) = core::str::from_utf8(command) else {
        pack_text(&mut reply, b"command is not UTF-8\n");
        return reply;
    };
    if command.trim() == "clear" {
        reply.flags = CLEAR_REPLY;
        return reply;
    }
    if let Some(text) = command.trim().strip_prefix("echo ") {
        let mut output = Text::new();
        let _ = writeln!(output, "{}", text);
        pack_output(&mut reply, output.as_bytes());
        return reply;
    }
    match parse(command) {
        Ok(Command::Empty) => {}
        Ok(Command::Help) => pack_output(
            &mut reply,
            b"help uptime ps clear echo <text>\nls [path]  cat <path>  stat <path>\nwrite <path> <text>  mkdir <path>\nmv <source> <destination>  rm <path>  sync\n",
        ),
        Ok(Command::Uptime) => {
            let mut output = Text::new();
            let _ = writeln!(output, "uptime: {} ms", uptime() / 1_000_000);
            pack_output(&mut reply, output.as_bytes());
            let _ = microsystem_user_rt::debug_write(
                b"[gui] terminal command executed name=uptime output=true\n",
            );
        }
        Ok(Command::Ps) => {
            let mut stats = SystemStats::default();
            if microsystem_user_rt::system_stats(boot_cap::SYSTEM_INFO, &mut stats).is_ok() {
                let mut output = Text::new();
                let _ = writeln!(
                    output,
                    "tasks: run={} block={} free={}",
                    stats.runnable_threads, stats.blocked_threads, stats.free_frames
                );
                pack_output(&mut reply, output.as_bytes());
            } else {
                pack_output(&mut reply, b"ps: unavailable\n");
            }
        }
        Ok(Command::Ls(path)) => fs_output(&mut reply, FsOperation::List, path),
        Ok(Command::Cat(path)) => fs_output(&mut reply, FsOperation::Read, path),
        Ok(Command::Stat(path)) => match fs_request(FsOperation::Stat, path, &[]) {
            Ok(stat) if stat.words[0] == 1 => {
                let mut output = Text::new();
                let _ = writeln!(output, "file {} bytes", stat.words[1]);
                pack_output(&mut reply, output.as_bytes());
                report_filesystem_command(b"stat");
            }
            Ok(stat) if stat.words[0] == 2 => {
                let mut output = Text::new();
                let _ = writeln!(output, "directory {} entries", stat.words[1]);
                pack_output(&mut reply, output.as_bytes());
                report_filesystem_command(b"stat");
            }
            Ok(_) => pack_output(&mut reply, b"stat: invalid response\n"),
            Err(status) => pack_fs_error(&mut reply, "stat", status),
        },
        Ok(Command::Write { path, value }) => fs_status(
            &mut reply,
            "write",
            FsOperation::Write,
            path,
            value.as_bytes(),
        ),
        Ok(Command::Mkdir(path)) => {
            fs_status(&mut reply, "mkdir", FsOperation::Mkdir, path, &[])
        }
        Ok(Command::Rename {
            source,
            destination,
        }) => fs_status(
            &mut reply,
            "mv",
            FsOperation::Rename,
            source,
            destination.as_bytes(),
        ),
        Ok(Command::Unlink(path)) => {
            fs_status(&mut reply, "rm", FsOperation::Unlink, path, &[])
        }
        Ok(Command::Sync) => fs_status(&mut reply, "sync", FsOperation::Sync, "/", &[]),
        _ => pack_output(&mut reply, b"unsupported; type help\n"),
    }
    reply
}

fn uptime() -> u64 {
    let request = Message::new(protocol::TIME, time::Operation::Uptime as u16);
    let mut reply = Message::new(protocol::TIME, 0);
    if microsystem_user_rt::ipc_call(boot_cap::TIME_ENDPOINT, &request, &mut reply, 0).is_ok() {
        reply.words[0]
    } else {
        0
    }
}

fn pack_text(message: &mut Message, text: &[u8]) {
    let length = text.len().min(INLINE_TEXT_BYTES);
    message.words[0] = length as u64;
    let bytes = unsafe {
        core::slice::from_raw_parts_mut(message.words[1..].as_mut_ptr().cast::<u8>(), INLINE_TEXT_BYTES)
    };
    bytes.fill(0);
    bytes[..length].copy_from_slice(&text[..length]);
}

fn pack_output(message: &mut Message, text: &[u8]) {
    let length = text.len().min(SHARED_BYTES);
    unsafe {
        core::ptr::copy_nonoverlapping(text.as_ptr(), SHARED_DATA as *mut u8, length);
        core::arch::asm!("dmb ish", options(nostack, preserves_flags));
    }
    message.words[0] = length as u64;
    message.words[4] = SHARED_TEXT_MAGIC;
}

fn unpack_command<'a>(message: &Message, command: &'a mut [u8; COMMAND_BYTES]) -> Option<&'a [u8]> {
    let length = usize::try_from(message.words[0]).ok()?;
    if length > command.len() {
        return None;
    }
    if message.words[4] == SHARED_TEXT_MAGIC {
        unsafe {
            core::arch::asm!("dmb ish", options(nostack, preserves_flags));
            core::ptr::copy_nonoverlapping(SHARED_DATA as *const u8, command.as_mut_ptr(), length);
        }
    } else {
        if length > INLINE_TEXT_BYTES {
            return None;
        }
        let bytes = unsafe {
            core::slice::from_raw_parts(
                message.words[1..].as_ptr().cast::<u8>(),
                INLINE_TEXT_BYTES,
            )
        };
        command[..length].copy_from_slice(&bytes[..length]);
    }
    Some(&command[..length])
}

fn fs_output(reply: &mut Message, operation: FsOperation, path: &str) {
    match fs_request(operation, path, &[]) {
        Ok(response) => {
            let mut length = (response.words[0] as usize).min(SHARED_BYTES);
            if length < SHARED_BYTES {
                unsafe { (SHARED_DATA as *mut u8).add(length).write(b'\n') };
                length += 1;
            }
            unsafe { core::arch::asm!("dmb ish", options(nostack, preserves_flags)) };
            reply.words[0] = length as u64;
            reply.words[4] = SHARED_TEXT_MAGIC;
            report_filesystem_command(match operation {
                FsOperation::List => b"ls",
                _ => b"cat",
            });
        }
        Err(status) => pack_fs_error(
            reply,
            if operation == FsOperation::List {
                "ls"
            } else {
                "cat"
            },
            status,
        ),
    }
}

fn fs_status(
    reply: &mut Message,
    name: &str,
    operation: FsOperation,
    path: &str,
    data: &[u8],
) {
    match fs_request(operation, path, data) {
        Ok(_) => {
            let mut output = Text::new();
            let _ = writeln!(output, "{}: ok", name);
            pack_output(reply, output.as_bytes());
            report_filesystem_command(name.as_bytes());
        }
        Err(status) => pack_fs_error(reply, name, status),
    }
}

fn fs_request(operation: FsOperation, path: &str, data: &[u8]) -> Result<Message, Status> {
    if path.is_empty() || path.len() > 255 || path.len().saturating_add(data.len()) > SHARED_BYTES {
        return Err(Status::Invalid);
    }
    unsafe {
        core::ptr::copy_nonoverlapping(path.as_ptr(), SHARED_DATA as *mut u8, path.len());
        core::ptr::copy_nonoverlapping(
            data.as_ptr(),
            (SHARED_DATA as *mut u8).add(path.len()),
            data.len(),
        );
        core::arch::asm!("dmb ish", options(nostack, preserves_flags));
    }
    let mut request = Message::new(protocol::FILESYSTEM, operation as u16);
    request.words[0] = path.len() as u64;
    request.words[1] = data.len() as u64;
    request.caps[0] = boot_cap::GUI_TERMINAL_COMMANDS;
    let mut reply = Message::new(protocol::FILESYSTEM, 0);
    microsystem_user_rt::ipc_call(
        boot_cap::TERMINAL_FILESYSTEM_ENDPOINT,
        &request,
        &mut reply,
        0,
    )?;
    if reply.protocol != protocol::FILESYSTEM {
        return Err(Status::Io);
    }
    let status = status_from_raw(reply.words[5] as i64);
    if status != Status::Ok {
        return Err(status);
    }
    if matches!(operation, FsOperation::List | FsOperation::Read | FsOperation::ReadRange)
        && reply.words[0] as usize > SHARED_BYTES
    {
        return Err(Status::Io);
    }
    unsafe { core::arch::asm!("dmb ish", options(nostack, preserves_flags)) };
    Ok(reply)
}

fn status_from_raw(status: i64) -> Status {
    match status {
        0 => Status::Ok,
        -1 => Status::Invalid,
        -2 => Status::BadCapability,
        -3 => Status::AccessDenied,
        -4 => Status::NotFound,
        -5 => Status::NoMemory,
        -6 => Status::Busy,
        -7 => Status::TimedOut,
        -8 => Status::Fault,
        -9 => Status::NotSupported,
        -11 => Status::NoSpace,
        -12 => Status::Corrupt,
        _ => Status::Io,
    }
}

fn pack_fs_error(reply: &mut Message, name: &str, status: Status) {
    let reason = match status {
        Status::NotFound => "not found",
        Status::NoSpace => "no space",
        Status::AccessDenied => "access denied",
        Status::Invalid => "invalid path or arguments",
        _ => "failed",
    };
    let mut output = Text::new();
    let _ = writeln!(output, "{}: {}", name, reason);
    pack_output(reply, output.as_bytes());
    let _ = microsystem_user_rt::debug_write(b"[gui] terminal filesystem command=");
    let _ = microsystem_user_rt::debug_write(name.as_bytes());
    let _ = microsystem_user_rt::debug_write(b" status=");
    let _ = microsystem_user_rt::debug_write(reason.as_bytes());
    let _ = microsystem_user_rt::debug_write(b"\n");
}

fn report_filesystem_command(name: &[u8]) {
    let _ = microsystem_user_rt::debug_write(b"[gui] terminal filesystem command=");
    let _ = microsystem_user_rt::debug_write(name);
    let _ = microsystem_user_rt::debug_write(b" status=ok\n");
}

struct Text {
    bytes: [u8; SHARED_BYTES],
    length: usize,
}

impl Text {
    const fn new() -> Self {
        Self { bytes: [0; SHARED_BYTES], length: 0 }
    }

    fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.length]
    }
}

impl Write for Text {
    fn write_str(&mut self, value: &str) -> fmt::Result {
        let available = self.bytes.len().saturating_sub(self.length);
        let length = value.len().min(available);
        self.bytes[self.length..self.length + length].copy_from_slice(&value.as_bytes()[..length]);
        self.length += length;
        Ok(())
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo<'_>) -> ! { microsystem_user_rt::exit(1) }
