#![no_std]
#![no_main]

extern crate alloc;

use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::fmt::{self, Write};
use core::panic::PanicInfo;
use microsystem_abi::{
    FilesystemStatsV1, Message, Status, SystemStats, boot_cap, gui, network, process, protocol,
    time,
};
use microsystem_fs::Operation as FsOperation;
use microsystem_shell::{Command, parse, resolve_path};

const INLINE_TEXT_BYTES: usize = 40;
const SHARED_DATA: usize = 0x0061_0000;
const SHARED_BYTES: usize = 4096;
const COMMAND_BYTES: usize = 512;
const HISTORY_ENTRIES: usize = 32;
const SHARED_TEXT_MAGIC: u64 = 0x5445_524d_5348_4152;
const CLEAR_REPLY: u16 = 1;

struct TerminalState {
    cwd: String,
    history: Vec<String>,
}

impl TerminalState {
    fn new() -> Self {
        Self {
            cwd: "/".to_string(),
            history: Vec::new(),
        }
    }

    fn record(&mut self, command: &str) {
        let command = command.trim();
        if command.is_empty() {
            return;
        }
        if self.history.len() == HISTORY_ENTRIES {
            self.history.remove(0);
        }
        self.history.push(command.to_string());
    }

    fn resolve(&self, path: &str) -> Result<String, Status> {
        let mut output = [0u8; 256];
        resolve_path(&self.cwd, path, &mut output)
            .map(str::to_string)
            .map_err(|_| Status::Invalid)
    }

    fn logout(&mut self) {
        self.cwd.clear();
        self.cwd.push('/');
        self.history.clear();
    }
}

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
        b"[gui] terminal window ready interactive=true extended-shell=true filesystem=ls,cat,stat,touch,cp,write,mkdir,rmdir,mv,rm,fsync,sync shared-bytes=4096\n",
    );
    let _ = microsystem_user_rt::service_online();

    let mut state = TerminalState::new();
    let mut request = Message::new(0, 0);
    if microsystem_user_rt::ipc_recv(boot_cap::GUI_TERMINAL_EVENTS, &mut request, 0).is_err() {
        microsystem_user_rt::exit(3);
    }
    loop {
        let reply = execute(&request, &mut state);
        let mut next = Message::new(0, 0);
        if microsystem_user_rt::ipc_reply_recv(boot_cap::GUI_TERMINAL_EVENTS, &reply, &mut next, 0)
            .is_err()
        {
            microsystem_user_rt::exit(4);
        }
        request = next;
    }
}

fn execute(request: &Message, state: &mut TerminalState) -> Message {
    let mut reply = Message::new(protocol::GUI, gui::Operation::TerminalCommand as u16);
    if request.protocol != protocol::GUI || request.opcode != gui::Operation::TerminalCommand as u16
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
    state.record(command);
    let mut output = Text::new();
    match parse(command) {
        Ok(Command::Empty) => {}
        Ok(Command::Clear) => {
            reply.flags = CLEAR_REPLY;
            return reply;
        }
        Ok(Command::Help) => output.push(
            b"help pwd cd echo clear history ps kill wait uptime sleep date free sysinfo\n\
ls cat head tail wc hexdump grep find tree du df stat touch cp write\n\
mkdir rmdir mv rm fsync sync netstat exit shutdown poweroff reboot\n",
        ),
        Ok(Command::Pwd) => {
            let _ = writeln!(output, "{}", state.cwd);
        }
        Ok(Command::Cd(path)) => match state.resolve(path) {
            Ok(path) => match fs_request(FsOperation::Stat, &path, &[], 0) {
                Ok(stat) if stat.words[0] == 2 => state.cwd = path,
                Ok(_) => output.push(b"cd: not a directory\n"),
                Err(status) => write_status(&mut output, "cd", status),
            },
            Err(status) => write_status(&mut output, "cd", status),
        },
        Ok(Command::Echo(text)) => {
            let _ = writeln!(output, "{}", text);
        }
        Ok(Command::History) => {
            for (index, command) in state.history.iter().enumerate() {
                let _ = writeln!(output, "{:>3}  {}", index + 1, command);
            }
        }
        Ok(Command::Ps) | Ok(Command::SystemInfo) => write_system_info(&mut output),
        Ok(Command::Uptime) => {
            let _ = writeln!(output, "uptime: {} ms", uptime() / 1_000_000);
            let _ = microsystem_user_rt::debug_write(
                b"[gui] terminal command executed name=uptime output=true\n",
            );
        }
        Ok(Command::Date) => write_date(&mut output),
        Ok(Command::Sleep(value)) => match parse_duration_ns(value) {
            Some(duration) if sleep_ns(duration).is_ok() => {}
            Some(_) => output.push(b"sleep: failed\n"),
            None => output.push(b"sleep: invalid duration\n"),
        },
        Ok(Command::Kill { pid, status }) => write_kill(&mut output, pid, status),
        Ok(Command::Wait(pid)) => write_wait(&mut output, pid),
        Ok(Command::Ls(path)) => with_path(state, path, &mut output, |path, output| {
            fs_read_to_text(FsOperation::List, path, output)
        }),
        Ok(Command::Cat(path)) => with_path(state, path, &mut output, fs_cat),
        Ok(Command::Head { path, lines }) => with_path(state, path, &mut output, |path, output| {
            fs_head(path, lines, output)
        }),
        Ok(Command::Tail { path, lines }) => with_path(state, path, &mut output, |path, output| {
            fs_tail(path, lines, output)
        }),
        Ok(Command::Wc(path)) => with_path(state, path, &mut output, fs_wc),
        Ok(Command::Hexdump(path)) => with_path(state, path, &mut output, fs_hexdump),
        Ok(Command::Grep { pattern, path }) => {
            with_path(state, path, &mut output, |path, output| {
                fs_grep(path, pattern, output)
            })
        }
        Ok(Command::Find(path)) => with_path(state, path, &mut output, |path, output| {
            fs_walk(path, false, 0, output)
        }),
        Ok(Command::Tree(path)) => with_path(state, path, &mut output, |path, output| {
            fs_walk(path, true, 0, output)
        }),
        Ok(Command::Du(path)) => {
            with_path(state, path, &mut output, |path, output| {
                match fs_usage(path, 0) {
                    Ok(bytes) => {
                        let _ = writeln!(output, "{}\t{}", bytes, path);
                        Ok(())
                    }
                    Err(status) => Err(status),
                }
            })
        }
        Ok(Command::Df) => fs_df(&mut output),
        Ok(Command::Stat(path)) => with_path(state, path, &mut output, fs_stat),
        Ok(Command::Touch(path)) => with_path(state, path, &mut output, fs_touch),
        Ok(Command::Copy {
            source,
            destination,
            recursive,
        }) => match (state.resolve(source), state.resolve(destination)) {
            (Ok(source), Ok(destination)) => {
                match copy_target(&source, &destination)
                    .and_then(|target| fs_copy(&source, &target, recursive, 0))
                {
                    Ok(()) => {
                        output.push(b"cp: ok\n");
                        report_filesystem_command(b"cp");
                    }
                    Err(status) => write_status(&mut output, "cp", status),
                }
            }
            _ => output.push(b"cp: invalid path\n"),
        },
        Ok(Command::Write { path, value }) => {
            with_path(state, path, &mut output, |path, output| {
                fs_mutation("write", FsOperation::Write, path, value.as_bytes(), output)
            })
        }
        Ok(Command::Mkdir { path, parents }) => {
            with_path(state, path, &mut output, |path, output| {
                match fs_mkdir(path, parents) {
                    Ok(()) => {
                        output.push(b"mkdir: ok\n");
                        report_filesystem_command(b"mkdir");
                    }
                    Err(status) => write_status(output, "mkdir", status),
                }
                Ok(())
            })
        }
        Ok(Command::Rmdir(path)) => with_path(state, path, &mut output, fs_rmdir),
        Ok(Command::Rename {
            source,
            destination,
        }) => match (state.resolve(source), state.resolve(destination)) {
            (Ok(source), Ok(destination)) => {
                let _ = fs_mutation(
                    "mv",
                    FsOperation::Rename,
                    &source,
                    destination.as_bytes(),
                    &mut output,
                );
            }
            _ => output.push(b"mv: invalid path\n"),
        },
        Ok(Command::Unlink { path, recursive }) => {
            with_path(state, path, &mut output, |path, output| {
                match fs_remove(path, recursive, 0) {
                    Ok(()) => {
                        output.push(b"rm: ok\n");
                        report_filesystem_command(b"rm");
                    }
                    Err(status) => write_status(output, "rm", status),
                }
                Ok(())
            })
        }
        Ok(Command::Fsync(path)) => with_path(state, path, &mut output, |path, output| {
            match fs_fsync(path) {
                Ok(()) => {
                    output.push(b"fsync: ok\n");
                    report_filesystem_command(b"fsync");
                }
                Err(status) => write_status(output, "fsync", status),
            }
            Ok(())
        }),
        Ok(Command::Sync) => {
            let _ = fs_mutation("sync", FsOperation::Sync, "/", &[], &mut output);
        }
        Ok(Command::FsHelp) => output.push(
            b"fs: ls cat head tail wc hexdump grep find tree du df stat touch cp [-r]\n\
write mkdir [-p] rmdir mv rm [-r] fsync sync\n",
        ),
        Ok(Command::Netstat) => write_netstat(&mut output),
        Ok(Command::Nslookup(_)) => output.push(b"nslookup: use the serial shell\n"),
        Ok(Command::Curl(_))
        | Ok(Command::Run(_))
        | Ok(Command::MicaEval(_))
        | Ok(Command::MicaFile(_))
        | Ok(Command::MicaRepl)
        | Ok(Command::MicaArgs(_)) => output.push(b"command is available on the serial shell\n"),
        Ok(Command::Exit) => {
            state.logout();
            output.push(b"logout\n");
        }
        Ok(Command::Shutdown | Command::Poweroff) => request_power(false, &mut output),
        Ok(Command::Reboot) => request_power(true, &mut output),
        Err(_) => output.push(b"unknown or invalid command\n"),
    }
    output.finalize();
    pack_output(&mut reply, output.as_bytes());
    reply
}

fn with_path(
    state: &TerminalState,
    path: &str,
    output: &mut Text,
    operation: impl FnOnce(&str, &mut Text) -> Result<(), Status>,
) {
    match state
        .resolve(path)
        .and_then(|path| operation(&path, output))
    {
        Ok(()) => {}
        Err(status) => write_status(output, "filesystem", status),
    }
}

fn write_system_info(output: &mut Text) {
    let mut stats = SystemStats::default();
    if microsystem_user_rt::system_stats(boot_cap::SYSTEM_INFO, &mut stats).is_err() {
        output.push(b"sysinfo: unavailable\n");
        return;
    }
    let _ = writeln!(
        output,
        "cpus={} run={} block={} free_frames={} heap={}/{} irq={} ipc={}",
        stats.cpu_count,
        stats.runnable_threads,
        stats.blocked_threads,
        stats.free_frames,
        stats.kernel_heap_used,
        stats.kernel_heap_total,
        stats.irq_count,
        stats.ipc_calls,
    );
}

fn write_kill(output: &mut Text, pid: &str, status: Option<&str>) {
    let (Ok(pid), Ok(status)) = (pid.parse::<u64>(), status.unwrap_or("-15").parse::<i64>()) else {
        output.push(b"kill: invalid pid or status\n");
        return;
    };
    let mut request = Message::new(protocol::PROCESS, process::Operation::Kill as u16);
    request.words[0] = pid;
    request.words[1] = status as u64;
    let mut reply = Message::new(protocol::PROCESS, 0);
    if microsystem_user_rt::ipc_call(boot_cap::PROCESS_ENDPOINT, &request, &mut reply, 0).is_ok()
        && reply.words[5] as i64 == Status::Ok as i64
    {
        let _ = writeln!(output, "kill: pid={} status={}", pid, status);
    } else {
        output.push(b"kill: failed\n");
    }
}

fn write_wait(output: &mut Text, pid: &str) {
    let Ok(pid) = pid.parse::<u64>() else {
        output.push(b"wait: invalid pid\n");
        return;
    };
    loop {
        let mut request = Message::new(protocol::PROCESS, process::Operation::Wait as u16);
        request.words[0] = pid;
        let mut reply = Message::new(protocol::PROCESS, 0);
        if microsystem_user_rt::ipc_call(boot_cap::PROCESS_ENDPOINT, &request, &mut reply, 0)
            .is_err()
        {
            output.push(b"wait: failed\n");
            return;
        }
        match reply.words[5] as i64 {
            value if value == Status::Ok as i64 => {
                let _ = writeln!(output, "wait: pid={} status={}", pid, reply.words[0] as i64);
                return;
            }
            value if value == Status::Busy as i64 => {
                let _ = microsystem_user_rt::yield_now();
            }
            _ => {
                output.push(b"wait: process not found\n");
                return;
            }
        }
    }
}

fn uptime() -> u64 {
    time_request(time::Operation::Uptime, 0).unwrap_or(0)
}

fn time_request(operation: time::Operation, value: u64) -> Result<u64, Status> {
    let mut request = Message::new(protocol::TIME, operation as u16);
    request.words[0] = value;
    let mut reply = Message::new(protocol::TIME, 0);
    microsystem_user_rt::ipc_call(boot_cap::TIME_ENDPOINT, &request, &mut reply, 0)?;
    if reply.words[5] as i64 == Status::Ok as i64 {
        Ok(reply.words[0])
    } else {
        Err(Status::Io)
    }
}

fn parse_duration_ns(value: &str) -> Option<u64> {
    let split = value
        .bytes()
        .position(|byte| !byte.is_ascii_digit())
        .unwrap_or(value.len());
    let amount = value[..split].parse::<u64>().ok()?;
    let multiplier = match &value[split..] {
        "ns" => 1,
        "us" => 1_000,
        "ms" => 1_000_000,
        "" | "s" => 1_000_000_000,
        "m" => 60_000_000_000,
        _ => return None,
    };
    amount.checked_mul(multiplier)
}

fn sleep_ns(mut duration: u64) -> Result<(), Status> {
    while duration != 0 {
        let chunk = duration.min(1_000_000_000);
        time_request(time::Operation::Sleep, chunk)?;
        duration -= chunk;
    }
    Ok(())
}

fn write_date(output: &mut Text) {
    match microsystem_user_rt::clock_realtime() {
        Ok(seconds) => {
            let days = (seconds / 86_400) as i64;
            let seconds_of_day = seconds % 86_400;
            let (year, month, day) = civil_from_days(days);
            let _ = writeln!(
                output,
                "{:04}-{:02}-{:02} {:02}:{:02}:{:02} UTC",
                year,
                month,
                day,
                seconds_of_day / 3600,
                seconds_of_day / 60 % 60,
                seconds_of_day % 60,
            );
        }
        Err(_) => output.push(b"date: realtime clock unavailable\n"),
    }
}

fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let days = days + 719_468;
    let era = if days >= 0 { days } else { days - 146_096 } / 146_097;
    let doe = days - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let mut year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    year += i64::from(month <= 2);
    (year, month as u32, day as u32)
}

fn write_netstat(output: &mut Text) {
    let request = Message::new(protocol::NETWORK, network::Operation::Stats as u16);
    let mut reply = Message::new(protocol::NETWORK, 0);
    if microsystem_user_rt::ipc_call(boot_cap::NETWORK_ENDPOINT, &request, &mut reply, 0).is_err()
        || reply.words[5] as i64 != Status::Ok as i64
    {
        output.push(b"netstat: unavailable\n");
        return;
    }
    let _ = writeln!(
        output,
        "ipv4={} gateway={} dns={} sessions={} connections={}/{}",
        Ipv4(reply.words[0] as u32),
        Ipv4(reply.words[1] as u32),
        Ipv4(reply.words[2] as u32),
        reply.words[3],
        reply.words[4] as u32,
        (reply.words[4] >> 32) as u32,
    );
}

struct Ipv4(u32);

impl fmt::Display for Ipv4 {
    fn fmt(&self, output: &mut fmt::Formatter<'_>) -> fmt::Result {
        let octets = self.0.to_be_bytes();
        write!(
            output,
            "{}.{}.{}.{}",
            octets[0], octets[1], octets[2], octets[3]
        )
    }
}

fn request_power(reboot: bool, output: &mut Text) {
    if fs_request(FsOperation::Sync, "/", &[], 0).is_err() {
        output.push(b"power: filesystem sync failed\n");
        return;
    }
    let _ = microsystem_user_rt::debug_write(if reboot {
        b"[system] GUI terminal reboot requested\n"
    } else {
        b"[system] GUI terminal poweroff requested\n"
    });
    if microsystem_user_rt::system_power(boot_cap::SHELL_SYSTEM_CONTROL, reboot).is_err() {
        output.push(if reboot {
            b"reboot: failed\n"
        } else {
            b"poweroff: failed\n"
        });
    }
}

fn fs_stat(path: &str, output: &mut Text) -> Result<(), Status> {
    let stat = fs_request(FsOperation::Stat, path, &[], 0)?;
    match stat.words[0] {
        1 => {
            let _ = writeln!(output, "file {} bytes", stat.words[1]);
        }
        2 => {
            let _ = writeln!(output, "directory {} entries", stat.words[1]);
        }
        _ => return Err(Status::Io),
    }
    report_filesystem_command(b"stat");
    Ok(())
}

fn fs_touch(path: &str, output: &mut Text) -> Result<(), Status> {
    match fs_request(FsOperation::Stat, path, &[], 0) {
        Ok(stat) if stat.words[0] == 1 => output.push(b"touch: ok\n"),
        Err(Status::NotFound) => {
            fs_request(FsOperation::Write, path, &[], 0)?;
            output.push(b"touch: ok\n");
        }
        Ok(_) => return Err(Status::Invalid),
        Err(status) => return Err(status),
    }
    report_filesystem_command(b"touch");
    Ok(())
}

fn fs_rmdir(path: &str, output: &mut Text) -> Result<(), Status> {
    let stat = fs_request(FsOperation::Stat, path, &[], 0)?;
    if stat.words[0] != 2 {
        return Err(Status::Invalid);
    }
    fs_mutation("rmdir", FsOperation::Unlink, path, &[], output)
}

fn fs_read_to_text(operation: FsOperation, path: &str, output: &mut Text) -> Result<(), Status> {
    let reply = fs_request(operation, path, &[], 0)?;
    let length = reply.words[0] as usize;
    let bytes = unsafe { core::slice::from_raw_parts(SHARED_DATA as *const u8, length) };
    output.push(bytes);
    output.push(b"\n");
    if operation == FsOperation::List {
        report_filesystem_command(b"ls");
    }
    Ok(())
}

fn fs_cat(path: &str, output: &mut Text) -> Result<(), Status> {
    let size = fs_file_size(path)?;
    let mut offset = 0usize;
    while offset < size && !output.truncated {
        let length = fs_read_range(path, offset)?;
        if length == 0 {
            return Err(Status::Io);
        }
        let bytes = unsafe { core::slice::from_raw_parts(SHARED_DATA as *const u8, length) };
        output.push(bytes);
        offset += length;
    }
    report_filesystem_command(b"cat");
    Ok(())
}

fn fs_head(path: &str, lines: usize, output: &mut Text) -> Result<(), Status> {
    let size = fs_file_size(path)?;
    let mut offset = 0usize;
    let mut remaining = lines;
    while offset < size && remaining != 0 && !output.truncated {
        let length = fs_read_range(path, offset)?;
        if length == 0 {
            return Err(Status::Io);
        }
        let bytes = unsafe { core::slice::from_raw_parts(SHARED_DATA as *const u8, length) };
        let end = bytes
            .iter()
            .enumerate()
            .find_map(|(index, byte)| {
                if *byte == b'\n' {
                    remaining -= 1;
                    (remaining == 0).then_some(index + 1)
                } else {
                    None
                }
            })
            .unwrap_or(length);
        output.push(&bytes[..end]);
        offset += length;
    }
    Ok(())
}

fn fs_tail(path: &str, lines: usize, output: &mut Text) -> Result<(), Status> {
    let size = fs_file_size(path)?;
    if size == 0 {
        return Ok(());
    }
    let trailing = {
        let length = fs_read_range(path, size - 1)?;
        length != 0 && unsafe { *(SHARED_DATA as *const u8) } == b'\n'
    };
    let target = lines.saturating_add(usize::from(trailing));
    let mut found = 0usize;
    let mut cursor = size;
    let mut start = 0usize;
    'search: while cursor != 0 {
        let offset = cursor.saturating_sub(SHARED_BYTES);
        let length = fs_read_range(path, offset)?;
        let bytes = unsafe { core::slice::from_raw_parts(SHARED_DATA as *const u8, length) };
        for index in (0..length.min(cursor - offset)).rev() {
            if bytes[index] == b'\n' {
                found += 1;
                if found == target {
                    start = offset + index + 1;
                    break 'search;
                }
            }
        }
        cursor = offset;
    }
    let mut offset = start;
    while offset < size && !output.truncated {
        let length = fs_read_range(path, offset)?.min(size - offset);
        if length == 0 {
            break;
        }
        let bytes = unsafe { core::slice::from_raw_parts(SHARED_DATA as *const u8, length) };
        output.push(bytes);
        offset += length;
    }
    Ok(())
}

fn fs_wc(path: &str, output: &mut Text) -> Result<(), Status> {
    let size = fs_file_size(path)?;
    let mut offset = 0usize;
    let mut lines = 0usize;
    let mut words = 0usize;
    let mut in_word = false;
    while offset < size {
        let length = fs_read_range(path, offset)?;
        if length == 0 {
            break;
        }
        let bytes = unsafe { core::slice::from_raw_parts(SHARED_DATA as *const u8, length) };
        for byte in bytes {
            lines += usize::from(*byte == b'\n');
            let whitespace = byte.is_ascii_whitespace();
            words += usize::from(!whitespace && !in_word);
            in_word = !whitespace;
        }
        offset += length;
    }
    let _ = writeln!(output, "{} {} {} {}", lines, words, size, path);
    Ok(())
}

fn fs_hexdump(path: &str, output: &mut Text) -> Result<(), Status> {
    let size = fs_file_size(path)?;
    let mut offset = 0usize;
    while offset < size && !output.truncated {
        let length = fs_read_range(path, offset)?;
        if length == 0 {
            break;
        }
        let bytes = unsafe { core::slice::from_raw_parts(SHARED_DATA as *const u8, length) };
        for line in bytes.chunks(16) {
            let _ = write!(output, "{:08x}  ", offset);
            for byte in line {
                let _ = write!(output, "{:02x} ", byte);
            }
            output.push(b"\n");
            offset += line.len();
            if output.truncated {
                break;
            }
        }
    }
    Ok(())
}

fn fs_grep(path: &str, pattern: &str, output: &mut Text) -> Result<(), Status> {
    let size = fs_file_size(path)?;
    let mut offset = 0usize;
    let mut line = Vec::new();
    while offset < size && !output.truncated {
        let length = fs_read_range(path, offset)?;
        if length == 0 {
            break;
        }
        let bytes = unsafe { core::slice::from_raw_parts(SHARED_DATA as *const u8, length) };
        for byte in bytes {
            if line.len() == 32 * 1024 {
                return Err(Status::NoMemory);
            }
            line.push(*byte);
            if *byte == b'\n' {
                push_matching_line(output, &line, pattern.as_bytes());
                line.clear();
            }
        }
        offset += length;
    }
    if !line.is_empty() {
        push_matching_line(output, &line, pattern.as_bytes());
    }
    Ok(())
}

fn push_matching_line(output: &mut Text, line: &[u8], pattern: &[u8]) {
    if pattern.is_empty() || line.windows(pattern.len()).any(|window| window == pattern) {
        output.push(line);
        if line.last() != Some(&b'\n') {
            output.push(b"\n");
        }
    }
}

fn fs_walk(path: &str, tree: bool, depth: usize, output: &mut Text) -> Result<(), Status> {
    if depth > 32 {
        return Err(Status::Invalid);
    }
    if tree {
        for _ in 0..depth {
            output.push(b"  ");
        }
        let _ = writeln!(output, "{}", basename(path).unwrap_or("/"));
    } else {
        let _ = writeln!(output, "{}", path);
    }
    if output.truncated {
        return Ok(());
    }
    let stat = fs_request(FsOperation::Stat, path, &[], 0)?;
    if stat.words[0] == 2 {
        for child in fs_list(path)? {
            fs_walk(&child, tree, depth + 1, output)?;
            if output.truncated {
                break;
            }
        }
    }
    Ok(())
}

fn fs_usage(path: &str, depth: usize) -> Result<u64, Status> {
    if depth > 32 {
        return Err(Status::Invalid);
    }
    let stat = fs_request(FsOperation::Stat, path, &[], 0)?;
    if stat.words[0] == 1 {
        return Ok(stat.words[1]);
    }
    let mut total = 0u64;
    for child in fs_list(path)? {
        total = total.saturating_add(fs_usage(&child, depth + 1)?);
    }
    Ok(total)
}

fn fs_df(output: &mut Text) {
    let Ok(reply) = fs_request(FsOperation::Stats, "/", &[], 0) else {
        output.push(b"df: unavailable\n");
        return;
    };
    if reply.words[0] as usize != core::mem::size_of::<FilesystemStatsV1>() {
        output.push(b"df: invalid response\n");
        return;
    }
    let stats = unsafe { core::ptr::read_unaligned(SHARED_DATA as *const FilesystemStatsV1) };
    if stats.version != 1 {
        output.push(b"df: unsupported version\n");
        return;
    }
    let _ = writeln!(
        output,
        "blocks={} used={} free={} block_size={} entries={}",
        stats.total_blocks, stats.used_blocks, stats.free_blocks, stats.block_size, stats.entries,
    );
}

fn fs_copy(source: &str, destination: &str, recursive: bool, depth: usize) -> Result<(), Status> {
    if depth > 32 || source == destination {
        return Err(Status::Invalid);
    }
    let stat = fs_request(FsOperation::Stat, source, &[], 0)?;
    if stat.words[0] == 1 {
        return fs_copy_file(source, destination);
    }
    if stat.words[0] != 2 || !recursive {
        return Err(Status::Invalid);
    }
    if destination
        .strip_prefix(source)
        .is_some_and(|suffix| suffix.starts_with('/'))
    {
        return Err(Status::Invalid);
    }
    fs_mkdir(destination, true)?;
    for child in fs_list(source)? {
        let child_destination = join_path(destination, basename(&child).ok_or(Status::Invalid)?)?;
        fs_copy(&child, &child_destination, true, depth + 1)?;
    }
    Ok(())
}

fn fs_copy_file(source: &str, destination: &str) -> Result<(), Status> {
    let temporary = copy_temporary_path(destination)?;
    match fs_request(FsOperation::Stat, &temporary, &[], 0) {
        Err(Status::NotFound) => {}
        Ok(_) => return Err(Status::Busy),
        Err(status) => return Err(status),
    }
    let copied = fs_copy_file_contents(source, &temporary)
        .and_then(|()| fs_fsync(&temporary))
        .and_then(|()| {
            fs_request(FsOperation::Replace, &temporary, destination.as_bytes(), 0).map(|_| ())
        });
    if copied.is_err() {
        let _ = fs_request(FsOperation::Unlink, &temporary, &[], 0);
    }
    copied
}

fn fs_copy_file_contents(source: &str, destination: &str) -> Result<(), Status> {
    let size = fs_file_size(source)?;
    fs_request(FsOperation::Write, destination, &[], 0)?;
    let mut data = [0u8; SHARED_BYTES];
    let mut offset = 0usize;
    while offset < size {
        let length = fs_read_range(source, offset)?;
        if length == 0 {
            return Err(Status::Io);
        }
        unsafe {
            core::ptr::copy_nonoverlapping(SHARED_DATA as *const u8, data.as_mut_ptr(), length)
        };
        let maximum = SHARED_BYTES.saturating_sub(destination.len());
        if maximum == 0 {
            return Err(Status::Invalid);
        }
        let mut written = 0usize;
        while written < length {
            let chunk = (length - written).min(maximum);
            fs_request(
                FsOperation::WriteRange,
                destination,
                &data[written..written + chunk],
                (offset + written) as u64,
            )?;
            written += chunk;
        }
        offset += length;
    }
    Ok(())
}

fn copy_temporary_path(destination: &str) -> Result<String, Status> {
    let parent = destination
        .rsplit_once('/')
        .map(|(parent, _)| if parent.is_empty() { "/" } else { parent })
        .ok_or(Status::Invalid)?;
    let name = alloc::format!(".cp-{:016x}", microsystem_user_rt::clock_now().unwrap_or(0));
    join_path(parent, &name)
}

fn copy_target(source: &str, destination: &str) -> Result<String, Status> {
    match fs_request(FsOperation::Stat, destination, &[], 0) {
        Ok(stat) if stat.words[0] == 2 => {
            join_path(destination, basename(source).ok_or(Status::Invalid)?)
        }
        Ok(_) | Err(Status::NotFound) => Ok(destination.to_string()),
        Err(status) => Err(status),
    }
}

fn fs_mkdir(path: &str, parents: bool) -> Result<(), Status> {
    if !parents {
        return fs_request(FsOperation::Mkdir, path, &[], 0).map(|_| ());
    }
    if path == "/" {
        return Ok(());
    }
    let mut current = String::new();
    for component in path.split('/').filter(|component| !component.is_empty()) {
        current.push('/');
        current.push_str(component);
        match fs_request(FsOperation::Stat, &current, &[], 0) {
            Ok(stat) if stat.words[0] == 2 => {}
            Err(Status::NotFound) => {
                fs_request(FsOperation::Mkdir, &current, &[], 0)?;
            }
            _ => return Err(Status::Invalid),
        }
    }
    Ok(())
}

fn fs_remove(path: &str, recursive: bool, depth: usize) -> Result<(), Status> {
    if path == "/" || depth > 32 {
        return Err(Status::AccessDenied);
    }
    let stat = fs_request(FsOperation::Stat, path, &[], 0)?;
    if stat.words[0] == 2 {
        if !recursive {
            return Err(Status::Invalid);
        }
        let children = fs_list(path)?;
        for child in children {
            fs_remove(&child, true, depth + 1)?;
        }
    }
    fs_request(FsOperation::Unlink, path, &[], 0).map(|_| ())
}

fn fs_list(path: &str) -> Result<Vec<String>, Status> {
    let reply = fs_request(FsOperation::List, path, &[], 0)?;
    let bytes =
        unsafe { core::slice::from_raw_parts(SHARED_DATA as *const u8, reply.words[0] as usize) };
    let text = core::str::from_utf8(bytes).map_err(|_| Status::Io)?;
    Ok(text
        .lines()
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect())
}

fn fs_file_size(path: &str) -> Result<usize, Status> {
    let stat = fs_request(FsOperation::Stat, path, &[], 0)?;
    if stat.words[0] != 1 {
        return Err(Status::Invalid);
    }
    usize::try_from(stat.words[1]).map_err(|_| Status::Invalid)
}

fn fs_read_range(path: &str, offset: usize) -> Result<usize, Status> {
    let reply = fs_request(FsOperation::ReadRange, path, &[], offset as u64)?;
    usize::try_from(reply.words[0]).map_err(|_| Status::Io)
}

fn fs_mutation(
    name: &str,
    operation: FsOperation,
    path: &str,
    data: &[u8],
    output: &mut Text,
) -> Result<(), Status> {
    match fs_request(operation, path, data, 0) {
        Ok(_) => {
            let _ = writeln!(output, "{}: ok", name);
            report_filesystem_command(name.as_bytes());
            Ok(())
        }
        Err(status) => {
            write_status(output, name, status);
            Err(status)
        }
    }
}

fn fs_fsync(path: &str) -> Result<(), Status> {
    let descriptor = fs_request(FsOperation::Open, path, &[], 0)?.words[0];
    if descriptor == 0 {
        return Err(Status::Io);
    }
    let synced = fs_request(FsOperation::Fsync, "", &[], descriptor).map(|_| ());
    let closed = fs_request(FsOperation::Close, "", &[], descriptor).map(|_| ());
    synced.and(closed)
}

fn fs_request(
    operation: FsOperation,
    path: &str,
    data: &[u8],
    word2: u64,
) -> Result<Message, Status> {
    if path.len() > 255 || path.len().saturating_add(data.len()) > SHARED_BYTES {
        return Err(Status::Invalid);
    }
    if path.is_empty() && word2 == 0 {
        return Err(Status::Invalid);
    }
    unsafe {
        core::ptr::copy_nonoverlapping(path.as_ptr(), SHARED_DATA as *mut u8, path.len());
        core::ptr::copy_nonoverlapping(
            data.as_ptr(),
            (SHARED_DATA as *mut u8).add(path.len()),
            data.len(),
        );
        microsystem_user_rt::fence();
    }
    let mut request = Message::new(protocol::FILESYSTEM, operation as u16);
    request.words[0] = path.len() as u64;
    request.words[1] = data.len() as u64;
    request.words[2] = word2;
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
    let returns_shared_bytes = matches!(
        operation,
        FsOperation::List | FsOperation::Read | FsOperation::ReadRange | FsOperation::Stats
    );
    if status != Status::Ok || (returns_shared_bytes && reply.words[0] as usize > SHARED_BYTES) {
        return Err(if status == Status::Ok {
            Status::Io
        } else {
            status
        });
    }
    microsystem_user_rt::fence();
    Ok(reply)
}

fn basename(path: &str) -> Option<&str> {
    (path != "/")
        .then(|| path.rsplit('/').find(|part| !part.is_empty()))
        .flatten()
}

fn join_path(parent: &str, name: &str) -> Result<String, Status> {
    let separator = if parent == "/" { "" } else { "/" };
    if name.is_empty() || name.contains('/') || parent.len() + separator.len() + name.len() > 255 {
        return Err(Status::Invalid);
    }
    let mut path = String::from(parent);
    path.push_str(separator);
    path.push_str(name);
    Ok(path)
}

fn write_status(output: &mut Text, name: &str, status: Status) {
    let reason = match status {
        Status::NotFound => "not found",
        Status::NoSpace => "no space",
        Status::AccessDenied => "access denied",
        Status::Busy => "busy",
        Status::Invalid => "invalid path or arguments",
        _ => "failed",
    };
    let _ = writeln!(output, "{}: {}", name, reason);
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

fn pack_text(message: &mut Message, text: &[u8]) {
    let length = text.len().min(INLINE_TEXT_BYTES);
    message.words[0] = length as u64;
    let bytes = unsafe {
        core::slice::from_raw_parts_mut(
            message.words[1..].as_mut_ptr().cast::<u8>(),
            INLINE_TEXT_BYTES,
        )
    };
    bytes.fill(0);
    bytes[..length].copy_from_slice(&text[..length]);
}

fn pack_output(message: &mut Message, text: &[u8]) {
    let length = text.len().min(SHARED_BYTES);
    unsafe {
        core::ptr::copy_nonoverlapping(text.as_ptr(), SHARED_DATA as *mut u8, length);
        microsystem_user_rt::fence();
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
            microsystem_user_rt::fence();
            core::ptr::copy_nonoverlapping(SHARED_DATA as *const u8, command.as_mut_ptr(), length);
        }
    } else {
        if length > INLINE_TEXT_BYTES {
            return None;
        }
        let bytes = unsafe {
            core::slice::from_raw_parts(message.words[1..].as_ptr().cast::<u8>(), INLINE_TEXT_BYTES)
        };
        command[..length].copy_from_slice(&bytes[..length]);
    }
    Some(&command[..length])
}

fn report_filesystem_command(name: &[u8]) {
    let _ = microsystem_user_rt::debug_write(b"[gui] terminal filesystem command=");
    let _ = microsystem_user_rt::debug_write(name);
    let _ = microsystem_user_rt::debug_write(b" status=ok\n");
}

struct Text {
    bytes: [u8; SHARED_BYTES],
    length: usize,
    truncated: bool,
}

impl Text {
    const MARKER: &'static [u8] = b"\n[output truncated]\n";

    const fn new() -> Self {
        Self {
            bytes: [0; SHARED_BYTES],
            length: 0,
            truncated: false,
        }
    }

    fn push(&mut self, value: &[u8]) {
        let available = self.bytes.len().saturating_sub(self.length);
        let length = value.len().min(available);
        self.bytes[self.length..self.length + length].copy_from_slice(&value[..length]);
        self.length += length;
        self.truncated |= length != value.len();
    }

    fn finalize(&mut self) {
        if !self.truncated {
            return;
        }
        let start = SHARED_BYTES - Self::MARKER.len();
        self.bytes[start..].copy_from_slice(Self::MARKER);
        self.length = SHARED_BYTES;
    }

    fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.length]
    }
}

impl Write for Text {
    fn write_str(&mut self, value: &str) -> fmt::Result {
        self.push(value.as_bytes());
        Ok(())
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo<'_>) -> ! {
    microsystem_user_rt::exit(1)
}
