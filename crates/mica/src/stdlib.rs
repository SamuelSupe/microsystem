use alloc::string::{String, ToString};
use alloc::vec::Vec;

use base64::Engine as _;
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};

use crate::{ErrorValue, Value, json};

pub fn module(name: &str) -> Option<Value> {
    let functions: &[&str] = match name {
        "json" => &["encode", "decode"],
        "encoding" => &[
            "hex_encode",
            "hex_decode",
            "base64_encode",
            "base64_decode",
            "utf8_valid",
        ],
        "hash" => &["sha256", "hmac_sha256"],
        "string" => &[
            "len",
            "lower",
            "upper",
            "sub",
            "find",
            "trim",
            "starts_with",
            "ends_with",
            "replace",
            "collapse_space",
        ],
        "bytes" => &["from_string", "to_string", "len"],
        "table" => &["len", "insert", "remove", "next"],
        "math" => &["abs", "floor", "ceil", "min", "max"],
        "fs" => &[
            "stat",
            "list",
            "open",
            "read",
            "write",
            "fsync",
            "close",
            "read_file",
            "write_file",
            "mkdir",
            "rename",
            "unlink",
            "sync",
        ],
        "proc" => &["list", "spawn", "wait", "kill"],
        "time" => &["uptime", "sleep", "realtime"],
        "sys" => &["version", "stats"],
        "random" => &["bytes", "int"],
        "net" => &[
            "resolve",
            "tcp_connect",
            "tcp_read",
            "tcp_write",
            "tcp_close",
            "udp_open",
            "udp_send_to",
            "udp_recv_from",
            "udp_close",
            "tls_connect",
            "cancel",
        ],
        "http" => &["request", "get", "post", "put", "patch", "delete"],
        "io" => &["read", "write", "flush"],
        "args" => &["get", "all"],
        _ => return None,
    };
    Some(Value::Table(
        functions
            .iter()
            .map(|function| {
                (
                    Value::String((*function).to_string()),
                    Value::Native(alloc::format!("{name}.{function}")),
                )
            })
            .collect(),
    ))
}

pub fn call(name: &str, arguments: &[Value]) -> Option<Result<Vec<Value>, ErrorValue>> {
    Some(match name {
        "require" => require(arguments),
        "json.encode" => unary(arguments, |value| json::encode(value).map(Value::String)),
        "json.decode" => unary_string(arguments, |value| json::decode(value)),
        "encoding.hex_encode" => {
            unary_bytes(arguments, |value| Ok(Value::String(hex_encode(value))))
        }
        "encoding.hex_decode" => {
            unary_string(arguments, |value| hex_decode(value).map(Value::Bytes))
        }
        "encoding.base64_encode" => unary_bytes(arguments, |value| {
            Ok(Value::String(
                base64::engine::general_purpose::STANDARD.encode(value),
            ))
        }),
        "encoding.base64_decode" => unary_string(arguments, |value| {
            base64::engine::general_purpose::STANDARD
                .decode(value)
                .map(Value::Bytes)
                .map_err(|_| ErrorValue::new("encoding", "invalid base64"))
        }),
        "encoding.utf8_valid" => unary_bytes(arguments, |value| {
            Ok(Value::Bool(core::str::from_utf8(value).is_ok()))
        }),
        "hash.sha256" => unary_bytes(arguments, |value| {
            Ok(Value::Bytes(Sha256::digest(value).to_vec()))
        }),
        "hash.hmac_sha256" => hmac_sha256(arguments),
        "string.len" => unary_string(arguments, |value| {
            Ok(Value::Integer(value.chars().count() as i64))
        }),
        "string.lower" => unary_string(arguments, |value| Ok(Value::String(value.to_lowercase()))),
        "string.upper" => unary_string(arguments, |value| Ok(Value::String(value.to_uppercase()))),
        "string.sub" => string_sub(arguments),
        "string.find" => string_find(arguments),
        "string.trim" => unary_string(arguments, |value| {
            Ok(Value::String(value.trim().to_string()))
        }),
        "string.starts_with" => {
            string_predicate(arguments, |value, needle| value.starts_with(needle))
        }
        "string.ends_with" => string_predicate(arguments, |value, needle| value.ends_with(needle)),
        "string.replace" => string_replace(arguments),
        "string.collapse_space" => unary_string(arguments, |value| {
            let mut output = String::with_capacity(value.len());
            let mut pending_space = false;
            for character in value.chars() {
                if character.is_whitespace() {
                    pending_space = !output.is_empty();
                } else {
                    if pending_space {
                        output.push(' ');
                    }
                    output.push(character);
                    pending_space = false;
                }
            }
            Ok(Value::String(output))
        }),
        "bytes.from_string" => unary_string(arguments, |value| {
            Ok(Value::Bytes(value.as_bytes().to_vec()))
        }),
        "bytes.to_string" => unary_bytes(arguments, |value| {
            core::str::from_utf8(value)
                .map(|value| Value::String(value.to_string()))
                .map_err(|_| ErrorValue::new("utf8", "bytes are not valid UTF-8"))
        }),
        "bytes.len" => unary_bytes(arguments, |value| Ok(Value::Integer(value.len() as i64))),
        "table.len" => unary_table(arguments, |value| Ok(Value::Integer(value.len() as i64))),
        "table.insert" => table_insert(arguments),
        "table.remove" => table_remove(arguments),
        "table.next" => table_next(arguments),
        "math.abs" => unary_number(arguments, |value| match value {
            Number::Integer(value) => value
                .checked_abs()
                .map(Value::Integer)
                .ok_or_else(|| ErrorValue::new("overflow", "integer overflow")),
            Number::Float(value) => Ok(Value::Float(value.abs())),
        }),
        "math.floor" => unary_number(arguments, |value| {
            Ok(Value::Float(libm::floor(value.float())))
        }),
        "math.ceil" => unary_number(arguments, |value| {
            Ok(Value::Float(libm::ceil(value.float())))
        }),
        "math.min" => minmax(arguments, true),
        "math.max" => minmax(arguments, false),
        _ => return None,
    })
}

fn require(arguments: &[Value]) -> Result<Vec<Value>, ErrorValue> {
    let name = string_argument(arguments, 0)?;
    module(name)
        .map(|value| alloc::vec![value])
        .ok_or_else(|| ErrorValue::new("module", "module not found").operation(name))
}

fn unary(
    arguments: &[Value],
    operation: impl FnOnce(&Value) -> Result<Value, ErrorValue>,
) -> Result<Vec<Value>, ErrorValue> {
    let value = arguments
        .first()
        .ok_or_else(|| ErrorValue::new("argument", "missing argument"))?;
    Ok(alloc::vec![operation(value)?])
}
fn unary_string(
    arguments: &[Value],
    operation: impl FnOnce(&str) -> Result<Value, ErrorValue>,
) -> Result<Vec<Value>, ErrorValue> {
    let value = string_argument(arguments, 0)?;
    Ok(alloc::vec![operation(value)?])
}
fn unary_bytes(
    arguments: &[Value],
    operation: impl FnOnce(&[u8]) -> Result<Value, ErrorValue>,
) -> Result<Vec<Value>, ErrorValue> {
    let bytes = bytes_argument(arguments, 0)?;
    Ok(alloc::vec![operation(bytes)?])
}
fn unary_table(
    arguments: &[Value],
    operation: impl FnOnce(&[(Value, Value)]) -> Result<Value, ErrorValue>,
) -> Result<Vec<Value>, ErrorValue> {
    let Value::Table(value) = arguments
        .first()
        .ok_or_else(|| ErrorValue::new("argument", "missing argument"))?
    else {
        return Err(ErrorValue::new("type", "expected table"));
    };
    Ok(alloc::vec![operation(value)?])
}

enum Number {
    Integer(i64),
    Float(f64),
}
impl Number {
    fn float(&self) -> f64 {
        match self {
            Self::Integer(value) => *value as f64,
            Self::Float(value) => *value,
        }
    }
}
fn unary_number(
    arguments: &[Value],
    operation: impl FnOnce(Number) -> Result<Value, ErrorValue>,
) -> Result<Vec<Value>, ErrorValue> {
    let value = match arguments.first() {
        Some(Value::Integer(value)) => Number::Integer(*value),
        Some(Value::Float(value)) => Number::Float(*value),
        _ => return Err(ErrorValue::new("type", "expected number")),
    };
    Ok(alloc::vec![operation(value)?])
}

fn string_argument(arguments: &[Value], index: usize) -> Result<&str, ErrorValue> {
    match arguments.get(index) {
        Some(Value::String(value)) => Ok(value),
        _ => Err(ErrorValue::new("type", "expected string")),
    }
}
fn bytes_argument(arguments: &[Value], index: usize) -> Result<&[u8], ErrorValue> {
    match arguments.get(index) {
        Some(Value::Bytes(value)) => Ok(value),
        Some(Value::String(value)) => Ok(value.as_bytes()),
        _ => Err(ErrorValue::new("type", "expected bytes or string")),
    }
}

fn hex_encode(input: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(input.len() * 2);
    for byte in input {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 15) as usize] as char)
    }
    output
}
fn hex_decode(input: &str) -> Result<Vec<u8>, ErrorValue> {
    if input.len() % 2 != 0 {
        return Err(ErrorValue::new("encoding", "hex length must be even"));
    }
    let mut output = Vec::with_capacity(input.len() / 2);
    for pair in input.as_bytes().chunks_exact(2) {
        let high = hex_digit(pair[0])?;
        let low = hex_digit(pair[1])?;
        output.push(high << 4 | low)
    }
    Ok(output)
}
fn hex_digit(value: u8) -> Result<u8, ErrorValue> {
    match value {
        b'0'..=b'9' => Ok(value - b'0'),
        b'a'..=b'f' => Ok(value - b'a' + 10),
        b'A'..=b'F' => Ok(value - b'A' + 10),
        _ => Err(ErrorValue::new("encoding", "invalid hex")),
    }
}
fn hmac_sha256(arguments: &[Value]) -> Result<Vec<Value>, ErrorValue> {
    let key = bytes_argument(arguments, 0)?;
    let data = bytes_argument(arguments, 1)?;
    let mut mac = Hmac::<Sha256>::new_from_slice(key)
        .map_err(|_| ErrorValue::new("hash", "invalid HMAC key"))?;
    mac.update(data);
    Ok(alloc::vec![Value::Bytes(
        mac.finalize().into_bytes().to_vec()
    )])
}
fn string_sub(arguments: &[Value]) -> Result<Vec<Value>, ErrorValue> {
    let value = string_argument(arguments, 0)?;
    let start = match arguments.get(1) {
        Some(Value::Integer(value)) => *value,
        _ => return Err(ErrorValue::new("type", "expected integer start")),
    };
    let end = match arguments.get(2) {
        Some(Value::Integer(value)) => *value,
        None => value.chars().count() as i64,
        _ => return Err(ErrorValue::new("type", "expected integer end")),
    };
    if start < 1 || end < start {
        return Ok(alloc::vec![Value::String(String::new())]);
    }
    let output = value
        .chars()
        .skip(start as usize - 1)
        .take((end - start + 1) as usize)
        .collect();
    Ok(alloc::vec![Value::String(output)])
}
fn string_find(arguments: &[Value]) -> Result<Vec<Value>, ErrorValue> {
    let value = string_argument(arguments, 0)?;
    let needle = string_argument(arguments, 1)?;
    let start = match arguments.get(2) {
        Some(Value::Integer(value)) if *value >= 1 => *value as usize,
        None => 1,
        _ => return Err(ErrorValue::new("type", "expected positive start index")),
    };
    let byte_start = value
        .char_indices()
        .nth(start.saturating_sub(1))
        .map(|(index, _)| index)
        .unwrap_or(value.len());
    let found = value[byte_start..]
        .find(needle)
        .map(|offset| value[..byte_start + offset].chars().count() as i64 + 1);
    Ok(alloc::vec![found.map(Value::Integer).unwrap_or(Value::Nil)])
}
fn string_predicate(
    arguments: &[Value],
    predicate: impl FnOnce(&str, &str) -> bool,
) -> Result<Vec<Value>, ErrorValue> {
    let value = string_argument(arguments, 0)?;
    let needle = string_argument(arguments, 1)?;
    Ok(alloc::vec![Value::Bool(predicate(value, needle))])
}
fn string_replace(arguments: &[Value]) -> Result<Vec<Value>, ErrorValue> {
    const MAX_RESULT_BYTES: usize = 256 * 1024;
    let value = string_argument(arguments, 0)?;
    let needle = string_argument(arguments, 1)?;
    let replacement = string_argument(arguments, 2)?;
    if needle.is_empty() {
        return Err(ErrorValue::new("argument", "replacement pattern is empty"));
    }
    let matches = value.match_indices(needle).count();
    let bytes = value
        .len()
        .checked_sub(matches.saturating_mul(needle.len()))
        .and_then(|bytes| bytes.checked_add(matches.saturating_mul(replacement.len())))
        .ok_or_else(|| ErrorValue::new("limit", "replacement result is too large"))?;
    if bytes > MAX_RESULT_BYTES {
        return Err(ErrorValue::new(
            "limit",
            "replacement result exceeds 256 KiB",
        ));
    }
    Ok(alloc::vec![Value::String(
        value.replace(needle, replacement)
    )])
}
fn table_insert(arguments: &[Value]) -> Result<Vec<Value>, ErrorValue> {
    let Some(Value::Table(entries)) = arguments.first() else {
        return Err(ErrorValue::new("type", "expected table"));
    };
    let value = arguments
        .get(1)
        .cloned()
        .ok_or_else(|| ErrorValue::new("argument", "missing value"))?;
    let mut output = entries.clone();
    output.push((Value::Integer(output.len() as i64 + 1), value));
    Ok(alloc::vec![Value::Table(output)])
}
fn table_remove(arguments: &[Value]) -> Result<Vec<Value>, ErrorValue> {
    let Some(Value::Table(entries)) = arguments.first() else {
        return Err(ErrorValue::new("type", "expected table"));
    };
    let index = match arguments.get(1) {
        Some(Value::Integer(value)) if *value >= 1 => *value as usize - 1,
        _ => return Err(ErrorValue::new("type", "expected positive index")),
    };
    let mut output = entries.clone();
    let removed = if index < output.len() {
        output.remove(index).1
    } else {
        Value::Nil
    };
    for (position, (key, _)) in output.iter_mut().enumerate() {
        if matches!(key, Value::Integer(_)) {
            *key = Value::Integer(position as i64 + 1)
        }
    }
    Ok(alloc::vec![Value::Table(output), removed])
}
fn table_next(arguments: &[Value]) -> Result<Vec<Value>, ErrorValue> {
    let Some(Value::Table(entries)) = arguments.first() else {
        return Err(ErrorValue::new("type", "expected table"));
    };
    let index = match arguments.get(1) {
        Some(Value::Integer(value)) if *value >= 1 => *value as usize - 1,
        _ => return Err(ErrorValue::new("type", "expected positive index")),
    };
    Ok(alloc::vec![
        entries
            .get(index)
            .map(|(key, value)| Value::Table(alloc::vec![
                (Value::Integer(1), key.clone()),
                (Value::Integer(2), value.clone())
            ]))
            .unwrap_or(Value::Nil)
    ])
}
fn minmax(arguments: &[Value], minimum: bool) -> Result<Vec<Value>, ErrorValue> {
    let mut values = arguments.iter();
    let first = values
        .next()
        .ok_or_else(|| ErrorValue::new("argument", "expected at least one number"))?;
    let mut best = match first {
        Value::Integer(value) => *value as f64,
        Value::Float(value) => *value,
        _ => return Err(ErrorValue::new("type", "expected number")),
    };
    for value in values {
        let value = match value {
            Value::Integer(value) => *value as f64,
            Value::Float(value) => *value,
            _ => return Err(ErrorValue::new("type", "expected number")),
        };
        if (minimum && value < best) || (!minimum && value > best) {
            best = value
        }
    }
    Ok(alloc::vec![Value::Float(best)])
}
