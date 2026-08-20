#![no_std]
#![no_main]

extern crate alloc;

use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::fmt::{self, Write};
use core::panic::PanicInfo;
use microsystem_abi::{
    FilesystemStatsV1, Message, Rights, Status, SystemStats, boot_cap, network, process, protocol,
    script, time,
};
use microsystem_console::{INLINE_BYTES, Operation as ConsoleOperation};
use microsystem_fs::Operation as FsOperation;
use microsystem_shell::{Command, parse, resolve_path};

const SHARED_DATA: usize = 0x005e_0000;
const SHARED_BYTES: usize = 4096;
const HISTORY_ENTRIES: usize = 32;
const HISTORY_COMMAND_BYTES: usize = 256;

struct ShellState {
    cwd: String,
    history: Vec<String>,
}

impl ShellState {
    fn new() -> Self {
        Self {
            cwd: "/".to_string(),
            history: Vec::new(),
        }
    }

    fn record(&mut self, line: &str) {
        let line = line.trim();
        if line.is_empty() {
            return;
        }
        if self.history.len() == HISTORY_ENTRIES {
            self.history.remove(0);
        }
        let mut end = line.len().min(HISTORY_COMMAND_BYTES);
        while !line.is_char_boundary(end) {
            end -= 1;
        }
        self.history.push(line[..end].to_string());
    }

    fn resolve(&self, path: &str) -> Result<String, ()> {
        let mut output = [0u8; 256];
        resolve_path(&self.cwd, path, &mut output)
            .map(str::to_string)
            .map_err(|_| ())
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    let _ = microsystem_user_rt::debug_write(b"[user] shell service ELF entered EL0\n");
    let request = Message::new(protocol::FILESYSTEM, FsOperation::Stat as u16);
    let mut reply = Message::new(protocol::FILESYSTEM, 0);
    if microsystem_user_rt::ipc_call(boot_cap::FILESYSTEM_ENDPOINT, &request, &mut reply, 0)
        .is_err()
        || reply.protocol != protocol::FILESYSTEM
        || reply.words[0] != 0x4d46_5331
    {
        microsystem_user_rt::exit(2);
    }
    let _ = microsystem_user_rt::debug_write(b"[ipc] resident shell->mfs magic=MFS1\n");
    if verify_filesystem_protocol().is_err() {
        microsystem_user_rt::exit(6);
    }
    let _ = microsystem_user_rt::debug_write(
        b"[ipc] filesystem open/read/write/fsync/sync/stat/readdir/mkdir/rename/unlink protocol=true\n",
    );
    if verify_time_protocol().is_err() {
        microsystem_user_rt::exit(7);
    }
    let _ = microsystem_user_rt::debug_write(
        b"[ipc] resident time sleep/uptime endpoint=4 shared-procman=true\n",
    );
    let _ = microsystem_user_rt::service_online();

    let start = microsystem_user_rt::clock_now().unwrap_or(0);
    while microsystem_user_rt::clock_now().unwrap_or(start) < start.saturating_add(100_000_000) {
        let _ = microsystem_user_rt::yield_now();
    }
    console_write(b"[service] shell ready\n");

    let mut line = [0u8; 4096];
    let mut length = 0usize;
    let mut state = ShellState::new();
    print_prompt(&state);
    loop {
        match console_read() {
            Ok(Some(b'\r' | b'\n')) => {
                console_write(b"\n");
                if let Ok(command) = core::str::from_utf8(&line[..length]) {
                    state.record(command);
                    execute(command, &mut state);
                }
                length = 0;
                print_prompt(&state);
            }
            Ok(Some(8 | 127)) if length > 0 => {
                length -= 1;
                console_write(b"\x08 \x08");
            }
            Ok(Some(byte @ 0x20..=0x7e)) if length < line.len() => {
                line[length] = byte;
                length += 1;
                console_write(&[byte]);
            }
            Ok(Some(_)) | Ok(None) => {
                let _ = microsystem_user_rt::yield_now();
            }
            Err(_) => microsystem_user_rt::exit(3),
        }
    }
}

fn print_prompt(_state: &ShellState) {
    console_write(b"micro> ");
}

fn execute(line: &str, state: &mut ShellState) {
    match parse(line) {
        Ok(Command::Empty) => {}
        Ok(Command::Help) => console_write(
            b"shell: help pwd cd echo clear history ps kill wait uptime sleep date free sysinfo\n\
files: ls cat head tail wc hexdump xxd grep find tree du df stat touch cp write\n\
       mkdir rmdir mv rm fsync sync (also available through fs <command>)\n\
network: curl nslookup netstat\nprograms: run mica\nsystem: exit shutdown poweroff reboot\n\
options: head/tail -n N, mkdir -p, cp -r, rm -r, curl [-s] [-i] [-o PATH] URL\n",
        ),
        Ok(Command::Pwd) => output(format_args!("{}\n", state.cwd)),
        Ok(Command::Cd(path)) => {
            let Some(path) = resolve_command_path(state, "cd", path) else {
                return;
            };
            match fs_request(FsOperation::Stat, &path, &[], 0) {
                Ok(reply) if reply.words[0] == 2 => state.cwd = path,
                Ok(_) => console_write(b"cd: not a directory\n"),
                Err(_) => console_write(b"cd: not found\n"),
            }
        }
        Ok(Command::Echo(text)) => output(format_args!("{}\n", text)),
        Ok(Command::Clear) => console_write(b"\x1b[2J\x1b[H"),
        Ok(Command::History) => {
            for (index, command) in state.history.iter().enumerate() {
                output(format_args!("{:>3}  {}\n", index + 1, command));
            }
        }
        Ok(Command::Ps) => {
            let cpu0 = microsystem_user_rt::timer_ticks(0).unwrap_or(0);
            let cpu1 = microsystem_user_rt::timer_ticks(1).unwrap_or(0);
            let shell_cpu = microsystem_user_rt::current_cpu().unwrap_or(0);
            output(format_args!(
                "PID 1 init resident cpu=shared\nPID 2 console resident cpu=shared\nPID 3 block resident cpu=shared\nPID 4 mfs resident cpu=shared\nPID 5 shell running cpu={}\nPID 6 devmgr resident cpu=shared ticks=[{},{}]\n",
                shell_cpu, cpu0, cpu1
            ));
            let mut cursor = 7;
            while cursor != 0 {
                let mut request = Message::new(protocol::PROCESS, process::Operation::List as u16);
                request.words[0] = cursor;
                let mut reply = Message::new(protocol::PROCESS, 0);
                if microsystem_user_rt::ipc_call(
                    boot_cap::PROCESS_ENDPOINT,
                    &request,
                    &mut reply,
                    0,
                )
                .is_err()
                    || reply.words[5] as i64 != 0
                    || reply.words[0] == 0
                {
                    break;
                }
                let pid = reply.words[0];
                let name = program_name(reply.words[4]).unwrap_or("application");
                if reply.words[1] != 0 {
                    output(format_args!("PID {} {} running cpu=shared\n", pid, name));
                } else {
                    output(format_args!(
                        "PID {} {} exited status={}\n",
                        pid, name, reply.words[2] as i64
                    ));
                }
                cursor = reply.words[3];
            }
        }
        Ok(Command::Uptime) => {
            let milliseconds = time_request(time::Operation::Uptime, 0).unwrap_or(0) / 1_000_000;
            output(format_args!("uptime: {} ms\n", milliseconds));
        }
        Ok(Command::Date) => print_date(),
        Ok(Command::SystemInfo) => print_system_info(),
        Ok(Command::Sleep(duration)) => sleep_command(duration),
        Ok(Command::Kill { pid, status }) => kill_command(pid, status),
        Ok(Command::Wait(pid)) => wait_command(pid),
        Ok(Command::Ls(path)) => {
            if let Some(path) = resolve_command_path(state, "ls", path) {
                fs_print(FsOperation::List, &path);
            }
        }
        Ok(Command::Cat(path)) => {
            if let Some(path) = resolve_command_path(state, "cat", path) {
                fs_cat(&path);
            }
        }
        Ok(Command::Head { path, lines }) => {
            if let Some(path) = resolve_command_path(state, "head", path) {
                fs_head(&path, lines);
            }
        }
        Ok(Command::Tail { path, lines }) => {
            if let Some(path) = resolve_command_path(state, "tail", path) {
                fs_tail(&path, lines);
            }
        }
        Ok(Command::Wc(path)) => {
            if let Some(path) = resolve_command_path(state, "wc", path) {
                fs_wc(&path);
            }
        }
        Ok(Command::Hexdump(path)) => {
            if let Some(path) = resolve_command_path(state, "hexdump", path) {
                fs_hexdump(&path);
            }
        }
        Ok(Command::Grep { pattern, path }) => {
            if let Some(path) = resolve_command_path(state, "grep", path) {
                fs_grep(pattern, &path);
            }
        }
        Ok(Command::Find(path)) => {
            if let Some(path) = resolve_command_path(state, "find", path) {
                fs_find(&path, false);
            }
        }
        Ok(Command::Tree(path)) => {
            if let Some(path) = resolve_command_path(state, "tree", path) {
                fs_find(&path, true);
            }
        }
        Ok(Command::Du(path)) => {
            if let Some(path) = resolve_command_path(state, "du", path) {
                fs_du(&path);
            }
        }
        Ok(Command::Df) => fs_df(),
        Ok(Command::Stat(path)) => {
            let Some(path) = resolve_command_path(state, "stat", path) else {
                return;
            };
            match fs_request(FsOperation::Stat, &path, &[], 0) {
                Ok(reply) if reply.words[0] == 1 => {
                    output(format_args!("file {} bytes\n", reply.words[1]));
                }
                Ok(reply) if reply.words[0] == 2 => {
                    output(format_args!("directory {} entries\n", reply.words[1]));
                }
                _ => console_write(b"stat: failed\n"),
            }
        }
        Ok(Command::Touch(path)) => {
            let Some(path) = resolve_command_path(state, "touch", path) else {
                return;
            };
            match fs_request(FsOperation::Stat, &path, &[], 0) {
                Ok(reply) if reply.words[0] == 1 => console_write(b"touch: ok\n"),
                Ok(reply) if reply.words[0] == 2 => console_write(b"touch: failed\n"),
                Err(Status::NotFound) => {
                    if fs_call(FsOperation::Write, &path, &[]).is_ok() {
                        console_write(b"touch: ok\n");
                    } else {
                        console_write(b"touch: failed\n");
                    }
                }
                _ => console_write(b"touch: failed\n"),
            }
        }
        Ok(Command::Copy {
            source,
            destination,
            recursive,
        }) => {
            let Some(source) = resolve_command_path(state, "cp", source) else {
                return;
            };
            let Some(destination) = resolve_command_path(state, "cp", destination) else {
                return;
            };
            fs_copy(&source, &destination, recursive);
        }
        Ok(Command::Write { path, value }) => {
            let Some(path) = resolve_command_path(state, "write", path) else {
                return;
            };
            if fs_call(FsOperation::Write, &path, value.as_bytes()).is_ok() {
                console_write(b"write: ok\n");
            } else {
                console_write(b"write: failed\n");
            }
        }
        Ok(Command::Mkdir { path, parents }) => {
            let Some(path) = resolve_command_path(state, "mkdir", path) else {
                return;
            };
            if fs_mkdir(&path, parents).is_ok() {
                console_write(b"mkdir: ok\n");
            } else {
                console_write(b"mkdir: failed\n");
            }
        }
        Ok(Command::Rmdir(path)) => {
            let Some(path) = resolve_command_path(state, "rmdir", path) else {
                return;
            };
            match fs_request(FsOperation::Stat, &path, &[], 0) {
                Ok(reply) if reply.words[0] == 2 => {
                    if fs_call(FsOperation::Unlink, &path, &[]).is_ok() {
                        console_write(b"rmdir: ok\n");
                    } else {
                        console_write(b"rmdir: failed\n");
                    }
                }
                _ => console_write(b"rmdir: failed\n"),
            }
        }
        Ok(Command::Rename {
            source,
            destination,
        }) => {
            let Some(source) = resolve_command_path(state, "mv", source) else {
                return;
            };
            let Some(destination) = resolve_command_path(state, "mv", destination) else {
                return;
            };
            if fs_call(FsOperation::Rename, &source, destination.as_bytes()).is_ok() {
                console_write(b"mv: ok\n");
            } else {
                console_write(b"mv: failed\n");
            }
        }
        Ok(Command::Unlink { path, recursive }) => {
            let Some(path) = resolve_command_path(state, "rm", path) else {
                return;
            };
            if fs_remove(&path, recursive).is_ok() {
                console_write(b"rm: ok\n");
            } else {
                console_write(b"rm: failed\n");
            }
        }
        Ok(Command::Fsync(path)) => {
            let Some(path) = resolve_command_path(state, "fsync", path) else {
                return;
            };
            if fs_fsync(&path).is_ok() {
                console_write(b"fsync: ok\n");
            } else {
                console_write(b"fsync: failed\n");
            }
        }
        Ok(Command::FsHelp) => console_write(
            b"fs: ls/list [path], cat/read <path>, head/tail [-n N] <path>, wc/hexdump/grep,\n\
find/tree/du/df, stat, touch/create, cp [-r], write, mkdir [-p], rmdir,\n\
mv/rename, rm/remove/unlink [-r], fsync, sync\n",
        ),
        Ok(Command::Curl(arguments)) => run_curl(arguments, state),
        Ok(Command::Nslookup(host)) => run_nslookup(host),
        Ok(Command::Netstat) => run_netstat(),
        Ok(Command::Run(path)) => {
            let mut request = Message::new(protocol::PROCESS, process::Operation::Spawn as u16);
            request.words[0] = program_id(path);
            let mut reply = Message::new(protocol::PROCESS, 0);
            if microsystem_user_rt::ipc_call(boot_cap::PROCESS_ENDPOINT, &request, &mut reply, 0)
                .is_ok()
                && reply.words[5] as i64 == 0
            {
                output(format_args!("run: {} pid={}\n", path, reply.words[0]));
                if path == "spinner" {
                    output_loader(path, reply.words[0]);
                } else if path == "resourcekill" {
                    if wait_for_resources(reply.words[0], 2, 1, 2) {
                        output(format_args!(
                            "[mm] application live resources pid={} frames=2 pools=1 mappings=2\n",
                            reply.words[0]
                        ));
                        if kill_process(reply.words[0], -15)
                            && let Some(wait_reply) = wait_process(reply.words[0])
                        {
                            output(format_args!(
                                "kill: pid={} status={}\n",
                                reply.words[0], wait_reply.words[0] as i64
                            ));
                            report_completion(path, reply.words[0], &wait_reply);
                        }
                    }
                } else {
                    if let Some(wait_reply) = wait_process(reply.words[0]) {
                        report_completion(path, reply.words[0], &wait_reply);
                    }
                }
            } else {
                output(format_args!("run: {}: not found\n", path));
            }
        }
        Ok(Command::MicaEval(source)) => run_mica(
            source.as_bytes(),
            b"",
            b"",
            "",
            10_000_000_000,
            script::MODE_EVAL,
            false,
        ),
        Ok(Command::MicaFile(path)) => {
            if let Some(path) = resolve_command_path(state, "mica", path) {
                run_mica_file(&path, b"", b"", 10_000_000_000, false);
            }
        }
        Ok(Command::MicaRepl) => run_mica_repl(b"", 5_000_000_000),
        Ok(Command::MicaArgs(arguments)) => run_mica_arguments(arguments),
        Ok(Command::Sync) => {
            if fs_call(FsOperation::Sync, "/", &[]).is_ok() {
                console_write(b"sync: ok\n");
            } else {
                console_write(b"sync: failed\n");
            }
        }
        Ok(Command::Exit) => {
            state.cwd.clear();
            state.cwd.push('/');
            state.history.clear();
            console_write(b"logout\n");
        }
        Ok(Command::Shutdown | Command::Poweroff) => request_system_power(false),
        Ok(Command::Reboot) => request_system_power(true),
        Err(_) => console_write(b"unknown or invalid command\n"),
    }
}

fn resolve_command_path(state: &ShellState, command: &str, path: &str) -> Option<String> {
    match state.resolve(path) {
        Ok(path) => Some(path),
        Err(()) => {
            output(format_args!("{}: invalid path\n", command));
            None
        }
    }
}

fn sleep_command(value: &str) {
    let Some(duration) = parse_duration_ns(value) else {
        console_write(b"sleep: duration must be an integer followed by ns, us, ms, s, or m\n");
        return;
    };
    let mut remaining = duration;
    while remaining != 0 {
        let chunk = remaining.min(1_000_000_000);
        if time_request(time::Operation::Sleep, chunk).is_err() {
            console_write(b"sleep: failed\n");
            return;
        }
        remaining -= chunk;
    }
}

fn parse_duration_ns(value: &str) -> Option<u64> {
    let suffix = value
        .bytes()
        .position(|byte| !byte.is_ascii_digit())
        .unwrap_or(value.len());
    let amount = value[..suffix].parse::<u64>().ok()?;
    let multiplier = match &value[suffix..] {
        "ns" => 1,
        "us" => 1_000,
        "ms" => 1_000_000,
        "s" | "" => 1_000_000_000,
        "m" => 60_000_000_000,
        _ => return None,
    };
    amount.checked_mul(multiplier)
}

fn kill_command(pid: &str, status: Option<&str>) {
    let (Ok(pid), Ok(status)) = (pid.parse::<u64>(), status.unwrap_or("-15").parse::<i64>()) else {
        console_write(b"kill: usage: kill <pid> [status]\n");
        return;
    };
    if kill_process(pid, status) {
        output(format_args!("kill: pid={} status={}\n", pid, status));
    } else {
        console_write(b"kill: failed\n");
    }
}

fn wait_command(pid: &str) {
    let Ok(pid) = pid.parse::<u64>() else {
        console_write(b"wait: invalid pid\n");
        return;
    };
    match wait_process(pid) {
        Some(reply) => output(format_args!(
            "wait: pid={} status={}\n",
            pid, reply.words[0] as i64
        )),
        None => console_write(b"wait: process not found\n"),
    }
}

fn print_system_info() {
    let mut stats = SystemStats::default();
    if microsystem_user_rt::system_stats(boot_cap::SYSTEM_INFO, &mut stats).is_err() {
        console_write(b"sysinfo: unavailable\n");
        return;
    }
    output(format_args!(
        "cpus={} ticks=[{},{}] runnable={} blocked={}\nfree_frames={} kernel_heap={}/{} irq={} ipc={}\n",
        stats.cpu_count,
        stats.cpu_ticks[0],
        stats.cpu_ticks[1],
        stats.runnable_threads,
        stats.blocked_threads,
        stats.free_frames,
        stats.kernel_heap_used,
        stats.kernel_heap_total,
        stats.irq_count,
        stats.ipc_calls,
    ));
}

fn print_date() {
    let Ok(seconds) = microsystem_user_rt::clock_realtime() else {
        console_write(b"date: realtime clock unavailable\n");
        return;
    };
    let days = (seconds / 86_400) as i64;
    let day_seconds = seconds % 86_400;
    let (year, month, day) = civil_from_days(days);
    output(format_args!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02} UTC\n",
        year,
        month,
        day,
        day_seconds / 3_600,
        day_seconds / 60 % 60,
        day_seconds % 60,
    ));
}

fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let days = days + 719_468;
    let era = if days >= 0 { days } else { days - 146_096 } / 146_097;
    let day_of_era = days - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    year += i64::from(month <= 2);
    (year, month as u32, day as u32)
}

fn request_system_power(reboot: bool) {
    if fs_call(FsOperation::Sync, "/", &[]).is_err() {
        console_write(b"power: filesystem sync failed\n");
        return;
    }
    console_write(if reboot {
        b"[system] reboot requested\n"
    } else {
        b"[system] poweroff requested\n"
    });
    if microsystem_user_rt::system_power(boot_cap::SHELL_SYSTEM_CONTROL, reboot).is_err() {
        console_write(if reboot {
            b"reboot: failed\n"
        } else {
            b"poweroff: failed\n"
        });
    }
}

fn run_mica_arguments(arguments: &str) {
    let (options, eval_source) = arguments
        .find("-e ")
        .map(|eval| (arguments[..eval].trim(), Some(arguments[eval + 3..].trim())))
        .unwrap_or((arguments, None));
    let mut policy = [0u8; script::POLICY_BYTES];
    let mut policy_bytes = 0usize;
    let mut timeout_ns = 10_000_000_000u64;
    let mut gui = false;
    let mut words = options.split_whitespace().peekable();
    let mut path = None;
    let mut argv = [0u8; script::STDIN_BYTES];
    let mut argv_bytes = 0usize;
    while let Some(option) = words.next() {
        match option {
            "--gui" => gui = true,
            "--allow" => {
                let Some(rule) = words.next() else {
                    console_write(b"mica: --allow requires a rule\n");
                    return;
                };
                if policy_bytes + rule.len() + 1 > policy.len() {
                    console_write(b"mica: permission policy too large\n");
                    return;
                }
                policy[policy_bytes..policy_bytes + rule.len()].copy_from_slice(rule.as_bytes());
                policy_bytes += rule.len();
                policy[policy_bytes] = b'\n';
                policy_bytes += 1;
            }
            "--timeout" => {
                let Some(value) = words.next() else {
                    console_write(b"mica: --timeout requires a duration\n");
                    return;
                };
                let milliseconds = value
                    .strip_suffix("ms")
                    .and_then(|value| value.parse::<u64>().ok())
                    .or_else(|| {
                        value
                            .strip_suffix('s')
                            .and_then(|value| value.parse::<u64>().ok())
                            .and_then(|seconds| seconds.checked_mul(1000))
                    });
                let Some(milliseconds @ 1..=86_400_000) = milliseconds else {
                    console_write(b"mica: timeout must be 1ms..24h\n");
                    return;
                };
                timeout_ns = milliseconds * 1_000_000;
            }
            "--" => {
                for argument in words.by_ref() {
                    if argv_bytes + argument.len() + 1 > argv.len() {
                        console_write(b"mica: arguments too large\n");
                        return;
                    }
                    argv[argv_bytes..argv_bytes + argument.len()]
                        .copy_from_slice(argument.as_bytes());
                    argv_bytes += argument.len() + 1;
                }
            }
            value if !value.starts_with('-') && path.is_none() => path = Some(value),
            _ => {
                console_write(b"mica: unknown option\n");
                return;
            }
        }
    }
    if !gui && timeout_ns > 60_000_000_000 {
        console_write(b"mica: non-GUI timeout must be at most 60s\n");
        return;
    }
    if let Some(source) = eval_source {
        if gui {
            console_write(b"mica: --gui requires a script file\n");
            return;
        }
        let source = source
            .strip_prefix('\'')
            .and_then(|source| source.strip_suffix('\''))
            .or_else(|| {
                source
                    .strip_prefix('"')
                    .and_then(|source| source.strip_suffix('"'))
            })
            .unwrap_or(source);
        run_mica(
            source.as_bytes(),
            &policy[..policy_bytes],
            &argv[..argv_bytes],
            "",
            timeout_ns,
            script::MODE_EVAL,
            false,
        );
    } else if let Some(path) = path {
        run_mica_file(
            path,
            &policy[..policy_bytes],
            &argv[..argv_bytes],
            timeout_ns,
            gui,
        );
    } else {
        if gui {
            console_write(b"mica: --gui requires a script file\n");
            return;
        }
        run_mica_repl(&policy[..policy_bytes], timeout_ns.min(5_000_000_000));
    }
}

fn run_mica_file(path: &str, policy: &[u8], argv: &[u8], timeout_ns: u64, gui: bool) {
    let Ok(reply) = fs_request(FsOperation::ReadRange, path, &[], 0) else {
        console_write(b"mica: cannot read script\n");
        return;
    };
    let bytes = reply.words[0] as usize;
    let prefix = unsafe { core::slice::from_raw_parts(SHARED_DATA as *const u8, bytes) };
    let Ok(prefix) = core::str::from_utf8(prefix) else {
        console_write(b"mica: script is not UTF-8\n");
        return;
    };
    let Some(manifest_bytes) = manifest_prefix_bytes(prefix) else {
        console_write(b"mica: missing --!mica 1 directive\n");
        return;
    };
    run_mica(
        &prefix.as_bytes()[..manifest_bytes],
        policy,
        argv,
        path,
        timeout_ns,
        script::MODE_FILE,
        gui,
    );
}

fn run_mica_repl(policy: &[u8], timeout_ns: u64) {
    if policy.len() > script::POLICY_BYTES {
        console_write(b"mica: permission policy too large\n");
        return;
    }
    let prepare = Message::new(protocol::PROCESS, process::Operation::SpawnScript as u16);
    let mut prepared = Message::new(protocol::PROCESS, 0);
    if microsystem_user_rt::ipc_call(boot_cap::PROCESS_ENDPOINT, &prepare, &mut prepared, 0)
        .is_err()
        || prepared.words[5] as i64 != Status::Ok as i64
        || prepared.caps[0] == microsystem_abi::CapHandle::INVALID
    {
        console_write(b"mica: session allocation failed\n");
        return;
    }
    let region = prepared.caps[0];
    if microsystem_user_rt::frame_map(
        region,
        script::SESSION_VA,
        Rights(Rights::READ.0 | Rights::WRITE.0),
    )
    .is_err()
    {
        let _ = microsystem_user_rt::cap_delete(region);
        console_write(b"mica: session mapping failed\n");
        return;
    }
    unsafe { core::ptr::write_bytes(script::SESSION_VA as *mut u8, 0, script::SESSION_BYTES) };
    let header = unsafe { &mut *(script::SESSION_VA as *mut script::SessionHeaderV1) };
    *header = script::SessionHeaderV1 {
        magic: script::SESSION_MAGIC,
        version: script::VERSION,
        mode: script::MODE_REPL,
        timeout_ns,
        instruction_limit: 10_000_000,
        policy_bytes: policy.len() as u32,
        ..script::SessionHeaderV1::default()
    };
    unsafe {
        core::ptr::copy_nonoverlapping(
            policy.as_ptr(),
            (script::SESSION_VA as *mut u8).add(script::POLICY_OFFSET),
            policy.len(),
        )
    };
    let _ = microsystem_user_rt::frame_unmap(region, script::SESSION_VA);

    let mut request = Message::new(protocol::PROCESS, process::Operation::SpawnScript as u16);
    request.words[0] = 1;
    request.caps[0] = region;
    request.flags |= microsystem_abi::message_cap_move(0);
    let mut reply = Message::new(protocol::PROCESS, 0);
    if microsystem_user_rt::ipc_call(boot_cap::PROCESS_ENDPOINT, &request, &mut reply, 0).is_err()
        || reply.words[5] as i64 != Status::Ok as i64
        || reply.words[0] < process::FIRST_APPLICATION_PID
        || reply.caps[0] == microsystem_abi::CapHandle::INVALID
        || reply.caps[1] == microsystem_abi::CapHandle::INVALID
    {
        console_write(b"mica: REPL launch failed\n");
        return;
    }
    let pid = reply.words[0];
    let region = reply.caps[0];
    let transferred = reply.caps[1];
    let notification = match microsystem_user_rt::cap_copy(transferred, Rights::WRITE) {
        Ok(notification) => notification,
        Err(_) => {
            let _ = microsystem_user_rt::cap_delete(transferred);
            let _ = microsystem_user_rt::cap_delete(region);
            let _ = kill_process(pid, -15);
            console_write(b"mica: REPL notification failed\n");
            return;
        }
    };
    let _ = microsystem_user_rt::cap_delete(transferred);
    if wait_repl_output(region).is_err() {
        let _ = kill_process(pid, -15);
        let _ = wait_process(pid);
        let _ = microsystem_user_rt::cap_delete(notification);
        let _ = microsystem_user_rt::cap_delete(region);
        console_write(b"mica: REPL did not become ready\n");
        return;
    }
    drain_repl_output(region);
    console_write(b"mica> ");

    let mut line = [0u8; script::STDIN_BYTES];
    let mut length = 0usize;
    let status = loop {
        match console_read() {
            Ok(Some(b'\r' | b'\n')) => {
                console_write(b"\n");
                let exit = line[..length] == *b"exit";
                if send_repl_input(region, notification, &line[..length]).is_err() {
                    break 1;
                }
                length = 0;
                let bits = wait_repl_output(region);
                drain_repl_output(region);
                if exit || bits.is_ok_and(|bits| bits & script::EVENT_EXIT != 0) {
                    break 0;
                }
                if bits.is_err() {
                    break 1;
                }
                console_write(b"mica> ");
            }
            Ok(Some(4)) if length == 0 => {
                let _ = send_repl_input(region, notification, b"exit");
                let _ = wait_repl_output(region);
                drain_repl_output(region);
                break 0;
            }
            Ok(Some(3)) => {
                length = 0;
                console_write(b"^C\nmica> ");
            }
            Ok(Some(8 | 127)) if length > 0 => {
                length -= 1;
                console_write(b"\x08 \x08");
            }
            Ok(Some(byte @ 0x20..=0xff)) if length < line.len() => {
                line[length] = byte;
                length += 1;
                console_write(&[byte]);
            }
            Ok(Some(_)) | Ok(None) => {
                let _ = microsystem_user_rt::yield_now();
            }
            Err(_) => break 1,
        }
    };
    if status != 0 {
        let _ = kill_process(pid, -15);
    }
    let exit_status = wait_process(pid)
        .map(|reply| reply.words[0] as i64)
        .unwrap_or(status);
    let _ = microsystem_user_rt::cap_delete(notification);
    let _ = microsystem_user_rt::cap_delete(region);
    output(format_args!("mica: pid={} status={}\n", pid, exit_status));
}

fn send_repl_input(
    region: microsystem_abi::CapHandle,
    notification: microsystem_abi::CapHandle,
    input: &[u8],
) -> Result<(), Status> {
    microsystem_user_rt::frame_map(
        region,
        script::SESSION_VA,
        Rights(Rights::READ.0 | Rights::WRITE.0),
    )?;
    let header = unsafe { &mut *(script::SESSION_VA as *mut script::SessionHeaderV1) };
    header.stdin_head = input.len() as u32;
    header.stdin_tail = 0;
    header.stdout_head = 0;
    header.stdout_tail = 0;
    header.flags &= !(script::FLAG_INTERRUPT | script::FLAG_OUTPUT_READY | script::FLAG_EXIT_READY);
    unsafe {
        core::ptr::copy_nonoverlapping(
            input.as_ptr(),
            (script::SESSION_VA as *mut u8).add(script::STDIN_OFFSET),
            input.len(),
        );
        microsystem_user_rt::fence();
    };
    microsystem_user_rt::frame_unmap(region, script::SESSION_VA)?;
    microsystem_user_rt::notification_signal(notification, script::EVENT_INPUT)
}

fn wait_repl_output(region: microsystem_abi::CapHandle) -> Result<u64, Status> {
    let deadline = microsystem_user_rt::clock_now()
        .unwrap_or(0)
        .saturating_add(61_000_000_000);
    loop {
        if let Some(bits) = repl_event(region)? {
            return Ok(bits);
        }
        if microsystem_user_rt::clock_now().unwrap_or(deadline) >= deadline {
            return Err(Status::TimedOut);
        }
        if console_read().ok().flatten() == Some(3) {
            microsystem_user_rt::frame_map(
                region,
                script::SESSION_VA,
                Rights(Rights::READ.0 | Rights::WRITE.0),
            )?;
            unsafe {
                (*((script::SESSION_VA) as *mut script::SessionHeaderV1)).flags |=
                    script::FLAG_INTERRUPT;
            }
            microsystem_user_rt::frame_unmap(region, script::SESSION_VA)?;
        }
        let _ = microsystem_user_rt::yield_now();
    }
}

fn repl_event(region: microsystem_abi::CapHandle) -> Result<Option<u64>, Status> {
    microsystem_user_rt::frame_map(
        region,
        script::SESSION_VA,
        Rights(Rights::READ.0 | Rights::WRITE.0),
    )?;
    microsystem_user_rt::fence();
    let flags = unsafe { (*(script::SESSION_VA as *const script::SessionHeaderV1)).flags };
    microsystem_user_rt::frame_unmap(region, script::SESSION_VA)?;
    Ok(if flags & script::FLAG_EXIT_READY != 0 {
        Some(script::EVENT_EXIT)
    } else if flags & script::FLAG_OUTPUT_READY != 0 {
        Some(script::EVENT_OUTPUT)
    } else {
        None
    })
}

fn drain_repl_output(region: microsystem_abi::CapHandle) {
    if microsystem_user_rt::frame_map(
        region,
        script::SESSION_VA,
        Rights(Rights::READ.0 | Rights::WRITE.0),
    )
    .is_err()
    {
        return;
    }
    let header = unsafe { &mut *(script::SESSION_VA as *mut script::SessionHeaderV1) };
    let head = (header.stdout_head as usize).min(script::STDOUT_BYTES);
    let tail = (header.stdout_tail as usize).min(head);
    let output = unsafe {
        core::slice::from_raw_parts(
            (script::SESSION_VA as *const u8).add(script::STDOUT_OFFSET + tail),
            head - tail,
        )
    };
    console_write(output);
    header.stdout_tail = head as u32;
    let _ = microsystem_user_rt::frame_unmap(region, script::SESSION_VA);
}

fn manifest_prefix_bytes(source: &str) -> Option<usize> {
    let mut bytes = 0usize;
    let mut version = false;
    for line in source.split_inclusive('\n') {
        let trimmed = line.trim();
        if trimmed == "--!mica 1" {
            version = true;
        } else if trimmed.starts_with("--!") && !trimmed.starts_with("--!allow ") {
            return None;
        } else if !trimmed.is_empty() && !trimmed.starts_with("--") {
            break;
        }
        bytes += line.len();
    }
    version.then_some(bytes)
}

struct CurlOptions<'a> {
    url: &'a str,
    output: Option<&'a str>,
    include_status: bool,
}

fn parse_curl_arguments(arguments: &str) -> Result<CurlOptions<'_>, ()> {
    let mut words = arguments.split_whitespace();
    let mut url = None;
    let mut output = None;
    let mut include_status = false;

    while let Some(argument) = words.next() {
        match argument {
            "-s" | "--silent" => {}
            "-i" | "--include" => include_status = true,
            "-o" | "--output" => {
                let value = words.next().ok_or(())?;
                if value.is_empty() || value.len() > 255 {
                    return Err(());
                }
                output = Some(value);
            }
            "--" => {
                let value = words.next().ok_or(())?;
                if url.is_some() || words.next().is_some() {
                    return Err(());
                }
                url = Some(value);
            }
            value if value.starts_with('-') => return Err(()),
            value if url.is_none() => url = Some(value),
            _ => return Err(()),
        }
    }

    let url = url.ok_or(())?;
    if url.len() > 2_048
        || (!url.starts_with("http://") && !url.starts_with("https://"))
        || url.bytes().any(|byte| !(0x20..=0x7e).contains(&byte))
    {
        return Err(());
    }
    if output.is_some_and(|path| path.bytes().any(|byte| !(0x20..=0x7e).contains(&byte))) {
        return Err(());
    }
    Ok(CurlOptions {
        url,
        output,
        include_status,
    })
}

fn push_mica_string_literal(source: &mut Vec<u8>, value: &str) -> Result<(), ()> {
    source.push(b'"');
    for byte in value.bytes() {
        match byte {
            b'\\' => source.extend_from_slice(b"\\\\"),
            b'"' => source.extend_from_slice(b"\\\""),
            b'\n' => source.extend_from_slice(b"\\n"),
            b'\r' => source.extend_from_slice(b"\\r"),
            0x20..=0x7e => source.push(byte),
            _ => return Err(()),
        }
    }
    source.push(b'"');
    Ok(())
}

fn run_curl(arguments: &str, state: &ShellState) {
    let Ok(options) = parse_curl_arguments(arguments) else {
        console_write(b"curl: usage: curl [-s] [-i] [-o <path>] <http[s]://url>\n");
        return;
    };
    let output_path = match options.output {
        Some(path) => match state.resolve(path) {
            Ok(path) => Some(path),
            Err(()) => {
                console_write(b"curl: invalid output path\n");
                return;
            }
        },
        None => None,
    };

    let mut source = Vec::new();
    source.extend_from_slice(b"local bytes = require(\"bytes\")\nlocal http = require(\"http\")\n");
    if output_path.is_some() {
        source.extend_from_slice(b"local fs = require(\"fs\")\n");
    }
    source.extend_from_slice(b"local response, request_error = http.get(");
    if push_mica_string_literal(&mut source, options.url).is_err() {
        console_write(b"curl: URL contains unsupported characters\n");
        return;
    }
    source.extend_from_slice(
        b")\nif response == nil then\n  local message = \"request failed\"\n  if request_error ~= nil then message = request_error.message end\n  print(\"curl: \" + message)\n  return 1\nend\n",
    );
    if options.include_status {
        source.extend_from_slice(b"print(\"HTTP status=\" + tostring(response.status))\n");
    }
    source.extend_from_slice(
        b"local body, read_error = response.read_all(response, 32768)\nif body == nil then\n  local message = \"response read failed\"\n  if read_error ~= nil then message = read_error.message end\n  print(\"curl: \" + message)\n  return 1\nend\n",
    );
    let mut policy = Vec::from(&b"net.browse\n"[..]);
    if let Some(path) = output_path.as_deref() {
        source.extend_from_slice(b"local saved, save_error = fs.write_file(");
        if push_mica_string_literal(&mut source, path).is_err() {
            console_write(b"curl: output path contains unsupported characters\n");
            return;
        }
        source.extend_from_slice(
            b", body, {atomic = true, fsync = true})\nif saved ~= true then\n  local message = \"write failed\"\n  if save_error ~= nil then message = save_error.message end\n  print(\"curl: \" + message)\n  return 1\nend\nprint(\"curl: saved \" + ",
        );
        if push_mica_string_literal(&mut source, path).is_err() {
            console_write(b"curl: output path contains unsupported characters\n");
            return;
        }
        source.extend_from_slice(b")\n");
        policy.extend_from_slice(b"fs.write:");
        policy.extend_from_slice(path.as_bytes());
        policy.push(b'\n');
    } else {
        source.extend_from_slice(
            b"local text, text_error = bytes.to_string(body)\nif text == nil then\n  print(\"curl: response body is not UTF-8\")\n  return 1\nend\nio.write(text)\nio.write(\"\\n\")\n",
        );
    }

    if source.len() > script::SOURCE_BYTES || policy.len() > script::POLICY_BYTES {
        console_write(b"curl: request is too large\n");
        return;
    }
    run_mica_quiet(
        &source,
        &policy,
        b"",
        "",
        10_000_000_000,
        script::MODE_EVAL,
        false,
    );
}

fn run_nslookup(host: &str) {
    if host.len() > 253
        || host.is_empty()
        || host
            .bytes()
            .any(|byte| !(byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-')))
    {
        console_write(b"nslookup: invalid IPv4 host name\n");
        return;
    }
    let mut source =
        Vec::from(&b"local net = require(\"net\")\nlocal address, lookup_error = net.resolve("[..]);
    if push_mica_string_literal(&mut source, host).is_err() {
        console_write(b"nslookup: invalid host name\n");
        return;
    }
    source.extend_from_slice(
        b")\nif address == nil then\n  local message = \"lookup failed\"\n  if lookup_error ~= nil then message = lookup_error.message end\n  print(\"nslookup: \" + message)\n  return 1\nend\nprint(address)\n",
    );
    let mut policy = Vec::from(&b"net.connect:"[..]);
    policy.extend_from_slice(host.as_bytes());
    policy.extend_from_slice(b":53\n");
    run_mica_quiet(
        &source,
        &policy,
        b"",
        "",
        10_000_000_000,
        script::MODE_EVAL,
        false,
    );
}

fn run_netstat() {
    let request = Message::new(protocol::NETWORK, network::Operation::Stats as u16);
    let mut reply = Message::new(protocol::NETWORK, 0);
    if microsystem_user_rt::ipc_call(boot_cap::NETWORK_ENDPOINT, &request, &mut reply, 0).is_err()
        || reply.words[5] as i64 != Status::Ok as i64
    {
        console_write(b"netstat: unavailable\n");
        return;
    }
    let active = reply.words[4] as u32;
    let maximum = (reply.words[4] >> 32) as u32;
    output(format_args!(
        "ipv4={} gateway={} dns={} sessions={} connections={}/{}\n",
        ipv4_text(reply.words[0] as u32),
        ipv4_text(reply.words[1] as u32),
        ipv4_text(reply.words[2] as u32),
        reply.words[3],
        active,
        maximum,
    ));
}

fn ipv4_text(address: u32) -> Ipv4Text {
    Ipv4Text(address.to_be_bytes())
}

struct Ipv4Text([u8; 4]);

impl fmt::Display for Ipv4Text {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{}.{}.{}.{}",
            self.0[0], self.0[1], self.0[2], self.0[3]
        )
    }
}

fn run_mica(
    source: &[u8],
    policy: &[u8],
    argv: &[u8],
    path: &str,
    timeout_ns: u64,
    mode: u16,
    gui: bool,
) {
    run_mica_inner(source, policy, argv, path, timeout_ns, mode, gui, true);
}

fn run_mica_quiet(
    source: &[u8],
    policy: &[u8],
    argv: &[u8],
    path: &str,
    timeout_ns: u64,
    mode: u16,
    gui: bool,
) {
    run_mica_inner(source, policy, argv, path, timeout_ns, mode, gui, false);
}

fn run_mica_inner(
    source: &[u8],
    policy: &[u8],
    argv: &[u8],
    path: &str,
    timeout_ns: u64,
    mode: u16,
    gui: bool,
    report_status: bool,
) {
    if source.is_empty()
        || source.len() > script::SOURCE_BYTES
        || path.len() > u16::MAX as usize
        || policy.len().saturating_add(path.len()) > script::POLICY_BYTES
        || core::str::from_utf8(source).is_err()
    {
        console_write(b"mica: source must be UTF-8 and at most 16 KiB\n");
        return;
    }
    let prepare = Message::new(protocol::PROCESS, process::Operation::SpawnScript as u16);
    let mut prepared = Message::new(protocol::PROCESS, 0);
    if microsystem_user_rt::ipc_call(boot_cap::PROCESS_ENDPOINT, &prepare, &mut prepared, 0)
        .is_err()
        || prepared.words[5] as i64 != Status::Ok as i64
        || prepared.caps[0] == microsystem_abi::CapHandle::INVALID
    {
        console_write(b"mica: session allocation failed\n");
        return;
    }
    let session = prepared.caps[0];
    if microsystem_user_rt::frame_map(
        session,
        script::SESSION_VA,
        Rights(Rights::READ.0 | Rights::WRITE.0),
    )
    .is_err()
    {
        let _ = microsystem_user_rt::cap_delete(session);
        console_write(b"mica: session mapping failed\n");
        return;
    }
    unsafe { core::ptr::write_bytes(script::SESSION_VA as *mut u8, 0, script::SESSION_BYTES) };
    let header = unsafe { &mut *(script::SESSION_VA as *mut script::SessionHeaderV1) };
    *header = script::SessionHeaderV1 {
        magic: script::SESSION_MAGIC,
        version: script::VERSION,
        mode,
        source_bytes: source.len() as u32,
        stdin_head: argv.len() as u32,
        stdin_tail: argv.len() as u32,
        timeout_ns,
        instruction_limit: 10_000_000,
        flags: if gui { script::FLAG_GUI_SESSION } else { 0 },
        policy_bytes: policy.len() as u32,
        argv_bytes: argv.len() as u16,
        path_bytes: path.len() as u16,
        ..script::SessionHeaderV1::default()
    };
    unsafe {
        core::ptr::copy_nonoverlapping(
            policy.as_ptr(),
            (script::SESSION_VA as *mut u8).add(script::POLICY_OFFSET),
            policy.len(),
        )
    };
    unsafe {
        core::ptr::copy_nonoverlapping(
            path.as_ptr(),
            (script::SESSION_VA as *mut u8).add(script::POLICY_OFFSET + policy.len()),
            path.len(),
        )
    };
    unsafe {
        core::ptr::copy_nonoverlapping(
            source.as_ptr(),
            (script::SESSION_VA as *mut u8).add(script::SOURCE_OFFSET),
            source.len(),
        )
    };
    unsafe {
        core::ptr::copy_nonoverlapping(
            argv.as_ptr(),
            (script::SESSION_VA as *mut u8).add(script::STDIN_OFFSET),
            argv.len(),
        )
    };
    if microsystem_user_rt::frame_unmap(session, script::SESSION_VA).is_err() {
        let _ = microsystem_user_rt::cap_delete(session);
        console_write(b"mica: session unmap failed\n");
        return;
    }
    let mut request = Message::new(protocol::PROCESS, process::Operation::SpawnScript as u16);
    request.words[0] = source.len() as u64;
    request.words[1] = if gui {
        script::FLAG_GUI_SESSION as u64
    } else {
        0
    };
    request.caps[0] = session;
    request.flags |= microsystem_abi::message_cap_move(0);
    let mut reply = Message::new(protocol::PROCESS, 0);
    let launch_call =
        microsystem_user_rt::ipc_call(boot_cap::PROCESS_ENDPOINT, &request, &mut reply, 0);
    if launch_call.is_err()
        || reply.words[5] as i64 != Status::Ok as i64
        || reply.words[0] < process::FIRST_APPLICATION_PID
        || reply.caps[0] == microsystem_abi::CapHandle::INVALID
        || reply.caps[1] == microsystem_abi::CapHandle::INVALID
    {
        output(format_args!(
            "mica: launch failed status={}\n",
            launch_call
                .err()
                .map(|status| status as i64)
                .unwrap_or(reply.words[5] as i64)
        ));
        return;
    }
    let pid = reply.words[0];
    let session = reply.caps[0];
    let transferred_completion = reply.caps[1];
    let completion = match microsystem_user_rt::cap_copy(transferred_completion, Rights::READ) {
        Ok(completion) => completion,
        Err(_) => {
            let _ = microsystem_user_rt::cap_delete(transferred_completion);
            let _ = microsystem_user_rt::cap_delete(session);
            console_write(b"mica: completion capability failed\n");
            return;
        }
    };
    let _ = microsystem_user_rt::cap_delete(transferred_completion);
    let _ = microsystem_user_rt::debug_write(b"[mica] shell launch reply=true\n");
    let completion_deadline = microsystem_user_rt::clock_now()
        .unwrap_or(0)
        .saturating_add(timeout_ns)
        .saturating_add(1_000_000_000);
    let _ = microsystem_user_rt::notification_wait(completion, completion_deadline);
    let status = wait_process(pid)
        .map(|reply| reply.words[0] as i64)
        .unwrap_or(Status::Fault as i64);
    if microsystem_user_rt::frame_map(
        session,
        script::SESSION_VA,
        Rights(Rights::READ.0 | Rights::WRITE.0),
    )
    .is_ok()
    {
        let header = unsafe { &*(script::SESSION_VA as *const script::SessionHeaderV1) };
        let bytes = (header.stdout_head as usize).min(script::STDOUT_BYTES);
        let output = unsafe {
            core::slice::from_raw_parts(
                (script::SESSION_VA as *const u8).add(script::STDOUT_OFFSET),
                bytes,
            )
        };
        console_write(output);
        let _ = microsystem_user_rt::frame_unmap(session, script::SESSION_VA);
    }
    let _ = microsystem_user_rt::cap_delete(completion);
    let _ = microsystem_user_rt::cap_delete(session);
    if report_status {
        output(format_args!("mica: pid={} status={}\n", pid, status));
    }
}

fn wait_process(pid: u64) -> Option<Message> {
    loop {
        let mut request = Message::new(protocol::PROCESS, process::Operation::Wait as u16);
        request.words[0] = pid;
        let mut reply = Message::new(protocol::PROCESS, 0);
        if microsystem_user_rt::ipc_call(boot_cap::PROCESS_ENDPOINT, &request, &mut reply, 0)
            .is_err()
        {
            return None;
        }
        match reply.words[5] as i64 {
            0 => return Some(reply),
            value if value == microsystem_abi::Status::Busy as i64 => {
                let _ = microsystem_user_rt::yield_now();
            }
            _ => return None,
        }
    }
}

fn wait_for_resources(pid: u64, frames: u64, pools: u64, mappings: u64) -> bool {
    let deadline = microsystem_user_rt::clock_now()
        .unwrap_or(0)
        .saturating_add(2_000_000_000);
    loop {
        let mut request = Message::new(protocol::PROCESS, process::Operation::Wait as u16);
        request.words[0] = pid;
        let mut reply = Message::new(protocol::PROCESS, 0);
        if microsystem_user_rt::ipc_call(boot_cap::PROCESS_ENDPOINT, &request, &mut reply, 0)
            .is_err()
        {
            return false;
        }
        if reply.words[5] as i64 == microsystem_abi::Status::Busy as i64
            && reply.words[1] == frames
            && reply.words[2] == pools
            && reply.words[3] == mappings
        {
            return true;
        }
        if reply.words[5] as i64 != microsystem_abi::Status::Busy as i64
            || microsystem_user_rt::clock_now().unwrap_or(deadline) >= deadline
        {
            return false;
        }
        let _ = microsystem_user_rt::yield_now();
    }
}

fn kill_process(pid: u64, status: i64) -> bool {
    let mut request = Message::new(protocol::PROCESS, process::Operation::Kill as u16);
    request.words[0] = pid;
    request.words[1] = status as u64;
    let mut reply = Message::new(protocol::PROCESS, 0);
    microsystem_user_rt::ipc_call(boot_cap::PROCESS_ENDPOINT, &request, &mut reply, 0).is_ok()
        && reply.words[5] as i64 == 0
}

fn report_completion(program: &str, pid: u64, reply: &Message) {
    output(format_args!(
        "wait: pid={} status={}\n",
        pid, reply.words[0] as i64
    ));
    if reply.words[1] != 0 || reply.words[2] != 0 || reply.words[3] != 0 {
        output(format_args!(
            "[mm] application exit reclaimed pid={} frames={} pools={} mappings={}\n",
            pid, reply.words[1], reply.words[2], reply.words[3]
        ));
    }
    output_loader(program, pid);
}

fn program_id(name: &str) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in name.as_bytes() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x100_0000_01b3);
    }
    hash
}

fn output_loader(program: &str, pid: u64) {
    output(format_args!(
        "[proc] bootfs name-based loader program={} pid={} static-elf=true\n",
        program, pid
    ));
}

fn program_name(program: u64) -> Option<&'static str> {
    [
        "counter",
        "spinner",
        "privprobe",
        "resourceprobe",
        "resourcefault",
        "resourcekill",
    ]
    .into_iter()
    .find(|name| program_id(name) == program)
}

fn time_request(operation: time::Operation, argument: u64) -> Result<u64, Status> {
    let mut request = Message::new(protocol::TIME, operation as u16);
    request.words[0] = argument;
    let mut reply = Message::new(protocol::TIME, 0);
    microsystem_user_rt::ipc_call(boot_cap::TIME_ENDPOINT, &request, &mut reply, 0)?;
    if reply.protocol != protocol::TIME || reply.words[5] as i64 != Status::Ok as i64 {
        return Err(Status::Io);
    }
    Ok(reply.words[0])
}

fn verify_time_protocol() -> Result<(), Status> {
    const SLEEP_NS: u64 = 5_000_000;

    let before = time_request(time::Operation::Uptime, 0)?;
    let woke = time_request(time::Operation::Sleep, SLEEP_NS)?;
    let after = time_request(time::Operation::Uptime, 0)?;
    if woke < before.saturating_add(SLEEP_NS) || after < woke {
        return Err(Status::Io);
    }
    Ok(())
}

fn console_write(bytes: &[u8]) {
    for chunk in bytes.chunks(INLINE_BYTES) {
        let mut request = Message::new(protocol::CONSOLE, ConsoleOperation::Write as u16);
        request.words[0] = chunk.len() as u64;
        unsafe {
            core::ptr::copy_nonoverlapping(
                chunk.as_ptr(),
                request.words[1..5].as_mut_ptr().cast::<u8>(),
                chunk.len(),
            );
        }
        let mut reply = Message::new(protocol::CONSOLE, 0);
        if microsystem_user_rt::ipc_call(boot_cap::CONSOLE_ENDPOINT, &request, &mut reply, 0)
            .is_err()
            || reply.words[5] as i64 != 0
        {
            microsystem_user_rt::exit(4);
        }
    }
}

fn console_read() -> Result<Option<u8>, Status> {
    let request = Message::new(protocol::CONSOLE, ConsoleOperation::Read as u16);
    let mut reply = Message::new(protocol::CONSOLE, 0);
    microsystem_user_rt::ipc_call(boot_cap::CONSOLE_ENDPOINT, &request, &mut reply, 0)?;
    match reply.words[5] as i64 {
        0 => Ok(Some(reply.words[0] as u8)),
        value if value == Status::Busy as i64 => Ok(None),
        _ => Err(Status::Io),
    }
}

fn fs_print(operation: FsOperation, path: &str) {
    match fs_call(operation, path, &[]) {
        Ok(length) => {
            let bytes = unsafe { core::slice::from_raw_parts(SHARED_DATA as *const u8, length) };
            console_write(bytes);
            console_write(b"\n");
        }
        Err(_) => console_write(b"filesystem operation failed\n"),
    }
}

fn fs_cat(path: &str) {
    let Ok(size) = fs_file_size(path) else {
        console_write(b"cat: failed\n");
        return;
    };
    let mut offset = 0usize;
    let mut last = None;
    while offset < size {
        let Ok(length) = fs_read_range(path, offset) else {
            console_write(b"cat: failed\n");
            return;
        };
        if length == 0 {
            break;
        }
        let bytes = unsafe { core::slice::from_raw_parts(SHARED_DATA as *const u8, length) };
        last = bytes.last().copied();
        console_write(bytes);
        offset = offset.saturating_add(length);
    }
    if last.is_some_and(|byte| byte != b'\n') {
        console_write(b"\n");
    }
}

fn fs_head(path: &str, lines: usize) {
    let Ok(size) = fs_file_size(path) else {
        console_write(b"head: failed\n");
        return;
    };
    let mut offset = 0usize;
    let mut remaining = lines;
    while offset < size && remaining != 0 {
        let Ok(length) = fs_read_range(path, offset) else {
            console_write(b"head: failed\n");
            return;
        };
        if length == 0 {
            break;
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
            .unwrap_or(bytes.len());
        console_write(&bytes[..end]);
        offset = offset.saturating_add(length);
    }
}

fn fs_tail(path: &str, lines: usize) {
    let Ok(size) = fs_file_size(path) else {
        console_write(b"tail: failed\n");
        return;
    };
    if size == 0 {
        return;
    }
    let trailing_newline = fs_byte_at(path, size - 1) == Ok(b'\n');
    let target = lines.saturating_add(usize::from(trailing_newline));
    let mut found = 0usize;
    let mut cursor = size;
    let mut start = 0usize;
    'search: while cursor != 0 {
        let offset = cursor.saturating_sub(SHARED_BYTES);
        let Ok(length) = fs_read_range(path, offset) else {
            console_write(b"tail: failed\n");
            return;
        };
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
    fs_print_range(path, start, size, "tail");
}

fn fs_wc(path: &str) {
    let Ok(size) = fs_file_size(path) else {
        console_write(b"wc: failed\n");
        return;
    };
    let mut offset = 0usize;
    let mut lines = 0usize;
    let mut words = 0usize;
    let mut in_word = false;
    while offset < size {
        let Ok(length) = fs_read_range(path, offset) else {
            console_write(b"wc: failed\n");
            return;
        };
        if length == 0 {
            break;
        }
        let bytes = unsafe { core::slice::from_raw_parts(SHARED_DATA as *const u8, length) };
        for byte in bytes {
            lines += usize::from(*byte == b'\n');
            let whitespace = byte.is_ascii_whitespace();
            if !whitespace && !in_word {
                words += 1;
            }
            in_word = !whitespace;
        }
        offset += length;
    }
    output(format_args!("{} {} {} {}\n", lines, words, size, path));
}

fn fs_hexdump(path: &str) {
    let Ok(size) = fs_file_size(path) else {
        console_write(b"hexdump: failed\n");
        return;
    };
    let mut file_offset = 0usize;
    let mut line = [0u8; 16];
    let mut line_bytes = 0usize;
    while file_offset < size {
        let Ok(length) = fs_read_range(path, file_offset) else {
            console_write(b"hexdump: failed\n");
            return;
        };
        if length == 0 {
            break;
        }
        let bytes = unsafe { core::slice::from_raw_parts(SHARED_DATA as *const u8, length) };
        for byte in bytes {
            line[line_bytes] = *byte;
            line_bytes += 1;
            if line_bytes == line.len() {
                print_hex_line(file_offset + 1 - line_bytes, &line[..line_bytes]);
                line_bytes = 0;
            }
            file_offset += 1;
        }
    }
    if line_bytes != 0 {
        print_hex_line(file_offset - line_bytes, &line[..line_bytes]);
    }
}

fn print_hex_line(offset: usize, bytes: &[u8]) {
    output(format_args!("{:08x}  ", offset));
    for index in 0..16 {
        if let Some(byte) = bytes.get(index) {
            output(format_args!("{:02x} ", byte));
        } else {
            console_write(b"   ");
        }
        if index == 7 {
            console_write(b" ");
        }
    }
    console_write(b" |");
    for byte in bytes {
        console_write(&[if byte.is_ascii_graphic() || *byte == b' ' {
            *byte
        } else {
            b'.'
        }]);
    }
    console_write(b"|\n");
}

fn fs_grep(pattern: &str, path: &str) {
    let Ok(size) = fs_file_size(path) else {
        console_write(b"grep: failed\n");
        return;
    };
    let mut offset = 0usize;
    let mut line = Vec::new();
    while offset < size {
        let Ok(length) = fs_read_range(path, offset) else {
            console_write(b"grep: failed\n");
            return;
        };
        if length == 0 {
            break;
        }
        let bytes = unsafe { core::slice::from_raw_parts(SHARED_DATA as *const u8, length) };
        for byte in bytes {
            if line.len() == 32 * 1024 {
                console_write(b"grep: line exceeds 32768 bytes\n");
                return;
            }
            line.push(*byte);
            if *byte == b'\n' {
                print_matching_line(&line, pattern.as_bytes());
                line.clear();
            }
        }
        offset += length;
    }
    if !line.is_empty() {
        print_matching_line(&line, pattern.as_bytes());
    }
}

fn print_matching_line(line: &[u8], pattern: &[u8]) {
    if pattern.is_empty() || line.windows(pattern.len()).any(|window| window == pattern) {
        console_write(line);
        if line.last() != Some(&b'\n') {
            console_write(b"\n");
        }
    }
}

fn fs_copy(source: &str, destination: &str, recursive: bool) {
    let result = copy_target(source, destination)
        .and_then(|destination| fs_copy_entry(source, &destination, recursive, 0));
    console_write(if result.is_ok() {
        b"cp: ok\n"
    } else {
        b"cp: failed\n"
    });
}

fn fs_copy_entry(
    source: &str,
    destination: &str,
    recursive: bool,
    depth: usize,
) -> Result<(), Status> {
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
    for child in fs_list_paths(source)? {
        let name = basename(&child).ok_or(Status::Invalid)?;
        let child_destination = join_path(destination, name)?;
        fs_copy_entry(&child, &child_destination, true, depth + 1)?;
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
            fs_call(FsOperation::Replace, &temporary, destination.as_bytes()).map(|_| ())
        });
    if copied.is_err() {
        let _ = fs_call(FsOperation::Unlink, &temporary, &[]);
    }
    copied
}

fn fs_copy_file_contents(source: &str, destination: &str) -> Result<(), Status> {
    let size = fs_file_size(source)?;
    fs_call(FsOperation::Write, destination, &[])?;
    let mut offset = 0usize;
    let mut data = [0u8; SHARED_BYTES];
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
        Ok(reply) if reply.words[0] == 2 => {
            join_path(destination, basename(source).ok_or(Status::Invalid)?)
        }
        Ok(_) | Err(Status::NotFound) => Ok(destination.to_string()),
        Err(status) => Err(status),
    }
}

fn fs_mkdir(path: &str, parents: bool) -> Result<(), Status> {
    if !parents {
        return fs_call(FsOperation::Mkdir, path, &[]).map(|_| ());
    }
    if path == "/" {
        return Ok(());
    }
    let mut current = String::new();
    for component in path.split('/').filter(|component| !component.is_empty()) {
        current.push('/');
        current.push_str(component);
        match fs_request(FsOperation::Stat, &current, &[], 0) {
            Ok(reply) if reply.words[0] == 2 => {}
            Err(Status::NotFound) => {
                fs_call(FsOperation::Mkdir, &current, &[])?;
            }
            _ => return Err(Status::Invalid),
        }
    }
    Ok(())
}

fn fs_remove(path: &str, recursive: bool) -> Result<(), Status> {
    fs_remove_entry(path, recursive, 0)
}

fn fs_remove_entry(path: &str, recursive: bool, depth: usize) -> Result<(), Status> {
    if path == "/" || depth > 32 {
        return Err(Status::AccessDenied);
    }
    let stat = fs_request(FsOperation::Stat, path, &[], 0)?;
    if stat.words[0] == 2 {
        if !recursive {
            return Err(Status::Invalid);
        }
        let children = fs_list_paths(path)?;
        for child in children {
            fs_remove_entry(&child, true, depth + 1)?;
        }
    }
    fs_call(FsOperation::Unlink, path, &[]).map(|_| ())
}

fn fs_find(path: &str, tree: bool) {
    if fs_walk_print(path, tree, 0).is_err() {
        console_write(if tree {
            b"tree: failed\n"
        } else {
            b"find: failed\n"
        });
    }
}

fn fs_walk_print(path: &str, tree: bool, depth: usize) -> Result<(), Status> {
    if depth > 32 {
        return Err(Status::Invalid);
    }
    if tree {
        for _ in 0..depth {
            console_write(b"  ");
        }
        output(format_args!("{}\n", basename(path).unwrap_or("/")));
    } else {
        output(format_args!("{}\n", path));
    }
    let stat = fs_request(FsOperation::Stat, path, &[], 0)?;
    if stat.words[0] == 2 {
        for child in fs_list_paths(path)? {
            fs_walk_print(&child, tree, depth + 1)?;
        }
    }
    Ok(())
}

fn fs_du(path: &str) {
    match fs_usage(path, 0) {
        Ok(bytes) => output(format_args!("{}\t{}\n", bytes, path)),
        Err(_) => console_write(b"du: failed\n"),
    }
}

fn fs_usage(path: &str, depth: usize) -> Result<u64, Status> {
    if depth > 32 {
        return Err(Status::Invalid);
    }
    let stat = fs_request(FsOperation::Stat, path, &[], 0)?;
    if stat.words[0] == 1 {
        return Ok(stat.words[1]);
    }
    let mut bytes = 0u64;
    for child in fs_list_paths(path)? {
        bytes = bytes.saturating_add(fs_usage(&child, depth + 1)?);
    }
    Ok(bytes)
}

fn fs_df() {
    let Ok(reply) = fs_request(FsOperation::Stats, "/", &[], 0) else {
        console_write(b"df: unavailable\n");
        return;
    };
    if reply.words[0] as usize != core::mem::size_of::<FilesystemStatsV1>() {
        console_write(b"df: invalid response\n");
        return;
    }
    let stats = unsafe { core::ptr::read_unaligned(SHARED_DATA as *const FilesystemStatsV1) };
    if stats.version != 1 {
        console_write(b"df: unsupported version\n");
        return;
    }
    output(format_args!(
        "filesystem blocks={} used={} free={} block_size={} entries={} generation={} transaction={}\n",
        stats.total_blocks,
        stats.used_blocks,
        stats.free_blocks,
        stats.block_size,
        stats.entries,
        stats.generation,
        stats.transaction,
    ));
}

fn fs_list_paths(path: &str) -> Result<Vec<String>, Status> {
    let length = fs_call(FsOperation::List, path, &[])?;
    let bytes = unsafe { core::slice::from_raw_parts(SHARED_DATA as *const u8, length) };
    let text = core::str::from_utf8(bytes).map_err(|_| Status::Io)?;
    Ok(text
        .lines()
        .filter(|entry| !entry.is_empty())
        .map(str::to_string)
        .collect())
}

fn fs_file_size(path: &str) -> Result<usize, Status> {
    let reply = fs_request(FsOperation::Stat, path, &[], 0)?;
    if reply.words[0] != 1 {
        return Err(Status::Invalid);
    }
    usize::try_from(reply.words[1]).map_err(|_| Status::Invalid)
}

fn fs_read_range(path: &str, offset: usize) -> Result<usize, Status> {
    let reply = fs_request(FsOperation::ReadRange, path, &[], offset as u64)?;
    usize::try_from(reply.words[0]).map_err(|_| Status::Io)
}

fn fs_byte_at(path: &str, offset: usize) -> Result<u8, Status> {
    let length = fs_read_range(path, offset)?;
    if length == 0 {
        return Err(Status::Io);
    }
    Ok(unsafe { core::ptr::read(SHARED_DATA as *const u8) })
}

fn fs_print_range(path: &str, mut offset: usize, end: usize, command: &str) {
    while offset < end {
        let Ok(length) = fs_read_range(path, offset) else {
            output(format_args!("{}: failed\n", command));
            return;
        };
        if length == 0 {
            break;
        }
        let length = length.min(end - offset);
        let bytes = unsafe { core::slice::from_raw_parts(SHARED_DATA as *const u8, length) };
        console_write(bytes);
        offset += length;
    }
}

fn basename(path: &str) -> Option<&str> {
    if path == "/" {
        return None;
    }
    path.rsplit('/').find(|component| !component.is_empty())
}

fn join_path(parent: &str, name: &str) -> Result<String, Status> {
    let separator = if parent == "/" { "" } else { "/" };
    let length = parent.len() + separator.len() + name.len();
    if name.is_empty() || name.contains('/') || length > 255 {
        return Err(Status::Invalid);
    }
    let mut path = String::with_capacity(length);
    path.push_str(parent);
    path.push_str(separator);
    path.push_str(name);
    Ok(path)
}

fn fs_fsync(path: &str) -> Result<(), Status> {
    let descriptor = fs_request(FsOperation::Open, path, &[], 0)?.words[0];
    if descriptor == 0 {
        return Err(Status::Io);
    }
    let synced = fs_request(FsOperation::Fsync, "", &[], descriptor).map(|_| ());
    let closed = fs_request(FsOperation::Close, "", &[], descriptor).map(|_| ());
    match synced {
        Err(status) => Err(status),
        Ok(()) => closed,
    }
}

fn fs_call(operation: FsOperation, path: &str, data: &[u8]) -> Result<usize, Status> {
    let reply = fs_request(operation, path, data, 0)?;
    Ok(reply.words[0] as usize)
}

fn fs_request(
    operation: FsOperation,
    path: &str,
    data: &[u8],
    descriptor: u64,
) -> Result<Message, Status> {
    if path.len() > 255
        || path.len().saturating_add(data.len()) > SHARED_BYTES
        || (path.is_empty() && descriptor == 0)
    {
        return Err(Status::Invalid);
    }
    unsafe {
        core::ptr::copy_nonoverlapping(path.as_ptr(), SHARED_DATA as *mut u8, path.len());
        core::ptr::copy_nonoverlapping(
            data.as_ptr(),
            (SHARED_DATA + path.len()) as *mut u8,
            data.len(),
        );
        microsystem_user_rt::fence();
    }
    let mut request = Message::new(protocol::FILESYSTEM, operation as u16);
    request.words[0] = path.len() as u64;
    request.words[1] = data.len() as u64;
    request.words[2] = descriptor;
    request.caps[0] = boot_cap::SHARED_FILESYSTEM_FRAME;
    let mut reply = Message::new(protocol::FILESYSTEM, 0);
    microsystem_user_rt::ipc_call(boot_cap::FILESYSTEM_ENDPOINT, &request, &mut reply, 0)?;
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

fn verify_filesystem_protocol() -> Result<(), Status> {
    const CONTENT: &[u8] = b"descriptor-data";

    macro_rules! step {
        ($name:literal, $operation:expr) => {
            match $operation {
                Ok(value) => value,
                Err(status) => {
                    let _ = microsystem_user_rt::debug_write(
                        concat!("[shell] filesystem protocol failed stage=", $name, "\n")
                            .as_bytes(),
                    );
                    return Err(status);
                }
            }
        };
    }

    let token = microsystem_user_rt::clock_now().unwrap_or(0);
    let directory = alloc::format!("/.system/.shell-protocol-proof-{token:016x}");
    let source = alloc::format!("{directory}/a");
    let destination = alloc::format!("{directory}/b");
    step!(
        "preflight",
        match fs_request(FsOperation::Stat, &directory, &[], 0) {
            Err(Status::NotFound) => Ok(()),
            Ok(_) => Err(Status::Busy),
            Err(status) => Err(status),
        }
    );
    step!("mkdir", fs_call(FsOperation::Mkdir, &directory, &[]));
    let verified = (|| {
        step!(
            "path-write",
            fs_call(FsOperation::Write, &source, b"path-data")
        );

        let descriptor = step!("open", fs_request(FsOperation::Open, &source, &[], 0)).words[0];
        if descriptor == 0 {
            let _ = microsystem_user_rt::debug_write(
                b"[shell] filesystem protocol failed stage=open-descriptor\n",
            );
            return Err(Status::Io);
        }
        step!(
            "descriptor-write",
            fs_request(FsOperation::Write, "", CONTENT, descriptor)
        );
        step!(
            "descriptor-fsync",
            fs_request(FsOperation::Fsync, "", &[], descriptor)
        );
        let read = step!(
            "descriptor-read",
            fs_request(FsOperation::Read, "", &[], descriptor)
        );
        if read.words[0] as usize != CONTENT.len()
            || unsafe {
                core::slice::from_raw_parts(SHARED_DATA as *const u8, CONTENT.len()) != CONTENT
            }
        {
            let _ = microsystem_user_rt::debug_write(
                b"[shell] filesystem protocol failed stage=descriptor-contents\n",
            );
            return Err(Status::Io);
        }
        step!(
            "descriptor-close",
            fs_request(FsOperation::Close, "", &[], descriptor)
        );

        step!(
            "rename",
            fs_call(FsOperation::Rename, &source, destination.as_bytes())
        );
        let stat = step!(
            "stat-renamed",
            fs_request(FsOperation::Stat, &destination, &[], 0)
        );
        if stat.words[0] != 1 || stat.words[1] as usize != CONTENT.len() {
            let _ = microsystem_user_rt::debug_write(
                b"[shell] filesystem protocol failed stage=stat-contents\n",
            );
            return Err(Status::Io);
        }
        let listed = step!("list", fs_call(FsOperation::List, &directory, &[]));
        let listing = unsafe { core::slice::from_raw_parts(SHARED_DATA as *const u8, listed) };
        if !listing
            .windows(destination.len())
            .any(|candidate| candidate == destination.as_bytes())
        {
            let _ = microsystem_user_rt::debug_write(
                b"[shell] filesystem protocol failed stage=list-contents\n",
            );
            return Err(Status::Io);
        }
        Ok(())
    })();
    if let Err(status) = verified {
        let _ = fs_call(FsOperation::Unlink, &destination, &[]);
        let _ = fs_call(FsOperation::Unlink, &source, &[]);
        let _ = fs_call(FsOperation::Unlink, &directory, &[]);
        return Err(status);
    }
    step!(
        "unlink-destination",
        fs_call(FsOperation::Unlink, &destination, &[])
    );
    step!(
        "unlink-directory",
        fs_call(FsOperation::Unlink, &directory, &[])
    );
    step!("sync", fs_call(FsOperation::Sync, "/", &[]));
    Ok(())
}

fn output(arguments: fmt::Arguments<'_>) {
    let mut buffer = Text::new();
    let _ = buffer.write_fmt(arguments);
    console_write(buffer.as_bytes());
}

struct Text {
    bytes: [u8; 256],
    length: usize,
}

impl Text {
    const fn new() -> Self {
        Self {
            bytes: [0; 256],
            length: 0,
        }
    }

    fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.length]
    }
}

impl Write for Text {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        let end = self.length.checked_add(text.len()).ok_or(fmt::Error)?;
        if end > self.bytes.len() {
            return Err(fmt::Error);
        }
        self.bytes[self.length..end].copy_from_slice(text.as_bytes());
        self.length = end;
        Ok(())
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo<'_>) -> ! {
    microsystem_user_rt::exit(1)
}
