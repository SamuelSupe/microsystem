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
    Df,
    Stat(&'a str),
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

pub fn parse(line: &str) -> Result<Command<'_>, ParseError> {
    let line = line.trim();
    if line.is_empty() {
        return Ok(Command::Empty);
    }
    let (name, rest) = line
        .split_once(' ')
        .map(|(a, b)| (a, b.trim()))
        .unwrap_or((line, ""));
    if name == "fs" {
        if rest.is_empty() {
            return Err(ParseError::MissingArgument);
        }
        let (subcommand, arguments) = rest
            .split_once(' ')
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
        "df" if rest.is_empty() => Ok(Command::Df),
        "stat" => parse_filesystem_command("stat", rest),
        "touch" | "create" => parse_filesystem_command("touch", rest),
        "cp" => parse_filesystem_command("cp", rest),
        "write" => parse_filesystem_command("write", rest),
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
        "ls" | "list" => Ok(Command::Ls(if rest.is_empty() { "." } else { rest })),
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
        "df" if rest.is_empty() => Ok(Command::Df),
        "stat" => Ok(Command::Stat(one_argument(rest)?)),
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
        "write" => {
            let (path, value) = rest.split_once(' ').ok_or(ParseError::MissingArgument)?;
            if path.is_empty() || value.is_empty() {
                Err(ParseError::MissingArgument)
            } else {
                Ok(Command::Write { path, value })
            }
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
    let mut words = rest.split_whitespace();
    let first = words.next().ok_or(ParseError::MissingArgument)?;
    let (lines, path) = if first == "-n" {
        let lines = words
            .next()
            .ok_or(ParseError::MissingArgument)?
            .parse::<usize>()
            .map_err(|_| ParseError::Invalid)?;
        let path = words.next().ok_or(ParseError::MissingArgument)?;
        (lines, path)
    } else {
        (10, first)
    };
    if lines == 0 || words.next().is_some() {
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
    let mut arguments = rest.split_whitespace();
    let argument = arguments.next().ok_or(ParseError::MissingArgument)?;
    if arguments.next().is_some() {
        Err(ParseError::Invalid)
    } else {
        Ok(argument)
    }
}

fn two_arguments(rest: &str) -> Result<(&str, &str), ParseError> {
    let mut arguments = rest.split_whitespace();
    let first = arguments.next().ok_or(ParseError::MissingArgument)?;
    let second = arguments.next().ok_or(ParseError::MissingArgument)?;
    if arguments.next().is_some() {
        Err(ParseError::Invalid)
    } else {
        Ok((first, second))
    }
}

fn one_or_two_arguments(rest: &str) -> Result<(&str, Option<&str>), ParseError> {
    let mut arguments = rest.split_whitespace();
    let first = arguments.next().ok_or(ParseError::MissingArgument)?;
    let second = arguments.next();
    if arguments.next().is_some() {
        Err(ParseError::Invalid)
    } else {
        Ok((first, second))
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
    use super::{Command, ParseError, parse, resolve_path};

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
    fn rejects_unsupported_or_malformed_extended_commands() {
        for command in [
            "wget http://example.test",
            "ping example.test",
            "chmod 755 /data/a",
            "chown root /data/a",
            "ln /data/a /data/b",
            "mount /dev/vda /mnt",
            "jobs",
        ] {
            assert_eq!(parse(command), Err(ParseError::Unknown), "{command}");
        }
        assert_eq!(parse("head -n nope /data/a"), Err(ParseError::Invalid));
        assert_eq!(parse("head -n 0 /data/a"), Err(ParseError::Invalid));
        assert_eq!(parse("kill 1 -9 extra"), Err(ParseError::Invalid));
        assert_eq!(parse("df /data"), Err(ParseError::Unknown));
    }
}
