# Architecture

MicroSystem targets QEMU `virt` profiles on AArch64 and RV64GC, plus an x86_64
Q35 profile. The AArch64 profile uses `virt-7.2`, two CPUs, 4 KiB pages and
256 MiB RAM; the RISC-V profile uses RV64GC, two harts and QEMU `virt`; the
x86_64 profile uses two vCPUs, Q35, local APIC/IOAPIC routing, Intel VT-d, and
VirtIO. Each kernel consumes the platform's boot resource description for its
UART, interrupt controller, PCIe/VirtIO and IOMMU resources; device policy is
not encoded as a portable Linux device model.

## Target status and validation

The RV64GC path is implemented and has current QEMU `virt` serial evidence. Its
validated chain is RV64GC → QEMU `virt` → OpenSBI M-mode → S-mode → Sv39 → two
harts → PLIC/SBI timer and interrupt services → PCI VirtIO → RISC-V IOMMU. The
serial run reaches `[system] shutdown`.

The IOMMU fault gate records
`blocked=true sentinel=true event=0xf stream-id=0x10 completion-error=false`.
The same run verifies DMA map/unmap, RNG first-fill, MFS recovery with clean
`fsck`, the complete serial-shell filesystem command matrix, the Mica 8 KiB
argument/input path, and network/DNS. The AArch64 build and serial run also
reach shutdown. The `make test` log's built-in host suites total `81/81`
(`22+9+6+9+11+24`); a separate shell-parser run adds `6/6`, for `87/87`
combined recorded host checks. `make test` itself is not being described as
`87`.

`ARCH=riscv64` selects this validated RISC-V target/QEMU path. The command shape
is:

```sh
ARCH=riscv64 cargo build --release \
  --target riscv64gc-unknown-none-elf \
  -p microsystem-kernel --features baremetal
ARCH=riscv64 qemu-system-riscv64 \
  -machine virt,iommu-sys=on -bios default -cpu rv64 \
  -kernel target/riscv64gc-unknown-none-elf/release/microsystem-kernel \
  -nographic
```

This is a guest run with serial and IOMMU evidence, not a build-only result.

### QEMU virt/OpenSBI/S-mode contract

For a RISC-V `virt` bring-up, `-bios default` means that QEMU selects its
default OpenSBI firmware; no OpenSBI binary is checked into this repository.
OpenSBI owns M-mode and hands the kernel an S-mode entry plus the machine DTB.
The kernel uses SBI services where firmware owns hart start, timer/IPI or reset
operations and does not assume direct M-mode control or an AArch64 EL1 entry
contract. The validated run uses the default firmware path.

The RISC-V path has separate DTB-discovered NS16550/Goldfish RTC,
CLINT/ACLINT, PLIC, Sv39, supervisor-trap, SBI, dual-hart context-switch,
PCI VirtIO, and RISC-V IOMMU command/device/fault queue code. The IOMMU
fault-probe marker above confirms blocked DMA reaches the expected sentinel
without a completion error.

Evidence remains profile-specific: the listed RISC-V gates are current QEMU
evidence, while the exact `make ARCH=aarch64 test` and
`make ARCH=riscv64 test` full matrices remain `validation in progress`. The
x86_64 focused QEMU acceptance is passed, but `make ARCH=x86_64 test` has not
been run and is not a PASS result.

### x86_64 Q35/Multiboot2/VT-d contract

The x86_64 build target is `x86_64-unknown-none`. `xtask` places the kernel in
a GRUB Multiboot2 ISO, which QEMU boots on Q35 with two vCPUs. The x86_64
mechanism path uses long mode, the local APIC timer, IOAPIC INTx routing, Intel
VT-d for DMA isolation, and VirtIO for block, network, and RNG devices. PCI
interrupt routing reads the firmware-provided PCI Interrupt Line; the accepted
VirtIO-blk run bound line 11 to `ioapic-id=11` and observed two INTx
completions. VirtIO PCI discovery bounds each capability region to the firmware
MMIO window and the probed size of its assigned BAR, validates common and
notification region sizes, and checks queue notification offsets before
writing them.

`ARCH=x86_64 make build` passed; the full `make ARCH=x86_64 test` matrix has not
been run and is not a PASS result.

The supported serial command shape is:

```sh
ARCH=x86_64 make build
ARCH=x86_64 make run
```

## Boot and address spaces

On AArch64, EL1 validates the DTB, installs the high-half/MMU mappings,
initializes the GICv3 timer (5 ms slice), discovers VirtIO PCI and creates the
bounded SMMUv3 domain. On RISC-V, S-mode performs the corresponding DTB,
Sv39, PLIC/SBI, PCI VirtIO and RISC-V IOMMU setup. Both paths prepare init/root
and one idle task per CPU/hart; init then starts resident EL0 services by bootfs
name, and each service is re-parsed as a static ELF64 `ET_EXEC` image with
page-granular W^X checks; its entry point must lie in an executable `PT_LOAD`
segment. On x86_64, the Multiboot2 entry installs long-mode
high-half/user mappings, initializes the local APIC timer and IOAPIC, creates the
Intel VT-d domain, and prepares the same init/root and resident EL0 service
layout.

The boot entry passes Multiboot2 information into discovery. The memory map
preserves usable intervals and firmware/module reservations; ACPI RSDT/XSDT,
MADT, MCFG and DMAR are length/checksum checked. Enabled CPU IDs select the
bootstrap and secondary APIC destinations. Static `\_SB.PCI0._CRS` buffers
provide the root MMIO allocation window; dynamic AML methods remain unsupported.
The current VT-d profile accepts one segment-zero hardware unit and records its
endpoint scope. Early mapping bounds allocatable x86 memory to the first 4 GiB;
the frame allocator reserves all low boot/kernel/heap memory across intervals.
Firmware-table errors retain diagnostics and do not invent replacement resources.

Each PCI requester stream gets its own IOMMU context and page tables. Runtime
DMA mappings stay in the block stream; network, RNG, GPU and input streams
receive only their own queue and framebuffer pages. The physical allocator
reads all FDT RAM regions, excludes `/memreserve/` and `/reserved-memory`
ranges, and tracks at most 256 MiB of frames. It fails closed if its fixed
memory-map tables overflow. PCIe bridge setup assigns bounded secondary buses
and memory windows; the software limit is eight buses. Modern VirtIO-net
discovery includes those buses. AArch64 uses SMMUv3 two-level stream tables,
covering 16-bit requester IDs with eight resident second-level tables.

The bootfs has 25 entries (24 static ELF files and `etc/services`).
`SERVICE_COUNT=13` and the service-slot order is:

```text
init console block mfs shell devmgr windowd terminal files monitor sshd netd db
```

Without GPU/input, init starts the non-GUI manifest
`devmgr,console,block,mfs,db,shell,netd,sshd`; with GPU/input it additionally
starts `windowd,terminal,files,monitor`. The serial readiness mask reports
`8/8` for its required slots, while the GUI profile enables all 13 slots and
reports `13/13`. Service address spaces use ASIDs `0x20..0x2c`. Ordinary
application slots are independent, capacity sixteen, and the first dynamic PID
is 14. The tested Mica process is created through `ThreadStartEx` and is not a
resident service.

Each ordinary task receives a 64 KiB stack, a 1 MiB demand-backed user-rt heap
and a `0xe0000`-byte image window (`0x100000` on x86-64). MFS and database
receive a 32 MiB demand-backed heap; windowd
is allowed to map the large-window VA for GUI `FrameRegion` mappings but does
not receive that heap backing. The kernel's lock-free zeroing bump heap is
64 MiB; the FrameAllocator starts after it and is separately protected. User
mappings and ASIDs are invalidated on exit/reuse.

Tasks also have 32 anonymous virtual regions with a 16 MiB reservation budget.
Reservations consume physical pages on first access. They support in-place
growth/shrink, read-only protection and complete unmapping; exit, faults and
kill reclaim data and page-table pages. User-rt uses this path for allocations
that exceed its initial heap. `VirtualStats` and process owned-page counters
expose memory use. The allocator no longer preallocates and clears 1 MiB for
every ordinary task or 32 MiB for each storage/database service.

## Service lifecycle

Init supervises every started service except itself. The kernel retains service
identity, startup epochs, readiness times and exit status, completes abandoned
IPC with `Io`, and restores the service's fixed boot grants on restart. Runtime
caps and mappings are reclaimed before reloading an image. VirtIO block must
acknowledge controller reset and complete IOMMU unmapping before DMA memory is
released; failed quiescence pins the memory and disables automatic restart.

Init stops dependent services and script sessions, then restarts in dependency
order. Startup has a 120-second limit. Recovery waits 1, 2, 4, 8, 16 and then
30 seconds between repeated failures, with eight attempts before an explicit
administrative restart is required. Sixty seconds online resets this budget.
Service initialization reports online before waiting for its first client, so
recovery cannot deadlock on readiness. Shell can operate with storage/database
unavailable. `service list`, `service status NAME`, `service stop NAME` and
`service restart NAME` are available from shell and GUI Terminal.

Runtime initialization fills the scheduler state in place. A large temporary
must not remain on the kernel stack under every exception. Frame-region lookup
borrows the registry entry under the scheduler lock instead of copying an
8 KiB frame list into each lookup's stack frame.

## Native installation and execution

Init owns the application manager endpoint and a separate filesystem frame.
`app install/update NAME VERSION /absolute/ELF [PAGES] [none|random|stats|all]`
copies a static ELF to an immutable version, validates it and stores its SHA-256,
anonymous-memory budget and read-capability grants. Files and metadata are
fsynced before an atomic active-version pointer is published. `app rollback`
verifies the preceding version before swapping that pointer. Launch revalidates
ELF bounds and the checksum, then the kernel snapshots the file bytes. Existing
process images are independent of filesystem updates. The checksum detects
corruption; publication is authorized by the installer capability.

`app list/info/run/exec` are also available in Terminal. Unregistered `exec`
accepts an absolute static ELF path with the default memory budget and no
information capabilities. The manager is separate from the ordinary process
broker; the process broker cannot launch core service identities. Native
applications receive no process-control, filesystem or network endpoint, and
information capabilities are explicitly selected at installation.

The initial user heap is 1 MiB; `PAGES` bounds additional anonymous memory
(1–4,096 pages). The kernel heap reservation is now 64 MiB and holds fixed image,
stack and page-table slots. User data uses the frame allocator. The pressure
fixture is loaded from MFS, holds 16 MiB per process, and observes `NoMemory`
before any partial commit. It is outside the boot archive.

## EL1/EL0 boundary

The trusted computing base keeps only mechanisms that must touch hardware:

- page-table writes, EL0 address-space setup and W^X validation;
- GIC, local APIC/IOAPIC, and INTx routing, IRQ acknowledgement, and SMMUv3,
  RISC-V IOMMU, or Intel VT-d domain/fault handling;
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
Mica script is protocol 8, network is protocol 9, and database is protocol 10.

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
same-origin, while an explicit address-bar URL or redirect may choose another
origin. The fixed Reader policy also grants `fs.write:/data` so its explicit
Save control can atomically persist a loaded response of at most 32,512 bytes;
it does not grant filesystem reads. TLS 1.3 validation retains the trust
anchor when a server repeats it as the chain tail (the redundant tail is
omitted before revalidation) and explicitly supports P-384 ECDSA
`CertificateVerify`.

### User-space SQL service

`db` is a resident EL0 user-space service, not a kernel database or a SQLite
library. The shell reaches it through `protocol::DATABASE = 10` and the
capability-scoped `DATABASE_ENDPOINT`. SQL input and the response occupy the
same 4 KiB IPC shared frame; row responses use the `SQL1` response header and
tagged `Null`, `Integer`, `Text` and `Bool` values. The complete wire layout and
the supported SQL subset are in [docs/abi.md](abi.md).

At startup, `db` creates `/.system/db` if needed and loads the MSQLDB1 v1
snapshot at `/.system/db/main.db`; a missing file starts an empty database.
CRC failure or an unknown snapshot version reports `Corrupt` and stops the
service; it never silently replaces the snapshot with an empty database.
The service uses MFS1 through the filesystem endpoint rather than accessing
the block device directly. A successful mutating statement is executed on a
candidate copy, written in frame-bounded chunks through
`/.system/db/main.db.tmp`, fsynced, atomically replaced into `main.db`, and
fsynced again before the candidate becomes live. Thus each `Execute` carries
one statement and has a single statement commit boundary. Parse, constraint, or
persistence failure does not adopt the candidate. `SELECT` returns rows without
a persistence write. The
MSQLDB1 snapshot is a MicroSystem format with CRC32C, not a SQLite database
file and not a SQLite-compatible client/storage contract.

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

Range writes publish Patch records for the modified bytes and preserve file
ownership/mode/birth time. Attributes are separate checksummed records with four
Unix-second timestamps; the service samples RTC time for modifications and uses
noatime for reads. A file fsync includes preceding directory renames affecting
that file. Transactions that fit one segment are kept together, so metadata
records do not join otherwise independent GC victims. These formats, durability
rules and current memory limits are described in `docs/mfs1.md`.

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

The marker is from the retained AArch64 run. The `make test` log contains
`81/81` built-in host suites; the separate shell-parser run is `6/6`, for
`87/87` combined recorded host checks. The evidence is from
`target/unified-mfs1-resilience-final.log`; the corresponding `make fsck`
result is in `target/unified-mfs1-resilience-fsck.log`. The architecture-specific
full `make test` matrices remain `validation in progress`.

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

The retained AArch64 GUI gate passed. Its `make test` log contains `81/81`
built-in host suites; a separate shell-parser run contributes `6/6`, for
`87/87` combined recorded host checks. The exact AArch64 and RISC-V `make test`
matrices remain `validation in progress`.
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

The built-in Terminal has a bounded command surface (`ls/list`, `cat/read`,
`stat`, `touch/create`, `cp`, `write`, `append`, `mkdir`, `rmdir`, `mv/rename`,
`rm/remove/unlink`, `fsync`, `sync`) with terminal-local `pwd`/`cd` and relative
path resolution. Whole file arguments support single/double quotes without
escapes, expansion or concatenation. `mv` resolves directory destinations to
the source basename and retains the no-overwrite contract. `Append=17` reads
the current EOF and publishes one complete candidate within the serialized
MFS service, avoiding a client-side stat/write race; durability requires
`fsync` or `sync`. Its commands use
the dedicated `TERMINAL_FILESYSTEM_ENDPOINT` and terminal-only
`GUI_TERMINAL_COMMANDS` 4 KiB shared frame; the MFS broker keeps this frame
separate from block-DMA and other filesystem payload frames. Command input is
limited to 512 bytes and replies/output to 4 KiB, with the terminal viewport
following newest output. The serial shell and GUI terminal share the parser
and filesystem command aliases; the final GUI marker sequence is
`mkdir/touch/write/cp/stat/mv/cat/ls/fsync/rm/rm/rm/rmdir/sync`, each
`status=ok`.

The serial shell additionally exposes `help`, `pwd`, `cd`, `echo`, `clear`,
`history`, `ps`, `kill`, `wait`, `uptime`, `sleep`, `date`, `free`, `sysinfo`,
`head`, `tail`, `wc`, `hexdump/xxd`, `grep`, `find`, `tree`, `du`, `df`,
`curl`, `nslookup`, `netstat`, `run`, `mica`, `exit`, `shutdown`, `poweroff`,
and `reboot`. `fs <command>` exposes the filesystem commands and aliases;
`head/tail -n N`, `mkdir -p`, `cp -r`, and `rm -r` are supported bounded forms.
The serial shell and GUI Terminal share the filesystem parser, aliases and
working-directory path semantics. Terminal `ps` uses the process broker to
list application state and shares boot-program name resolution with the
serial shell. Terminal also supports `kill`, `wait`, system/time/history
commands, `netstat` and power control. DNS, curl, SQL and Mica execution use
the serial shell.

## Firmware, devices and identities

The x86 profile passes Multiboot2 information to the kernel and uses its memory
map/modules plus checksummed ACPI MADT/MCFG/DMAR and static root-bridge `_CRS`
resources. Dynamic AML execution and multiple VT-d units are unsupported and
remain outside the QEMU profile. AArch64 preserves a supplied DTB; its ELF
loader fallback uses QEMU's DTB at 0x40000000. FDT discovery includes all memory
nodes and reserved regions. Kernel mappings currently use the first 4 GiB and
the scheduler activates at most two CPUs even when firmware reports more.

PCIe allocation tracks up to eight buses and forwards subordinate bus numbers
through ancestor bridges. The network device scanner exposes up to four modern
VirtIO interfaces, with independent queues, MAC/link/epoch state, BAR assignment
and PCIe slot removal. SMMUv3, RISC-V IOMMU and VT-d tables admit those bridge
requester IDs. BAR sizing is cached while device/address identity is unchanged,
so active DMA mappings are not invalidated by a resource re-probe. Failed
activation disables bus mastering and reports once until the device changes.
Physical drivers and motherboard qualification await the user's target machine.

Init owns accounts and publishes an atomic readonly snapshot to trusted brokers.
The filesystem applies UID/GID/mode plus ancestor-search permissions after VFS
routing; init applies role and process ownership to launches/kills; administrative
service/network/app/SQL operations require an administrator. SSH has a signed-key
transport plus init-issued credentials, checks epoch/expiry on channel I/O, and
persists host-key rotation. See [identity.md](identity.md) for bootstrap trust,
limits, local audit and failure recovery.
