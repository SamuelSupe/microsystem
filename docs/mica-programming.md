# Mica Programming Guide

[简体中文](mica-programming.zh-CN.md) · **English**

Mica is the small scripting language shipped with MicroSystem. It runs as a
capability-brokered EL0 application: a script asks the filesystem, network,
process, time, or GUI broker for an operation, and the broker checks the
session policy before performing it.

This guide is a practical starting point. The complete runtime contract and
wire-level limits are in [Mica runtime](mica.md); runnable source examples are
under [`assets/mica/`](../assets/mica/).

## 1. Your first script

Every file script starts with the version marker:

```mica
--!mica 1

print("hello from Mica")
print("uptime_ns", time.uptime())
```

Run a small expression in the serial shell:

```text
mica -e 'print(40 + 2)'
```

Start the interactive REPL:

```text
mica
mica> print(6 * 7)
mica> exit
```

Run a file already present in the guest MFS. Arguments follow `--`:

```text
mica --allow fs.read:/data /data/example.mica -- first second
```

The file command needs an existing MFS path. `run mica` is intentionally not
supported: Mica sessions are created through the script broker and
`ThreadStartEx`, not through the ordinary static application loader.

## 2. Language basics

Mica supports `nil`, booleans, signed decimal and hexadecimal integers,
floating-point numbers, UTF-8 strings, bytes, tables, and functions. It has
local variables, assignment, multiple return values, lexical closures,
`if`/`elseif`/`else`, `while`, numeric/table `for`, `break`, `return`, indexing,
field access, method calls, and semicolons.

```mica
--!mica 1

local describe = function(name, score)
    if score >= 60 then
        return name + " passed", true
    else
        return name + " needs practice", false
    end
end

local message, passed = describe("Mica", 96)
print(message, passed)

local values = {2, 4, 6}
local total = 0
for index = 1, table.len(values) do
    total = total + values[index]
end
print("total", total)
```

Operators include arithmetic (`+ - * / // %`), comparisons, equality,
short-circuit `and`/`or`, and `not`. The examples use `+` for both numeric
addition and string concatenation.

The built-in runtime functions are `type`, `tostring`, `assert`, `error`,
`pcall`, and `require`.

## 3. Modules and standard library

Use `require` to load a built-in module or a UTF-8 `.mica` module from MFS:

```mica
--!mica 1

local json = require("json")
local status = {
    state = "ready",
    value = 42,
}

print(json.encode(status))
```

The built-in modules are:

| Module | Main functions |
| --- | --- |
| `json` | `encode`, `decode` |
| `encoding` | hexadecimal, Base64, and UTF-8 validation |
| `hash` | `sha256`, `hmac_sha256` |
| `string` | length, case, slicing, search, trim, replacement, whitespace collapse |
| `bytes` | convert to/from strings and inspect length |
| `table` | `len`, `insert`, `remove`, `next` |
| `math` | `abs`, `floor`, `ceil`, `min`, `max` |
| `fs` | file and directory operations |
| `proc` | policy-controlled list, spawn, wait, and kill |
| `time` | `uptime`, `sleep`, `realtime` |
| `sys` | `version`, `stats` |
| `random` | `bytes`, `int` |
| `net` | DNS, TCP, UDP, and cancellation primitives |
| `http` | `request`, `get`, `post`, `put`, `patch`, `delete` |
| `io` | standard input/output operations |
| `args` | `get`, `all` |

Modules are cached per session. A module is loaded as Mica source from MFS,
not as a dynamically linked native library. Keep modules small: one module is
limited to 64 KiB and total module source per session is limited to 256 KiB.

## 4. Permissions are part of the program

File scripts declare permissions after `--!mica 1` and before the first
non-comment statement:

```mica
--!mica 1
--!allow fs.read:/data
--!allow fs.write:/data/output
--!allow net.connect:10.0.2.2:8080

print("this script has explicit capabilities")
```

Common rules include:

```text
fs.read:/data                 path-prefix read access
fs.write:/data/output        path-prefix write access
net.connect:example.test:443 exact host and port
net.browse                   bundled Reader HTTP/HTTPS GET path
proc.list                    process listing
proc.spawn:counter           named application launch
proc.kill:counter            named application control
sys.stats                    system statistics
random                       random source
gui.window                   up to four retained GUI windows
```

`fs.read` and `fs.write` are absolute path-prefix permissions. `net.connect`
is an exact, case-insensitive host/port rule; wildcards are not accepted.
Invalid paths, `..` escapes, NULs, wildcards, and malformed rules are rejected.

For a file session, the effective policy is the intersection of the file
manifest, launcher `--allow` rules, and the fixed entry-point policy. A script
cannot grant itself a capability by adding a declaration. `net.browse` is a
special permission for the bundled Reader's GET path; it does not grant raw
DNS/TCP/UDP calls or POST/PUT/PATCH/DELETE.

## 5. Files and structured errors

Broker calls return a value on success and usually `nil, error` on failure.
Errors are tables with `kind`, `message`, `operation`, and numeric `code`
fields. Check the first return value before using it:

```mica
--!mica 1
--!allow fs.read:/data
--!allow fs.write:/data/output

local json = require("json")
local status = {
    generated_at = time.uptime(),
    source = "/data",
}

local ok, err = fs.write_file(
    "/data/output/status.json",
    json.encode(status),
    {atomic = true, fsync = true}
)
if not ok then
    error(err.message)
end
```

Useful filesystem calls include `stat`, `list`, `open`, `read`, `write`,
`close`, `read_file`, `write_file`, `mkdir`, `rename`, `unlink`, `fsync`, and
`sync`. `read_file` defaults to 64 KiB and accepts at most 256 KiB. For durable
configuration or user data, prefer `write_file(..., {atomic = true, fsync =
true})` and handle the returned error.

Use `pcall` when a script wants to convert a script/runtime failure into a
normal result:

```mica
local ok, value = pcall(function()
    return assert(false, "demonstration failure")
end)
if not ok then
    print("caught", value)
end
```

`pcall` does not make resource limits disappear. Instruction, timeout,
interrupt, stack, call-depth, and heap failures remain VM outcomes.

## 6. HTTP and networking

Networking is permission-gated. A direct HTTP request to a fixed endpoint can
use an exact `net.connect` rule:

```mica
--!mica 1
--!allow net.connect:10.0.2.2:8080

local http = require("http")
local response, err = http.get("http://10.0.2.2:8080/status")
if not response then
    error(err.message)
end

local body, read_err = response:read_all(1048576)
if not body then
    error(read_err.message)
end
print(response.status, body)
```

`http` supports HTTP and HTTPS, caller headers, `Content-Length`, chunked
responses, and the methods exposed in the standard-library table. Responses
are buffered; this is not a streaming API. Redirects are returned as-is and
are not followed. Low-level `net.tls_connect` is deliberately
`NotSupported`; the trusted HTTPS path lives inside the Mica runtime.

The bundled Reader uses `net.browse` for a bounded HTTP/HTTPS GET to a host
selected at runtime, and follows up to eight HTTP(S) redirects in the Reader
UI. Redirects may change host, while links within a page remain same-origin.
The Reader also has `fs.write:/data` and can atomically save up to 32,512
response bytes there after an explicit Save click; it has no file-read grant.
Raw `net.resolve`, TCP, UDP, or non-GET operations still need their exact
permissions. The raw network device belongs only to `netd`; Mica receives
broker access, never a network device or MMIO capability.

## 7. GUI applications

GUI execution is file-only. `-e` and the REPL cannot create a GUI session. A
GUI file must declare `gui.window` and be launched with `--gui`:

```text
mica --gui --timeout 86400s --allow gui.window /mica/gui-counter.mica
```

The smallest useful GUI program is:

```mica
--!mica 1
--!allow gui.window

local gui = require("gui")
local count = 0
local label = gui.label {text = "Count: 0"}
local app, err = gui.window {
    title = "Mica Counter",
    width = 420,
    height = 260,
}
if not app then error(err.message) end

app:set_root(gui.column {
    padding = 16,
    gap = 12,
    children = {
        label,
        gui.button {
            text = "Increment",
            on_click = function()
                count = count + 1
                label:set_text("Count: " + tostring(count))
                app:invalidate()
            end,
        },
    },
})

app:run()
```

The retained toolkit includes `window`, `label`, `button`, `text_input`,
`checkbox`, `list`, `scroll`, `row`, `column`, `spacer`, and `canvas`. Window
methods include `set_root`, `set_title`, `invalidate`, `present`, `run`, and
`close`. Widgets expose setters such as `set_text`, `set_checked`, and
`set_items`, plus `on_click`, `on_change`, `on_submit`, and `on_select`
callbacks.

Focused `text_input` widgets support Left/Right and Home/End caret movement,
Backspace/Delete by UTF-8 character, and Ctrl+A to select the entire field.
Typing replaces a whole-field selection. Clicking focuses the field and places
the caret at the end; Tab moves between interactive widgets and Enter submits.
Ctrl+C/X/V copies/cuts/pastes a Ctrl+A selection. `gui.clipboard.read()` and
`gui.clipboard.write(text)` use the shared 4 KiB UTF-8 store. Ctrl+Space toggles
basic Pinyin composition with a preedit/candidate overlay; Space/Enter commits,
1–8 selects, Backspace edits and Escape cancels. The editable TSV dictionary
and its limits are documented in `docs/gui.md`. Partial selection and
pointer-positioned caret placement are not implemented.

One script owns up to four top-level windows. Widgets belong to one window;
closing it releases its widgets. `window:run()` dispatches callbacks across all
windows and returns when the named window closes. The GUI task receives validated command,
event, and endpoint capabilities only; it never receives the framebuffer,
GPU, input device, DMA, BAR, or IRQ capability.

## 8. Arguments, time, and limits

Pass arguments after `--` and read them with `args`:

```mica
--!mica 1

local args = require("args")
print("first argument", args.get(1))
```

Useful runtime calls are `time.uptime()`, `time.sleep(milliseconds)`,
`time.realtime()`, `sys.version()`, and `sys.stats()` when the session policy
allows them.

Important limits include:

| Resource | Limit |
| --- | ---: |
| Source passed to `compile` | 64 KiB |
| Generated bytecode | 128 KiB |
| Call frames | 128 |
| Operand stack | 4,096 values |
| VM heap accounting | 512 KiB by default |
| Instructions | 10,000,000 by default |
| File-script wall time | 10 s default |
| REPL wall time | 5 s default |

Non-GUI sessions are hard-capped at 60 seconds. GUI file sessions may request
up to 24 hours, but they remain subject to cleanup and resource limits.

## 9. A practical workflow

1. Start with `--!mica 1` and one observable `print`.
2. Add only the `--!allow` rules the script needs.
3. Use `require` for standard modules and check every broker error.
4. Bound file reads, network bodies, table sizes, and loop work explicitly.
5. Use atomic writes plus `fsync` for durable state.
6. Keep GUI code event-driven and call `app:invalidate()` after state changes.
7. Run the script through the serial shell, SSH, or the GUI launcher that
   supplies its policy; do not use `run mica`.

For the complete contract, see [Mica runtime](mica.md). For larger examples,
read [`fs-status.mica`](../assets/mica/fs-status.mica),
[`http-status.mica`](../assets/mica/http-status.mica),
[`gui-counter.mica`](../assets/mica/gui-counter.mica),
[`browser.mica`](../assets/mica/browser.mica), and
[`editor.mica`](../assets/mica/editor.mica).
