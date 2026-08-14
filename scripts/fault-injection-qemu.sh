#!/usr/bin/env bash
set -Eeuo pipefail

repo_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

timeout_seconds="${MICROSYSTEM_QEMU_TIMEOUT:-45}"
gc_timeout_seconds="${MICROSYSTEM_FAULT_GC_TIMEOUT:-180}"
input_delay="${MICROSYSTEM_FAULT_INPUT_DELAY:-1}"
startup_delay="${MICROSYSTEM_FAULT_START_DELAY:-2}"
poll_delay="${MICROSYSTEM_FAULT_POLL_DELAY:-0.01}"
gc_only="${MICROSYSTEM_FAULT_GC_ONLY:-0}"
gc_fill_bytes="${MICROSYSTEM_FAULT_GC_FILL_BYTES:-4194304}"
gc_max_iterations="${MICROSYSTEM_FAULT_GC_MAX_ITERATIONS:-64}"
if ! [[ "$timeout_seconds" =~ ^[0-9]+$ ]]; then
  echo "fault-injection-qemu: MICROSYSTEM_QEMU_TIMEOUT must be an integer" >&2
  exit 2
fi
if ! [[ "$gc_timeout_seconds" =~ ^[0-9]+$ ]] || (( gc_timeout_seconds == 0 )); then
  echo "fault-injection-qemu: MICROSYSTEM_FAULT_GC_TIMEOUT must be a positive integer" >&2
  exit 2
fi
if [[ "$gc_only" != 0 && "$gc_only" != 1 ]]; then
  echo "fault-injection-qemu: MICROSYSTEM_FAULT_GC_ONLY must be 0 or 1" >&2
  exit 2
fi
if [[ ! "$gc_fill_bytes" =~ ^[0-9]+$ || "$gc_fill_bytes" -lt 4096 || $((gc_fill_bytes % 4096)) -ne 0 ]]; then
  echo "fault-injection-qemu: MICROSYSTEM_FAULT_GC_FILL_BYTES must be a positive 4KiB multiple" >&2
  exit 2
fi
if [[ ! "$gc_max_iterations" =~ ^[0-9]+$ || "$gc_max_iterations" -eq 0 ]]; then
  echo "fault-injection-qemu: MICROSYSTEM_FAULT_GC_MAX_ITERATIONS must be a positive integer" >&2
  exit 2
fi
if ! command -v qemu-system-aarch64 >/dev/null 2>&1; then
  echo "fault-injection-qemu: qemu-system-aarch64 is required" >&2
  exit 2
fi
kernel="$repo_root/target/aarch64-unknown-none-softfloat/release/microsystem-kernel"
base_image="$repo_root/build/microsystem.img"
mfsctl="$repo_root/target/release/mfsctl"
if [[ ! -f "$kernel" || ! -f "$base_image" || ! -x "$mfsctl" ]]; then
  echo "fault-injection-qemu: build ELF, disk image, or mfsctl is missing" >&2
  exit 2
fi
for source in "$repo_root/services/fs/src/main.rs" "$repo_root/crates/kernel/src/service_runtime.rs" "$repo_root/build/bootfs.cpio"; do
  if [[ "$kernel" -ot "$source" ]]; then
    echo "fault-injection-qemu: release kernel is older than $source; run make build first" >&2
    exit 2
  fi
done

run_id="$(date +%s)-$$-${RANDOM:-0}"
run_dir="$repo_root/target/fault-injection-$run_id"
mkdir -p "$run_dir"
old_token="faultcut-old-$run_id"
old_source="$run_dir/old-token.txt"
seed_image="$run_dir/seed.img"
printf '%s\n' "$old_token" >"$old_source"
cp "$base_image" "$seed_image"
"$mfsctl" put "$seed_image" "$old_source" /faultcut >"$run_dir/seed-put.log" 2>&1
"$mfsctl" fsck "$seed_image" >"$run_dir/seed-fsck.log" 2>&1

# Keep a separate image for the GC cut.  The default 4 MiB overwrite reaches
# the low-water mark quickly; callers can lower the live set (for example to
# 128 KiB) and increase iteration count when validating GC peak memory.
gc_proof_token="gc-proof-old-$run_id"
gc_proof_source="$run_dir/gc-proof.txt"
gc_fill_source="$run_dir/gc-fill-4m.bin"
gc_seed_image="$run_dir/gc-seed.img"
printf '%s\n' "$gc_proof_token" >"$gc_proof_source"
dd if=/dev/zero of="$gc_fill_source" bs=4096 count=$((gc_fill_bytes / 4096)) status=none
cp "$base_image" "$gc_seed_image"
"$mfsctl" put "$gc_seed_image" "$gc_proof_source" /gc-proof >"$run_dir/gc-seed-proof-put.log" 2>&1
gc_target_reached=0
gc_fill_iterations=0
gc_stats_line=""
gc_capacity_blocks=0
for iteration in $(seq 1 "$gc_max_iterations"); do
  gc_fill_iterations="$iteration"
  if ! "$mfsctl" put "$gc_seed_image" "$gc_fill_source" /gc-fill \
    >"$run_dir/gc-fill-$iteration.log" 2>&1; then
    echo "fault-injection-qemu: gc fill failed iteration=$iteration" >&2
    cat "$run_dir/gc-fill-$iteration.log" >&2 || true
    exit 1
  fi
  gc_stats_output="$("$mfsctl" inspect "$gc_seed_image" 2>&1)" || {
    echo "fault-injection-qemu: gc inspect failed iteration=$iteration" >&2
    printf '%s\n' "$gc_stats_output" >&2
    exit 1
  }
  gc_free_blocks="$(sed -n -E 's/[[:space:]]*free_blocks:[[:space:]]*([0-9]+),/\1/p' <<<"$gc_stats_output" | head -n 1)"
  gc_total_blocks="$(sed -n -E 's/[[:space:]]*total_blocks:[[:space:]]*([0-9]+),/\1/p' <<<"$gc_stats_output" | head -n 1)"
  gc_used_blocks="$(sed -n -E 's/[[:space:]]*used_blocks:[[:space:]]*([0-9]+),/\1/p' <<<"$gc_stats_output" | head -n 1)"
  if [[ ! "$gc_free_blocks" =~ ^[0-9]+$ || ! "$gc_total_blocks" =~ ^[0-9]+$ || ! "$gc_used_blocks" =~ ^[0-9]+$ || "$gc_total_blocks" -eq 0 ]]; then
    echo "fault-injection-qemu: gc inspect lacks numeric free/total/used stats iteration=$iteration" >&2
    printf '%s\n' "$gc_stats_output" >&2
    exit 1
  fi
  gc_capacity_blocks=$(( ((gc_total_blocks - 2) / 256) * 256 ))
  if (( gc_capacity_blocks <= 0 )); then
    echo "fault-injection-qemu: gc inspect produced invalid allocatable capacity total_blocks=$gc_total_blocks" >&2
    exit 1
  fi
  gc_stats_line="generation=$(sed -n -E 's/[[:space:]]*generation:[[:space:]]*([0-9]+),/\1/p' <<<"$gc_stats_output" | head -n 1) used_blocks=$gc_used_blocks free_blocks=$gc_free_blocks capacity_blocks=$gc_capacity_blocks total_blocks=$gc_total_blocks"
  echo "fault-injection-qemu: gc-fill bytes=$gc_fill_bytes iteration=$iteration $gc_stats_line"
  if (( gc_free_blocks * 100 < gc_capacity_blocks * 15 )); then
    gc_target_reached=1
    break
  fi
done
if (( gc_target_reached == 0 )); then
  echo "fault-injection-qemu: gc fill did not cross free-space low-water mark after $gc_fill_iterations iterations bytes=$gc_fill_bytes ($gc_stats_line)" >&2
  exit 1
fi
"$mfsctl" fsck "$gc_seed_image" >"$run_dir/gc-seed-fsck.log" 2>&1

active_qemu_pid=""
active_input_pid=""
active_fifo=""
cleanup() {
  if [[ -n "$active_qemu_pid" ]] && kill -0 "$active_qemu_pid" 2>/dev/null; then
    kill -KILL "$active_qemu_pid" 2>/dev/null || true
  fi
  if [[ -n "$active_input_pid" ]] && kill -0 "$active_input_pid" 2>/dev/null; then
    kill -KILL "$active_input_pid" 2>/dev/null || true
  fi
  if [[ -n "$active_fifo" ]]; then
    rm -f "$active_fifo"
  fi
}
trap cleanup EXIT INT TERM

qemu_command() {
  local image="$1"
  printf '%s\0' \
    qemu-system-aarch64 \
    -machine virt-7.2,virtualization=on,gic-version=3,iommu=smmuv3 \
    -cpu cortex-a72 \
    -accel tcg,thread=multi \
    -smp 2 \
    -m 256M \
    -nographic \
    -L /usr/lib/ipxe/qemu \
    -no-reboot \
    -semihosting-config enable=on,target=native \
    -kernel "$kernel" \
    -drive "if=none,file=$image,format=raw,cache=writeback,id=disk0" \
    -device virtio-blk-pci,drive=disk0,disable-legacy=on,iommu_platform=on,romfile=,addr=2 \
    -netdev user,id=net0,restrict=on \
    -device virtio-net-pci,netdev=net0,disable-legacy=on,iommu_platform=on,romfile=,addr=6,mac=52:54:00:12:34:56 \
    -object rng-random,filename=/dev/urandom,id=rng0 \
    -device virtio-rng-pci,rng=rng0,disable-legacy=on,romfile=,addr=7
}

run_qemu_with_image() {
  local image="$1"
  local fifo="$2"
  local log="$3"
  local -a command=()
  while IFS= read -r -d '' argument; do
    command+=("$argument")
  done < <(qemu_command "$image")
  "${command[@]}" <"$fifo" >"$log" 2>&1 &
  active_qemu_pid=$!
}

send_first_input() {
  local token="$1"
  local log="$2"
  local pid="${3:-}"
  if ! wait_for_prompt "$log" "$pid" "$timeout_seconds"; then
    echo "fault-injection-qemu: first boot did not expose shell prompt before input" >&2
    return 1
  fi
  printf 'write /faultcut %s\n' "$token"
  sleep "$input_delay"
  printf 'sync\n'
  sleep "$((timeout_seconds + 5))"
}

send_second_input() {
  local read_path="$1"
  local log="$2"
  local pid="${3:-}"
  local wait_seconds="${4:-$timeout_seconds}"
  if ! wait_for_prompt "$log" "$pid" "$wait_seconds"; then
    echo "fault-injection-qemu: recovery boot did not expose shell prompt before input" >&2
    return 1
  fi
  printf 'cat %s\n' "$read_path"
  sleep "$input_delay"
  printf 'shutdown\n'
}

wait_for_prompt() {
  local log="$1"
  local pid="$2"
  local wait_seconds="${3:-$timeout_seconds}"
  local deadline=$((SECONDS + wait_seconds))
  while (( SECONDS < deadline )); do
    if grep -Fq -- 'micro> ' "$log" 2>/dev/null; then
      return 0
    fi
    if [[ -n "$pid" ]] && ! kill -0 "$pid" 2>/dev/null; then
      return 1
    fi
    sleep "$poll_delay"
  done
  return 1
}

wait_for_marker() {
  local log="$1"
  local marker="$2"
  local pid="$3"
  local wait_seconds="${4:-$timeout_seconds}"
  local deadline=$((SECONDS + wait_seconds))
  while (( SECONDS < deadline )); do
    if grep -Fq -- "$marker" "$log" 2>/dev/null; then
      return 0
    fi
    if ! kill -0 "$pid" 2>/dev/null; then
      return 1
    fi
    sleep "$poll_delay"
  done
  return 1
}

# A stage marker from an earlier boot or an unrelated commit must not satisfy
# a cut.  Reconstruct a bounded UART byte window and require the exact command
# echo for this case before accepting the stage marker.
wait_for_marker_after_write() {
  local log="$1"
  local token="$2"
  local marker="$3"
  local pid="$4"
  local deadline=$((SECONDS + timeout_seconds))
  while (( SECONDS < deadline )); do
    if [[ ! -f "$log" ]]; then
      sleep "$poll_delay"
      continue
    fi
    if ! kill -0 "$pid" 2>/dev/null; then
      return 1
    fi
    if awk -v token="$token" -v marker="$marker" '
      {
        gsub(/\r/, "")
        joined = window $0
        if (!write_seen && index(joined, "write /faultcut " token)) {
          write_seen = 1
        }
        if (write_seen && index(joined, marker)) {
          found = 1
          exit
        }
        window = joined
        if (length(window) > 8192) {
          window = substr(window, length(window) - 8191)
        }
      }
      END { exit(found ? 0 : 1) }
    ' "$log" 2>/dev/null; then
      kill -STOP "$pid" 2>/dev/null || true
      return 0
    fi
    sleep "$poll_delay"
  done
  return 1
}

run_case() {
  local case_name="$1"
  local marker="$2"
  local expected="$3"
  local source_image="${4:-$seed_image}"
  local read_path="${5:-/faultcut}"
  local recovery_token="${6:-$old_token}"
  local case_image="$run_dir/$case_name.img"
  local first_log="$run_dir/$case_name-first.log"
  local second_log="$run_dir/$case_name-second.log"
  local fsck_log="$run_dir/$case_name-fsck.log"
  local fifo="$run_dir/$case_name.in"
  local new_token="faultcut-$case_name-new-$run_id"
  local forbidden_marker=""
  case "$case_name" in
    transaction-records-written) forbidden_marker='[mfs1-fault] stage=transaction-data-flushed' ;;
    transaction-data-flushed) forbidden_marker='[mfs1-fault] stage=transaction-superblock-written' ;;
    transaction-superblock-written) forbidden_marker='[mfs1-fault] stage=transaction-superblock-flushed' ;;
  esac

  cp "$source_image" "$case_image"
  mkfifo "$fifo"
  active_fifo="$fifo"
  send_first_input "$new_token" "$first_log" >"$fifo" &
  active_input_pid=$!
  run_qemu_with_image "$case_image" "$fifo" "$first_log"
  if ! wait_for_marker_after_write "$first_log" "$new_token" "$marker" "$active_qemu_pid"; then
    echo "fault-injection-qemu: $case_name did not reach marker: $marker" >&2
    tail -n 160 "$first_log" >&2 || true
    return 1
  fi
  # Freeze at the observed marker before the hard kill, avoiding a later stage
  # racing past the byte-level polling point.
  kill -STOP "$active_qemu_pid" 2>/dev/null || true
  kill -KILL "$active_qemu_pid" 2>/dev/null || true
  set +e
  wait "$active_qemu_pid"
  local first_status=$?
  set -e
  active_qemu_pid=""
  if kill -0 "$active_input_pid" 2>/dev/null; then
    kill -KILL "$active_input_pid" 2>/dev/null || true
  fi
  wait "$active_input_pid" 2>/dev/null || true
  active_input_pid=""
  rm -f "$fifo"
  active_fifo=""

  if grep -Eiq '\[panic\]|translation fault|allocator fault' "$first_log"; then
    echo "fault-injection-qemu: $case_name first boot reported an unexpected failure" >&2
    tail -n 160 "$first_log" >&2 || true
    return 1
  fi
  if grep -Fq '[system] shutdown' "$first_log"; then
    echo "fault-injection-qemu: $case_name first boot reached guest shutdown" >&2
    tail -n 160 "$first_log" >&2 || true
    return 1
  fi
  if [[ -n "$forbidden_marker" ]] && grep -Fq -- "$forbidden_marker" "$first_log"; then
    echo "fault-injection-qemu: $case_name raced past its cut marker: $forbidden_marker" >&2
    tail -n 160 "$first_log" >&2 || true
    return 1
  fi

  mkfifo "$fifo"
  active_fifo="$fifo"
  send_second_input "$read_path" "$second_log" >"$fifo" &
  active_input_pid=$!
  local timeout_bin=""
  if command -v timeout >/dev/null 2>&1; then
    timeout_bin=timeout
  elif command -v gtimeout >/dev/null 2>&1; then
    timeout_bin=gtimeout
  fi
  local second_status=0
  set +e
  if [[ -n "$timeout_bin" ]]; then
    local -a command=()
    while IFS= read -r -d '' argument; do
      command+=("$argument")
    done < <(qemu_command "$case_image")
    "$timeout_bin" --signal=TERM --kill-after=5s "${timeout_seconds}s" \
      "${command[@]}" <"$fifo" >"$second_log" 2>&1
    second_status=$?
  else
    echo "fault-injection-qemu: timeout(1) is required in the container" >&2
    set -e
    return 2
  fi
  set -e
  wait "$active_input_pid" 2>/dev/null || true
  active_input_pid=""
  rm -f "$fifo"
  active_fifo=""

  if (( second_status == 124 || second_status == 137 )); then
    echo "fault-injection-qemu: $case_name recovery boot timed out" >&2
    tail -n 160 "$second_log" >&2 || true
    return 1
  fi
  if grep -Eiq '\[panic\]|translation fault|allocator fault|terminating on signal 15' "$second_log"; then
    echo "fault-injection-qemu: $case_name recovery boot reported an unexpected failure" >&2
    tail -n 160 "$second_log" >&2 || true
    return 1
  fi
  if ! grep -Eqi '\[user\][[:space:]]+mfs1[[:space:]]+mounted[[:space:]]+via[[:space:]]+EL0[[:space:]]+block[[:space:]]+IPC' "$second_log"; then
    echo "fault-injection-qemu: $case_name recovery did not mount MFS1" >&2
    tail -n 160 "$second_log" >&2 || true
    return 1
  fi
  if ! grep -Fq '[system] shutdown' "$second_log"; then
    echo "fault-injection-qemu: $case_name recovery did not shut down cleanly" >&2
    tail -n 160 "$second_log" >&2 || true
    return 1
  fi
  local old_seen=0
  local new_seen=0
  grep -Fxq -- "$recovery_token" "$second_log" && old_seen=1 || true
  grep -Fxq -- "$new_token" "$second_log" && new_seen=1 || true
  if (( old_seen + new_seen != 1 )); then
    echo "fault-injection-qemu: $case_name recovery did not read exactly one old/new token (old=$old_seen new=$new_seen)" >&2
    tail -n 160 "$second_log" >&2 || true
    return 1
  fi
  local result="old"
  (( new_seen == 1 )) && result="new"
  case "$expected" in
    old) [[ "$result" == old ]] || { echo "fault-injection-qemu: $case_name expected old token, got $result" >&2; return 1; } ;;
    new) [[ "$result" == new ]] || { echo "fault-injection-qemu: $case_name expected new token, got $result" >&2; return 1; } ;;
    either) ;;
    *) echo "fault-injection-qemu: invalid expected result $expected" >&2; return 2 ;;
  esac

  "$mfsctl" fsck "$case_image" >"$fsck_log" 2>&1
  local fsck_line
  fsck_line="$(grep -E '^MFS1 clean generation=' "$fsck_log" | tail -n 1 || true)"
  if [[ -z "$fsck_line" ]]; then
    echo "fault-injection-qemu: $case_name fsck did not report a clean image" >&2
    cat "$fsck_log" >&2 || true
    return 1
  fi
  echo "fault-injection-qemu: PASS case=$case_name result=$result first-status=$first_status second-status=$second_status"
  echo "fault-injection-qemu: marker=$marker"
  echo "fault-injection-qemu: first-log=$first_log second-log=$second_log fsck=$fsck_line"
}

run_gc_case() {
  local case_name="gc-candidate-superblock-flushed"
  local marker='[mfs1-fault] stage=gc-candidate-superblock-flushed'
  local forbidden_marker='[mfs1-fault] stage=gc-sealed-superblock-written'
  local case_image="$run_dir/$case_name.img"
  local first_log="$run_dir/$case_name-first.log"
  local second_log="$run_dir/$case_name-second.log"
  local fsck_log="$run_dir/$case_name-fsck.log"
  local fifo="$run_dir/$case_name.in"
  local hold_fd=""

  cp "$gc_seed_image" "$case_image"
  mkfifo "$fifo"
  active_fifo="$fifo"
  # Keep the guest stdin open but intentionally send no command: the mfs
  # service's boot-proof fsync observes the low-water image and enters GC.
  run_qemu_with_image "$case_image" "$fifo" "$first_log"
  exec {hold_fd}>"$fifo"
  if ! wait_for_marker "$first_log" "$marker" "$active_qemu_pid" "$gc_timeout_seconds"; then
    echo "fault-injection-qemu: $case_name did not reach marker: $marker" >&2
    echo "fault-injection-qemu: gc seed stats iterations=$gc_fill_iterations $gc_stats_line" >&2
    cat "$run_dir/gc-seed-fsck.log" >&2 || true
    tail -n 180 "$first_log" >&2 || true
    eval "exec ${hold_fd}>&-" 2>/dev/null || true
    return 1
  fi
  if grep -Fq -- "$forbidden_marker" "$first_log"; then
    echo "fault-injection-qemu: $case_name raced past candidate cut marker: $forbidden_marker" >&2
    tail -n 180 "$first_log" >&2 || true
    eval "exec ${hold_fd}>&-" 2>/dev/null || true
    return 1
  fi
  if ! grep -Fq '[user] mfs1 mounted via EL0 block IPC' "$first_log"; then
    echo "fault-injection-qemu: $case_name reached GC without an MFS mount" >&2
    tail -n 180 "$first_log" >&2 || true
    eval "exec ${hold_fd}>&-" 2>/dev/null || true
    return 1
  fi
  kill -STOP "$active_qemu_pid" 2>/dev/null || true
  kill -KILL "$active_qemu_pid" 2>/dev/null || true
  set +e
  wait "$active_qemu_pid"
  local first_status=$?
  set -e
  active_qemu_pid=""
  eval "exec ${hold_fd}>&-" 2>/dev/null || true
  hold_fd=""
  rm -f "$fifo"
  active_fifo=""

  if grep -Eiq '\[panic\]|translation fault|allocator fault' "$first_log"; then
    echo "fault-injection-qemu: $case_name first boot reported an unexpected failure" >&2
    tail -n 180 "$first_log" >&2 || true
    return 1
  fi
  if grep -Fq '[system] shutdown' "$first_log"; then
    echo "fault-injection-qemu: $case_name first boot reached guest shutdown" >&2
    tail -n 180 "$first_log" >&2 || true
    return 1
  fi

  mkfifo "$fifo"
  active_fifo="$fifo"
  send_second_input /gc-proof "$second_log" "" "$gc_timeout_seconds" >"$fifo" &
  active_input_pid=$!
  local timeout_bin=""
  if command -v timeout >/dev/null 2>&1; then
    timeout_bin=timeout
  elif command -v gtimeout >/dev/null 2>&1; then
    timeout_bin=gtimeout
  fi
  local second_status=0
  set +e
  if [[ -n "$timeout_bin" ]]; then
    local -a command=()
    while IFS= read -r -d '' argument; do
      command+=("$argument")
    done < <(qemu_command "$case_image")
    "$timeout_bin" --signal=TERM --kill-after=5s "${gc_timeout_seconds}s" \
      "${command[@]}" <"$fifo" >"$second_log" 2>&1
    second_status=$?
  else
    echo "fault-injection-qemu: timeout(1) is required in the container" >&2
    set -e
    return 2
  fi
  set -e
  wait "$active_input_pid" 2>/dev/null || true
  active_input_pid=""
  rm -f "$fifo"
  active_fifo=""

  if (( second_status == 124 || second_status == 137 )); then
    echo "fault-injection-qemu: $case_name recovery boot timed out" >&2
    tail -n 180 "$second_log" >&2 || true
    return 1
  fi
  if grep -Eiq '\[panic\]|translation fault|allocator fault|terminating on signal 15' "$second_log"; then
    echo "fault-injection-qemu: $case_name recovery boot reported an unexpected failure" >&2
    tail -n 180 "$second_log" >&2 || true
    return 1
  fi
  if ! grep -Eqi '\[user\][[:space:]]+mfs1[[:space:]]+mounted[[:space:]]+via[[:space:]]+EL0[[:space:]]+block[[:space:]]+IPC' "$second_log"; then
    echo "fault-injection-qemu: $case_name recovery did not mount MFS1" >&2
    tail -n 180 "$second_log" >&2 || true
    return 1
  fi
  if ! grep -Fxq -- "$gc_proof_token" "$second_log"; then
    echo "fault-injection-qemu: $case_name recovery did not read the exact gc-proof token" >&2
    tail -n 180 "$second_log" >&2 || true
    return 1
  fi
  if ! grep -Fq '[system] shutdown' "$second_log"; then
    echo "fault-injection-qemu: $case_name recovery did not shut down cleanly" >&2
    tail -n 180 "$second_log" >&2 || true
    return 1
  fi
  "$mfsctl" fsck "$case_image" >"$fsck_log" 2>&1
  local fsck_line
  fsck_line="$(grep -E '^MFS1 clean generation=' "$fsck_log" | tail -n 1 || true)"
  if [[ -z "$fsck_line" ]]; then
    echo "fault-injection-qemu: $case_name fsck did not report a clean image" >&2
    cat "$fsck_log" >&2 || true
    return 1
  fi
  echo "fault-injection-qemu: PASS case=$case_name result=gc-proof first-status=$first_status second-status=$second_status"
  echo "fault-injection-qemu: marker=$marker"
  echo "fault-injection-qemu: first-log=$first_log second-log=$second_log fsck=$fsck_line"
}

if [[ "$gc_only" == 1 ]]; then
  run_gc_case
  echo "fault-injection-qemu: PASS run=$run_id logs=$run_dir gc=validated fill-bytes=$gc_fill_bytes fill-iterations=$gc_fill_iterations stats=($gc_stats_line)"
  exit 0
fi

run_case transaction-records-written \
  '[mfs1-fault] stage=transaction-records-written' old
run_case transaction-data-flushed \
  '[mfs1-fault] stage=transaction-data-flushed' old
run_case transaction-superblock-written \
  '[mfs1-fault] stage=transaction-superblock-written' either
run_case transaction-superblock-flushed \
  '[mfs1-fault] stage=transaction-superblock-flushed' new

run_gc_case

echo "fault-injection-qemu: PASS run=$run_id logs=$run_dir gc=validated fill-bytes=$gc_fill_bytes fill-iterations=$gc_fill_iterations stats=($gc_stats_line)"
