# MicroSystem

MicroSystem is an AArch64 `virt-7.2` research microkernel. EL1 keeps the
minimal boot, MMU, interrupt, scheduling, capability and device-isolation
mechanisms; filesystems, network, SSH, GUI and Mica run as static AArch64 EL0
services from the generated bootfs. It is not Linux/POSIX and does not promise
real-hardware or arbitrary ELF compatibility.

## Current status

The final OrbStack acceptance run passed `make build`, `make test`
and `make fsck` (exit 0). `make test` runs host tests, normal smoke, powercut,
fault/GC recovery, the MFS1 resilience campaign, GUI, SSH and Mica QEMU gates.
The host counts were:

```text
MFS recovery 22/22
ABI capability_abi 9/9
GUI command-stream 6/6 + Desktop 9/9
kernel core_boundaries 11/11
Mica tests 24/24
Total host tests 81/81
```

The final offline check was:

```text
MFS1 clean generation=2544 transactions=2381 entries=24 used_blocks=11898
```

The authoritative unified MFS1 resilience acceptance run is recorded in
`target/unified-mfs1-resilience-final.log` and
`target/unified-mfs1-resilience-fsck.log`. Its smoke marker is:

```text
[sched] fp-simd context-isolation=true tasks=2 context-switches=20 checks=[603697,686002] mismatches=[0,0] q-regs=q0-q31 fpcr-fpsr=true signatures=[0x11,0x22]
```

Powercut, all transaction/GC fault cuts, GUI, SSH and Mica/TLS stages passed.

MFS1's seeded randomized crash campaign executed 10,024 deterministic,
reproducible cases with seed `0x4d46533143524153`. It uses sparse
volatile/durable blocks: a crash discards
volatile writes, and every case records its seed, case and event so a failure
can be replayed. The triggered classes were write I/O error `1668`, short
write `1711`, 512-byte torn-sector `1626`, and flush I/O error `5019`.
Superblock metadata has two independently CRC32C-checked copies; fsck verifies
both copies and their complete replay, and exits non-zero when only one copy is
healthy. `mfsctl repair IMAGE` is deliberately offline and narrow: it can heal
one checksum-invalid stale mirror only when the other copy fully replays and a
whole-device scan finds no later commit or ambiguous record. Latest corruption,
double corruption, read I/O failure and record ambiguity are rejected without a
write. The repair gate preserved payload bytes; active-latest and double-
corruption cases kept their payload SHA unchanged. This is metadata mirror
repair, not data-block error correction or general data recovery.

The kernel enables AArch64 FP/Advanced SIMD on both boot CPUs. Each EL0 entry
helper clears q0-q31 plus FPCR/FPSR; the ordinary non-switch SVC path keeps its
256-byte GPR exception frame. Only the real scheduler `preempt::save`/`load`
switch path carries the separate 528-byte, 16-byte-aligned q0-q31 + FPCR/FPSR
context. Rust and EL0 services still target `aarch64-unknown-none-softfloat`;
this is not a hard-float ABI and SVE is not supported.

The materialized-paths performance review keeps MFS single-path operations on a
`view_before` snapshot: read, metadata, put, mkdir and rename avoid whole
`BTreeMap` and file-data clones; list materializes only `BTreeSet` paths, and
stat reads `Metadata` without copying file bodies. Windowd dirty-region culling
skips non-intersecting windows, display lists, icons and taskbar work. Files and
Monitor use blocking event IPC (`wait=event-blocked`) instead of yield-spin.
These are bounded functional changes; OrbStack QEMU TCG is regression evidence,
not a native-hardware performance benchmark.

P0/P1 durability and bounded-I/O review facts are part of this acceptance:
failed `fsync` restores the dirty mutation list in its original order;
`WriteAtomic` uses one `replace_file` transaction (`Remove(target)` then
`Rename(temp,target)`) followed by a durable final fsync. Each fault point
remounted as either `target=old,temp=new` or `target=new,temp=absent`, and retry
ended at the latter. Directory-renaming into its own descendant is rejected
without dirty mutation. GUI partial Present is bounded across user-rt, kernel,
GPU and windowd, carries pointer damage, and reports
`rect=1024x720+0,48`; pending input is batched at 16 and the net marker reports
`rx-buffers=2`. The renderer marker reports
`damage-merge=true local-window-damage=true command-cull=true glyph-cache=2way
input-batch=16`.

The image contains 24 bootfs entries (23 static ELF files plus `etc/services`).
The serial profile reports eight resident services ready/online (`8/8`); when
VirtIO-GPU/input are present the GUI profile enables all twelve
`SERVICE_COUNT=12` services and reports `12/12`. Resident ASIDs are
`0x20..0x2b`; dynamic application slots start at PID 13 and have capacity 8.
The kernel heap is 128 MiB. An ordinary EL0 task has a 64 KiB stack and 1 MiB
user heap; its Mica/task image window is `0xe0000` bytes. GUI has eight dynamic
Mica client slots in addition to the three built-in windows (desktop capacity
11).

See [the Mica/runtime contract](docs/mica.md), [ABI reference](docs/abi.md),
[architecture](docs/architecture.md), [network/netd](docs/network.md),
[SSH](docs/ssh.md), [GUI](docs/gui.md), [MFS1](docs/mfs1.md) and
[runbook](docs/runbook.md). Raw acceptance evidence is retained in
[`.workflow/microsystem-kernel/results/tests.md`](.workflow/microsystem-kernel/results/tests.md);
the documentation-only review result is
[`.workflow/microsystem-kernel/results/docs.md`](.workflow/microsystem-kernel/results/docs.md).

## Quick start

Use the OrbStack Docker context and the pinned image/toolchain:

```sh
docker context show                         # orbstack
make build
make test
make fsck
```

For an interactive serial QEMU shell:

```sh
make run
```

For the GUI/QMP/VNC gate:

```sh
make gui
```

For SSH, with the generated test key:

```sh
make ssh
# equivalent direct command:
ssh -F /dev/null -T -p "${SSH_PORT:-2222}" \
  -i build/ssh/id_ed25519 -o IdentitiesOnly=yes \
  -o StrictHostKeyChecking=accept-new micro@127.0.0.1
```

Mica examples on the serial shell (`micro>`):

```text
mica -e 'print(40 + 2)'
mica
mica> print(6 * 7)
mica> exit
mica --allow fs.read:/data /data/example.mica -- arg1 arg2
mica --gui --timeout 86400s --allow gui.window /mica/gui-counter.mica
mica --gui --timeout 86400s --allow gui.window --allow net.browse \
  /.system/examples/mica/browser.mica -- https://example.com/
mica --gui --timeout 86400s --allow gui.window --allow fs.read:/data --allow fs.write:/data \
  /.system/examples/mica/editor.mica -- /data/note.txt
```

The `--timeout` parser accepts `1ms` through `24h`. Non-GUI Mica sessions are
hard-capped at 60 seconds; GUI file sessions may use the full 24-hour limit.

The unified Mica stage enables its deterministic HTTPS fixture with
`MICROSYSTEM_MICA_TLS_FIXTURE=1`; it verifies the trusted body
`mica-https-ok`, a presented root-chain, P-384 `CertificateVerify` and a long
CA DNS name, while rejecting unknown-CA, expired, not-yet-valid and hostname
mismatch certificates.

Guest HTTPS keeps the pinned Mozilla-derived 121-root MCAB set and appends
deduplicated certificates from the build container's `SSL_CERT_FILE`. Each
host certificate is capped at 16 KiB and the complete guest bundle at 256 KiB.
`xtask` generates the bundle before compiling the Mica service, injects its
full SHA-256 into that build, and the runtime verifies the complete bundle
before using it.

`run mica` is intentionally rejected; Mica runs through the script broker and
`ThreadStartEx`, not the ordinary static application loader.

GUI Mica is file-only and requires `--!allow gui.window` in the script plus the
launcher/SSH policy. `require("gui")` provides a single-window retained widget
tree (`label`, `button`, `text_input`, `checkbox`, `list`, `scroll`, row/column,
spacer and canvas) over a validated command/event broker; it never exposes the
framebuffer, GPU, input device, DMA, BAR or IRQ capabilities.

The desktop has five fixed init-owned launcher icons: Terminal (id 1), Files
(id 2) and Monitor (id 3) at the lower left, followed by Reader (id 4) at
`x=864` and Editor (id 5) at
`x=940` (each icon is 64×64 at `y=650`). The `Icon` display-list command is
only drawing; it is not an application registry. Built-in icons focus or
restore their existing window. Reader and Editor clicks send only an
application id over the init-owned launcher endpoint (capability slot 84);
init maps that id to the fixed script path, manifest and policy. Active and
pending launches are deduplicated, while dynamic registrations bind the same
id in registration word 3. In-flight launches are deduplicated. The effective
script permissions remain the
manifest ∩ launcher rules ∩ entry policy intersection.

The MFS1 `/.system/examples/mica/browser.mica` is a bounded Mica Reader, not a
general Web browser. It uses the existing HTTP/HTTPS and GUI brokers to display
UTF-8 text or a small HTML subset with same-origin links, back/forward/reload,
and paged output. Its explicit `net.browse` permission lets `http.get` issue
GET requests to a dynamic HTTP/HTTPS host selected in the address bar; it does
not grant raw DNS/TCP/UDP access or POST/PUT/PATCH/DELETE. Exact
`net.connect:host:port` rules remain available for scripts that need those
lower-level or non-GET operations. Page links stay same-origin, while a user
may explicitly enter another origin in the address bar. Host validation,
TLS verification, the 98,304-byte response-body cap and 96-line document cap
still apply. JavaScript, CSS, images, cookies, downloads, HTTP/2, WebSocket and
automatic redirect following are intentionally absent.

The final GUI evidence is retained in
`target/unified-browser-public-internet-final.log`,
`target/unified-browser-public-internet-fsck.log`, `target/gui-qemu.log` and
`target/gui-qemu.qmp.jsonl`; the expanded gate covers the refreshed navy visual
style, Unicode, window actions, taskbar, endpoint reclaim/reuse and concurrent
GUI sessions. The active title color is `#1e3b63`; the decoded QMP geometry
checks report max `962`, resize/restore `418`, minimize `0`, taskbar `121`, and
the stable client crop is `9014e4dd`; partial Present reports
`rect=1024x720+0,48` (737,280 pixels, strictly below the 1024×768 full-screen
area).

The dedicated desktop-icon gate also exited 0. It clicked Editor (`application=5`),
accepted and registered the init launch, presented once, suppressed duplicate
clicks, restored the minimized window without another session, then closed and
reclaimed the endpoint. A second click reused the slot and reclaimed it again;
the decoded icon CRCs were `d2a43609` (pre), `f31681cc` (active), `ed8a4c52`
(restored) and `f31681cc` (restart). The preserved serial/QEMU evidence is in
`target/unified-browser-public-internet-final.log`; detached-session stdout was intentionally
not used as a load marker, with the filesystem read plus Present serving as the
load proof.

The browser/public-internet gate is recorded in
`target/unified-browser-public-internet-final.log`,
`target/unified-browser-public-internet-fsck.log`, `target/gui-browser-fixture.log`
and `target/mica-dns-fixture.log`. The GUI fixture served exactly one
`GET /index.html` and one `GET /next.html`; no cross-origin `evil` request was
accepted. The Mica network fixture observed DNS for `mica.test` and
`GET /index.txt?browse=1` after the URL fragment was stripped. Its combo marker
records `browse=true`, raw resolve/TCP/UDP and POST denials, and the trusted
HTTPS body plus unknown-CA/expired/not-yet-valid/hostname-mismatch rejection.
The Reader loaded both GUI responses with HTTP 200, and its endpoint registered,
presented, reclaimed, and exited with status 0. A real public-internet capture
also shows `https://example.com/` returning `HTTP 200 — https://example.com`
with `# Example Domain` in
[`target/browser-internet-reader-imagebound-final.png`](target/browser-internet-reader-imagebound-final.png).
Screenshot CRCs were
`2286d988` (pre-navigation), `5dab179e` (same-origin link), and `f60453d9`
(cross-origin rejection).

The GUI Terminal accepts `ls [path]`, `cat <path>`, `stat <path>`,
`write <path> <text>`, `mkdir <path>`, `mv <source> <destination>`,
`rm <path>` and `sync`. These commands take absolute paths and have no working
directory. Terminal uses the dedicated `TERMINAL_FILESYSTEM_ENDPOINT` together
with the `GUI_TERMINAL_COMMANDS` 4 KiB shared frame, keeping its MFS broker
payload isolated from other filesystem/device frames. Command input is capped at
512 bytes and command/output text at 4 KiB; the viewport follows the newest
output. The serial shell parser accepts the same `stat`/`mv`/`rm` forms (targeted
parser 2/2). The unified marker sequence is `mkdir`, `write`, `stat`, `mv`,
`cat`, `ls`, `rm`, `rm`, `sync`, all with `status=ok`; the help screenshot has
140 foreground rows, proving the shared-frame path rather than the old inline
reply limit.

The shipped Mica GUI contract includes synchronous `window:present()`, which
submits a dirty retained frame before returning. The Reader sets a visible
`Loading` status and presents it before its blocking `http.get`; its bounded
HTML parser caches the source length locally. Windowd's Unifont glyph cache is
bounded at 128 entries and uses 2-way set associativity. Windowd merges
overlapping damage, redraws only local DesktopVisual windows for drag/resize/
focus/Terminal updates, culls clean display-list command/text/glyph work, and
batches at most 16 input events. These are bounded work-reduction changes only;
OrbStack QEMU TCG demonstrates functional regression coverage, not a native-
hardware performance benchmark or a percentage speedup. Existing capability,
security and resource limits are unchanged.

The MFS1 `/.system/examples/mica/editor.mica` is a bounded single-window text
editor. It reads and writes only below `/data`, accepts UTF-8 files up to
32,512 bytes and 32 lines, normalizes CRLF/CR to LF, and uses the retained
`list` and `text_input` widgets for line selection and replacement. Save uses
`fs.write_file(..., {atomic=true, fsync=true})`; Open, Save, Reload and close
are exercised by the GUI gate. It is deliberately not a rich-text editor and
has no arbitrary path access, clipboard, undo history or multi-window support.

## Hardware/profile boundary

The tested target is QEMU `virt-7.2`, AArch64 little-endian, 4 KiB pages,
two CPUs and 256 MiB RAM. Device addresses are read from the DTB. The EL0
`netd` service alone receives the raw `NetworkDevice` capability; Mica, SSH and
other services use endpoint IPC. GUI enables the extra four resident services
only after the GPU/input devices are detected. These are bounded profile gates,
not a claim of arbitrary CPU counts, hotplug, dynamic linking, generic DMA,
network TLS sockets, SVE, hard-float ABI or complete desktop application
semantics. The supported architectural FP state is AArch64 FP/Advanced SIMD
only.

For detailed Mica language, VM limits, capabilities, stdlib, netd/TLS and SSH
commands, start with [docs/mica.md](docs/mica.md).
