use std::{cell::RefCell, rc::Rc};

use microsystem_mica::{
    Access, ErrorValue, Host, Limits, Op, Permission, PermissionSet, Resource, Value, Vm, VmError,
    YIELD_INTERVAL, compile, compile_with_prelude, json, stdlib,
};

#[derive(Default)]
struct TestHost {
    now: u64,
    yields: usize,
    interrupted: bool,
    print_calls: Rc<RefCell<Vec<Vec<Value>>>>,
    callback: Option<Value>,
}

impl Host for TestHost {
    fn call(&mut self, name: &str, arguments: &[Value]) -> Result<Vec<Value>, ErrorValue> {
        if name == "print" {
            self.print_calls.borrow_mut().push(arguments.to_vec());
            return Ok(Vec::new());
        }
        if name == "echo" {
            return match arguments {
                [Value::Bytes(bytes)] => Ok(vec![Value::Bytes(bytes.clone())]),
                _ => Err(ErrorValue::new("type", "echo expects bytes")),
            };
        }
        if name == "register_callback" {
            self.callback = arguments.first().cloned();
            return Ok(vec![Value::Nil]);
        }
        if name == "get_callback" {
            return Ok(vec![self.callback.clone().unwrap_or(Value::Nil)]);
        }
        stdlib::call(name, arguments)
            .unwrap_or_else(|| Err(ErrorValue::new("name", "unknown host function")))
    }

    fn now_ns(&mut self) -> u64 {
        let now = self.now;
        self.now = self.now.saturating_add(1);
        now
    }

    fn yield_now(&mut self) {
        self.yields += 1;
    }

    fn interrupted(&mut self) -> bool {
        self.interrupted
    }

    fn gc_roots(&self) -> Vec<Value> {
        self.callback.iter().cloned().collect()
    }
}

fn run(source: &str) -> Result<microsystem_mica::VmOutcome, VmError> {
    Vm::new(
        &compile(source).expect("source compiles"),
        TestHost::default(),
    )
    .run()
}

fn gui_prelude() -> &'static str {
    let service_source = include_str!("../../../services/mica/src/gui.rs");
    let prelude_start = service_source
        .find("pub const PRELUDE: &str = r#\"\n")
        .expect("GUI prelude declaration")
        + "pub const PRELUDE: &str = r#\"\n".len();
    let prelude_end = service_source[prelude_start..]
        .find("\n\"#;")
        .map(|offset| prelude_start + offset)
        .expect("GUI prelude terminator");
    &service_source[prelude_start..prelude_end]
}

#[test]
fn print_call_completes_and_preserves_following_statement() {
    let print_calls = Rc::new(RefCell::new(Vec::new()));
    let host = TestHost {
        print_calls: print_calls.clone(),
        ..TestHost::default()
    };
    let chunk = compile("print(40 + 2); return 7").expect("source compiles");
    let outcome = Vm::new(&chunk, host).run().expect("VM completes");

    assert_eq!(outcome.values, vec![Value::Integer(7)]);
    assert_eq!(print_calls.borrow().as_slice(), &[vec![Value::Integer(42)]]);
}

#[test]
fn host_echo_round_trips_bytes_through_stdlib_conversion() {
    let outcome = run(r#"
        local bytes = require("bytes")
        local payload = bytes.from_string("mica-tcp-ok")
        local result = echo(payload)
        return bytes.to_string(result)
        "#)
    .expect("echo script completes");
    assert_eq!(outcome.values, vec![Value::String("mica-tcp-ok".into())]);
}

#[test]
fn host_echo_accepts_nested_bytes_conversion() {
    let outcome = run(r#"
        local bytes = require("bytes")
        return bytes.to_string(echo(bytes.from_string("mica-nested")))
        "#)
    .expect("nested echo script completes");
    assert_eq!(outcome.values, vec![Value::String("mica-nested".into())]);
}

#[test]
fn host_gc_roots_keep_callback_alive_across_yield_collection() {
    let chunk = compile(
        r#"
        register_callback(function()
            return 42
        end)
        local i = 0
        while i < 2048 do
            i = i + 1
        end
        local callback = get_callback()
        return callback()
        "#,
    )
    .expect("callback script compiles");
    let outcome = Vm::new(&chunk, TestHost::default())
        .with_limits(Limits {
            instructions: 30_000,
            timeout_ns: u64::MAX,
            ..Limits::default()
        })
        .run()
        .expect("host callback survives collection");

    assert!(outcome.instructions >= YIELD_INTERVAL);
    assert_eq!(outcome.values, vec![Value::Integer(42)]);
}

#[test]
fn compile_and_run_arithmetic_locals_control_flow_and_tables() {
    let chunk = compile("local answer = 40 + 2\nreturn answer").expect("source compiles");
    assert!(!chunk.code.is_empty());
    assert!(chunk.constants.contains(&Value::Integer(40)));
    assert!(chunk.constants.contains(&Value::Integer(2)));

    let outcome = run(r#"
        local total = 0
        local i = 0
        while i < 3 do
            i = i + 1
            total = total + i
        end
        for j = 1, 4 do
            total = total + j
        end
        local values = {name = "mica", [2] = total}
        if total == 16 then
            return total, values.name, values[2]
        end
        return 0
        "#)
    .expect("VM completes");
    assert_eq!(
        outcome.values,
        vec![
            Value::Integer(16),
            Value::String("mica".into()),
            Value::Integer(16),
        ]
    );
}

#[test]
fn functions_capture_locals_and_preserve_multiple_returns() {
    let outcome = run(r#"
        local bias = 3
        function pair(value)
            return value + bias, value * 2
        end
        local first, second = pair(4)
        return first, second
        "#)
    .expect("VM completes");
    assert_eq!(outcome.values, vec![Value::Integer(7), Value::Integer(8)]);
}

#[test]
fn closure_state_persists_across_repeated_calls() {
    let outcome = run(r#"
        local make_counter = function()
            local count = 0
            return function(step)
                count = count + step
                return count
            end
        end
        local next = make_counter()
        local first = next(1)
        local second = next(2)
        local third = next(3)
        return first, second, third
        "#)
    .expect("closure script completes");
    assert_eq!(
        outcome.values,
        vec![Value::Integer(1), Value::Integer(3), Value::Integer(6)]
    );
}

#[test]
fn boolean_operators_short_circuit_rhs_evaluation() {
    let outcome = run(r#"
        local left = false and error("and rhs must not run")
        local right = true or error("or rhs must not run")
        return left, right
        "#)
    .expect("short-circuit script completes");
    assert_eq!(outcome.values, vec![Value::Bool(false), Value::Bool(true)]);
}

#[test]
fn pcall_captures_script_errors_but_not_budget_or_timeout() {
    let outcome = run(r#"
        local ok, error = pcall(function()
            error("caught")
        end)
        return ok, error.kind, error.message
        "#)
    .expect("script errors are protected");
    assert_eq!(
        outcome.values,
        vec![
            Value::Bool(false),
            Value::String("script".into()),
            Value::String("caught".into()),
        ]
    );

    let chunk =
        compile("local ok, error = pcall(function() while true do end end); return ok, error")
            .expect("budget script compiles");
    assert_eq!(
        Vm::new(&chunk, TestHost::default())
            .with_limits(Limits {
                instructions: 32,
                ..Limits::default()
            })
            .run(),
        Err(VmError::InstructionLimit)
    );
    assert_eq!(
        Vm::new(&chunk, TestHost::default())
            .with_limits(Limits {
                timeout_ns: 2,
                instructions: u64::MAX,
                ..Limits::default()
            })
            .run(),
        Err(VmError::Timeout)
    );
}

#[test]
fn iterator_and_negative_numeric_for_preserve_order_and_direction() {
    let outcome = run(r#"
        local table = {first = 2, second = 3}
        local first_key = ""
        local second_key = ""
        local seen = 0
        local total = 0
        for key, value in table do
            seen = seen + 1
            if seen == 1 then
                first_key = key
            elseif seen == 2 then
                second_key = key
            end
            total = total + value
        end
        local descending = 0
        for value = 5, 1, -2 do
            descending = descending * 10 + value
        end
        return first_key, second_key, seen, total, descending
        "#)
    .expect("loop forms execute");
    assert_eq!(
        outcome.values,
        vec![
            Value::String("first".into()),
            Value::String("second".into()),
            Value::Integer(2),
            Value::Integer(5),
            Value::Integer(531),
        ]
    );
}

#[test]
fn reachable_closure_survives_mark_sweep_while_transients_are_reallocated() {
    let chunk = compile(
        r#"
        local seed = 41
        local keep = function()
            return seed + 1
        end
        function make(value)
            return {value = value}
        end
        local i = 1
        while i <= 2000 do
            local transient = make(i)
            i = i + 1
        end
        return keep()
        "#,
    )
    .expect("GC script compiles");
    let outcome = Vm::new(&chunk, TestHost::default())
        .with_limits(Limits {
            instructions: 200_000,
            heap_bytes: 4096,
            ..Limits::default()
        })
        .run()
        .expect("mark-sweep reclaims transient values");
    assert_eq!(outcome.values, vec![Value::Integer(42)]);
}

#[test]
fn malformed_bytecode_indices_and_jumps_are_rejected() {
    let mut bad_constant = compile("return 1").expect("source compiles");
    bad_constant.code[0] = Op::Constant(u16::MAX);
    assert_eq!(
        Vm::new(&bad_constant, TestHost::default()).run(),
        Err(VmError::InvalidBytecode)
    );

    let mut bad_jump = compile("return 1").expect("source compiles");
    bad_jump.code[0] = Op::Jump(usize::MAX);
    assert_eq!(
        Vm::new(&bad_jump, TestHost::default()).run(),
        Err(VmError::InvalidBytecode)
    );
}

#[test]
fn undeclared_assignment_is_rejected_during_compilation() {
    let error = compile("missing = 1").expect_err("assignment must be declared");
    assert_eq!(error.line, 1);
    assert_eq!(error.message, "assignment to undeclared variable");
}

#[test]
fn host_stdlib_calls_cover_math_and_encoding() {
    let outcome = run(r#"
        local math = require("math")
        return math.abs(-3), math.floor(3.9), math.max(4, 7)
        "#)
    .expect("VM completes");
    assert_eq!(
        outcome.values,
        vec![Value::Integer(3), Value::Float(3.0), Value::Float(7.0),]
    );

    let input = Value::Bytes(b"mica".to_vec());
    let encoded = stdlib::call("encoding.base64_encode", &[input.clone()])
        .expect("builtin exists")
        .expect("encoding succeeds");
    assert_eq!(encoded, vec![Value::String("bWljYQ==".into())]);
    let decoded = stdlib::call("encoding.base64_decode", &encoded)
        .expect("builtin exists")
        .expect("decoding succeeds");
    assert_eq!(decoded, vec![input]);
}

#[test]
fn host_stdlib_string_helpers_cover_reader_text_operations() {
    let outcome = run(r#"
        local string = require("string")
        local padded = "  héllo   世界  "
        local trimmed = string.trim(padded)
        local collapsed = string.collapse_space(padded)
        local found = string.find(trimmed, "世界")
        local shifted = string.find("abcabc", "a", 2)
        local missing = string.find(trimmed, "missing")
        local replaced = string.replace("a &amp; b", "&amp;", "&")
        return trimmed, collapsed, found, shifted, missing,
            string.starts_with(trimmed, "hé"),
            string.ends_with(trimmed, "世界"), replaced
        "#)
    .expect("string helper script completes");
    assert_eq!(
        outcome.values,
        vec![
            Value::String("héllo   世界".into()),
            Value::String("héllo 世界".into()),
            Value::Integer(9),
            Value::Integer(4),
            Value::Nil,
            Value::Bool(true),
            Value::Bool(true),
            Value::String("a & b".into()),
        ]
    );
}

#[test]
fn instruction_and_clock_limits_stop_non_terminating_scripts() {
    let chunk = compile("while true do end").expect("source compiles");
    let instruction_limited = Vm::new(&chunk, TestHost::default()).with_limits(Limits {
        instructions: 8,
        ..Limits::default()
    });
    assert_eq!(instruction_limited.run(), Err(VmError::InstructionLimit));

    let timeout_limited = Vm::new(&chunk, TestHost::default()).with_limits(Limits {
        timeout_ns: 2,
        instructions: u64::MAX,
        ..Limits::default()
    });
    assert_eq!(timeout_limited.run(), Err(VmError::Timeout));
}

#[test]
fn permission_paths_hosts_and_three_way_intersection_are_narrowed() {
    let parsed = Permission::parse("fs.read:/data/./app/../shared").expect("path rule parses");
    assert_eq!(parsed.resource, Resource::Path("/data/shared".to_string()));
    assert!(Permission::parse("fs.read:relative").is_err());
    assert!(Permission::parse("fs.read:/../../escape").is_err());
    assert!(Permission::parse("fs.read:/bad\0path").is_err());

    let parent =
        PermissionSet::from_rules(["fs.read:/data", "net.connect:EXAMPLE.com:443", "proc.list"])
            .expect("parent rules parse");
    let child = PermissionSet::from_rules([
        "fs.read:/data/app",
        "net.connect:example.com:443",
        "proc.list",
    ])
    .expect("child rules parse");
    let grant =
        PermissionSet::from_rules(["fs.read:/data/app/cache", "net.connect:example.com:443"])
            .expect("grant rules parse");
    let effective = grant.intersect(&child).intersect(&parent);

    assert!(effective.allows_path(Access::Read, "/data/app/cache/file"));
    assert!(!effective.allows_path(Access::Read, "/data/app/cache-other"));
    assert!(effective.allows_host("Example.COM", 443));
    assert!(!effective.allows_host("example.com", 8443));
    assert!(!effective.allows_unscoped("proc", Access::List));
}

#[test]
fn browse_permission_is_unscoped_and_never_grants_raw_network_access() {
    let browse = Permission::parse("net.browse").expect("browse rule parses");
    assert_eq!(browse.resource, Resource::Unscoped);
    assert_eq!(browse.rule(), "net.browse");
    assert!(Permission::parse("net.browse:example.com:80").is_err());

    let declared = PermissionSet::from_rules(["net.browse"]).expect("manifest parses");
    let launcher = PermissionSet::from_rules(["net.browse"]).expect("launcher parses");
    let effective = declared.intersect(&launcher);
    assert!(effective.allows_unscoped("net", Access::Browse));
    assert!(!effective.allows_host("example.com", 80));
    assert!(!effective.allows_host_any_port("example.com"));

    let exact_connect = PermissionSet::from_rules(["net.connect:example.com:80"])
        .expect("exact network rule parses");
    assert!(
        !declared
            .intersect(&exact_connect)
            .allows_unscoped("net", Access::Browse)
    );
}

#[test]
fn gui_window_permission_requires_an_exact_manifest_rule_and_policy_grant() {
    let declared = PermissionSet::parse_manifest("--!mica 1\n--!allow gui.window\nlocal app = 1\n")
        .expect("GUI manifest rule parses");
    assert!(declared.allows_unscoped("gui", Access::Window));

    let launcher_without_gui =
        PermissionSet::from_rules(["sys.stats"]).expect("launcher policy parses");
    assert!(
        !declared
            .intersect(&launcher_without_gui)
            .allows_unscoped("gui", Access::Window)
    );

    let launcher_gui =
        PermissionSet::from_rules(["gui.window"]).expect("GUI launcher policy parses");
    let entry_policy = PermissionSet::from_rules(["gui.window"]).expect("GUI entry policy parses");
    assert!(
        declared
            .intersect(&launcher_gui)
            .intersect(&entry_policy)
            .allows_unscoped("gui", Access::Window)
    );

    assert!(Permission::parse("gui.window:extra").is_err());
    assert_eq!(
        Permission::parse("gui.window").unwrap().rule(),
        "gui.window"
    );
}

#[test]
fn service_gui_prelude_compiles_widget_tree_and_event_callbacks() {
    // Keep this test coupled to the service's actual prelude instead of a
    // second test-only copy: a change that leaves the compiler/API shape
    // incompatible with the running GUI service must fail here.
    let prelude = gui_prelude();
    let source = r#"
        local gui = require("gui")
        local count = 0
        local label = gui.label {text = "Count: 0"}
        local app, err = gui.window {title = "Counter 界", width = 420, height = 260}
        if not app then error(err.message) end
        app:set_root(gui.column {
            padding = 16,
            gap = 12,
            children = {
                label,
                gui.button {
                    text = "Increment",
                    on_click = function(event)
                        count = count + 1
                        label:set_text("Count: " + tostring(count))
                        app:invalidate()
                    end,
                },
                gui.text_input {
                    placeholder = "Type here",
                    on_submit = function(text)
                        label:set_text(text)
                        app:invalidate()
                    end,
                },
            },
        })
        return app:run()
    "#;
    let chunk = compile_with_prelude(prelude, source).expect("GUI script compiles");
    assert!(!chunk.code.is_empty());
}

#[test]
fn service_gui_prelude_supports_set_callback_and_browser_asset_compiles() {
    let prelude = gui_prelude();
    let source = r#"
        local gui = require("gui")
        local button = gui.button {text = "Open"}
        local list = gui.list {items = {"one", "two"}}
        button:set_callback("click", function(event) return event.kind end)
        button:set_callback("click", nil)
        list:set_callback("select", function(index) return index end)
        local app, err = gui.window {title = "Callbacks", width = 320, height = 220}
        if not app then error(err.message) end
        app:set_root(gui.column {children = {button, list}})
        app:present()
        return true
    "#;
    let chunk = compile_with_prelude(prelude, source).expect("set_callback script compiles");
    assert!(!chunk.code.is_empty());

    let browser_source = include_str!("../../../assets/mica/browser.mica");
    for marker in [
        "--!allow gui.window",
        "--!allow net.browse",
        "response.content_type",
        "response.location",
        "if scheme ~= \"http\" and scheme ~= \"https\"",
        "local query = string.find(url, \"?\", authority_start)",
        "local hash = string.find(url, \"#\", authority_start)",
        "if target_parts.origin ~= base_parts.origin then",
        "resolve_url",
        "set_callback",
        "app:present()",
        "status:set_text(\"Loading \" + parsed.origin)",
        "local source_length = string.len(source)",
    ] {
        assert!(
            browser_source.contains(marker),
            "browser asset must retain public contract marker {marker:?}"
        );
    }
    let browser_chunk = compile_with_prelude(prelude, browser_source)
        .expect("browser asset compiles against the service GUI prelude");
    assert!(!browser_chunk.code.is_empty());
}

#[test]
fn service_gui_prelude_compiles_editor_asset_and_preserves_file_contract() {
    let prelude = gui_prelude();
    let editor_source = include_str!("../../../assets/mica/editor.mica");
    for marker in [
        "--!mica 1",
        "--!allow gui.window",
        "--!allow fs.read:/data",
        "--!allow fs.write:/data",
        "local MAX_FILE_BYTES = 32512",
        "local MAX_LINES = 32",
        "local DEFAULT_PATH = \"/data/note.txt\"",
        "string.replace(text, \"\\r\\n\", \"\\n\")",
        "string.replace(text, \"\\r\", \"\\n\")",
        "fs.read_file(normalized, MAX_FILE_BYTES)",
        "fs.write_file(normalized, data, {atomic = true, fsync = true})",
        "path must be a file below /data",
        "Insert failed: document already has 32 lines",
        "mica-editor loaded path=",
        "mica-editor saved path=",
        "atomic=true fsync=true",
        "document:set_callback(\"select\"",
        "line_input:set_callback(\"submit\"",
        "save_as_button:set_callback(\"click\"",
    ] {
        assert!(
            editor_source.contains(marker),
            "editor asset must retain public contract marker {marker:?}"
        );
    }
    let editor_chunk = compile_with_prelude(prelude, editor_source)
        .expect("editor asset compiles against the service GUI prelude");
    assert!(!editor_chunk.code.is_empty());
}

#[test]
fn json_round_trips_and_rejects_duplicate_or_deep_objects() {
    let value = json::decode(r#"{"name":"mica","items":[1,true,null]}"#).expect("JSON decodes");
    assert_eq!(
        json::encode(&value).expect("JSON encodes"),
        r#"{"name":"mica","items":[1,true,null]}"#
    );
    assert!(json::decode(r#"{"name":1,"name":2}"#).is_err());

    let mut deeply_nested = String::new();
    for _ in 0..65 {
        deeply_nested.push('[');
    }
    deeply_nested.push('0');
    for _ in 0..65 {
        deeply_nested.push(']');
    }
    assert!(json::decode(&deeply_nested).is_err());
}

#[test]
fn encoding_utf8_sha256_and_hmac_have_stable_vectors() {
    let bytes = Value::Bytes(vec![0, 0xab, 0xff]);
    let hex = stdlib::call("encoding.hex_encode", &[bytes.clone()])
        .expect("builtin exists")
        .expect("hex encoding succeeds");
    assert_eq!(hex, vec![Value::String("00abff".into())]);
    assert_eq!(
        stdlib::call("encoding.hex_decode", &hex)
            .expect("builtin exists")
            .expect("hex decoding succeeds"),
        vec![bytes]
    );
    assert_eq!(
        stdlib::call("encoding.utf8_valid", &[Value::Bytes(vec![0xff])])
            .expect("builtin exists")
            .expect("UTF-8 check succeeds"),
        vec![Value::Bool(false)]
    );

    let sha = stdlib::call("hash.sha256", &[Value::Bytes(b"abc".to_vec())])
        .expect("builtin exists")
        .expect("SHA-256 succeeds");
    assert_eq!(
        sha,
        vec![Value::Bytes(hex_bytes(
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        ))]
    );
    let hmac = stdlib::call(
        "hash.hmac_sha256",
        &[
            Value::Bytes(b"key".to_vec()),
            Value::Bytes(b"The quick brown fox jumps over the lazy dog".to_vec()),
        ],
    )
    .expect("builtin exists")
    .expect("HMAC succeeds");
    assert_eq!(
        hmac,
        vec![Value::Bytes(hex_bytes(
            "f7bc83f430538424b13298e6aa6fb143ef4d59a14946175997479dbc2d1a3cd8"
        ))]
    );
}

fn hex_bytes(value: &str) -> Vec<u8> {
    value
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let high = (pair[0] as char).to_digit(16).expect("hex") as u8;
            let low = (pair[1] as char).to_digit(16).expect("hex") as u8;
            high << 4 | low
        })
        .collect()
}
