# Runbook and acceptance model

## Software/QEMU maturity phase, 2026-10-06

The current implementation ledger is `.workflow/os-maturity/plan.md`. Physical
machine acceptance is deferred by the user; remaining functionality stays open.

### Published snapshot status

These are recorded implementation-phase results; publishing the snapshot does
not rerun or complete the matrices. `target/` logs stay local and are ignored by Git.

| Check | Latest recorded result | Local evidence |
| --- | --- | --- |
| Storage host regressions | PASS: 25 MFS and four filesystem tests | `target/maturity-remediation-storage-host.log` |
| GUI, identity, init, kernel, Mica, network and shell host suites | PASS | `target/maturity-current-host.log` |
| AArch64 dual-interface IPv6/DNS/failover and three hotplug cycles | PASS on kernel `0eb4c9cf4ea86155ee3ec7a95675f148d57d423fbf1a58ef61a18288853d3912` | `target/maturity-final-network-runtime.log` |
| RISC-V corresponding network gate | PASS on kernel `f9dd22bf9a1bbd099725a7465348f0c39fa3aaf9343584bb65da9130967b78d9` | `target/maturity-riscv64-network-current-runtime.log` |
| AArch64 full matrix | Host suites and two-boot serial smoke PASS; power-cut stage FAIL; later stages not reached | `target/maturity-matrix-aarch64.log` |
| Mounted MFS recovery after block restart | FAIL: readiness timed out | `target/maturity-pressure-diag-runtime.log` |
| Twelve write/fsync/native/network lifecycle rounds | PASS, no page drift after warmup; 0.716–1.343 s per round | `target/maturity-soak-fixed-runtime.log` |

The lifecycle run is a short baseline. Long soak, current SSH 36-login/key/listener
qualification and all three complete architecture matrices remain unfinished.
Mounted cold startup is intermittent and has exceeded 120 s. The earlier focused
results below apply to their recorded kernels and do not supersede these failures.

Desktop qualification now includes `make test-desktop`: two complete application
lifecycles cover independent windows, quota/reuse, widget isolation, cross-window
clipboard, Pinyin candidate/preedit/cancel and UTF-8 deletion. The images in
`target/desktop-{composition,two-windows,one-window}.png` were inspected; the
Chinese glyphs and inactive-window focus display correctly. The gate reports
PASS in `target/maturity-desktop-runtime.log`, kernel SHA-256
`b76275e0a72da25160c0cb8373ca7c4f5c81cb459624238690d53132812bada2`.

Storage host suites report 25/25 PASS in `target/maturity-storage-host.log`,
including the existing 10,024-case crash campaign, range write cost and power
cuts, and metadata/GC/renamed-child fsync. `target/maturity-storage-runtime.log`
reports the full recovery gate PASS with chmod/chown retained after mfs restart,
kernel SHA-256 `d0d7d712ad2cbf736203946a0a8f9e39d8c41a79b49d588a3f3601f496e5bdaa`.
`target/maturity-vfs-runtime.log` also passes two mounted 3 MiB volumes,
longest-prefix isolation, backing-image protection, readonly/attribute persistence
over two MFS restarts and per-volume df (768 blocks), kernel SHA-256
`cbf5797eadbcc4328a0d6a7eb0fad88db1de3123647f6e914357a8d31882448d`.
`target/maturity-storage-current-host.log` reruns all 25 MFS tests after batched
image writes; the 10,024-case power-cut campaign passes. This is AArch64 runtime
evidence; later identity checks are recorded in the maturity ledger.
Other-architecture desktop/storage and full soak work remains open.

`make test-network` adds two QEMU user networks and a local HTTP fixture. Its
current implementation gate covers DHCP, SLAAC/IPv6 TCP, longest-prefix routing,
preferred-interface failover, persisted static configuration and invalid routes.
The maturity ledger records the completed gate when runtime acceptance succeeds.

The AArch64 `make test-recovery` gate uses an isolated disk copy and covers
native install/update/corruption rejection/rollback, 16 process slots, physical
memory exhaustion with zero partial commit, reclamation and reuse, asynchronous
Terminal cancellation, client reconnection, database hold/resume, network restart,
and MFS/block/devmgr dependency recovery. It also cancels a command after the
complete GUI has restarted. `target/native-pressure-64m-runtime.log` exited 0;
its kernel SHA-256 is `339300f3137c32b0f300008347d5d9679a47e7b8bfa06219a4a1a3fec06db96d`.
`target/maturity-native-host.log` records 31 passing host tests.

The kernel heap reservation is now 64 MiB. User heaps are demand-backed, and
fallible allocations commit storage before returning it. The scheduler's former
57 KiB persistent initializer stack frame was replaced by in-place initialization;
AArch64 disassembly shows about 12 KiB. This resolves the reproduced stack
corruption during service teardown. Frame-region lookup borrows its entry.

RISC-V's earlier two-boot smoke completed guest commands but the acceptance
script failed on a fragmented resource lifecycle record. Console output now
shares the kernel UART lock, and structured shell output is emitted in one call;
the refreshed RISC-V serial smoke passed in `target/maturity-riscv64-smoke-check.log`.
x86-64 ELF compilation passed, but native OrbStack
has no grub-mkrescue; the existing Docker image produced its ISO instead, and the
serial smoke passed in `target/maturity-x86_64-smoke-check.log`. Current desktop,
storage and full soak/performance qualification on those profiles remain pending.

This runbook describes the current AArch64, RV64GC, and x86_64 OrbStack/QEMU
paths. The recorded evidence includes AArch64 build and serial shutdown, a
RISC-V QEMU `virt` run through serial shutdown and the IOMMU/device gates below,
and a focused x86_64 QEMU acceptance through resident services, VirtIO-blk,
MFS, and serial-shell shutdown. The functionality gate below records fresh
focused checks. The complete `make test` matrices have not all passed for this
snapshot; see the latest status above.

## Functionality refinement (2026-10-05)

This pass adds whole quoted file arguments, serialized `Append=17`, directory
destinations for `mv`, and a real application list in GUI Terminal `ps`. It
also initializes every SMMUv3 context descriptor and separates terminal prompts
from file output without modifying file content. Existing unrelated worktree
changes were retained.

The following checks ran in OrbStack; this is focused evidence, not a complete
`make test` matrix:

| Check | Result | Evidence under `target/` |
| --- | --- | --- |
| ABI, kernel, MFS host tests | PASS, 44 tests | `functional-host-tests.log` |
| Shell parser, including quoted paths and rejected malformed mutations | PASS, 11 tests | `functional-parser-tests.log` |
| AArch64 full build | PASS | `functional-aarch64-build.log` |
| RISC-V full build and current windowd check | PASS | `functional-riscv64-build.log`, `functional-riscv64-windowd-check.log` |
| RISC-V/x86_64 affected service and baremetal kernel checks | PASS | `functional-{riscv64,x86_64}-check.log`, `functional-{riscv64,x86_64}-kernel-check.log`, `functional-x86_64-windowd-check.log` |
| AArch64/RISC-V serial smoke, including restart recovery | PASS | `functional-{aarch64,riscv64}-smoke-check.log` and corresponding serial logs |
| AArch64 GUI keyboard input, quoted append/move/fsync/read and application ps | PASS; screenshots inspected | `functional-gui-check.log`, `functional-gui.log`, `functional-gui-files.png`, `functional-gui-ps.png` |
| GUI append+fsync read after a fresh serial boot | PASS, exact content `hello gui world` | `functional-append-recovery-check.log`, `functional-append-recovery.log` |
| Offline filesystem audits | All clean | `functional-aarch64-fsck.log` (generation 79), `functional-riscv64-fsck.log` (69), `functional-gui-fsck.log` (90) |

The serial smoke verifies the copied/moved content before and after a malformed
append, and both transcripts retain exactly two unchanged content lines. The
GUI screenshots show PID 14 running and PID 15 exited with status -15; they
also confirm that a file without a trailing newline does not absorb the prompt.
The shell parser suite is now included in xtask's host gate.

One GUI startup attempt exceeded a 60-second budget without a panic or DMA
fault. Its logs are retained as `functional-gui-startup-timeout{,-check}.log`;
the retry completed with a 120-second budget. This does not establish a maximum
boot time. All QEMU runs used separate `functional-*.img` disks.

Not run for this pass: full powercut/fault-injection/GUI/SSH/Mica matrices,
x86_64 QEMU, RISC-V GUI, real hardware, and performance benchmarks.

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
ARCH=x86_64 make build
ARCH=x86_64 make run      # serial shell; QEMU Q35 profile
ARCH=x86_64 make test     # validation in progress; not run
```

`make test` is the unified gate. It builds the image, runs host tests and then
the normal, power-cut, fault-injection/recovery, MFS1 crash campaign, GUI, SSH,
and Mica stages. The test harness uses the repository's OrbStack image and QEMU
configuration; a host KVM accelerator is not required.

## Target matrix and current validation

| Target | Current status | Evidence boundary |
| --- | --- | --- |
| AArch64 + QEMU `virt-7.2` | Build and serial run reach shutdown | `make test` built-in host suites `81/81`; separate shell parser `6/6`; `87/87` combined recorded checks. |
| RV64GC + QEMU `virt` | Serial run reaches shutdown | OpenSBI M-mode → S-mode, Sv39, two harts, PLIC/SBI, PCI VirtIO and RISC-V IOMMU gates are verified below. |
| x86_64 + QEMU Q35 | Focused QEMU acceptance passed | `x86_64-unknown-none`, GRUB Multiboot2 ISO, two vCPUs, local APIC/IOAPIC, Intel VT-d, VirtIO-blk INTx, block/MFS I/O, resident services, and shell shutdown. The full `make ARCH=x86_64 test` matrix is not run. |

The Makefile and `xtask` select the target and QEMU profile from `ARCH`. For
RISC-V, that profile includes the default OpenSBI firmware and `iommu-sys=on`;
the validated RISC-V chain is RV64GC → QEMU `virt` → OpenSBI M-mode → S-mode →
Sv39 → two harts → PLIC/SBI → PCI VirtIO → RISC-V IOMMU. x86_64 selects the Q35,
local APIC/IOAPIC, Intel VT-d, and VirtIO profile described below.

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

### x86_64 Q35/Multiboot2/VT-d acceptance

The x86_64 target is `x86_64-unknown-none`. `xtask` packages the kernel as a
GRUB Multiboot2 ISO and boots it on QEMU Q35 with two vCPUs, local APIC/IOAPIC
interrupt routing, Intel VT-d, and VirtIO devices. The focused acceptance used
the serial path:

```sh
ARCH=x86_64 make build
ARCH=x86_64 make run
```

`ARCH=x86_64 make build` passed; the recorded acceptance used the equivalent
QEMU profile with a temporary raw disk so the existing build disk was not
locked.

The recorded run passed the SMP/RR/IPC gates and resident EL0 readiness, then
reported the blocked VT-d fault probe, firmware-routed PCI INTx line 11, and
two VirtIO-blk INTx completions:

```text
[iommu] fault-probe blocked=true sentinel=true event=0x1 stream-id=0x10 iova=0x798000 completion-error=false
[irq] virtio-blk INTx pin=1 ioapic-id=11 bound cpu0
[irq] virtio-blk INTx completions=2
[user] block driver queue0 read+write sector=0 mfs1=true flush=ok
[service] resident EL0 ready=8/8 online=8/8 switches=827090
```

The same run mounted and recovered MFS1, reached `micro> help`, accepted
`micro> shutdown`, and exited through the guest success shutdown code. The
full `make ARCH=x86_64 test` matrix has not been run and is not a PASS result.

## Final gate

| Evidence | Result |
| --- | --- |
| AArch64 build | passed |
| AArch64 serial run | reached `[system] shutdown` |
| RV64GC QEMU `virt` serial run | reached `[system] shutdown` |
| RISC-V IOMMU fault probe | `blocked=true sentinel=true event=0xf stream-id=0x10 completion-error=false` |
| x86_64 `make build` | passed |
| x86_64 focused QEMU acceptance | passed; resident, VT-d, INTx, block/MFS, shell `help`/`shutdown`; guest exit `33` |
| `make test` built-in host suites | `81/81` (`22+9+6+9+11+24`) |
| Separate shell-parser run | `6/6` |
| Combined recorded host checks | `87/87` |
| `make ARCH=aarch64 test` | `validation in progress` |
| `make ARCH=riscv64 test` | `validation in progress` |
| `make ARCH=x86_64 test` | `validation in progress` (not run) |

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
That recorded fixture predates automatic redirects and the Save control; it
does not validate redirected navigation or Reader file writes.

The serial shell help surface is:

```text
shell: help pwd cd echo clear history ps kill wait uptime sleep date free sysinfo
files: ls cat head tail wc hexdump xxd grep find tree du df stat touch cp write append
       mkdir rmdir mv rm fsync sync (also available through fs <command>)
database: sql <CREATE|DROP|INSERT|SELECT|UPDATE|DELETE statement>
network: curl nslookup netstat net [status|config|dhcp|static|default|reset]
programs: run mica
system: exit shutdown poweroff reboot
```

Filesystem aliases include `ls/list`, `cat/read`, `touch/create`, `mv/rename`,
and `rm/remove/unlink`; bounded options are `head/tail -n N`, `mkdir -p`,
`cp -r`, and `rm -r`. The GUI Terminal accepts `ls/list [path]`, `cat/read`,
`stat`, `touch/create`, `cp`, `write`, `append`, `mkdir`, `rmdir`, `mv/rename`,
`rm/remove/unlink`, `fsync` and `sync`. Both shells maintain `pwd`/`cd` working
directories and resolve relative paths. File arguments support whole single-
or double-quoted arguments without escapes, expansion or concatenation.
For example:

```text
mkdir -p "/data/work notes"
append "/data/daily note" "hello"
fs append "/data/daily note" " world"
mv "/data/daily note" "/data/work notes"
fsync "/data/work notes/daily note"
cat "/data/work notes/daily note"
```

`append` creates an absent file and reads the EOF inside the serialized MFS
request. `mv` moves into an existing directory while refusing to overwrite a
target. The existing serial smoke gate includes quoted copy/move/append/read
and malformed-argument rejection. Terminal uses the
dedicated `TERMINAL_FILESYSTEM_ENDPOINT` plus the `GUI_TERMINAL_COMMANDS`
4 KiB shared frame; the MFS broker keeps this path separate from block DMA and
other filesystem payload frames. Input commands are capped at 512 bytes and
output/replies at 4 KiB, with the viewport following the newest lines. The
serial shell and GUI terminal share the filesystem parser and aliases.

The serial shell's `sql <statement>` command sends one UTF-8 statement to the
resident EL0 `db` service. For example:

```text
sql CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT NOT NULL)
sql INSERT INTO users VALUES (1, 'Alice')
sql SELECT * FROM users WHERE id = 1
```

The request and response use the 4 KiB IPC shared frame. Mutating statements
commit one complete candidate snapshot through MFS1 at
`/.system/db/main.db`; the staging path is `/.system/db/main.db.tmp`. The
service is a bounded MicroSystem SQL subset, not SQLite-compatible.

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
outside this command boundary. GUI Terminal also supports application `ps`,
`kill`/`wait`, system/time/history commands, `netstat` and power control.
DNS, curl, SQL and Mica execution use the serial shell.

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

The normal serial profile reports bootfs `25/24` (25 entries, 24 static
ELFs), `SERVICE_COUNT=13`, resident ASIDs `0x20..0x2c`, serial ready/online
`8/8`, and the first dynamic process PID `14`. The GUI profile reports
GUI ready/online `13/13`. The kernel heap budget is `0x8000000` (128 MiB);
ordinary user tasks have a 64 KiB stack and 1 MiB heap, and a Mica image is
loaded at `0xe0000`.

## Stable serial markers

These marker fields are useful when diagnosing a boot or profile mismatch
(the service fields may be emitted on one line):

```text
[bootfs] valid=true entries=25 static-elfs=24
[service] ... address-spaces=13 asids=[0x20..0x2c] ... ready=8/8 online=8/8
[service] ... address-spaces=13 asids=[0x20..0x2c] ... ready=13/13 online=13/13
[proc] dynamic application capacity=16 first-pid=14
[ipc] resident shell->db ping=true
[mm] kernel heap ready bytes=0x4000000
[net] ... raw-device-isolated=true tcp-owner=true dns=true
[ssh] sshd ready address=10.0.2.15 port=22 auth=publickey accounts=persistent
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

The service table reserves 13 resident slots in this order:
`init console block mfs shell devmgr windowd terminal files monitor sshd netd db`.
The serial manifest starts `devmgr,console,block,mfs,db,shell,netd,sshd`; the
GUI profile adds `windowd,terminal,files,monitor`. `SERVICE_COUNT` and the
address-space/ASID range come from the kernel service table, not from a
hand-maintained marker. The serial readiness mask still reports `8/8`, while
the GUI mask covers all 13 and reports `13/13`.

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

- No `[bootfs]` or an unexpected 25/24 count: inspect image construction before
  debugging services.
- Serial `8/8` or GUI `13/13` not reached: inspect the profile/device path and
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

## Accounts and network maturity gates

Run `make test-identity` for user/role/private-file/key revocation, SSH host-key
rotation, service recovery and cold-boot persistence; `make test-network` for
DHCP, two routes, IPv6 TCP/UDP/DNS, saved configuration, link failover and three
PCIe insertion/removal cycles. These gates currently use the AArch64 QMP profile.
They overwrite only `target/service-recovery.img`, an isolated source-disk copy.
Run them sequentially because they share their serial/QMP artifacts.

Accounts are fail-closed when `/.system/accounts` is corrupt. The physical serial
console retains root file access and can restore a saved valid file; restart
then loads the repaired account data. Do not replace the account file with empty
bytes or a different schema. Private host-key rotation intentionally changes the
SSH fingerprint; inspect it on the trusted console/client before accepting it.
See [identity.md](identity.md) for roles, bounded audit retention and limits.
