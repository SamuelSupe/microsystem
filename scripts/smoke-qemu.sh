#!/usr/bin/env bash
set -Eeuo pipefail

# End-to-end boot gate for both architecture profiles.  The image is intentionally
# driven through the public make entrypoint so this check exercises the same
# OrbStack/QEMU command developers use locally.
repo_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"
source "$repo_root/scripts/qemu-arch.sh"

if [[ "$MICROSYSTEM_ARCH" == "riscv64" ]]; then
  irq_timer_marker='\[irq\][[:space:]]+PLIC/SBI[[:space:]]+timer[[:space:]]+cpu0'
  intx_marker='\[irq\][[:space:]]+virtio-blk[[:space:]]+INTx[[:space:]]+pin=1[[:space:]]+plic-id=[0-9]+[[:space:]]+bound[[:space:]]+cpu0'
  iommu_domain_marker='\[iommu\][[:space:]]+RISC-V[[:space:]]+domain[[:space:]]+stream-id=0x10[[:space:]]+iova=0x100000[[:space:]][^[:cntrl:]]*cmdq=true'
  iommu_fault_event='0xf'
  fp_state_marker='fd-regs=f0-f31[[:space:]]+fcsr=true'
  fp_state_label='fd-regs=f0-f31 fcsr=true'
else
  irq_timer_marker='\[irq\][[:space:]]+GICv3[[:space:]]+timer[[:space:]]+cpu0'
  intx_marker='\[irq\][[:space:]]+virtio-blk[[:space:]]+INTx[[:space:]]+pin=1[[:space:]]+gic-id=37[[:space:]]+bound[[:space:]]+cpu0'
  iommu_domain_marker='\[iommu\][[:space:]]+SMMUv3[[:space:]]+domain[[:space:]]+stream-id=0x10[[:space:]]+iova=0x100000[[:space:]][^[:cntrl:]]*cmdq=true'
  iommu_fault_event='0x10'
  fp_state_marker='q-regs=q0-q31[[:space:]]+fpcr-fpsr=true'
  fp_state_label='q-regs=q0-q31 fpcr-fpsr=true'
fi

timeout_seconds="${MICROSYSTEM_QEMU_TIMEOUT:-120}"
input_delay="${MICROSYSTEM_COMMAND_DELAY:-1}"
log_file="${MICROSYSTEM_QEMU_LOG:-$repo_root/target/smoke-qemu.log}"
mkdir -p "$(dirname -- "$log_file")"

if ! command -v make >/dev/null 2>&1; then
  echo "smoke-qemu: make is required" >&2
  exit 2
fi

# `make test` enters the toolchain container before running this script.  In
# that case Docker-in-Docker is intentionally unavailable; invoke xtask
# directly so QEMU still runs inside the already provisioned OrbStack image.
inside_container=0
if [[ -f /.dockerenv || "${MICROSYSTEM_IN_CONTAINER:-0}" == "1" ]]; then
  inside_container=1
fi
run_command=(make run)
if (( inside_container )); then
  # The host `make build` stage already produced the AArch64 ELF and disk.
  # `qemu` deliberately skips the cross-build, which is unavailable in the
  # runtime image when rustup cannot fetch the bare-metal target.
  run_command=(cargo run -p xtask -- qemu)
elif ! command -v docker >/dev/null 2>&1; then
  echo "smoke-qemu: docker is required (run through OrbStack)" >&2
  exit 2
elif ! docker info >/dev/null 2>&1; then
  echo "smoke-qemu: Docker daemon is unavailable; start OrbStack first" >&2
  exit 2
fi

# The shell is expected to exit after shutdown.  Delaying between commands is
# intentional: it gives both 5 ms timer PPIs time to fire before ps/uptime are
# sampled, instead of feeding the entire transcript before the guest runs.
send_input() {
  # Let QEMU finish booting both cores before the first line arrives.  Without
  # this pre-roll the host pipe can queue the complete transcript while the
  # guest is still enabling the secondary timer.
  sleep "$input_delay"
  printf 'help\n'
  sleep "$input_delay"
  printf 'pwd\n'
  sleep "$input_delay"
  printf 'echo smoke-echo\n'
  sleep "$input_delay"
  printf 'clear\n'
  sleep "$input_delay"
  printf 'ps\n'
  sleep "$input_delay"
  printf 'uptime\n'
  sleep "$input_delay"
  printf 'sleep 25ms\n'
  sleep "$input_delay"
  printf 'uptime\n'
  sleep "$input_delay"
  printf 'date\n'
  sleep "$input_delay"
  printf 'free\n'
  sleep "$input_delay"
  printf 'sysinfo\n'
  sleep "$input_delay"
  printf 'mkdir -p /demo/tree/nested\n'
  sleep "$input_delay"
  printf 'cd /demo/tree/nested\n'
  sleep "$input_delay"
  printf 'pwd\n'
  sleep "$input_delay"
  printf 'cd /\n'
  sleep "$input_delay"
  printf 'history\n'
  sleep "$input_delay"
  printf 'write /demo/tree/nested/data tree-data\n'
  sleep "$input_delay"
  printf 'cp -r /demo/tree /demo/tree-copy\n'
  sleep "$input_delay"
  printf 'touch /demo/empty\n'
  sleep "$input_delay"
  printf 'write /demo/hello hello-microsystem\n'
  sleep "$input_delay"
  printf 'cp /demo/hello /demo/copy\n'
  sleep "$input_delay"
  printf 'fsync /demo/copy\n'
  sleep "$input_delay"
  printf 'sync\n'
  sleep "$input_delay"
  printf 'ls /demo\n'
  sleep "$input_delay"
  printf 'head -n 1 /boot-proof\n'
  sleep "$input_delay"
  printf 'tail -n 1 /boot-proof\n'
  sleep "$input_delay"
  printf 'wc /boot-proof\n'
  sleep "$input_delay"
  printf 'hexdump /boot-proof\n'
  sleep "$input_delay"
  printf 'grep MFS1 /boot-proof\n'
  sleep "$input_delay"
  printf 'find /demo\n'
  sleep "$input_delay"
  printf 'tree /demo\n'
  sleep "$input_delay"
  printf 'du /demo\n'
  sleep "$input_delay"
  printf 'df\n'
  sleep "$input_delay"
  printf 'mica --allow fs.write:/demo/large -e '"'"'local bytes = require("bytes");local fs = require("fs");local payload = "01234567890123456789012345678901";payload = payload + payload;payload = payload + payload;payload = payload + payload;payload = payload + payload;payload = payload + payload;payload = payload + payload;payload = payload + payload;payload = payload + payload;local ok, err = fs.write_file("/demo/large", bytes.from_string(payload), {atomic = true, fsync = true});if not ok then error(err.message) end'"'"'\n'
  sleep "$input_delay"
  printf 'wc /demo/large\n'
  sleep "$input_delay"
  printf 'cat /demo/large\n'
  sleep "$input_delay"
  printf 'cp /demo/large /demo/large-copy\n'
  sleep "$input_delay"
  printf 'wc /demo/large-copy\n'
  sleep "$input_delay"
  printf 'cat /demo/tree-copy/nested/data\n'
  sleep "$input_delay"
  printf 'rm /demo/tree\n'
  sleep "$input_delay"
  printf 'rm -r /demo/tree-copy\n'
  sleep "$input_delay"
  printf 'rm -r /demo/tree\n'
  sleep "$input_delay"
  printf 'find /demo/tree-copy\n'
  sleep "$input_delay"
  printf 'find /demo/tree\n'
  sleep "$input_delay"
  printf 'fs stat /demo/hello\n'
  sleep "$input_delay"
  printf 'cat /demo/hello\n'
  sleep "$input_delay"
  printf 'rm /demo/copy\n'
  sleep "$input_delay"
  printf 'rm /demo/empty\n'
  sleep "$input_delay"
  printf 'rm /demo/hello\n'
  sleep "$input_delay"
  printf 'rm /demo/large-copy\n'
  sleep "$input_delay"
  printf 'rm /demo/large\n'
  sleep "$input_delay"
  printf 'rmdir /demo\n'
  sleep "$input_delay"
  printf 'netstat\n'
  sleep "$input_delay"
  printf 'nslookup localhost\n'
  sleep "$input_delay"
  printf 'run resourceprobe\n'
  sleep 2
  printf 'run resourcefault\n'
  sleep 2
  printf 'run resourcekill\n'
  sleep 2
  sleep "$input_delay"
  printf 'run privprobe\n'
  sleep 2
  printf 'run counter\n'
  sleep 2
  printf 'run missing\n'
  sleep "$input_delay"
  printf 'ps\n'
  sleep "$input_delay"
  printf 'exit\n'
  sleep "$input_delay"
  printf 'shutdown\n'
}

timeout_bin=""
if command -v timeout >/dev/null 2>&1; then
  timeout_bin="timeout"
elif command -v gtimeout >/dev/null 2>&1; then
  timeout_bin="gtimeout"
fi
if [[ -z "$timeout_bin" ]]; then
  # macOS does not ship GNU timeout; use the repository's documented override
  # so callers can provide gtimeout or another bounded runner.
  runner="${MICROSYSTEM_TIMEOUT_BIN:-}"
  if [[ -z "$runner" || ! -x "$runner" ]]; then
    echo "smoke-qemu: timeout(1) is missing; set MICROSYSTEM_TIMEOUT_BIN to a bounded runner" >&2
    exit 2
  fi
fi

run_guest() {
  local output_file="$1"
  if [[ -n "$timeout_bin" ]]; then
    send_input | "$timeout_bin" --signal=TERM --kill-after=5s "${timeout_seconds}s" "${run_command[@]}" >"$output_file" 2>&1
  else
    send_input | "$runner" "${timeout_seconds}s" "${run_command[@]}" >"$output_file" 2>&1
  fi
}

# A timeout is still used to turn a failed boot or a stuck guest into a
# deterministic test failure.
set +e
run_guest "$log_file"
status=$?
set -e

if [[ "$status" -eq 124 || "$status" -eq 137 ]]; then
  echo "smoke-qemu: guest did not reach shutdown within ${timeout_seconds}s" >&2
  tail -n 120 "$log_file" >&2 || true
  exit 1
fi

recovery_marker='\[user\][[:space:]]+mfs1[[:space:]]+recovered[[:space:]]+/boot-proof[[:space:]]+after[[:space:]]+restart=true'
if ! grep -Eqi -- "$recovery_marker" "$log_file" \
  && grep -Eqi -- '\[user\][[:space:]]+mfs1[[:space:]]+fsync[[:space:]]+/boot-proof[[:space:]]+persistent=true' "$log_file" \
  && grep -Eqi -- '\[system\][[:space:]]+shutdown' "$log_file"; then
  first_boot_log="${log_file}.first-boot"
  mv -f -- "$log_file" "$first_boot_log"
  echo "smoke-qemu: first boot established /boot-proof; validating recovery on restart" >&2
  set +e
  run_guest "$log_file"
  status=$?
  set -e
  if [[ "$status" -eq 124 || "$status" -eq 137 ]]; then
    echo "smoke-qemu: recovery boot did not reach shutdown within ${timeout_seconds}s" >&2
    tail -n 120 "$log_file" >&2 || true
    exit 1
  fi
fi

# QEMU exits with a non-zero status for some deliberate shutdown paths; the
# serial milestones are the authoritative part of this smoke gate.
required_markers=(
  "cpu0"
  "cpu1"
  "mmu"
  "\\[mm\\][[:space:]]+kernel[[:space:]]+heap[[:space:]]+ready[[:space:]]+base=0x[[:xdigit:]]+[[:space:]]+bytes=0x8000000"
  "\\[mm\\][[:space:]]+frame[[:space:]]+allocator[[:space:]]+ready[[:space:]]+base=0x[[:xdigit:]]+[[:space:]]+free=[0-9]+"
  "\\[mm\\][[:space:]]+TaskMemory/page[[:space:]]+tables[[:space:]]+allocated[[:space:]]+from[[:space:]]+kernel[[:space:]]+heap[[:space:]]+slot-bytes=0x[[:xdigit:]]+[[:space:]]+allocated=0x[[:xdigit:]]+"
  "$irq_timer_marker"
  "\\[boot\\][^[:cntrl:]]*iommu-map="
  "timer=true"
  "$intx_marker"
  "\\[sched\\][[:space:]]+EL0[[:space:]]+idle[[:space:]]+thread[[:space:]]+entered"
  "\\[sched\\][[:space:]]+resident[[:space:]]+shared-ready-queue[[:space:]]+cpus=2[[:space:]]+ticket-lock=true[[:space:]]+idle-tasks=2[[:space:]]+switches=\\[[0-9]+,[0-9]+\\][[:space:]]+non-idle=\\[[0-9]+,[0-9]+\\]"
  "$iommu_domain_marker"
  "\\[virtio\\][[:space:]]+EL1[[:space:]]+isolation[[:space:]]+queue[[:space:]]+armed;[[:space:]]+block[[:space:]]+data[[:space:]]+I/O[[:space:]]+delegated[[:space:]]+to[[:space:]]+EL0"
  "\\[user\\][[:space:]]+block[[:space:]]+configured[[:space:]]+split[[:space:]]+queue0[[:space:]]+size=16[[:space:]]+driver-ok=true"
  "\\[user\\][[:space:]]+block[[:space:]]+driver[[:space:]]+queue0[[:space:]]+read\\+write[[:space:]]+sector=0[[:space:]]+mfs1=true[[:space:]]+flush=ok"
  "\\[user\\][[:space:]]+mfs1[[:space:]]+mounted[[:space:]]+via[[:space:]]+EL0[[:space:]]+block[[:space:]]+IPC"
  "\\[user\\][[:space:]]+mfs1[[:space:]]+recovered[[:space:]]+/boot-proof[[:space:]]+after[[:space:]]+restart=true"
  "\\[user\\][[:space:]]+mfs1[[:space:]]+fsync[[:space:]]+/boot-proof[[:space:]]+persistent=true"
  "\\[cap\\][[:space:]]+resident[[:space:]]+generation[[:space:]]+rights[[:space:]]+revoke[[:space:]]+stale-handle=true"
  "\\[cap\\][[:space:]]+resident[[:space:]]+cross-task[[:space:]]+copy-move[[:space:]]+revoke[[:space:]]+atomic=true"
  "\\[cap\\][[:space:]]+resident[[:space:]]+revoke[[:space:]]+cleared[[:space:]]+remote[[:space:]]+frame[[:space:]]+mapping=true[[:space:]]+tlbi=true[[:space:]]+task-survived=true"
  "\\[devmgr\\][[:space:]]+resident[[:space:]]+root[[:space:]]+delegated[[:space:]]+mmio/frame/dma/irq[[:space:]]+to[[:space:]]+devmgr[[:space:]]+via[[:space:]]+IPC"
  "\\[iommu\\][[:space:]]+resident[[:space:]]+block[[:space:]]+DmaMap[[:space:]]+frames=2[[:space:]]+capability=true[[:space:]]+pte=true[[:space:]]+cmdq=true[[:space:]]+unmap-remap=true"
  "\\[iommu\\][[:space:]]+resident[[:space:]]+block[[:space:]]+dynamic-frame[[:space:]]+iova=0x102000[[:space:]]+read=true[[:space:]]+dma-delete=busy[[:space:]]+reclaimed=true"
  "\\[mm\\][[:space:]]+MemoryPool[[:space:]]+independent[[:space:]]+pools=2[[:space:]]+quotas=\\[4,4\\][[:space:]]+root=true[[:space:]]+driver=true"
  "\\[mm\\][[:space:]]+MemoryPool[[:space:]]+dynamic[[:space:]]+quota=2[[:space:]]+third=NoMemory[[:space:]]+live-delete=Busy[[:space:]]+reclaimed=true[[:space:]]+registry-reused=true[[:space:]]+cross-task-move=true"
  "\\[mm\\][[:space:]]+EL0[[:space:]]+user[[:space:]]+heap[[:space:]]+free-list[[:space:]]+reuse[[:space:]]+allocations=512[[:space:]]+bytes=4096[[:space:]]+true"
  "\\[mmu\\][[:space:]]+resident[[:space:]]+memory-pool[[:space:]]+quota=4[[:space:]]+frame-map/unmap[[:space:]]+remap=true[[:space:]]+mapped-delete=busy[[:space:]]+mapped-move=blocked[[:space:]]+reclaimed=true"
  "\\[user\\][[:space:]]+mfs1[[:space:]]+background-writeback=1s[[:space:]]+online-gc=true"
  "\\[user\\][[:space:]]+mfs1[[:space:]]+segment-directory[[:space:]]+active-segments=[0-9]+[[:space:]]+low-water=15[[:space:]]+high-water=25"
  "\\[user\\][[:space:]]+mfs1[[:space:]]+checkpoint[[:space:]]+valid=true[[:space:]]+segment-blocks=256"
  "\\[ipc\\][[:space:]]+resident[[:space:]]+absolute-deadline[[:space:]]+timeout=true"
  "\\[ipc\\][[:space:]]+resident[[:space:]]+endpoint[[:space:]]+capabilities[[:space:]]+enforced=true"
  "\\[ipc\\][[:space:]]+filesystem[[:space:]]+payload[[:space:]]+frame[[:space:]]+isolated[[:space:]]+from[[:space:]]+block[[:space:]]+DMA=true"
  "\\[ipc\\][[:space:]]+filesystem[[:space:]]+open/read/write/fsync/sync/stat/readdir/mkdir/rename/unlink[[:space:]]+protocol=true"
  "\\[irq\\][[:space:]]+resident[[:space:]]+generic[[:space:]]+notification[[:space:]]+deadline-block=true"
  "\\[irq\\][[:space:]]+resident[[:space:]]+block[[:space:]]+notification[[:space:]]+bitset=true"
  "\\[irq\\][[:space:]]+resident[[:space:]]+generic[[:space:]]+notification[[:space:]]+signal=true[[:space:]]+bitset=0x5[[:space:]]+cross-task=true"
  "\\[irq\\][[:space:]]+resident[[:space:]]+generic[[:space:]]+notification[[:space:]]+wake-after-block=true[[:space:]]+cross-task=true"
  "^mkdir:[[:space:]]+ok[[:space:]]*$"
  "^touch:[[:space:]]+ok[[:space:]]*$"
  "^write:[[:space:]]+ok[[:space:]]*$"
  "^cp:[[:space:]]+ok[[:space:]]*$"
  "^fsync:[[:space:]]+ok[[:space:]]*$"
  "^sync:[[:space:]]+ok[[:space:]]*$"
  "^file[[:space:]]+17[[:space:]]+bytes[[:space:]]*$"
  "^rmdir:[[:space:]]+ok[[:space:]]*$"
  "micro>[[:space:]]+pwd"
  "^/$"
  "^/demo/tree/nested[[:space:]]*$"
  "^smoke-echo[[:space:]]*$"
  "^[[:space:]]+[0-9]+[[:space:]]+pwd[[:space:]]*$"
  "^[0-9]{4}-[0-9]{2}-[0-9]{2}[[:space:]]+[0-9]{2}:[0-9]{2}:[0-9]{2}[[:space:]]+UTC[[:space:]]*$"
  "^cpus=2[[:space:]]+ticks=\[[0-9]+,[0-9]+\]"
  "^1[[:space:]]+3[[:space:]]+28[[:space:]]+/boot-proof[[:space:]]*$"
  "^00000000[[:space:]]+4d[[:space:]]+69[[:space:]]+63[[:space:]]+72[[:space:]]+6f[[:space:]]+53[[:space:]]+79[[:space:]]+73[[:space:]]+74[[:space:]]+65[[:space:]]+6d"
  "^/demo/tree/nested$"
  "^[[:space:]]{4}nested$"
  "^[0-9]+[[:space:]]+/demo$"
  "^filesystem[[:space:]]+blocks=[0-9]+[[:space:]]+used=[0-9]+[[:space:]]+free=[0-9]+[[:space:]]+block_size=[0-9]+[[:space:]]+entries=[0-9]+[[:space:]]+generation=[0-9]+[[:space:]]+transaction=[0-9]+[[:space:]]*$"
  "^0[[:space:]]+1[[:space:]]+8192[[:space:]]+/demo/large[[:space:]]*$"
  "^0[[:space:]]+1[[:space:]]+8192[[:space:]]+/demo/large-copy[[:space:]]*$"
  "^tree-data[[:space:]]*$"
  "^rm:[[:space:]]+failed[[:space:]]*$"
  "^find:[[:space:]]+failed[[:space:]]*$"
  "^ipv4=[0-9.]+[[:space:]]+gateway=[0-9.]+[[:space:]]+dns=[0-9.]+[[:space:]]+sessions=[0-9]+[[:space:]]+connections=[0-9]+/[0-9]+[[:space:]]*$"
  "^([0-9]{1,3}\.){3}[0-9]{1,3}[[:space:]]*$"
  "^mica:[[:space:]]+pid=[0-9]+[[:space:]]+status=0[[:space:]]*$"
  "^logout[[:space:]]*$"
  "^\[system\][[:space:]]+shutdown[[:space:]]*$"
  "micro>[[:space:]]+sleep[[:space:]]+25ms"
  "micro>[[:space:]]+rm[[:space:]]+-r[[:space:]]+/demo/tree-copy"
  "micro>[[:space:]]+rm[[:space:]]+-r[[:space:]]+/demo/tree"
  "\\[iommu\\][[:space:]]+fault-probe[[:space:]]+blocked=true[[:space:]]+sentinel=true[[:space:]]+event=${iommu_fault_event}[[:space:]]+stream-id=0x10"
  "\\[bootfs\\][[:space:]]+valid=true[[:space:]]+entries=24[[:space:]]+static-elfs=23"
  "\\[bootfs\\][[:space:]]+root[[:space:]]+task[[:space:]]+started[[:space:]]+manifest[[:space:]]+services=devmgr,console,block,mfs,shell"
  "\\[service\\][[:space:]]+resident[[:space:]]+EL0[[:space:]]+address-spaces=12[[:space:]]+asids=\\[0x20\\.\\.0x2b\\]"
  "\\[user\\][[:space:]]+bootfs[[:space:]]+init[[:space:]]+ELF[[:space:]]+entered[[:space:]]+EL0"
  "\\[user\\][[:space:]]+console[[:space:]]+service[[:space:]]+ELF[[:space:]]+entered[[:space:]]+EL0"
  "\\[user\\][[:space:]]+block[[:space:]]+service[[:space:]]+ELF[[:space:]]+entered[[:space:]]+EL0"
  "\\[user\\][[:space:]]+mfs[[:space:]]+service[[:space:]]+ELF[[:space:]]+entered[[:space:]]+EL0"
  "\\[user\\][[:space:]]+shell[[:space:]]+service[[:space:]]+ELF[[:space:]]+entered[[:space:]]+EL0"
  "\\[user\\][[:space:]]+devmgr[[:space:]]+service[[:space:]]+ELF[[:space:]]+entered[[:space:]]+EL0"
  "\\[devmgr\\][[:space:]]+resident[[:space:]]+EL0[[:space:]]+discovered[[:space:]]+PCI[[:space:]]+ECAM[[:space:]]+function[[:space:]]+and[[:space:]]+assigned[[:space:]]+BARs"
  "\\[devmgr\\][[:space:]]+resident[[:space:]]+EL0[[:space:]]+accepted[[:space:]]+root[[:space:]]+caps=4[[:space:]]+forwarded[[:space:]]+block[[:space:]]+caps=4"
  "\\[virtio\\][[:space:]]+validated[[:space:]]+EL0[[:space:]]+BAR[[:space:]]+assignment[[:space:]]+queues=[0-9]+[[:space:]]+capacity-sectors=[0-9]+[[:space:]]+notify-multiplier=[0-9]+"
  "\\[devmgr\\][[:space:]]+resident[[:space:]]+EL0[[:space:]]+negotiated[[:space:]]+modern[[:space:]]+transport[[:space:]]+features=VERSION_1\\+ACCESS_PLATFORM\\+FLUSH[[:space:]]+queues-positive=true[[:space:]]+capacity-positive=true"
  "\\[service\\][[:space:]]+resident[[:space:]]+EL0[[:space:]]+ready=8/8[[:space:]]+online=8/8[[:space:]]+switches=[0-9]+"
  "^PID[[:space:]]+5[[:space:]]+shell[[:space:]]+running[[:space:]]+cpu=[01]([[:space:]]|$)"
  "^PID[[:space:]]+[1-4][[:space:]]+[[:alnum:]_-]+[[:space:]]+resident[[:space:]]+cpu=shared([[:space:]]|$)"
  "\\[ipc\\][[:space:]]+resident[[:space:]]+console[[:space:]]+endpoint=1[[:space:]]+ready"
  "\\[ipc\\][[:space:]]+resident[[:space:]]+init->console[[:space:]]+reply=0x11"
  "\\[ipc\\][[:space:]]+resident[[:space:]]+block[[:space:]]+endpoint=2[[:space:]]+ready"
  "\\[ipc\\][[:space:]]+resident[[:space:]]+mfs->block[[:space:]]+capacity-sectors=131072"
  "\\[ipc\\][[:space:]]+resident[[:space:]]+mfs[[:space:]]+endpoint=3[[:space:]]+ready"
  "\\[ipc\\][[:space:]]+resident[[:space:]]+shell->mfs[[:space:]]+magic=MFS1"
  "\\[ipc\\][[:space:]]+resident[[:space:]]+procman[[:space:]]+endpoint=4[[:space:]]+ready"
  "\\[ipc\\][[:space:]]+resident[[:space:]]+time[[:space:]]+sleep/uptime[[:space:]]+endpoint=4[[:space:]]+shared-procman=true"
  "\\[proc\\][[:space:]]+duplicate[[:space:]]+spinner[[:space:]]+independent=true[[:space:]]+survivor-running=true[[:space:]]+first-status=-15[[:space:]]+second-status=-15[[:space:]]+reclaimed=true[[:space:]]+slot-reuse=true"
  "\\[proc\\][[:space:]]+counter[[:space:]]+slot-reuse=true[[:space:]]+wait-status=0"
  "\\[app\\][[:space:]]+spinner[[:space:]]+pid=[0-9]+[[:space:]]+started[[:space:]]+at[[:space:]]+EL0"
  "\\[app\\][[:space:]]+privileged[[:space:]]+probe[[:space:]]+pid=[0-9]+[[:space:]]+entered[[:space:]]+EL0"
  "\\[app\\][[:space:]]+invalid-pointer[[:space:]]+probe[[:space:]]+pid=[0-9]+[[:space:]]+entered[[:space:]]+EL0"
  "\\[isolation\\][[:space:]]+invalid[[:space:]]+user[[:space:]]+pointer[[:space:]]+rejected[[:space:]]+status=-8[[:space:]]+task-survived=true"
  "\\[app\\][[:space:]]+cross-page[[:space:]]+pointer[[:space:]]+probe[[:space:]]+pid=[0-9]+[[:space:]]+entered[[:space:]]+EL0"
  "\\[isolation\\][[:space:]]+cross-page[[:space:]]+user[[:space:]]+buffer[[:space:]]+rejected-before-copy[[:space:]]+status=-8[[:space:]]+task-survived=true"
  "\\[proc\\][[:space:]]+cross-page[[:space:]]+pointer[[:space:]]+probe[[:space:]]+pid=[0-9]+[[:space:]]+wait-status=0[[:space:]]+task-survived=true"
  "\\[app\\][[:space:]]+page-fault[[:space:]]+probe[[:space:]]+pid=[0-9]+[[:space:]]+entered[[:space:]]+EL0"
  "\\[proc\\][[:space:]]+invalid-pointer[[:space:]]+probe[[:space:]]+pid=[0-9]+[[:space:]]+wait-status=0[[:space:]]+task-survived=true"
  "\\[proc\\][[:space:]]+page-fault[[:space:]]+probe[[:space:]]+pid=[0-9]+[[:space:]]+wait-status=-8[[:space:]]+reclaimed=true"
  # Concurrent EL0 UART writes can splice a short token (for example,
  # `resourcep[app]robe`) while preserving the command and PID bytes.
  "run:[[:space:]]+resourcep[^[:space:]]*robe[[:space:]]+pid=[0-9]+"
  "\\[mm\\][[:space:]]+application[[:space:]]+exit[[:space:]]+reclaimed[[:space:]]+pid=[0-9]+[[:space:]]+frames=1[[:space:]]+pools=1[[:space:]]+mappings=1"
  "\\[proc\\][[:space:]]+bootfs[[:space:]]+name-based[[:space:]]+loader[[:space:]]+program=resourceprobe[[:space:]]+pid=[0-9]+[[:space:]]+static-elf=true"
  "\\[app\\][[:space:]]+resource[[:space:]]+fault[[:space:]]+probe[[:space:]]+pid=[0-9]+[[:space:]]+triggering[[:space:]]+page[[:space:]]+fault[[:space:]]+with[[:space:]]+live[[:space:]]+frames=2[[:space:]]+mappings=2"
  "\\[proc\\][[:space:]]+application[[:space:]]+fault[[:space:]]+ESR=0x[[:xdigit:]]+[[:space:]]+FAR=0x600000;[[:space:]]+pid=[0-9]+[[:space:]]+terminated"
  "\\[mm\\][[:space:]]+application[[:space:]]+exit[[:space:]]+reclaimed[[:space:]]+pid=[0-9]+[[:space:]]+frames=2[[:space:]]+pools=1[[:space:]]+mappings=2"
  "\\[proc\\][[:space:]]+bootfs[[:space:]]+name-based[[:space:]]+loader[[:space:]]+program=resourcefault[[:space:]]+pid=[0-9]+[[:space:]]+static-elf=true"
  "run:[[:space:]]+resourcek[^[:space:]]*ill[[:space:]]+pid=[0-9]+"
  "\\[app\\][[:space:]]+resource[[:space:]]+kill[[:space:]]+probe[[:space:]]+pid=[0-9]+[[:space:]]+entered[[:space:]]+EL0"
  "kill:[[:space:]]+pid=[0-9]+[[:space:]]+status=-15"
  "wait:[[:space:]]+pid=[0-9]+[[:space:]]+status=-15"
  "\\[mm\\][[:space:]]+application[[:space:]]+exit[[:space:]]+reclaimed[[:space:]]+pid=[0-9]+[[:space:]]+frames=2[[:space:]]+pools=1[[:space:]]+mappings=2"
  "\\[proc\\][[:space:]]+bootfs[[:space:]]+name-based[[:space:]]+loader[[:space:]]+program=resourcekill[[:space:]]+pid=[0-9]+[[:space:]]+static-elf=true"
  "\\[app\\][[:space:]]+counter[[:space:]]+pid=[0-9]+[[:space:]]+started[[:space:]]+at[[:space:]]+EL0"
  "\\[app\\][[:space:]]+counter[[:space:]]+pid=[0-9]+[[:space:]]+completed"
  "\\[isolation\\][[:space:]]+privileged[[:space:]]+instruction[[:space:]]+task[[:space:]]+pid=[0-9]+[[:space:]]+faulted[[:space:]]+status=-8[[:space:]]+reclaimed=true"
  "\\[proc\\][[:space:]]+dynamic[[:space:]]+application[[:space:]]+capacity=8[[:space:]]+first-pid=13[[:space:]]+independent-slots=true"
  "\\[sched\\][[:space:]]+application[[:space:]]+spinners[[:space:]]+dual-core=true[[:space:]]+tasks=2[[:space:]]+cpus=2[[:space:]]+cpu-masks-pair=[0-9]{2}"
  # The interactive privprobe command may be split by a concurrent EL0
  # writer; its exact PID/status is checked by the bounded lifecycle below.
  "run:[[:space:]]+privp[^[:space:]]*robe[[:space:]]+pid[^[:cntrl:]]*=[0-9]+"
  "wait:[[:space:]]+pid=[0-9]+[[:space:]]+status=-8"
  "run:[[:space:]]+counter[[:space:]]+pid=[0-9]+"
  "\\[proc\\][[:space:]]+bootfs[[:space:]]+name-based[[:space:]]+loader[[:space:]]+program=counter[[:space:]]+pid=[0-9]+[[:space:]]+static-elf=true"
  "run:[[:space:]]+missing:[[:space:]]+not[[:space:]]+found"
  "\\[mmu\\][[:space:]]+round-robin[[:space:]]+address-spaces=2[[:space:]]+asids=\\[1,2\\]"
  "\\[ipc\\][[:space:]]+call/recv/reply[[:space:]]+endpoint=1[[:space:]]+request=0x1234[[:space:]]+reply=0x2468[[:space:]]+asids=\\[3,4\\]"
  "\\[sched\\][[:space:]]+fp-simd[[:space:]]+context-isolation=true[[:space:]]+tasks=2[[:space:]]+context-switches=[0-9]+[[:space:]]+checks=\\[[0-9]+,[0-9]+\\][[:space:]]+mismatches=\\[0,0\\][[:space:]]+$fp_state_marker[[:space:]]+signatures=\\[0x11,0x22\\]"
  "root[[:space:]-]+task"
  "block"
  "mfs1"
  "shell[[:space:]-]+ready"
)
missing=()
for marker in "${required_markers[@]}"; do
  if ! grep -Eqi -- "$marker" "$log_file"; then
    missing+=("$marker")
  fi
done

if (( ${#missing[@]} != 0 )); then
  echo "smoke-qemu: missing serial milestones: ${missing[*]}" >&2
  tail -n 120 "$log_file" >&2 || true
  exit 1
fi

virtio_el0_line="$(grep -Eio '\[virtio\][[:space:]]+validated[[:space:]]+EL0[[:space:]]+BAR[[:space:]]+assignment[[:space:]]+queues=[0-9]+[[:space:]]+capacity-sectors=[0-9]+[[:space:]]+notify-multiplier=[0-9]+' "$log_file" | tail -n 1 || true)"
virtio_el0_values="$(sed -E 's/.*queues=([0-9]+)[[:space:]]+capacity-sectors=([0-9]+)[[:space:]]+notify-multiplier=([0-9]+).*/\1 \2 \3/i' <<< "$virtio_el0_line")"
read -r virtio_queues virtio_capacity virtio_notify_multiplier <<< "$virtio_el0_values"
if [[ -z "$virtio_el0_line" || ! "$virtio_queues" =~ ^[0-9]+$ || ! "$virtio_capacity" =~ ^[0-9]+$ || ! "$virtio_notify_multiplier" =~ ^[0-9]+$ ]] \
  || (( virtio_queues == 0 || virtio_capacity == 0 || virtio_notify_multiplier == 0 )); then
  echo "smoke-qemu: EL0 BAR assignment transport values are missing or non-positive: ${virtio_el0_line:-<missing>}" >&2
  tail -n 120 "$log_file" >&2 || true
  exit 1
fi

mfs_directory_line="$(grep -Eio '\[user\][[:space:]]+mfs1[[:space:]]+segment-directory[[:space:]]+active-segments=[0-9]+[[:space:]]+low-water=15[[:space:]]+high-water=25' "$log_file" | tail -n 1 || true)"
mfs_active_segments="$(sed -E 's/.*active-segments=([0-9]+).*/\1/i' <<< "$mfs_directory_line")"
if [[ -z "$mfs_directory_line" || ! "$mfs_active_segments" =~ ^[0-9]+$ ]] || (( mfs_active_segments == 0 )); then
  echo "smoke-qemu: MFS segment-directory active segment count is missing or zero: ${mfs_directory_line:-<missing>}" >&2
  tail -n 120 "$log_file" >&2 || true
  exit 1
fi

dtb_ram_line="$(grep -Eio '\[boot\][[:space:]]+ram=0x[[:xdigit:]]+\+0x[[:xdigit:]]+' "$log_file" | tail -n 1 || true)"
dtb_ram_values="$(sed -E 's/.*ram=(0x[0-9a-f]+)\+(0x[0-9a-f]+).*/\1 \2/i' <<< "$dtb_ram_line")"
read -r dtb_ram_base_hex dtb_ram_size_hex <<< "$dtb_ram_values"
allocator_line="$(grep -Eio '\[mm\][[:space:]]+frame[[:space:]]+allocator[[:space:]]+ready[[:space:]]+base=0x[[:xdigit:]]+[[:space:]]+free=[0-9]+' "$log_file" | tail -n 1 || true)"
allocator_values="$(sed -E 's/.*base=(0x[0-9a-f]+)[[:space:]]+free=([0-9]+).*/\1 \2/i' <<< "$allocator_line")"
read -r allocator_base_hex allocator_free <<< "$allocator_values"
if [[ -z "$dtb_ram_line" || ! "$dtb_ram_base_hex" =~ ^0x[[:xdigit:]]+$ || ! "$dtb_ram_size_hex" =~ ^0x[[:xdigit:]]+$ \
  || -z "$allocator_line" || ! "$allocator_base_hex" =~ ^0x[[:xdigit:]]+$ || ! "$allocator_free" =~ ^[0-9]+$ ]]; then
  echo "smoke-qemu: DTB/frame allocator diagnostics are missing or malformed: ram=${dtb_ram_line:-<missing>} allocator=${allocator_line:-<missing>}" >&2
  tail -n 120 "$log_file" >&2 || true
  exit 1
fi
dtb_ram_base=$((dtb_ram_base_hex))
dtb_ram_size=$((dtb_ram_size_hex))
allocator_base=$((allocator_base_hex))
if (( allocator_base % 0x1000 != 0 )); then
  echo "smoke-qemu: frame allocator base is not 4KiB aligned: $allocator_line" >&2
  tail -n 120 "$log_file" >&2 || true
  exit 1
fi
if (( allocator_base < dtb_ram_base || allocator_base >= dtb_ram_base + dtb_ram_size )); then
  echo "smoke-qemu: frame allocator base is outside DTB RAM range: ram=$dtb_ram_line allocator=$allocator_line" >&2
  tail -n 120 "$log_file" >&2 || true
  exit 1
fi
if (( allocator_free == 0 )); then
  echo "smoke-qemu: frame allocator has no free frames: $allocator_line" >&2
  tail -n 120 "$log_file" >&2 || true
  exit 1
fi

kernel_heap_line="$(grep -Eio '\[mm\][[:space:]]+kernel[[:space:]]+heap[[:space:]]+ready[[:space:]]+base=0x[[:xdigit:]]+[[:space:]]+bytes=0x8000000' "$log_file" | tail -n 1 || true)"
kernel_heap_values="$(sed -E 's/.*base=(0x[0-9a-f]+)[[:space:]]+bytes=(0x[0-9a-f]+).*/\1 \2/i' <<< "$kernel_heap_line")"
read -r kernel_heap_base_hex kernel_heap_bytes_hex <<< "$kernel_heap_values"
task_memory_line="$(grep -Eio '\[mm\][[:space:]]+TaskMemory/page[[:space:]]+tables[[:space:]]+allocated[[:space:]]+from[[:space:]]+kernel[[:space:]]+heap[[:space:]]+slot-bytes=0x[[:xdigit:]]+[[:space:]]+allocated=0x[[:xdigit:]]+' "$log_file" | tail -n 1 || true)"
task_memory_values="$(sed -E 's/.*slot-bytes=(0x[0-9a-f]+)[[:space:]]+allocated=(0x[0-9a-f]+).*/\1 \2/i' <<< "$task_memory_line")"
read -r task_memory_slot_bytes_hex task_memory_allocated_hex <<< "$task_memory_values"
if [[ -z "${kernel_heap_line}" || ! "${kernel_heap_base_hex}" =~ ^0x[[:xdigit:]]+$ || ! "${kernel_heap_bytes_hex}" =~ ^0x[[:xdigit:]]+$ \
  || -z "${task_memory_line}" || ! "${task_memory_slot_bytes_hex}" =~ ^0x[[:xdigit:]]+$ || ! "${task_memory_allocated_hex}" =~ ^0x[[:xdigit:]]+$ ]]; then
  echo "smoke-qemu: kernel heap/TaskMemory diagnostics are missing or malformed: heap=${kernel_heap_line:-<missing>} task=${task_memory_line:-<missing>}" >&2
  tail -n 120 "$log_file" >&2 || true
  exit 1
fi
kernel_heap_base=$((kernel_heap_base_hex))
kernel_heap_bytes=$((kernel_heap_bytes_hex))
task_memory_slot_bytes=$((task_memory_slot_bytes_hex))
task_memory_allocated=$((task_memory_allocated_hex))
if (( kernel_heap_base % 0x1000 != 0 || kernel_heap_bytes != 0x8000000 )); then
  echo "smoke-qemu: kernel heap base/size is invalid: $kernel_heap_line" >&2
  tail -n 120 "$log_file" >&2 || true
  exit 1
fi
if (( kernel_heap_base < dtb_ram_base || kernel_heap_base + kernel_heap_bytes > dtb_ram_base + dtb_ram_size )); then
  echo "smoke-qemu: kernel heap is outside DTB RAM range: ram=$dtb_ram_line heap=$kernel_heap_line" >&2
  tail -n 120 "$log_file" >&2 || true
  exit 1
fi
if (( allocator_base != kernel_heap_base + kernel_heap_bytes )); then
  echo "smoke-qemu: frame allocator base does not follow kernel heap: heap=$kernel_heap_line allocator=$allocator_line" >&2
  tail -n 120 "$log_file" >&2 || true
  exit 1
fi
if (( task_memory_slot_bytes == 0 || task_memory_slot_bytes > kernel_heap_bytes || task_memory_allocated == 0 || task_memory_allocated > kernel_heap_bytes )); then
  echo "smoke-qemu: TaskMemory heap allocation is outside kernel heap budget: heap=$kernel_heap_line task=$task_memory_line" >&2
  tail -n 120 "$log_file" >&2 || true
  exit 1
fi

# The three application probes are correlated by their dynamic PID and their
# ordered entry/wait windows.  PIDs may be reused, so a global PID-only grep
# would incorrectly associate the page fault with badptr or privprobe.
first_line() {
  grep -Ein -- "$1" "$log_file" | head -n 1 | cut -d: -f1 || true
}

# EL0 writers can interleave at byte granularity, splitting one diagnostic
# across adjacent UART lines.  Search a bounded sliding byte window so the
# resourceprobe lifecycle remains observable without normalizing the raw log.
first_line_joined() {
  local pattern="$1"
  awk '
    BEGIN {
      pattern = ARGV[1]
      delete ARGV[1]
    }
    {
      gsub(/\r/, "")
      joined = window $0
      if (joined ~ pattern) {
        print NR - 1
        exit
      }
      window = joined
      if (length(window) > 8192) {
        window = substr(window, length(window) - 8191)
      }
    }
  ' "$pattern" "$log_file"
}

first_line_joined_between() {
  local pattern="$1"
  local start="$2"
  local end="$3"
  awk -v start="$start" -v end="$end" '
    BEGIN {
      pattern = ARGV[1]
      delete ARGV[1]
    }
    {
      gsub(/\r/, "")
      line = NR - 1
      if (line <= start) {
        window = $0
        next
      }
      joined = window $0
      if ((end == 0 || line < end) && joined ~ pattern) {
        print line
        exit
      }
      window = joined
      if (length(window) > 8192) {
        window = substr(window, length(window) - 8191)
      }
    }
  ' "$pattern" "$log_file"
}

first_joined_pid() {
  local pattern="$1"
  awk -v pattern="$pattern" '
    {
      gsub(/\r/, "")
      joined = window $0
      start = index(joined, "run:")
      candidate = (start > 0) ? substr(joined, start) : joined
      if (candidate ~ pattern && match(candidate, /pid[^0-9]*[0-9]+/)) {
        value = substr(candidate, RSTART, RLENGTH)
        sub(/^pid[^0-9]*/, "", value)
        print value
        exit
      }
      window = joined
      if (length(window) > 8192) {
        window = substr(window, length(window) - 8191)
      }
    }
  ' "$log_file"
}

# The resourcefault command and its entry banner can be split by another EL0
# writer.  Keep this exception bounded to these two semantic markers; all
# other required markers remain line-oriented.
resourcefault_run_joined_re='run:[[:space:]]+resourcefault[^[:cntrl:]]*pid[^[:cntrl:]]*=[0-9]+'
privprobe_run_joined_re='run:[[:space:]]+privp[^[:space:]]*robe[[:space:]]+pid[^[:cntrl:]]*=[0-9]+'
if [[ -z "$(first_line_joined "$resourcefault_run_joined_re")" ]]; then
  echo "smoke-qemu: resourcefault command marker missing across bounded UART window" >&2
  tail -n 120 "$log_file" >&2 || true
  exit 1
fi

first_line_between() {
  local pattern="$1"
  local start="$2"
  local end="$3"
  grep -Ein -- "$pattern" "$log_file" \
    | awk -F: -v start="$start" -v end="$end" \
      '$1 > start && (end == 0 || $1 < end) { print $1; exit }' \
    || true
}

if grep -Fqi -- 'XPG!' "$log_file"; then
  echo "smoke-qemu: cross-page sentinel XPG! leaked into serial output" >&2
  tail -n 120 "$log_file" >&2 || true
  exit 1
fi

crossptr_app_line="$(grep -Eio '\[app\][[:space:]]+cross-page[[:space:]]+pointer[[:space:]]+probe[[:space:]]+pid=[0-9]+[[:space:]]+entered[[:space:]]+EL0' "$log_file" | head -n 1 || true)"
crossptr_pid="$(sed -E 's/.*pid=([0-9]+).*/\1/i' <<< "$crossptr_app_line")"
crossptr_entry_line="$(first_line '\[app\][[:space:]]+cross-page[[:space:]]+pointer[[:space:]]+probe[[:space:]]+pid=[0-9]+[[:space:]]+entered[[:space:]]+EL0')"
crossptr_reject_line="$(first_line '\[isolation\][[:space:]]+cross-page[[:space:]]+user[[:space:]]+buffer[[:space:]]+rejected-before-copy[[:space:]]+status=-8[[:space:]]+task-survived=true')"
crossptr_wait_line="$(first_line "\\[proc\\][[:space:]]+cross-page[[:space:]]+pointer[[:space:]]+probe[[:space:]]+pid=${crossptr_pid}[[:space:]]+wait-status=0[[:space:]]+task-survived=true")"
crossptr_fault_line="$(awk -v start="${crossptr_entry_line}" -v end="${crossptr_wait_line}" 'NR > start && NR < end && /application fault/ { print; exit }' "$log_file")"
if [[ -z "${crossptr_app_line}" || ! "${crossptr_pid}" =~ ^[0-9]+$ || ! "${crossptr_entry_line}" =~ ^[0-9]+$ || ! "${crossptr_reject_line}" =~ ^[0-9]+$ || ! "${crossptr_wait_line}" =~ ^[0-9]+$ ]] \
  || (( crossptr_entry_line >= crossptr_reject_line || crossptr_reject_line >= crossptr_wait_line )) \
  || [[ -n "${crossptr_fault_line}" ]]; then
  echo "smoke-qemu: cross-page pointer PID/window validation failed: app=${crossptr_app_line:-<missing>} reject=${crossptr_reject_line:-<missing>} wait=${crossptr_wait_line:-<missing>} fault=${crossptr_fault_line:-<none>}" >&2
  tail -n 120 "$log_file" >&2 || true
  exit 1
fi

privileged_app_line="$(grep -Eio '\[app\][[:space:]]+privileged[[:space:]]+probe[[:space:]]+pid=[0-9]+[[:space:]]+entered[[:space:]]+EL0' "$log_file" | head -n 1 || true)"
privileged_pid="$(sed -E 's/.*pid=([0-9]+).*/\1/i' <<< "$privileged_app_line")"
privileged_entry_line="$(first_line '\[app\][[:space:]]+privileged[[:space:]]+probe[[:space:]]+pid=[0-9]+[[:space:]]+entered[[:space:]]+EL0')"
privileged_fault_line="$(awk -v start="$privileged_entry_line" -v pid="$privileged_pid" 'NR >= start && /application fault/ && index($0, "pid=" pid " terminated") { print; exit }' "$log_file")"
if [[ -z "$privileged_app_line" || ! "$privileged_pid" =~ ^[0-9]+$ || ! "$privileged_entry_line" =~ ^[0-9]+$ || -z "$privileged_fault_line" ]]; then
  echo "smoke-qemu: privileged probe application/fault PID association is missing: app=${privileged_app_line:-<missing>} fault=${privileged_fault_line:-<missing>}" >&2
  tail -n 120 "$log_file" >&2 || true
  exit 1
fi
privileged_esr="$(sed -E 's/.*ESR=(0x[0-9a-f]+).*/\1/i' <<< "$privileged_fault_line")"
if ! [[ "$privileged_esr" =~ ^0x[0-9a-fA-F]+$ ]]; then
  echo "smoke-qemu: privileged fault ESR is malformed: $privileged_fault_line" >&2
  tail -n 120 "$log_file" >&2 || true
  exit 1
fi
if [[ "$MICROSYSTEM_ARCH" == "riscv64" ]]; then
  if (( privileged_esr != 2 )); then
    printf 'smoke-qemu: privileged fault scause=%s, expected illegal-instruction cause 0x2: %s\n' \
      "$privileged_esr" "$privileged_fault_line" >&2
    tail -n 120 "$log_file" >&2 || true
    exit 1
  fi
else
  privileged_ec=$(( (privileged_esr >> 26) & 0x3f ))
  privileged_il=$(( (privileged_esr >> 25) & 1 ))
  if (( privileged_ec != 0x18 )) && ! (( privileged_ec == 0 && privileged_il == 1 )); then
    printf 'smoke-qemu: privileged fault ESR class EC=0x%x IL=%d, expected EC=0x18 or EC=0/IL=1: %s\n' \
      "$privileged_ec" "$privileged_il" "$privileged_fault_line" >&2
    tail -n 120 "$log_file" >&2 || true
    exit 1
  fi
fi
if ! grep -Eqi "wait:[[:space:]]+pid=$privileged_pid[[:space:]]+status=-8" "$log_file" \
  && ! grep -Eqi "\\[isolation\\][[:space:]]+privileged[[:space:]]+instruction[[:space:]]+task[[:space:]]+pid=$privileged_pid[[:space:]]+faulted[[:space:]]+status=-8[[:space:]]+reclaimed=true" "$log_file"; then
  echo "smoke-qemu: privileged probe status/reclaim evidence is missing for pid=$privileged_pid" >&2
  tail -n 120 "$log_file" >&2 || true
  exit 1
fi

badptr_app_line="$(grep -Eio '\[app\][[:space:]]+invalid-pointer[[:space:]]+probe[[:space:]]+pid=[0-9]+[[:space:]]+entered[[:space:]]+EL0' "$log_file" | head -n 1 || true)"
badptr_pid="$(sed -E 's/.*pid=([0-9]+).*/\1/i' <<< "$badptr_app_line")"
badptr_entry_line="$(first_line '\[app\][[:space:]]+invalid-pointer[[:space:]]+probe[[:space:]]+pid=[0-9]+[[:space:]]+entered[[:space:]]+EL0')"
badptr_reject_line="$(first_line '\[isolation\][[:space:]]+invalid[[:space:]]+user[[:space:]]+pointer[[:space:]]+rejected[[:space:]]+status=-8[[:space:]]+task-survived=true')"
badptr_wait_line="$(first_line "\\[proc\\][[:space:]]+invalid-pointer[[:space:]]+probe[[:space:]]+pid=$badptr_pid[[:space:]]+wait-status=0[[:space:]]+task-survived=true")"
badptr_fault_line="$(awk -v start="$badptr_entry_line" -v end="$badptr_wait_line" 'NR > start && NR < end && /application fault/ { print; exit }' "$log_file")"
if [[ -z "$badptr_app_line" || ! "$badptr_pid" =~ ^[0-9]+$ || ! "$badptr_entry_line" =~ ^[0-9]+$ || ! "$badptr_reject_line" =~ ^[0-9]+$ || ! "$badptr_wait_line" =~ ^[0-9]+$ ]] \
  || (( badptr_entry_line >= badptr_reject_line || badptr_reject_line >= badptr_wait_line )) \
  || [[ -n "$badptr_fault_line" ]]; then
  echo "smoke-qemu: invalid-pointer probe did not survive with status=0: ${badptr_app_line:-<missing>}" >&2
  tail -n 120 "$log_file" >&2 || true
  exit 1
fi

pageprobe_app_line="$(grep -Eio '\[app\][[:space:]]+page-fault[[:space:]]+probe[[:space:]]+pid=[0-9]+[[:space:]]+entered[[:space:]]+EL0' "$log_file" | head -n 1 || true)"
pageprobe_pid="$(sed -E 's/.*pid=([0-9]+).*/\1/i' <<< "$pageprobe_app_line")"
pageprobe_entry_line="$(first_line '\[app\][[:space:]]+page-fault[[:space:]]+probe[[:space:]]+pid=[0-9]+[[:space:]]+entered[[:space:]]+EL0')"
pageprobe_upper_line="$(first_line '\[app\][[:space:]]+resource[[:space:]]+fault[[:space:]]+probe[[:space:]]+pid=[0-9]+[[:space:]]+entered[[:space:]]+EL0')"
pageprobe_wait_re="\\[proc\\][[:space:]]+page-fault[[:space:]]+probe[[:space:]]+pid=$pageprobe_pid[[:space:]]+wait-status=-8[[:space:]]+reclaimed=true"
pageprobe_fault_re="\\[proc\\][[:space:]]+application[[:space:]]+fault[[:space:]]+ESR=0x[[:xdigit:]]+[[:space:]]+FAR=0x600000;[[:space:]]+pid=$pageprobe_pid[[:space:]]+terminated"
pageprobe_wait_line="$(first_line_between "$pageprobe_wait_re" "$pageprobe_entry_line" "$pageprobe_upper_line")"
pageprobe_fault_line="$(first_line_between "$pageprobe_fault_re" "$pageprobe_entry_line" "$pageprobe_upper_line")"
if [[ -z "$pageprobe_app_line" || ! "$pageprobe_pid" =~ ^[0-9]+$ || ! "$pageprobe_entry_line" =~ ^[0-9]+$ || ! "$pageprobe_upper_line" =~ ^[0-9]+$ || ! "$pageprobe_wait_line" =~ ^[0-9]+$ || ! "$pageprobe_fault_line" =~ ^[0-9]+$ ]] \
  || (( pageprobe_entry_line >= pageprobe_upper_line || pageprobe_entry_line >= pageprobe_wait_line || pageprobe_wait_line >= pageprobe_upper_line || pageprobe_fault_line >= pageprobe_upper_line )) \
  || ! grep -Eqi "\\[proc\\][[:space:]]+page-fault[[:space:]]+probe[[:space:]]+pid=$pageprobe_pid[[:space:]]+wait-status=-8[[:space:]]+reclaimed=true" "$log_file"; then
  echo "smoke-qemu: page-fault probe PID/fault/reclaim association is missing: app=${pageprobe_app_line:-<missing>} fault=${pageprobe_fault_line:-<missing>} wait=${pageprobe_wait_line:-<missing>} upper=${pageprobe_upper_line:-<missing>}" >&2
  tail -n 120 "$log_file" >&2 || true
  exit 1
fi

resource_loader_text="$(grep -Eio '\[proc\][[:space:]]+bootfs[[:space:]]+name-based[[:space:]]+loader[[:space:]]+program=resourceprobe[[:space:]]+pid=[0-9]+[[:space:]]+static-elf=true' "$log_file" | tail -n 1 || true)"
resource_pid="$(sed -E 's/.*pid=([0-9]+).*/\1/i' <<< "$resource_loader_text")"
if [[ -z "$resource_pid" ]]; then
  resource_reclaim_text="$(grep -Eio '\[mm\][[:space:]]+application[[:space:]]+exit[[:space:]]+reclaimed[[:space:]]+pid=[0-9]+[[:space:]]+frames=1[[:space:]]+pools=1[[:space:]]+mappings=1' "$log_file" | tail -n 1 || true)"
  resource_pid="$(sed -E 's/.*pid=([0-9]+).*/\1/i' <<< "$resource_reclaim_text")"
fi
resource_run_joined_re='run:[[:space:]]+resourcep[^[:space:]]*robe[^[:cntrl:]]*pid'
resource_command_line="$(first_line '^micro>[[:space:]]+run[[:space:]]+resourceprobe[[:space:]]*$')"
resource_upper_line="$(first_line '^micro>[[:space:]]+run[[:space:]]+resourcefault[[:space:]]*$')"
resource_run_line_number="$(first_line_joined_between "$resource_run_joined_re" "$resource_command_line" "$resource_upper_line")"
resource_run_line="joined run: resourceprobe pid=$resource_pid"
resource_entry_line="$(first_line_joined_between "\\[app\\][[:space:]]+resource[[:space:]]+cleanup[[:space:]]+probe[[:space:]]+pid=$resource_pid[[:space:]]+entered[[:space:]]+EL0" "$resource_command_line" "$resource_upper_line")"
resource_live_line="$(first_line_joined_between "\\[app\\][[:space:]]+resource[[:space:]]+cleanup[[:space:]]+probe[[:space:]]+pid=$resource_pid[[:space:]]+exiting[[:space:]]+with[[:space:]]+live[[:space:]]+pool-frame-mapping=true" "$resource_command_line" "$resource_upper_line")"
resource_wait_line="$(first_line_between "^wait:[[:space:]]+pid=$resource_pid[[:space:]]+status=0[[:space:]]*$" "$resource_command_line" "$resource_upper_line")"
resource_reclaim_line="$(first_line_between "\\[mm\\][[:space:]]+application[[:space:]]+exit[[:space:]]+reclaimed[[:space:]]+pid=$resource_pid[[:space:]]+frames=1[[:space:]]+pools=1[[:space:]]+mappings=1" "$resource_command_line" "$resource_upper_line")"
resource_loader_line="$(first_line_between "\\[proc\\][[:space:]]+bootfs[[:space:]]+name-based[[:space:]]+loader[[:space:]]+program=resourceprobe[[:space:]]+pid=$resource_pid[[:space:]]+static-elf=true" "$resource_command_line" "$resource_upper_line")"
if [[ -z "$resource_entry_line" ]]; then
  resource_entry_line="$resource_command_line"
fi
if [[ ! "$resource_pid" =~ ^[0-9]+$ || ! "$resource_command_line" =~ ^[0-9]+$ || ! "$resource_upper_line" =~ ^[0-9]+$ || ! "$resource_run_line_number" =~ ^[0-9]+$ || ! "$resource_entry_line" =~ ^[0-9]+$ || ! "$resource_live_line" =~ ^[0-9]+$ || ! "$resource_wait_line" =~ ^[0-9]+$ || ! "$resource_reclaim_line" =~ ^[0-9]+$ || ! "$resource_loader_line" =~ ^[0-9]+$ ]] \
  || (( resource_command_line >= resource_live_line || resource_run_line_number <= resource_command_line || resource_run_line_number >= resource_wait_line || resource_entry_line > resource_live_line || resource_live_line >= resource_wait_line || resource_wait_line >= resource_reclaim_line || resource_reclaim_line >= resource_loader_line || resource_loader_line >= resource_upper_line )); then
  echo "smoke-qemu: resourceprobe PID/resource cleanup lifecycle is missing or out of order: command=${resource_command_line:-<missing>} run=${resource_run_line:-<missing>} entry=${resource_entry_line:-<missing>} live=${resource_live_line:-<missing>} wait=${resource_wait_line:-<missing>} reclaim=${resource_reclaim_line:-<missing>} loader=${resource_loader_line:-<missing>} upper=${resource_upper_line:-<missing>}" >&2
  tail -n 120 "$log_file" >&2 || true
  exit 1
fi

resourcefault_run_line="$(grep -Eio '^run:[[:space:]]+resourcefault[[:space:]]+pid=[0-9]+' "$log_file" | tail -n 1 || true)"
resourcefault_pid="$(sed -E 's/^run:[[:space:]]+resourcefault[[:space:]]+pid=([0-9]+).*/\1/i' <<< "$resourcefault_run_line")"
resourcefault_run_line_number="$(first_line_joined "$resourcefault_run_joined_re")"
if [[ -z "$resourcefault_run_line" ]]; then
  resourcefault_pid="$(first_joined_pid "$resourcefault_run_joined_re")"
  resourcefault_run_line="joined resourcefault pid=$resourcefault_pid"
fi
resourcefault_upper_line="$(first_line_joined "$privprobe_run_joined_re")"
resourcefault_entry_re="\\[app\\][[:space:]]+resource[[:space:]]+fault[[:space:]]+probe[[:space:]]+pid=$resourcefault_pid[[:space:]]+entered[[:space:]]+EL0"
resourcefault_entry_line="$(first_line "$resourcefault_entry_re")"
if [[ -z "$resourcefault_entry_line" ]]; then
  resourcefault_entry_line="$(first_line_joined "$resourcefault_entry_re")"
fi
# QEMU's SMP UART can interleave the three writes used by debug_write_u64 so
# deeply reconstructing the entry banner is impossible in some runs. The
# later live-resource, fault, wait, reclaim and loader markers still prove the
# same PID lifecycle; use the command boundary when only that banner is split.
if [[ -z "$resourcefault_entry_line" ]]; then
  resourcefault_entry_line="$resourcefault_run_line_number"
fi
resourcefault_live_re="\\[app\\][[:space:]]+resource[[:space:]]+fault[[:space:]]+probe[[:space:]]+pid=$resourcefault_pid[[:space:]]+triggering[[:space:]]+page[[:space:]]+fault[[:space:]]+with[[:space:]]+live[[:space:]]+frames=2[[:space:]]+mappings=2"
resourcefault_fault_re="\\[proc\\][[:space:]]+application[[:space:]]+fault[[:space:]]+ESR=0x[[:xdigit:]]+[[:space:]]+FAR=0x600000;[[:space:]]+pid=$resourcefault_pid[[:space:]]+terminated"
resourcefault_wait_re="^wait:[[:space:]]+pid=$resourcefault_pid[[:space:]]+status=-8[[:space:]]*$"
resourcefault_reclaim_re="\\[mm\\][[:space:]]+application[[:space:]]+exit[[:space:]]+reclaimed[[:space:]]+pid=$resourcefault_pid[[:space:]]+frames=2[[:space:]]+pools=1[[:space:]]+mappings=2"
resourcefault_loader_re="\\[proc\\][[:space:]]+bootfs[[:space:]]+name-based[[:space:]]+loader[[:space:]]+program=resourcefault[[:space:]]+pid=$resourcefault_pid[[:space:]]+static-elf=true"
resourcefault_live_line="$(first_line_joined_between "$resourcefault_live_re" "$resourcefault_entry_line" "$resourcefault_upper_line")"
resourcefault_fault_line="$(first_line_between "$resourcefault_fault_re" "$resourcefault_live_line" "$resourcefault_upper_line")"
resourcefault_wait_line="$(first_line_between "$resourcefault_wait_re" "$resourcefault_live_line" "$resourcefault_upper_line")"
resourcefault_reclaim_line="$(first_line_between "$resourcefault_reclaim_re" "$resourcefault_live_line" "$resourcefault_upper_line")"
resourcefault_loader_line="$(first_line_between "$resourcefault_loader_re" "$resourcefault_live_line" "$resourcefault_upper_line")"
if [[ -z "$resourcefault_run_line" || ! "$resourcefault_pid" =~ ^[0-9]+$ || ! "$resourcefault_entry_line" =~ ^[0-9]+$ || ! "$resourcefault_upper_line" =~ ^[0-9]+$ || ! "$resourcefault_live_line" =~ ^[0-9]+$ || ! "$resourcefault_fault_line" =~ ^[0-9]+$ || ! "$resourcefault_wait_line" =~ ^[0-9]+$ || ! "$resourcefault_reclaim_line" =~ ^[0-9]+$ || ! "$resourcefault_loader_line" =~ ^[0-9]+$ ]] \
  || (( resourcefault_entry_line >= resourcefault_live_line || resourcefault_live_line >= resourcefault_upper_line || resourcefault_fault_line >= resourcefault_upper_line || resourcefault_wait_line >= resourcefault_upper_line || resourcefault_reclaim_line >= resourcefault_upper_line || resourcefault_loader_line >= resourcefault_upper_line )); then
  echo "smoke-qemu: resourcefault PID/fault/reclaim lifecycle is missing or out of window: run=${resourcefault_run_line:-<missing>} entry=${resourcefault_entry_line:-<missing>} live=${resourcefault_live_line:-<missing>} fault=${resourcefault_fault_line:-<missing>} wait=${resourcefault_wait_line:-<missing>} reclaim=${resourcefault_reclaim_line:-<missing>} loader=${resourcefault_loader_line:-<missing>} upper=${resourcefault_upper_line:-<missing>}" >&2
  tail -n 120 "$log_file" >&2 || true
  exit 1
fi

resourcekill_run_re='^run:[[:space:]]+resourcek[^[:space:]]*ill[[:space:]]+pid=[0-9]+'
resourcekill_run_line="$(grep -Eio "$resourcekill_run_re" "$log_file" | tail -n 1 || true)"
resourcekill_pid="$(sed -E 's/^run:[[:space:]]+resourcek[^[:space:]]*ill[[:space:]]+pid=([0-9]+).*/\1/i' <<< "$resourcekill_run_line")"
resourcekill_run_number="$(first_line "$resourcekill_run_re")"
resourcekill_upper_line="$(first_line_joined "$privprobe_run_joined_re")"
resourcekill_entry_re="\\[app\\][[:space:]]+resource[[:space:]]+kill[[:space:]]+probe[[:space:]]+pid=$resourcekill_pid[[:space:]]+entered[[:space:]]+EL0"
resourcekill_live_re="\\[app\\][[:space:]]+resource[[:space:]]+kill[[:space:]]+probe[[:space:]]+pid=$resourcekill_pid[[:space:]]+ready[[:space:]]+with"
resourcekill_resources_re="pid=$resourcekill_pid[[:space:]]+frames=2[[:space:]]+pools=1[[:space:]]+mappings=2"
resourcekill_kill_re="^kill:[[:space:]]+pid=$resourcekill_pid[[:space:]]+status=-15[[:space:]]*$"
resourcekill_wait_re="^wait:[[:space:]]+pid=$resourcekill_pid[[:space:]]+status=-15[[:space:]]*$"
resourcekill_reclaim_re="\\[mm\\][[:space:]]+application[[:space:]]+exit[[:space:]]+reclaimed[[:space:]]+pid=$resourcekill_pid[[:space:]]+frames=2[[:space:]]+pools=1[[:space:]]+mappings=2"
resourcekill_loader_re="\\[proc\\][[:space:]]+bootfs[[:space:]]+name-based[[:space:]]+loader[[:space:]]+program=resourcekill[[:space:]]+pid=$resourcekill_pid[[:space:]]+static-elf=true"
# SMP UART can emit the child EL0 banner before the parent's run reply.
# Bound the same-PID lifecycle by the preceding probe and the next command,
# while retaining the complete entry/live/kill/wait/reclaim/loader sequence.
resourcekill_lower_line="$resourcefault_loader_line"
resourcekill_entry_line="$(first_line_between "$resourcekill_entry_re" "$resourcekill_lower_line" "$resourcekill_upper_line")"
resourcekill_live_line="$(first_line_joined_between "$resourcekill_live_re" "$resourcekill_entry_line" "$resourcekill_upper_line")"
resourcekill_resources_line="$(first_line_joined_between "$resourcekill_resources_re" "$resourcekill_entry_line" "$resourcekill_upper_line")"
if [[ -z "$resourcekill_live_line" ]]; then
  resourcekill_live_line="$resourcekill_resources_line"
fi
resourcekill_kill_line="$(first_line_between "$resourcekill_kill_re" "$resourcekill_entry_line" "$resourcekill_upper_line")"
resourcekill_wait_line="$(first_line_between "$resourcekill_wait_re" "$resourcekill_entry_line" "$resourcekill_upper_line")"
resourcekill_reclaim_line="$(first_line_between "$resourcekill_reclaim_re" "$resourcekill_entry_line" "$resourcekill_upper_line")"
resourcekill_loader_line="$(first_line_between "$resourcekill_loader_re" "$resourcekill_entry_line" "$resourcekill_upper_line")"
if [[ -z "$resourcekill_run_line" || ! "$resourcekill_pid" =~ ^[0-9]+$ || ! "$resourcekill_lower_line" =~ ^[0-9]+$ || ! "$resourcekill_run_number" =~ ^[0-9]+$ || ! "$resourcekill_upper_line" =~ ^[0-9]+$ || ! "$resourcekill_entry_line" =~ ^[0-9]+$ || ! "$resourcekill_live_line" =~ ^[0-9]+$ || ! "$resourcekill_resources_line" =~ ^[0-9]+$ || ! "$resourcekill_kill_line" =~ ^[0-9]+$ || ! "$resourcekill_wait_line" =~ ^[0-9]+$ || ! "$resourcekill_reclaim_line" =~ ^[0-9]+$ || ! "$resourcekill_loader_line" =~ ^[0-9]+$ ]] \
  || (( resourcekill_lower_line >= resourcekill_entry_line || resourcekill_run_number <= resourcekill_lower_line || resourcekill_run_number >= resourcekill_upper_line || resourcekill_entry_line >= resourcekill_upper_line || resourcekill_live_line >= resourcekill_upper_line || resourcekill_resources_line >= resourcekill_upper_line || resourcekill_kill_line >= resourcekill_upper_line || resourcekill_wait_line >= resourcekill_upper_line || resourcekill_reclaim_line >= resourcekill_upper_line || resourcekill_loader_line >= resourcekill_upper_line || resourcekill_live_line >= resourcekill_kill_line )); then
  echo "smoke-qemu: resourcekill PID/resource lifecycle is missing or out of window: run=${resourcekill_run_line:-<missing>} entry=${resourcekill_entry_line:-<missing>} live=${resourcekill_live_line:-<missing>} resources=${resourcekill_resources_line:-<missing>} kill=${resourcekill_kill_line:-<missing>} wait=${resourcekill_wait_line:-<missing>} reclaim=${resourcekill_reclaim_line:-<missing>} loader=${resourcekill_loader_line:-<missing>} upper=${resourcekill_upper_line:-<missing>}" >&2
  tail -n 120 "$log_file" >&2 || true
  exit 1
fi

spinner_pids=( $(grep -Eio '\[app\][[:space:]]+spinner[[:space:]]+pid=[0-9]+[[:space:]]+started[[:space:]]+at[[:space:]]+EL0' "$log_file" | sed -E 's/.*pid=([0-9]+).*/\1/i' | sort -n -u) )
if (( ${#spinner_pids[@]} < 2 )) || [[ "${spinner_pids[0]}" == "${spinner_pids[1]}" ]]; then
  echo "smoke-qemu: expected two independently started spinner pids, got: ${spinner_pids[*]:-<missing>}" >&2
  tail -n 120 "$log_file" >&2 || true
  exit 1
fi

spinner_mask_line="$(grep -Eio '\[sched\][[:space:]]+application[[:space:]]+spinners[[:space:]]+dual-core=true[[:space:]]+tasks=2[[:space:]]+cpus=2[[:space:]]+cpu-masks-pair=[0-9]{2}' "$log_file" | tail -n 1 || true)"
spinner_mask_pair="$(sed -E 's/.*cpu-masks-pair=([0-9]{2}).*/\1/i' <<< "$spinner_mask_line")"
if [[ -z "$spinner_mask_line" || ! "$spinner_mask_pair" =~ ^[0-9]{2}$ ]]; then
  echo "smoke-qemu: dual-core spinner CPU mask marker is missing or malformed: ${spinner_mask_line:-<missing>}" >&2
  tail -n 120 "$log_file" >&2 || true
  exit 1
fi
spinner_first_mask=${spinner_mask_pair:0:1}
spinner_second_mask=${spinner_mask_pair:1:1}
if ! [[ "$spinner_first_mask" =~ ^[1-3]$ && "$spinner_second_mask" =~ ^[1-3]$ ]] \
  || (( (spinner_first_mask | spinner_second_mask) != 3 )); then
  echo "smoke-qemu: dual-core spinner CPU masks must each be 1..3 and cover both CPUs: $spinner_mask_line" >&2
  tail -n 120 "$log_file" >&2 || true
  exit 1
fi

# Correlate the interactive counter command, its EL0 lifecycle, and the wait
# reply by the same dynamic PID.  A generic wait/status marker can be emitted
# by an earlier probe and would not prove that `run counter` completed.
counter_run_line="$(grep -Eio '^run:[[:space:]]+counter[[:space:]]+pid=[0-9]+' "$log_file" | tail -n 1 || true)"
counter_pid="$(sed -E 's/^run:[[:space:]]+counter[[:space:]]+pid=([0-9]+).*/\1/i' <<< "$counter_run_line")"
counter_started_line="$(grep -Eio "\\[app\\][[:space:]]+counter[[:space:]]+pid=$counter_pid[[:space:]]+started[[:space:]]+at[[:space:]]+EL0" "$log_file" | tail -n 1 || true)"
counter_completed_line="$(grep -Eio "\\[app\\][[:space:]]+counter[[:space:]]+pid=$counter_pid[[:space:]]+completed" "$log_file" | tail -n 1 || true)"
counter_wait_line="$(grep -Eio "^wait:[[:space:]]+pid=$counter_pid[[:space:]]+status=0[[:space:]]*$" "$log_file" | tail -n 1 || true)"
if [[ -z "$counter_run_line" || ! "$counter_pid" =~ ^[0-9]+$ || -z "$counter_started_line" || -z "$counter_completed_line" || -z "$counter_wait_line" ]]; then
  echo "smoke-qemu: counter lifecycle did not preserve one PID through start/completion/wait: run=${counter_run_line:-<missing>} started=${counter_started_line:-<missing>} completed=${counter_completed_line:-<missing>} wait=${counter_wait_line:-<missing>}" >&2
  tail -n 120 "$log_file" >&2 || true
  exit 1
fi

counter_ps_line="$(grep -Eio '^PID[[:space:]]+[0-9]+[[:space:]]+counter[[:space:]]+exited[[:space:]]+status=0[[:space:]]*$' "$log_file" | tail -n 1 || true)"
spinner_ps_line="$(grep -Eio '^PID[[:space:]]+[0-9]+[[:space:]]+spinner[[:space:]]+exited[[:space:]]+status=-15[[:space:]]*$' "$log_file" | tail -n 1 || true)"
if [[ -z "$counter_ps_line" || -z "$spinner_ps_line" ]]; then
  echo "smoke-qemu: final ps did not preserve independent counter/spinner statuses" >&2
  tail -n 120 "$log_file" >&2 || true
  exit 1
fi
counter_ps_pid="$(sed -E 's/^PID[[:space:]]+([0-9]+).*/\1/i' <<< "$counter_ps_line")"
spinner_ps_pid="$(sed -E 's/^PID[[:space:]]+([0-9]+).*/\1/i' <<< "$spinner_ps_line")"
if [[ "$counter_ps_pid" != "$counter_pid" ]]; then
  echo "smoke-qemu: final ps counter PID does not match run counter PID: run=$counter_pid ps=$counter_ps_pid" >&2
  tail -n 120 "$log_file" >&2 || true
  exit 1
fi
if [[ "$counter_ps_pid" == "$spinner_ps_pid" ]]; then
  echo "smoke-qemu: final ps counter/spinner statuses alias one pid: counter=$counter_ps_pid spinner=$spinner_ps_pid" >&2
  tail -n 120 "$log_file" >&2 || true
  exit 1
fi

ls_demo_line="$(grep -Ei '(^|[[:space:]])/demo/hello([[:space:]]|$)' "$log_file" | grep -Evi '^[[:space:]]*micro>[[:space:]]*' | tail -n 1 || true)"
if [[ -z "$ls_demo_line" ]]; then
  echo "smoke-qemu: ls /demo did not list /demo/hello independently of command echo" >&2
  tail -n 120 "$log_file" >&2 || true
  exit 1
fi
cat_output_line="$(grep -E '^hello-microsystem[[:space:]]*$' "$log_file" | tail -n 1 || true)"
if [[ -z "$cat_output_line" ]]; then
  echo "smoke-qemu: cat /demo/hello did not return standalone hello-microsystem" >&2
  tail -n 120 "$log_file" >&2 || true
  exit 1
fi

intx_completion_line="$(grep -Eio '\[irq\][[:space:]]+virtio-blk[[:space:]]+INTx[[:space:]]+completions=[0-9]+' "$log_file" | tail -n 1 || true)"
if [[ -z "$intx_completion_line" ]]; then
  echo "smoke-qemu: VirtIO INTx completion count is missing" >&2
  tail -n 120 "$log_file" >&2 || true
  exit 1
fi
intx_completions="$(sed -E 's/.*completions=([0-9]+).*/\1/i' <<< "$intx_completion_line")"
if ! [[ "$intx_completions" =~ ^[0-9]+$ ]] || (( intx_completions == 0 )); then
  echo "smoke-qemu: VirtIO INTx completion count is not positive: $intx_completion_line" >&2
  tail -n 120 "$log_file" >&2 || true
  exit 1
fi

resident_line="$(grep -Eio '\[service\][[:space:]]+resident[[:space:]]+EL0[[:space:]]+ready=8/8[[:space:]]+online=8/8[[:space:]]+switches=[0-9]+' "$log_file" | tail -n 1 || true)"
if [[ -z "$resident_line" ]]; then
  echo "smoke-qemu: resident EL0 readiness line is missing" >&2
  tail -n 120 "$log_file" >&2 || true
  exit 1
fi
resident_switches="$(sed -E 's/.*switches=([0-9]+).*/\1/i' <<< "$resident_line")"
if ! [[ "$resident_switches" =~ ^[0-9]+$ ]] || (( resident_switches == 0 )); then
  echo "smoke-qemu: resident EL0 switches did not advance: $resident_line" >&2
  tail -n 120 "$log_file" >&2 || true
  exit 1
fi

shared_ready_line="$(grep -Eio '\[sched\][[:space:]]+resident[[:space:]]+shared-ready-queue[[:space:]]+cpus=2[[:space:]]+ticket-lock=true[[:space:]]+idle-tasks=2[[:space:]]+switches=\[[0-9]+,[0-9]+\][[:space:]]+non-idle=\[[0-9]+,[0-9]+\]' "$log_file" | tail -n 1 || true)"
if [[ -z "$shared_ready_line" ]]; then
  echo "smoke-qemu: shared ready-queue diagnostics are missing" >&2
  tail -n 120 "$log_file" >&2 || true
  exit 1
fi
shared_ready_values="$(sed -E 's/.*switches=\[([0-9]+),([0-9]+)\][[:space:]]+non-idle=\[([0-9]+),([0-9]+)\].*/\1 \2 \3 \4/i' <<< "$shared_ready_line")"
read -r shared_switches_a shared_switches_b shared_non_idle_a shared_non_idle_b <<< "$shared_ready_values"
if ! [[ "$shared_switches_a" =~ ^[0-9]+$ && "$shared_switches_b" =~ ^[0-9]+$ && "$shared_non_idle_a" =~ ^[0-9]+$ && "$shared_non_idle_b" =~ ^[0-9]+$ ]] \
  || (( shared_switches_a < 20 || shared_switches_b < 20 || shared_non_idle_a == 0 || shared_non_idle_b == 0 )); then
  echo "smoke-qemu: shared ready-queue switches/non-idle runs did not advance on both CPUs: $shared_ready_line" >&2
  tail -n 120 "$log_file" >&2 || true
  exit 1
fi

shell_cpu_line="$(grep -Eio '^PID[[:space:]]+5[[:space:]]+shell[[:space:]]+running[[:space:]]+cpu=[01]([[:space:]]|$)' "$log_file" | tail -n 1 || true)"
resident_cpu_line="$(grep -Eio '^PID[[:space:]]+[1-4][[:space:]]+[[:alnum:]_-]+[[:space:]]+resident[[:space:]]+cpu=shared([[:space:]]|$)' "$log_file" | tail -n 1 || true)"
if [[ -z "$shell_cpu_line" || -z "$resident_cpu_line" ]]; then
  echo "smoke-qemu: ps CPU ownership output missing shell/resident evidence: shell=${shell_cpu_line:-<missing>} resident=${resident_cpu_line:-<missing>}" >&2
  tail -n 120 "$log_file" >&2 || true
  exit 1
fi

notification_line="$(grep -Eio '\[irq\][[:space:]]+resident[[:space:]]+notification[[:space:]]+blocking-waits=[0-9]+[[:space:]]+irq-acks=[0-9]+[[:space:]]+sgi-wakeups=[0-9]+' "$log_file" | tail -n 1 || true)"
if [[ -z "$notification_line" ]]; then
  echo "smoke-qemu: resident notification diagnostics are missing" >&2
  tail -n 120 "$log_file" >&2 || true
  exit 1
fi
notification_values="$(sed -E 's/.*blocking-waits=([0-9]+)[[:space:]]+irq-acks=([0-9]+)[[:space:]]+sgi-wakeups=([0-9]+).*/\1 \2 \3/i' <<< "$notification_line")"
read -r notification_waits notification_acks notification_wakeups <<< "$notification_values"
if ! [[ "$notification_waits" =~ ^[0-9]+$ && "$notification_acks" =~ ^[0-9]+$ && "$notification_wakeups" =~ ^[0-9]+$ ]] \
  || (( notification_waits == 0 || notification_acks == 0 || notification_wakeups == 0 )); then
  echo "smoke-qemu: resident notification counters must all be positive: $notification_line" >&2
  tail -n 120 "$log_file" >&2 || true
  exit 1
fi

# The shell's ps/uptime responses are the observable timer contract.  Keep the
# checks tied to the guest output instead of declaring success solely from
# initialization banners: both per-CPU tick counters and uptime must advance.
tick_line="$(grep -Eio 'ticks=\[[0-9]+,[0-9]+\]' "$log_file" | tail -n 1 || true)"
if [[ -z "$tick_line" ]]; then
  echo "smoke-qemu: ps output did not contain per-CPU ticks" >&2
  tail -n 120 "$log_file" >&2 || true
  exit 1
fi
tick_values="$(sed -E 's/^ticks=\[([0-9]+),([0-9]+)\]$/\1 \2/' <<< "$tick_line")"
read -r cpu0_ticks cpu1_ticks <<< "$tick_values"
if ! [[ "$cpu0_ticks" =~ ^[0-9]+$ && "$cpu1_ticks" =~ ^[0-9]+$ ]] \
  || (( cpu0_ticks == 0 || cpu1_ticks == 0 )); then
  echo "smoke-qemu: timer ticks did not advance on both CPUs: $tick_line" >&2
  tail -n 120 "$log_file" >&2 || true
  exit 1
fi

mapfile -t uptime_values < <(
  grep -Eio 'uptime:[[:space:]]*[0-9]+[[:space:]]+ms' "$log_file" \
    | sed -E 's/[^0-9]*([0-9]+)[^0-9]*/\1/'
)
if (( ${#uptime_values[@]} < 2 )); then
  echo "smoke-qemu: expected at least two shell uptime samples (before/after time Sleep): ${uptime_values[*]:-<missing>}" >&2
  tail -n 120 "$log_file" >&2 || true
  exit 1
fi
previous_uptime=-1
for uptime_ms in "${uptime_values[@]}"; do
  if ! [[ "$uptime_ms" =~ ^[0-9]+$ ]] || (( uptime_ms == 0 || (previous_uptime >= 0 && uptime_ms < previous_uptime) )); then
    echo "smoke-qemu: shell uptime samples are not positive/monotonic: ${uptime_values[*]}" >&2
    tail -n 120 "$log_file" >&2 || true
    exit 1
  fi
  previous_uptime=$uptime_ms
done
uptime_ms="${uptime_values[${#uptime_values[@]}-1]}"

# The scheduler demo is deliberately observable at the syscall boundary: both
# EL0 workers must run to the exact iteration count, and each CPU must have
# experienced at least one timer-driven preemption.  This catches a guest that
# merely starts a second core without actually sharing CPU time.
sched_line="$(grep -Eio '\[sched\][^[:cntrl:]]*counters=\[[0-9]+,[0-9]+\][^[:cntrl:]]*timer-preemptions=\[[0-9]+,[0-9]+\]' "$log_file" | tail -n 1 || true)"
if [[ -z "$sched_line" ]]; then
  echo "smoke-qemu: scheduler completion line is missing" >&2
  tail -n 120 "$log_file" >&2 || true
  exit 1
fi
if ! grep -Eqi 'counters=\[80000000,80000000\]' <<< "$sched_line"; then
  echo "smoke-qemu: EL0 scheduler counters did not reach 80000000 each: $sched_line" >&2
  tail -n 120 "$log_file" >&2 || true
  exit 1
fi
preempt_values="$(sed -E 's/.*timer-preemptions=\[([0-9]+),([0-9]+)\].*/\1 \2/i' <<< "$sched_line")"
read -r cpu0_preemptions cpu1_preemptions <<< "$preempt_values"
if ! [[ "$cpu0_preemptions" =~ ^[0-9]+$ && "$cpu1_preemptions" =~ ^[0-9]+$ ]] \
  || (( cpu0_preemptions == 0 || cpu1_preemptions == 0 )); then
  echo "smoke-qemu: timer preemptions did not advance on both CPUs: $sched_line" >&2
  tail -n 120 "$log_file" >&2 || true
  exit 1
fi

# The single-core round-robin demo exercises the full IRQ context frame.  It
# must perform exactly twenty switches and make progress in both independent
# user counters before returning to the kernel.
rr_line="$(grep -Eio '\[sched\][^[:cntrl:]]*round-robin[^[:cntrl:]]*context-switches=[0-9]+[^[:cntrl:]]*progress=\[[0-9]+,[0-9]+\]' "$log_file" | tail -n 1 || true)"
if [[ -z "$rr_line" ]]; then
  echo "smoke-qemu: round-robin context-switch line is missing" >&2
  tail -n 120 "$log_file" >&2 || true
  exit 1
fi
if ! grep -Eqi 'context-switches=20' <<< "$rr_line"; then
  echo "smoke-qemu: round-robin switch count was not 20: $rr_line" >&2
  tail -n 120 "$log_file" >&2 || true
  exit 1
fi
rr_progress="$(sed -E 's/.*progress=\[([0-9]+),([0-9]+)\].*/\1 \2/i' <<< "$rr_line")"
read -r rr_a rr_b <<< "$rr_progress"
if ! [[ "$rr_a" =~ ^[0-9]+$ && "$rr_b" =~ ^[0-9]+$ ]] \
  || (( rr_a == 0 || rr_b == 0 )); then
  echo "smoke-qemu: round-robin progress did not advance for both threads: $rr_line" >&2
  tail -n 120 "$log_file" >&2 || true
  exit 1
fi

# The FP/SIMD context demo is a real EL0 preemption regression, not a host
# marker: each worker keeps a distinct architecture FP-register pattern and
# control state, then verifies it after every timer-driven resume.  Require
# explicit checks and zero mismatches so a kernel that merely prints an
# isolation banner cannot satisfy the gate.
fp_simd_line="$(grep -Eio "\\[sched\\][[:space:]]+fp-simd[[:space:]]+context-isolation=true[[:space:]]+tasks=2[[:space:]]+context-switches=[0-9]+[[:space:]]+checks=\\[[0-9]+,[0-9]+\\][[:space:]]+mismatches=\\[0,0\\][[:space:]]+$fp_state_marker[[:space:]]+signatures=\\[0x11,0x22\\]" "$log_file" | tail -n 1 || true)"
if [[ -z "$fp_simd_line" ]]; then
  echo "smoke-qemu: FP/SIMD context-isolation marker is missing or malformed" >&2
  tail -n 120 "$log_file" >&2 || true
  exit 1
fi
fp_simd_switches="$(sed -E 's/.*context-switches=([0-9]+).*/\1/i' <<< "$fp_simd_line")"
fp_simd_checks="$(sed -E 's/.*checks=\[([0-9]+),([0-9]+)\].*/\1 \2/i' <<< "$fp_simd_line")"
fp_simd_mismatches="$(sed -E 's/.*mismatches=\[([0-9]+),([0-9]+)\].*/\1 \2/i' <<< "$fp_simd_line")"
read -r fp_simd_checks_a fp_simd_checks_b <<< "$fp_simd_checks"
read -r fp_simd_mismatches_a fp_simd_mismatches_b <<< "$fp_simd_mismatches"
if ! [[ "$fp_simd_switches" =~ ^[0-9]+$ && "$fp_simd_checks_a" =~ ^[0-9]+$ && "$fp_simd_checks_b" =~ ^[0-9]+$ \
  && "$fp_simd_mismatches_a" =~ ^[0-9]+$ && "$fp_simd_mismatches_b" =~ ^[0-9]+$ ]] \
  || (( fp_simd_switches < 20 || fp_simd_checks_a == 0 || fp_simd_checks_b == 0 \
    || fp_simd_mismatches_a != 0 || fp_simd_mismatches_b != 0 )); then
  echo "smoke-qemu: FP/SIMD state was not independently checked across preemption: $fp_simd_line" >&2
  tail -n 120 "$log_file" >&2 || true
  exit 1
fi

echo "smoke-qemu: PASS (serial milestones present)"
echo "smoke-qemu: timers cpu0=$cpu0_ticks cpu1=$cpu1_ticks uptime_ms=$uptime_ms"
echo "smoke-qemu: kernel-heap base=$kernel_heap_base_hex bytes=$kernel_heap_bytes_hex frame-base=$allocator_base_hex"
echo "smoke-qemu: task-memory slot-bytes=$task_memory_slot_bytes_hex allocated=$task_memory_allocated_hex"
echo "smoke-qemu: scheduler counters=[80000000,80000000] timer-preemptions=[$cpu0_preemptions,$cpu1_preemptions]"
echo "smoke-qemu: round-robin context-switches=20 progress=[$rr_a,$rr_b]"
echo "smoke-qemu: fp-simd context-switches=$fp_simd_switches checks=[$fp_simd_checks_a,$fp_simd_checks_b] mismatches=[$fp_simd_mismatches_a,$fp_simd_mismatches_b] $fp_state_label"
echo "smoke-qemu: shared ready-queue switches=[$shared_switches_a,$shared_switches_b] non-idle=[$shared_non_idle_a,$shared_non_idle_b]"
echo "smoke-qemu: virtio-blk INTx completions=$intx_completions"
echo "smoke-qemu: resident EL0 ready=8/8 online=8/8 switches=$resident_switches"
echo "smoke-qemu: resident notification blocking-waits=$notification_waits irq-acks=$notification_acks sgi-wakeups=$notification_wakeups"
echo "smoke-qemu: mfs segment-directory active-segments=$mfs_active_segments"
echo "smoke-qemu: file-chain mkdir/touch/write/cp/fsync/sync/ls/cat/rm/rmdir=ok"
echo "smoke-qemu: log=$log_file"
