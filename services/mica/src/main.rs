#![no_std]
#![no_main]

extern crate alloc;

mod gui;
mod tls;

use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::panic::PanicInfo;
use microsystem_abi::{
    CapHandle, Message, ProcessInfoV2, Rights, Status, SystemStats, filesystem, network, protocol,
    script,
};
use microsystem_mica::{
    Access, ErrorValue, Host, Limits, PermissionSet, Value, Vm, VmError, compile,
    compile_with_prelude,
};

struct SessionOutput {
    base: *mut u8,
}

impl SessionOutput {
    fn write(&mut self, bytes: &[u8]) {
        let header = unsafe { &mut *self.base.cast::<script::SessionHeaderV1>() };
        let head = header.stdout_head as usize;
        let available = script::STDOUT_BYTES.saturating_sub(head);
        let count = bytes.len().min(available);
        if count == 0 {
            return;
        }
        unsafe {
            core::ptr::copy_nonoverlapping(
                bytes.as_ptr(),
                self.base.add(script::STDOUT_OFFSET + head),
                count,
            )
        };
        header.stdout_head = (head + count) as u32;
    }
}

struct KernelHost {
    output: SessionOutput,
    random: CapHandle,
    system_info: CapHandle,
    filesystem: CapHandle,
    network: CapHandle,
    token: u64,
    permissions: PermissionSet,
    arguments: Vec<String>,
    script_directory: String,
    modules: Vec<(String, Option<Value>)>,
    module_source_bytes: usize,
    http_pending: [Option<Vec<u8>>; network::MAX_CONNECTIONS_PER_SESSION],
    http_browse_connect: bool,
    gui: Option<gui::GuiHost>,
}

impl Host for KernelHost {
    fn resolve(&mut self, name: &str) -> Option<Value> {
        microsystem_mica::stdlib::module(name)
    }

    fn call(&mut self, name: &str, arguments: &[Value]) -> Result<Vec<Value>, ErrorValue> {
        if name == "require" {
            return self.require_module(arguments);
        }
        if let Some(result) = microsystem_mica::stdlib::call(name, arguments) {
            return result;
        }
        if name.starts_with("_gui_") {
            return self
                .gui
                .as_mut()
                .ok_or_else(|| ErrorValue::new("access", "GUI session is not enabled"))?
                .call(name, arguments);
        }
        match name {
            "print" => {
                for (index, value) in arguments.iter().enumerate() {
                    if index != 0 {
                        self.output.write(b"\t");
                    }
                    let value = value.display();
                    self.output.write(value.as_bytes());
                }
                self.output.write(b"\n");
                Ok(Vec::new())
            }
            "io.write" => {
                for value in arguments {
                    self.output.write(value.display().as_bytes());
                }
                Ok(alloc::vec![Value::Bool(true), Value::Nil])
            }
            "io.flush" => Ok(alloc::vec![Value::Bool(true), Value::Nil]),
            "args.get" => {
                let index = integer_argument(arguments, 0)?;
                if index < 1 {
                    return Ok(alloc::vec![Value::Nil, Value::Nil]);
                }
                Ok(alloc::vec![
                    self.arguments
                        .get(index as usize - 1)
                        .cloned()
                        .map(Value::String)
                        .unwrap_or(Value::Nil),
                    Value::Nil,
                ])
            }
            "args.all" => Ok(alloc::vec![
                Value::Table(
                    self.arguments
                        .iter()
                        .enumerate()
                        .map(|(index, value)| {
                            (
                                Value::Integer(index as i64 + 1),
                                Value::String(value.clone()),
                            )
                        })
                        .collect(),
                ),
                Value::Nil,
            ]),
            "time.uptime" => system_pair(
                microsystem_user_rt::clock_now()
                    .map(|value| Value::Integer(value as i64))
                    .map_err(|status| host_status("time.uptime", status)),
            ),
            "time.realtime" => system_pair(
                microsystem_user_rt::clock_realtime()
                    .map(|value| Value::Integer(value as i64))
                    .map_err(|status| host_status("time.realtime", status)),
            ),
            "time.sleep" => system_pair((|| {
                let duration = integer_argument(arguments, 0)?;
                if !(0..=60_000).contains(&duration) {
                    return Err(ErrorValue::new("argument", "sleep must be 0..60000 ms"));
                }
                let start = microsystem_user_rt::clock_now()
                    .map_err(|status| host_status("time.sleep", status))?;
                let deadline = start.saturating_add(duration as u64 * 1_000_000);
                while microsystem_user_rt::clock_now().unwrap_or(deadline) < deadline {
                    let _ = microsystem_user_rt::yield_now();
                }
                Ok(Value::Bool(true))
            })()),
            "random.bytes" => system_pair((|| {
                if self.random == CapHandle::INVALID
                    || !self.permissions.allows_unscoped("random", Access::Random)
                {
                    return Err(ErrorValue::new("access", "random permission denied"));
                }
                let bytes = integer_argument(arguments, 0)?;
                if !(1..=256).contains(&bytes) {
                    return Err(ErrorValue::new(
                        "argument",
                        "random byte count must be 1..256",
                    ));
                }
                let mut output = alloc::vec![0u8; bytes as usize];
                microsystem_user_rt::random_fill(self.random, &mut output)
                    .map_err(|status| host_status("random.bytes", status))?;
                Ok(Value::Bytes(output))
            })()),
            "random.int" => system_pair((|| {
                if self.random == CapHandle::INVALID
                    || !self.permissions.allows_unscoped("random", Access::Random)
                {
                    return Err(ErrorValue::new("access", "random permission denied"));
                }
                let minimum = integer_argument(arguments, 0)?;
                let maximum = integer_argument(arguments, 1)?;
                if maximum < minimum {
                    return Err(ErrorValue::new("argument", "random range is empty"));
                }
                let width = (maximum as i128 - minimum as i128 + 1) as u128;
                let zone = (u64::MAX as u128 + 1) / width * width;
                let value = loop {
                    let mut bytes = [0u8; 8];
                    microsystem_user_rt::random_fill(self.random, &mut bytes)
                        .map_err(|status| host_status("random.int", status))?;
                    let candidate = u64::from_le_bytes(bytes) as u128;
                    if candidate < zone {
                        break minimum as i128 + (candidate % width) as i128;
                    }
                };
                Ok(Value::Integer(value as i64))
            })()),
            "sys.version" => Ok(alloc::vec![
                Value::String("MicroSystem 0.1.0".into()),
                Value::Nil
            ]),
            "sys.stats" => system_pair((|| {
                if self.system_info == CapHandle::INVALID
                    || !self.permissions.allows_unscoped("sys", Access::Stats)
                {
                    return Err(ErrorValue::new("access", "sys.stats permission denied"));
                }
                let mut stats = SystemStats::default();
                microsystem_user_rt::system_stats(self.system_info, &mut stats)
                    .map_err(|status| host_status("sys.stats", status))?;
                Ok(system_stats_value(&stats))
            })()),
            name if name.starts_with("fs.") => match self.call_filesystem(name, arguments) {
                Ok(value) => Ok(value),
                Err(error) => Ok(alloc::vec![Value::Nil, error_value(error)]),
            },
            name if name.starts_with("proc.") => match self.call_process(name, arguments) {
                Ok(value) => Ok(value),
                Err(error) => Ok(alloc::vec![Value::Nil, error_value(error)]),
            },
            name if name.starts_with("net.") => match self.call_network(name, arguments) {
                Ok(value) => Ok(value),
                Err(error) => Ok(alloc::vec![Value::Nil, error_value(error)]),
            },
            name if name.starts_with("http.") => match self.call_http(name, arguments) {
                Ok(value) => Ok(value),
                Err(error) => Ok(alloc::vec![Value::Nil, error_value(error)]),
            },
            _ => Err(ErrorValue::new("name", "unknown host function").operation(name)),
        }
    }

    fn gc_roots(&self) -> Vec<Value> {
        self.gui
            .as_ref()
            .map(gui::GuiHost::gc_roots)
            .unwrap_or_default()
    }

    fn now_ns(&mut self) -> u64 {
        microsystem_user_rt::clock_now().unwrap_or(0)
    }

    fn yield_now(&mut self) {
        let _ = microsystem_user_rt::yield_now();
    }

    fn interrupted(&mut self) -> bool {
        unsafe {
            (*self.output.base.cast::<script::SessionHeaderV1>()).flags & script::FLAG_INTERRUPT
                != 0
        }
    }
}

impl KernelHost {
    fn require_module(&mut self, arguments: &[Value]) -> Result<Vec<Value>, ErrorValue> {
        let name = string_argument(arguments, 0)?;
        if let Some(module) = microsystem_mica::stdlib::module(name) {
            return Ok(alloc::vec![module]);
        }
        if name.is_empty()
            || name.starts_with('/')
            || name
                .split('/')
                .any(|component| component.is_empty() || matches!(component, "." | ".."))
            || !name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'/'))
        {
            return Err(ErrorValue::new("module", "invalid module name"));
        }
        if let Some((_, value)) = self.modules.iter().find(|(cached, _)| cached == name) {
            return value
                .clone()
                .map(|value| alloc::vec![value])
                .ok_or_else(|| ErrorValue::new("module", "cyclic require").operation(name));
        }
        self.modules.push((name.to_string(), None));
        let mut candidates = Vec::with_capacity(3);
        let directory = self.script_directory.trim_end_matches('/');
        if !directory.is_empty() {
            candidates.push(alloc::format!("{directory}/{name}.mica"));
            candidates.push(alloc::format!("{directory}/{name}/init.mica"));
        }
        candidates.push(alloc::format!("/.system/mica/{name}.mica"));
        let mut loaded = None;
        for path in candidates {
            if !self.permissions.allows_path(Access::Read, &path) {
                continue;
            }
            if let Ok(bytes) = self.read_path_chunks(&path, microsystem_mica::SOURCE_LIMIT) {
                loaded = Some((path, bytes));
                break;
            }
        }
        let result = (|| {
            let (path, bytes) = loaded
                .ok_or_else(|| ErrorValue::new("module", "module not found").operation(name))?;
            self.module_source_bytes = self.module_source_bytes.saturating_add(bytes.len());
            if bytes.len() > microsystem_mica::SOURCE_LIMIT
                || self.module_source_bytes > microsystem_mica::MODULE_SOURCE_LIMIT
            {
                return Err(
                    ErrorValue::new("limit", "module source limit exceeded").operation(path)
                );
            }
            let source = core::str::from_utf8(&bytes)
                .map_err(|_| ErrorValue::new("utf8", "module is not UTF-8").operation(&path))?;
            let chunk = compile(source)
                .map_err(|error| ErrorValue::new("compile", error.message).operation(&path))?;
            let outcome = Vm::new(&chunk, &mut *self)
                .with_limits(Limits {
                    instructions: 1_000_000,
                    timeout_ns: 2_000_000_000,
                    ..Limits::default()
                })
                .run()
                .map_err(|_| {
                    ErrorValue::new("module", "module execution failed").operation(&path)
                })?;
            Ok(outcome
                .values
                .into_iter()
                .next()
                .unwrap_or(Value::Bool(true)))
        })();
        match result {
            Ok(value) => {
                if let Some((_, slot)) = self.modules.iter_mut().find(|(cached, _)| cached == name)
                {
                    *slot = Some(value.clone());
                }
                Ok(alloc::vec![value])
            }
            Err(error) => {
                self.modules.retain(|(cached, _)| cached != name);
                Err(error)
            }
        }
    }

    fn call_http(&mut self, name: &str, arguments: &[Value]) -> Result<Vec<Value>, ErrorValue> {
        if name == "http.response_read_all" {
            let response = arguments
                .first()
                .ok_or_else(|| ErrorValue::new("argument", "missing response"))?;
            let connection = table_integer(response, "connection")? as u64;
            let content_length = table_integer_optional(response, "content_length")?;
            let chunked = table_bool(Some(response), "chunked");
            let complete = table_bool(Some(response), "complete");
            let maximum = arguments
                .get(1)
                .map(|_| positive_argument(arguments, 1))
                .transpose()?
                .unwrap_or(1024 * 1024) as usize;
            if maximum > 4 * 1024 * 1024 {
                return Err(ErrorValue::new("limit", "HTTP body limit exceeds 4 MiB"));
            }
            let slot = connection
                .checked_sub(1)
                .filter(|slot| *slot < self.http_pending.len() as u64)
                .ok_or_else(|| ErrorValue::new("argument", "invalid HTTP response"))?
                as usize;
            let mut body = self.http_pending[slot].take().unwrap_or_default();
            if !complete {
                loop {
                    if content_length.is_some_and(|length| body.len() >= length as usize) {
                        break;
                    }
                    if body.len() >= maximum {
                        return Err(ErrorValue::new("limit", "HTTP body exceeds read limit"));
                    }
                    let chunk = self.tcp_read_value(
                        connection,
                        (maximum - body.len()).min(script::BROKER_BYTES),
                    )?;
                    if chunk.is_empty() {
                        break;
                    }
                    body.extend_from_slice(&chunk);
                }
                let _ = self.tcp_close_value(connection);
            }
            if body.len() > maximum {
                return Err(ErrorValue::new("limit", "HTTP body exceeds read limit"));
            }
            if let Some(length) = content_length {
                if body.len() < length as usize {
                    return Err(ErrorValue::new("http", "truncated HTTP response body"));
                }
                body.truncate(length as usize);
            }
            if chunked {
                body = decode_chunked(&body, maximum)?;
            }
            return Ok(alloc::vec![Value::Bytes(body), Value::Nil]);
        }

        let (method, url, body, options) = if name == "http.request" {
            (
                string_argument(arguments, 0)?.to_ascii_uppercase(),
                string_argument(arguments, 1)?,
                arguments
                    .get(2)
                    .map(value_bytes)
                    .transpose()?
                    .unwrap_or(&[]),
                arguments.get(3),
            )
        } else {
            let method = match name {
                "http.get" => "GET",
                "http.post" => "POST",
                "http.put" => "PUT",
                "http.patch" => "PATCH",
                "http.delete" => "DELETE",
                _ => return Err(ErrorValue::new("name", "unknown HTTP function")),
            };
            (
                method.to_string(),
                string_argument(arguments, 0)?,
                arguments
                    .get(1)
                    .map(value_bytes)
                    .transpose()?
                    .unwrap_or(&[]),
                arguments.get(2),
            )
        };
        if !matches!(method.as_str(), "GET" | "POST" | "PUT" | "PATCH" | "DELETE") {
            return Err(ErrorValue::new("http", "unsupported HTTP method"));
        }
        let url = parse_http_url(url)?;
        let exact_destination = self.permissions.allows_host(&url.host, url.port);
        let browse_destination =
            method == "GET" && self.permissions.allows_unscoped("net", Access::Browse);
        if !exact_destination && !browse_destination {
            return Err(ErrorValue::new("access", "HTTP destination denied"));
        }
        let mut request = Vec::new();
        request.extend_from_slice(method.as_bytes());
        request.push(b' ');
        request.extend_from_slice(url.path.as_bytes());
        request.extend_from_slice(b" HTTP/1.1\r\nHost: ");
        request.extend_from_slice(url.host.as_bytes());
        if url.port != if url.tls { 443 } else { 80 } {
            request.push(b':');
            append_decimal(&mut request, url.port as u64);
        }
        request.extend_from_slice(
            b"\r\nUser-Agent: MicaReader/1.0\r\nAccept: text/html,text/plain;q=0.9,*/*;q=0.1\r\nConnection: close\r\n",
        );
        if !body.is_empty() {
            request.extend_from_slice(b"Content-Length: ");
            append_decimal(&mut request, body.len() as u64);
            request.extend_from_slice(b"\r\n");
        }
        if let Some(Value::Table(entries)) = options {
            if let Some(Value::Table(headers)) = entries
                .iter()
                .find(|(key, _)| key == &Value::String("headers".into()))
                .map(|(_, value)| value)
            {
                for (key, value) in headers {
                    let (Value::String(key), Value::String(value)) = (key, value) else {
                        return Err(ErrorValue::new("http", "headers must be string pairs"));
                    };
                    if !valid_header(key)
                        || value
                            .chars()
                            .any(|character| matches!(character, '\r' | '\n'))
                    {
                        return Err(ErrorValue::new("http", "invalid HTTP header"));
                    }
                    request.extend_from_slice(key.as_bytes());
                    request.extend_from_slice(b": ");
                    request.extend_from_slice(value.as_bytes());
                    request.extend_from_slice(b"\r\n");
                }
            }
        }
        request.extend_from_slice(b"\r\n");
        request.extend_from_slice(body);
        if request.len() > script::BROKER_BYTES {
            return Err(ErrorValue::new("limit", "HTTP request exceeds 32 KiB"));
        }
        self.http_browse_connect = browse_destination && !exact_destination;
        let exchange = (|| {
            if url.tls {
                let bundle =
                    self.read_trusted_path_chunks("/.system/certs/ca-bundle.derpack", 256 * 1024)?;
                let random = self.random;
                let (connection, response) =
                    tls::exchange(self, random, &url.host, url.port, &request, &bundle)?;
                Ok((connection, response, true))
            } else {
                let connection = self.tcp_connect_value(&url.host, url.port)?;
                let mut sent = 0usize;
                while sent < request.len() {
                    let written = self.tcp_write_value(connection, &request[sent..])?;
                    if written == 0 {
                        return Err(ErrorValue::new(
                            "http",
                            "connection closed while writing request",
                        ));
                    }
                    sent += written;
                }
                let mut response = Vec::new();
                loop {
                    if find_bytes(&response, b"\r\n\r\n").is_some() {
                        break;
                    }
                    if response.len() >= 16 * 1024 {
                        let _ = self.tcp_close_value(connection);
                        return Err(ErrorValue::new("limit", "HTTP headers exceed 16 KiB"));
                    }
                    let chunk = self.tcp_read_value(connection, 4096)?;
                    if chunk.is_empty() {
                        return Err(ErrorValue::new(
                            "http",
                            "connection closed before HTTP headers",
                        ));
                    }
                    response.extend_from_slice(&chunk);
                }
                Ok((connection, response, false))
            }
        })();
        self.http_browse_connect = false;
        let (network_connection, received, complete) = exchange?;
        let header_end = find_bytes(&received, b"\r\n\r\n")
            .map(|end| end + 4)
            .ok_or_else(|| ErrorValue::new("http", "connection closed before HTTP headers"))?;
        if header_end > 16 * 1024 {
            return Err(ErrorValue::new("limit", "HTTP headers exceed 16 KiB"));
        }
        let headers = core::str::from_utf8(&received[..header_end])
            .map_err(|_| ErrorValue::new("http", "HTTP headers are not UTF-8"))?;
        let mut lines = headers.split("\r\n");
        let status_line = lines.next().unwrap_or("");
        let status = status_line
            .split_whitespace()
            .nth(1)
            .and_then(|value| value.parse::<i64>().ok())
            .filter(|status| (100..=599).contains(status))
            .ok_or_else(|| ErrorValue::new("http", "invalid HTTP status line"))?;
        let mut content_length = None;
        let mut chunked = false;
        let mut content_type = None;
        let mut location = None;
        for line in lines {
            let Some((name, value)) = line.split_once(':') else {
                continue;
            };
            if name.eq_ignore_ascii_case("content-length") {
                content_length = value.trim().parse::<u64>().ok();
            } else if name.eq_ignore_ascii_case("transfer-encoding")
                && value.trim().eq_ignore_ascii_case("chunked")
            {
                chunked = true;
            } else if name.eq_ignore_ascii_case("content-type") && content_type.is_none() {
                let value = value.trim();
                if value.len() <= 256 {
                    content_type = Some(value.to_string());
                }
            } else if name.eq_ignore_ascii_case("location") && location.is_none() {
                let value = value.trim();
                if value.len() <= 2_048 {
                    location = Some(value.to_string());
                }
            }
        }
        let connection = if complete {
            self.http_pending
                .iter()
                .position(Option::is_none)
                .map(|slot| slot as u64 + 1)
                .ok_or_else(|| ErrorValue::new("limit", "too many pending HTTP responses"))?
        } else {
            network_connection
        };
        let slot = connection
            .checked_sub(1)
            .filter(|slot| *slot < self.http_pending.len() as u64)
            .ok_or_else(|| ErrorValue::new("fault", "invalid HTTP connection identifier"))?
            as usize;
        self.http_pending[slot] = Some(received[header_end..].to_vec());
        Ok(alloc::vec![
            Value::Table(alloc::vec![
                (Value::String("status".into()), Value::Integer(status)),
                (
                    Value::String("connection".into()),
                    Value::Integer(connection as i64)
                ),
                (
                    Value::String("content_length".into()),
                    content_length
                        .map(|value| Value::Integer(value as i64))
                        .unwrap_or(Value::Nil),
                ),
                (Value::String("chunked".into()), Value::Bool(chunked)),
                (Value::String("complete".into()), Value::Bool(complete)),
                (
                    Value::String("content_type".into()),
                    content_type.map(Value::String).unwrap_or(Value::Nil),
                ),
                (
                    Value::String("location".into()),
                    location.map(Value::String).unwrap_or(Value::Nil),
                ),
                (
                    Value::String("read_all".into()),
                    Value::Native("http.response_read_all".into()),
                ),
            ]),
            Value::Nil,
        ])
    }

    fn tcp_connect_value(&mut self, host: &str, port: u16) -> Result<u64, ErrorValue> {
        let values = self.call_network(
            "net.tcp_connect",
            &[Value::String(host.into()), Value::Integer(port as i64)],
        )?;
        match values.first() {
            Some(Value::Integer(connection)) if *connection > 0 => Ok(*connection as u64),
            _ => Err(ErrorValue::new("network", "TCP connection failed")),
        }
    }

    fn tcp_read_value(&mut self, connection: u64, maximum: usize) -> Result<Vec<u8>, ErrorValue> {
        let values = self.call_network(
            "net.tcp_read",
            &[
                Value::Integer(connection as i64),
                Value::Integer(maximum as i64),
            ],
        )?;
        match values.first() {
            Some(Value::Bytes(bytes)) => Ok(bytes.clone()),
            _ => Err(ErrorValue::new("network", "TCP read failed")),
        }
    }

    fn tcp_write_value(&mut self, connection: u64, data: &[u8]) -> Result<usize, ErrorValue> {
        let values = self.call_network(
            "net.tcp_write",
            &[
                Value::Integer(connection as i64),
                Value::Bytes(data.to_vec()),
            ],
        )?;
        match values.first() {
            Some(Value::Integer(bytes)) if *bytes >= 0 => Ok(*bytes as usize),
            _ => Err(ErrorValue::new("network", "TCP write failed")),
        }
    }

    fn tcp_close_value(&mut self, connection: u64) -> Result<(), ErrorValue> {
        self.call_network("net.tcp_close", &[Value::Integer(connection as i64)])?;
        Ok(())
    }

    fn call_network(&mut self, name: &str, arguments: &[Value]) -> Result<Vec<Value>, ErrorValue> {
        match name {
            "net.resolve" => {
                let host = string_argument(arguments, 0)?;
                if !self.permissions.allows_host_any_port(host) {
                    return Err(ErrorValue::new("access", "network host permission denied"));
                }
                let address = self.resolve_host(host)?;
                Ok(alloc::vec![Value::String(ipv4_text(address)), Value::Nil])
            }
            "net.tcp_connect" => {
                let host = string_argument(arguments, 0)?;
                let port = positive_argument(arguments, 1)?;
                let allowed = self.permissions.allows_host(host, port as u16)
                    || (self.http_browse_connect
                        && self.permissions.allows_unscoped("net", Access::Browse));
                if port > u16::MAX as i64 || !allowed {
                    return Err(ErrorValue::new("access", "network destination denied"));
                }
                let address = self.resolve_host(host)?;
                let deadline = microsystem_user_rt::clock_now()
                    .unwrap_or(0)
                    .saturating_add(10_000_000_000);
                let mut connection = 0u64;
                loop {
                    let mut words = [0u64; 5];
                    words[0] = connection;
                    words[1] = address as u64;
                    words[2] = port as u64;
                    words[3] = host.len() as u64;
                    words[4] = if self.http_browse_connect {
                        network::BROWSE_REQUEST_MAGIC
                    } else {
                        0
                    };
                    let (reply, status, _) = self.network_request(
                        network::Operation::TcpConnect,
                        words,
                        host.as_bytes(),
                    )?;
                    connection = reply.words[0].max(connection);
                    if status == Status::Ok {
                        return Ok(alloc::vec![Value::Integer(connection as i64), Value::Nil]);
                    }
                    if status != Status::Busy {
                        return Err(host_status("net.tcp_connect", status));
                    }
                    if microsystem_user_rt::clock_now().unwrap_or(deadline) >= deadline {
                        return Err(ErrorValue::new("timeout", "TCP connect timed out"));
                    }
                    let _ = microsystem_user_rt::yield_now();
                }
            }
            "net.tcp_read" => {
                let connection = positive_argument(arguments, 0)? as u64;
                let maximum = positive_argument(arguments, 1)? as usize;
                if maximum > script::BROKER_BYTES {
                    return Err(ErrorValue::new("limit", "network read exceeds 32 KiB"));
                }
                let deadline = microsystem_user_rt::clock_now()
                    .unwrap_or(0)
                    .saturating_add(10_000_000_000);
                loop {
                    let (reply, status, payload) = self.network_request(
                        network::Operation::TcpRead,
                        [connection, maximum as u64, 0, 0, 0],
                        &[],
                    )?;
                    if status == Status::Ok {
                        let bytes = reply.words[0] as usize;
                        if bytes > payload.len() {
                            return Err(ErrorValue::new("fault", "invalid network read length"));
                        }
                        return Ok(alloc::vec![
                            Value::Bytes(payload[..bytes].to_vec()),
                            Value::Nil
                        ]);
                    }
                    if status != Status::Busy {
                        return Err(host_status("net.tcp_read", status));
                    }
                    if microsystem_user_rt::clock_now().unwrap_or(deadline) >= deadline {
                        return Err(ErrorValue::new("timeout", "TCP read timed out"));
                    }
                    let _ = microsystem_user_rt::yield_now();
                }
            }
            "net.tcp_write" => {
                let connection = positive_argument(arguments, 0)? as u64;
                let data = bytes_argument(arguments, 1)?;
                if data.is_empty() || data.len() > script::BROKER_BYTES {
                    return Err(ErrorValue::new(
                        "limit",
                        "network write must be 1..32768 bytes",
                    ));
                }
                let deadline = microsystem_user_rt::clock_now()
                    .unwrap_or(0)
                    .saturating_add(10_000_000_000);
                loop {
                    let (reply, status, _) = self.network_request(
                        network::Operation::TcpWrite,
                        [connection, data.len() as u64, 0, 0, 0],
                        data,
                    )?;
                    if status == Status::Ok {
                        return Ok(alloc::vec![
                            Value::Integer(reply.words[0] as i64),
                            Value::Nil
                        ]);
                    }
                    if status != Status::Busy {
                        return Err(host_status("net.tcp_write", status));
                    }
                    if microsystem_user_rt::clock_now().unwrap_or(deadline) >= deadline {
                        return Err(ErrorValue::new("timeout", "TCP write timed out"));
                    }
                    let _ = microsystem_user_rt::yield_now();
                }
            }
            "net.tcp_close" => {
                let connection = positive_argument(arguments, 0)? as u64;
                let (_, status, _) = self.network_request(
                    network::Operation::TcpClose,
                    [connection, 0, 0, 0, 0],
                    &[],
                )?;
                if status != Status::Ok {
                    return Err(host_status("net.tcp_close", status));
                }
                Ok(alloc::vec![Value::Bool(true), Value::Nil])
            }
            "net.cancel" => {
                let (_, status, _) =
                    self.network_request(network::Operation::Cancel, [0; 5], &[])?;
                if status != Status::Ok {
                    return Err(host_status("net.cancel", status));
                }
                Ok(alloc::vec![Value::Bool(true), Value::Nil])
            }
            "net.tls_connect" => Err(ErrorValue::new(
                "not_supported",
                "TLS transport is not available in this build",
            )),
            "net.udp_open" => {
                let local_port = arguments
                    .first()
                    .map(|_| integer_argument(arguments, 0))
                    .transpose()?
                    .unwrap_or(0);
                if !(0..=u16::MAX as i64).contains(&local_port) {
                    return Err(ErrorValue::new("argument", "invalid UDP local port"));
                }
                let (reply, status, _) = self.network_request(
                    network::Operation::UdpOpen,
                    [local_port as u64, 0, 0, 0, 0],
                    &[],
                )?;
                if status != Status::Ok {
                    return Err(host_status("net.udp_open", status));
                }
                Ok(alloc::vec![
                    Value::Integer(reply.words[0] as i64),
                    Value::Nil
                ])
            }
            "net.udp_send_to" => {
                let socket = positive_argument(arguments, 0)? as u64;
                let host = string_argument(arguments, 1)?;
                let port = positive_argument(arguments, 2)?;
                let data = bytes_argument(arguments, 3)?;
                if port > u16::MAX as i64 || !self.permissions.allows_host(host, port as u16) {
                    return Err(ErrorValue::new("access", "network destination denied"));
                }
                if data.is_empty() || host.len().saturating_add(data.len()) > script::BROKER_BYTES {
                    return Err(ErrorValue::new(
                        "limit",
                        "UDP datagram exceeds transfer limit",
                    ));
                }
                let address = self.resolve_host(host)?;
                let mut payload = Vec::with_capacity(host.len() + data.len());
                payload.extend_from_slice(host.as_bytes());
                payload.extend_from_slice(data);
                let (reply, status, _) = self.network_request(
                    network::Operation::UdpSendTo,
                    [
                        socket,
                        address as u64,
                        port as u64,
                        host.len() as u64,
                        data.len() as u64,
                    ],
                    &payload,
                )?;
                if status != Status::Ok {
                    return Err(host_status("net.udp_send_to", status));
                }
                Ok(alloc::vec![
                    Value::Integer(reply.words[0] as i64),
                    Value::Nil
                ])
            }
            "net.udp_recv_from" => {
                let socket = positive_argument(arguments, 0)? as u64;
                let maximum = positive_argument(arguments, 1)? as usize;
                if maximum > script::BROKER_BYTES {
                    return Err(ErrorValue::new(
                        "limit",
                        "UDP receive exceeds transfer limit",
                    ));
                }
                let deadline = microsystem_user_rt::clock_now()
                    .unwrap_or(0)
                    .saturating_add(10_000_000_000);
                loop {
                    let (reply, status, payload) = self.network_request(
                        network::Operation::UdpRecvFrom,
                        [socket, maximum as u64, 0, 0, 0],
                        &[],
                    )?;
                    if status == Status::Ok {
                        return Ok(alloc::vec![
                            Value::Table(alloc::vec![
                                (Value::String("data".into()), Value::Bytes(payload)),
                                (
                                    Value::String("address".into()),
                                    Value::String(ipv4_text(reply.words[1] as u32))
                                ),
                                (
                                    Value::String("port".into()),
                                    Value::Integer(reply.words[2] as i64)
                                ),
                            ]),
                            Value::Nil,
                        ]);
                    }
                    if status != Status::Busy {
                        return Err(host_status("net.udp_recv_from", status));
                    }
                    if microsystem_user_rt::clock_now().unwrap_or(deadline) >= deadline {
                        return Err(ErrorValue::new("timeout", "UDP receive timed out"));
                    }
                    let _ = microsystem_user_rt::yield_now();
                }
            }
            "net.udp_close" => {
                let socket = positive_argument(arguments, 0)? as u64;
                let (_, status, _) =
                    self.network_request(network::Operation::UdpClose, [socket, 0, 0, 0, 0], &[])?;
                if status != Status::Ok {
                    return Err(host_status("net.udp_close", status));
                }
                Ok(alloc::vec![Value::Bool(true), Value::Nil])
            }
            _ => Err(ErrorValue::new("name", "unknown network function")),
        }
    }

    fn resolve_host(&mut self, host: &str) -> Result<u32, ErrorValue> {
        if let Some(address) = parse_ipv4(host) {
            return Ok(address);
        }
        let deadline = microsystem_user_rt::clock_now()
            .unwrap_or(0)
            .saturating_add(10_000_000_000);
        let mut query = 0u64;
        loop {
            let (reply, status, _) = self.network_request(
                network::Operation::Resolve,
                [
                    query,
                    0,
                    host.len() as u64,
                    0,
                    if self.http_browse_connect {
                        network::BROWSE_REQUEST_MAGIC
                    } else {
                        0
                    },
                ],
                host.as_bytes(),
            )?;
            if status == Status::Ok {
                return Ok(reply.words[0] as u32);
            }
            query = reply.words[0].max(query);
            if status != Status::Busy {
                return Err(host_status("net.resolve", status));
            }
            if microsystem_user_rt::clock_now().unwrap_or(deadline) >= deadline {
                let _ = self.network_request(network::Operation::Cancel, [0; 5], &[]);
                return Err(ErrorValue::new("timeout", "DNS resolution timed out"));
            }
            let _ = microsystem_user_rt::yield_now();
        }
    }

    fn network_request(
        &mut self,
        operation: network::Operation,
        words: [u64; 5],
        data: &[u8],
    ) -> Result<(Message, Status, Vec<u8>), ErrorValue> {
        if self.network == CapHandle::INVALID || data.len() > script::BROKER_BYTES {
            return Err(ErrorValue::new("limit", "network request exceeds 32 KiB"));
        }
        let output_bytes =
            unsafe { (*self.output.base.cast::<script::SessionHeaderV1>()).stdout_head as usize }
                .min(script::STDOUT_BYTES);
        let saved_output = unsafe {
            core::slice::from_raw_parts(self.output.base.add(script::STDOUT_OFFSET), output_bytes)
        }
        .to_vec();
        unsafe {
            core::ptr::copy_nonoverlapping(
                data.as_ptr(),
                self.output.base.add(script::BROKER_OFFSET),
                data.len(),
            );
            core::arch::asm!("dmb ish", options(nostack, preserves_flags));
        }
        let mut request = Message::new(protocol::NETWORK, operation as u16);
        request.words[0] = self.token;
        request.words[1..].copy_from_slice(&words);
        let mut reply = Message::new(protocol::NETWORK, 0);
        let deadline = microsystem_user_rt::clock_now()
            .unwrap_or(0)
            .saturating_add(1_000_000_000);
        let call = microsystem_user_rt::ipc_call(self.network, &request, &mut reply, deadline)
            .map_err(|status| host_status("net", status));
        let result = call.and_then(|()| {
            let status = status_from_word(reply.words[5]);
            let bytes = if matches!(
                operation,
                network::Operation::TcpRead | network::Operation::UdpRecvFrom
            ) && status == Status::Ok
            {
                reply.words[0] as usize
            } else {
                0
            };
            if bytes > script::BROKER_BYTES {
                return Err(ErrorValue::new("fault", "invalid network reply length"));
            }
            let payload = unsafe {
                core::slice::from_raw_parts(self.output.base.add(script::BROKER_OFFSET), bytes)
            }
            .to_vec();
            Ok((reply, status, payload))
        });
        unsafe {
            core::ptr::copy_nonoverlapping(
                saved_output.as_ptr(),
                self.output.base.add(script::STDOUT_OFFSET),
                saved_output.len(),
            );
            (*self.output.base.cast::<script::SessionHeaderV1>()).stdout_head = output_bytes as u32;
        }
        result
    }

    fn call_process(&mut self, name: &str, arguments: &[Value]) -> Result<Vec<Value>, ErrorValue> {
        match name {
            "proc.list" => {
                if !self.permissions.allows_unscoped("proc", Access::List) {
                    return Err(ErrorValue::new("access", "proc.list permission denied"));
                }
                let mut cursor = microsystem_abi::process::FIRST_APPLICATION_PID;
                let mut tasks = Vec::new();
                while cursor != 0 {
                    let mut request =
                        Message::new(protocol::SCRIPT, script::Operation::ProcessList as u16);
                    request.words[0] = self.token;
                    request.words[1] = cursor;
                    let (reply, payload) = self.script_request_bytes(&request)?;
                    if reply.words[0] == 0 {
                        break;
                    }
                    if payload.len() != core::mem::size_of::<ProcessInfoV2>() {
                        return Err(ErrorValue::new("fault", "invalid process information"));
                    }
                    let info = unsafe {
                        core::ptr::read_unaligned(payload.as_ptr().cast::<ProcessInfoV2>())
                    };
                    tasks.push((
                        Value::Integer(tasks.len() as i64 + 1),
                        process_info_value(&info),
                    ));
                    cursor = reply.words[1];
                }
                Ok(alloc::vec![Value::Table(tasks), Value::Nil])
            }
            "proc.spawn" => {
                let program = string_argument(arguments, 0)?;
                if !self.permissions.allows_program(Access::Spawn, program) {
                    return Err(ErrorValue::new("access", "proc.spawn permission denied"));
                }
                let mut request =
                    Message::new(protocol::SCRIPT, script::Operation::ProcessSpawn as u16);
                request.words[0] = self.token;
                request.words[1] = program_id(program);
                let reply = self.script_request(&request)?;
                Ok(alloc::vec![
                    Value::Integer(reply.words[0] as i64),
                    Value::Nil
                ])
            }
            "proc.wait" => {
                let pid = positive_argument(arguments, 0)? as u64;
                loop {
                    let mut request =
                        Message::new(protocol::SCRIPT, script::Operation::ProcessWait as u16);
                    request.words[0] = self.token;
                    request.words[1] = pid;
                    match self.script_request(&request) {
                        Ok(reply) => {
                            return Ok(alloc::vec![
                                Value::Integer(reply.words[0] as i64),
                                Value::Nil,
                            ]);
                        }
                        Err(error) if error.code == Status::Busy as i64 => {
                            let _ = microsystem_user_rt::yield_now();
                        }
                        Err(error) => return Err(error),
                    }
                }
            }
            "proc.kill" => {
                let pid = positive_argument(arguments, 0)? as u64;
                let program = string_argument(arguments, 1)?;
                if !self.permissions.allows_program(Access::Kill, program) {
                    return Err(ErrorValue::new("access", "proc.kill permission denied"));
                }
                let mut request =
                    Message::new(protocol::SCRIPT, script::Operation::ProcessKill as u16);
                request.words[0] = self.token;
                request.words[1] = pid;
                self.script_request(&request)?;
                Ok(alloc::vec![Value::Bool(true), Value::Nil])
            }
            _ => Err(ErrorValue::new("name", "unknown process function")),
        }
    }

    fn script_request(&mut self, request: &Message) -> Result<Message, ErrorValue> {
        let mut reply = Message::new(protocol::SCRIPT, 0);
        microsystem_user_rt::ipc_call(
            microsystem_abi::boot_cap::SCRIPT_BROKER_ENDPOINT,
            request,
            &mut reply,
            0,
        )
        .map_err(|status| host_status("proc", status))?;
        reply_status(reply.words[5])?;
        Ok(reply)
    }

    fn script_request_bytes(
        &mut self,
        request: &Message,
    ) -> Result<(Message, Vec<u8>), ErrorValue> {
        let output_bytes =
            unsafe { (*self.output.base.cast::<script::SessionHeaderV1>()).stdout_head as usize }
                .min(script::STDOUT_BYTES);
        let saved_output = unsafe {
            core::slice::from_raw_parts(self.output.base.add(script::STDOUT_OFFSET), output_bytes)
        }
        .to_vec();
        let mut reply = Message::new(protocol::SCRIPT, 0);
        let result = microsystem_user_rt::ipc_call(
            microsystem_abi::boot_cap::SCRIPT_BROKER_ENDPOINT,
            request,
            &mut reply,
            0,
        )
        .map_err(|status| host_status("proc", status))
        .and_then(|()| reply_status(reply.words[5]))
        .and_then(|()| {
            let bytes = reply.words[0] as usize;
            if bytes > script::BROKER_BYTES {
                return Err(ErrorValue::new(
                    "fault",
                    "invalid script broker reply length",
                ));
            }
            let payload = unsafe {
                core::slice::from_raw_parts(self.output.base.add(script::BROKER_OFFSET), bytes)
            }
            .to_vec();
            Ok((reply, payload))
        });
        unsafe {
            core::ptr::copy_nonoverlapping(
                saved_output.as_ptr(),
                self.output.base.add(script::STDOUT_OFFSET),
                saved_output.len(),
            );
            (*self.output.base.cast::<script::SessionHeaderV1>()).stdout_head = output_bytes as u32;
        }
        result
    }

    fn call_filesystem(
        &mut self,
        name: &str,
        arguments: &[Value],
    ) -> Result<Vec<Value>, ErrorValue> {
        match name {
            "fs.stat" => {
                let path = string_argument(arguments, 0)?;
                self.require_path(Access::Read, path)?;
                let reply = self.fs_request(filesystem::Operation::Stat, path, &[], 0)?;
                let kind = if reply.words[0] == 2 {
                    "directory"
                } else {
                    "file"
                };
                Ok(alloc::vec![
                    Value::Table(alloc::vec![
                        (Value::String("type".into()), Value::String(kind.into())),
                        (
                            Value::String("size".into()),
                            Value::Integer(reply.words[1] as i64)
                        ),
                    ]),
                    Value::Nil,
                ])
            }
            "fs.list" => {
                let path = string_argument(arguments, 0)?;
                self.require_path(Access::Read, path)?;
                let (_, bytes) =
                    self.fs_request_bytes(filesystem::Operation::List, path, &[], 0)?;
                let text = core::str::from_utf8(&bytes)
                    .map_err(|_| ErrorValue::new("utf8", "directory entry is not UTF-8"))?;
                let entries = text
                    .lines()
                    .enumerate()
                    .map(|(index, entry)| {
                        (
                            Value::Integer(index as i64 + 1),
                            Value::String(entry.to_string()),
                        )
                    })
                    .collect();
                Ok(alloc::vec![Value::Table(entries), Value::Nil])
            }
            "fs.open" => {
                let path = string_argument(arguments, 0)?;
                self.require_path(Access::Read, path)?;
                let reply = self.fs_request(filesystem::Operation::Open, path, &[], 0)?;
                Ok(alloc::vec![
                    Value::Integer(reply.words[0] as i64),
                    Value::Nil
                ])
            }
            "fs.read" => {
                let descriptor = positive_argument(arguments, 0)? as u64;
                let (_, bytes) =
                    self.fs_request_bytes(filesystem::Operation::Read, "", &[], descriptor)?;
                Ok(alloc::vec![Value::Bytes(bytes), Value::Nil])
            }
            "fs.write" => {
                let descriptor = positive_argument(arguments, 0)? as u64;
                let data = bytes_argument(arguments, 1)?;
                self.fs_request(filesystem::Operation::Write, "", data, descriptor)?;
                Ok(alloc::vec![Value::Bool(true), Value::Nil])
            }
            "fs.fsync" => {
                let descriptor = positive_argument(arguments, 0)? as u64;
                self.fs_request(filesystem::Operation::Fsync, "", &[], descriptor)?;
                Ok(alloc::vec![Value::Bool(true), Value::Nil])
            }
            "fs.close" => {
                let descriptor = positive_argument(arguments, 0)? as u64;
                self.fs_request(filesystem::Operation::Close, "", &[], descriptor)?;
                Ok(alloc::vec![Value::Bool(true), Value::Nil])
            }
            "fs.read_file" => {
                let path = string_argument(arguments, 0)?;
                self.require_path(Access::Read, path)?;
                let maximum = arguments
                    .get(1)
                    .map(|_| positive_argument(arguments, 1))
                    .transpose()?
                    .unwrap_or(64 * 1024) as usize;
                if maximum > 256 * 1024 {
                    return Err(ErrorValue::new("limit", "read limit exceeds 256 KiB"));
                }
                let bytes = self.read_path_chunks(path, maximum)?;
                Ok(alloc::vec![Value::Bytes(bytes), Value::Nil])
            }
            "fs.write_file" => {
                let path = string_argument(arguments, 0)?.to_string();
                self.require_path(Access::Write, &path)?;
                let data = bytes_argument(arguments, 1)?.to_vec();
                if data.len() > script::BROKER_BYTES {
                    return Err(ErrorValue::new("limit", "write exceeds 32 KiB"));
                }
                let atomic = table_bool(arguments.get(2), "atomic");
                let fsync = table_bool(arguments.get(2), "fsync");
                if atomic {
                    self.fs_request(
                        filesystem::Operation::WriteAtomic,
                        &path,
                        &data,
                        fsync as u64,
                    )?;
                } else {
                    self.fs_request(filesystem::Operation::Write, &path, &data, 0)?;
                    if fsync {
                        self.fs_request(filesystem::Operation::Fsync, &path, &[], 0)?;
                    }
                }
                Ok(alloc::vec![Value::Bool(true), Value::Nil])
            }
            "fs.mkdir" | "fs.unlink" => {
                let path = string_argument(arguments, 0)?;
                self.require_path(Access::Write, path)?;
                let operation = if name == "fs.mkdir" {
                    filesystem::Operation::Mkdir
                } else {
                    filesystem::Operation::Unlink
                };
                self.fs_request(operation, path, &[], 0)?;
                Ok(alloc::vec![Value::Bool(true), Value::Nil])
            }
            "fs.rename" => {
                let source = string_argument(arguments, 0)?;
                let destination = string_argument(arguments, 1)?;
                self.require_path(Access::Write, source)?;
                self.require_path(Access::Write, destination)?;
                self.fs_request(
                    filesystem::Operation::Rename,
                    source,
                    destination.as_bytes(),
                    0,
                )?;
                Ok(alloc::vec![Value::Bool(true), Value::Nil])
            }
            "fs.sync" => {
                if !self.permissions.entries().iter().any(|permission| {
                    permission.namespace == "fs" && permission.access == Access::Write
                }) {
                    return Err(ErrorValue::new("access", "fs.write permission denied"));
                }
                self.fs_request(filesystem::Operation::Sync, "/", &[], 0)?;
                Ok(alloc::vec![Value::Bool(true), Value::Nil])
            }
            _ => Err(ErrorValue::new("name", "unknown filesystem function")),
        }
    }

    fn require_path(&self, access: Access, path: &str) -> Result<(), ErrorValue> {
        if self.permissions.allows_path(access, path) {
            Ok(())
        } else {
            Err(ErrorValue::new("access", "filesystem permission denied"))
        }
    }

    fn fs_request(
        &mut self,
        operation: filesystem::Operation,
        path: &str,
        data: &[u8],
        descriptor: u64,
    ) -> Result<Message, ErrorValue> {
        self.fs_request_bytes(operation, path, data, descriptor)
            .map(|(reply, _)| reply)
    }

    fn fs_request_bytes(
        &mut self,
        operation: filesystem::Operation,
        path: &str,
        data: &[u8],
        descriptor: u64,
    ) -> Result<(Message, Vec<u8>), ErrorValue> {
        self.fs_request_bytes_inner(operation, path, data, descriptor, false)
    }

    fn fs_request_bytes_inner(
        &mut self,
        operation: filesystem::Operation,
        path: &str,
        data: &[u8],
        descriptor: u64,
        trusted_read: bool,
    ) -> Result<(Message, Vec<u8>), ErrorValue> {
        if self.filesystem == CapHandle::INVALID
            || path.len().saturating_add(data.len()) > script::BROKER_BYTES
        {
            return Err(ErrorValue::new(
                "limit",
                "filesystem request exceeds 32 KiB",
            ));
        }
        let header = unsafe { &mut *self.output.base.cast::<script::SessionHeaderV1>() };
        let output_bytes = (header.stdout_head as usize).min(script::STDOUT_BYTES);
        let saved_output = unsafe {
            core::slice::from_raw_parts(self.output.base.add(script::STDOUT_OFFSET), output_bytes)
        }
        .to_vec();
        unsafe {
            let broker = self.output.base.add(script::BROKER_OFFSET);
            core::ptr::copy_nonoverlapping(path.as_ptr(), broker, path.len());
            core::ptr::copy_nonoverlapping(data.as_ptr(), broker.add(path.len()), data.len());
            core::arch::asm!("dmb ish", options(nostack, preserves_flags));
        }
        let mut request = Message::new(protocol::FILESYSTEM, operation as u16);
        request.words[0] = path.len() as u64;
        request.words[1] = data.len() as u64;
        request.words[2] = descriptor;
        request.words[3] = self.token;
        request.words[4] = filesystem::SCRIPT_REQUEST_MAGIC;
        if trusted_read {
            request.words[5] = filesystem::SCRIPT_TRUSTED_READ_MAGIC;
        }
        let _ = microsystem_user_rt::debug_write_u64(
            b"[mica] filesystem request operation=",
            operation as u64,
            b" entered=true\n",
        );
        let mut reply = Message::new(protocol::FILESYSTEM, 0);
        let call = microsystem_user_rt::ipc_call(self.filesystem, &request, &mut reply, 0);
        let _ = microsystem_user_rt::debug_write_u64(
            b"[mica] filesystem request operation=",
            operation as u64,
            b" returned=true\n",
        );
        let result = call
            .map_err(|status| host_status("fs", status))
            .and_then(|()| reply_status(reply.words[5]))
            .and_then(|()| {
                let bytes = reply.words[0] as usize;
                if bytes > script::BROKER_BYTES {
                    return Err(ErrorValue::new("fault", "invalid filesystem reply length"));
                }
                let output = unsafe {
                    core::slice::from_raw_parts(self.output.base.add(script::BROKER_OFFSET), bytes)
                }
                .to_vec();
                Ok((reply, output))
            });
        unsafe {
            core::ptr::copy_nonoverlapping(
                saved_output.as_ptr(),
                self.output.base.add(script::STDOUT_OFFSET),
                saved_output.len(),
            );
        }
        header.stdout_head = output_bytes as u32;
        result
    }

    fn read_path_chunks(&mut self, path: &str, maximum: usize) -> Result<Vec<u8>, ErrorValue> {
        self.read_path_chunks_inner(path, maximum, false)
    }

    fn read_trusted_path_chunks(
        &mut self,
        path: &str,
        maximum: usize,
    ) -> Result<Vec<u8>, ErrorValue> {
        self.read_path_chunks_inner(path, maximum, true)
    }

    fn read_path_chunks_inner(
        &mut self,
        path: &str,
        maximum: usize,
        trusted_read: bool,
    ) -> Result<Vec<u8>, ErrorValue> {
        let mut output = Vec::new();
        loop {
            let (_, chunk) = self.fs_request_bytes_inner(
                filesystem::Operation::ReadRange,
                path,
                &[],
                output.len() as u64,
                trusted_read,
            )?;
            if chunk.is_empty() {
                break;
            }
            if output.len().saturating_add(chunk.len()) > maximum {
                return Err(ErrorValue::new("limit", "file exceeds read limit"));
            }
            let complete = chunk.len() < script::BROKER_BYTES;
            output.extend_from_slice(&chunk);
            if complete {
                break;
            }
        }
        Ok(output)
    }
}

impl tls::NetworkIo for KernelHost {
    fn connect(&mut self, host: &str, port: u16) -> Result<u64, ErrorValue> {
        self.tcp_connect_value(host, port)
    }

    fn read(&mut self, connection: u64, output: &mut [u8]) -> Result<usize, ErrorValue> {
        let bytes = self.tcp_read_value(connection, output.len().min(script::BROKER_BYTES))?;
        output[..bytes.len()].copy_from_slice(&bytes);
        Ok(bytes.len())
    }

    fn write(&mut self, connection: u64, input: &[u8]) -> Result<usize, ErrorValue> {
        self.tcp_write_value(connection, &input[..input.len().min(script::BROKER_BYTES)])
    }

    fn close(&mut self, connection: u64) -> Result<(), ErrorValue> {
        self.tcp_close_value(connection)
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn _start(
    _pid: u64,
    session: u64,
    notification: u64,
    filesystem: u64,
    network: u64,
    random: u64,
    system_info: u64,
) -> ! {
    let session = CapHandle(session as u32);
    if microsystem_user_rt::frame_map(
        session,
        script::SESSION_VA,
        Rights(Rights::READ.0 | Rights::WRITE.0),
    )
    .is_err()
    {
        microsystem_user_rt::exit(1);
    }
    let base = script::SESSION_VA as *mut u8;
    let header = unsafe { *base.cast::<script::SessionHeaderV1>() };
    if header.magic != script::SESSION_MAGIC
        || header.version != script::VERSION
        || !matches!(
            header.mode,
            script::MODE_EVAL | script::MODE_FILE | script::MODE_REPL
        )
        || (header.mode != script::MODE_REPL && header.source_bytes == 0)
        || header.source_bytes as usize > script::SOURCE_BYTES
        || header.policy_bytes as usize > script::POLICY_BYTES
        || header.argv_bytes as usize > script::STDIN_BYTES
        || header.path_bytes as usize
            > script::POLICY_BYTES.saturating_sub(header.policy_bytes as usize)
        || (header.mode == script::MODE_FILE && header.path_bytes == 0)
    {
        microsystem_user_rt::exit(2);
    }
    let _ = microsystem_user_rt::debug_write(
        b"[mica] session isolated=true brokers=fs,network,process time=true\n",
    );
    let inline_source = unsafe {
        core::slice::from_raw_parts(
            base.add(script::SOURCE_OFFSET),
            header.source_bytes as usize,
        )
    }
    .to_vec();
    if core::str::from_utf8(&inline_source).is_err() {
        unsafe { (*base.cast::<script::SessionHeaderV1>()).exit_status = 2 };
        microsystem_user_rt::exit(2);
    }
    let policy = unsafe {
        core::slice::from_raw_parts(
            base.add(script::POLICY_OFFSET),
            header.policy_bytes as usize,
        )
    };
    let launcher_policy = core::str::from_utf8(policy)
        .ok()
        .and_then(|policy| {
            PermissionSet::from_rules(policy.lines().filter(|line| !line.is_empty())).ok()
        })
        .unwrap_or_default();
    let arguments = if header.argv_bytes as usize <= script::STDIN_BYTES {
        unsafe {
            core::slice::from_raw_parts(base.add(script::STDIN_OFFSET), header.argv_bytes as usize)
        }
        .split(|byte| *byte == 0)
        .filter(|value| !value.is_empty())
        .filter_map(|value| core::str::from_utf8(value).ok().map(str::to_string))
        .collect()
    } else {
        Vec::new()
    };
    let path = if header.path_bytes as usize
        <= script::POLICY_BYTES.saturating_sub(header.policy_bytes as usize)
    {
        unsafe {
            core::slice::from_raw_parts(
                base.add(script::POLICY_OFFSET + header.policy_bytes as usize),
                header.path_bytes as usize,
            )
        }
    } else {
        &[]
    };
    let path = core::str::from_utf8(path).unwrap_or("").to_string();
    let script_directory = path
        .rsplit_once('/')
        .map(|(directory, _)| directory)
        .filter(|directory| !directory.is_empty())
        .unwrap_or("/")
        .to_string();
    let mut host = KernelHost {
        output: SessionOutput { base },
        random: CapHandle(random as u32),
        system_info: CapHandle(system_info as u32),
        filesystem: CapHandle(filesystem as u32),
        network: CapHandle(network as u32),
        token: header.permission_mask,
        permissions: launcher_policy.clone(),
        arguments,
        script_directory,
        modules: Vec::new(),
        module_source_bytes: 0,
        http_pending: core::array::from_fn(|_| None),
        http_browse_connect: false,
        gui: None,
    };
    if header.mode == script::MODE_REPL {
        host.permissions = launcher_policy;
        run_repl(host, base, CapHandle(notification as u32), header);
    }
    let source_bytes = if header.mode == script::MODE_FILE {
        match host.read_path_chunks(&path, microsystem_mica::SOURCE_LIMIT) {
            Ok(source) => source,
            Err(error) => {
                host.output.write(b"compile error: ");
                host.output.write(error.message.as_bytes());
                host.output.write(b"\n");
                unsafe { (*base.cast::<script::SessionHeaderV1>()).exit_status = 2 };
                if notification != 0 {
                    let _ =
                        microsystem_user_rt::notification_signal(CapHandle(notification as u32), 1);
                }
                microsystem_user_rt::exit(2);
            }
        }
    } else {
        inline_source
    };
    let source = match core::str::from_utf8(&source_bytes) {
        Ok(source) => source,
        Err(_) => {
            host.output.write(b"compile error: source is not UTF-8\n");
            unsafe { (*base.cast::<script::SessionHeaderV1>()).exit_status = 2 };
            microsystem_user_rt::exit(2);
        }
    };
    let declared = match PermissionSet::parse_manifest(source) {
        Ok(permissions) => permissions,
        Err(error) if header.mode == script::MODE_FILE => {
            host.output.write(b"compile error: ");
            host.output.write(error.as_bytes());
            host.output.write(b"\n");
            unsafe { (*base.cast::<script::SessionHeaderV1>()).exit_status = 2 };
            if notification != 0 {
                let _ = microsystem_user_rt::notification_signal(CapHandle(notification as u32), 1);
            }
            microsystem_user_rt::exit(2);
        }
        Err(_) => PermissionSet::default(),
    };
    host.permissions = if header.mode == script::MODE_FILE {
        declared.intersect(&launcher_policy)
    } else {
        launcher_policy
    };
    let gui_session = header.flags & script::FLAG_GUI_SESSION != 0;
    if gui_session {
        if header.mode != script::MODE_FILE
            || !host.permissions.allows_unscoped("gui", Access::Window)
        {
            host.output
                .write(b"runtime error: gui.window permission denied\n");
            unsafe { (*base.cast::<script::SessionHeaderV1>()).exit_status = 1 };
            microsystem_user_rt::exit(1);
        }
        host.gui = match gui::GuiHost::new(CapHandle(notification as u32)) {
            Ok(gui) => Some(gui),
            Err(_) => {
                host.output
                    .write(b"runtime error: GUI service unavailable\n");
                unsafe { (*base.cast::<script::SessionHeaderV1>()).exit_status = 1 };
                microsystem_user_rt::exit(1);
            }
        };
    }
    let compiled = if gui_session {
        compile_with_prelude(gui::PRELUDE, source)
    } else {
        compile(source)
    };
    let status = match compiled {
        Ok(chunk) => {
            let _ = microsystem_user_rt::debug_write(
                b"[mica] vm verified=true gc=mark-sweep pcall=true modules=true\n",
            );
            let limits = Limits {
                instructions: if header.instruction_limit == 0 {
                    microsystem_mica::DEFAULT_INSTRUCTION_LIMIT
                } else {
                    header.instruction_limit
                },
                timeout_ns: if header.timeout_ns == 0 {
                    microsystem_mica::DEFAULT_TIMEOUT_NS
                } else {
                    header.timeout_ns
                },
                ..Limits::default()
            };
            let vm = Vm::new(&chunk, host).with_limits(limits);
            let status = match vm.run() {
                Ok(_) => 0,
                Err(error) => {
                    write_vm_error(base, &error);
                    match error {
                        VmError::Timeout | VmError::InstructionLimit => 124,
                        VmError::Interrupted => 130,
                        _ => 1,
                    }
                }
            };
            let _ = microsystem_user_rt::debug_write(b"[mica] vm returned=true\n");
            status
        }
        Err(error) => {
            let mut output = SessionOutput { base };
            output.write(b"compile error: ");
            output.write(error.message.as_bytes());
            output.write(b"\n");
            2
        }
    };
    unsafe { (*base.cast::<script::SessionHeaderV1>()).exit_status = status };
    if notification != 0 {
        let _ = microsystem_user_rt::notification_signal(CapHandle(notification as u32), 1);
    }
    let _ = microsystem_user_rt::debug_write(b"[mica] exiting=true\n");
    microsystem_user_rt::exit(status as u64)
}

fn run_repl(
    mut host: KernelHost,
    base: *mut u8,
    notification: CapHandle,
    header: script::SessionHeaderV1,
) -> ! {
    host.output.write(b"Mica 0.1\n");
    unsafe {
        (*base.cast::<script::SessionHeaderV1>()).flags |= script::FLAG_OUTPUT_READY;
        core::arch::asm!("dmb ish", options(nostack, preserves_flags));
    }
    if notification == CapHandle::INVALID
        || microsystem_user_rt::notification_signal(notification, script::EVENT_OUTPUT).is_err()
    {
        microsystem_user_rt::exit(1);
    }
    loop {
        let bits = match microsystem_user_rt::notification_wait(notification, 0) {
            Ok(bits) => bits,
            Err(_) => microsystem_user_rt::exit(1),
        };
        if bits & script::EVENT_INPUT == 0 {
            continue;
        }
        unsafe { core::arch::asm!("dmb ish", options(nostack, preserves_flags)) };
        let session = unsafe { &mut *base.cast::<script::SessionHeaderV1>() };
        let bytes = (session.stdin_head as usize).min(script::STDIN_BYTES);
        let input = unsafe { core::slice::from_raw_parts(base.add(script::STDIN_OFFSET), bytes) };
        let source = match core::str::from_utf8(input) {
            Ok(source) => source.trim(),
            Err(_) => {
                host.output.write(b"compile error: input is not UTF-8\n");
                ""
            }
        };
        session.stdin_tail = session.stdin_head;
        if source == "exit" {
            session.exit_status = 0;
            session.flags |= script::FLAG_EXIT_READY;
            unsafe { core::arch::asm!("dmb ish", options(nostack, preserves_flags)) };
            let _ = microsystem_user_rt::notification_signal(notification, script::EVENT_EXIT);
            microsystem_user_rt::exit(0);
        }
        session.flags &= !script::FLAG_INTERRUPT;
        if !source.is_empty() {
            match compile(source) {
                Ok(chunk) => {
                    let limits = Limits {
                        instructions: if header.instruction_limit == 0 {
                            microsystem_mica::DEFAULT_INSTRUCTION_LIMIT
                        } else {
                            header.instruction_limit
                        },
                        timeout_ns: if header.timeout_ns == 0 {
                            5_000_000_000
                        } else {
                            header.timeout_ns
                        },
                        ..Limits::default()
                    };
                    match Vm::new(&chunk, &mut host).with_limits(limits).run() {
                        Ok(outcome) => {
                            for value in outcome.values {
                                host.output.write(value.display().as_bytes());
                                host.output.write(b"\n");
                            }
                        }
                        Err(error) => write_vm_error(base, &error),
                    }
                }
                Err(error) => {
                    host.output.write(b"compile error: ");
                    host.output.write(error.message.as_bytes());
                    host.output.write(b"\n");
                }
            }
        }
        unsafe {
            (*base.cast::<script::SessionHeaderV1>()).flags |= script::FLAG_OUTPUT_READY;
            core::arch::asm!("dmb ish", options(nostack, preserves_flags));
        }
        if microsystem_user_rt::notification_signal(notification, script::EVENT_OUTPUT).is_err() {
            microsystem_user_rt::exit(1);
        }
    }
}

fn write_vm_error(base: *mut u8, error: &VmError) {
    let mut output = SessionOutput { base };
    output.write(b"runtime error: ");
    match error {
        VmError::Runtime(error) => {
            if !error.operation.is_empty() {
                output.write(error.operation.as_bytes());
                output.write(b": ");
            }
            output.write(error.message.as_bytes());
        }
        VmError::InstructionLimit => output.write(b"instruction limit exceeded"),
        VmError::Timeout => output.write(b"execution timed out"),
        VmError::Interrupted => output.write(b"execution interrupted"),
        VmError::StackOverflow => output.write(b"operand stack limit exceeded"),
        VmError::CallDepth => output.write(b"call depth limit exceeded"),
        VmError::HeapLimit => output.write(b"VM heap limit exceeded"),
        VmError::InvalidBytecode => output.write(b"bytecode validation failed"),
    }
    output.write(b"\n");
}

fn integer_argument(arguments: &[Value], index: usize) -> Result<i64, ErrorValue> {
    match arguments.get(index) {
        Some(Value::Integer(value)) => Ok(*value),
        _ => Err(ErrorValue::new("type", "expected integer")),
    }
}

fn positive_argument(arguments: &[Value], index: usize) -> Result<i64, ErrorValue> {
    let value = integer_argument(arguments, index)?;
    (value > 0)
        .then_some(value)
        .ok_or_else(|| ErrorValue::new("argument", "expected positive integer"))
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

fn table_bool(value: Option<&Value>, key: &str) -> bool {
    value
        .and_then(|value| value.table_get(&Value::String(key.into())))
        .is_some_and(|value| value == &Value::Bool(true))
}

fn table_integer(value: &Value, key: &str) -> Result<i64, ErrorValue> {
    match value.table_get(&Value::String(key.into())) {
        Some(Value::Integer(value)) => Ok(*value),
        _ => Err(ErrorValue::new("type", "response field is not an integer")),
    }
}

fn table_integer_optional(value: &Value, key: &str) -> Result<Option<i64>, ErrorValue> {
    match value.table_get(&Value::String(key.into())) {
        Some(Value::Integer(value)) => Ok(Some(*value)),
        Some(Value::Nil) | None => Ok(None),
        _ => Err(ErrorValue::new("type", "response field is not an integer")),
    }
}

fn value_bytes(value: &Value) -> Result<&[u8], ErrorValue> {
    match value {
        Value::Bytes(bytes) => Ok(bytes),
        Value::String(text) => Ok(text.as_bytes()),
        Value::Nil => Ok(&[]),
        _ => Err(ErrorValue::new("type", "expected string or bytes")),
    }
}

struct HttpUrl {
    tls: bool,
    host: String,
    port: u16,
    path: String,
}

fn parse_http_url(url: &str) -> Result<HttpUrl, ErrorValue> {
    let (tls, remainder, default_port) = if let Some(value) = url.strip_prefix("http://") {
        (false, value, 80)
    } else if let Some(value) = url.strip_prefix("https://") {
        (true, value, 443)
    } else {
        return Err(ErrorValue::new("http", "URL must use http or https"));
    };
    let authority_end = remainder
        .find(|character| matches!(character, '/' | '?' | '#'))
        .unwrap_or(remainder.len());
    let authority = &remainder[..authority_end];
    let suffix = &remainder[authority_end..];
    if authority.is_empty() || authority.contains('@') {
        return Err(ErrorValue::new("http", "invalid URL authority"));
    }
    let (host, port) = match authority.rsplit_once(':') {
        Some((host, port)) if !host.is_empty() && !port.is_empty() => (
            host,
            port.parse::<u16>()
                .map_err(|_| ErrorValue::new("http", "invalid URL port"))?,
        ),
        _ => (authority, default_port),
    };
    if !valid_http_host(host) {
        return Err(ErrorValue::new("http", "invalid URL host"));
    }
    let mut normalized_path = String::from("/");
    if let Some(path) = suffix.strip_prefix('/') {
        normalized_path.push_str(path.split('#').next().unwrap_or(""));
    } else if suffix.starts_with('?') {
        normalized_path.push_str(suffix.split('#').next().unwrap_or(""));
    }
    Ok(HttpUrl {
        tls,
        host: host.to_ascii_lowercase(),
        port,
        path: normalized_path,
    })
}

fn valid_http_host(host: &str) -> bool {
    !host.is_empty()
        && host.len() <= 253
        && host.is_ascii()
        && !host.starts_with('.')
        && !host.ends_with('.')
        && host
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-'))
}

fn valid_header(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

fn append_decimal(output: &mut Vec<u8>, mut value: u64) {
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
    output.extend_from_slice(&digits[cursor..]);
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    (!needle.is_empty() && haystack.len() >= needle.len())
        .then(|| {
            haystack
                .windows(needle.len())
                .position(|window| window == needle)
        })
        .flatten()
}

fn decode_chunked(input: &[u8], maximum: usize) -> Result<Vec<u8>, ErrorValue> {
    let mut output = Vec::new();
    let mut cursor = 0usize;
    loop {
        let line_end = find_bytes(&input[cursor..], b"\r\n")
            .map(|offset| cursor + offset)
            .ok_or_else(|| ErrorValue::new("http", "truncated chunk header"))?;
        let line = core::str::from_utf8(&input[cursor..line_end])
            .map_err(|_| ErrorValue::new("http", "invalid chunk header"))?;
        let size_text = line.split(';').next().unwrap_or("");
        let size = usize::from_str_radix(size_text.trim(), 16)
            .map_err(|_| ErrorValue::new("http", "invalid chunk size"))?;
        cursor = line_end + 2;
        if size == 0 {
            return Ok(output);
        }
        let end = cursor
            .checked_add(size)
            .ok_or_else(|| ErrorValue::new("http", "chunk size overflow"))?;
        if end + 2 > input.len() || &input[end..end + 2] != b"\r\n" {
            return Err(ErrorValue::new("http", "truncated chunk body"));
        }
        if output.len().saturating_add(size) > maximum {
            return Err(ErrorValue::new("limit", "HTTP body exceeds read limit"));
        }
        output.extend_from_slice(&input[cursor..end]);
        cursor = end + 2;
    }
}

fn reply_status(value: u64) -> Result<(), ErrorValue> {
    let status = status_from_word(value);
    if status == Status::Ok {
        return Ok(());
    }
    Err(host_status("fs", status))
}

fn status_from_word(value: u64) -> Status {
    match value as i64 {
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
        -10 => Status::Io,
        -11 => Status::NoSpace,
        -12 => Status::Corrupt,
        _ => Status::Fault,
    }
}

fn parse_ipv4(input: &str) -> Option<u32> {
    let mut octets = [0u8; 4];
    let mut count = 0usize;
    for component in input.split('.') {
        if count == octets.len() || component.is_empty() {
            return None;
        }
        octets[count] = component.parse().ok()?;
        count += 1;
    }
    (count == 4).then_some(u32::from_be_bytes(octets))
}

fn ipv4_text(address: u32) -> String {
    let octets = address.to_be_bytes();
    alloc::format!("{}.{}.{}.{}", octets[0], octets[1], octets[2], octets[3])
}

fn error_value(error: ErrorValue) -> Value {
    Value::Table(alloc::vec![
        (Value::String("kind".into()), Value::String(error.kind)),
        (
            Value::String("message".into()),
            Value::String(error.message)
        ),
        (
            Value::String("operation".into()),
            Value::String(error.operation)
        ),
        (Value::String("code".into()), Value::Integer(error.code)),
    ])
}

fn system_pair(result: Result<Value, ErrorValue>) -> Result<Vec<Value>, ErrorValue> {
    Ok(match result {
        Ok(value) => alloc::vec![value, Value::Nil],
        Err(error) => alloc::vec![Value::Nil, error_value(error)],
    })
}

fn host_status(operation: &str, status: microsystem_abi::Status) -> ErrorValue {
    let (kind, message) = match status {
        Status::Invalid => ("argument", "system service rejected an invalid request"),
        Status::BadCapability => ("capability", "system capability is unavailable"),
        Status::AccessDenied => ("access", "system service denied access"),
        Status::NotFound => ("not_found", "system resource was not found"),
        Status::NoMemory => ("memory", "system resource limit was reached"),
        Status::Busy => ("busy", "system resource is busy"),
        Status::TimedOut => ("timeout", "system operation timed out"),
        Status::Fault => ("fault", "system operation faulted"),
        Status::NotSupported => ("not_supported", "system operation is not supported"),
        Status::Io => ("io", "system I/O operation failed"),
        Status::NoSpace => ("space", "system storage is full"),
        Status::Corrupt => ("corrupt", "system data is corrupt"),
        Status::Ok => ("system", "system service returned an invalid success error"),
    };
    ErrorValue::new(kind, message)
        .operation(operation)
        .code(status as i64)
}

fn system_stats_value(stats: &SystemStats) -> Value {
    let entries = [
        ("cpu0_ticks", stats.cpu_ticks[0]),
        ("cpu1_ticks", stats.cpu_ticks[1]),
        ("free_frames", stats.free_frames),
        ("kernel_heap_used", stats.kernel_heap_used),
        ("kernel_heap_total", stats.kernel_heap_total),
        ("runnable", stats.runnable_threads as u64),
        ("blocked", stats.blocked_threads as u64),
        ("irq", stats.irq_count),
        ("ipc", stats.ipc_calls),
    ];
    Value::Table(
        entries
            .into_iter()
            .map(|(key, value)| (Value::String(key.into()), Value::Integer(value as i64)))
            .collect(),
    )
}

fn process_info_value(info: &ProcessInfoV2) -> Value {
    Value::Table(alloc::vec![
        (Value::String("pid".into()), Value::Integer(info.pid as i64)),
        (
            Value::String("running".into()),
            Value::Bool(info.running != 0)
        ),
        (
            Value::String("status".into()),
            Value::Integer(info.exit_status)
        ),
        (
            Value::String("program".into()),
            Value::Integer(info.program as i64)
        ),
        (
            Value::String("cpu0_ticks".into()),
            Value::Integer(info.cpu_ticks[0] as i64)
        ),
        (
            Value::String("cpu1_ticks".into()),
            Value::Integer(info.cpu_ticks[1] as i64)
        ),
        (
            Value::String("cpu_mask".into()),
            Value::Integer(info.cpu_mask as i64)
        ),
        (
            Value::String("owned_pages".into()),
            Value::Integer(info.owned_pages as i64)
        ),
    ])
}

fn program_id(name: &str) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in name.as_bytes() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x100_0000_01b3);
    }
    hash
}

#[panic_handler]
fn panic(_info: &PanicInfo<'_>) -> ! {
    microsystem_user_rt::exit(1)
}
