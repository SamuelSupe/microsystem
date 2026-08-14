# Architecture

MicroSystem targets the QEMU `virt-7.2` AArch64 machine used by the OrbStack
gates: two CPUs, 4 KiB pages and 256 MiB RAM. The kernel consumes the DTB for
UART, GICv3, PCIe, SMMUv3, VirtIO and PL031 addresses; device policy is not
encoded as a portable Linux device model.

## Boot and address spaces

EL1 validates the DTB, installs the high-half/MMU mappings, initializes the
GICv3 timer (5 ms slice), discovers VirtIO PCI and creates the bounded SMMUv3
domain. It prepares init/root and one idle task per CPU. Init then starts
resident EL0 services by bootfs name; each service is re-parsed as a static
ELF64 `ET_EXEC` image with page-granular W^X checks.

The bootfs has 24 entries (23 static ELF files and `etc/services`).
`SERVICE_COUNT=12` and the service order is:

```text
init console block mfs shell devmgr windowd terminal files monitor sshd netd
```

Without GPU/input, the expected resident set is the six core services plus
`sshd`/`netd` (`8/8`). With GPU/input, the kernel enables windowd/terminal/files/
monitor too (`12/12`). Service address spaces use ASIDs `0x20..0x2b`.
Ordinary application slots are independent, capacity eight, and the first
dynamic PID is 13. The tested Mica process is created through `ThreadStartEx`
and is not a resident service.

Each ordinary task receives a 64 KiB stack, a 1 MiB user-rt heap and a
`0xe0000`-byte image window. MFS receives a 32 MiB large-heap backing; windowd
is allowed to map the large-window VA for GUI `FrameRegion` mappings but does
not receive that heap backing. The kernel's lock-free zeroing bump heap is
128 MiB; the FrameAllocator starts after it and is separately protected. User
mappings and ASIDs are invalidated on exit/reuse.

## EL1/EL0 boundary

The trusted computing base keeps only mechanisms that must touch hardware:

- page-table writes, EL0 address-space setup and W^X validation;
- GIC/INTx routing, IRQ acknowledgement and SMMUv3 domain/fault handling;
- bounded VirtIO activation/probe and capability creation;
- scheduling, timer, IPC, capability derivation/revoke and resource cleanup;
- PL031 realtime reads for syscall 31.

Device policy is delegated to EL0. `devmgr` receives the PCI/MMIO/queue/DMA/IRQ
grant from init, validates the DTB-derived BAR window and negotiates VirtIO;
`block` owns the production split queue and MFS owns its disk protocol. The
EL1 VirtIO path is only the isolation/fault probe; normal sector I/O is EL0.

The raw network device is a separate boundary: only `netd` has
`NetworkDevice` `READ|WRITE` rights, and kernel `NetReceive`/`NetSend` reject
other tasks. Mica, SSH and GUI use endpoint IPC and never receive raw device,
MMIO, block or IRQ capabilities.

## IPC and capabilities

Messages have six words and four caps. The high four flag bits carry the move
mask; move/copy pre-validates all destination slots and rights and uses stable
derivation nodes for revoke. Endpoints are capability-scoped, not globally
addressable. The core protocol numbers are documented in [docs/abi.md](abi.md):
Mica script is protocol 8 and network is protocol 9.

The script launch path is:

```text
PROCESS SpawnScript=6 (init allocates session region)
  -> inspect manifest/launcher policy and mint token
  -> register per-session MFS and netd broker state
  -> ThreadStartEx=30 (V1-compatible Mica profile; V2 adds GUI caps)
  -> Mica VM calls FS/NETWORK/SCRIPT brokers by IPC
```

MFS and netd map the per-process session region only around an authenticated
request. Mica's trusted CA read is an exact `ReadRange` of
`/.system/certs/ca-bundle.derpack`; the bytes do not come from a shared
FrameRegion. Before the Mica service is compiled, `xtask` builds this MCAB
from the pinned Mozilla-derived 121-root snapshot plus deduplicated
`SSL_CERT_FILE` host roots, enforcing 16 KiB per certificate and 256 KiB for
the complete bundle. The build injects the full SHA-256 and runtime verifies
the header, payload and hash before use.

The network permission boundary is split intentionally. Exact
`net.connect:host:port` rules authorize raw resolve/TCP/UDP and non-GET HTTP
methods. The unscoped `net.browse` rule is consumed only by the Reader's
`http.get` path for dynamic HTTP/HTTPS GET destinations; its private broker
marker cannot be set by script-level network calls. Page links remain
same-origin, while an explicit address-bar URL may choose another origin.
TLS 1.3 validation retains the trust anchor when a server repeats it as the
chain tail (the redundant tail is omitted before revalidation) and explicitly
supports P-384 ECDSA `CertificateVerify`.

## Storage and failure isolation

MFS1 runs as an EL0 FileService over the EL0 block service. Block DMA uses its
own shared frame; MFS/Shell payloads and SSH filesystem payloads use distinct
frames, preventing a DMA write from corrupting script/SSH data. Runtime frames,
memory pools and mappings are reclaimed on normal exit, fault and kill. A
mapped final frame cap is reported `Busy` until the mapping is removed.

The materialized MFS view uses borrowed `NodeView` entries for single-path
lookup, read, metadata/stat, put, mkdir and rename validation. `list` derives
only path names for its result instead of cloning file bodies, while chained
rename/recreate/remove paths are resolved against the ordered mutation view and
remain stable after sync/remount.

MFS1 remains format version 1. Each metadata root has two independent
CRC32C-checked superblock copies; mount validates the head and then replays the
complete record history before selecting the newest valid root. Records carry
their own CRC32C. `fsck` audits both copies and exits non-zero for degraded
`1/2` metadata, even when the selected payload is readable.

The host resilience matrix runs 10,024 deterministic sparse volatile/durable
crash cases from seed `0x4d46533143524153`. A crash discards volatile writes;
the recorded seed, case and event reproduce every case. Triggered classes are
write I/O error `1668`, short write `1711`, 512-byte torn-sector `1626` and
flush I/O error `5019`. This is a block-device fault model, not a claim of
physical disk emulation or data-block correction.

The only write-capable checker action is offline `mfsctl repair IMAGE`. It may
rebuild one checksum-invalid stale mirror only when the other copy fully
replays and a whole-image scan finds no later `Commit` or ambiguous record.
Latest corruption, double corruption, read I/O failure and record ambiguity
are refused without a write. The repair gate preserves payload bytes and keeps
the active-latest/double-corruption payload SHA unchanged; it is not general
data recovery. Transaction and GC cut stages remain bounded recovery probes,
not coverage of every physical power-loss or extent interleaving.

The final image `fsck` result is recorded in the runbook.

## Scheduling and isolation probes

The kernel runs a shared two-CPU ready queue with ticket locking, PPI timer
preemption and per-CPU current state. The acceptance smoke observes both CPUs,
round-robin progress, independent ASIDs, cap revoke/TLBI and target-level EL0
fault termination. It does not generalize to arbitrary CPU counts, hotplug,
long-term load fairness or dynamic linking.

### AArch64 FP/Advanced SIMD context

The boot assembly enables AArch64 FP/Advanced SIMD on both boot CPUs by setting
`CPACR_EL1.FPEN`. Every EL0 entry helper first clears q0-q31 and FPCR/FPSR, so
the first user instruction observes a clean architectural state. The ordinary
GPR-only `ExceptionFrame` remains 256 bytes for non-switch exceptions and SVCs;
the generic exception path does not copy vector state. Only a real scheduler
switch through `preempt::save`/`load` carries the separate 16-byte-aligned
528-byte context containing q0-q31 plus FPCR/FPSR. Rust and EL0 service binaries
remain `aarch64-unknown-none-softfloat`; the explicit assembly helpers are the
only architectural FP/ASIMD use. This is not a hard-float ABI, and SVE is not
supported.

The RR probe uses two EL0 workers with signatures `0x11` and `0x22`. Each worker
adds its register index to the signature when filling q0-q31, then checks both
lanes of every register and its FPCR/FPSR values after every resume. The final
unified smoke marker was:

```text
[sched] fp-simd context-isolation=true tasks=2 context-switches=20 checks=[603697,686002] mismatches=[0,0] q-regs=q0-q31 fpcr-fpsr=true signatures=[0x11,0x22]
```

The marker and the host 81/81 result are from
`target/unified-mfs1-resilience-final.log`; the corresponding `make fsck`
result is in `target/unified-mfs1-resilience-fsck.log`.

## GUI profile

When VirtIO-GPU/input is present, windowd creates a 1024×768 XRGB8888
FrameRegion and submits scanout through the bounded GUI path. GUI protocol 6
uses a 64 KiB command FrameRegion, a 4 KiB event Frame, `script::EVENT_GUI` and
an identity-bound endpoint per Mica client. ThreadLaunchV1 remains compatible;
ThreadLaunchV2 atomically installs the three GUI entries. Windowd validates and
atomically replaces Present display lists; validated damage rectangles bound
redraws at up to 60 Hz. Dirty-region culling skips non-intersecting windows,
display lists, icons and taskbar work; overlapping damage is merged and local
`DesktopVisual` windows are redrawn for drag/resize/focus/Terminal updates
without default full redraw. Display-list command/text/glyph work is culled,
input is batched at 16 events, and the glyph cache is 2-way set-associative.
Files and Monitor block on event IPC and report `wait=event-blocked` rather than
spinning on `yield`.
It loads Unifont from MFS with an ASCII/replacement fallback.

There are three built-in windows and eight dynamic Mica windows (desktop
capacity 11). The desktop launcher is a fixed init-owned registry of five
icons: Terminal, Files, Monitor, Reader and Editor. Built-in icon clicks focus
or restore the existing client; Reader and Editor send only an application id
over capability slot 84 (`GUI_LAUNCH_ENDPOINT`). Init maps those ids to the
bundled script path, manifest and entry policy, and windowd deduplicates active
or pending launches. Dynamic registration carries the same application id in
word 3 for ownership binding. The `Icon` display-list command remains drawing
only and cannot register an application. `require("gui")` gives a Mica file a single retained window with
label/button/text-input/checkbox/list/scroll/layout/spacer/canvas widgets; the
script receives no framebuffer, GPU/BAR, input, DMA or IRQ capability. Terminal
supports the tested keyboard/editing and command subset; Files and Monitor
remain fixed-window business clients.

The final unified acceptance passed host MFS1 22/22, ABI capability 9/9, GUI
command 6/6 and Desktop 9/9, kernel 11/11 and Mica 24/24 (81/81 total).
All smoke, powercut, fault, GUI, SSH and Mica/TLS gates passed. The GUI gate passed
plus dynamic registration/Present, Unicode, resize/maximize/restore/minimize,
taskbar, close, endpoint reclaim and slot reuse. Unicode accumulators
`4/78/1250/20013` were delivered/rendered. The current GUI markers include
partial Present `rect=1024x720+0,48`, pending-event batch `16`, renderer
`damage-merge=true local-window-damage=true command-cull=true glyph-cache=2way
input-batch=16`, and VirtIO-net `rx-buffers=2`. Geometry checks reported max `962`, resize/restore
`418`, minimized `0`, taskbar `121`, with stable client crop `9014e4dd`.
Endpoint 1 registered/presented/reclaimed twice and was reused; endpoint 0 was
reclaimed. The second and third sessions used the persistent serial shell while
the first remained on SSH, proving GUI-session concurrency. Final acceptance
evidence is in `target/unified-mfs1-resilience-final.log` and
`target/unified-mfs1-resilience-fsck.log`; browser-specific fixture evidence remains in
the GUI/network logs listed by the runbook.
OrbStack QEMU TCG is functional/regression and work-reduction evidence only;
this gate is not a native-hardware performance benchmark and reports no
percentage speedup.

The MFS P0 contract preserves the dirty mutation order when `fsync` returns
`Err`; retry replays that order. `WriteAtomic` performs one `replace_file`
transaction (`Remove(target)` + `Rename(temp,target)`) and a durable final
fsync. All four fault points remounted as old+temp or new+no-temp, then retry
finished new+no-temp. Directory descendant renames are rejected without dirty
mutation. The GUI partial-Present path remains bounded across user-rt, kernel,
GPU and windowd, with pointer damage carried through to redraw.

The built-in Terminal has a bounded command surface (`ls`, `cat`, `stat`,
`write`, `mkdir`, `mv`, `rm`, `sync`) over absolute paths only. Its commands use
the dedicated `TERMINAL_FILESYSTEM_ENDPOINT` and terminal-only
`GUI_TERMINAL_COMMANDS` 4 KiB shared frame; the MFS broker keeps this frame
separate from block-DMA and other filesystem payload frames. Command input is
limited to 512 bytes and replies/output to 4 KiB, with the terminal viewport
following newest output. The serial shell parser covers `stat`, `mv` and `rm`
(targeted 2/2); the final GUI marker sequence is
`mkdir/write/stat/mv/cat/ls/rm/rm/sync`, each `status=ok`.
