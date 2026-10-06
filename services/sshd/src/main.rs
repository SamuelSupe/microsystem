#![no_std]
#![no_main]

extern crate alloc;

use alloc::vec::Vec;
use core::future::Future;
use core::panic::PanicInfo;
use core::pin::Pin;
use core::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use core::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};

use embedded_io_async::{ErrorKind, ErrorType, Read, Write};
use microsystem_abi::{
    Message, Rights, Status, SystemStats, boot_cap, filesystem, message_cap_move, network, process,
    protocol, script,
};
use microsystem_mica::PermissionSet;
use microsystem_shell::{Command as ShellCommand, parse};
use rand::{CryptoRng, RngCore};
use zssh::ed25519_dalek::SigningKey;
use zssh::{AuthMethod, Behavior, PublicKey, Request, SecretKey, Transport};

const NETWORK_SHARED_VA: u64 = 0x0058_0000;
const SCRIPT_SESSION_VA: u64 = 0x0059_0000;
const FILESYSTEM_SHARED_VA: u64 = 0x005e_0000;
const FILESYSTEM_SHARED_BYTES: usize = 4096;
const SSH_MICA_POLICY: &str = "/.system/ssh/mica-policy";
static mut SSH_PACKET: [u8; 16384] = [0; 16384];
static SSH_FIRST_READ: AtomicBool = AtomicBool::new(false);
static SSH_FIRST_WRITE: AtomicBool = AtomicBool::new(false);
static SSH_FIRST_RANDOM: AtomicBool = AtomicBool::new(false);
static SSH_CREDENTIAL: AtomicU64 = AtomicU64::new(0);
static SSH_AUTHENTICATED: AtomicBool = AtomicBool::new(false);
static SSH_CHANNEL_ACTIVE: AtomicBool = AtomicBool::new(false);
static SSH_IO_DEADLINE: AtomicU64 = AtomicU64::new(0);
static SSH_CONNECTION: AtomicU64 = AtomicU64::new(0);
static SSH_POLICY_BYTES: AtomicUsize = AtomicUsize::new(0);
static mut SSH_POLICY: [u8; script::POLICY_BYTES] = [0; script::POLICY_BYTES];

#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    let _ = microsystem_user_rt::debug_write(b"[user] sshd service ELF entered EL0\n");
    let _shared = NetdStream::initialize().unwrap_or_else(|_| microsystem_user_rt::exit(3));
    let public = SigningKey::from_bytes(&host_secret())
        .verifying_key()
        .to_bytes();
    let mut hex = [0; 64];
    for (index, byte) in public.iter().enumerate() {
        hex[index * 2] = b"0123456789abcdef"[(byte >> 4) as usize];
        hex[index * 2 + 1] = b"0123456789abcdef"[(byte & 15) as usize];
    }
    let prefix = b"[ssh] host public key ed25519=";
    let mut line = [0; 128];
    line[..prefix.len()].copy_from_slice(prefix);
    line[prefix.len()..prefix.len() + 64].copy_from_slice(&hex);
    line[prefix.len() + 64] = b'\n';
    let _ = microsystem_user_rt::debug_write(&line[..prefix.len() + 65]);
    let _ = microsystem_user_rt::debug_write(
        b"[ssh] sshd ready address=10.0.2.15 port=22 auth=publickey accounts=persistent\n",
    );
    let _ = microsystem_user_rt::service_online();

    loop {
        SSH_CREDENTIAL.store(0, Ordering::Release);
        SSH_AUTHENTICATED.store(false, Ordering::Release);
        SSH_CHANNEL_ACTIVE.store(false, Ordering::Release);
        let stream = NetdStream::wait_accept();
        SSH_IO_DEADLINE.store(
            microsystem_user_rt::clock_now()
                .unwrap_or(0)
                .saturating_add(30_000_000_000),
            Ordering::Release,
        );
        {
            let behavior = ServerBehavior::new(stream);
            let packet = unsafe { &mut *core::ptr::addr_of_mut!(SSH_PACKET) };
            packet.fill(0);
            let mut transport = Transport::new(packet, behavior);
            let graceful = block_on(serve(&mut transport)).is_ok();
            stream.close(graceful);
        }
        let cookie = SSH_CREDENTIAL.swap(0, Ordering::AcqRel);
        let mut request = Message::new(
            protocol::IDENTITY,
            microsystem_abi::identity::Operation::DropCredential as u16,
        );
        request.words[0] = cookie;
        let mut reply = Message::new(protocol::IDENTITY, 0);
        let deadline = microsystem_user_rt::clock_now()
            .unwrap_or(0)
            .saturating_add(1_000_000_000);
        let _ = microsystem_user_rt::ipc_call(
            boot_cap::IDENTITY_ENDPOINT,
            &request,
            &mut reply,
            deadline,
        );
        SSH_AUTHENTICATED.store(false, Ordering::Release);
        SSH_CONNECTION.store(0, Ordering::Release);
    }
}

async fn serve(transport: &mut Transport<'_, ServerBehavior>) -> Result<(), ()> {
    let mut channel = transport.accept().await.map_err(|_| ())?;
    SSH_CHANNEL_ACTIVE.store(true, Ordering::Release);
    SSH_IO_DEADLINE.store(
        microsystem_user_rt::clock_now()
            .map_err(|_| ())?
            .saturating_add(300_000_000_000),
        Ordering::Release,
    );
    let mut accepted = Message::new(
        protocol::IDENTITY,
        microsystem_abi::identity::Operation::SessionAccepted as u16,
    );
    accepted.words[0] = SSH_CREDENTIAL.load(Ordering::Acquire);
    let mut audited = Message::new(protocol::IDENTITY, 0);
    let deadline = microsystem_user_rt::clock_now()
        .map_err(|_| ())?
        .saturating_add(5_000_000_000);
    microsystem_user_rt::ipc_call(
        boot_cap::IDENTITY_ENDPOINT,
        &accepted,
        &mut audited,
        deadline,
    )
    .map_err(|_| ())?;
    if audited.words[5] != 0 {
        return Err(());
    }
    match channel.request() {
        Request::Exec(command) => {
            let status = write_command(&mut channel, command).await?;
            channel.exit(status).await.map_err(|_| ())?;
        }
        Request::Shell => {
            channel
                .write_all_stdout(b"MicroSystem SSH\r\nmicro> ")
                .await
                .map_err(|_| ())?;
            let mut line = [0u8; 256];
            let mut length = 0usize;
            loop {
                let mut byte = [0u8; 1];
                if channel.read_exact_stdin(&mut byte).await.map_err(|_| ())? == 0 {
                    break;
                }
                match byte[0] {
                    b'\r' | b'\n' => {
                        channel.write_all_stdout(b"\r\n").await.map_err(|_| ())?;
                        let command = core::str::from_utf8(&line[..length])
                            .ok()
                            .map(parse_command)
                            .unwrap_or(SshCommand::Invalid);
                        if command == SshCommand::Exit {
                            break;
                        }
                        if command == SshCommand::MicaRepl {
                            let _ = run_ssh_mica_repl(&mut channel).await?;
                        } else {
                            let _ = write_command(&mut channel, command).await?;
                        }
                        length = 0;
                        channel.write_all_stdout(b"micro> ").await.map_err(|_| ())?;
                    }
                    0x7f | 0x08 if length > 0 => {
                        length -= 1;
                        channel
                            .write_all_stdout(b"\x08 \x08")
                            .await
                            .map_err(|_| ())?;
                    }
                    value if (0x20..=0x7e).contains(&value) && length < line.len() => {
                        line[length] = value;
                        length += 1;
                        channel.write_all_stdout(&[value]).await.map_err(|_| ())?;
                    }
                    _ => {}
                }
            }
            channel.exit(0).await.map_err(|_| ())?;
        }
    }
    Ok(())
}

async fn write_command(
    channel: &mut zssh::Channel<'_, '_, ServerBehavior>,
    command: SshCommand,
) -> Result<u32, ()> {
    if !session_valid() {
        return Err(());
    }
    match command {
        SshCommand::Help => channel
            .write_all_stdout(b"help uptime ps clear echo whoami user mica exit\r\n")
            .await
            .map_err(|_| ())?,
        SshCommand::Uptime => {
            let mut output = [0u8; 96];
            let mut cursor = copy(&mut output, 0, b"uptime: ");
            cursor = decimal(
                &mut output,
                cursor,
                microsystem_user_rt::clock_now().unwrap_or(0) / 1_000_000,
            );
            cursor = copy(&mut output, cursor, b" ms\r\n");
            channel
                .write_all_stdout(&output[..cursor])
                .await
                .map_err(|_| ())?;
        }
        SshCommand::Ps => {
            let mut stats = SystemStats::default();
            if microsystem_user_rt::system_stats(boot_cap::SYSTEM_INFO, &mut stats).is_err() {
                channel
                    .write_all_stderr(b"ps: unavailable\r\n")
                    .await
                    .map_err(|_| ())?;
                return Ok(1);
            }
            let mut output = [0u8; 192];
            let mut cursor = copy(&mut output, 0, b"tasks runnable=");
            cursor = decimal(&mut output, cursor, stats.runnable_threads as u64);
            cursor = copy(&mut output, cursor, b" blocked=");
            cursor = decimal(&mut output, cursor, stats.blocked_threads as u64);
            cursor = copy(&mut output, cursor, b" free-frames=");
            cursor = decimal(&mut output, cursor, stats.free_frames);
            cursor = copy(&mut output, cursor, b"\r\n");
            channel
                .write_all_stdout(&output[..cursor])
                .await
                .map_err(|_| ())?;
        }
        SshCommand::Clear => channel
            .write_all_stdout(b"\x1b[2J\x1b[H")
            .await
            .map_err(|_| ())?,
        SshCommand::Echo => channel
            .write_all_stdout(b"echo\r\n")
            .await
            .map_err(|_| ())?,
        SshCommand::MicaEval { source, bytes } => {
            return run_mica_eval(channel, &source[..bytes as usize], &[]).await;
        }
        SshCommand::User { arguments, bytes } => {
            unsafe {
                core::ptr::copy_nonoverlapping(
                    arguments.as_ptr(),
                    FILESYSTEM_SHARED_VA as *mut u8,
                    bytes as usize,
                );
            }
            let mut request = Message::new(
                protocol::IDENTITY,
                microsystem_abi::identity::Operation::Command as u16,
            );
            request.words[0] = bytes as u64;
            request.words[3] = SSH_CREDENTIAL.load(Ordering::Acquire);
            request.caps[0] = boot_cap::SSH_FILESYSTEM_FRAME;
            let mut reply = Message::new(protocol::IDENTITY, 0);
            let deadline = microsystem_user_rt::clock_now()
                .map_err(|_| ())?
                .saturating_add(30_000_000_000);
            microsystem_user_rt::ipc_call(
                boot_cap::IDENTITY_ENDPOINT,
                &request,
                &mut reply,
                deadline,
            )
            .map_err(|_| ())?;
            let length = reply.words[0] as usize;
            if length > 4096 {
                return Err(());
            }
            let output =
                unsafe { core::slice::from_raw_parts(FILESYSTEM_SHARED_VA as *const u8, length) }
                    .to_vec();
            channel.write_all_stdout(&output).await.map_err(|_| ())?;
            return Ok(u32::from(reply.words[5] != 0));
        }
        SshCommand::MicaArgs { arguments, bytes } => {
            return run_mica_arguments(channel, &arguments[..bytes as usize]).await;
        }
        SshCommand::MicaRepl => {
            channel
                .write_all_stderr(b"mica: interactive REPL requires an SSH shell\r\n")
                .await
                .map_err(|_| ())?;
            return Ok(2);
        }
        SshCommand::Exit => return Ok(0),
        SshCommand::Empty => {}
        SshCommand::Invalid => {
            channel
                .write_all_stderr(b"unknown command\r\n")
                .await
                .map_err(|_| ())?;
            return Ok(127);
        }
    }
    Ok(0)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SshCommand {
    Empty,
    Help,
    Uptime,
    Ps,
    Clear,
    Echo,
    MicaEval { source: [u8; 256], bytes: u16 },
    MicaArgs { arguments: [u8; 256], bytes: u16 },
    User { arguments: [u8; 256], bytes: u16 },
    MicaRepl,
    Exit,
    Invalid,
}

fn parse_command(command: &str) -> SshCommand {
    if command.trim() == "exit" {
        return SshCommand::Exit;
    }
    match parse(command) {
        Ok(ShellCommand::Empty) => SshCommand::Empty,
        Ok(ShellCommand::Help) => SshCommand::Help,
        Ok(ShellCommand::Uptime) => SshCommand::Uptime,
        Ok(ShellCommand::Ps) => SshCommand::Ps,
        Ok(ShellCommand::Echo(_)) => SshCommand::Echo,
        Ok(ShellCommand::Clear) => SshCommand::Clear,
        Ok(ShellCommand::User(value)) if value.len() <= 256 => {
            let mut arguments = [0; 256];
            arguments[..value.len()].copy_from_slice(value.as_bytes());
            SshCommand::User {
                arguments,
                bytes: value.len() as u16,
            }
        }
        Ok(ShellCommand::MicaEval(source)) if source.len() <= 256 => {
            let mut bytes = [0u8; 256];
            bytes[..source.len()].copy_from_slice(source.as_bytes());
            SshCommand::MicaEval {
                source: bytes,
                bytes: source.len() as u16,
            }
        }
        Ok(ShellCommand::MicaRepl) => SshCommand::MicaRepl,
        Ok(ShellCommand::MicaFile(path)) if path.len() <= 256 => {
            let mut arguments = [0u8; 256];
            arguments[..path.len()].copy_from_slice(path.as_bytes());
            SshCommand::MicaArgs {
                arguments,
                bytes: path.len() as u16,
            }
        }
        Ok(ShellCommand::MicaArgs(value)) if value.len() <= 256 => {
            let mut arguments = [0u8; 256];
            arguments[..value.len()].copy_from_slice(value.as_bytes());
            SshCommand::MicaArgs {
                arguments,
                bytes: value.len() as u16,
            }
        }
        _ if command.trim().starts_with("echo") => SshCommand::Echo,
        _ if command.trim() == "clear" => SshCommand::Clear,
        _ => SshCommand::Invalid,
    }
}

async fn run_mica_arguments(
    channel: &mut zssh::Channel<'_, '_, ServerBehavior>,
    arguments: &[u8],
) -> Result<u32, ()> {
    let Ok(arguments) = core::str::from_utf8(arguments) else {
        return mica_cli_error(channel, b"mica: arguments are not UTF-8\r\n").await;
    };
    let (options, eval_source) = arguments
        .find("-e ")
        .map(|offset| {
            (
                arguments[..offset].trim(),
                Some(arguments[offset + 3..].trim()),
            )
        })
        .unwrap_or((arguments, None));
    let mut requested = Vec::new();
    let mut timeout_ns = 10_000_000_000u64;
    let mut gui = false;
    let mut path = None;
    let mut argv = [0u8; script::STDIN_BYTES];
    let mut argv_bytes = 0usize;
    let mut words = options.split_whitespace().peekable();
    while let Some(option) = words.next() {
        match option {
            "--gui" => gui = true,
            "--allow" => {
                let Some(rule) = words.next() else {
                    return mica_cli_error(channel, b"mica: --allow requires a rule\r\n").await;
                };
                requested.push(rule);
            }
            "--timeout" => {
                let Some(value) = words.next() else {
                    return mica_cli_error(channel, b"mica: --timeout requires a duration\r\n")
                        .await;
                };
                let milliseconds = parse_duration_ms(value);
                let Some(milliseconds @ 1..=86_400_000) = milliseconds else {
                    return mica_cli_error(channel, b"mica: timeout must be 1ms..24h\r\n").await;
                };
                timeout_ns = milliseconds * 1_000_000;
            }
            "--" => {
                for argument in words.by_ref() {
                    if argv_bytes + argument.len() + 1 > argv.len() {
                        return mica_cli_error(channel, b"mica: arguments too large\r\n").await;
                    }
                    argv[argv_bytes..argv_bytes + argument.len()]
                        .copy_from_slice(argument.as_bytes());
                    argv_bytes += argument.len() + 1;
                }
            }
            value if !value.starts_with('-') && path.is_none() => path = Some(value),
            _ => return mica_cli_error(channel, b"mica: unknown option\r\n").await,
        }
    }
    if !gui && timeout_ns > 60_000_000_000 {
        return mica_cli_error(channel, b"mica: non-GUI timeout must be at most 60s\r\n").await;
    }
    let policy = match ssh_policy(&requested) {
        Ok(policy) => policy,
        Err(message) => return mica_cli_error(channel, message).await,
    };
    if let Some(source) = eval_source {
        if gui {
            return mica_cli_error(channel, b"mica: --gui requires a script file\r\n").await;
        }
        let source = unquote(source);
        if source.is_empty() {
            return mica_cli_error(channel, b"mica: -e requires source\r\n").await;
        }
        return run_mica_session(
            channel,
            script::MODE_EVAL,
            source.as_bytes(),
            &policy,
            "",
            &argv[..argv_bytes],
            timeout_ns,
            false,
        )
        .await;
    }
    let Some(path) = path else {
        return mica_cli_error(channel, b"mica: SSH REPL requires an interactive shell\r\n").await;
    };
    let prefix = match read_file_prefix(path) {
        Ok(prefix) => prefix,
        Err(_) => return mica_cli_error(channel, b"mica: cannot read script\r\n").await,
    };
    let Ok(prefix) = core::str::from_utf8(&prefix) else {
        return mica_cli_error(channel, b"mica: script is not UTF-8\r\n").await;
    };
    let Some(manifest_bytes) = manifest_prefix_bytes(prefix) else {
        return mica_cli_error(channel, b"mica: missing --!mica 1 directive\r\n").await;
    };
    run_mica_session(
        channel,
        script::MODE_FILE,
        &prefix.as_bytes()[..manifest_bytes],
        &policy,
        path,
        &argv[..argv_bytes],
        timeout_ns,
        gui,
    )
    .await
}

async fn mica_cli_error(
    channel: &mut zssh::Channel<'_, '_, ServerBehavior>,
    message: &[u8],
) -> Result<u32, ()> {
    channel.write_all_stderr(message).await.map_err(|_| ())?;
    Ok(2)
}

fn parse_duration_ms(value: &str) -> Option<u64> {
    value
        .strip_suffix("ms")
        .and_then(|value| value.parse().ok())
        .or_else(|| {
            value
                .strip_suffix('s')
                .and_then(|value| value.parse::<u64>().ok())
                .and_then(|seconds| seconds.checked_mul(1000))
        })
}

fn unquote(value: &str) -> &str {
    value
        .strip_prefix('\'')
        .and_then(|value| value.strip_suffix('\''))
        .or_else(|| {
            value
                .strip_prefix('"')
                .and_then(|value| value.strip_suffix('"'))
        })
        .unwrap_or(value)
}

fn ssh_policy(requested: &[&str]) -> Result<Vec<u8>, &'static [u8]> {
    let maximum = ssh_maximum_policy()?;
    let requested = PermissionSet::from_rules(requested.iter().copied())
        .map_err(|_| b"mica: requested permission is invalid\r\n" as &'static [u8])?;
    let effective = requested.intersect(&maximum);
    let mut output = Vec::new();
    for permission in effective.entries() {
        let rule = permission.rule();
        if output.len().saturating_add(rule.len() + 1) > script::POLICY_BYTES {
            return Err(b"mica: effective policy is too large\r\n");
        }
        output.extend_from_slice(rule.as_bytes());
        output.push(b'\n');
    }
    Ok(output)
}

fn ssh_maximum_policy() -> Result<PermissionSet, &'static [u8]> {
    let cached = SSH_POLICY_BYTES.load(Ordering::Acquire);
    if cached != 0 {
        let bytes = unsafe {
            core::slice::from_raw_parts(core::ptr::addr_of!(SSH_POLICY).cast::<u8>(), cached)
        };
        let policy = core::str::from_utf8(bytes)
            .map_err(|_| b"mica: SSH policy is not UTF-8\r\n" as &'static [u8])?;
        return PermissionSet::from_rules(policy.lines().filter(|line| !line.trim().is_empty()))
            .map_err(|_| b"mica: SSH policy is invalid\r\n" as &'static [u8]);
    }
    let bytes = read_file_prefix(SSH_MICA_POLICY).map_err(ssh_policy_read_error)?;
    let policy = core::str::from_utf8(&bytes)
        .map_err(|_| b"mica: SSH policy is not UTF-8\r\n" as &'static [u8])?;
    let parsed = PermissionSet::from_rules(policy.lines().filter(|line| !line.trim().is_empty()))
        .map_err(|_| b"mica: SSH policy is invalid\r\n" as &'static [u8])?;
    if bytes.is_empty() || bytes.len() > script::POLICY_BYTES {
        return Err(b"mica: SSH policy is invalid\r\n");
    }
    unsafe {
        core::ptr::copy_nonoverlapping(
            bytes.as_ptr(),
            core::ptr::addr_of_mut!(SSH_POLICY).cast::<u8>(),
            bytes.len(),
        );
    }
    SSH_POLICY_BYTES.store(bytes.len(), Ordering::Release);
    Ok(parsed)
}

fn ssh_policy_read_error(status: Status) -> &'static [u8] {
    match status {
        Status::NotFound => b"mica: SSH policy is unavailable (not found)\r\n",
        Status::AccessDenied => b"mica: SSH policy is unavailable (access denied)\r\n",
        Status::BadCapability => b"mica: SSH policy is unavailable (bad capability)\r\n",
        Status::Fault => b"mica: SSH policy is unavailable (fault)\r\n",
        _ => b"mica: SSH policy is unavailable (I/O)\r\n",
    }
}

fn read_file_prefix(path: &str) -> Result<Vec<u8>, Status> {
    let mut output = Vec::new();
    loop {
        let reply =
            filesystem_request(filesystem::Operation::ReadRange, path, output.len() as u64)?;
        let bytes = reply.words[0] as usize;
        if bytes > FILESYSTEM_SHARED_BYTES {
            return Err(Status::Fault);
        }
        let chunk =
            unsafe { core::slice::from_raw_parts(FILESYSTEM_SHARED_VA as *const u8, bytes) };
        if output.len().saturating_add(bytes) > script::POLICY_BYTES.max(script::SOURCE_BYTES) {
            return Err(Status::NoMemory);
        }
        output.extend_from_slice(chunk);
        if bytes < FILESYSTEM_SHARED_BYTES || output.len() >= script::SOURCE_BYTES {
            break;
        }
    }
    Ok(output)
}

fn filesystem_request(
    operation: filesystem::Operation,
    path: &str,
    offset: u64,
) -> Result<Message, Status> {
    if path.is_empty() || path.len() > 255 {
        return Err(Status::Invalid);
    }
    unsafe {
        core::ptr::copy_nonoverlapping(path.as_ptr(), FILESYSTEM_SHARED_VA as *mut u8, path.len());
        microsystem_user_rt::fence();
    }
    let mut request = Message::new(protocol::FILESYSTEM, operation as u16);
    request.words[0] = path.len() as u64;
    request.words[2] = offset;
    request.caps[0] = boot_cap::SSH_FILESYSTEM_FRAME;
    let mut reply = Message::new(protocol::FILESYSTEM, 0);
    authorized_call(boot_cap::FILESYSTEM_ENDPOINT, &request, &mut reply, 0)?;
    let status = status_from_word(reply.words[5]);
    if status != Status::Ok {
        return Err(status);
    }
    microsystem_user_rt::fence();
    Ok(reply)
}

fn status_from_word(value: u64) -> Status {
    match value as i64 {
        value if value == Status::Ok as i64 => Status::Ok,
        value if value == Status::Invalid as i64 => Status::Invalid,
        value if value == Status::AccessDenied as i64 => Status::AccessDenied,
        value if value == Status::NotFound as i64 => Status::NotFound,
        value if value == Status::Busy as i64 => Status::Busy,
        value if value == Status::NoMemory as i64 => Status::NoMemory,
        value if value == Status::Fault as i64 => Status::Fault,
        value if value == Status::BadCapability as i64 => Status::BadCapability,
        value if value == Status::NotSupported as i64 => Status::NotSupported,
        value if value == Status::TimedOut as i64 => Status::TimedOut,
        value if value == Status::NoSpace as i64 => Status::NoSpace,
        _ => Status::Io,
    }
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

async fn run_ssh_mica_repl(channel: &mut zssh::Channel<'_, '_, ServerBehavior>) -> Result<u32, ()> {
    let prepare = Message::new(protocol::PROCESS, process::Operation::SpawnScript as u16);
    let mut prepared = Message::new(protocol::PROCESS, 0);
    if authorized_call(boot_cap::PROCESS_ENDPOINT, &prepare, &mut prepared, 0).is_err()
        || prepared.words[5] as i64 != Status::Ok as i64
        || prepared.caps[0] == microsystem_abi::CapHandle::INVALID
    {
        return mica_cli_error(channel, b"mica: session allocation failed\r\n").await;
    }
    let region = prepared.caps[0];
    if microsystem_user_rt::frame_map(
        region,
        SCRIPT_SESSION_VA,
        Rights(Rights::READ.0 | Rights::WRITE.0),
    )
    .is_err()
    {
        let _ = microsystem_user_rt::cap_delete(region);
        return mica_cli_error(channel, b"mica: session mapping failed\r\n").await;
    }
    unsafe { core::ptr::write_bytes(SCRIPT_SESSION_VA as *mut u8, 0, script::SESSION_BYTES) };
    unsafe {
        *(&mut *(SCRIPT_SESSION_VA as *mut script::SessionHeaderV1)) = script::SessionHeaderV1 {
            magic: script::SESSION_MAGIC,
            version: script::VERSION,
            mode: script::MODE_REPL,
            timeout_ns: 5_000_000_000,
            instruction_limit: 10_000_000,
            ..script::SessionHeaderV1::default()
        };
    }
    let _ = microsystem_user_rt::frame_unmap(region, SCRIPT_SESSION_VA);
    let mut request = Message::new(protocol::PROCESS, process::Operation::SpawnScript as u16);
    request.words[0] = 1;
    request.caps[0] = region;
    request.flags |= message_cap_move(0);
    let mut reply = Message::new(protocol::PROCESS, 0);
    if authorized_call(boot_cap::PROCESS_ENDPOINT, &request, &mut reply, 0).is_err()
        || reply.words[5] as i64 != Status::Ok as i64
        || reply.caps[0] == microsystem_abi::CapHandle::INVALID
        || reply.caps[1] == microsystem_abi::CapHandle::INVALID
    {
        return mica_cli_error(channel, b"mica: REPL launch failed\r\n").await;
    }
    let pid = reply.words[0];
    let region = reply.caps[0];
    let transferred = reply.caps[1];
    let notification = match microsystem_user_rt::cap_copy(transferred, Rights::WRITE) {
        Ok(notification) => notification,
        Err(_) => {
            let _ = microsystem_user_rt::cap_delete(transferred);
            let _ = microsystem_user_rt::cap_delete(region);
            let _ = kill_script(pid);
            return mica_cli_error(channel, b"mica: REPL notification failed\r\n").await;
        }
    };
    let _ = microsystem_user_rt::cap_delete(transferred);
    if wait_ssh_repl_output(region).is_err() {
        let _ = kill_script(pid);
        cleanup_script_handles(region, notification);
        return mica_cli_error(channel, b"mica: REPL did not become ready\r\n").await;
    }
    write_repl_output(channel, region).await?;
    channel.write_all_stdout(b"mica> ").await.map_err(|_| ())?;

    let mut line = [0u8; 4096];
    let mut length = 0usize;
    let mut disconnected = false;
    loop {
        let mut byte = [0u8; 1];
        match channel.read_exact_stdin(&mut byte).await {
            Ok(0) | Err(_) => {
                disconnected = true;
                break;
            }
            Ok(_) => {}
        }
        match byte[0] {
            b'\r' | b'\n' => {
                channel.write_all_stdout(b"\r\n").await.map_err(|_| ())?;
                let exit = &line[..length] == b"exit";
                if send_ssh_repl_input(region, notification, &line[..length]).is_err() {
                    disconnected = true;
                    break;
                }
                length = 0;
                let bits = wait_ssh_repl_output(region);
                write_repl_output(channel, region).await?;
                if exit || bits.is_ok_and(|bits| bits & script::EVENT_EXIT != 0) {
                    break;
                }
                if bits.is_err() {
                    disconnected = true;
                    break;
                }
                channel.write_all_stdout(b"mica> ").await.map_err(|_| ())?;
            }
            3 => {
                length = 0;
                channel
                    .write_all_stdout(b"^C\r\nmica> ")
                    .await
                    .map_err(|_| ())?;
            }
            4 if length == 0 => {
                let _ = send_ssh_repl_input(region, notification, b"exit");
                let _ = wait_ssh_repl_output(region);
                write_repl_output(channel, region).await?;
                break;
            }
            0x7f | 0x08 if length > 0 => {
                length -= 1;
                channel
                    .write_all_stdout(b"\x08 \x08")
                    .await
                    .map_err(|_| ())?;
            }
            value if (0x20..=0xff).contains(&value) && length < line.len() => {
                line[length] = value;
                length += 1;
                channel.write_all_stdout(&[value]).await.map_err(|_| ())?;
            }
            _ => {}
        }
    }
    if disconnected {
        let _ = kill_script(pid);
    }
    let status = wait_script_status(pid).unwrap_or(if disconnected { 130 } else { 1 });
    cleanup_script_handles(region, notification);
    Ok(status as u32)
}

fn send_ssh_repl_input(
    region: microsystem_abi::CapHandle,
    notification: microsystem_abi::CapHandle,
    input: &[u8],
) -> Result<(), Status> {
    microsystem_user_rt::frame_map(
        region,
        SCRIPT_SESSION_VA,
        Rights(Rights::READ.0 | Rights::WRITE.0),
    )?;
    let header = unsafe { &mut *(SCRIPT_SESSION_VA as *mut script::SessionHeaderV1) };
    header.stdin_head = input.len() as u32;
    header.stdin_tail = 0;
    header.stdout_head = 0;
    header.stdout_tail = 0;
    header.flags &= !(script::FLAG_INTERRUPT | script::FLAG_OUTPUT_READY | script::FLAG_EXIT_READY);
    unsafe {
        core::ptr::copy_nonoverlapping(
            input.as_ptr(),
            (SCRIPT_SESSION_VA as *mut u8).add(script::STDIN_OFFSET),
            input.len(),
        );
        microsystem_user_rt::fence();
    };
    microsystem_user_rt::frame_unmap(region, SCRIPT_SESSION_VA)?;
    microsystem_user_rt::notification_signal(notification, script::EVENT_INPUT)
}

fn wait_ssh_repl_output(region: microsystem_abi::CapHandle) -> Result<u64, Status> {
    let deadline = microsystem_user_rt::clock_now()
        .unwrap_or(0)
        .saturating_add(61_000_000_000);
    loop {
        if let Some(bits) = ssh_repl_event(region)? {
            return Ok(bits);
        }
        let now = microsystem_user_rt::clock_now().unwrap_or(deadline);
        if now >= deadline || !NetdStream::connected() {
            return Err(Status::TimedOut);
        }
        let _ = microsystem_user_rt::yield_now();
    }
}

fn ssh_repl_event(region: microsystem_abi::CapHandle) -> Result<Option<u64>, Status> {
    microsystem_user_rt::frame_map(
        region,
        SCRIPT_SESSION_VA,
        Rights(Rights::READ.0 | Rights::WRITE.0),
    )?;
    microsystem_user_rt::fence();
    let header = unsafe { &*(SCRIPT_SESSION_VA as *const script::SessionHeaderV1) };
    let flags = header.flags;
    microsystem_user_rt::frame_unmap(region, SCRIPT_SESSION_VA)?;
    Ok(if flags & script::FLAG_EXIT_READY != 0 {
        Some(script::EVENT_EXIT)
    } else if flags & script::FLAG_OUTPUT_READY != 0 {
        Some(script::EVENT_OUTPUT)
    } else {
        None
    })
}

async fn write_repl_output(
    channel: &mut zssh::Channel<'_, '_, ServerBehavior>,
    region: microsystem_abi::CapHandle,
) -> Result<(), ()> {
    microsystem_user_rt::frame_map(
        region,
        SCRIPT_SESSION_VA,
        Rights(Rights::READ.0 | Rights::WRITE.0),
    )
    .map_err(|_| ())?;
    let header = unsafe { &mut *(SCRIPT_SESSION_VA as *mut script::SessionHeaderV1) };
    let head = (header.stdout_head as usize).min(script::STDOUT_BYTES);
    let tail = (header.stdout_tail as usize).min(head);
    let output = unsafe {
        core::slice::from_raw_parts(
            (SCRIPT_SESSION_VA as *const u8).add(script::STDOUT_OFFSET + tail),
            head - tail,
        )
    }
    .to_vec();
    header.stdout_tail = head as u32;
    let _ = microsystem_user_rt::frame_unmap(region, SCRIPT_SESSION_VA);
    channel.write_all_stdout(&output).await.map_err(|_| ())
}

fn wait_script_status(pid: u64) -> Option<i64> {
    loop {
        let mut request = Message::new(protocol::PROCESS, process::Operation::Wait as u16);
        request.words[0] = pid;
        let mut reply = Message::new(protocol::PROCESS, 0);
        authorized_call(boot_cap::PROCESS_ENDPOINT, &request, &mut reply, 0).ok()?;
        match reply.words[5] as i64 {
            value if value == Status::Ok as i64 => return Some(reply.words[0] as i64),
            value if value == Status::Busy as i64 => {
                let _ = microsystem_user_rt::yield_now();
            }
            _ => return None,
        }
    }
}

fn cleanup_script_handles(
    region: microsystem_abi::CapHandle,
    notification: microsystem_abi::CapHandle,
) {
    let _ = microsystem_user_rt::cap_delete(notification);
    let _ = microsystem_user_rt::cap_delete(region);
}

async fn run_mica_eval(
    channel: &mut zssh::Channel<'_, '_, ServerBehavior>,
    source: &[u8],
    policy: &[u8],
) -> Result<u32, ()> {
    run_mica_session(
        channel,
        script::MODE_EVAL,
        source,
        policy,
        "",
        &[],
        10_000_000_000,
        false,
    )
    .await
}

async fn run_mica_session(
    channel: &mut zssh::Channel<'_, '_, ServerBehavior>,
    mode: u16,
    source: &[u8],
    policy: &[u8],
    path: &str,
    argv: &[u8],
    timeout_ns: u64,
    gui: bool,
) -> Result<u32, ()> {
    if source.is_empty()
        || source.len() > script::SOURCE_BYTES
        || policy.len().saturating_add(path.len()) > script::POLICY_BYTES
        || argv.len() > script::STDIN_BYTES
        || (mode == script::MODE_FILE && path.is_empty())
        || !matches!(mode, script::MODE_EVAL | script::MODE_FILE)
    {
        channel
            .write_all_stderr(b"mica: invalid source or policy\r\n")
            .await
            .map_err(|_| ())?;
        return Ok(2);
    }
    let prepare = Message::new(protocol::PROCESS, process::Operation::SpawnScript as u16);
    let mut prepared = Message::new(protocol::PROCESS, 0);
    if authorized_call(boot_cap::PROCESS_ENDPOINT, &prepare, &mut prepared, 0).is_err()
        || prepared.words[5] as i64 != Status::Ok as i64
        || prepared.caps[0] == microsystem_abi::CapHandle::INVALID
    {
        channel
            .write_all_stderr(b"mica: session allocation failed\r\n")
            .await
            .map_err(|_| ())?;
        return Ok(1);
    }
    let session = prepared.caps[0];
    if microsystem_user_rt::frame_map(
        session,
        SCRIPT_SESSION_VA,
        Rights(Rights::READ.0 | Rights::WRITE.0),
    )
    .is_err()
    {
        let _ = microsystem_user_rt::cap_delete(session);
        return Ok(1);
    }
    unsafe { core::ptr::write_bytes(SCRIPT_SESSION_VA as *mut u8, 0, script::SESSION_BYTES) };
    let header = unsafe { &mut *(SCRIPT_SESSION_VA as *mut script::SessionHeaderV1) };
    *header = script::SessionHeaderV1 {
        magic: script::SESSION_MAGIC,
        version: script::VERSION,
        mode,
        source_bytes: source.len() as u32,
        policy_bytes: policy.len() as u32,
        argv_bytes: argv.len() as u16,
        path_bytes: path.len() as u16,
        timeout_ns,
        instruction_limit: 10_000_000,
        flags: if gui { script::FLAG_GUI_SESSION } else { 0 },
        ..script::SessionHeaderV1::default()
    };
    unsafe {
        core::ptr::copy_nonoverlapping(
            policy.as_ptr(),
            (SCRIPT_SESSION_VA as *mut u8).add(script::POLICY_OFFSET),
            policy.len(),
        );
        core::ptr::copy_nonoverlapping(
            path.as_ptr(),
            (SCRIPT_SESSION_VA as *mut u8).add(script::POLICY_OFFSET + policy.len()),
            path.len(),
        );
        core::ptr::copy_nonoverlapping(
            source.as_ptr(),
            (SCRIPT_SESSION_VA as *mut u8).add(script::SOURCE_OFFSET),
            source.len(),
        );
        core::ptr::copy_nonoverlapping(
            argv.as_ptr(),
            (SCRIPT_SESSION_VA as *mut u8).add(script::STDIN_OFFSET),
            argv.len(),
        );
    };
    let _ = microsystem_user_rt::frame_unmap(session, SCRIPT_SESSION_VA);
    let mut request = Message::new(protocol::PROCESS, process::Operation::SpawnScript as u16);
    request.words[0] = source.len() as u64;
    request.words[1] = if gui {
        script::FLAG_GUI_SESSION as u64
    } else {
        0
    };
    request.caps[0] = session;
    request.flags |= message_cap_move(0);
    let mut launched = Message::new(protocol::PROCESS, 0);
    if authorized_call(boot_cap::PROCESS_ENDPOINT, &request, &mut launched, 0).is_err()
        || launched.words[5] as i64 != Status::Ok as i64
        || launched.caps[0] == microsystem_abi::CapHandle::INVALID
        || launched.caps[1] == microsystem_abi::CapHandle::INVALID
    {
        return Ok(1);
    }
    let session = launched.caps[0];
    let transferred_completion = launched.caps[1];
    let completion = match microsystem_user_rt::cap_copy(transferred_completion, Rights::READ) {
        Ok(completion) => completion,
        Err(_) => {
            let _ = microsystem_user_rt::cap_delete(transferred_completion);
            let _ = microsystem_user_rt::cap_delete(session);
            return Ok(1);
        }
    };
    let _ = microsystem_user_rt::cap_delete(transferred_completion);
    let pid = launched.words[0];
    let completion_deadline = microsystem_user_rt::clock_now()
        .unwrap_or(0)
        .saturating_add(timeout_ns)
        .saturating_add(1_000_000_000);
    if !wait_for_script_completion(pid, completion, completion_deadline) {
        let _ = kill_script(pid);
    }
    let exit_status = loop {
        let mut wait = Message::new(protocol::PROCESS, process::Operation::Wait as u16);
        wait.words[0] = pid;
        let mut reply = Message::new(protocol::PROCESS, 0);
        if authorized_call(boot_cap::PROCESS_ENDPOINT, &wait, &mut reply, 0).is_err() {
            break 1;
        }
        match reply.words[5] as i64 {
            value if value == Status::Ok as i64 => break reply.words[0] as u32,
            value if value == Status::Busy as i64 => {
                let _ = microsystem_user_rt::yield_now();
            }
            _ => break 1,
        }
    };
    if microsystem_user_rt::frame_map(
        session,
        SCRIPT_SESSION_VA,
        Rights(Rights::READ.0 | Rights::WRITE.0),
    )
    .is_ok()
    {
        let header = unsafe { &*(SCRIPT_SESSION_VA as *const script::SessionHeaderV1) };
        let bytes = (header.stdout_head as usize).min(script::STDOUT_BYTES);
        let output = unsafe {
            core::slice::from_raw_parts(
                (SCRIPT_SESSION_VA as *const u8).add(script::STDOUT_OFFSET),
                bytes,
            )
        };
        channel.write_all_stdout(output).await.map_err(|_| ())?;
        let _ = microsystem_user_rt::frame_unmap(session, SCRIPT_SESSION_VA);
    }
    let _ = microsystem_user_rt::cap_delete(completion);
    let _ = microsystem_user_rt::cap_delete(session);
    Ok(exit_status)
}

fn wait_for_script_completion(
    pid: u64,
    notification: microsystem_abi::CapHandle,
    deadline: u64,
) -> bool {
    loop {
        let now = microsystem_user_rt::clock_now().unwrap_or(deadline);
        if now >= deadline || !NetdStream::connected() {
            return false;
        }
        let slice = now.saturating_add(50_000_000).min(deadline);
        match microsystem_user_rt::notification_wait(notification, slice) {
            Ok(_) => return true,
            Err(Status::TimedOut) => {}
            Err(_) => return false,
        }
        let mut request = Message::new(protocol::PROCESS, process::Operation::Wait as u16);
        request.words[0] = pid;
        let mut reply = Message::new(protocol::PROCESS, 0);
        if authorized_call(boot_cap::PROCESS_ENDPOINT, &request, &mut reply, 0).is_ok()
            && reply.words[5] as i64 == Status::Ok as i64
        {
            return true;
        }
    }
}

fn kill_script(pid: u64) -> Result<(), Status> {
    let mut request = Message::new(protocol::PROCESS, process::Operation::Kill as u16);
    request.words[0] = pid;
    request.words[1] = -15i64 as u64;
    let mut reply = Message::new(protocol::PROCESS, 0);
    authorized_call(boot_cap::PROCESS_ENDPOINT, &request, &mut reply, 0)?;
    if reply.words[5] as i64 == Status::Ok as i64 {
        Ok(())
    } else {
        Err(Status::Io)
    }
}

fn session_valid() -> bool {
    let deadline = SSH_IO_DEADLINE.load(Ordering::Acquire);
    if deadline != 0 && microsystem_user_rt::clock_now().unwrap_or(u64::MAX) >= deadline {
        return false;
    }
    if !SSH_AUTHENTICATED.load(Ordering::Acquire) {
        return true;
    }
    let cookie = SSH_CREDENTIAL.load(Ordering::Acquire);
    unsafe { microsystem_identity::read_shared(microsystem_abi::identity::SNAPSHOT_VA) }.is_ok_and(
        |state| {
            state
                .actor(
                    11,
                    cookie,
                    microsystem_user_rt::clock_now().unwrap_or(u64::MAX),
                )
                .is_ok()
        },
    )
}
fn authorized_call(
    endpoint: microsystem_abi::CapHandle,
    request: &Message,
    reply: &mut Message,
    deadline: u64,
) -> Result<(), Status> {
    let mut request = *request;
    if matches!(request.protocol, protocol::FILESYSTEM | protocol::PROCESS) {
        request.words[3] = SSH_CREDENTIAL.load(Ordering::Acquire);
    }
    microsystem_user_rt::ipc_call(endpoint, &request, reply, deadline)
}
fn host_secret() -> [u8; 32] {
    let deadline = microsystem_user_rt::clock_now()
        .unwrap_or(0)
        .saturating_add(30_000_000_000);
    loop {
        match filesystem_request(filesystem::Operation::ReadRange, "/.system/ssh/host.key", 0) {
            Ok(reply) if reply.words[0] == 32 => {
                let mut key = [0; 32];
                unsafe {
                    core::ptr::copy_nonoverlapping(
                        FILESYSTEM_SHARED_VA as *const u8,
                        key.as_mut_ptr(),
                        32,
                    );
                }
                return key;
            }
            Err(Status::NotFound | Status::Busy)
                if microsystem_user_rt::clock_now().unwrap_or(deadline) < deadline =>
            {
                let _ = microsystem_user_rt::yield_now();
            }
            _ => {
                let _ = microsystem_user_rt::debug_write(
                    b"[ssh] persisted host key unavailable; fail-closed=true\n",
                );
                microsystem_user_rt::exit(4)
            }
        }
    }
}

struct ServerBehavior {
    stream: NetdStream,
    random: KernelRandom,
    host: SecretKey,
}

impl ServerBehavior {
    fn new(stream: NetdStream) -> Self {
        Self {
            stream,
            random: KernelRandom,
            host: SecretKey::Ed25519 {
                secret_key: SigningKey::from_bytes(&host_secret()),
            },
        }
    }
}

impl Behavior for ServerBehavior {
    type Stream = NetdStream;
    fn stream(&mut self) -> &mut Self::Stream {
        &mut self.stream
    }
    type Random = KernelRandom;
    fn random(&mut self) -> &mut Self::Random {
        &mut self.random
    }
    fn host_secret_key(&self) -> &SecretKey {
        &self.host
    }
    type User = ();
    fn allow_user(&mut self, username: &str, method: &AuthMethod) -> Option<()> {
        let AuthMethod::PublicKey(PublicKey::Ed25519 { public_key }) = method else {
            return None;
        };
        if !microsystem_identity::valid_name(username) {
            return None;
        }
        unsafe {
            core::ptr::copy_nonoverlapping(
                public_key.as_bytes().as_ptr(),
                FILESYSTEM_SHARED_VA as *mut u8,
                32,
            );
        }
        let mut request = Message::new(
            protocol::IDENTITY,
            microsystem_abi::identity::Operation::AuthorizeSsh as u16,
        );
        let mut name = [0; 32];
        name[..username.len()].copy_from_slice(username.as_bytes());
        for i in 0..4 {
            request.words[i] = u64::from_le_bytes(name[i * 8..i * 8 + 8].try_into().unwrap());
        }
        request.caps[0] = boot_cap::SSH_FILESYSTEM_FRAME;
        let mut reply = Message::new(protocol::IDENTITY, 0);
        let deadline = microsystem_user_rt::clock_now()
            .ok()?
            .saturating_add(30_000_000_000);
        microsystem_user_rt::ipc_call(boot_cap::IDENTITY_ENDPOINT, &request, &mut reply, deadline)
            .ok()?;
        if reply.words[5] != 0 || reply.words[0] == 0 {
            return None;
        }
        SSH_CREDENTIAL.store(reply.words[0], Ordering::Release);
        SSH_AUTHENTICATED.store(true, Ordering::Release);
        Some(())
    }
    fn allow_shell(&self) -> bool {
        true
    }
    type Command = SshCommand;
    fn parse_command(&mut self, command: &str) -> Self::Command {
        parse_command(command)
    }
    fn server_id(&self) -> &'static str {
        "SSH-2.0-MicroSystem_0.1"
    }
}

struct KernelRandom;
impl RngCore for KernelRandom {
    fn next_u32(&mut self) -> u32 {
        let mut b = [0; 4];
        self.fill_bytes(&mut b);
        u32::from_le_bytes(b)
    }
    fn next_u64(&mut self) -> u64 {
        let mut b = [0; 8];
        self.fill_bytes(&mut b);
        u64::from_le_bytes(b)
    }
    fn fill_bytes(&mut self, dest: &mut [u8]) {
        if !SSH_FIRST_RANDOM.swap(true, Ordering::AcqRel) {
            let _ = microsystem_user_rt::debug_write(b"[ssh] transport random ready=true\n");
        }
        for chunk in dest.chunks_mut(256) {
            microsystem_user_rt::random_fill(boot_cap::RANDOM_SOURCE, chunk).unwrap();
        }
    }
    fn try_fill_bytes(&mut self, dest: &mut [u8]) -> Result<(), rand::Error> {
        self.fill_bytes(dest);
        Ok(())
    }
}
impl CryptoRng for KernelRandom {}

#[derive(Clone, Copy)]
struct NetdStream {
    connection: u64,
}

impl NetdStream {
    fn initialize() -> Result<microsystem_abi::CapHandle, Status> {
        let request = Message::new(protocol::NETWORK, network::Operation::SshSharedFrame as u16);
        let mut reply = Message::new(protocol::NETWORK, 0);
        authorized_call(boot_cap::SSH_NETWORK_ENDPOINT, &request, &mut reply, 0)?;
        if reply.words[5] as i64 != Status::Ok as i64
            || reply.caps[0] == microsystem_abi::CapHandle::INVALID
        {
            return Err(Status::Io);
        }
        microsystem_user_rt::frame_map(
            reply.caps[0],
            NETWORK_SHARED_VA,
            Rights(Rights::READ.0 | Rights::WRITE.0),
        )?;
        for connection in 1..=network::MAX_INTERFACES as u64 * 2 {
            Self { connection }.close(false);
        }
        Ok(reply.caps[0])
    }

    fn wait_accept() -> Self {
        loop {
            let request = Message::new(protocol::NETWORK, network::Operation::SshAccept as u16);
            let mut reply = Message::new(protocol::NETWORK, 0);
            match authorized_call(boot_cap::SSH_NETWORK_ENDPOINT, &request, &mut reply, 0) {
                Ok(()) if reply.words[5] as i64 == Status::Ok as i64 && reply.words[0] != 0 => {
                    SSH_CONNECTION.store(reply.words[0], Ordering::Release);
                    return Self {
                        connection: reply.words[0],
                    };
                }
                Ok(()) if reply.words[5] as i64 == Status::Busy as i64 => {
                    let _ = microsystem_user_rt::yield_now();
                }
                _ => {
                    let _ = microsystem_user_rt::yield_now();
                }
            }
        }
    }

    fn close(self, graceful: bool) {
        let deadline = microsystem_user_rt::clock_now()
            .unwrap_or(0)
            .saturating_add(1_000_000_000);
        loop {
            let mut request = Message::new(protocol::NETWORK, network::Operation::SshClose as u16);
            request.words[0] = self.connection;
            request.words[1] = graceful as u64;
            let mut reply = Message::new(protocol::NETWORK, 0);
            match authorized_call(boot_cap::SSH_NETWORK_ENDPOINT, &request, &mut reply, 0) {
                Ok(()) if reply.words[5] as i64 == Status::Ok as i64 => return,
                Ok(())
                    if reply.words[5] as i64 == Status::Busy as i64
                        && microsystem_user_rt::clock_now().unwrap_or(deadline) < deadline =>
                {
                    let _ = microsystem_user_rt::yield_now();
                }
                _ if graceful => return self.close(false),
                _ => return,
            }
        }
    }

    fn connected() -> bool {
        if !session_valid() {
            return false;
        }
        let connection = SSH_CONNECTION.load(Ordering::Acquire);
        if connection == 0 {
            return false;
        }
        let mut request = Message::new(protocol::NETWORK, network::Operation::SshStatus as u16);
        request.words[0] = connection;
        let mut reply = Message::new(protocol::NETWORK, 0);
        authorized_call(boot_cap::SSH_NETWORK_ENDPOINT, &request, &mut reply, 0).is_ok()
            && reply.words[5] as i64 == Status::Ok as i64
            && reply.words[0] != 0
    }
}

#[derive(Debug)]
struct StreamError;
impl embedded_io_async::Error for StreamError {
    fn kind(&self) -> ErrorKind {
        ErrorKind::Other
    }
}
impl ErrorType for NetdStream {
    type Error = StreamError;
}
impl Read for NetdStream {
    async fn read(&mut self, output: &mut [u8]) -> Result<usize, Self::Error> {
        loop {
            if !session_valid() {
                return Err(StreamError);
            }
            let mut request =
                Message::new(protocol::NETWORK, network::Operation::SshReceive as u16);
            request.words[0] = self.connection;
            request.words[1] = output.len().min(4096) as u64;
            let mut reply = Message::new(protocol::NETWORK, 0);
            match authorized_call(boot_cap::SSH_NETWORK_ENDPOINT, &request, &mut reply, 0) {
                Ok(()) if reply.words[5] as i64 == Status::Ok as i64 => {
                    let bytes = reply.words[0] as usize;
                    if bytes > output.len().min(4096) {
                        return Err(StreamError);
                    }
                    unsafe {
                        microsystem_user_rt::fence();
                        core::ptr::copy_nonoverlapping(
                            NETWORK_SHARED_VA as *const u8,
                            output.as_mut_ptr(),
                            bytes,
                        );
                    }
                    if !SSH_FIRST_READ.swap(true, Ordering::AcqRel) {
                        let _ =
                            microsystem_user_rt::debug_write(b"[ssh] transport first-read=true\n");
                    }
                    if bytes != 0 && SSH_CHANNEL_ACTIVE.load(Ordering::Acquire) {
                        SSH_IO_DEADLINE.store(
                            microsystem_user_rt::clock_now()
                                .unwrap_or(0)
                                .saturating_add(300_000_000_000),
                            Ordering::Release,
                        );
                    }
                    return Ok(bytes);
                }
                Ok(()) if reply.words[5] as i64 == Status::Busy as i64 => {}
                _ => return Err(StreamError),
            }
            let _ = microsystem_user_rt::yield_now();
        }
    }
}
impl Write for NetdStream {
    async fn write(&mut self, input: &[u8]) -> Result<usize, Self::Error> {
        if !SSH_FIRST_WRITE.swap(true, Ordering::AcqRel) {
            let _ = microsystem_user_rt::debug_write(b"[ssh] transport first-write=true\n");
        }
        let bytes = input.len().min(4096);
        unsafe {
            core::ptr::copy_nonoverlapping(input.as_ptr(), NETWORK_SHARED_VA as *mut u8, bytes);
            microsystem_user_rt::fence();
        }
        loop {
            if !session_valid() {
                return Err(StreamError);
            }
            let mut request = Message::new(protocol::NETWORK, network::Operation::SshSend as u16);
            request.words[0] = self.connection;
            request.words[1] = bytes as u64;
            let mut reply = Message::new(protocol::NETWORK, 0);
            match authorized_call(boot_cap::SSH_NETWORK_ENDPOINT, &request, &mut reply, 0) {
                Ok(()) if reply.words[5] as i64 == Status::Ok as i64 => {
                    return Ok(reply.words[0] as usize);
                }
                Ok(()) if reply.words[5] as i64 == Status::Busy as i64 => {}
                _ => return Err(StreamError),
            }
            let _ = microsystem_user_rt::yield_now();
        }
    }
    async fn flush(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }
}
fn copy(output: &mut [u8], offset: usize, bytes: &[u8]) -> usize {
    output[offset..offset + bytes.len()].copy_from_slice(bytes);
    offset + bytes.len()
}
fn decimal(output: &mut [u8], offset: usize, mut value: u64) -> usize {
    let mut digits = [0u8; 20];
    let mut cursor = digits.len();
    loop {
        cursor -= 1;
        digits[cursor] = b'0' + (value % 10) as u8;
        value /= 10;
        if value == 0 {
            break;
        }
    }
    copy(output, offset, &digits[cursor..])
}

fn block_on<F: Future>(mut future: F) -> F::Output {
    let waker = unsafe { Waker::from_raw(raw_waker()) };
    let mut context = Context::from_waker(&waker);
    let mut future = unsafe { Pin::new_unchecked(&mut future) };
    loop {
        if let Poll::Ready(value) = future.as_mut().poll(&mut context) {
            return value;
        }
        let _ = microsystem_user_rt::yield_now();
    }
}
const fn raw_waker() -> RawWaker {
    const VTABLE: RawWakerVTable = RawWakerVTable::new(|_| raw_waker(), |_| {}, |_| {}, |_| {});
    RawWaker::new(core::ptr::null(), &VTABLE)
}

#[panic_handler]
fn panic(_: &PanicInfo<'_>) -> ! {
    microsystem_user_rt::exit(1)
}
