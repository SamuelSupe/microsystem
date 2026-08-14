#![no_std]
#![no_main]

use core::fmt::{self, Write};
use core::panic::PanicInfo;
use microsystem_abi::{Message, Rights, Status, boot_cap, process, protocol, script, time};
use microsystem_console::{INLINE_BYTES, Operation as ConsoleOperation};
use microsystem_fs::Operation as FsOperation;
use microsystem_shell::{Command, parse};

const SHARED_DATA: usize = 0x005e_0000;
const SHARED_BYTES: usize = 4096;

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
    console_write(b"[service] shell ready\nmicro> ");

    let mut line = [0u8; 4096];
    let mut length = 0usize;
    loop {
        match console_read() {
            Ok(Some(b'\r' | b'\n')) => {
                console_write(b"\n");
                if let Ok(command) = core::str::from_utf8(&line[..length]) {
                    execute(command);
                }
                length = 0;
                console_write(b"micro> ");
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

fn execute(line: &str) {
    match parse(line) {
        Ok(Command::Empty) => {}
        Ok(Command::Help) => {
            console_write(
                b"help ps uptime ls cat stat write mkdir mv rm sync run mica shutdown\n",
            )
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
        Ok(Command::Ls(path)) => fs_print(FsOperation::List, path),
        Ok(Command::Cat(path)) => fs_print(FsOperation::Read, path),
        Ok(Command::Stat(path)) => match fs_request(FsOperation::Stat, path, &[], 0) {
            Ok(reply) if reply.words[0] == 1 => {
                output(format_args!("file {} bytes\n", reply.words[1]));
            }
            Ok(reply) if reply.words[0] == 2 => {
                output(format_args!("directory {} entries\n", reply.words[1]));
            }
            _ => console_write(b"stat: failed\n"),
        },
        Ok(Command::Write { path, value }) => {
            if fs_call(FsOperation::Write, path, value.as_bytes()).is_ok() {
                console_write(b"write: ok\n");
            } else {
                console_write(b"write: failed\n");
            }
        }
        Ok(Command::Mkdir(path)) => {
            if fs_call(FsOperation::Mkdir, path, &[]).is_ok() {
                console_write(b"mkdir: ok\n");
            } else {
                console_write(b"mkdir: failed\n");
            }
        }
        Ok(Command::Rename {
            source,
            destination,
        }) => {
            if fs_call(FsOperation::Rename, source, destination.as_bytes()).is_ok() {
                console_write(b"mv: ok\n");
            } else {
                console_write(b"mv: failed\n");
            }
        }
        Ok(Command::Unlink(path)) => {
            if fs_call(FsOperation::Unlink, path, &[]).is_ok() {
                console_write(b"rm: ok\n");
            } else {
                console_write(b"rm: failed\n");
            }
        }
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
        Ok(Command::MicaFile(path)) => run_mica_file(path, b"", b"", 10_000_000_000, false),
        Ok(Command::MicaRepl) => run_mica_repl(b"", 5_000_000_000),
        Ok(Command::MicaArgs(arguments)) => run_mica_arguments(arguments),
        Ok(Command::Sync) => {
            if fs_call(FsOperation::Sync, "/", &[]).is_ok() {
                console_write(b"sync: ok\n");
            } else {
                console_write(b"sync: failed\n");
            }
        }
        Ok(Command::Shutdown) => microsystem_user_rt::exit(0),
        Err(_) => console_write(b"unknown or invalid command\n"),
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
        core::arch::asm!("dmb ish", options(nostack, preserves_flags));
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
    unsafe { core::arch::asm!("dmb ish", options(nostack, preserves_flags)) };
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

fn run_mica(
    source: &[u8],
    policy: &[u8],
    argv: &[u8],
    path: &str,
    timeout_ns: u64,
    mode: u16,
    gui: bool,
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
    output(format_args!("mica: pid={} status={}\n", pid, status));
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
    if path.is_empty() || path.len() > 255 || path.len() + data.len() > SHARED_BYTES {
        if descriptor == 0 || !path.is_empty() || path.len() + data.len() > SHARED_BYTES {
            return Err(Status::Invalid);
        }
    }
    unsafe {
        core::ptr::copy_nonoverlapping(path.as_ptr(), SHARED_DATA as *mut u8, path.len());
        core::ptr::copy_nonoverlapping(
            data.as_ptr(),
            (SHARED_DATA + path.len()) as *mut u8,
            data.len(),
        );
        core::arch::asm!("dmb ish", options(nostack, preserves_flags));
    }
    let mut request = Message::new(protocol::FILESYSTEM, operation as u16);
    request.words[0] = path.len() as u64;
    request.words[1] = data.len() as u64;
    request.words[2] = descriptor;
    request.caps[0] = boot_cap::SHARED_FILESYSTEM_FRAME;
    let mut reply = Message::new(protocol::FILESYSTEM, 0);
    microsystem_user_rt::ipc_call(boot_cap::FILESYSTEM_ENDPOINT, &request, &mut reply, 0)?;
    if reply.words[5] as i64 != 0 || reply.words[0] as usize > SHARED_BYTES {
        return Err(Status::Io);
    }
    unsafe { core::arch::asm!("dmb ish", options(nostack, preserves_flags)) };
    Ok(reply)
}

fn verify_filesystem_protocol() -> Result<(), Status> {
    const DIRECTORY: &str = "/protocol-proof";
    const SOURCE: &str = "/protocol-proof/a";
    const DESTINATION: &str = "/protocol-proof/b";
    const CONTENT: &[u8] = b"descriptor-data";

    let _ = fs_call(FsOperation::Unlink, DESTINATION, &[]);
    let _ = fs_call(FsOperation::Unlink, SOURCE, &[]);
    let _ = fs_call(FsOperation::Unlink, DIRECTORY, &[]);
    fs_call(FsOperation::Mkdir, DIRECTORY, &[])?;
    fs_call(FsOperation::Write, SOURCE, b"path-data")?;

    let descriptor = fs_request(FsOperation::Open, SOURCE, &[], 0)?.words[0];
    if descriptor == 0 {
        return Err(Status::Io);
    }
    fs_request(FsOperation::Write, "", CONTENT, descriptor)?;
    fs_request(FsOperation::Fsync, "", &[], descriptor)?;
    let read = fs_request(FsOperation::Read, "", &[], descriptor)?;
    if read.words[0] as usize != CONTENT.len()
        || unsafe {
            core::slice::from_raw_parts(SHARED_DATA as *const u8, CONTENT.len()) != CONTENT
        }
    {
        return Err(Status::Io);
    }
    fs_request(FsOperation::Close, "", &[], descriptor)?;

    fs_call(FsOperation::Rename, SOURCE, DESTINATION.as_bytes())?;
    let stat = fs_request(FsOperation::Stat, DESTINATION, &[], 0)?;
    if stat.words[0] != 1 || stat.words[1] as usize != CONTENT.len() {
        return Err(Status::Io);
    }
    let listed = fs_call(FsOperation::List, DIRECTORY, &[])?;
    let listing = unsafe { core::slice::from_raw_parts(SHARED_DATA as *const u8, listed) };
    if !listing
        .windows(DESTINATION.len())
        .any(|candidate| candidate == DESTINATION.as_bytes())
    {
        return Err(Status::Io);
    }
    fs_call(FsOperation::Unlink, DESTINATION, &[])?;
    fs_call(FsOperation::Unlink, DIRECTORY, &[])?;
    fs_call(FsOperation::Sync, "/", &[])?;
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
