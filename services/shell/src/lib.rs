#![no_std]

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Command<'a> {
    Empty,
    Help,
    Pwd,
    Cd(&'a str),
    Echo(&'a str),
    Clear,
    History,
    Ps,
    Service(&'a str),
    App(&'a str),
    User(&'a str),
    Uptime,
    Date,
    SystemInfo,
    Sleep(&'a str),
    Kill {
        pid: &'a str,
        status: Option<&'a str>,
    },
    Wait(&'a str),
    Ls(&'a str),
    Cat(&'a str),
    Head {
        path: &'a str,
        lines: usize,
    },
    Tail {
        path: &'a str,
        lines: usize,
    },
    Wc(&'a str),
    Hexdump(&'a str),
    Grep {
        pattern: &'a str,
        path: &'a str,
    },
    Find(&'a str),
    Tree(&'a str),
    Du(&'a str),
    Df(&'a str),
    Stat(&'a str),
    Mounts,
    Mount {
        image: &'a str,
        point: &'a str,
        readonly: bool,
    },
    Unmount(&'a str),
    VolumeCreate {
        image: &'a str,
        mib: u32,
    },
    Chmod {
        mode: u32,
        path: &'a str,
    },
    Chown {
        uid: u32,
        gid: u32,
        path: &'a str,
    },
    Touch(&'a str),
    Copy {
        source: &'a str,
        destination: &'a str,
        recursive: bool,
    },
    Write {
        path: &'a str,
        value: &'a str,
    },
    Append {
        path: &'a str,
        value: &'a str,
    },
    Mkdir {
        path: &'a str,
        parents: bool,
    },
    Rmdir(&'a str),
    Rename {
        source: &'a str,
        destination: &'a str,
    },
    Unlink {
        path: &'a str,
        recursive: bool,
    },
    Fsync(&'a str),
    FsHelp,
    Sql(&'a str),
    Curl(&'a str),
    Nslookup(&'a str),
    Netstat,
    Network(&'a str),
    Run(&'a str),
    MicaEval(&'a str),
    MicaFile(&'a str),
    MicaRepl,
    MicaArgs(&'a str),
    Sync,
    Exit,
    Shutdown,
    Reboot,
    Poweroff,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ParseError {
    Unknown,
    MissingArgument,
    Invalid,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AppCommand<'a> {
    List,
    Info(&'a str),
    Run(&'a str),
    Exec(&'a str),
    Rollback(&'a str),
    Install {
        name: &'a str,
        version: &'a str,
        source: &'a str,
        pages: u32,
        permissions: u64,
    },
}

pub fn app_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 48
        && value != "."
        && value != ".."
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
}

pub fn parse_app(input: &str) -> Result<AppCommand<'_>, ParseError> {
    let (operation, rest) = required_argument(if input.trim().is_empty() {
        "list"
    } else {
        input
    })?;
    match operation {
        "list" if rest.is_empty() => Ok(AppCommand::List),
        "info" | "run" | "rollback" => {
            let name = one_argument(rest)?;
            if !app_identifier(name) {
                return Err(ParseError::Invalid);
            }
            Ok(match operation {
                "info" => AppCommand::Info(name),
                "run" => AppCommand::Run(name),
                _ => AppCommand::Rollback(name),
            })
        }
        "exec" => {
            let path = one_argument(rest)?;
            if !path.starts_with('/') || path.len() > 255 {
                return Err(ParseError::Invalid);
            }
            Ok(AppCommand::Exec(path))
        }
        "install" | "update" => {
            let (name, rest) = required_argument(rest)?;
            let (version, rest) = required_argument(rest)?;
            let (source, rest) = required_argument(rest)?;
            if !app_identifier(name)
                || !app_identifier(version)
                || !source.starts_with('/')
                || source.len() > 255
            {
                return Err(ParseError::Invalid);
            }
            let (pages, rest) = if rest.is_empty() {
                (microsystem_abi::virtual_memory::PAGE_BUDGET, "")
            } else {
                let (pages, rest) = required_argument(rest)?;
                (pages.parse::<u32>().map_err(|_| ParseError::Invalid)?, rest)
            };
            if pages == 0 || pages > microsystem_abi::virtual_memory::PAGE_BUDGET {
                return Err(ParseError::Invalid);
            }
            let permissions = match if rest.is_empty() {
                "none"
            } else {
                one_argument(rest)?
            } {
                "none" => 0,
                "random" => microsystem_abi::application::RANDOM,
                "stats" => microsystem_abi::application::SYSTEM_INFO,
                "all" => {
                    microsystem_abi::application::RANDOM | microsystem_abi::application::SYSTEM_INFO
                }
                _ => return Err(ParseError::Invalid),
            };
            Ok(AppCommand::Install {
                name,
                version,
                source,
                pages,
                permissions,
            })
        }
        _ => Err(ParseError::Invalid),
    }
}

#[cfg(feature = "user-bin")]
pub fn network_command(
    arguments: &str,
    frame: microsystem_abi::CapHandle,
    address: usize,
    output: &mut impl core::fmt::Write,
) -> Result<(), microsystem_abi::Status> {
    use microsystem_abi::{Message, Status, boot_cap, network, protocol};
    if arguments.len() > 512 {
        return Err(Status::Invalid);
    }
    unsafe {
        core::ptr::copy_nonoverlapping(arguments.as_ptr(), address as *mut u8, arguments.len());
    }
    microsystem_user_rt::fence();
    let mut request = Message::new(protocol::NETWORK, network::Operation::Configure as u16);
    request.words[0] = arguments.len() as u64;
    request.caps[0] = frame;
    let mut reply = Message::new(protocol::NETWORK, 0);
    let deadline = microsystem_user_rt::clock_now()?.saturating_add(15_000_000_000);
    microsystem_user_rt::ipc_call(boot_cap::NETWORK_ENDPOINT, &request, &mut reply, deadline)?;
    if reply.words[5] as i64 != 0 {
        return Err(match reply.words[5] as i64 {
            -3 => Status::AccessDenied,
            -1 => Status::Invalid,
            _ => Status::Io,
        });
    }
    let length = reply.words[0] as usize;
    if length > 4096 {
        return Err(Status::Corrupt);
    }
    let bytes = unsafe { core::slice::from_raw_parts(address as *const u8, length) };
    let text = core::str::from_utf8(bytes).map_err(|_| Status::Corrupt)?;
    output.write_str(text).map_err(|_| Status::Io)
}

#[cfg(feature = "user-bin")]
pub fn identity_command(
    arguments: &str,
    frame: microsystem_abi::CapHandle,
    address: usize,
    output: &mut impl core::fmt::Write,
) -> Result<(), microsystem_abi::Status> {
    use microsystem_abi::{Message, Status, boot_cap, identity, protocol};
    if arguments.len() > 512 {
        return Err(Status::Invalid);
    }
    unsafe {
        core::ptr::copy_nonoverlapping(arguments.as_ptr(), address as *mut u8, arguments.len());
    }
    let mut request = Message::new(protocol::IDENTITY, identity::Operation::Command as u16);
    request.words[0] = arguments.len() as u64;
    request.caps[0] = frame;
    let mut reply = Message::new(protocol::IDENTITY, 0);
    let deadline = microsystem_user_rt::clock_now()?.saturating_add(120_000_000_000);
    microsystem_user_rt::ipc_call(boot_cap::IDENTITY_ENDPOINT, &request, &mut reply, deadline)?;
    if reply.words[0] > 4096 {
        return Err(Status::Corrupt);
    }
    if reply.words[0] == 0 && reply.words[5] != 0 {
        return Err(Status::AccessDenied);
    }
    let bytes =
        unsafe { core::slice::from_raw_parts(address as *const u8, reply.words[0] as usize) };
    output
        .write_str(core::str::from_utf8(bytes).map_err(|_| Status::Corrupt)?)
        .map_err(|_| Status::Io)
}

#[cfg(feature = "user-bin")]
pub fn app_command(
    arguments: &str,
    frame: microsystem_abi::CapHandle,
    address: usize,
    output: &mut impl core::fmt::Write,
) -> Result<(), microsystem_abi::Status> {
    use microsystem_abi::{Message, Status, application, boot_cap, protocol};
    parse_app(arguments).map_err(|_| Status::Invalid)?;
    if arguments.len() > 512 {
        return Err(Status::Invalid);
    }
    unsafe {
        core::ptr::copy_nonoverlapping(arguments.as_ptr(), address as *mut u8, arguments.len());
    }
    let mut request = Message::new(
        protocol::APPLICATION,
        application::Operation::Command as u16,
    );
    request.words[0] = arguments.len() as u64;
    request.caps[0] = frame;
    let mut reply = Message::new(0, 0);
    let deadline = microsystem_user_rt::clock_now()?.saturating_add(120_000_000_000);
    microsystem_user_rt::ipc_call(
        boot_cap::APPLICATION_ENDPOINT,
        &request,
        &mut reply,
        deadline,
    )?;
    if reply.protocol != protocol::APPLICATION || reply.words[0] > 4096 {
        return Err(Status::Corrupt);
    }
    if reply.words[5] != 0 && reply.words[0] == 0 {
        let _ = writeln!(output, "app: failed status={}", reply.words[5] as i64);
        return Ok(());
    }
    let bytes =
        unsafe { core::slice::from_raw_parts(address as *const u8, reply.words[0] as usize) };
    let text = core::str::from_utf8(bytes).map_err(|_| Status::Corrupt)?;
    let _ = output.write_str(text);
    Ok(())
}

pub fn parse_service(
    arguments: &str,
) -> Result<(microsystem_abi::service::Operation, Option<usize>), ParseError> {
    use microsystem_abi::service::{NAMES, Operation};
    let mut words = arguments.split_ascii_whitespace();
    let operation = match words.next().unwrap_or("list") {
        "list" | "status" => Operation::List,
        "restart" => Operation::Restart,
        "stop" => Operation::Stop,
        _ => return Err(ParseError::Invalid),
    };
    let task = words
        .next()
        .map(|name| {
            NAMES
                .iter()
                .position(|candidate| *candidate == name)
                .ok_or(ParseError::Invalid)
        })
        .transpose()?;
    if words.next().is_some() || (operation != Operation::List && task.is_none()) {
        return Err(ParseError::Invalid);
    }
    Ok((operation, task))
}

#[cfg(feature = "user-bin")]
pub fn service_command(
    arguments: &str,
    output: &mut impl core::fmt::Write,
) -> Result<(), microsystem_abi::Status> {
    use microsystem_abi::{Message, Status, boot_cap, protocol, service};
    let (operation, selected) = parse_service(arguments).map_err(|_| Status::Invalid)?;
    for task in 0..service::NAMES.len() {
        if selected.is_some_and(|selected| selected != task) {
            continue;
        }
        let mut request = Message::new(protocol::SERVICE, operation as u16);
        request.words[0] = (task + 1) as u64;
        let mut reply = Message::new(0, 0);
        let deadline = microsystem_user_rt::clock_now()?.saturating_add(5_000_000_000);
        microsystem_user_rt::ipc_call(boot_cap::SERVICE_ENDPOINT, &request, &mut reply, deadline)?;
        if reply.protocol != protocol::SERVICE || reply.opcode != operation as u16 {
            return Err(Status::Corrupt);
        }
        if reply.words[5] != 0 {
            let _ = writeln!(
                output,
                "service: {} failed status={}",
                service::NAMES[task],
                reply.words[5] as i64
            );
        } else if operation == service::Operation::List {
            let state = match reply.words[1] as u16 {
                service::STOPPED => "stopped",
                service::STARTING => "starting",
                service::ONLINE => "online",
                service::STOPPING => "stopping",
                service::QUIESCE_FAILED => "quiesce-failed",
                _ => "unknown",
            };
            let _ = writeln!(
                output,
                "{} {} starts={} exit={} held={}",
                service::NAMES[task],
                state,
                reply.words[2],
                reply.words[3] as i64,
                reply.words[4]
            );
        } else {
            let _ = writeln!(output, "service: {} accepted", service::NAMES[task]);
        }
    }
    Ok(())
}

/// The FNV-1a identifier used by the boot archive's name-based ELF loader.
pub fn program_id(name: &str) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in name.as_bytes() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x100_0000_01b3);
    }
    hash
}

pub fn program_name(program: u64) -> Option<&'static str> {
    [
        "counter",
        "spinner",
        "privprobe",
        "resourceprobe",
        "resourcefault",
        "resourcekill",
        "badptr",
        "crossptr",
        "pageprobe",
        "mica",
    ]
    .into_iter()
    .find(|name| program_id(name) == program)
}

pub fn parse(line: &str) -> Result<Command<'_>, ParseError> {
    let line = line.trim();
    if line.is_empty() {
        return Ok(Command::Empty);
    }
    let (name, rest) = line
        .split_once(|ch: char| ch.is_ascii_whitespace())
        .map(|(a, b)| (a, b.trim()))
        .unwrap_or((line, ""));
    if name == "fs" {
        if rest.is_empty() {
            return Err(ParseError::MissingArgument);
        }
        let (subcommand, arguments) = rest
            .split_once(|ch: char| ch.is_ascii_whitespace())
            .map(|(a, b)| (a, b.trim()))
            .unwrap_or((rest, ""));
        return parse_filesystem_command(subcommand, arguments);
    }
    match name {
        "help" if rest.is_empty() => Ok(Command::Help),
        "pwd" if rest.is_empty() => Ok(Command::Pwd),
        "cd" => Ok(Command::Cd(if rest.is_empty() {
            "/"
        } else {
            one_argument(rest)?
        })),
        "echo" => Ok(Command::Echo(rest)),
        "clear" if rest.is_empty() => Ok(Command::Clear),
        "history" if rest.is_empty() => Ok(Command::History),
        "ps" if rest.is_empty() => Ok(Command::Ps),
        "service" => {
            parse_service(rest)?;
            Ok(Command::Service(rest))
        }
        "app" => {
            parse_app(rest)?;
            Ok(Command::App(rest))
        }
        "uptime" if rest.is_empty() => Ok(Command::Uptime),
        "date" if rest.is_empty() => Ok(Command::Date),
        "free" | "sysinfo" if rest.is_empty() => Ok(Command::SystemInfo),
        "sleep" => Ok(Command::Sleep(one_argument(rest)?)),
        "kill" => {
            let (first, second) = one_or_two_arguments(rest)?;
            let (pid, status) = if first.starts_with('-') {
                (second.ok_or(ParseError::MissingArgument)?, Some(first))
            } else {
                (first, second)
            };
            Ok(Command::Kill { pid, status })
        }
        "wait" => Ok(Command::Wait(one_argument(rest)?)),
        "ls" | "list" => parse_filesystem_command("ls", rest),
        "cat" | "read" => parse_filesystem_command("cat", rest),
        "head" => parse_head_tail(rest, true),
        "tail" => parse_head_tail(rest, false),
        "wc" => Ok(Command::Wc(one_argument(rest)?)),
        "hexdump" | "xxd" => Ok(Command::Hexdump(one_argument(rest)?)),
        "grep" => {
            let (pattern, path) = two_arguments(rest)?;
            Ok(Command::Grep { pattern, path })
        }
        "find" => Ok(Command::Find(optional_path(rest)?)),
        "tree" => Ok(Command::Tree(optional_path(rest)?)),
        "du" => Ok(Command::Du(optional_path(rest)?)),
        "df" => Ok(Command::Df(optional_path(rest)?)),
        "stat" => parse_filesystem_command("stat", rest),
        "mount" | "umount" | "mkvol" => parse_filesystem_command(name, rest),
        "volume" => {
            let (command, arguments) = rest
                .split_once(|ch: char| ch.is_ascii_whitespace())
                .unwrap_or((rest, ""));
            match command {
                "" | "list" if arguments.trim().is_empty() => Ok(Command::Mounts),
                "create" => parse_filesystem_command("mkvol", arguments.trim()),
                "mount" => parse_filesystem_command("mount", arguments.trim()),
                "unmount" => parse_filesystem_command("umount", arguments.trim()),
                _ => Err(ParseError::Invalid),
            }
        }
        "chmod" => parse_filesystem_command("chmod", rest),
        "chown" => parse_filesystem_command("chown", rest),
        "touch" | "create" => parse_filesystem_command("touch", rest),
        "cp" => parse_filesystem_command("cp", rest),
        "write" => parse_filesystem_command("write", rest),
        "append" => parse_filesystem_command("append", rest),
        "mkdir" => parse_filesystem_command("mkdir", rest),
        "rmdir" => parse_filesystem_command("rmdir", rest),
        "mv" | "rename" => parse_filesystem_command("mv", rest),
        "rm" | "remove" | "unlink" => parse_filesystem_command("rm", rest),
        "fsync" => parse_filesystem_command("fsync", rest),
        "sync" if rest.is_empty() => Ok(Command::Sync),
        "sql" if !rest.is_empty() => Ok(Command::Sql(rest)),
        "sql" => Err(ParseError::MissingArgument),
        "curl" if !rest.is_empty() => Ok(Command::Curl(rest)),
        "nslookup" => Ok(Command::Nslookup(one_argument(rest)?)),
        "netstat" if rest.is_empty() => Ok(Command::Netstat),
        "net" => Ok(Command::Network(rest)),
        "user" => Ok(Command::User(rest)),
        "whoami" if rest.is_empty() => Ok(Command::User("whoami")),
        "run" if !rest.is_empty() => Ok(Command::Run(rest)),
        "mica" if rest.is_empty() => Ok(Command::MicaRepl),
        "mica" if rest.starts_with("--") => Ok(Command::MicaArgs(rest)),
        "mica" if let Some(source) = rest.strip_prefix("-e ") => {
            let source = source.trim();
            let source = source
                .strip_prefix('\'')
                .and_then(|source| source.strip_suffix('\''))
                .or_else(|| {
                    source
                        .strip_prefix('"')
                        .and_then(|source| source.strip_suffix('"'))
                })
                .unwrap_or(source);
            if source.is_empty() {
                Err(ParseError::MissingArgument)
            } else {
                Ok(Command::MicaEval(source))
            }
        }
        "mica" if rest.contains(' ') => Ok(Command::MicaArgs(rest)),
        "mica" => Ok(Command::MicaFile(rest)),
        "exit" if rest.is_empty() => Ok(Command::Exit),
        "shutdown" if rest.is_empty() => Ok(Command::Shutdown),
        "reboot" if rest.is_empty() => Ok(Command::Reboot),
        "poweroff" if rest.is_empty() => Ok(Command::Poweroff),
        "run" => Err(ParseError::MissingArgument),
        "curl" => Err(ParseError::MissingArgument),
        _ => Err(ParseError::Unknown),
    }
}

fn parse_filesystem_command<'a>(name: &str, rest: &'a str) -> Result<Command<'a>, ParseError> {
    match name {
        "help" if rest.is_empty() => Ok(Command::FsHelp),
        "ls" | "list" => Ok(Command::Ls(optional_path(rest)?)),
        "cat" | "read" => Ok(Command::Cat(one_argument(rest)?)),
        "head" => parse_head_tail(rest, true),
        "tail" => parse_head_tail(rest, false),
        "wc" => Ok(Command::Wc(one_argument(rest)?)),
        "hexdump" | "xxd" => Ok(Command::Hexdump(one_argument(rest)?)),
        "grep" => {
            let (pattern, path) = two_arguments(rest)?;
            Ok(Command::Grep { pattern, path })
        }
        "find" => Ok(Command::Find(optional_path(rest)?)),
        "tree" => Ok(Command::Tree(optional_path(rest)?)),
        "du" => Ok(Command::Du(optional_path(rest)?)),
        "df" => Ok(Command::Df(optional_path(rest)?)),
        "stat" => Ok(Command::Stat(one_argument(rest)?)),
        "mount" if rest.is_empty() => Ok(Command::Mounts),
        "mount" => {
            let (image, rest) = required_argument(rest)?;
            let (point, rest) = required_argument(rest)?;
            let readonly = match rest {
                "" | "rw" => false,
                "ro" => true,
                _ => return Err(ParseError::Invalid),
            };
            Ok(Command::Mount {
                image,
                point,
                readonly,
            })
        }
        "umount" => Ok(Command::Unmount(one_argument(rest)?)),
        "mkvol" => {
            let (image, mib) = two_arguments(rest)?;
            let mib = mib.parse::<u32>().map_err(|_| ParseError::Invalid)?;
            if !(3..=8).contains(&mib) {
                return Err(ParseError::Invalid);
            }
            Ok(Command::VolumeCreate { image, mib })
        }
        "chmod" => {
            let (mode, path) = two_arguments(rest)?;
            let mode = u32::from_str_radix(mode.strip_prefix("0o").unwrap_or(mode), 8)
                .map_err(|_| ParseError::Invalid)?;
            if mode > 0o777 {
                return Err(ParseError::Invalid);
            }
            Ok(Command::Chmod { mode, path })
        }
        "chown" => {
            let (owner, path) = two_arguments(rest)?;
            let (uid, gid) = owner.split_once(':').ok_or(ParseError::Invalid)?;
            let uid = uid.parse().map_err(|_| ParseError::Invalid)?;
            let gid = gid.parse().map_err(|_| ParseError::Invalid)?;
            Ok(Command::Chown { uid, gid, path })
        }
        "touch" | "create" => Ok(Command::Touch(one_argument(rest)?)),
        "cp" => {
            let (recursive, arguments) = strip_recursive_flag(rest);
            let (source, destination) = two_arguments(arguments)?;
            Ok(Command::Copy {
                source,
                destination,
                recursive,
            })
        }
        "write" | "append" => {
            let (path, rest) = required_argument(rest)?;
            if path.is_empty() || rest.is_empty() {
                return Err(ParseError::MissingArgument);
            }
            let value = if rest.starts_with(['\'', '"']) {
                one_argument(rest)?
            } else {
                rest
            };
            Ok(if name == "write" {
                Command::Write { path, value }
            } else {
                Command::Append { path, value }
            })
        }
        "mkdir" => {
            let (parents, path) = strip_flag(rest, "-p");
            Ok(Command::Mkdir {
                path: one_argument(path)?,
                parents,
            })
        }
        "rmdir" => Ok(Command::Rmdir(one_argument(rest)?)),
        "mv" | "rename" => {
            let (source, destination) = two_arguments(rest)?;
            Ok(Command::Rename {
                source,
                destination,
            })
        }
        "rm" | "remove" | "unlink" => {
            let (recursive, path) = strip_recursive_flag(rest);
            Ok(Command::Unlink {
                path: one_argument(path)?,
                recursive,
            })
        }
        "fsync" => Ok(Command::Fsync(one_argument(rest)?)),
        "sync" if rest.is_empty() => Ok(Command::Sync),
        "sync" => Err(ParseError::MissingArgument),
        _ => Err(ParseError::Unknown),
    }
}

fn parse_head_tail(rest: &str, head: bool) -> Result<Command<'_>, ParseError> {
    let (first, rest) = required_argument(rest)?;
    let (lines, path) = if first == "-n" {
        let (lines, rest) = required_argument(rest)?;
        let lines = lines.parse::<usize>().map_err(|_| ParseError::Invalid)?;
        let path = one_argument(rest)?;
        (lines, path)
    } else {
        if !rest.is_empty() {
            return Err(ParseError::Invalid);
        }
        (10, first)
    };
    if lines == 0 {
        return Err(ParseError::Invalid);
    }
    Ok(if head {
        Command::Head { path, lines }
    } else {
        Command::Tail { path, lines }
    })
}

fn optional_path(rest: &str) -> Result<&str, ParseError> {
    if rest.is_empty() {
        Ok(".")
    } else {
        one_argument(rest)
    }
}

fn strip_flag<'a>(rest: &'a str, flag: &str) -> (bool, &'a str) {
    rest.strip_prefix(flag)
        .filter(|remaining| {
            remaining.is_empty()
                || remaining
                    .as_bytes()
                    .first()
                    .is_some_and(u8::is_ascii_whitespace)
        })
        .map(|remaining| (true, remaining.trim_start()))
        .unwrap_or((false, rest))
}

fn strip_recursive_flag(rest: &str) -> (bool, &str) {
    let (short, remaining) = strip_flag(rest, "-r");
    if short {
        return (true, remaining);
    }
    strip_flag(rest, "-R")
}

fn one_argument(rest: &str) -> Result<&str, ParseError> {
    let (argument, rest) = required_argument(rest)?;
    if !rest.is_empty() {
        Err(ParseError::Invalid)
    } else {
        Ok(argument)
    }
}

fn two_arguments(rest: &str) -> Result<(&str, &str), ParseError> {
    let (first, rest) = required_argument(rest)?;
    Ok((first, one_argument(rest)?))
}

fn one_or_two_arguments(rest: &str) -> Result<(&str, Option<&str>), ParseError> {
    let (first, rest) = required_argument(rest)?;
    Ok((
        first,
        if rest.is_empty() {
            None
        } else {
            Some(one_argument(rest)?)
        },
    ))
}

fn required_argument(input: &str) -> Result<(&str, &str), ParseError> {
    let input = input.trim_start();
    let Some(first) = input.as_bytes().first().copied() else {
        return Err(ParseError::MissingArgument);
    };
    if input.as_bytes().contains(&0) {
        return Err(ParseError::Invalid);
    }
    if first == b'\'' || first == b'"' {
        let end = input[1..]
            .find(first as char)
            .map(|end| end + 1)
            .ok_or(ParseError::Invalid)?;
        let rest = &input[end + 1..];
        if rest
            .as_bytes()
            .first()
            .is_some_and(|byte| !byte.is_ascii_whitespace())
        {
            return Err(ParseError::Invalid);
        }
        Ok((&input[1..end], rest.trim_start()))
    } else {
        let end = input
            .find(|ch: char| ch.is_ascii_whitespace())
            .unwrap_or(input.len());
        let argument = &input[..end];
        if argument.contains(['\'', '"']) {
            return Err(ParseError::Invalid);
        }
        Ok((argument, input[end..].trim_start()))
    }
}

pub fn resolve_path<'a>(
    cwd: &str,
    path: &str,
    output: &'a mut [u8; 256],
) -> Result<&'a str, ParseError> {
    if !cwd.starts_with('/') || path.as_bytes().contains(&0) {
        return Err(ParseError::Invalid);
    }
    output[0] = b'/';
    let mut length = 1usize;
    if !path.starts_with('/') {
        for component in cwd.split('/').filter(|component| !component.is_empty()) {
            push_component(output, &mut length, component)?;
        }
    }
    for component in path.split('/').filter(|component| !component.is_empty()) {
        match component {
            "." => {}
            ".." => pop_component(output, &mut length),
            component => push_component(output, &mut length, component)?,
        }
    }
    core::str::from_utf8(&output[..length]).map_err(|_| ParseError::Invalid)
}

fn push_component(
    output: &mut [u8; 256],
    length: &mut usize,
    component: &str,
) -> Result<(), ParseError> {
    if component.len() > 255 {
        return Err(ParseError::Invalid);
    }
    let separator = usize::from(*length > 1);
    let end = length
        .checked_add(separator)
        .and_then(|value| value.checked_add(component.len()))
        .filter(|end| *end <= 255)
        .ok_or(ParseError::Invalid)?;
    if separator != 0 {
        output[*length] = b'/';
        *length += 1;
    }
    output[*length..end].copy_from_slice(component.as_bytes());
    *length = end;
    Ok(())
}

fn pop_component(output: &[u8; 256], length: &mut usize) {
    if *length <= 1 {
        return;
    }
    *length = output[..*length]
        .iter()
        .rposition(|byte| *byte == b'/')
        .unwrap_or(0)
        .max(1);
}

#[cfg(test)]
mod tests {
    use super::{Command, ParseError, parse, parse_service, resolve_path};

    #[test]
    fn native_install_paths_permissions_and_budgets_have_an_explicit_boundary() {
        use super::{AppCommand, parse_app};
        assert_eq!(
            parse_app("install example v2 '/data/my app.elf' 8 stats"),
            Ok(AppCommand::Install {
                name: "example",
                version: "v2",
                source: "/data/my app.elf",
                pages: 8,
                permissions: 2
            })
        );
        assert_eq!(
            parse_app("rollback example"),
            Ok(AppCommand::Rollback("example"))
        );
        for command in [
            "install ../x v1 /data/app.elf",
            "install x .. /data/app.elf",
            "install x v1 relative",
            "install x v1 /data/app.elf 0",
            "install x v1 /data/app.elf 4097",
            "install x v1 /data/app.elf 8 admin",
            "run example extra",
        ] {
            assert!(parse_app(command).is_err());
        }
    }

    #[test]
    fn service_management_requires_a_known_service_and_an_explicit_mutation_target() {
        use microsystem_abi::service::Operation;
        assert_eq!(parse_service(""), Ok((Operation::List, None)));
        assert_eq!(parse_service("status mfs"), Ok((Operation::List, Some(3))));
        assert_eq!(
            parse_service("restart terminal"),
            Ok((Operation::Restart, Some(7)))
        );
        assert_eq!(parse_service("stop block"), Ok((Operation::Stop, Some(2))));
        for arguments in [
            "restart",
            "stop",
            "restart all",
            "status mfs extra",
            "unknown mfs",
        ] {
            assert_eq!(parse_service(arguments), Err(ParseError::Invalid));
        }
    }

    #[test]
    fn parses_terminal_filesystem_commands_and_arguments() {
        assert_eq!(parse("stat /data/note"), Ok(Command::Stat("/data/note")));
        assert_eq!(
            parse("mv /data/old /data/new"),
            Ok(Command::Rename {
                source: "/data/old",
                destination: "/data/new",
            })
        );
        assert_eq!(
            parse("rm /data/new"),
            Ok(Command::Unlink {
                path: "/data/new",
                recursive: false,
            })
        );
    }

    #[test]
    fn parses_complete_filesystem_command_aliases() {
        assert_eq!(
            parse("touch /data/empty"),
            Ok(Command::Touch("/data/empty"))
        );
        assert_eq!(
            parse("fs cp /data/a /data/b"),
            Ok(Command::Copy {
                source: "/data/a",
                destination: "/data/b",
                recursive: false,
            })
        );
        assert_eq!(parse("fsync /data/a"), Ok(Command::Fsync("/data/a")));
        assert_eq!(parse("fs rmdir /data"), Ok(Command::Rmdir("/data")));
        assert_eq!(parse("fs help"), Ok(Command::FsHelp));
        assert_eq!(parse("read /data/a"), Ok(Command::Cat("/data/a")));
        assert_eq!(
            parse("unlink /data/a"),
            Ok(Command::Unlink {
                path: "/data/a",
                recursive: false,
            })
        );
        assert_eq!(
            parse("curl --include -o /data/response http://example.test/status"),
            Ok(Command::Curl(
                "--include -o /data/response http://example.test/status"
            ))
        );
    }

    #[test]
    fn rejects_ambiguous_filesystem_arguments() {
        assert_eq!(parse("stat"), Err(ParseError::MissingArgument));
        assert_eq!(parse("mv /data/old"), Err(ParseError::MissingArgument));
        assert_eq!(
            parse("mv /data/old /data/new extra"),
            Err(ParseError::Invalid)
        );
        assert_eq!(parse("rm /data/new extra"), Err(ParseError::Invalid));
        assert_eq!(parse("fs cp /data/old"), Err(ParseError::MissingArgument));
        assert_eq!(parse("fs stat /data/a extra"), Err(ParseError::Invalid));
        assert_eq!(parse("fs"), Err(ParseError::MissingArgument));
    }

    #[test]
    fn parses_extended_shell_commands_and_options() {
        assert_eq!(parse("cd ../tmp"), Ok(Command::Cd("../tmp")));
        assert_eq!(parse("echo hello world"), Ok(Command::Echo("hello world")));
        assert_eq!(
            parse("head -n 3 note"),
            Ok(Command::Head {
                path: "note",
                lines: 3,
            })
        );
        assert_eq!(
            parse("cp -r src dst"),
            Ok(Command::Copy {
                source: "src",
                destination: "dst",
                recursive: true,
            })
        );
        assert_eq!(
            parse("mkdir -p a/b"),
            Ok(Command::Mkdir {
                path: "a/b",
                parents: true,
            })
        );
        assert_eq!(
            parse("rm -R tree"),
            Ok(Command::Unlink {
                path: "tree",
                recursive: true,
            })
        );
        assert_eq!(
            parse("kill 42 -9"),
            Ok(Command::Kill {
                pid: "42",
                status: Some("-9"),
            })
        );
        assert_eq!(
            parse("kill -9 42"),
            Ok(Command::Kill {
                pid: "42",
                status: Some("-9"),
            })
        );
        assert_eq!(parse("xxd file"), Ok(Command::Hexdump("file")));
        assert_eq!(parse("find"), Ok(Command::Find(".")));
        assert_eq!(parse("sysinfo"), Ok(Command::SystemInfo));
    }

    #[test]
    fn parses_sql_as_one_complete_statement() {
        let statement = "SELECT id,  name FROM users WHERE note = 'a; b' ORDER BY id DESC;";
        assert_eq!(
            parse("sql SELECT id,  name FROM users WHERE note = 'a; b' ORDER BY id DESC;"),
            Ok(Command::Sql(statement))
        );
    }

    #[test]
    fn sql_requires_a_statement_argument() {
        assert_eq!(parse("sql"), Err(ParseError::MissingArgument));
        assert_eq!(parse("sql   "), Err(ParseError::MissingArgument));
    }

    #[test]
    fn sql_statement_is_not_truncated_when_parser_input_is_long() {
        const SQL_BYTES: usize = 4097;
        let mut bytes = [b'x'; 12 + SQL_BYTES + 2];
        bytes[..12].copy_from_slice(b"sql SELECT '");
        bytes[12 + SQL_BYTES..].copy_from_slice(b"';");
        let line = core::str::from_utf8(&bytes).unwrap();
        let statement = &line["sql ".len()..];

        assert!(statement.len() > 4096);
        assert_eq!(parse(&line), Ok(Command::Sql(statement)));
    }

    #[test]
    fn resolves_relative_paths_without_escaping_root() {
        let mut output = [0u8; 256];
        assert_eq!(
            resolve_path("/data/work", "../logs/./today", &mut output),
            Ok("/data/logs/today")
        );
        assert_eq!(resolve_path("/", "../../etc", &mut output), Ok("/etc"));
        assert_eq!(resolve_path("/data", "/tmp//a", &mut output), Ok("/tmp/a"));
    }

    #[test]
    fn quoted_file_arguments_work_across_aliases_and_options() {
        assert_eq!(
            parse("volume create '/volumes/data image' 4"),
            Ok(Command::VolumeCreate {
                image: "/volumes/data image",
                mib: 4
            })
        );
        assert_eq!(
            parse("mount '/volumes/data image' /mnt/data ro"),
            Ok(Command::Mount {
                image: "/volumes/data image",
                point: "/mnt/data",
                readonly: true
            })
        );
        assert_eq!(
            parse("fs umount /mnt/data"),
            Ok(Command::Unmount("/mnt/data"))
        );
        assert_eq!(parse("volume list"), Ok(Command::Mounts));
        assert_eq!(parse("df /mnt/a"), Ok(Command::Df("/mnt/a")));
        assert_eq!(parse("mkvol /volumes/data 9"), Err(ParseError::Invalid));
        assert_eq!(
            parse("mount /volumes/data /mnt/data ro extra"),
            Err(ParseError::Invalid)
        );
        assert_eq!(
            parse("chmod 640 '/data/work notes'"),
            Ok(Command::Chmod {
                mode: 0o640,
                path: "/data/work notes"
            })
        );
        assert_eq!(
            parse("fs chown 1000:42 '/data/work notes'"),
            Ok(Command::Chown {
                uid: 1000,
                gid: 42,
                path: "/data/work notes"
            })
        );
        assert_eq!(parse("chmod 888 /data/a"), Err(ParseError::Invalid));
        assert_eq!(parse("chmod 1000 /data/a"), Err(ParseError::Invalid));
        assert_eq!(
            parse("chown 4294967296:0 /data/a"),
            Err(ParseError::Invalid)
        );
        assert_eq!(
            parse("ls \"/data/work notes\""),
            Ok(Command::Ls("/data/work notes"))
        );
        assert_eq!(
            parse("fs\tcp\t-r\t'/data/work notes'\t\"/data/backup notes\""),
            Ok(Command::Copy {
                source: "/data/work notes",
                destination: "/data/backup notes",
                recursive: true,
            })
        );
        assert_eq!(
            parse("head -n 2 '/data/work notes'"),
            Ok(Command::Head {
                path: "/data/work notes",
                lines: 2
            })
        );
        assert_eq!(
            parse("grep 'hello world' \"/data/work notes\""),
            Ok(Command::Grep {
                pattern: "hello world",
                path: "/data/work notes"
            })
        );
        assert_eq!(
            parse("write '/data/work notes' hello world"),
            Ok(Command::Write {
                path: "/data/work notes",
                value: "hello world"
            })
        );
        assert_eq!(
            parse("fs append '/data/work notes' \" and more\""),
            Ok(Command::Append {
                path: "/data/work notes",
                value: " and more"
            })
        );
        assert_eq!(
            parse("write '/data/empty file' ''"),
            Ok(Command::Write {
                path: "/data/empty file",
                value: ""
            })
        );
    }

    #[test]
    fn malformed_quotes_and_extra_arguments_cannot_reach_file_mutations() {
        for command in [
            "rm '/data/work notes",
            "rm /data/work\"notes",
            "rm '/data/a'junk",
            "mv '/data/work notes' '/data/new notes' extra",
            "write '/data/work notes' 'unterminated",
            "ls /data/a /data/b",
            "append /data/a 'value' extra",
            "append /data/a value\0",
        ] {
            assert_eq!(parse(command), Err(ParseError::Invalid), "{command}");
        }
        assert_eq!(parse("append /data/a"), Err(ParseError::MissingArgument));
    }

    #[test]
    fn rejects_unsupported_or_malformed_extended_commands() {
        for command in [
            "wget http://example.test",
            "ping example.test",
            "ln /data/a /data/b",
            "jobs",
        ] {
            assert_eq!(parse(command), Err(ParseError::Unknown), "{command}");
        }
        assert_eq!(parse("head -n nope /data/a"), Err(ParseError::Invalid));
        assert_eq!(parse("head -n 0 /data/a"), Err(ParseError::Invalid));
        assert_eq!(parse("kill 1 -9 extra"), Err(ParseError::Invalid));
    }
}
