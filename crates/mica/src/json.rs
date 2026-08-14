use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::{ErrorValue, Value};

pub const MAX_INPUT: usize = 1024 * 1024;
pub const MAX_DEPTH: usize = 64;

pub fn decode(input: &str) -> Result<Value, ErrorValue> {
    if input.len() > MAX_INPUT {
        return Err(ErrorValue::new("limit", "JSON input exceeds 1 MiB"));
    }
    let mut parser = Parser {
        input: input.as_bytes(),
        cursor: 0,
    };
    let value = parser.value(0)?;
    parser.space();
    if parser.cursor != parser.input.len() {
        return Err(parser.error("trailing JSON input"));
    }
    Ok(value)
}

pub fn encode(value: &Value) -> Result<String, ErrorValue> {
    let mut output = String::new();
    encode_value(value, &mut output, 0)?;
    if output.len() > MAX_INPUT {
        return Err(ErrorValue::new("limit", "JSON output exceeds 1 MiB"));
    }
    Ok(output)
}

fn encode_value(value: &Value, output: &mut String, depth: usize) -> Result<(), ErrorValue> {
    if depth > MAX_DEPTH {
        return Err(ErrorValue::new("limit", "JSON nesting exceeds 64"));
    }
    match value {
        Value::Nil => output.push_str("null"),
        Value::Bool(v) => output.push_str(if *v { "true" } else { "false" }),
        Value::Integer(v) => output.push_str(&v.to_string()),
        Value::Float(v) if v.is_finite() => output.push_str(&v.to_string()),
        Value::String(v) => encode_string(v, output),
        Value::Table(entries) => {
            let array = entries
                .iter()
                .enumerate()
                .all(|(i, (key, _))| matches!(key,Value::Integer(value) if *value==i as i64+1));
            if array {
                output.push('[');
                for (index, (_, item)) in entries.iter().enumerate() {
                    if index > 0 {
                        output.push(',')
                    }
                    encode_value(item, output, depth + 1)?;
                }
                output.push(']')
            } else {
                output.push('{');
                for (index, (key, item)) in entries.iter().enumerate() {
                    let Value::String(key) = key else {
                        return Err(ErrorValue::new("type", "JSON object keys must be strings"));
                    };
                    if index > 0 {
                        output.push(',')
                    }
                    encode_string(key, output);
                    output.push(':');
                    encode_value(item, output, depth + 1)?;
                }
                output.push('}')
            }
        }
        Value::Float(_) => {
            return Err(ErrorValue::new(
                "number",
                "JSON cannot encode non-finite number",
            ));
        }
        _ => return Err(ErrorValue::new("type", "value is not JSON encodable")),
    }
    Ok(())
}
fn encode_string(value: &str, output: &mut String) {
    output.push('"');
    for character in value.chars() {
        match character {
            '"' => output.push_str("\\\""),
            '\\' => output.push_str("\\\\"),
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            '\t' => output.push_str("\\t"),
            value if value < ' ' => {
                use core::fmt::Write;
                let _ = write!(output, "\\u{:04x}", value as u32);
            }
            value => output.push(value),
        }
    }
    output.push('"')
}

struct Parser<'a> {
    input: &'a [u8],
    cursor: usize,
}
impl Parser<'_> {
    fn value(&mut self, depth: usize) -> Result<Value, ErrorValue> {
        if depth > MAX_DEPTH {
            return Err(self.error("JSON nesting exceeds 64"));
        }
        self.space();
        match self.peek() {
            Some(b'n') => {
                self.literal(b"null")?;
                Ok(Value::Nil)
            }
            Some(b't') => {
                self.literal(b"true")?;
                Ok(Value::Bool(true))
            }
            Some(b'f') => {
                self.literal(b"false")?;
                Ok(Value::Bool(false))
            }
            Some(b'"') => Ok(Value::String(self.string()?)),
            Some(b'[') => self.array(depth + 1),
            Some(b'{') => self.object(depth + 1),
            Some(b'-' | b'0'..=b'9') => self.number(),
            _ => Err(self.error("expected JSON value")),
        }
    }
    fn array(&mut self, depth: usize) -> Result<Value, ErrorValue> {
        self.cursor += 1;
        let mut entries = Vec::new();
        self.space();
        if self.take(b']') {
            return Ok(Value::Table(entries));
        }
        let mut index = 1i64;
        loop {
            entries.push((Value::Integer(index), self.value(depth)?));
            index += 1;
            self.space();
            if self.take(b']') {
                break;
            }
            if !self.take(b',') {
                return Err(self.error("expected ',' or ']'"));
            }
        }
        Ok(Value::Table(entries))
    }
    fn object(&mut self, depth: usize) -> Result<Value, ErrorValue> {
        self.cursor += 1;
        let mut entries: Vec<(Value, Value)> = Vec::new();
        self.space();
        if self.take(b'}') {
            return Ok(Value::Table(entries));
        }
        loop {
            self.space();
            if self.peek() != Some(b'"') {
                return Err(self.error("object key must be string"));
            }
            let key = self.string()?;
            if entries
                .iter()
                .any(|(candidate, _)| matches!(candidate,Value::String(value)if value==&key))
            {
                return Err(self.error("duplicate object key"));
            }
            self.space();
            if !self.take(b':') {
                return Err(self.error("expected ':'"));
            }
            let value = self.value(depth)?;
            entries.push((Value::String(key), value));
            self.space();
            if self.take(b'}') {
                break;
            }
            if !self.take(b',') {
                return Err(self.error("expected ',' or '}'"));
            }
        }
        Ok(Value::Table(entries))
    }
    fn number(&mut self) -> Result<Value, ErrorValue> {
        let start = self.cursor;
        if self.take(b'-') {}
        while self.peek().is_some_and(|v| v.is_ascii_digit()) {
            self.cursor += 1
        }
        let mut float = false;
        if self.take(b'.') {
            float = true;
            while self.peek().is_some_and(|v| v.is_ascii_digit()) {
                self.cursor += 1
            }
        }
        if matches!(self.peek(), Some(b'e' | b'E')) {
            float = true;
            self.cursor += 1;
            if matches!(self.peek(), Some(b'+' | b'-')) {
                self.cursor += 1
            }
            while self.peek().is_some_and(|v| v.is_ascii_digit()) {
                self.cursor += 1
            }
        }
        let text = core::str::from_utf8(&self.input[start..self.cursor])
            .map_err(|_| self.error("invalid number"))?;
        if float {
            Ok(Value::Float(
                text.parse().map_err(|_| self.error("invalid number"))?,
            ))
        } else {
            Ok(Value::Integer(
                text.parse()
                    .map_err(|_| self.error("integer out of range"))?,
            ))
        }
    }
    fn string(&mut self) -> Result<String, ErrorValue> {
        self.cursor += 1;
        let mut output = String::new();
        loop {
            let byte = self
                .peek()
                .ok_or_else(|| self.error("unterminated string"))?;
            self.cursor += 1;
            match byte {
                b'"' => return Ok(output),
                b'\\' => {
                    let escaped = self
                        .peek()
                        .ok_or_else(|| self.error("unterminated escape"))?;
                    self.cursor += 1;
                    match escaped {
                        b'"' => output.push('"'),
                        b'\\' => output.push('\\'),
                        b'/' => output.push('/'),
                        b'b' => output.push('\u{0008}'),
                        b'f' => output.push('\u{000c}'),
                        b'n' => output.push('\n'),
                        b'r' => output.push('\r'),
                        b't' => output.push('\t'),
                        b'u' => {
                            let code = self.hex4()?;
                            let character = char::from_u32(code)
                                .ok_or_else(|| self.error("invalid Unicode escape"))?;
                            output.push(character)
                        }
                        _ => return Err(self.error("invalid escape")),
                    }
                }
                value if value < 0x20 => return Err(self.error("control character in string")),
                value if value < 0x80 => output.push(value as char),
                _ => {
                    let start = self.cursor - 1;
                    let remaining = core::str::from_utf8(&self.input[start..])
                        .map_err(|_| self.error("invalid UTF-8"))?;
                    let character = remaining
                        .chars()
                        .next()
                        .ok_or_else(|| self.error("invalid UTF-8"))?;
                    self.cursor = start + character.len_utf8();
                    output.push(character)
                }
            }
        }
    }
    fn hex4(&mut self) -> Result<u32, ErrorValue> {
        if self.cursor + 4 > self.input.len() {
            return Err(self.error("short Unicode escape"));
        }
        let text = core::str::from_utf8(&self.input[self.cursor..self.cursor + 4])
            .map_err(|_| self.error("invalid Unicode escape"))?;
        self.cursor += 4;
        u32::from_str_radix(text, 16).map_err(|_| self.error("invalid Unicode escape"))
    }
    fn literal(&mut self, value: &[u8]) -> Result<(), ErrorValue> {
        if self.input.get(self.cursor..self.cursor + value.len()) == Some(value) {
            self.cursor += value.len();
            Ok(())
        } else {
            Err(self.error("invalid literal"))
        }
    }
    fn space(&mut self) {
        while self.peek().is_some_and(|value| value.is_ascii_whitespace()) {
            self.cursor += 1
        }
    }
    fn take(&mut self, value: u8) -> bool {
        if self.peek() == Some(value) {
            self.cursor += 1;
            true
        } else {
            false
        }
    }
    fn peek(&self) -> Option<u8> {
        self.input.get(self.cursor).copied()
    }
    fn error(&self, message: &str) -> ErrorValue {
        ErrorValue::new(
            "json",
            alloc::format!("{} at byte {}", message, self.cursor),
        )
    }
}
