#!/usr/bin/env bash
set -Ee -o pipefail

repo_root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$repo_root"

timeout_seconds="$MICROSYSTEM_SSH_QEMU_TIMEOUT"
[[ -n "$timeout_seconds" ]] || timeout_seconds=45
ssh_port="$SSH_PORT"
[[ -n "$ssh_port" ]] || ssh_port=2223
gui_port="$GUI_PORT"
[[ -n "$gui_port" ]] || gui_port=5901
log_file="$MICROSYSTEM_SSH_QEMU_LOG"
[[ -n "$log_file" ]] || log_file="$repo_root/target/ssh-qemu.log"
ssh_output="$MICROSYSTEM_SSH_OUTPUT"
[[ -n "$ssh_output" ]] || ssh_output="$repo_root/target/ssh-uptime.out"
ssh_error="$MICROSYSTEM_SSH_ERROR"
[[ -n "$ssh_error" ]] || ssh_error="$repo_root/target/ssh-uptime.err"
bad_output="$MICROSYSTEM_SSH_BAD_OUTPUT"
[[ -n "$bad_output" ]] || bad_output="$repo_root/target/ssh-bad-key.out"
no_key_output="$MICROSYSTEM_SSH_NO_KEY_OUTPUT"
[[ -n "$no_key_output" ]] || no_key_output="$repo_root/target/ssh-no-key.out"
mica_eval_output="$MICROSYSTEM_SSH_MICA_EVAL_OUTPUT"
[[ -n "$mica_eval_output" ]] || mica_eval_output="$repo_root/target/ssh-mica-eval.out"
mica_repl_output="$MICROSYSTEM_SSH_MICA_REPL_OUTPUT"
[[ -n "$mica_repl_output" ]] || mica_repl_output="$repo_root/target/ssh-mica-repl.out"
mica_file_output="$MICROSYSTEM_SSH_MICA_FILE_OUTPUT"
[[ -n "$mica_file_output" ]] || mica_file_output="$repo_root/target/ssh-mica-file.out"
mica_denied_output="$MICROSYSTEM_SSH_MICA_DENIED_OUTPUT"
[[ -n "$mica_denied_output" ]] || mica_denied_output="$repo_root/target/ssh-mica-denied.out"
mica_disconnect_output="$MICROSYSTEM_SSH_MICA_DISCONNECT_OUTPUT"
[[ -n "$mica_disconnect_output" ]] || mica_disconnect_output="$repo_root/target/ssh-mica-disconnect.out"
mica_disconnect_error="$MICROSYSTEM_SSH_MICA_DISCONNECT_ERROR"
[[ -n "$mica_disconnect_error" ]] || mica_disconnect_error="$repo_root/target/ssh-mica-disconnect.err"
pcap_file="$MICROSYSTEM_SSH_PCAP"
[[ -n "$pcap_file" ]] || pcap_file="$repo_root/target/ssh-qemu.pcap"
trace_file="$MICROSYSTEM_SSH_TRACE"
[[ -n "$trace_file" ]] || trace_file="$repo_root/target/ssh-qemu.trace"
trace_events="$trace_file.events"
qmp_socket="$MICROSYSTEM_SSH_QMP_SOCKET"
[[ -n "$qmp_socket" ]] || qmp_socket="$repo_root/target/ssh-qemu.sock"
qmp_log="$MICROSYSTEM_SSH_QMP_LOG"
[[ -n "$qmp_log" ]] || qmp_log="$repo_root/target/ssh-qemu.qmp.jsonl"
diagnostic="$MICROSYSTEM_SSH_DIAGNOSTIC"
[[ -n "$diagnostic" ]] || diagnostic=0

if ! [[ "$timeout_seconds" =~ ^[0-9]+$ ]] || (( timeout_seconds == 0 )); then
  echo "ssh-qemu: MICROSYSTEM_SSH_QEMU_TIMEOUT must be a positive integer" >&2
  exit 2
fi
if ! [[ "$ssh_port" =~ ^[0-9]+$ ]] || (( ssh_port < 1 || ssh_port > 65535 )); then
  echo "ssh-qemu: SSH_PORT must be a valid TCP port" >&2
  exit 2
fi
if ! [[ "$gui_port" =~ ^[0-9]+$ ]] || (( gui_port < 1 || gui_port > 65535 )); then
  echo "ssh-qemu: GUI_PORT must be a valid TCP port" >&2
  exit 2
fi

inside_container=0
if [[ -f /.dockerenv || "$MICROSYSTEM_IN_CONTAINER" == "1" ]]; then
  inside_container=1
fi
if (( inside_container == 0 )); then
  if ! command -v docker >/dev/null 2>&1 || ! docker info >/dev/null 2>&1; then
    echo "ssh-qemu: OrbStack Docker is required" >&2
    exit 2
  fi
  image="$IMAGE"
  [[ -n "$image" ]] || image=microsystem-dev:rust-1.97.1
  inner_log="/workspace/target/$(basename "$log_file")"
  inner_output="/workspace/target/$(basename "$ssh_output")"
  inner_error="/workspace/target/$(basename "$ssh_error")"
  inner_bad="/workspace/target/$(basename "$bad_output")"
  inner_no_key="/workspace/target/$(basename "$no_key_output")"
  inner_mica_eval="/workspace/target/$(basename "$mica_eval_output")"
  inner_mica_repl="/workspace/target/$(basename "$mica_repl_output")"
  inner_mica_file="/workspace/target/$(basename "$mica_file_output")"
  inner_mica_denied="/workspace/target/$(basename "$mica_denied_output")"
  inner_mica_disconnect="/workspace/target/$(basename "$mica_disconnect_output")"
  inner_mica_disconnect_error="/workspace/target/$(basename "$mica_disconnect_error")"
  inner_pcap="/workspace/target/$(basename "$pcap_file")"
  inner_trace="/workspace/target/$(basename "$trace_file")"
  inner_qmp="/workspace/target/$(basename "$qmp_socket")"
  inner_qmp_log="/workspace/target/$(basename "$qmp_log")"
  exec docker run --rm -i --init \
    -e RUSTUP_TOOLCHAIN=1.97.1 -e MICROSYSTEM_IN_CONTAINER=1 \
    -e SSH_PORT="$ssh_port" -e GUI_PORT="$gui_port" \
    -e MICROSYSTEM_SSH_QEMU_TIMEOUT="$timeout_seconds" \
    -e MICROSYSTEM_SSH_QEMU_LOG="$inner_log" \
    -e MICROSYSTEM_SSH_OUTPUT="$inner_output" \
    -e MICROSYSTEM_SSH_ERROR="$inner_error" \
    -e MICROSYSTEM_SSH_BAD_OUTPUT="$inner_bad" \
    -e MICROSYSTEM_SSH_NO_KEY_OUTPUT="$inner_no_key" \
    -e MICROSYSTEM_SSH_MICA_EVAL_OUTPUT="$inner_mica_eval" \
    -e MICROSYSTEM_SSH_MICA_REPL_OUTPUT="$inner_mica_repl" \
    -e MICROSYSTEM_SSH_MICA_FILE_OUTPUT="$inner_mica_file" \
    -e MICROSYSTEM_SSH_MICA_DENIED_OUTPUT="$inner_mica_denied" \
    -e MICROSYSTEM_SSH_MICA_DISCONNECT_OUTPUT="$inner_mica_disconnect" \
    -e MICROSYSTEM_SSH_MICA_DISCONNECT_ERROR="$inner_mica_disconnect_error" \
    -e MICROSYSTEM_SSH_PCAP="$inner_pcap" \
    -e MICROSYSTEM_SSH_TRACE="$inner_trace" \
    -e MICROSYSTEM_SSH_QMP_SOCKET="$inner_qmp" \
    -e MICROSYSTEM_SSH_QMP_LOG="$inner_qmp_log" \
    -e MICROSYSTEM_SSH_DIAGNOSTIC="$diagnostic" \
    -v "$repo_root:/workspace" -w /workspace "$image" bash scripts/ssh-qemu.sh
fi

for command_name in qemu-system-aarch64 ssh ssh-keygen timeout python3; do
  if ! command -v "$command_name" >/dev/null 2>&1; then
    echo "ssh-qemu: $command_name is required inside the OrbStack container" >&2
    exit 2
  fi
done
if [[ ! -f target/aarch64-unknown-none-softfloat/release/microsystem-kernel ||
      ! -f build/microsystem.img || ! -f build/ssh/id_ed25519 ||
      ! -x target/release/mfsctl ]]; then
  echo "ssh-qemu: build artifacts are missing; run make build first" >&2
  exit 2
fi

mkdir -p "$(dirname "$log_file")" "$(dirname "$ssh_output")" "$(dirname "$trace_file")" "$(dirname "$qmp_log")" \
  "$(dirname "$mica_eval_output")" "$(dirname "$mica_repl_output")" \
  "$(dirname "$mica_file_output")" "$(dirname "$mica_denied_output")" \
  "$(dirname "$mica_disconnect_output")" "$(dirname "$mica_disconnect_error")"
: >"$log_file"
: >"$ssh_output"
: >"$ssh_error"
: >"$bad_output"
: >"$no_key_output"
: >"$mica_eval_output"
: >"$mica_repl_output"
: >"$mica_file_output"
: >"$mica_denied_output"
: >"$mica_disconnect_output"
: >"$mica_disconnect_error"
rm -f "$pcap_file"
rm -f "$qmp_socket" "$qmp_log"
printf '%s\n' virtio_queue_notify virtqueue_pop virtqueue_fill virtqueue_flush >"$trace_events"

fixture_dir="$repo_root/target/ssh-mica-fixture"
mkdir -p "$fixture_dir"
printf '%s' 'ssh-mica-ok' >"$fixture_dir/payload.txt"
cat >"$fixture_dir/allowed.mica" <<'MICA'
--!mica 1
--!allow fs.read:/data

local fs = require("fs")
local bytes = require("bytes")
local value, read_error = fs.read_file("/data/ssh-mica-payload")
if value == nil then
  error(read_error.message)
end
print("ssh-mica-file=" + bytes.to_string(value))
MICA
if ! target/release/mfsctl mkdir build/microsystem.img /data >"$fixture_dir/mkdir.log" 2>&1; then
  :
fi
if ! target/release/mfsctl put build/microsystem.img "$fixture_dir/payload.txt" /data/ssh-mica-payload >"$fixture_dir/payload.log" 2>&1; then
  echo "ssh-qemu: failed to install Mica SSH filesystem fixture" >&2
  cat "$fixture_dir/payload.log" >&2 || true
  exit 1
fi
if ! target/release/mfsctl put build/microsystem.img "$fixture_dir/allowed.mica" /data/ssh-mica.mica >"$fixture_dir/script.log" 2>&1; then
  echo "ssh-qemu: failed to install Mica SSH script fixture" >&2
  cat "$fixture_dir/script.log" >&2 || true
  exit 1
fi

qemu_pid=""
wrong_key=""
cleanup() {
  status=$?
  if [[ -n "$qemu_pid" ]] && kill -0 "$qemu_pid" 2>/dev/null; then
    kill -TERM "$qemu_pid" 2>/dev/null || true
    sleep 1
    kill -KILL "$qemu_pid" 2>/dev/null || true
    wait "$qemu_pid" 2>/dev/null || true
  fi
  if [[ -n "$wrong_key" ]]; then
    rm -f "$wrong_key" "$wrong_key.pub"
  fi
  exit "$status"
}
trap cleanup EXIT INT TERM

setsid qemu-system-aarch64 \
  -machine virt-7.2,virtualization=on,gic-version=3,iommu=smmuv3 \
  -cpu cortex-a72 -accel tcg,thread=multi -smp 2 -m 256M -nographic \
  -L /usr/lib/ipxe/qemu -no-reboot -semihosting-config enable=on,target=native \
  -kernel target/aarch64-unknown-none-softfloat/release/microsystem-kernel \
  -drive "if=none,file=$repo_root/build/microsystem.img,format=raw,cache=writeback,id=disk0" \
  -device virtio-blk-pci,drive=disk0,disable-legacy=on,iommu_platform=on,romfile=,addr=2 \
  -netdev "user,id=net0,restrict=on,hostfwd=tcp:127.0.0.1:$ssh_port-10.0.2.15:22" \
  -device virtio-net-pci,netdev=net0,disable-legacy=on,iommu_platform=on,romfile=,addr=6,mac=52:54:00:12:34:56 \
  -object "filter-dump,id=netdump,netdev=net0,file=$pcap_file" \
  -trace "events=$trace_events,file=$trace_file" \
  -qmp "unix:$qmp_socket,server=on,wait=off" \
  -object rng-random,filename=/dev/urandom,id=rng0 \
  -device virtio-rng-pci,rng=rng0,disable-legacy=on,romfile=,addr=7 \
  >"$log_file" 2>&1 &
qemu_pid=$!

required_markers='[bootfs] valid=true entries=24 static-elfs=23
[service] resident EL0 address-spaces=12 asids=[0x20..0x2b]
[service] resident EL0 ready=8/8 online=8/8 switches=
[proc] dynamic application capacity=8 first-pid=13 independent-slots=true
[net] netd ready ipv4=10.0.2.15 outbound=true raw-device-isolated=true
[ssh] sshd ready address=10.0.2.15 port=22 auth=publickey user=micro'
fatal_re='\[panic\]|DMA fault|translation fault|gerror=0x[1-9a-f][0-9a-f]*|allocator fault'
deadline=$((SECONDS + timeout_seconds))
while (( SECONDS < deadline )); do
  if grep -Eiq "$fatal_re" "$log_file"; then
    echo "ssh-qemu: fatal panic/DMA/translation marker appeared" >&2
    tail -n 160 "$log_file" >&2 || true
    exit 1
  fi
  all_ready=1
  while IFS= read -r marker; do
    [[ -z "$marker" ]] && continue
    if ! grep -Fq -- "$marker" "$log_file"; then
      all_ready=0
      break
    fi
  done <<< "$required_markers"
  (( all_ready == 1 )) && break
  if ! kill -0 "$qemu_pid" 2>/dev/null; then
    echo "ssh-qemu: QEMU exited before SSH markers were complete" >&2
    tail -n 160 "$log_file" >&2 || true
    exit 1
  fi
  sleep 0.2
done

missing=""
while IFS= read -r marker; do
  [[ -z "$marker" ]] && continue
  if ! grep -Fq -- "$marker" "$log_file"; then
    missing="$missing | $marker"
  fi
done <<< "$required_markers"
if [[ -n "$missing" ]]; then
  echo "ssh-qemu: missing serial markers:$missing" >&2
  tail -n 160 "$log_file" >&2 || true
  exit 1
fi

qmp_deadline=$((SECONDS + timeout_seconds))
while [[ ! -S "$qmp_socket" && SECONDS -lt qmp_deadline ]]; do
  sleep 0.1
done
if [[ ! -S "$qmp_socket" ]]; then
  echo "ssh-qemu: QMP socket was not created: $qmp_socket" >&2
  exit 1
fi

collect_qmp() {
  phase="$1"
  python3 - "$qmp_socket" "$qmp_log" "$phase" <<'PY'
import json
import socket
import sys

socket_path, log_path, phase = sys.argv[1:]
next_id = 1

def write_log(value):
    with open(log_path, "a", encoding="utf-8") as output:
        output.write(json.dumps({"phase": phase, **value}, sort_keys=True) + "\n")

def command(stream, name, execute, arguments=None):
    global next_id
    command_id = next_id
    next_id += 1
    request = {"execute": execute, "id": command_id}
    if arguments is not None:
        request["arguments"] = arguments
    stream.write((json.dumps(request) + "\r\n").encode())
    while True:
        line = stream.readline()
        if not line:
            raise RuntimeError("QMP closed")
        reply = json.loads(line.decode())
        if reply.get("id") == command_id:
            write_log({"command": execute, "arguments": arguments, "reply": reply})
            return reply

def paths(value):
    found = []
    if isinstance(value, dict):
        candidate = value.get("path")
        if isinstance(candidate, str):
            found.append(candidate)
        for child in value.values():
            found.extend(paths(child))
    elif isinstance(value, list):
        for child in value:
            found.extend(paths(child))
    return found

def named_paths(value, wanted):
    found = []
    if isinstance(value, dict):
        candidate = value.get("path")
        if value.get("name") in wanted and isinstance(candidate, str):
            found.append(candidate)
        for child in value.values():
            found.extend(named_paths(child, wanted))
    elif isinstance(value, list):
        for child in value:
            found.extend(named_paths(child, wanted))
    return found

with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as sock:
    sock.settimeout(5.0)
    sock.connect(socket_path)
    stream = sock.makefile("rwb", buffering=0)
    greeting = json.loads(stream.readline().decode())
    write_log({"command": "greeting", "reply": greeting})
    command(stream, "capabilities", "qmp_capabilities")
    command(stream, "status", "query-status")
    virtio = command(stream, "virtio", "x-query-virtio")
    payload = virtio.get("return", virtio)
    all_paths = sorted(set(paths(payload)))
    write_log({"command": "virtio-paths", "reply": {"paths": all_paths}})
    net_paths = sorted(set(named_paths(payload, {"virtio-net"})))
    for path in net_paths:
        command(stream, "virtio-status", "x-query-virtio-status", {"path": path})
        for queue in (0, 1):
            command(
                stream,
                "virtio-queue-status",
                "x-query-virtio-queue-status",
                {"path": path, "queue": queue},
            )
PY
}

if [[ "$diagnostic" == "1" ]]; then
  collect_qmp pre
fi

deadline=$((SECONDS + timeout_seconds))
ssh_status=255
if [[ "$diagnostic" == "1" ]]; then
  set +e
  timeout 8s ssh -F /dev/null -T -o BatchMode=yes -o IdentitiesOnly=yes \
    -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null \
    -o ConnectTimeout=5 -o ConnectionAttempts=1 -p "$ssh_port" \
    -i build/ssh/id_ed25519 micro@127.0.0.1 uptime >"$ssh_output" 2>"$ssh_error"
  ssh_status=$?
  set -e
  collect_qmp post
else
  while (( SECONDS < deadline )); do
    set +e
    timeout 8s ssh -F /dev/null -T -o BatchMode=yes -o IdentitiesOnly=yes \
      -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null \
      -o ConnectTimeout=5 -o ConnectionAttempts=1 -p "$ssh_port" \
      -i build/ssh/id_ed25519 micro@127.0.0.1 uptime >"$ssh_output" 2>"$ssh_error"
    ssh_status=$?
    set -e
    (( ssh_status == 0 )) && break
    sleep 0.2
  done
fi
if (( ssh_status != 0 )); then
  echo "ssh-qemu: authorized-key uptime failed exit=$ssh_status" >&2
  cat "$ssh_error" >&2 || true
  tail -n 160 "$log_file" >&2 || true
  exit 1
fi
if ! grep -Eq '^uptime:[[:space:]]+[0-9]+[[:space:]]+ms' "$ssh_output"; then
  echo "ssh-qemu: authorized-key output lacks uptime: $(cat "$ssh_output")" >&2
  exit 1
fi

ssh_authorized_exec() {
  local duration="$1"
  shift
  timeout "${duration}s" ssh -F /dev/null -T -o BatchMode=yes -o IdentitiesOnly=yes \
    -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null \
    -o ConnectTimeout=5 -o ConnectionAttempts=1 -p "$ssh_port" \
    -i build/ssh/id_ed25519 micro@127.0.0.1 "$@"
}

set +e
ssh_authorized_exec 12 "mica -e 'print(40 + 2)'" >"$mica_eval_output" 2>"$ssh_error"
mica_eval_status=$?
set -e
if (( mica_eval_status != 0 )); then
  echo "ssh-qemu: authorized-key Mica eval failed exit=$mica_eval_status" >&2
  cat "$ssh_error" >&2 || true
  cat "$mica_eval_output" >&2 || true
  exit 1
fi
if ! grep -Eq '(^|[^0-9])42([^0-9]|$)' "$mica_eval_output"; then
  echo "ssh-qemu: Mica eval output lacks 42: $(cat "$mica_eval_output")" >&2
  exit 1
fi

set +e
printf 'mica\nprint(6 * 7)\nexit\n' | ssh_authorized_exec 18 >"$mica_repl_output" 2>"$ssh_error"
mica_repl_status=${PIPESTATUS[1]}
set -e
if (( mica_repl_status != 0 )); then
  echo "ssh-qemu: SSH Mica REPL failed exit=$mica_repl_status" >&2
  cat "$ssh_error" >&2 || true
  cat "$mica_repl_output" >&2 || true
  exit 1
fi
if ! grep -Eq 'MicroSystem SSH|micro>[[:space:]]+mica|mica>[[:space:]]+print\(6 \* 7\)' "$mica_repl_output" \
    || ! grep -Eq '(^|[^0-9])42([^0-9]|$)' "$mica_repl_output" \
    || ! grep -Eq 'micro>[[:space:]]*$' "$mica_repl_output"; then
  echo "ssh-qemu: SSH Mica REPL markers missing" >&2
  cat "$mica_repl_output" >&2 || true
  exit 1
fi

set +e
ssh_authorized_exec 18 "mica --allow fs.read:/data /data/ssh-mica.mica" \
  >"$mica_file_output" 2>"$ssh_error"
mica_file_status=$?
set -e
if (( mica_file_status != 0 )); then
  echo "ssh-qemu: SSH Mica filesystem script failed exit=$mica_file_status" >&2
  cat "$ssh_error" >&2 || true
  cat "$mica_file_output" >&2 || true
  exit 1
fi
if ! grep -Eq 'ssh-mica-file=ssh-mica-ok' "$mica_file_output"; then
  echo "ssh-qemu: SSH Mica filesystem payload marker missing" >&2
  cat "$mica_file_output" >&2 || true
  exit 1
fi

denied_command="mica --allow fs.read:/secret -e 'local f=require(\"fs\");local v,e=f.read_file(\"/secret\");if v==nil and e~=nil then print(\"ssh-mica-denied kind=\"+e.kind)else error(\"bad\")end'"
if (( ${#denied_command} >= 256 )); then
  echo "ssh-qemu: denied-policy probe command exceeds SSH parser limit (${#denied_command} bytes)" >&2
  exit 1
fi
set +e
ssh_authorized_exec 12 "$denied_command" >"$mica_denied_output" 2>"$ssh_error"
mica_denied_status=$?
set -e
if (( mica_denied_status != 0 )); then
  echo "ssh-qemu: SSH Mica denied-policy probe failed exit=$mica_denied_status" >&2
  cat "$ssh_error" >&2 || true
  cat "$mica_denied_output" >&2 || true
  exit 1
fi
if ! grep -Eq 'ssh-mica-denied kind=access' "$mica_denied_output"; then
  echo "ssh-qemu: SSH Mica denied-policy marker missing" >&2
  cat "$mica_denied_output" >&2 || true
  exit 1
fi

long_command="mica -e 'local time = require(\"time\"); time.sleep(5000); print(\"ssh-long-done\")'"
set +e
ssh_authorized_exec 2 "$long_command" >"$mica_disconnect_output" 2>"$mica_disconnect_error"
mica_disconnect_status=$?
set -e
if (( mica_disconnect_status == 0 )); then
  echo "ssh-qemu: long Mica disconnect probe unexpectedly completed" >&2
  cat "$mica_disconnect_output" >&2 || true
  exit 1
fi
set +e
ssh_authorized_exec 12 uptime >"$ssh_output" 2>"$ssh_error"
recovery_status=$?
set -e
if (( recovery_status != 0 )) || ! grep -Eq '^uptime:[[:space:]]+[0-9]+[[:space:]]+ms' "$ssh_output"; then
  echo "ssh-qemu: SSH recovery after Mica disconnect failed exit=$recovery_status" >&2
  cat "$ssh_error" >&2 || true
  cat "$ssh_output" >&2 || true
  exit 1
fi

wrong_key="$(mktemp "$repo_root/target/ssh-wrong-key.XXXXXX")"
rm -f "$wrong_key"
ssh-keygen -q -t ed25519 -N "" -f "$wrong_key" >/dev/null
set +e
timeout 8s ssh -F /dev/null -T -o BatchMode=yes -o IdentitiesOnly=yes \
  -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null \
  -o ConnectTimeout=5 -o ConnectionAttempts=1 -p "$ssh_port" \
  -i "$wrong_key" micro@127.0.0.1 uptime >"$bad_output" 2>&1
bad_status=$?
set -e
if (( bad_status == 0 )); then
  echo "ssh-qemu: wrong-key authentication unexpectedly succeeded" >&2
  cat "$bad_output" >&2 || true
  exit 1
fi

set +e
timeout 8s ssh -F /dev/null -T -o BatchMode=yes -o IdentitiesOnly=yes \
  -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null \
  -o ConnectTimeout=5 -o ConnectionAttempts=1 -o IdentityFile=/dev/null \
  -p "$ssh_port" micro@127.0.0.1 uptime >"$no_key_output" 2>&1
no_key_status=$?
set -e
if (( no_key_status == 0 )); then
  echo "ssh-qemu: no-key authentication unexpectedly succeeded" >&2
  cat "$no_key_output" >&2 || true
  exit 1
fi

echo "ssh-qemu: PASS ssh-port=$ssh_port gui-port=$gui_port uptime=$(tr -d '\r\n' <"$ssh_output") mica-eval-status=$mica_eval_status mica-repl-status=$mica_repl_status mica-file-status=$mica_file_status mica-denied-status=$mica_denied_status mica-disconnect-exit=$mica_disconnect_status recovery-status=$recovery_status wrong-key-exit=$bad_status no-key-exit=$no_key_status"
echo "ssh-qemu: log=$log_file authorized-output=$ssh_output authorized-error=$ssh_error mica-eval=$mica_eval_output mica-repl=$mica_repl_output mica-file=$mica_file_output mica-denied=$mica_denied_output mica-disconnect=$mica_disconnect_output"
