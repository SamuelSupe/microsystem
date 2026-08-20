# Runbook and acceptance model

This runbook describes the current AArch64 and RV64GC OrbStack/QEMU paths. The
recorded evidence includes AArch64 build and serial shutdown, and a RISC-V QEMU
`virt` run through serial shutdown and the IOMMU/device gates below. This
documentation pass does not rerun the end-to-end tests. The exact full test
matrices remain `validation in progress`.

## Build and run

Use OrbStack as the Docker context and keep the Rust toolchain selected by the
repository image:

```sh
docker context show
docker info --format '{{.OperatingSystem}} | {{.ServerVersion}}'
make build
make test
make fsck
make run                 # serial shell; SSH host port defaults to 2222
make gui                 # GUI profile, VNC/QMP as printed by xtask
make ssh                 # SSH acceptance client
ARCH=aarch64 make test   # validation in progress
ARCH=riscv64 make test   # validation in progress
```

`make test` is the unified gate. It builds the image, runs host tests and then
the normal, power-cut, fault-injection/recovery, MFS1 crash campaign, GUI, SSH,
and Mica stages. The test harness uses the repository's OrbStack image and QEMU
configuration; a host KVM accelerator is not required.

## Target matrix and current RISC-V validation

| Target | Current status | Evidence boundary |
| --- | --- | --- |
| AArch64 + QEMU `virt-7.2` | Build and serial run reach shutdown | `make test` built-in host suites `81/81`; separate shell parser `6/6`; `87/87` combined recorded checks. |
| RV64GC + QEMU `virt` | Serial run reaches shutdown | OpenSBI M-mode → S-mode, Sv39, two harts, PLIC/SBI, PCI VirtIO and RISC-V IOMMU gates are verified below. |

The Makefile and `xtask` select the target, QEMU binary, default OpenSBI firmware
and `iommu-sys=on` from `ARCH`. The validated RISC-V chain is RV64GC → QEMU
`virt` → OpenSBI M-mode → S-mode → Sv39 → two harts → PLIC/SBI → PCI VirtIO →
RISC-V IOMMU.

```sh
ARCH=riscv64 make build
ARCH=riscv64 make run
```

The RISC-V serial run records `[system] shutdown` and the IOMMU fault gate:

```text
[iommu] fault-probe blocked=true sentinel=true event=0xf stream-id=0x10 completion-error=false
```

The same run verifies DMA map/unmap, RNG first-fill, MFS recovery with clean
`fsck`, the complete serial-shell filesystem command matrix, the Mica 8 KiB
argument/input path, and network/DNS. The exact `make ARCH=aarch64 test` and
`make ARCH=riscv64 test` full matrices are still `validation in progress` and
must not be recorded as PASS.

The corresponding low-level command shape is:

```sh
ARCH=riscv64 cargo build --release \
  --target riscv64gc-unknown-none-elf \
  -p microsystem-kernel --features baremetal
ARCH=riscv64 qemu-system-riscv64 \
  -machine virt,iommu-sys=on -bios default -cpu rv64 \
  -kernel target/riscv64gc-unknown-none-elf/release/microsystem-kernel \
  -nographic
```

For this command, QEMU `virt` loads its default OpenSBI firmware. OpenSBI owns
M-mode and hands the kernel an S-mode entry and DTB; the kernel uses SBI for
firmware-owned hart, timer/IPI and reset services. The repository does not ship
a checked-in OpenSBI image. The current run validates the default firmware path;
the full architecture matrices remain in progress.

## Final gate

| Evidence | Result |
| --- | --- |
| AArch64 build | passed |
| AArch64 serial run | reached `[system] shutdown` |
| RV64GC QEMU `virt` serial run | reached `[system] shutdown` |
| RISC-V IOMMU fault probe | `blocked=true sentinel=true event=0xf stream-id=0x10 completion-error=false` |
| `make test` built-in host suites | `81/81` (`22+9+6+9+11+24`) |
| Separate shell-parser run | `6/6` |
| Combined recorded host checks | `87/87` |
| `make ARCH=aarch64 test` | `validation in progress` |
| `make ARCH=riscv64 test` | `validation in progress` |

The retained AArch64 normal, power-cut, fault-cut plus GC, GUI, SSH, and Mica
stages passed. Its final `make fsck` also exited zero:

```text
MFS1 clean generation=2544 transactions=2381 entries=24 used_blocks=11898
```

The smoke scheduler marker was:

```text
[sched] fp-simd context-isolation=true tasks=2 context-switches=20 checks=[603697,686002] mismatches=[0,0] q-regs=q0-q31 fpcr-fpsr=true signatures=[0x11,0x22]
```

The final gate keeps the ordinary 256-byte GPR exception frame on non-switch
SVC paths. A real `preempt::save`/`load` switch carries the separate 528-byte
q0-q31 + FPCR/FPSR context; boot enables FP/Advanced SIMD on both CPUs and each
EL0 entry clears that state. Rust and services remain `aarch64-unknown-none-softfloat`;
SVE and a hard-float ABI are not supported.

The browser fixture gate also passed. The GUI fixture served exactly
`GET /index.html` and `GET /next.html`, rejected the cross-origin `evil` request,
and observed two HTTP 200 Reader loads. The network fixture observed
`GET /index.txt?browse=1` after stripping `#fragment`, DNS for `mica.test`,
raw resolve/TCP/UDP and POST denials under `net.browse`, and trusted TLS plus
all four certificate rejection cases. Endpoint 1 registered, presented,
reclaimed, and the Reader exited with status 0. The browser screenshot CRCs
were `2286d988`, `5dab179e`, and `f60453d9` for pre-navigation, same-origin
navigation, and cross-origin rejection. Evidence is retained in
`target/unified-browser-public-internet-final.log`,
`target/unified-browser-public-internet-fsck.log`, `target/gui-browser-fixture.log`,
and `target/mica-dns-fixture.log`.

The serial shell help surface is:

```text
shell: help pwd cd echo clear history ps kill wait uptime sleep date free sysinfo
files: ls cat head tail wc hexdump xxd grep find tree du df stat touch cp write
       mkdir rmdir mv rm fsync sync (also available through fs <command>)
network: curl nslookup netstat
programs: run mica
system: exit shutdown poweroff reboot
```

Filesystem aliases include `ls/list`, `cat/read`, `touch/create`, `mv/rename`,
and `rm/remove/unlink`; bounded options are `head/tail -n N`, `mkdir -p`,
`cp -r`, and `rm -r`. The GUI Terminal accepts `ls/list [path]`, `cat/read`,
`stat`, `touch/create`, `cp`, `write`, `mkdir`, `rmdir`, `mv/rename`,
`rm/remove/unlink`, `fsync` and `sync` with absolute paths and no working
directory. It uses the
dedicated `TERMINAL_FILESYSTEM_ENDPOINT` plus the `GUI_TERMINAL_COMMANDS`
4 KiB shared frame; the MFS broker keeps this path separate from block DMA and
other filesystem payload frames. Input commands are capped at 512 bytes and
output/replies at 4 KiB, with the viewport following the newest lines. The
serial shell and GUI terminal share the filesystem parser and aliases.

The serial shell also has a bounded curl-like GET command:

```text
curl [-s|--silent] [-i|--include] [-o|--output <absolute-path>] <http[s]://url>
curl -i https://example.com/
curl -s -o /data/response https://example.com/
```

`-i` prints the HTTP status; without `-o`, the response body must be UTF-8 and
is printed to serial; `-o` writes the body atomically to MFS with fsync. The
request is GET only through `net.browse`, accepts HTTP or HTTPS URLs, and reads
at most 32 KiB. HTTP is plaintext; HTTPS uses trusted TLS certificate, hostname
and time validation. Raw TCP/UDP, POST/PUT/PATCH/DELETE, and low-level TLS are
outside this command boundary. Process, network, Mica, and power commands
remain serial-shell commands rather than GUI Terminal commands.

The unified GUI marker sequence was `mkdir`, `touch`, `write`, `cp`, `stat`,
`mv`, `cat`, `ls`, `fsync`, `rm`, `rm`, `rm`, `rmdir`, `sync`, each with
`status=ok`. The Terminal help screenshot had
140 foreground rows and CRC `b3c61c67`, confirming shared-frame output instead
of the legacy 40-byte inline reply.

The P0 MFS checks require a failed `fsync` to leave the dirty mutation list in
its original order for retry. `WriteAtomic` uses one `replace_file` transaction
(`Remove(target)` + `Rename(temp,target)`) and a durable final fsync; all four
fault points remounted as old+temp or new+no-temp before retrying to new+no-temp.
Directory descendant renames are rejected without dirty mutation. P1 GUI
partial Present is bounded across user-rt, kernel, GPU and windowd, carries
pointer damage, and caps pending events at 16; the marker is
`rect=1024x720+0,48`. VirtIO-net reports `rx-buffers=2`; the renderer marker is
`damage-merge=true local-window-damage=true command-cull=true glyph-cache=2way
input-batch=16`.

### MFS1 resilience and repair gate

The MFS1 host campaign runs 10,024 seeded-randomized, deterministic cases with
seed `0x4d46533143524153`. Its sparse volatile/durable model makes a crash discard
volatile writes; each failure is reproducible from its case, seed and event.
Triggered classes are write I/O error `1668`, short write `1711`, 512-byte
torn-sector `1626`, and flush I/O error `5019`.

The two format-v1 superblock copies are independently checksum-checked and
must each pass head validation and complete record replay. `fsck` reports
checksum/replay health for both copies and exits non-zero for degraded `1/2`.
`mfsctl repair IMAGE` is offline and can only heal one checksum-invalid stale
mirror after the other copy fully replays and a whole-image scan finds no later
`Commit` or ambiguous record. Latest corruption, double corruption, read I/O
errors and record ambiguity are refused without writing. The positive stale-
mirror case preserves payload; active-latest and double-corruption cases keep
their payload SHA unchanged. This does not repair data blocks or provide
general data recovery.

The GUI renderer merges overlapping damage and redraws only local `DesktopVisual`
windows for drag, resize, focus and Terminal updates; clean display-list
command/text/glyph work is culled. Input is batched at 16 events and Files and
Monitor wait on event IPC. OrbStack QEMU TCG proves functional regression and
reduced work only, not native-hardware performance or a percentage speedup.

The same GUI gate also passed the Mica Editor example. It used exact
`fs.read:/data` and `fs.write:/data` rules, loaded the CRLF fixture, dispatched
line selection/Apply/Save/Reload, completed one WriteAtomic with
`atomic=true fsync=true`, delivered CloseRequested, reclaimed endpoint 1 and
reported `mica: pid=14 status=0`. The persistence crop remained stable after
Reload; refresh-run Editor full-frame CRCs (pre/focused/typed/apply/saved/reload)
were `8d767f5a/5ea9fc60/dcccc6a3/da105c73/18f38838/7e895e1d`, with
persistence crop `0a8ab59f` for both Apply and Reload.

The desktop launcher gate also exited 0. The fixed five-icon registry contains
Terminal, Files, Monitor, Reader and Editor; built-ins focus/restore, while
Reader/Editor send only an application id to init's endpoint slot 84. The
Editor click (`application=5`) was accepted, registered and presented;
duplicate click and minimize/restore created no second session. Close reclaimed
the endpoint, and a subsequent click registered/presented/reclaimed a fresh
session. Unified icon JSON reported launches=2, registrations=2, Presents=2
and reclaims=2. Refresh-run icon CRCs were `5cb170eb`, `74eebaaa`, `05025d7a`
and `f686e259` (pre, active, restored, restart). The fixture and QEMU cleanup
completed without residue.

The normal serial profile reports bootfs `24/23` (24 entries, 23 static
ELFs), `SERVICE_COUNT=12`, resident ASIDs `0x20..0x2b`, serial ready/online
`8/8`, and the first dynamic process PID `13`. The GUI profile reports
GUI ready/online `12/12`. The kernel heap budget is `0x8000000` (128 MiB);
ordinary user tasks have a 64 KiB stack and 1 MiB heap, and a Mica image is
loaded at `0xe0000`.

## Stable serial markers

These marker fields are useful when diagnosing a boot or profile mismatch
(the service fields may be emitted on one line):

```text
[bootfs] valid=true entries=24 static-elfs=23
[service] ... address-spaces=12 asids=[0x20..0x2b] ... ready=8/8 online=8/8
[service] ... address-spaces=12 asids=[0x20..0x2b] ... ready=12/12 online=12/12
[proc] dynamic application capacity=8 first-pid=13
[mm] kernel heap ready bytes=0x8000000
[net] ... raw-device-isolated=true tcp-owner=true dns=true
[ssh] sshd ready address=10.0.2.15 port=22 auth=publickey user=micro
[mica] session isolated=true brokers=fs,network,process time=true
[mica] vm verified=true gc=mark-sweep pcall=true modules=true
[gui] mica registration requested endpoint=0
[gui] mica registration received endpoint=0
[gui] mica client registered command=65536 event=4096 endpoint-isolated=true
[mica] gui presented widgets=true atomic=true isolated=true
[mica] vm returned=true
[mica] exiting=true
[gui] mica client unregistered endpoint=0 resources-reclaimed=true
[system] shutdown
```

The exact boot line ordering is not an API; use the profile counts and the
final gate for acceptance.

## Power cut, fault recovery, and GC

The power-cut stage records a cut token, reboots, and checks that the committed
transaction is recovered exactly. Fault-injection cuts are expected at the
transaction boundaries; each run must either retain the prior committed state
or complete the transaction, and then leave a clean superblock. The GC fault
case explicitly checks that a candidate superblock was flushed before the cut.
These QEMU cuts are bounded recovery probes; the deterministic 10,024-case
campaign supplies the broader write/flush fault matrix, but neither claims
coverage of every physical power-loss mode.

For a standalone check, run `make fsck` after the stage. A non-zero exit,
degraded superblock health, or a different clean
generation/transaction/entry/usage tuple is a release blocker. If fsck reports
`checksum-valid=1/2`, use `mfsctl repair IMAGE` only within the offline repair
boundary described above; do not use it as a general data-recovery tool.

## Profiles and service boundaries

The serial profile enables the eight boot services that can operate without a
GPU. The GUI profile enables all twelve services, including `windowd` and the
terminal/GUI path. `SERVICE_COUNT` and the address-space/ASID range come from
the kernel service table, not from a hand-maintained marker.

`netd` is the only task with the raw `NETWORK_DEVICE` capability. Applications,
Mica, and `sshd` receive broker endpoints and bounded shared buffers; raw
`NetReceive`/`NetSend` are kernel-gated to `netd`. See
[network.md](network.md), [architecture.md](architecture.md), and
[abi.md](abi.md) for the cap and protocol contracts.

MFS remains the durable boundary. A process receives an endpoint and transient
broker frames rather than a shared filesystem/device frame. The Mica trusted
CA bundle is read per process through the exact MFS path
`/.system/certs/ca-bundle.derpack` with the trusted-read operation; it is not a
shared `FrameRegion`.
The bundle preserves the pinned Mozilla-derived 121-root snapshot and appends
deduplicated certificates from the build container's `SSL_CERT_FILE`. `xtask`
generates it before compiling the Mica service, injects the full SHA-256, and
the runtime validates the complete MCAB before use; each certificate is at
most 16 KiB and the complete bundle at most 256 KiB.

## SSH and Mica smoke

`make ssh` connects as `micro` with the generated Ed25519 key. The accepted
shell commands are `help`, `uptime`, `ps`, `clear`, `echo`, `mica`, and `exit`.
`run mica` is deliberately rejected by the ordinary shell. The SSH Mica policy
is read from `/.system/ssh/mica-policy` and intersected with requested
`--allow` permissions; wrong or missing keys must exit 255.

The Mica acceptance stage covers `mica -e`, REPL, file/argument execution,
modules, filesystem, DNS/TCP/UDP/HTTP, and trusted HTTPS. The bundled Reader's
unscoped `net.browse` policy covers only its HTTP/HTTPS GET path to a dynamic
address-bar host; raw resolve/TCP/UDP and POST/PUT/PATCH/DELETE remain exact
`net.connect:host:port` operations. Page links are same-origin, while an
explicit address-bar URL can cross origin. The unified xtask
stage sets `MICROSYSTEM_MICA_TLS_FIXTURE=1`; it observes trusted body
`mica-https-ok`, a presented root-chain, P-384 `CertificateVerify` and a long
CA DNS name, while rejecting unknown-CA, expired, not-yet-valid and hostname
mismatch certificates. When the presented chain tail equals the trust anchor,
the verifier omits that redundant tail before revalidation. The language and VM
contract, limits, stdlib, permissions, and TLS boundaries are documented in
[mica.md](mica.md). In particular, low-level `net.tls_connect` is currently
`NotSupported`; trusted HTTPS is the supported Mica runtime path. HTTPS uses
TLS 1.3, hostname/time/chain validation, RSA-PSS and P-384 ECDSA
CertificateVerify, and the
fixed Mozilla MCAB bundle described in [mica.md](mica.md).

The dynamic GUI Counter stage additionally covers `mica --gui --timeout 86400s`, the
`gui.window` permission intersection, isolated registration, atomic Present,
button/ASCII/Unicode input, resize/maximize/restore/minimize, taskbar, close,
endpoint reclaim and slot reuse. Unicode accumulators `4/78/1250/20013` were
delivered/rendered. The authoritative unified Terminal-run 1024×768 CRCs include
desktop `b1998cf3`, Terminal-help `b3c61c67`, browser pre/link/cross
`2286d988/5dab179e/f60453d9`, editor pre/apply/saved/reload
`3eeb31ac/da105c73/18f38838/7e895e1d`, and stable client `9014e4dd`.
Geometry checks
reported active-title runs max `962`, resize/restore `418`, minimized `0`,
and taskbar `121`; the active title color is `#1e3b63`. Endpoint 1
registered/presented/reclaimed twice and was reused; endpoint 0 was reclaimed.
The second and third sessions ran via persistent serial while the first
remained on SSH, proving GUI-session concurrency rather than SSH concurrency.
Final acceptance logs are `target/unified-mfs1-resilience-final.log` and
`target/unified-mfs1-resilience-fsck.log` (GUI/QMP details are in
`target/gui-qemu.qmp.jsonl`); browser-specific fixture evidence remains in the
browser and network logs named above.
The public Reader capture [`target/browser-internet-reader-imagebound-final.png`](../target/browser-internet-reader-imagebound-final.png)
shows `https://example.com/` returning `HTTP 200 — https://example.com` and
rendering `# Example Domain`.
The timeout parser accepts `1ms` through `24h`; non-GUI Mica is hard-capped at
60 seconds, while the final GUI Editor invocation used `--timeout 86400s`.

## Failure interpretation

- No `[bootfs]` or an unexpected 24/23 count: inspect image construction before
  debugging services.
- Serial `8/8` or GUI `12/12` not reached: inspect the profile/device path and
  the first service marker; do not infer a process leak from a partial boot.
- A raw network syscall from a non-`netd` task must fail; a successful raw
  receive/send outside `netd` is an isolation regression.
- TLS negative cases (unknown CA, expired, future, hostname mismatch) must be
  rejected. A successful HTTP response alone is not HTTPS acceptance.
- A dirty `make fsck` result after a fault stage is a storage/recovery failure,
  even when the QEMU process exited normally.
- A `checksum-valid=1/2` or `replay-verified=1/2` MFS1 result is degraded
  metadata and must fail the gate. `mfsctl repair IMAGE` is allowed only for
  the single checksum-invalid stale-mirror case; latest/double corruption,
  read-I/O and ambiguous-record cases must remain unchanged and fail closed.

For detailed wire numbers and cap slots, use [abi.md](abi.md); for GUI input
and rendering scope use [gui.md](gui.md); for MFS1 recovery and GC history use
[mfs1.md](mfs1.md).
