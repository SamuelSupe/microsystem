# Documentation package result

Review date: 2026-08-14. This was a documentation-only pass after the final
OrbStack acceptance run. No tests or production sources were changed or rerun
by this packet. The package owns the selected `README.md`/`docs/*.md` files and
this result file; ABI and SSH documents were not changed by this pass.

## Sources and files

Facts were checked against the current ABI, kernel, init, windowd, Mica,
netd, sshd and MFS sources plus the final OrbStack evidence in
`target/unified-mfs1-resilience-final.log`,
`target/unified-mfs1-resilience-fsck.log`,
`target/gui-browser-fixture.log`, `target/mica-dns-fixture.log` and
`target/mica-qemu.log`.
The real public-internet image-bound capture
[`target/browser-internet-reader-imagebound-final.png`](../../../target/browser-internet-reader-imagebound-final.png)
shows `https://example.com/` with `HTTP 200 — https://example.com` and
`# Example Domain`.

| File | Contract covered |
| --- | --- |
| `README.md` | final build/test/fsck status, topology, entry points and Mica GUI summary |
| `docs/mica.md` | language/VM limits, stdlib, permissions, GUI module/widgets, FS/network brokers, HTTPS/TLS and SSH |
| `docs/abi.md` | protocol/syscall numbers, V1/V2 launch ABI, GUI caps, command/event wire contract and topology |
| `docs/architecture.md` | EL1/EL0 ownership, FP/Advanced SIMD context boundary, V2 launch, dynamic GUI isolation and service profiles |
| `docs/network.md` | netd raw-device boundary, DNS/TCP/UDP limits and TLS handoff |
| `docs/ssh.md` | public-key SSH, Mica policy intersection, `--gui` command and acceptance |
| `docs/gui.md` | dynamic GUI capability boundary, widgets, Present/event semantics and QEMU evidence |
| `docs/mfs1.md` | MFS1 v1 format, dual-metadata audit/repair, crash campaign, final fsck tuple and recovery boundary |
| `docs/runbook.md` | OrbStack commands, final gate, stable markers and remaining boundaries |

## Final acceptance evidence

The final authorized `make build`, `make test` and `make fsck` run exited zero;
the full test and fsck logs are `target/unified-mfs1-resilience-final.log` and
`target/unified-mfs1-resilience-fsck.log`. Host suites were MFS1 22/22,
capability ABI 9/9, GUI command-stream 6/6, Desktop 9/9, kernel 11/11 and
Mica 24/24 (81/81 total). Normal smoke, powercut, all transaction/GC fault
cuts plus the MFS1 resilience campaign, GUI, SSH and Mica/TLS QEMU stages all
passed. The smoke marker was:

```text
[sched] fp-simd context-isolation=true tasks=2 context-switches=20 checks=[603697,686002] mismatches=[0,0] q-regs=q0-q31 fpcr-fpsr=true signatures=[0x11,0x22]
```

The final fsck reported:

```text
MFS1 clean generation=2544 transactions=2381 entries=24 used_blocks=11898
```

Current topology and memory facts are:

```text
bootfs entries/static-ELFs = 24/23
SERVICE_COUNT              = 12
serial ready/online        = 8/8
GUI ready/online           = 12/12
resident ASIDs             = 0x20..0x2b
first dynamic PID          = 13
dynamic GUI clients        = 8
desktop window capacity    = 11
kernel heap                = 128 MiB (0x8000000 bytes)
ordinary stack/heap        = 64 KiB / 1 MiB
Mica image window bytes    = 0xe0000
```

Only `netd` owns the raw network-device capability. Mica, SSH and GUI receive
bounded endpoint IPC and transient frames, never raw `NetReceive`/`NetSend`,
MMIO, BAR, DMA or IRQ capabilities.

## AArch64 FP/Advanced SIMD context

The boot assembly enables FP/Advanced SIMD through `CPACR_EL1.FPEN` on both boot
CPUs. Each EL0 entry helper clears q0-q31 and FPCR/FPSR before the first user
instruction. Ordinary non-switch exceptions and SVCs keep the 256-byte GPR-only
`ExceptionFrame`; the generic exception path does not move vector state. A real
`preempt::save`/`load` scheduler switch carries a separate 16-byte-aligned
528-byte context containing q0-q31 plus FPCR/FPSR. Rust and EL0 services remain
`aarch64-unknown-none-softfloat`, so this is not a hard-float ABI. Only AArch64
FP/Advanced SIMD is supported; SVE is outside the contract.

The two RR workers use `0x11` and `0x22` signatures plus the register index when
filling q0-q31, then check both lanes of every register and FPCR/FPSR after each
resume. The final marker above records 20 switches, non-zero checks for both
workers and zero mismatches.

## Mica GUI contract

`ThreadLaunchV1` remains compatible for ordinary Mica sessions. GUI sessions
use `ThreadLaunchV2` with a 64 KiB command FrameRegion, 4 KiB event Frame and a
dedicated endpoint. `script::EVENT_GUI` signals the event ring. Init installs
these entries atomically and windowd binds each endpoint to the registered
PID/session token. The effective `gui.window` permission is the intersection of
the script manifest, launcher `--allow` rules and entry-point policy.

GUI is file-only (`mica --gui --timeout 86400s /path/script.mica`); GUI `-e` and GUI REPL are
rejected. `require("gui")` supplies the retained single-window widgets
`label`, `button`, `text_input`, `checkbox`, `list`, `scroll`, `row`, `column`,
`spacer` and `canvas`, with the documented callbacks and bounded flex layout.
Windowd validates all six drawing commands, UTF-8, bounds, clip, hash, command
and damage limits before atomically replacing a Present list. Validated damage
rectangles bound redraws, merged at up to 60 Hz. It loads Unifont UFB with an
ASCII/replacement fallback. The desktop has three built-in windows and eight
dynamic Mica slots (11 total). Its fixed init-owned launcher registry has five
icons: Terminal, Files, Monitor, Reader and Editor. Built-ins focus/restore;
Reader and Editor send only an application id via endpoint capability slot 84,
which init maps to a fixed script path, manifest and entry policy. Active and
pending requests are deduplicated, and dynamic registration binds the id in
word 3. The display-list `Icon` command remains drawing only. Effective
permissions are manifest ∩ launcher `--allow` ∩ entry policy.
The shell/SSHD timeout parser accepts `1ms` through `24h`; non-GUI sessions are
hard-capped at 60 seconds, while GUI file sessions may use 24 hours. The final
Editor invocation used `--timeout 86400s`.

The unified Mica stage explicitly set `MICROSYSTEM_MICA_TLS_FIXTURE=1`; trusted
body `mica-https-ok`, a presented root-chain, P-384 `CertificateVerify` and a
long CA DNS name were accepted, while unknown-CA, expired, not-yet-valid and
hostname-mismatch certificates were rejected. The final build marker was
`CA bundle: pinned=121 host-extra=43 bytes=173925`, with guest MCAB SHA-256
`70572323c1e9e151d300b76888bfb43906afb6473c573208a565e943b5426d50`. The
bundle keeps pinned Mozilla 121 roots, deduplicates additions from the build
container's `SSL_CERT_FILE`, caps each certificate at 16 KiB and the complete
bundle at 256 KiB; `xtask` injects the full hash before compiling Mica and the
runtime verifies the complete bundle before use. If a presented chain repeats
the trust anchor as its tail, validation omits that redundant tail first.

The clean dynamic Counter gate produced these lifecycle markers:

```text
[gui] mica registration requested endpoint=0
[gui] mica registration received endpoint=0
[gui] mica client registered command=65536 event=4096 endpoint-isolated=true
[mica] gui presented widgets=true atomic=true isolated=true
[mica] vm returned=true
[mica] exiting=true
[gui] mica client unregistered endpoint=0 resources-reclaimed=true
```

QMP button, ASCII and Unicode text, resize/maximize/restore/minimize/taskbar
and close actions completed with SSH status 0. Unicode accumulators
`4/78/1250/20013` were delivered/rendered. Decoded 1024×768 screenshot CRCs
were:

```text
desktop  b1998cf3
mica-pre 7d6a4f44
button   e8128290
ascii    9f1f4603
unicode  60f7c6f6
resize   f19b1e8d
restore  957bdb75
max      524aacc7
min      083b1435
taskbar  c9110363
second-pre    cca2e7d6
second-button 40858a7f
second-text   a445076b
after-close-reuse 35366f14
stable-client 9014e4dd
```

Geometry checks reported max `962`, resize/restore `418`, minimize `0` and
taskbar `121`; the active title color was `#1e3b63`. Endpoint 1
registered/presented/reclaimed twice and was
reused; endpoint 0 was reclaimed. The second and third sessions were launched
through persistent serial while the first remained on SSH (sshd serves one
session), proving GUI-session concurrency rather than SSH concurrency. No panic
or DMA fault appeared and cleanup left no QEMU/container residue. Final
acceptance logs are `target/unified-mfs1-resilience-final.log` and
`target/unified-mfs1-resilience-fsck.log` (GUI/QMP details are in
`target/gui-qemu.qmp.jsonl`).

## P0/P1 durability and bounded-I/O review

MFS `fsync` returns `Err` without dropping or reordering dirty mutations, so a
retry replays the original order. `WriteAtomic` stages a temporary file, then
commits one `replace_file` transaction (`Remove(target)` followed by
`Rename(temp,target)`) and performs a durable final fsync. The four fault points
remounted only as `target=old,temp=new` or `target=new,temp=absent`; retry ended
at the latter. Directory descendant renames are rejected without dirty mutation.

The GUI partial-Present path is bounded in user-rt, kernel, GPU and windowd;
pointer damage is carried through, input is batched at 16, and the
marker is `partial-present=true rect=1024x720+0,48`. Renderer marker is
`damage-merge=true local-window-damage=true command-cull=true glyph-cache=2way
input-batch=16`; VirtIO-net reports
`rx-buffers=2`. The authoritative final logs are
`target/unified-mfs1-resilience-final.log` and
`target/unified-mfs1-resilience-fsck.log`.

MFS1 remains format version 1 with record CRC32C and two independently
CRC32C-checked superblock copies. The resilience campaign ran 10,024
seeded-randomized, deterministic cases with seed `0x4d46533143524153` using sparse volatile and
durable block maps; a crash drops volatile writes and each case is replayable
by seed, case and event. Triggered classes were write I/O error `1668`, short
write `1711`, 512-byte torn-sector `1626`, and flush I/O error `5019`.

`fsck` checks both superblocks' checksum, head and complete replay and exits
non-zero for degraded `1/2` metadata. The offline `mfsctl repair IMAGE` path
only rebuilds one checksum-invalid stale mirror when the other copy fully
replays and a whole-image scan finds no later `Commit` or ambiguous record.
Latest corruption, double corruption, read I/O and record ambiguity are
refused without writing. The positive CLI case preserves payload; active-latest
and double-corruption cases retain their payload SHA. This is bounded metadata
repair, not data-block correction or general data recovery.

## Materialized-path performance review

MFS single-path `view_before` keeps read, metadata, put, mkdir and rename from
cloning the complete `BTreeMap` or file bodies. `list` materializes only
`BTreeSet` paths, and `stat` reads `Metadata` without copying file data.
Windowd merges overlapping damage and dirty-region culling skips
non-intersecting windows, display lists, icons and taskbar work. Local
`DesktopVisual` windows redraw for drag/resize/focus/Terminal updates without a
default full redraw; command/text/glyph work is culled. Files and Monitor use blocking event IPC
(`wait=event-blocked`) instead of yield-spin. These bounded changes preserve
the existing ABI, security and resource limits. OrbStack QEMU TCG is used as
functional/regression and work-reduction evidence, not as a native-hardware
performance benchmark or a percentage claim.

## Mica Reader example

The MFS1 example `/.system/examples/mica/browser.mica` is a bounded, single-
window document reader. It uses `http.get` and the retained GUI widgets to
provide an address field, back/forward/reload, same-origin links, and paged
text. Its fixed entry policy is `gui.window` plus unscoped `net.browse`; the
script accepts a 98,304-byte body, renders at most 96 lines of 64 characters, and
shows 18 lines per page. `net.browse` is used only by the Reader's `http.get`
HTTP/HTTPS GET path for a dynamic address-bar host. Raw resolve/TCP/UDP and
POST/PUT/PATCH/DELETE remain exact `net.connect:host:port` operations. Page
links are same-origin, while an explicit address-bar URL can cross origin;
URL fragments are removed before the request. The supported HTML subset is
`head`/`title`, `p`,
`div`, `h1`-`h3`, `li`, `pre`, `a`, `br`, comments and entities. Script/style
bodies are skipped. JavaScript, CSS, images, forms, cookies, downloads,
HTTP/2, WebSocket, persistence and automatic redirects remain outside the
contract. A 3xx `Location` is rendered as an explicit link instead.

HTTP response values now include bounded `content_type` and `location` fields;
arbitrary headers are not exposed. The example uses the new string helpers
(`find`, `trim`, `starts_with`, `ends_with`, `replace`, `collapse_space`) and
`widget:set_callback` for interaction. The targeted OrbStack Mica host suite
passed `24/24` after browser source, UTF-8 string, chained `elseif`,
short-circuit boolean, closure callback-state, and `set_callback` regressions
were added. The GUI fixture served exactly one `GET /index.html` and one
`GET /next.html`; no `evil` request was accepted. The Reader loaded both
responses with HTTP 200. The network fixture observed
`GET /index.txt?browse=1` after stripping `#fragment` and DNS for `mica.test`.
The combo marker records `browse=true`, raw resolve/TCP/UDP and POST denials,
trusted body `mica-https-ok`, presented root-chain, P-384 `CertificateVerify`,
long CA DNS name, and unknown-CA/expired/not-yet-valid/hostname-mismatch
rejection. Endpoint 1 registered, presented, reclaimed, and exited
with status 0. Browser screenshot CRCs were `2286d988` (pre-navigation),
`5dab179e` (same-origin link), and `f60453d9` (cross-origin rejection). Logs are
`target/unified-browser-public-internet-final.log`,
`target/unified-browser-public-internet-fsck.log`, `target/gui-browser-fixture.log`,
`target/mica-dns-fixture.log` and `target/mica-qemu.log`.

## Mica Editor example

The unified Terminal filesystem gate also passed the absolute-path command
sequence `mkdir`, `write`, `stat`, `mv`, `cat`, `ls`, `rm`, `rm`, `sync`, with
every marker `status=ok`. The dedicated `TERMINAL_FILESYSTEM_ENDPOINT` uses a
terminal-only `GUI_TERMINAL_COMMANDS` 4 KiB shared frame; commands are capped at
512 bytes and output/replies at 4 KiB, while the viewport follows newest output.
The serial shell parser's `stat`/`mv`/`rm` targeted tests passed 2/2. The help
screen decoded to 1024x768 with 140 foreground rows and CRC `b3c61c67`.

The MFS1 `/.system/examples/mica/editor.mica` is a single-window bounded text
editor. It requires `gui.window`, `fs.read:/data` and `fs.write:/data`; the
effective permission remains the manifest/launcher/entry-policy intersection.
Paths are normalized below `/data/`, input is UTF-8 checked and CRLF/CR is
normalized to LF, and the document is limited to 32 lines and 32,512 bytes.
Open/Reload use `fs.read_file`; Save uses `fs.write_file` with
`atomic=true, fsync=true`. The example has no network, process, clipboard,
rich-text or multi-window surface.

The final Editor GUI gate passed host Mica `24/24`, pointer sequence
`4, 5, 6, 10, 9`, ReadRange/WriteAtomic completion, two load markers, the
saved marker `bytes=44 atomic=true fsync=true`, CloseRequested, endpoint-1
reclaim and `mica: pid=14 status=0`. Current QMP decoded full-frame CRCs were
`3eeb31ac` (pre), `da105c73` (Apply), `18f38838` (Save), and `7e895e1d`
(Reload); the stable first-row crop was `0a8ab59f` for both Apply and Reload.
The browser fixture remained exactly `GET /index.html` and `GET /next.html`;
cleanup left no QEMU, fixture, endpoint or helper-process residue.

The dedicated desktop Editor-icon gate also passed. It observed application id
5 click/accept, endpoint registration and Present; duplicate click and
minimize/restore did not create another session. Close reclaimed the endpoint,
and a second click registered/presented/reclaimed a fresh session. Icon CRCs
were `5cb170eb`, `74eebaaa`, `05025d7a` and `f686e259` (pre, active, restored,
restart). Unified icon JSON reported launches=2, registrations=2, Presents=2
and reclaims=2. Filesystem read plus Present proved the load; detached-session stdout
was intentionally not used as a marker. Cleanup left no QEMU, fixture or
helper-process residue.

## Mica, SSH, TLS and recovery boundaries

The Mica documentation records the current syntax, closures/tables, mark-sweep
GC, `pcall`, modules, source/bytecode/heap/stack/instruction/time limits and
the permission intersection for file and SSH sessions. It lists the filesystem,
process, time, system, random, network, HTTP, encoding/hash, JSON,
string/bytes/table/math, IO, args and GUI modules.

The low-level `net.tls_connect` opcode remains `NotSupported`; trusted Mica
HTTPS uses embedded TLS 1.3, PL031 realtime, hostname/time/chain checks,
RSA-PSS and P-384 ECDSA `CertificateVerify` and the fixed Mozilla MCAB bundle.
When the presented chain tail equals the trust anchor, validation omits that
redundant tail before revalidation. Unknown-CA, expired, not-yet-valid and
hostname-mismatch fixtures were rejected. SSH keeps
the generated Ed25519 key, policy file and bounded command surface; `run mica`
remains rejected.

MFS1 fault/GC documentation retains only the recovery facts useful for
interpreting the final gate: transaction cuts preserve the last committed
state and the GC candidate-superblock-flushed cut recovers cleanly. The new
campaign and repair boundary are explicit, but neither is turned into a claim
of physical power-loss coverage, data-block error correction or unbounded
extent coverage.

## Static documentation checks

The owned files passed the trailing-whitespace scan, stale-topology scan,
`git diff --check`, and untracked-file diff check. No test command is part of
this result; the acceptance status above is copied from the final test result
packet rather than rerun during documentation review.
