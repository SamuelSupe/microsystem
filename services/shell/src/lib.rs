#![no_std]

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Command<'a> {
    Empty,
    Help,
    Ps,
    Uptime,
    Ls(&'a str),
    Cat(&'a str),
    Stat(&'a str),
    Write { path: &'a str, value: &'a str },
    Mkdir(&'a str),
    Rename { source: &'a str, destination: &'a str },
    Unlink(&'a str),
    Run(&'a str),
    MicaEval(&'a str),
    MicaFile(&'a str),
    MicaRepl,
    MicaArgs(&'a str),
    Sync,
    Shutdown,
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
    match name {
        "help" if rest.is_empty() => Ok(Command::Help),
        "ps" if rest.is_empty() => Ok(Command::Ps),
        "uptime" if rest.is_empty() => Ok(Command::Uptime),
        "ls" => Ok(Command::Ls(if rest.is_empty() { "/" } else { rest })),
        "cat" if !rest.is_empty() => Ok(Command::Cat(rest)),
        "stat" if !rest.is_empty() => Ok(Command::Stat(rest)),
        "mkdir" if !rest.is_empty() => Ok(Command::Mkdir(rest)),
        "mv" => {
            let mut arguments = rest.split_whitespace();
            let source = arguments.next().ok_or(ParseError::MissingArgument)?;
            let destination = arguments.next().ok_or(ParseError::MissingArgument)?;
            if arguments.next().is_some() {
                Err(ParseError::Invalid)
            } else {
                Ok(Command::Rename {
                    source,
                    destination,
                })
            }
        }
        "rm" => {
            let mut arguments = rest.split_whitespace();
            let path = arguments.next().ok_or(ParseError::MissingArgument)?;
            if arguments.next().is_some() {
                Err(ParseError::Invalid)
            } else {
                Ok(Command::Unlink(path))
            }
        }
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
        "write" => {
            let (path, value) = rest.split_once(' ').ok_or(ParseError::MissingArgument)?;
            if path.is_empty() {
                Err(ParseError::MissingArgument)
            } else {
                Ok(Command::Write { path, value })
            }
        }
        "sync" if rest.is_empty() => Ok(Command::Sync),
        "shutdown" if rest.is_empty() => Ok(Command::Shutdown),
        "cat" | "stat" | "mkdir" | "run" => Err(ParseError::MissingArgument),
        _ => Err(ParseError::Unknown),
    }
}

#[cfg(test)]
mod tests {
    use super::{Command, ParseError, parse};

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
        assert_eq!(parse("rm /data/new"), Ok(Command::Unlink("/data/new")));
    }

    #[test]
    fn rejects_ambiguous_filesystem_arguments() {
        assert_eq!(parse("stat"), Err(ParseError::MissingArgument));
        assert_eq!(parse("mv /data/old"), Err(ParseError::MissingArgument));
        assert_eq!(
            parse("mv /data/old /data/new extra"),
            Err(ParseError::Invalid)
        );
        assert_eq!(
            parse("rm /data/new extra"),
            Err(ParseError::Invalid)
        );
    }
}
