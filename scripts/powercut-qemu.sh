#!/usr/bin/env bash
set -Eeuo pipefail

repo_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

timeout_seconds="${MICROSYSTEM_QEMU_TIMEOUT:-45}"
input_delay="${MICROSYSTEM_COMMAND_DELAY:-1}"
startup_delay="${MICROSYSTEM_POWERCUT_START_DELAY:-2}"
if ! [[ "$timeout_seconds" =~ ^[0-9]+$ ]]; then
  echo "powercut-qemu: MICROSYSTEM_QEMU_TIMEOUT must be an integer" >&2
  exit 2
fi

if ! command -v qemu-system-aarch64 >/dev/null 2>&1; then
  echo "powercut-qemu: qemu-system-aarch64 is required" >&2
  exit 2
fi
if [[ ! -f "target/aarch64-unknown-none-softfloat/release/microsystem-kernel" ]]; then
  echo "powercut-qemu: built kernel ELF is missing" >&2
  exit 2
fi
if [[ ! -f build/microsystem.img ]]; then
  echo "powercut-qemu: build/microsystem.img is missing" >&2
  exit 2
fi

token="powercut-$(date +%s)-$$-${RANDOM:-0}"
first_log="${MICROSYSTEM_POWERCUT_FIRST_LOG:-$repo_root/target/${token}-first.log}"
second_log="${MICROSYSTEM_POWERCUT_SECOND_LOG:-$repo_root/target/${token}-second.log}"
mkdir -p "$(dirname -- "$first_log")" "$(dirname -- "$second_log")"

qemu_command=(
  qemu-system-aarch64
  -machine virt-7.2,virtualization=on,gic-version=3,iommu=smmuv3
  -cpu cortex-a72
  -accel tcg,thread=multi
  -smp 2
  -m 256M
  -nographic
  -L /usr/lib/ipxe/qemu
  -no-reboot
  -semihosting-config enable=on,target=native
  -kernel "$repo_root/target/aarch64-unknown-none-softfloat/release/microsystem-kernel"
  -drive "if=none,file=$repo_root/build/microsystem.img,format=raw,cache=writeback,id=disk0"
  -device virtio-blk-pci,drive=disk0,disable-legacy=on,iommu_platform=on,romfile=,addr=2
  -netdev user,id=net0,restrict=on
  -device virtio-net-pci,netdev=net0,disable-legacy=on,iommu_platform=on,romfile=,addr=6,mac=52:54:00:12:34:56
  -object rng-random,filename=/dev/urandom,id=rng0
  -device virtio-rng-pci,rng=rng0,disable-legacy=on,romfile=,addr=7
)

first_pid=""
cleanup() {
  if [[ -n "$first_pid" ]] && kill -0 "$first_pid" 2>/dev/null; then
    kill -KILL "$first_pid" 2>/dev/null || true
  fi
}
trap cleanup EXIT INT TERM

send_first_input() {
  sleep "$startup_delay"
  printf 'write /powercut %s\n' "$token"
  sleep "$input_delay"
  printf 'sync\n'
  # Keep stdin open until the host observes sync and issues the power cut.
  sleep "$((timeout_seconds + 5))"
}

send_second_input() {
  sleep "$startup_delay"
  printf 'cat /powercut\n'
  sleep "$input_delay"
  printf 'shutdown\n'
}

# First boot: persist a uniquely tagged file, observe the acknowledged sync,
# then terminate QEMU without sending the guest shutdown command.
send_first_input | "${qemu_command[@]}" >"$first_log" 2>&1 &
first_pid=$!
sync_seen=0
for ((attempt = 0; attempt < timeout_seconds * 2; attempt++)); do
  if grep -Eq '^sync:[[:space:]]+ok[[:space:]]*$' "$first_log" 2>/dev/null; then
    sync_seen=1
    break
  fi
  if ! kill -0 "$first_pid" 2>/dev/null; then
    break
  fi
  sleep 0.5
done
if (( sync_seen == 0 )); then
  echo "powercut-qemu: first boot did not acknowledge sync" >&2
  tail -n 120 "$first_log" >&2 || true
  exit 1
fi
if ! kill -0 "$first_pid" 2>/dev/null; then
  echo "powercut-qemu: first QEMU exited before the forced power cut" >&2
  tail -n 120 "$first_log" >&2 || true
  exit 1
fi
kill -TERM "$first_pid"
for ((attempt = 0; attempt < 20; attempt++)); do
  if ! kill -0 "$first_pid" 2>/dev/null; then
    break
  fi
  sleep 0.25
done
if kill -0 "$first_pid" 2>/dev/null; then
  kill -KILL "$first_pid" 2>/dev/null || true
fi
set +e
wait "$first_pid"
first_status=$?
set -e
first_pid=""

if grep -Eiq '\[panic\]|translation fault|allocator fault|guest did not reach shutdown' "$first_log"; then
  echo "powercut-qemu: first boot reported an unexpected failure" >&2
  tail -n 120 "$first_log" >&2 || true
  exit 1
fi
if ! grep -Eiq 'terminating on signal 15|SIGTERM' "$first_log"; then
  echo "powercut-qemu: first boot lacks host SIGTERM termination evidence" >&2
  tail -n 120 "$first_log" >&2 || true
  exit 1
fi
if grep -Fq '[system] shutdown' "$first_log"; then
  echo "powercut-qemu: first boot reached guest shutdown instead of power cut" >&2
  tail -n 120 "$first_log" >&2 || true
  exit 1
fi

# Second boot: read the exact line emitted by cat, then perform a normal guest
# shutdown.  Exact-line matching prevents the command echo from satisfying the
# persistence assertion.
timeout_bin=""
if command -v timeout >/dev/null 2>&1; then
  timeout_bin=timeout
elif command -v gtimeout >/dev/null 2>&1; then
  timeout_bin=gtimeout
fi
set +e
if [[ -n "$timeout_bin" ]]; then
  send_second_input | "$timeout_bin" --signal=TERM --kill-after=5s "${timeout_seconds}s" \
    "${qemu_command[@]}" >"$second_log" 2>&1
  second_status=$?
else
  runner="${MICROSYSTEM_TIMEOUT_BIN:-}"
  if [[ -z "$runner" || ! -x "$runner" ]]; then
    echo "powercut-qemu: timeout(1) is missing; set MICROSYSTEM_TIMEOUT_BIN" >&2
    exit 2
  fi
  send_second_input | "$runner" "${timeout_seconds}s" "${qemu_command[@]}" >"$second_log" 2>&1
  second_status=$?
fi
set -e

if [[ "$second_status" -eq 124 || "$second_status" -eq 137 ]]; then
  echo "powercut-qemu: second boot timed out" >&2
  tail -n 120 "$second_log" >&2 || true
  exit 1
fi
if grep -Eiq '\[panic\]|translation fault|allocator fault|guest did not reach shutdown|terminating on signal 15' "$second_log"; then
  echo "powercut-qemu: second boot reported an unexpected failure" >&2
  tail -n 120 "$second_log" >&2 || true
  exit 1
fi
if ! grep -Eqi '\[user\][[:space:]]+mfs1[[:space:]]+mounted[[:space:]]+via[[:space:]]+EL0[[:space:]]+block[[:space:]]+IPC' "$second_log"; then
  echo "powercut-qemu: second boot did not mount MFS1" >&2
  tail -n 120 "$second_log" >&2 || true
  exit 1
fi
if ! grep -Eq "^${token}[[:space:]]*$" "$second_log"; then
  echo "powercut-qemu: second boot did not read back the persisted token" >&2
  tail -n 120 "$second_log" >&2 || true
  exit 1
fi
if ! grep -Fq '[system] shutdown' "$second_log"; then
  echo "powercut-qemu: second boot did not reach normal shutdown" >&2
  tail -n 120 "$second_log" >&2 || true
  exit 1
fi

echo "powercut-qemu: PASS token=$token first-status=$first_status second-status=$second_status"
echo "powercut-qemu: first-log=$first_log second-log=$second_log"
