#!/usr/bin/env bash
set -Eeuo pipefail

repo_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"
source "$repo_root/scripts/qemu-arch.sh"

image_path="${MICROSYSTEM_DISK_PATH:-$repo_root/build/microsystem.img}"
case "$image_path" in /*) ;; *) image_path="$repo_root/$image_path" ;; esac
mfsctl="${MICROSYSTEM_MFSCTL:-$repo_root/target/release/mfsctl}"

timeout_seconds="${MICROSYSTEM_GUI_QEMU_TIMEOUT:-45}"
browser_http_port="${MICROSYSTEM_GUI_BROWSER_HTTP_PORT:-18083}"
log_file="${MICROSYSTEM_GUI_QEMU_LOG:-$repo_root/target/gui-qemu.log}"
qmp_socket="${MICROSYSTEM_GUI_QMP_SOCKET:-$repo_root/target/gui-qmp.sock}"
screenshot_file="${MICROSYSTEM_GUI_SCREENSHOT:-$repo_root/target/gui-screendump.png}"
pre_screenshot_file="${MICROSYSTEM_GUI_PRE_SCREENSHOT:-${screenshot_file%.png}-pre.png}"
qmp_log="${MICROSYSTEM_GUI_QMP_LOG:-${log_file%.log}.qmp.jsonl}"
serial_fifo="${MICROSYSTEM_GUI_SERIAL_FIFO:-$repo_root/target/gui-serial.in}"

if ! [[ "$timeout_seconds" =~ ^[0-9]+$ ]] || (( timeout_seconds == 0 )); then
  echo "gui-qemu: MICROSYSTEM_GUI_QEMU_TIMEOUT must be a positive integer" >&2
  exit 2
fi
if ! [[ "$browser_http_port" =~ ^[0-9]+$ ]] || (( browser_http_port == 0 || browser_http_port > 65535 )); then
  echo "gui-qemu: MICROSYSTEM_GUI_BROWSER_HTTP_PORT must be in 1..65535" >&2
  exit 2
fi

# Keep host invocation consistent with the other OrbStack scripts.  All
# artifacts live under the repository mount so the inner process can validate
# the QMP screenshot and leave it available to the caller.
if [[ ! -f /.dockerenv && "${MICROSYSTEM_IN_CONTAINER:-0}" != "1" ]]; then
  if ! command -v docker >/dev/null 2>&1; then
    echo "gui-qemu: docker is required (run through OrbStack)" >&2
    exit 2
  fi
  image="${IMAGE:-microsystem-dev:rust-1.97.1}"
  inner_log="/workspace/target/$(basename -- "$log_file")"
  inner_qmp="/workspace/target/$(basename -- "$qmp_socket")"
  inner_screenshot="/workspace/target/$(basename -- "$screenshot_file")"
  inner_pre_screenshot="/workspace/target/$(basename -- "$pre_screenshot_file")"
  inner_qmp_log="/workspace/target/$(basename -- "$qmp_log")"
  inner_serial_fifo="/workspace/target/$(basename -- "$serial_fifo")"
  exec docker run --rm -i --init \
    -e ARCH="$MICROSYSTEM_ARCH" \
    -e RUSTUP_TOOLCHAIN=1.97.1 \
    -e MICROSYSTEM_IN_CONTAINER=1 \
    -e MICROSYSTEM_GUI_QEMU_TIMEOUT="$timeout_seconds" \
    -e MICROSYSTEM_GUI_QEMU_LOG="$inner_log" \
    -e MICROSYSTEM_GUI_QMP_SOCKET="$inner_qmp" \
    -e MICROSYSTEM_GUI_SCREENSHOT="$inner_screenshot" \
    -e MICROSYSTEM_GUI_PRE_SCREENSHOT="$inner_pre_screenshot" \
    -e MICROSYSTEM_GUI_QMP_LOG="$inner_qmp_log" \
    -e MICROSYSTEM_GUI_SERIAL_FIFO="$inner_serial_fifo" \
    -e MICROSYSTEM_GUI_BROWSER_HTTP_PORT="$browser_http_port" \
    -v "$repo_root:/workspace" \
    -w /workspace "$image" bash scripts/gui-qemu.sh
fi

if ! command -v cargo >/dev/null 2>&1; then
  echo "gui-qemu: cargo is required inside the OrbStack toolchain container" >&2
  exit 2
fi
if ! command -v "$MICROSYSTEM_QEMU_BINARY" >/dev/null 2>&1; then
	echo "gui-qemu: $MICROSYSTEM_QEMU_BINARY is required" >&2
  exit 2
fi
if ! command -v python3 >/dev/null 2>&1; then
  echo "gui-qemu: python3 is required for the HTTP fixture and QMP gate" >&2
  exit 2
fi
if [[ ! -f "target/$MICROSYSTEM_TARGET/release/microsystem-kernel" ]]; then
  echo "gui-qemu: built kernel ELF is missing; run make build first" >&2
  exit 2
fi
if [[ ! -f "$image_path" || ! -x "$mfsctl" ]]; then
  echo "gui-qemu: image $image_path is missing; run make build first" >&2
  exit 2
fi

mkdir -p "$(dirname -- "$log_file")" "$(dirname -- "$qmp_log")" "$(dirname -- "$screenshot_file")" "$(dirname -- "$pre_screenshot_file")" "$(dirname -- "$serial_fifo")"
: >"$log_file"
rm -f "$qmp_socket" "$serial_fifo" "$screenshot_file" "$pre_screenshot_file" "$qmp_log"

qemu_pid=""
qemu_pgid=""
serial_fd=""
counter_fixture_dir="$repo_root/target/gui-mica-fixture.$$"
counter_image_backup="$repo_root/target/gui-mica-image-backup.$$"
counter_image_backed_up=0
browser_fixture_dir="$repo_root/target/gui-browser-fixture.$$"
browser_fixture_log="$browser_fixture_dir/server.log"
browser_fixture_saved_log="$repo_root/target/gui-browser-fixture.log"
browser_fixture_pid=""
editor_fixture_dir="$repo_root/target/gui-editor-fixture.$$"
rm -f "$browser_fixture_saved_log"
cleanup() {
  local status=$?
  if [[ -n "$qemu_pid" ]] && kill -0 "$qemu_pid" 2>/dev/null; then
    if [[ -n "$qemu_pgid" ]]; then
      kill -TERM -- "-$qemu_pgid" 2>/dev/null || true
    else
      kill -TERM "$qemu_pid" 2>/dev/null || true
    fi
    for _ in {1..20}; do
      kill -0 "$qemu_pid" 2>/dev/null || break
      sleep 0.1
    done
    if kill -0 "$qemu_pid" 2>/dev/null; then
      if [[ -n "$qemu_pgid" ]]; then
        kill -KILL -- "-$qemu_pgid" 2>/dev/null || true
      else
        kill -KILL "$qemu_pid" 2>/dev/null || true
      fi
    fi
    wait "$qemu_pid" 2>/dev/null || true
  fi
  if [[ -n "$serial_fd" ]]; then
    eval "exec ${serial_fd}>&-" || true
  fi
  if [[ -n "$browser_fixture_pid" ]] && kill -0 "$browser_fixture_pid" 2>/dev/null; then
    kill -TERM "$browser_fixture_pid" 2>/dev/null || true
    wait "$browser_fixture_pid" 2>/dev/null || true
  fi
  rm -f "$qmp_socket"
  rm -f "$serial_fifo"
  if [[ "$counter_image_backed_up" == 1 && -f "$counter_image_backup" ]]; then
    cp "$counter_image_backup" "$image_path" || true
    if ! cmp -s "$counter_image_backup" "$image_path"; then
      echo "gui-qemu: restored MFS1 image differs from backup" >&2
      status=1
    fi
    rm -f "$counter_image_backup"
  fi
  if [[ -f "$browser_fixture_log" ]]; then
    cp "$browser_fixture_log" "$browser_fixture_saved_log" || status=1
  fi
  rm -rf "$counter_fixture_dir"
  rm -rf "$browser_fixture_dir"
  rm -rf "$editor_fixture_dir"
  exit "$status"
}
trap cleanup EXIT INT TERM

# Install a short-lived Counter script into the test image.  The image is
# restored by cleanup, so the GUI gate never leaves test data in the caller's
# MFS1 artifact.
mkdir -p "$counter_fixture_dir"
cp "$image_path" "$counter_image_backup"
counter_image_backed_up=1
cat >"$counter_fixture_dir/counter.mica" <<'MICA'
--!mica 1
--!allow gui.window

local gui = require("gui")
local count = 0
local label = gui.label {text = "Count: 0"}
local app, err = gui.window {title = "Counter", width = 420, height = 260}
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
        gui.text_input {
            placeholder = "Type here",
            on_submit = function(text)
                label:set_text(text)
                app:invalidate()
            end,
        },
    },
})
app:run()
MICA
"$mfsctl" mkdir "$image_path" /mica >"$counter_fixture_dir/mkdir.log" 2>&1 || true
if ! "$mfsctl" put "$image_path" \
  "$counter_fixture_dir/counter.mica" /mica/gui-counter.mica \
  >"$counter_fixture_dir/put.log" 2>&1; then
  echo "gui-qemu: failed to inject Counter Mica script into MFS1" >&2
  cat "$counter_fixture_dir/put.log" >&2 || true
  exit 1
fi

# Install the production Mica Reader asset with a test-only endpoint rewrite.
# Keep its unscoped browser permission intact so the gate exercises the
# shipped GET-only network contract, parser, navigation, and GUI entry point.
browser_source="$repo_root/assets/mica/browser.mica"
if ! grep -Fq -- "--!allow net.browse" "$browser_source" \
  || ! grep -Fq -- "https://example.com/" "$browser_source"; then
  echo "gui-qemu: browser asset contract markers changed; refuse fixture rewrite" >&2
  exit 1
fi
mkdir -p "$browser_fixture_dir"
sed \
  -e "s|https://example.com/|http://10.0.2.2:${browser_http_port}/index.html|" \
  "$browser_source" >"$browser_fixture_dir/browser.mica"
if ! "$mfsctl" put "$image_path" \
  "$browser_fixture_dir/browser.mica" /mica/gui-browser.mica \
  >"$browser_fixture_dir/browser-put.log" 2>&1; then
  echo "gui-qemu: failed to inject Mica Reader asset into MFS1" >&2
  cat "$browser_fixture_dir/browser-put.log" >&2 || true
  exit 1
fi

# Install the production Mica Editor and a two-line CRLF fixture. The guest
# editor reloads this same path after saving, so its visible document crop is
# the persistence assertion; cleanup restores the original image afterward.
editor_source="$repo_root/assets/mica/editor.mica"
if [[ ! -f "$editor_source" ]]; then
  echo "gui-qemu: editor asset is missing: $editor_source" >&2
  exit 1
fi
mkdir -p "$editor_fixture_dir"
printf 'First editor line\r\nOriginal second line\r\n' >"$editor_fixture_dir/editor-note.txt"
"$mfsctl" mkdir "$image_path" /data \
  >"$editor_fixture_dir/mkdir.log" 2>&1 || true
if ! "$mfsctl" put "$image_path" \
  "$editor_source" /mica/editor.mica \
  >"$editor_fixture_dir/editor-put.log" 2>&1; then
  echo "gui-qemu: failed to inject Mica Editor asset into MFS1" >&2
  cat "$editor_fixture_dir/editor-put.log" >&2 || true
  exit 1
fi
if ! "$mfsctl" put "$image_path" \
  "$editor_fixture_dir/editor-note.txt" /data/editor-note.txt \
  >"$editor_fixture_dir/note-put.log" 2>&1; then
  echo "gui-qemu: failed to inject editor note fixture into MFS1" >&2
  cat "$editor_fixture_dir/note-put.log" >&2 || true
  exit 1
fi
if ! "$mfsctl" put "$image_path" \
  "$editor_fixture_dir/editor-note.txt" /data/note.txt \
  >"$editor_fixture_dir/default-note-put.log" 2>&1; then
  echo "gui-qemu: failed to inject desktop Editor note fixture into MFS1" >&2
  cat "$editor_fixture_dir/default-note-put.log" >&2 || true
  exit 1
fi

# The guest's user-mode network maps 10.0.2.2 to this fixture process.  The
# server intentionally exposes only two pages and records every request so
# same-origin navigation and cross-origin rejection are observable without
# duplicating the browser parser in the host harness.
python3 -u - "$browser_http_port" >"$browser_fixture_log" 2>&1 <<'PY' &
import http.server
import sys

port = int(sys.argv[1])
index_body = """<!doctype html><html><head><title>Mica Browser 首页</title><style>style-hidden</style><script>script_hidden()</script></head><body><h1>Reader 首页</h1><p>中文首页</p><p><a href=\"/next.html\">同源下一页</a></p><p><a href=\"https://evil.example/\">跨域拒绝</a></p></body></html>""".encode("utf-8")
next_body = """<!doctype html><html><head><title>Mica Browser 下一页</title><style>style-hidden</style><script>script_hidden()</script></head><body><h1>Reader 下一页</h1><p>中文下一页</p><p><a href=\"https://evil.example/\">跨域拒绝</a></p></body></html>""".encode("utf-8")

class Handler(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        print(f"GET {self.path} host={self.headers.get('Host', '')}", flush=True)
        if self.path in ("/", "/index.html"):
            body = index_body
        elif self.path == "/next.html":
            body = next_body
        else:
            self.send_error(404)
            return
        self.send_response(200)
        self.send_header("Content-Type", "text/html; charset=utf-8")
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Connection", "close")
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, _format, *_args):
        pass

server = http.server.ThreadingHTTPServer(("0.0.0.0", port), Handler)
server.serve_forever()
PY
browser_fixture_pid=$!
sleep 0.2
if ! kill -0 "$browser_fixture_pid" 2>/dev/null; then
  echo "gui-qemu: browser HTTP fixture failed to start" >&2
  cat "$browser_fixture_log" >&2 || true
  exit 1
fi

if [[ -x target/debug/xtask ]]; then
  qemu_command=(target/debug/xtask gui)
else
  qemu_command=(cargo run -p xtask -- gui)
fi
mkfifo "$serial_fifo"
exec {serial_fd}<>"$serial_fifo"
if command -v setsid >/dev/null 2>&1; then
  setsid -- "${qemu_command[@]}" <&"$serial_fd" >"$log_file" 2>&1 &
  qemu_pid=$!
  qemu_pgid=$qemu_pid
else
  "${qemu_command[@]}" <&"$serial_fd" >"$log_file" 2>&1 &
  qemu_pid=$!
fi

required_markers=(
  "[bootfs] valid=true entries=25 static-elfs=24"
  "[service] resident EL0 address-spaces=13 asids=[0x20..0x2c]"
  "[service] resident EL0 ready=13/13 online=13/13 switches="
  "[proc] dynamic application capacity=16 first-pid=14 independent-slots=true"
  "[gui] virtio-gpu scanout ready"
  "[net] virtio-net ready mac=52:54:00:12:34:56 rx-buffers=2"
  "[gui] unifont runtime loaded=true cache=128 fallback=ascii"
  "[gui] windowd ready resolution=1024x768 format=XRGB8888 clients=3 renderer=commands"
  "[gui] renderer damage-merge=true local-window-damage=true command-cull=true glyph-cache=2way input-batch=16"
  "[gui] desktop windows=3 move=true resize=true minimize=true maximize=true alt-tab=true"
  "[gui] desktop launcher icons=5 fixed-registry=true click-to-open=true"
  "[gui] EL0 windowd presented scanout bytes="
  "[input] virtio-keyboard+tablet ready streams=2 queues=2 dma-isolated=true polling=true"
  "[gui] terminal window ready interactive=true extended-shell=true filesystem=ls,cat,stat,touch,cp,write,mkdir,rmdir,mv,rm,fsync,sync shared-bytes=4096"
  "[gui] files window ready"
  "[gui] files service wait=event-blocked"
  "[gui] monitor window ready"
  "[gui] monitor service wait=event-blocked"
)

deadline=$((SECONDS + timeout_seconds))
while (( SECONDS < deadline )); do
  all_ready=1
  for marker in "${required_markers[@]}"; do
    if ! grep -Fq -- "$marker" "$log_file"; then
      all_ready=0
      break
    fi
  done
  if (( all_ready )); then
    break
  fi
  if ! kill -0 "$qemu_pid" 2>/dev/null; then
    echo "gui-qemu: QEMU exited before GUI markers were complete" >&2
    tail -n 160 "$log_file" >&2 || true
    exit 1
  fi
  sleep 0.2
done

missing=()
for marker in "${required_markers[@]}"; do
  if ! grep -Fq -- "$marker" "$log_file"; then
    missing+=("$marker")
  fi
done
if (( ${#missing[@]} != 0 )); then
  echo "gui-qemu: missing serial GUI markers: ${missing[*]}" >&2
  tail -n 160 "$log_file" >&2 || true
  exit 1
fi

if [[ ! -S "$qmp_socket" ]]; then
  echo "gui-qemu: QMP socket was not created: $qmp_socket" >&2
  tail -n 160 "$log_file" >&2 || true
  exit 1
fi

python3 - "$qmp_socket" "$pre_screenshot_file" "$screenshot_file" "$log_file" "$serial_fifo" "$timeout_seconds" "$browser_fixture_log" "$browser_http_port" >"$qmp_log" <<'PY'
import json
import atexit
import re
import socket
import struct
import subprocess
import sys
import time
import zlib

socket_path, pre_screenshot_path, screenshot_path, serial_log, serial_fifo_path, timeout_value, browser_fixture_log, browser_http_port = sys.argv[1:]
counter_pre_path = screenshot_path.rsplit(".", 1)[0] + "-mica-pre.png"
counter_button_path = screenshot_path.rsplit(".", 1)[0] + "-mica-button.png"
terminal_help_path = screenshot_path.rsplit(".", 1)[0] + "-terminal-help.png"
browser_pre_path = screenshot_path.rsplit(".", 1)[0] + "-browser-pre.png"
browser_link_path = screenshot_path.rsplit(".", 1)[0] + "-browser-link.png"
browser_cross_path = screenshot_path.rsplit(".", 1)[0] + "-browser-cross.png"
editor_pre_path = screenshot_path.rsplit(".", 1)[0] + "-editor-pre.png"
editor_focused_path = screenshot_path.rsplit(".", 1)[0] + "-editor-focused.png"
editor_typed_path = screenshot_path.rsplit(".", 1)[0] + "-editor-typed.png"
editor_apply_path = screenshot_path.rsplit(".", 1)[0] + "-editor-apply.png"
editor_saved_path = screenshot_path.rsplit(".", 1)[0] + "-editor-saved.png"
editor_reload_path = screenshot_path.rsplit(".", 1)[0] + "-editor-reload.png"
icon_editor_pre_path = screenshot_path.rsplit(".", 1)[0] + "-icon-editor-pre.png"
icon_editor_active_path = screenshot_path.rsplit(".", 1)[0] + "-icon-editor-active.png"
icon_editor_minimized_path = screenshot_path.rsplit(".", 1)[0] + "-icon-editor-minimized.png"
icon_editor_restored_path = screenshot_path.rsplit(".", 1)[0] + "-icon-editor-restored.png"
icon_editor_restart_path = screenshot_path.rsplit(".", 1)[0] + "-icon-editor-restart.png"
alt_tab_marker = b"[gui] keyboard+tablet input routed focus=true drag=true alt-tab=true"
terminal_command_marker = b"[gui] terminal command executed name=uptime output=true"
mica_registered_marker = b"[gui] mica client registered command=65536 event=4096 endpoint-isolated=true"
mica_presented_marker = b"[mica] gui presented widgets=true atomic=true isolated=true"
fatal_markers = (b"[panic]", b"DMA fault")
mica_process = None
mica_process2 = None
mica_process3 = None
mica_output_recorded = False
mica_output2_recorded = False
mica_output3_recorded = False
mica_stdout = b""
mica_stderr = b""
mica_stdout2 = b""
mica_stderr2 = b""
mica_stdout3 = b""
mica_stderr3 = b""

def collect_mica_output(index, terminate=False):
    global mica_output_recorded, mica_output2_recorded
    global mica_stdout, mica_stderr, mica_stdout2, mica_stderr2
    process = mica_process if index == 1 else mica_process2
    recorded = mica_output_recorded if index == 1 else mica_output2_recorded
    stdout = mica_stdout if index == 1 else mica_stdout2
    stderr = mica_stderr if index == 1 else mica_stderr2
    if process is None or recorded:
        return stdout, stderr
    if terminate and process.poll() is None:
        process.terminate()
    try:
        stdout, stderr = process.communicate(timeout=3)
    except subprocess.TimeoutExpired:
        process.kill()
        stdout, stderr = process.communicate(timeout=3)
    if index == 1:
        mica_stdout, mica_stderr = stdout, stderr
        mica_output_recorded = True
    else:
        mica_stdout2, mica_stderr2 = stdout, stderr
        mica_output2_recorded = True
    print(json.dumps({f"mica_ssh_{index}": {
        "status": process.returncode,
        "stdout": stdout.decode(errors="replace"),
        "stderr": stderr.decode(errors="replace"),
    }}, sort_keys=True))
    return stdout, stderr

def collect_all_mica_output(terminate=False):
    collect_mica_output(1, terminate=terminate)
    collect_mica_output(2, terminate=terminate)
    collect_mica3_output(terminate=terminate)

def collect_mica3_output(terminate=False):
    global mica_output3_recorded, mica_stdout3, mica_stderr3
    if mica_process3 is None or mica_output3_recorded:
        return mica_stdout3, mica_stderr3
    if terminate and mica_process3.poll() is None:
        mica_process3.terminate()
    try:
        mica_stdout3, mica_stderr3 = mica_process3.communicate(timeout=3)
    except subprocess.TimeoutExpired:
        mica_process3.kill()
        mica_stdout3, mica_stderr3 = mica_process3.communicate(timeout=3)
    mica_output3_recorded = True
    print(json.dumps({"mica_ssh_3": {
        "status": mica_process3.returncode,
        "stdout": mica_stdout3.decode(errors="replace"),
        "stderr": mica_stderr3.decode(errors="replace"),
    }}, sort_keys=True))
    return mica_stdout3, mica_stderr3

def terminate_mica_processes():
    for process in (mica_process, mica_process2, mica_process3):
        if process is not None and process.poll() is None:
            process.terminate()
            try:
                process.wait(timeout=3)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait(timeout=3)

atexit.register(terminate_mica_processes)

def read_reply(stream, wanted_id):
    while True:
        line = stream.readline()
        if not line:
            raise RuntimeError("QMP closed before command reply")
        reply = json.loads(line.decode("utf-8"))
        if reply.get("id") == wanted_id:
            return reply

def send_command(stream, name, execute, arguments=None):
    command = {"execute": execute, "id": name}
    if arguments is not None:
        command["arguments"] = arguments
    stream.write((json.dumps(command) + "\r\n").encode("utf-8"))
    reply = read_reply(stream, name)
    print(json.dumps({name: reply}, sort_keys=True))
    if "error" in reply:
        raise RuntimeError(f"QMP {execute} failed: {reply['error']}")
    return reply

def wait_for_marker(marker, description):
    deadline = time.monotonic() + float(timeout_value)
    while time.monotonic() < deadline:
        try:
            with open(serial_log, "rb") as serial:
                contents = serial.read()
                if any(fatal in contents for fatal in fatal_markers):
                    raise RuntimeError("fatal panic/DMA fault appeared in GUI serial log")
                if marker in contents:
                    return
        except FileNotFoundError:
            pass
        time.sleep(0.05)
    # Preserve the launcher's stdout/stderr even when the serial marker is the
    # first failure; otherwise a blocked init/windowd registration would hide
    # the SSH-side diagnostic that explains the timeout.
    collect_all_mica_output(terminate=True)
    raise RuntimeError(f"{description} marker did not arrive")

def wait_for_marker_count(marker, count, description):
    deadline = time.monotonic() + float(timeout_value)
    while time.monotonic() < deadline:
        try:
            with open(serial_log, "rb") as serial:
                contents = serial.read()
                if any(fatal in contents for fatal in fatal_markers):
                    raise RuntimeError("fatal panic/DMA fault appeared in GUI serial log")
                if contents.count(marker) >= count:
                    return
        except FileNotFoundError:
            pass
        time.sleep(0.05)
    collect_all_mica_output(terminate=True)
    raise RuntimeError(f"{description} marker count did not reach {count}")

def wait_for_endpoint_marker(prefix, endpoint, description):
    marker = prefix + str(endpoint).encode("ascii")
    wait_for_marker(marker, description)

def serial_contents():
    try:
        with open(serial_log, "rb") as serial:
            return serial.read()
    except FileNotFoundError:
        return b""

def browser_fixture_contents():
    try:
        with open(browser_fixture_log, "rb") as fixture:
            return fixture.read()
    except FileNotFoundError:
        return b""

def wait_for_browser_request(path, count, description):
    marker = f"GET {path} ".encode("ascii")
    deadline = time.monotonic() + float(timeout_value)
    while time.monotonic() < deadline:
        contents = browser_fixture_contents()
        if contents.count(marker) >= count:
            return
        serial = serial_contents()
        if any(fatal in serial for fatal in fatal_markers):
            raise RuntimeError("fatal panic/DMA fault appeared in GUI serial log")
        time.sleep(0.05)
    collect_all_mica_output(terminate=True)
    raise RuntimeError(f"{description} fixture request did not arrive")

def send_serial(command):
    # The shell keeps the read/write FIFO descriptor open for QEMU's stdio;
    # each command uses a short-lived writer so a newline is delivered without
    # ever closing the shell's input stream.
    with open(serial_fifo_path, "wb", buffering=0) as serial:
        serial.write(command.encode("ascii") + b"\n")

def wait_for_serial_completion(cursor, description):
    deadline = time.monotonic() + float(timeout_value)
    pattern = re.compile(rb"mica: pid=[0-9]+ status=0")
    while time.monotonic() < deadline:
        contents = serial_contents()
        tail = contents[cursor:]
        if any(fatal in contents for fatal in fatal_markers):
            raise RuntimeError("fatal panic/DMA fault appeared in GUI serial log")
        if pattern.search(tail) and b"micro> " in tail:
            return len(contents)
        time.sleep(0.05)
    collect_all_mica_output(terminate=True)
    raise RuntimeError(f"{description} serial completion did not arrive")

def text_events(text):
    qcodes = {" ": "spc", "-": "minus", "/": "slash", ".": "dot"}
    events = []
    for character in text:
        qcode = qcodes.get(character, character)
        events.extend(
            [
                {"type": "key", "data": {"down": True, "key": {"type": "qcode", "data": qcode}}},
                {"type": "key", "data": {"down": False, "key": {"type": "qcode", "data": qcode}}},
            ]
        )
    events.extend(
        [
            {"type": "key", "data": {"down": True, "key": {"type": "qcode", "data": "ret"}}},
            {"type": "key", "data": {"down": False, "key": {"type": "qcode", "data": "ret"}}},
        ]
    )
    return events

def text_key_events(text):
    qcodes = {" ": "spc", "-": "minus", "/": "slash", ".": "dot"}
    events = []
    for character in text:
        qcode = qcodes.get(character, character)
        events.extend(
            [
                {"type": "key", "data": {"down": True, "key": {"type": "qcode", "data": qcode}}},
                {"type": "key", "data": {"down": False, "key": {"type": "qcode", "data": qcode}}},
            ]
        )
    return {"events": events}

def repeated_key_events(qcode, count):
    events = []
    for _ in range(count):
        events.extend(
            [
                {"type": "key", "data": {"down": True, "key": {"type": "qcode", "data": qcode}}},
                {"type": "key", "data": {"down": False, "key": {"type": "qcode", "data": qcode}}},
            ]
        )
    return {"events": events}

def key_events(*keys):
    return {
        "events": [
            {
                "type": "key",
                "data": {"down": down, "key": {"type": "qcode", "data": qcode}},
            }
            for qcode, down in keys
        ]
    }

def send_input_events_sequential(stream, prefix, payload):
    for event_index, event in enumerate(payload["events"]):
        send_command(
            stream,
            f"{prefix}-event-{event_index}",
            "input-send-event",
            {"events": [event]},
        )
        time.sleep(0.1)

def tablet_click(x, y):
    # VirtIO tablet coordinates are normalized to the visible 1024x768 scanout.
    return {
        "events": [
            {"type": "abs", "data": {"axis": "x", "value": round(x * 32767 / 1023)}},
            {"type": "abs", "data": {"axis": "y", "value": round(y * 32767 / 767)}},
            {"type": "btn", "data": {"down": True, "button": "left"}},
            {"type": "btn", "data": {"down": False, "button": "left"}},
        ]
    }

def pointer_move(x, y):
    return {
        "events": [
            {"type": "abs", "data": {"axis": "x", "value": round(x * 32767 / 1023)}},
            {"type": "abs", "data": {"axis": "y", "value": round(y * 32767 / 767)}},
        ]
    }

def screenshot_bytes(path):
    try:
        with open(path, "rb") as image:
            return image.read()
    except FileNotFoundError:
        return b""

def wait_for_screenshot_change(stream, command_prefix, path, previous, description):
    # Mica emits its public Present marker only once per session.  Polling a
    # fresh QMP screendump until its bytes differ supplies an observable
    # redraw boundary for later callbacks without inventing a second service
    # marker; the fixture request-count assertion below rules out a network
    # side effect while this redraw is observed.
    deadline = time.monotonic() + float(timeout_value)
    sequence = 0
    while time.monotonic() < deadline:
        send_command(
            stream,
            f"{command_prefix}-{sequence}",
            "screendump",
            {"filename": path},
        )
        current = screenshot_bytes(path)
        if current and current != previous:
            return current
        sequence += 1
        time.sleep(0.05)
    collect_all_mica_output(terminate=True)
    raise RuntimeError(f"{description} screenshot redraw did not arrive")

with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as sock:
    sock.settimeout(5.0)
    sock.connect(socket_path)
    stream = sock.makefile("rwb", buffering=0)
    greeting = json.loads(stream.readline().decode("utf-8"))
    print(json.dumps({"greeting": greeting}, sort_keys=True))

    send_command(stream, "capabilities", "qmp_capabilities")
    status = send_command(stream, "status-pre", "query-status").get("return", {}).get("status")
    if status != "running":
        raise RuntimeError(f"QMP query-status is not running: {status!r}")
    send_command(stream, "screendump-pre", "screendump", {"filename": pre_screenshot_path})
    send_command(
        stream,
        "input-alt-tab",
        "input-send-event",
        {
            "events": [
                {"type": "key", "data": {"down": True, "key": {"type": "qcode", "data": "alt"}}},
                {"type": "key", "data": {"down": True, "key": {"type": "qcode", "data": "tab"}}},
                {"type": "key", "data": {"down": False, "key": {"type": "qcode", "data": "tab"}}},
                {"type": "key", "data": {"down": False, "key": {"type": "qcode", "data": "alt"}}},
            ]
        },
    )
    wait_for_marker(alt_tab_marker, "GUI input routed")
    # Terminal starts at (48, 56) with a 640x430 client area.  Send a tablet
    # click in its body so this gate proves pointer focus, not only the
    # Alt-Tab shortcut.  QEMU's absolute tablet coordinates are 0..32767.
    send_command(
        stream,
        "input-terminal-click",
        "input-send-event",
        {
            "events": [
                {"type": "abs", "data": {"axis": "x", "value": 3200}},
                {"type": "abs", "data": {"axis": "y", "value": 4200}},
                {"type": "btn", "data": {"down": True, "button": "left"}},
                {"type": "btn", "data": {"down": False, "button": "left"}},
            ]
        },
    )
    partial_present_marker = b"[gui] EL0 windowd partial-present=true rect="
    wait_for_marker(partial_present_marker, "GUI partial Present")
    partial_match = re.search(
        rb"\[gui\] EL0 windowd partial-present=true rect=(\d+)x(\d+)\+(\d+),(\d+)",
        serial_contents(),
    )
    if partial_match is None:
        raise RuntimeError("GUI partial Present marker had no rectangle geometry")
    partial_width, partial_height, partial_x, partial_y = (
        int(value) for value in partial_match.groups()
    )
    if not (
        0 < partial_width <= 1024
        and 0 < partial_height <= 768
        and 0 <= partial_x
        and 0 <= partial_y
        and partial_x + partial_width <= 1024
        and partial_y + partial_height <= 768
        and partial_width * partial_height < 1024 * 768
    ):
        raise RuntimeError(
            "GUI partial Present rectangle was not a bounded non-full scanout: "
            f"{partial_width}x{partial_height}+{partial_x},{partial_y}"
        )
    print(json.dumps({
        "partial_present": {
            "width": partial_width,
            "height": partial_height,
            "x": partial_x,
            "y": partial_y,
        }
    }, sort_keys=True))
    time.sleep(0.2)
    for character_index, character in enumerate("uptime"):
        send_input_events_sequential(
            stream,
            f"input-terminal-uptime-char-{character_index}",
            text_key_events(character),
        )
    send_input_events_sequential(
        stream,
        "input-terminal-uptime-return",
        key_events(("ret", True), ("ret", False)),
    )
    wait_for_marker(terminal_command_marker, "terminal uptime command")

    terminal_filesystem_marker = b"[gui] terminal filesystem command="

    def terminal_command(name, command, marker_name):
        marker = None
        before = 0
        if marker_name:
            marker = (
                terminal_filesystem_marker
                + marker_name.encode("ascii")
                + b" status=ok"
            )
            before = serial_contents().count(marker)
        # Keep each VirtIO input request to one key event. The windowd event
        # ring can be busy while the terminal drains a prior key, so even a
        # combined key-down/key-up request can eventually exhaust descriptors
        # during a long command. Return stays separate from the command text.
        for character_index, character in enumerate(command):
            send_input_events_sequential(
                stream,
                f"input-terminal-{name}-char-{character_index}",
                text_key_events(character),
            )
        send_input_events_sequential(
            stream,
            f"input-terminal-{name}-return",
            key_events(("ret", True), ("ret", False)),
        )
        if marker is None:
            return
        wait_for_marker_count(
            marker,
            before + 1,
            f"terminal {name} filesystem command",
        )

    # Capture the long help response before filesystem commands.  The
    # terminal service must return this through its shared 4096-byte frame;
    # the former inline reply path truncated it at 40 bytes.
    terminal_command("help", "help", "")
    time.sleep(0.2)
    send_command(
        stream,
        "screendump-terminal-help",
        "screendump",
        {"filename": terminal_help_path},
    )

    terminal_command("mkdir", "mkdir /tfs", "mkdir")
    terminal_command("touch", "touch /tfs/empty", "touch")
    terminal_command(
        "write",
        "write /tfs/a proof",
        "write",
    )
    terminal_command("cp", "cp /tfs/a /tfs/c", "cp")
    terminal_command("stat", "stat /tfs/a", "stat")
    terminal_command(
        "mv",
        "mv /tfs/a /tfs/b",
        "mv",
    )
    terminal_command("cat", "cat /tfs/b", "cat")
    terminal_command("ls", "ls /tfs", "ls")
    terminal_command("fsync", "fsync /tfs/b", "fsync")
    terminal_command("rm-file", "rm /tfs/b", "rm")
    terminal_command("rm-copy", "rm /tfs/c", "rm")
    terminal_command("rm-empty", "rm /tfs/empty", "rm")
    terminal_command("rm-dir", "rmdir /tfs", "rmdir")
    terminal_command("sync", "sync", "sync")
    # The fixed Terminal client intentionally exposes only diagnostic
    # commands. Launch Mica through the real SSH shell; QMP still drives all
    # GUI interaction below.  Two independent SSH launches exercise the
    # init/windowd endpoint allocator and per-session event/display regions.
    def launch_mica():
        return subprocess.Popen(
            [
                "ssh",
                "-F",
                "/dev/null",
                "-T",
                "-o",
                "BatchMode=yes",
                "-o",
                "IdentitiesOnly=yes",
                "-o",
                "StrictHostKeyChecking=no",
                "-o",
                "UserKnownHostsFile=/dev/null",
                "-o",
                "ConnectTimeout=5",
                "-o",
                "ConnectionAttempts=1",
                "-p",
                "2222",
                "-i",
                "build/ssh/id_ed25519",
                "micro@127.0.0.1",
                "mica --gui --timeout 60s --allow gui.window /mica/gui-counter.mica",
            ],
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
        )

    mica_process = launch_mica()
    wait_for_endpoint_marker(
        b"[gui] mica registration requested endpoint=", 0,
        "first Mica endpoint registration",
    )
    wait_for_endpoint_marker(
        b"[gui] mica registration received endpoint=", 0,
        "first windowd endpoint registration",
    )
    wait_for_marker(mica_registered_marker, "Mica GUI client registration")
    wait_for_marker(mica_presented_marker, "Mica GUI first Present")
    send_command(
        stream,
        "screendump-mica-pre",
        "screendump",
        {"filename": counter_pre_path},
    )
    # GuiHost lays the Counter column out at x=16, y=16 inside the client;
    # with the requested (72,76) window this point is in the button body.
    send_command(
        stream,
        "input-counter-button",
        "input-send-event",
        tablet_click(140, 180),
    )
    time.sleep(0.3)
    send_command(
        stream,
        "screendump-mica-button",
        "screendump",
        {"filename": counter_button_path},
    )
    # Focus the text input, submit ASCII, and require a second visible redraw.
    send_command(
        stream,
        "input-counter-text",
        "input-send-event",
        tablet_click(140, 224),
    )
    time.sleep(0.1)
    send_command(
        stream,
        "input-counter-typed",
        "input-send-event",
        {"events": text_events("ok")},
    )
    time.sleep(0.3)
    ascii_path = screenshot_path.rsplit(".", 1)[0] + "-ascii.png"
    send_command(stream, "screendump-ascii", "screendump", {"filename": ascii_path})
    send_command(
        stream,
        "input-counter-unicode-start",
        "input-send-event",
        key_events(("ctrl", True), ("shift", True), ("u", True), ("u", False)),
    )
    wait_for_marker(
        b"[gui] unicode text input active=true",
        "Unicode input activation",
    )
    for sequence, (qcode, accumulated) in enumerate(
        (("4", 4), ("e", 78), ("2", 1250), ("d", 20013)),
        start=1,
    ):
        send_command(
            stream,
            f"input-counter-unicode-digit-{sequence}",
            "input-send-event",
            key_events((qcode, True), (qcode, False)),
        )
        wait_for_marker(
            f"[gui] unicode text input accumulated={accumulated}".encode(),
            f"Unicode input digit {sequence}",
        )
    send_command(
        stream,
        "input-counter-unicode-submit",
        "input-send-event",
        key_events(("ret", True), ("ret", False)),
    )
    wait_for_marker(
        b"[gui] unicode text input codepoint=20013 delivered=true",
        "Unicode input delivery",
    )
    wait_for_marker(
        b"[mica] gui unicode rendered codepoint=20013 true",
        "Mica Unicode text rendering",
    )
    send_command(
        stream,
        "input-counter-unicode-release",
        "input-send-event",
        key_events(("shift", False), ("ctrl", False)),
    )
    time.sleep(0.1)
    status = send_command(stream, "status-post", "query-status").get("return", {}).get("status")
    if status != "running":
        raise RuntimeError(f"QMP post-input query-status is not running: {status!r}")
    send_command(stream, "screendump-post", "screendump", {"filename": screenshot_path})

    # Resize from the default window's bottom-right grip (72+420-14,
    # 76+260-14) to a visibly larger geometry before testing maximize/restore.
    send_command(
        stream,
        "input-counter-resize",
        "input-send-event",
        {
            "events": [
                {"type": "abs", "data": {"axis": "x", "value": round(486 * 32767 / 1023)}},
                {"type": "abs", "data": {"axis": "y", "value": round(330 * 32767 / 767)}},
                {"type": "btn", "data": {"down": True, "button": "left"}},
                {"type": "abs", "data": {"axis": "x", "value": round(550 * 32767 / 1023)}},
                {"type": "abs", "data": {"axis": "y", "value": round(390 * 32767 / 767)}},
                {"type": "btn", "data": {"down": False, "button": "left"}},
            ]
        },
    )
    time.sleep(0.3)
    resized_path = screenshot_path.rsplit(".", 1)[0] + "-resized.png"
    send_command(stream, "screendump-resized", "screendump", {"filename": resized_path})

    # Alt+F10 is windowd's maximize/restore shortcut (Linux input codes 68).
    # Capture both states; the pixel validator below checks the expected
    # title-bar geometry rather than treating a CRC-only change as geometry.
    send_command(
        stream,
        "input-counter-maximize",
        "input-send-event",
        {
            "events": [
                {"type": "key", "data": {"down": True, "key": {"type": "qcode", "data": "alt"}}},
                {"type": "key", "data": {"down": True, "key": {"type": "qcode", "data": "f10"}}},
                {"type": "key", "data": {"down": False, "key": {"type": "qcode", "data": "f10"}}},
                {"type": "key", "data": {"down": False, "key": {"type": "qcode", "data": "alt"}}},
            ]
        },
    )
    time.sleep(0.3)
    max_path = screenshot_path.rsplit(".", 1)[0] + "-max.png"
    send_command(stream, "screendump-max", "screendump", {"filename": max_path})
    send_command(
        stream,
        "input-counter-restore",
        "input-send-event",
        {
            "events": [
                {"type": "key", "data": {"down": True, "key": {"type": "qcode", "data": "alt"}}},
                {"type": "key", "data": {"down": True, "key": {"type": "qcode", "data": "f10"}}},
                {"type": "key", "data": {"down": False, "key": {"type": "qcode", "data": "f10"}}},
                {"type": "key", "data": {"down": False, "key": {"type": "qcode", "data": "alt"}}},
            ]
        },
    )
    time.sleep(0.3)
    restore_path = screenshot_path.rsplit(".", 1)[0] + "-restore.png"
    send_command(stream, "screendump-restore", "screendump", {"filename": restore_path})

    # Alt+F9 minimizes (Linux input code 67).  Restore the first dynamic
    # window from its taskbar entry (fourth slot while the three built-ins are
    # present), proving taskbar hit testing rather than a second shortcut.
    send_command(
        stream,
        "input-counter-minimize",
        "input-send-event",
        {
            "events": [
                {"type": "key", "data": {"down": True, "key": {"type": "qcode", "data": "alt"}}},
                {"type": "key", "data": {"down": True, "key": {"type": "qcode", "data": "f9"}}},
                {"type": "key", "data": {"down": False, "key": {"type": "qcode", "data": "f9"}}},
                {"type": "key", "data": {"down": False, "key": {"type": "qcode", "data": "alt"}}},
            ]
        },
    )
    time.sleep(0.3)
    minimized_path = screenshot_path.rsplit(".", 1)[0] + "-minimized.png"
    send_command(stream, "screendump-minimized", "screendump", {"filename": minimized_path})
    send_command(stream, "input-counter-taskbar-restore", "input-send-event", tablet_click(480, 750))
    time.sleep(0.3)
    send_command(stream, "input-counter-stable-pointer", "input-send-event", {
        "events": [
            {"type": "abs", "data": {"axis": "x", "value": round(140 * 32767 / 1023)}},
            {"type": "abs", "data": {"axis": "y", "value": round(224 * 32767 / 767)}},
        ]
    })
    time.sleep(0.3)
    taskbar_path = screenshot_path.rsplit(".", 1)[0] + "-taskbar-restore.png"
    send_command(stream, "screendump-taskbar-restore", "screendump", {"filename": taskbar_path})
    first_before_second_path = screenshot_path.rsplit(".", 1)[0] + "-first-before-second.png"
    send_command(stream, "input-first-baseline-pointer", "input-send-event", {
        "events": [
            {"type": "abs", "data": {"axis": "x", "value": round(140 * 32767 / 1023)}},
            {"type": "abs", "data": {"axis": "y", "value": round(224 * 32767 / 767)}},
        ]
    })
    time.sleep(0.3)
    send_command(stream, "screendump-first-before-second", "screendump", {"filename": first_before_second_path})

    # Start a second independent GUI session while the first remains alive.
    # sshd serves one connection synchronously, so the serial shell starts
    # later sessions while the first SSH command remains active.  Both scripts
    # use the same requested geometry, so the second is the focused/topmost
    # client and its edits must not mutate the first display list.
    second_serial_cursor = len(serial_contents())
    send_serial("mica --gui --allow gui.window /mica/gui-counter.mica")
    wait_for_endpoint_marker(
        b"[gui] mica registration requested endpoint=", 1,
        "second Mica endpoint registration",
    )
    wait_for_endpoint_marker(
        b"[gui] mica registration received endpoint=", 1,
        "second windowd endpoint registration",
    )
    wait_for_marker_count(mica_registered_marker, 2, "two Mica client registrations")
    wait_for_marker_count(mica_presented_marker, 2, "two Mica client Presents")
    second_pre_path = screenshot_path.rsplit(".", 1)[0] + "-second-pre.png"
    second_button_path = screenshot_path.rsplit(".", 1)[0] + "-second-button.png"
    second_text_path = screenshot_path.rsplit(".", 1)[0] + "-second-text.png"
    after_second_close_path = screenshot_path.rsplit(".", 1)[0] + "-after-second-close.png"
    send_command(stream, "screendump-second-pre", "screendump", {"filename": second_pre_path})
    send_command(stream, "input-second-button", "input-send-event", tablet_click(140, 180))
    time.sleep(0.3)
    send_command(stream, "screendump-second-button", "screendump", {"filename": second_button_path})
    send_command(stream, "input-second-text", "input-send-event", tablet_click(140, 224))
    time.sleep(0.1)
    send_command(stream, "input-second-typed", "input-send-event", {"events": text_events("two")})
    time.sleep(0.3)
    send_command(stream, "screendump-second-text", "screendump", {"filename": second_text_path})

    # Close the focused/topmost second window, wait for its endpoint and
    # process to be reclaimed, then force one pointer redraw of the first
    # window before taking the comparison screenshot.
    send_command(stream, "input-second-close", "input-send-event", tablet_click(484, 85))
    wait_for_endpoint_marker(
        b"[gui] mica client unregistered endpoint=", 1,
        "second Mica endpoint reclaim",
    )
    wait_for_serial_completion(second_serial_cursor, "second Mica GUI")
    send_command(stream, "input-first-after-second-close", "input-send-event", {
        "events": [
            {"type": "abs", "data": {"axis": "x", "value": round(140 * 32767 / 1023)}},
            {"type": "abs", "data": {"axis": "y", "value": round(224 * 32767 / 767)}},
        ]
    })
    time.sleep(0.3)
    send_command(stream, "screendump-after-second-close", "screendump", {"filename": after_second_close_path})

    # Reuse the freed endpoint/window slot with a third session while the
    # first session remains alive. Endpoint 1 must be allocated again, and
    # its independent Present/close lifecycle must complete normally.
    third_serial_cursor = len(serial_contents())
    send_serial("mica --gui --allow gui.window /mica/gui-counter.mica")
    wait_for_endpoint_marker(
        b"[gui] mica registration requested endpoint=", 1,
        "reused Mica endpoint registration",
    )
    wait_for_endpoint_marker(
        b"[gui] mica registration received endpoint=", 1,
        "reused windowd endpoint registration",
    )
    wait_for_marker_count(mica_registered_marker, 3, "three Mica client registrations")
    wait_for_marker_count(mica_presented_marker, 3, "three Mica client Presents")
    send_command(stream, "input-reused-close", "input-send-event", tablet_click(484, 85))
    wait_for_marker_count(
        b"[gui] mica client unregistered endpoint=1 resources-reclaimed=true",
        2,
        "reused Mica endpoint reclaim",
    )
    wait_for_serial_completion(third_serial_cursor, "reused Mica GUI")
    send_command(stream, "input-first-after-reuse", "input-send-event", {
        "events": [
            {"type": "abs", "data": {"axis": "x", "value": round(140 * 32767 / 1023)}},
            {"type": "abs", "data": {"axis": "y", "value": round(224 * 32767 / 767)}},
        ]
    })
    time.sleep(0.3)
    after_reuse_close_path = screenshot_path.rsplit(".", 1)[0] + "-after-reuse-close.png"
    send_command(stream, "screendump-after-reuse-close", "screendump", {"filename": after_reuse_close_path})

    # Launch the production Mica Reader through the serial shell while the
    # original Counter remains alive.  Its fixture request and display-list
    # markers are counted relative to the insertion point so unrelated GUI
    # marker changes do not make this gate silently stale.
    browser_serial_cursor = len(serial_contents())
    browser_registered_before = serial_contents().count(mica_registered_marker)
    browser_presented_before = serial_contents().count(mica_presented_marker)
    browser_endpoint_requested_marker = b"[gui] mica registration requested endpoint=1"
    browser_endpoint_received_marker = b"[gui] mica registration received endpoint=1"
    browser_endpoint_unregistered_marker = b"[gui] mica client unregistered endpoint=1"
    browser_endpoint_requested_before = serial_contents().count(browser_endpoint_requested_marker)
    browser_endpoint_received_before = serial_contents().count(browser_endpoint_received_marker)
    browser_endpoint_unregistered_before = serial_contents().count(browser_endpoint_unregistered_marker)
    send_serial(
        "mica --gui --timeout 60s --allow gui.window --allow net.browse --allow fs.write:/data "
        "/mica/gui-browser.mica"
    )
    wait_for_marker_count(
        browser_endpoint_requested_marker,
        browser_endpoint_requested_before + 1,
        "Mica Reader endpoint registration",
    )
    wait_for_marker_count(
        browser_endpoint_received_marker,
        browser_endpoint_received_before + 1,
        "Mica Reader windowd endpoint registration",
    )
    wait_for_marker_count(
        mica_registered_marker,
        browser_registered_before + 1,
        "Mica Reader GUI client registration",
    )
    wait_for_marker_count(
        mica_presented_marker,
        browser_presented_before + 1,
        "Mica Reader first Present",
    )
    wait_for_browser_request("/index.html", 1, "Mica Reader initial page")
    send_command(
        stream,
        "input-browser-pre-pointer",
        "input-send-event",
        pointer_move(900, 700),
    )
    send_command(stream, "screendump-browser-pre", "screendump", {"filename": browser_pre_path})
    browser_pre_bytes = screenshot_bytes(browser_pre_path)

    # The fixture puts the same-origin link on the third document row.  A
    # fixed client-coordinate click is valid for the production 760x620
    # Reader geometry and is intentionally retained as evidence on failure.
    send_command(stream, "input-browser-same-origin", "input-send-event", tablet_click(180, 256))
    wait_for_browser_request("/next.html", 1, "Mica Reader same-origin navigation")
    send_command(
        stream,
        "input-browser-link-pointer",
        "input-send-event",
        pointer_move(900, 700),
    )
    wait_for_screenshot_change(
        stream,
        "screendump-browser-link",
        browser_link_path,
        browser_pre_bytes,
        "Mica Reader same-origin",
    )

    # The next page's third row is cross-origin.  It must redraw the status
    # error without causing a second network request.
    send_command(stream, "input-browser-cross-origin", "input-send-event", tablet_click(180, 256))
    send_command(
        stream,
        "input-browser-cross-pointer",
        "input-send-event",
        pointer_move(900, 700),
    )
    browser_link_bytes = screenshot_bytes(browser_link_path)
    wait_for_screenshot_change(
        stream,
        "screendump-browser-cross",
        browser_cross_path,
        browser_link_bytes,
        "Mica Reader cross-origin rejection",
    )
    browser_requests = browser_fixture_contents()
    if browser_requests.count(b"GET /index.html ") != 1:
        raise RuntimeError("Mica Reader fixture received an unexpected /index.html count")
    if browser_requests.count(b"GET /next.html ") != 1:
        raise RuntimeError("Mica Reader fixture received an unexpected /next.html count")
    if browser_requests.count(b"GET ") != 2:
        raise RuntimeError("Mica Reader cross-origin click issued an unexpected request")
    if b"evil.example" in browser_requests:
        raise RuntimeError("Mica Reader attempted a cross-origin request")

    send_command(stream, "input-browser-close", "input-send-event", tablet_click(824, 85))
    wait_for_marker_count(
        browser_endpoint_unregistered_marker,
        browser_endpoint_unregistered_before + 1,
        "Mica Reader endpoint reclaim",
    )
    wait_for_serial_completion(browser_serial_cursor, "Mica Reader GUI")
    browser_serial_tail = serial_contents()[browser_serial_cursor:]
    loaded_count = browser_serial_tail.count(b"mica-reader loaded status=200")
    if loaded_count != 2:
        raise RuntimeError(f"Mica Reader loaded marker count was {loaded_count}, expected 2")

    # Reuse endpoint 1 for the production Mica Editor. The fixture contains
    # two CRLF lines; selecting the second row, replacing its text, applying,
    # saving and reloading exercises the retained line model and atomic
    # persistence without creating a second GUI harness.
    editor_serial_cursor = len(serial_contents())
    editor_registered_before = serial_contents().count(mica_registered_marker)
    editor_presented_before = serial_contents().count(mica_presented_marker)
    editor_endpoint_requested_marker = b"[gui] mica registration requested endpoint=1"
    editor_endpoint_received_marker = b"[gui] mica registration received endpoint=1"
    editor_endpoint_unregistered_marker = b"[gui] mica client unregistered endpoint=1"
    editor_endpoint_requested_before = serial_contents().count(editor_endpoint_requested_marker)
    editor_endpoint_received_before = serial_contents().count(editor_endpoint_received_marker)
    editor_endpoint_unregistered_before = serial_contents().count(editor_endpoint_unregistered_marker)
    send_serial(
        "mica --gui --timeout 86400s --allow gui.window --allow fs.read:/data "
        "--allow fs.write:/data /mica/editor.mica -- /data/editor-note.txt"
    )
    wait_for_marker_count(
        editor_endpoint_requested_marker,
        editor_endpoint_requested_before + 1,
        "Mica Editor endpoint registration",
    )
    wait_for_marker_count(
        editor_endpoint_received_marker,
        editor_endpoint_received_before + 1,
        "Mica Editor windowd endpoint registration",
    )
    wait_for_marker_count(
        mica_registered_marker,
        editor_registered_before + 1,
        "Mica Editor GUI client registration",
    )
    wait_for_marker_count(
        mica_presented_marker,
        editor_presented_before + 1,
        "Mica Editor first Present",
    )
    time.sleep(0.2)
    editor_pointer_marker = b"[mica] gui pointer widget="
    editor_pointer_before = serial_contents().count(editor_pointer_marker)
    editor_widget4_marker = b"[mica] gui pointer widget=4 delivered=true"
    editor_widget5_marker = b"[mica] gui pointer widget=5 delivered=true"
    editor_widget6_marker = b"[mica] gui pointer widget=6 delivered=true"
    editor_widget9_marker = b"[mica] gui pointer widget=9 delivered=true"
    editor_widget10_marker = b"[mica] gui pointer widget=10 delivered=true"
    send_command(stream, "input-editor-pre-pointer", "input-send-event", pointer_move(900, 700))
    send_command(stream, "screendump-editor-pre", "screendump", {"filename": editor_pre_path})
    editor_pre_bytes = screenshot_bytes(editor_pre_path)

    # The editor's second document row is centered near (180,200); the line
    # input is below the list and Apply is the first button in the footer.
    editor_pointer_before_select = serial_contents().count(editor_pointer_marker)
    editor_widget4_before = serial_contents().count(editor_widget4_marker)
    send_command(stream, "input-editor-select-second", "input-send-event", tablet_click(180, 200))
    wait_for_marker_count(
        editor_pointer_marker,
        editor_pointer_before_select + 1,
        "Mica Editor document pointer dispatch",
    )
    wait_for_marker_count(
        editor_widget4_marker,
        editor_widget4_before + 1,
        "Mica Editor document pointer widget",
    )
    editor_pointer_before_line = serial_contents().count(editor_pointer_marker)
    editor_widget5_before = serial_contents().count(editor_widget5_marker)
    send_command(stream, "input-editor-line-input", "input-send-event", tablet_click(240, 590))
    wait_for_marker_count(
        editor_pointer_marker,
        editor_pointer_before_line + 1,
        "Mica Editor line pointer dispatch",
    )
    wait_for_marker_count(
        editor_widget5_marker,
        editor_widget5_before + 1,
        "Mica Editor line pointer widget",
    )
    send_command(stream, "input-editor-focused-pointer", "input-send-event", pointer_move(900, 700))
    send_command(stream, "screendump-editor-focused", "screendump", {"filename": editor_focused_path})
    editor_focused_bytes = screenshot_bytes(editor_focused_path)
    send_command(stream, "input-editor-append-suffix", "input-send-event", text_key_events("-edit"))
    editor_typed_bytes = wait_for_screenshot_change(
        stream,
        "screendump-editor-typed",
        editor_typed_path,
        editor_focused_bytes,
        "Mica Editor typed input",
    )
    editor_pointer_before_apply = serial_contents().count(editor_pointer_marker)
    editor_widget6_before = serial_contents().count(editor_widget6_marker)
    send_command(stream, "input-editor-apply", "input-send-event", tablet_click(110, 635))
    wait_for_marker_count(
        editor_pointer_marker,
        editor_pointer_before_apply + 1,
        "Mica Editor Apply pointer dispatch",
    )
    wait_for_marker_count(
        editor_widget6_marker,
        editor_widget6_before + 1,
        "Mica Editor Apply pointer widget",
    )
    send_command(stream, "input-editor-applied-pointer", "input-send-event", pointer_move(900, 700))
    wait_for_screenshot_change(
        stream,
        "screendump-editor-apply",
        editor_apply_path,
        editor_typed_bytes,
        "Mica Editor Apply",
    )
    editor_apply_bytes = screenshot_bytes(editor_apply_path)

    editor_pointer_before_save = serial_contents().count(editor_pointer_marker)
    editor_widget10_before = serial_contents().count(editor_widget10_marker)
    send_command(stream, "input-editor-save", "input-send-event", tablet_click(410, 635))
    wait_for_marker_count(
        editor_pointer_marker,
        editor_pointer_before_save + 1,
        "Mica Editor Save pointer dispatch",
    )
    wait_for_marker_count(
        editor_widget10_marker,
        editor_widget10_before + 1,
        "Mica Editor Save pointer widget",
    )
    send_command(stream, "input-editor-saved-pointer", "input-send-event", pointer_move(900, 700))
    wait_for_screenshot_change(
        stream,
        "screendump-editor-saved",
        editor_saved_path,
        editor_apply_bytes,
        "Mica Editor Save",
    )
    editor_saved_bytes = screenshot_bytes(editor_saved_path)

    editor_pointer_before_reload = serial_contents().count(editor_pointer_marker)
    editor_widget9_before = serial_contents().count(editor_widget9_marker)
    send_command(stream, "input-editor-reload", "input-send-event", tablet_click(350, 635))
    wait_for_marker_count(
        editor_pointer_marker,
        editor_pointer_before_reload + 1,
        "Mica Editor Reload pointer dispatch",
    )
    wait_for_marker_count(
        editor_widget9_marker,
        editor_widget9_before + 1,
        "Mica Editor Reload pointer widget",
    )
    send_command(stream, "input-editor-reload-pointer", "input-send-event", pointer_move(900, 700))
    wait_for_screenshot_change(
        stream,
        "screendump-editor-reload",
        editor_reload_path,
        editor_saved_bytes,
        "Mica Editor Reload",
    )

    editor_close_marker = b"[mica] gui close requested delivered=true"
    editor_close_before = serial_contents().count(editor_close_marker)
    send_command(
        stream,
        "input-editor-close",
        "input-send-event",
        key_events(("alt", True), ("f4", True), ("f4", False), ("alt", False)),
    )
    wait_for_marker_count(
        editor_close_marker,
        editor_close_before + 1,
        "Mica Editor CloseRequested delivery",
    )
    wait_for_marker_count(
        editor_endpoint_unregistered_marker,
        editor_endpoint_unregistered_before + 1,
        "Mica Editor endpoint reclaim",
    )
    wait_for_serial_completion(editor_serial_cursor, "Mica Editor GUI")
    editor_serial_tail = serial_contents()[editor_serial_cursor:]
    editor_loaded_count = editor_serial_tail.count(b"mica-editor loaded path=/data/editor-note.txt")
    if editor_loaded_count < 2:
        raise RuntimeError(
            f"Mica Editor loaded marker count was {editor_loaded_count}, expected at least 2"
        )
    if b"mica-editor saved path=/data/editor-note.txt" not in editor_serial_tail:
        raise RuntimeError("Mica Editor save marker did not arrive")
    if b"atomic=true fsync=true" not in editor_serial_tail:
        raise RuntimeError("Mica Editor save marker omitted atomic/fsync contract")

    # Exercise the fixed desktop Editor icon after the shell-launched Editor
    # has been reclaimed.  The original Counter window remains alive but does
    # not cover the icon strip at y=650..714.  Init supplies the fixed
    # /data/note.txt path and the exact gui/fs policy; windowd binds the
    # resulting dynamic client to Application::Editor (5).
    icon_editor_clicked_marker = b"[gui] desktop icon clicked application=5 launch-requested=true"
    icon_editor_launch_marker = b"[gui] desktop launch application=5 accepted=true"
    icon_editor_endpoint_requested_marker = b"[gui] mica registration requested endpoint=1"
    icon_editor_endpoint_received_marker = b"[gui] mica registration received endpoint=1"
    icon_editor_unregistered_marker = b"[gui] mica client unregistered endpoint=1"
    icon_editor_fs_read_marker = b"[mica] filesystem request operation=13 returned=true"
    icon_editor_exiting_marker = b"[mica] exiting=true"
    icon_editor_clicked_before = serial_contents().count(icon_editor_clicked_marker)
    icon_editor_launch_before = serial_contents().count(icon_editor_launch_marker)
    icon_editor_requested_before = serial_contents().count(icon_editor_endpoint_requested_marker)
    icon_editor_received_before = serial_contents().count(icon_editor_endpoint_received_marker)
    icon_editor_unregistered_before = serial_contents().count(icon_editor_unregistered_marker)
    icon_editor_registered_before = serial_contents().count(mica_registered_marker)
    icon_editor_presented_before = serial_contents().count(mica_presented_marker)
    icon_editor_fs_read_before = serial_contents().count(icon_editor_fs_read_marker)
    icon_editor_exiting_before = serial_contents().count(icon_editor_exiting_marker)

    send_command(stream, "input-icon-editor-pre-pointer", "input-send-event", pointer_move(900, 700))
    send_command(
        stream,
        "screendump-icon-editor-pre",
        "screendump",
        {"filename": icon_editor_pre_path},
    )
    icon_editor_pre_bytes = screenshot_bytes(icon_editor_pre_path)
    if not icon_editor_pre_bytes:
        raise RuntimeError("desktop Editor icon pre-launch screenshot is empty")

    send_command(stream, "input-icon-editor-launch", "input-send-event", tablet_click(972, 682))
    wait_for_marker_count(
        icon_editor_clicked_marker,
        icon_editor_clicked_before + 1,
        "desktop Editor icon launch request",
    )
    wait_for_marker_count(
        icon_editor_launch_marker,
        icon_editor_launch_before + 1,
        "desktop Editor fixed launch acceptance",
    )
    wait_for_marker_count(
        icon_editor_endpoint_requested_marker,
        icon_editor_requested_before + 1,
        "desktop Editor endpoint request",
    )
    wait_for_marker_count(
        icon_editor_endpoint_received_marker,
        icon_editor_received_before + 1,
        "desktop Editor endpoint registration",
    )
    wait_for_marker_count(
        mica_registered_marker,
        icon_editor_registered_before + 1,
        "desktop Editor GUI registration",
    )
    wait_for_marker_count(
        icon_editor_fs_read_marker,
        icon_editor_fs_read_before + 1,
        "desktop Editor filesystem load",
    )
    wait_for_marker_count(
        mica_presented_marker,
        icon_editor_presented_before + 1,
        "desktop Editor first Present",
    )

    send_command(stream, "input-icon-editor-active-pointer", "input-send-event", pointer_move(900, 700))
    send_command(
        stream,
        "screendump-icon-editor-active",
        "screendump",
        {"filename": icon_editor_active_path},
    )
    icon_editor_active_bytes = screenshot_bytes(icon_editor_active_path)
    if not icon_editor_active_bytes or icon_editor_active_bytes == icon_editor_pre_bytes:
        raise RuntimeError("desktop Editor icon launch did not change the screenshot")

    # A second click while the application is running must only focus/restore;
    # it must not submit another init launch or create another GUI session.
    icon_editor_clicked_after_first = serial_contents().count(icon_editor_clicked_marker)
    icon_editor_launch_after_first = serial_contents().count(icon_editor_launch_marker)
    icon_editor_registered_after_first = serial_contents().count(mica_registered_marker)
    icon_editor_presented_after_first = serial_contents().count(mica_presented_marker)
    send_command(stream, "input-icon-editor-duplicate", "input-send-event", tablet_click(972, 682))
    send_command(stream, "input-icon-editor-duplicate-pointer", "input-send-event", pointer_move(900, 700))
    time.sleep(0.4)
    if serial_contents().count(icon_editor_clicked_marker) != icon_editor_clicked_after_first:
        raise RuntimeError("duplicate desktop Editor click issued another launch request")
    if serial_contents().count(icon_editor_launch_marker) != icon_editor_launch_after_first:
        raise RuntimeError("duplicate desktop Editor click issued another init launch")
    if serial_contents().count(mica_registered_marker) != icon_editor_registered_after_first:
        raise RuntimeError("duplicate desktop Editor click created another GUI session")
    if serial_contents().count(mica_presented_marker) != icon_editor_presented_after_first:
        raise RuntimeError("duplicate desktop Editor click created another Present")

    # Minimize then click the icon: the already-running client is restored and
    # focused without a second registration.
    send_command(
        stream,
        "input-icon-editor-minimize",
        "input-send-event",
        key_events(("alt", True), ("f9", True), ("f9", False), ("alt", False)),
    )
    time.sleep(0.2)
    send_command(stream, "input-icon-editor-restore", "input-send-event", tablet_click(972, 682))
    send_command(stream, "input-icon-editor-restored-pointer", "input-send-event", pointer_move(900, 700))
    time.sleep(0.3)
    send_command(
        stream,
        "screendump-icon-editor-restored",
        "screendump",
        {"filename": icon_editor_restored_path},
    )
    icon_editor_restored_bytes = screenshot_bytes(icon_editor_restored_path)
    if not icon_editor_restored_bytes:
        raise RuntimeError("desktop Editor icon restore screenshot is empty")
    if serial_contents().count(mica_registered_marker) != icon_editor_registered_after_first:
        raise RuntimeError("desktop Editor icon restore created another GUI session")
    if serial_contents().count(mica_presented_marker) != icon_editor_presented_after_first:
        raise RuntimeError("desktop Editor icon restore created another Present")

    icon_editor_close_before = serial_contents().count(editor_close_marker)
    send_command(
        stream,
        "input-icon-editor-close",
        "input-send-event",
        key_events(("alt", True), ("f4", True), ("f4", False), ("alt", False)),
    )
    wait_for_marker_count(
        editor_close_marker,
        icon_editor_close_before + 1,
        "desktop Editor CloseRequested delivery",
    )
    wait_for_marker_count(
        icon_editor_unregistered_marker,
        icon_editor_unregistered_before + 1,
        "desktop Editor endpoint reclaim",
    )
    wait_for_marker_count(
        icon_editor_exiting_marker,
        icon_editor_exiting_before + 1,
        "desktop Editor process exit",
    )

    # The same icon must be reusable after the first icon-launched session is
    # reclaimed.  Keep the second launch intentionally short and close it via
    # the same CloseRequested path.
    send_command(stream, "input-icon-editor-restart-pointer", "input-send-event", pointer_move(900, 700))
    icon_editor_restart_clicked_before = serial_contents().count(icon_editor_clicked_marker)
    icon_editor_restart_launch_before = serial_contents().count(icon_editor_launch_marker)
    icon_editor_restart_requested_before = serial_contents().count(icon_editor_endpoint_requested_marker)
    icon_editor_restart_received_before = serial_contents().count(icon_editor_endpoint_received_marker)
    icon_editor_restart_registered_before = serial_contents().count(mica_registered_marker)
    icon_editor_restart_presented_before = serial_contents().count(mica_presented_marker)
    icon_editor_restart_fs_read_before = serial_contents().count(icon_editor_fs_read_marker)
    icon_editor_restart_unregistered_before = serial_contents().count(icon_editor_unregistered_marker)
    icon_editor_restart_exiting_before = serial_contents().count(icon_editor_exiting_marker)
    send_command(stream, "input-icon-editor-restart", "input-send-event", tablet_click(972, 682))
    wait_for_marker_count(icon_editor_clicked_marker, icon_editor_restart_clicked_before + 1, "desktop Editor icon restart request")
    wait_for_marker_count(icon_editor_launch_marker, icon_editor_restart_launch_before + 1, "desktop Editor icon restart acceptance")
    wait_for_marker_count(icon_editor_endpoint_requested_marker, icon_editor_restart_requested_before + 1, "desktop Editor restart endpoint request")
    wait_for_marker_count(icon_editor_endpoint_received_marker, icon_editor_restart_received_before + 1, "desktop Editor restart endpoint registration")
    wait_for_marker_count(mica_registered_marker, icon_editor_restart_registered_before + 1, "desktop Editor restart GUI registration")
    wait_for_marker_count(icon_editor_fs_read_marker, icon_editor_restart_fs_read_before + 1, "desktop Editor restart filesystem load")
    wait_for_marker_count(mica_presented_marker, icon_editor_restart_presented_before + 1, "desktop Editor restart Present")
    send_command(stream, "input-icon-editor-restart-pointer", "input-send-event", pointer_move(900, 700))
    send_command(
        stream,
        "screendump-icon-editor-restart",
        "screendump",
        {"filename": icon_editor_restart_path},
    )
    icon_editor_restart_bytes = screenshot_bytes(icon_editor_restart_path)
    if not icon_editor_restart_bytes:
        raise RuntimeError("desktop Editor restart screenshot is empty")
    send_command(
        stream,
        "input-icon-editor-restart-close",
        "input-send-event",
        key_events(("alt", True), ("f4", True), ("f4", False), ("alt", False)),
    )
    wait_for_marker_count(editor_close_marker, icon_editor_close_before + 2, "desktop Editor restart CloseRequested delivery")
    wait_for_marker_count(icon_editor_unregistered_marker, icon_editor_restart_unregistered_before + 1, "desktop Editor restart endpoint reclaim")
    wait_for_marker_count(icon_editor_exiting_marker, icon_editor_restart_exiting_before + 1, "desktop Editor restart process exit")
    print(json.dumps({
        "desktop_editor_icon": {
            "application": 5,
            "launches": serial_contents().count(icon_editor_clicked_marker) - icon_editor_clicked_before,
            "registrations": serial_contents().count(mica_registered_marker) - icon_editor_registered_before,
            "presents": serial_contents().count(mica_presented_marker) - icon_editor_presented_before,
            "reclaims": serial_contents().count(icon_editor_unregistered_marker) - icon_editor_unregistered_before,
            "screenshots": {
                "pre_crc": f"{zlib.crc32(icon_editor_pre_bytes) & 0xffffffff:08x}",
                "active_crc": f"{zlib.crc32(icon_editor_active_bytes) & 0xffffffff:08x}",
                "restored_crc": f"{zlib.crc32(icon_editor_restored_bytes) & 0xffffffff:08x}",
                "restart_crc": f"{zlib.crc32(icon_editor_restart_bytes) & 0xffffffff:08x}",
            },
        }
    }, sort_keys=True))

    # Finally close the original session at the restored window's close button
    # (the first window keeps its original 420px width after the second/third
    # sessions are reclaimed) and assert endpoint 0 reclamation.
    send_command(
        stream,
        "input-first-close",
        "input-send-event",
        tablet_click(484, 85),
    )
    wait_for_endpoint_marker(
        b"[gui] mica client unregistered endpoint=", 0,
        "first Mica endpoint reclaim",
    )
    try:
        mica_status = mica_process.wait(timeout=3)
    except subprocess.TimeoutExpired:
        mica_process.terminate()
        mica_status = mica_process.wait(timeout=3)
    stdout, stderr = collect_mica_output(1)
    if mica_status != 0:
        raise RuntimeError(
            f"Mica GUI SSH command exited {mica_status}: "
            f"stdout={stdout.decode(errors='replace')!r} "
            f"stderr={stderr.decode(errors='replace')!r}"
        )

collect_all_mica_output()

with open(screenshot_path, "rb") as image:
    data = image.read()

source_format = "PNG"
if data.startswith(b"P6"):
    # QEMU 7.2's screendump command emits a binary PPM (P6), even when the
    # filename uses a .png suffix.  Normalize it to PNG so the artifact has a
    # stable format while retaining strict geometry/pixel checks below.
    source_format = "PPM-P6"

    def ppm_token(blob, index):
        while index < len(blob):
            if blob[index:index + 1] == b"#":
                newline = blob.find(b"\n", index)
                index = len(blob) if newline < 0 else newline + 1
            elif blob[index:index + 1] in b" \t\r\n":
                index += 1
            else:
                break
        start = index
        while index < len(blob) and blob[index:index + 1] not in b" \t\r\n":
            index += 1
        if start == index:
            raise RuntimeError("truncated PPM header")
        return blob[start:index], index

    magic, cursor = ppm_token(data, 0)
    width_token, cursor = ppm_token(data, cursor)
    height_token, cursor = ppm_token(data, cursor)
    maxval_token, cursor = ppm_token(data, cursor)
    if magic != b"P6":
        raise RuntimeError(f"unsupported screendump format: {magic!r}")
    width = int(width_token)
    height = int(height_token)
    maxval = int(maxval_token)
    while cursor < len(data) and data[cursor:cursor + 1] in b" \t\r\n":
        cursor += 1
    pixels = data[cursor:]
    if maxval != 255 or len(pixels) < width * height * 3:
        raise RuntimeError("truncated or unsupported PPM screendump")
    pixels = pixels[:width * height * 3]

    def png_chunk(kind, payload):
        return (
            struct.pack(">I", len(payload))
            + kind
            + payload
            + struct.pack(">I", zlib.crc32(kind + payload) & 0xffffffff)
        )

    rows = b"".join(
        b"\x00" + pixels[row * width * 3:(row + 1) * width * 3]
        for row in range(height)
    )
    data = (
        b"\x89PNG\r\n\x1a\n"
        + png_chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0))
        + png_chunk(b"IDAT", zlib.compress(rows))
        + png_chunk(b"IEND", b"")
    )
    with open(screenshot_path, "wb") as image:
        image.write(data)

if len(data) <= 64 or data[:8] != b"\x89PNG\r\n\x1a\n":
    raise RuntimeError("screendump is not a non-empty PNG or PPM")

offset = 8
width = height = bit_depth = color_type = interlace = None
compressed = bytearray()
while offset + 12 <= len(data):
    size = struct.unpack(">I", data[offset:offset + 4])[0]
    kind = data[offset + 4:offset + 8]
    chunk = data[offset + 8:offset + 8 + size]
    offset += size + 12
    if kind == b"IHDR":
        width, height, bit_depth, color_type, _, _, interlace = struct.unpack(">IIBBBBB", chunk)
    elif kind == b"IDAT":
        compressed.extend(chunk)
    elif kind == b"IEND":
        break

if (width, height) != (1024, 768) or bit_depth != 8 or interlace != 0:
    raise RuntimeError(f"unexpected PNG geometry/format: {(width, height, bit_depth, interlace)}")
channels = {2: 3, 6: 4}.get(color_type)
if channels is None:
    raise RuntimeError(f"unsupported PNG color type: {color_type}")
raw = zlib.decompress(bytes(compressed))
stride = width * channels
expected = height * (stride + 1)
if len(raw) != expected:
    raise RuntimeError(f"unexpected PNG payload length: {len(raw)} != {expected}")

def paeth(a, b, c):
    estimate = a + b - c
    pa, pb, pc = abs(estimate - a), abs(estimate - b), abs(estimate - c)
    if pa <= pb and pa <= pc:
        return a
    if pb <= pc:
        return b
    return c

previous = bytearray(stride)
colors = set()
cursor = 0
for _ in range(height):
    filter_type = raw[cursor]
    encoded = raw[cursor + 1:cursor + 1 + stride]
    cursor += stride + 1
    row = bytearray(encoded)
    for index in range(stride):
        left = row[index - channels] if index >= channels else 0
        up = previous[index]
        up_left = previous[index - channels] if index >= channels else 0
        if filter_type == 1:
            row[index] = (row[index] + left) & 0xff
        elif filter_type == 2:
            row[index] = (row[index] + up) & 0xff
        elif filter_type == 3:
            row[index] = (row[index] + ((left + up) // 2)) & 0xff
        elif filter_type == 4:
            row[index] = (row[index] + paeth(left, up, up_left)) & 0xff
        elif filter_type != 0:
            raise RuntimeError(f"unsupported PNG filter: {filter_type}")
    for x in range(0, width, 16):
        start = x * channels
        colors.add(tuple(row[start:start + channels]))
        if len(colors) >= 2:
            break
    previous = row
    if len(colors) >= 2:
        break
if len(colors) < 2:
    raise RuntimeError("screendump pixels lack visible color diversity")
print(json.dumps({"png": {"bytes": len(data), "width": width, "height": height, "colors_sampled": len(colors), "source_format": source_format}}, sort_keys=True))
PY

python3 - "$pre_screenshot_file" "$screenshot_file" \
  "${screenshot_file%.png}-mica-pre.png" \
  "${screenshot_file%.png}-mica-button.png" \
  "${screenshot_file%.png}-terminal-help.png" \
  "${screenshot_file%.png}-ascii.png" \
  "${screenshot_file%.png}-resized.png" \
  "${screenshot_file%.png}-max.png" \
  "${screenshot_file%.png}-restore.png" \
  "${screenshot_file%.png}-minimized.png" \
  "${screenshot_file%.png}-taskbar-restore.png" \
  "${screenshot_file%.png}-first-before-second.png" \
  "${screenshot_file%.png}-second-pre.png" \
  "${screenshot_file%.png}-second-button.png" \
  "${screenshot_file%.png}-second-text.png" \
  "${screenshot_file%.png}-after-second-close.png" \
  "${screenshot_file%.png}-after-reuse-close.png" \
  "${screenshot_file%.png}-browser-pre.png" \
  "${screenshot_file%.png}-browser-link.png" \
  "${screenshot_file%.png}-browser-cross.png" \
  "${screenshot_file%.png}-editor-pre.png" \
  "${screenshot_file%.png}-editor-apply.png" \
  "${screenshot_file%.png}-editor-saved.png" \
  "${screenshot_file%.png}-editor-reload.png" \
  "$log_file" >>"$qmp_log" <<'PY'
import json
import re
import struct
import sys
import zlib

(
    pre_path,
    post_path,
    mica_pre_path,
    mica_button_path,
    terminal_help_path,
    ascii_path,
    resized_path,
    max_path,
    restore_path,
    minimized_path,
    taskbar_restore_path,
    first_before_second_path,
    second_pre_path,
    second_button_path,
    second_text_path,
    after_second_close_path,
    after_reuse_close_path,
    browser_pre_path,
    browser_link_path,
    browser_cross_path,
    editor_pre_path,
    editor_apply_path,
    editor_saved_path,
    editor_reload_path,
    serial_log_path,
) = sys.argv[1:]

serial = open(serial_log_path, "rb").read()
partial_match = re.search(
    rb"\[gui\] EL0 windowd partial-present=true rect=(\d+)x(\d+)\+(\d+),(\d+)",
    serial,
)
if partial_match is None:
    raise RuntimeError("GUI partial Present marker missing from final serial evidence")
partial_width, partial_height, partial_x, partial_y = (
    int(value) for value in partial_match.groups()
)
if not (
    0 < partial_width <= 1024
    and 0 < partial_height <= 768
    and 0 <= partial_x
    and 0 <= partial_y
    and partial_x + partial_width <= 1024
    and partial_y + partial_height <= 768
    and partial_width * partial_height < 1024 * 768
):
    raise RuntimeError(
        "GUI partial Present rectangle was not a bounded non-full scanout: "
        f"{partial_width}x{partial_height}+{partial_x},{partial_y}"
    )

def ppm_token(blob, index):
    while index < len(blob):
        if blob[index:index + 1] == b"#":
            newline = blob.find(b"\n", index)
            index = len(blob) if newline < 0 else newline + 1
        elif blob[index:index + 1] in b" \t\r\n":
            index += 1
        else:
            break
    start = index
    while index < len(blob) and blob[index:index + 1] not in b" \t\r\n":
        index += 1
    if start == index:
        raise RuntimeError("truncated PPM header")
    return blob[start:index], index

def decode(path):
    data = open(path, "rb").read()
    if data.startswith(b"P6"):
        magic, cursor = ppm_token(data, 0)
        width_token, cursor = ppm_token(data, cursor)
        height_token, cursor = ppm_token(data, cursor)
        maxval_token, cursor = ppm_token(data, cursor)
        width, height, maxval = int(width_token), int(height_token), int(maxval_token)
        while cursor < len(data) and data[cursor:cursor + 1] in b" \t\r\n":
            cursor += 1
        pixels = data[cursor:cursor + width * height * 3]
        if magic != b"P6" or maxval != 255 or len(pixels) != width * height * 3:
            raise RuntimeError(f"invalid PPM screenshot: {path}")
        return width, height, pixels
    if data[:8] != b"\x89PNG\r\n\x1a\n":
        raise RuntimeError(f"unsupported screenshot format: {path}")
    offset = 8
    width = height = bit_depth = color_type = interlace = None
    compressed = bytearray()
    while offset + 12 <= len(data):
        size = struct.unpack(">I", data[offset:offset + 4])[0]
        if offset + size + 12 > len(data):
            raise RuntimeError(f"truncated PNG: {path}")
        kind = data[offset + 4:offset + 8]
        chunk = data[offset + 8:offset + 8 + size]
        offset += size + 12
        if kind == b"IHDR":
            width, height, bit_depth, color_type, _, _, interlace = struct.unpack(">IIBBBBB", chunk)
        elif kind == b"IDAT":
            compressed.extend(chunk)
        elif kind == b"IEND":
            break
    channels = {2: 3, 6: 4}.get(color_type)
    if (width, height, bit_depth, interlace) != (1024, 768, 8, 0) or channels is None:
        raise RuntimeError(f"invalid PNG geometry/format: {path}")
    raw = zlib.decompress(bytes(compressed))
    stride = width * channels
    if len(raw) != height * (stride + 1):
        raise RuntimeError(f"invalid PNG payload: {path}")
    previous = bytearray(stride)
    pixels = bytearray()
    def paeth(a, b, c):
        estimate = a + b - c
        pa, pb, pc = abs(estimate - a), abs(estimate - b), abs(estimate - c)
        return a if pa <= pb and pa <= pc else (b if pb <= pc else c)
    cursor = 0
    for _ in range(height):
        filter_type = raw[cursor]
        row = bytearray(raw[cursor + 1:cursor + 1 + stride])
        cursor += stride + 1
        for index in range(stride):
            left = row[index - channels] if index >= channels else 0
            up = previous[index]
            up_left = previous[index - channels] if index >= channels else 0
            if filter_type == 1:
                row[index] = (row[index] + left) & 0xff
            elif filter_type == 2:
                row[index] = (row[index] + up) & 0xff
            elif filter_type == 3:
                row[index] = (row[index] + ((left + up) // 2)) & 0xff
            elif filter_type == 4:
                row[index] = (row[index] + paeth(left, up, up_left)) & 0xff
            elif filter_type != 0:
                raise RuntimeError(f"unsupported PNG filter: {filter_type}")
        pixels.extend(row)
        previous = row
    if channels == 4:
        pixels = b"".join(pixels[index:index + 3] for index in range(0, len(pixels), 4))
    return width, height, bytes(pixels)

def pixel(image, x, y):
    width, height, pixels = image
    if not (0 <= x < width and 0 <= y < height):
        raise RuntimeError(f"pixel out of range: {(x, y)}")
    offset = (y * width + x) * 3
    return tuple(pixels[offset:offset + 3])

def row_color_count(image, y, color):
    width, _, pixels = image
    offset = y * width * 3
    return sum(
        tuple(pixels[offset + x * 3:offset + x * 3 + 3]) == color
        for x in range(width)
    )

def terminal_text_rows(image):
    width, _, pixels = image
    foreground = (0xd9, 0xe6, 0xf7)
    rows = set()
    for y in range(100, 500):
        for x in range(70, 700):
            offset = (y * width + x) * 3
            if tuple(pixels[offset:offset + 3]) == foreground:
                rows.add(y)
                break
    return rows

snapshots = {
    "desktop_pre": decode(pre_path),
    "mica_pre": decode(mica_pre_path),
    "mica_button": decode(mica_button_path),
    "terminal_help": decode(terminal_help_path),
    "mica_text": decode(post_path),
    "ascii": decode(ascii_path),
    "resized": decode(resized_path),
    "max": decode(max_path),
    "restore": decode(restore_path),
    "minimized": decode(minimized_path),
    "taskbar_restore": decode(taskbar_restore_path),
    "first_before_second": decode(first_before_second_path),
    "second_pre": decode(second_pre_path),
    "second_button": decode(second_button_path),
    "second_text": decode(second_text_path),
    "after_second_close": decode(after_second_close_path),
    "after_reuse_close": decode(after_reuse_close_path),
    "browser_pre": decode(browser_pre_path),
    "browser_link": decode(browser_link_path),
    "browser_cross": decode(browser_cross_path),
    "editor_pre": decode(editor_pre_path),
    "editor_apply": decode(editor_apply_path),
    "editor_saved": decode(editor_saved_path),
    "editor_reload": decode(editor_reload_path),
}
dimensions = {name: image[:2] for name, image in snapshots.items()}
if any(size != (1024, 768) for size in dimensions.values()):
    raise RuntimeError(f"unexpected screenshot geometry: {dimensions}")
crcs = {
    name: f"{zlib.crc32(image[2]) & 0xffffffff:08x}"
    for name, image in snapshots.items()
}
if crcs["desktop_pre"] == crcs["mica_text"]:
    raise RuntimeError("Alt-Tab/Mica launch did not change decoded pixels")
if crcs["mica_pre"] == crcs["mica_button"]:
    raise RuntimeError("Counter button click did not change decoded pixels")
if crcs["mica_button"] == crcs["ascii"]:
    raise RuntimeError("Counter ASCII text input did not change decoded pixels")
terminal_help_rows = terminal_text_rows(snapshots["terminal_help"])
if len(terminal_help_rows) < 7:
    raise RuntimeError(
        "Terminal help response did not render the full shared-frame output: "
        f"foreground_rows={len(terminal_help_rows)}"
    )
if crcs["ascii"] == crcs["mica_text"]:
    raise RuntimeError("Counter Unicode input did not change decoded pixels")
if crcs["second_pre"] == crcs["second_button"]:
    raise RuntimeError("second Counter button click did not change decoded pixels")
if crcs["second_button"] == crcs["second_text"]:
    raise RuntimeError("second Counter text input did not change decoded pixels")
if crcs["second_text"] == crcs["after_second_close"]:
    raise RuntimeError("closing second GUI session did not restore first-window pixels")
if crcs["browser_pre"] == crcs["browser_link"]:
    raise RuntimeError("Mica Reader same-origin link did not change decoded pixels")
if crcs["browser_link"] == crcs["browser_cross"]:
    raise RuntimeError("Mica Reader cross-origin rejection redraw did not change decoded pixels")
if crcs["editor_pre"] == crcs["editor_apply"]:
    raise RuntimeError("Mica Editor Apply did not change decoded pixels")
if crcs["editor_apply"] == crcs["editor_saved"]:
    raise RuntimeError("Mica Editor Save did not change decoded pixels")
if crcs["editor_saved"] == crcs["editor_reload"]:
    raise RuntimeError("Mica Editor Reload did not change decoded pixels")
def crop_crc(image, x0, y0, x1, y1):
    width, height, pixels = image
    if not (0 <= x0 < x1 <= width and 0 <= y0 < y1 <= height):
        raise RuntimeError(f"invalid stable-client crop: {(x0, y0, x1, y1)}")
    rows = []
    for y in range(y0, y1):
        start = (y * width + x0) * 3
        rows.append(pixels[start:start + (x1 - x0) * 3])
    return f"{zlib.crc32(b''.join(rows)) & 0xffffffff:08x}"

# Compare only the document list rows, excluding the status line and title.
# Equality after Reload is the guest-observable persistence check for the
# edited line; the surrounding full-frame CRCs still cover each UI action.
# Compare the first document row, which is unaffected by the selected-row
# highlight and still proves Reload preserved the surrounding file content.
editor_content_crop = (82, 164, 820, 190)
editor_apply_content_crc = crop_crc(snapshots["editor_apply"], *editor_content_crop)
editor_reload_content_crc = crop_crc(snapshots["editor_reload"], *editor_content_crop)
if editor_apply_content_crc != editor_reload_content_crc:
    raise RuntimeError(
        "Mica Editor reload changed visible document content: "
        f"apply={editor_apply_content_crc} reload={editor_reload_content_crc}"
    )

# The pointer is deliberately parked at the same client coordinate for all
# captures. The title bar's active/inactive color legitimately changes when
# focus returns from the second client, so compare stable client content below
# the title bar rather than treating focus styling as data corruption.
stable_crop = (80, 110, 478, 382)
taskbar_client_crc = crop_crc(snapshots["taskbar_restore"], *stable_crop)
baseline_client_crc = crop_crc(snapshots["first_before_second"], *stable_crop)
after_close_client_crc = crop_crc(snapshots["after_second_close"], *stable_crop)
after_reuse_client_crc = crop_crc(snapshots["after_reuse_close"], *stable_crop)
if not (
    taskbar_client_crc == baseline_client_crc == after_close_client_crc == after_reuse_client_crc
):
    raise RuntimeError(
        "second GUI session changed first-window client crop: "
        f"taskbar={taskbar_client_crc} baseline={baseline_client_crc} "
        f"after={after_close_client_crc} reuse={after_reuse_client_crc}"
    )

# The active title color is stable in the windowd renderer.  A maximized
# dynamic window covers nearly the full scanout; restored geometry is the
# requested 420x260 client; minimized content disappears while its taskbar
# button remains active.  These bounds are deliberately broad around the
# exact title text/button pixels.
active_title = (0xf7, 0xf9, 0xfc)
max_run = row_color_count(snapshots["max"], 10, active_title)
resized_run = row_color_count(snapshots["resized"], 80, active_title)
restore_run = row_color_count(snapshots["restore"], 80, active_title)
minimized_run = row_color_count(snapshots["minimized"], 90, active_title)
taskbar_run = row_color_count(snapshots["taskbar_restore"], 750, active_title)
if max_run < 900:
    raise RuntimeError(f"maximize geometry evidence too narrow: active-title pixels={max_run}")
if resized_run < 400:
    raise RuntimeError(f"resize geometry evidence too narrow: active-title pixels={resized_run}")
if abs(restore_run - resized_run) > 12:
    raise RuntimeError(
        f"restore did not recover resized geometry: resized={resized_run} restore={restore_run}"
    )
if minimized_run > 20:
    raise RuntimeError(f"minimize geometry evidence still shows content title: pixels={minimized_run}")
if taskbar_run < 80:
    raise RuntimeError(f"taskbar restore evidence missing active task button: pixels={taskbar_run}")
print(json.dumps({
    "decoded_pixels": {**crcs, "changed": True},
    "geometry": {
        "max_active_title_row10": max_run,
        "resized_active_title_row80": resized_run,
        "restore_active_title_row80": restore_run,
        "minimized_active_title_row90": minimized_run,
        "taskbar_active_row750": taskbar_run,
        "terminal_help_foreground_rows": len(terminal_help_rows),
        "partial_present": {
            "width": partial_width,
            "height": partial_height,
            "x": partial_x,
            "y": partial_y,
        },
        "stable_first_client_before_second": taskbar_client_crc,
        "stable_first_client_after_second": after_close_client_crc,
        "stable_first_client_baseline": baseline_client_crc,
        "stable_first_client_after_reuse": after_reuse_client_crc,
    },
}, sort_keys=True))
PY

if grep -Eqi '\[panic\]|DMA fault' "$log_file"; then
  echo "gui-qemu: panic or DMA fault appeared in serial log" >&2
  tail -n 160 "$log_file" >&2 || true
  exit 1
fi

echo "gui-qemu: PASS serial-log=$log_file qmp-log=$qmp_log pre-screenshot=$pre_screenshot_file post-screenshot=$screenshot_file"
