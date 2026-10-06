# MicroSystem

![Targets](https://img.shields.io/badge/targets-AArch64%20%7C%20RV64GC%20%7C%20x86_64%20%2B%20QEMU-2563eb)
![Rust](https://img.shields.io/badge/implementation-Rust%202024-f97316)
![Status](https://img.shields.io/badge/status-research%20prototype-7c3aed)

> An AI-assisted attempt to implement a complete operating-system ecosystem — from a multi-architecture microkernel to storage, networking, SSH, GUI, a user-space database, and a capability-aware scripting runtime.

[简体中文](README.zh-CN.md) · **English**

MicroSystem is a small, explicit, end-to-end operating-system experiment. The goal is not to recreate Linux in miniature; it is to explore whether AI can help carry a coherent systems design all the way from boot code and hardware isolation to user-facing applications.

## Current target and status

All three boot profiles have real QEMU serial evidence. The validated RISC-V chain is
RV64GC on QEMU `virt`, default OpenSBI M-mode handoff to S-mode, Sv39, two harts,
PLIC/SBI timer and interrupt services, PCI VirtIO, and the RISC-V IOMMU. Its
serial run reaches `[system] shutdown`.

The RISC-V IOMMU gate records
`blocked=true sentinel=true event=0xf stream-id=0x10 completion-error=false`.
The same run verifies DMA map/unmap, RNG first-fill, MFS recovery with clean
`fsck`, the complete serial-shell filesystem command matrix, the Mica 8 KiB
argument/input path, and network/DNS. The AArch64 build and serial run also
reach shutdown. The `make test` log's built-in host suites total `81/81`
(`22+9+6+9+11+24`); a separate shell-parser run adds `6/6`, so the combined
recorded host checks are `87/87`. This does not mean `make test` itself reports
87.

The exact `make ARCH=aarch64 test` and `make ARCH=riscv64 test` full matrices
remain `validation in progress`; their in-flight state is not a PASS result.

The x86_64 path has also passed a focused QEMU serial acceptance run. It uses the
`x86_64-unknown-none` target, a GRUB Multiboot2 ISO, QEMU Q35 with two vCPUs,
local APIC/IOAPIC routing, Intel VT-d, and VirtIO devices. `ARCH=x86_64 make
build` passed; the acceptance run reached SMP/RR/IPC, resident EL0 services,
the blocked-DMA probe, VirtIO-blk INTx completions, block/MFS I/O, and the
serial shell's `help` and `shutdown`. The full `make ARCH=x86_64 test` matrix
has not been run and is not claimed as PASS.

The latest source snapshot also includes a resident EL0 `db` service and the
`microsystem-sql` library. It exposes a bounded CRUD subset through the serial
shell's `sql` command, uses protocol 10 over a capability-scoped IPC endpoint,
and persists MSQLDB1 snapshots through MFS1 with CRC32C validation and atomic
replace. This is a MicroSystem storage format, not SQLite compatibility.

The GUI renderer has also moved to a light-blue system surface: a pale system
bar, centered Dock-like launcher area, rounded windows, left-aligned red/yellow/
green controls, and blue focus/selection accents. The GUI ABI and capability
boundary remain unchanged; the checked-in PNG below is an earlier VNC frame and
does not claim to be a live capture of this visual refresh.

## See it running

The image below is a real GUI frame captured from the repository's QEMU GUI profile, which exposes the guest through VNC on the default port `5900`. It shows the code-drawn desktop, Terminal, Mica Counter, Monitor, Files, Reader, and Editor launchers.

![MicroSystem desktop captured through the GUI/VNC path](docs/assets/microsystem-vnc.png)

*This PNG is a QMP screendump from an earlier VNC-backed GUI run, not a mockup or a screenshot of the VNC client chrome. Current focused functionality checks are recorded in the runbook.*

## System architecture

The design keeps hardware mechanisms in AArch64 EL1, RISC-V S-mode, or x86_64
long mode and pushes
policy into isolated EL0/U-mode services. Applications use brokered IPC and
capabilities rather than receiving raw framebuffer, block, network, DMA, or IRQ
access.

![MicroSystem system architecture](docs/assets/microsystem-architecture.svg)

The main path is:

```text
QEMU virt / Q35 hardware profiles
          ↓
EL1 / S-mode / long-mode microkernel: MMU · scheduling · IPC · capabilities · IRQ/IOMMU
          ↓
EL0 services: init · devmgr · block · MFS1 · db · netd · sshd · windowd
          ↓
Mica scripts and GUI applications: Counter · Reader · Editor · Terminal
```

## What is implemented

| Layer | Current experiment |
| --- | --- |
| Kernel | AArch64 EL1, RV64GC S-mode, and x86_64 long-mode boot; MMU/Sv39, GICv3, PLIC/SBI, or local APIC/IOAPIC timer/interrupts; scheduling, ASIDs, W^X ELF loading, capability derivation/revoke, and bounded resource cleanup |
| Isolation | DTB-driven or PCI-config-driven VirtIO discovery, SMMUv3, RISC-V IOMMU, or Intel VT-d domain setup, device grants, and strict privileged-mechanism / user-policy boundaries |
| Storage | MFS1 transactional filesystem, metadata mirrors, fsck, offline narrow repair, power-cut and deterministic fault-injection paths |
| Database | Resident EL0 `db` service, bounded `microsystem-sql` CRUD subset, 4 KiB IPC responses, 4,096-row global budget, MSQLDB1 snapshots, CRC32C validation, atomic MFS1 persistence, and fail-closed corruption handling |
| Network and access | EL0 `netd`, endpoint-brokered networking, bounded HTTP/HTTPS access, SSH sessions, and policy intersection for scripts |
| Desktop | VirtIO-GPU/input profile, code-drawn 1024×768 desktop, retained GUI command stream, window management, input batching, damage culling, Terminal, Files, Monitor, Reader, and Editor |
| Runtime | Mica lexer/compiler/VM, capability-aware permissions, filesystem/network/GUI brokers, and bounded application slots |

The repository targets QEMU `virt-7.2` AArch64, RV64GC QEMU `virt`, and x86_64
Q35 profiles.
It is a research prototype, not a Linux/POSIX distribution, general-purpose
desktop, or promise of arbitrary real-hardware and ELF compatibility.

## Quick start

The supported development path uses the pinned Rust toolchain and an OrbStack Docker context:

```sh
docker context show                         # expected: orbstack
make build
make test
make fsck
ARCH=x86_64 make build
ARCH=x86_64 make run                 # x86_64 Q35 serial path
```

The commands above are the supported AArch64 path. `ARCH=riscv64 make build`
and `ARCH=riscv64 make run` select the validated RISC-V target/QEMU path. The
x86_64 commands `ARCH=x86_64 make build` and `ARCH=x86_64 make run` select the
x86_64 target and serial QEMU path. The x86_64 profile uses a GRUB Multiboot2
ISO and Q35 with two vCPUs, local APIC/IOAPIC, Intel VT-d, and VirtIO. Its
focused acceptance
passed, while the full `ARCH=x86_64 make test` matrix remains unrun. The
lower-level RISC-V command shape is:

```sh
ARCH=riscv64 cargo build --release \
  --target riscv64gc-unknown-none-elf \
  -p microsystem-kernel --features baremetal
ARCH=riscv64 qemu-system-riscv64 \
  -machine virt,iommu-sys=on -bios default -cpu rv64 \
  -kernel target/riscv64gc-unknown-none-elf/release/microsystem-kernel \
  -nographic
```

The RISC-V command uses QEMU's default OpenSBI firmware: OpenSBI keeps M-mode
and hands the kernel an S-mode entry with a DTB. The repository does not ship an
OpenSBI image. See the [architecture](docs/architecture.md) and
[runbook](docs/runbook.md) for the profile-specific evidence and the full-matrix
validation status.

Run the serial shell:

```sh
make run
```

The shell filesystem commands accept absolute paths or paths relative to `cd`.
Both short forms and the
explicit `fs` namespace are accepted: `ls/list`, `cat/read`, `stat`,
`touch/create`, `cp`, `write`, `append`, `mkdir`, `rmdir`, `mv/rename`,
`rm/remove/unlink`, `fsync`, and `sync`. File transfers are bounded by
the 4 KiB filesystem shared frame.

Whole file arguments can use single or double quotes: `write "work notes" "hello"`,
`append "work notes" " world"`, and `grep "hello world" "work notes"`.
Quoting does not add escapes, expansion or concatenation. `append` creates a
missing file and appends in one serialized service request; use `fsync` or `sync`
for durability. `mv file directory` moves into the directory and refuses to
overwrite an existing target.

The complete serial shell surface is `help`, `pwd`, `cd`, `echo`, `clear`,
`history`, `ps`, `kill`, `wait`, `uptime`, `sleep`, `date`, `free`, `sysinfo`,
the filesystem commands above plus `head`, `tail`, `wc`, `hexdump/xxd`, `grep`,
`find`, `tree`, `du`, and `df`, network `curl`, `nslookup`, `netstat`, and `net`,
program commands `run` and `mica`, and system commands `exit`, `shutdown`,
`poweroff`, and `reboot`. `fs <command>` exposes the filesystem namespace with
the same aliases; `head/tail -n N`, `mkdir -p`, `cp -r`, and `rm -r` are bounded
options.

The serial shell also exposes the bounded SQL service:

```text
sql CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT NOT NULL)
sql INSERT INTO users VALUES (1, 'Alice')
sql SELECT * FROM users WHERE id = 1
```

The supported subset covers `CREATE`, `DROP`, `INSERT`, `SELECT`, `UPDATE`, and
`DELETE` with `INTEGER`, `TEXT`, `BOOL`, `NULL`, primary-key, and `NOT NULL`
semantics. Each statement is limited to 4 KiB and each mutating statement is
committed as one bounded MFS1 snapshot transaction at `/.system/db/main.db`.
The database has a global budget of 4,096 rows. A corrupt or unavailable
snapshot leaves the service online for `Ping` but disables SQL execution rather
than silently replacing the database.
The full wire contract and storage boundary are documented in
[`docs/abi.md`](docs/abi.md) and [`docs/architecture.md`](docs/architecture.md);
the service is not SQLite-compatible.

The serial shell also provides a bounded curl-like HTTP client. It supports
GET over HTTP/HTTPS, `-i`/`--include`, `-s`/`--silent`, and atomic `-o`/
`--output` to an absolute MFS path:

```text
curl -i https://example.com/
curl -s -o /data/response https://example.com/
```

It performs GET only through the `net.browse` broker: HTTP is plaintext, while
HTTPS uses the trusted TLS path with certificate, hostname, and time checks.
Responses are limited to 32 KiB; console output must be UTF-8, and `-o` writes
an absolute MFS path atomically with fsync. Raw TCP/UDP, POST/PUT/PATCH/DELETE,
and the low-level TLS broker are outside this command's boundary.

The GUI Terminal and serial shell share the filesystem parser, aliases, quoted
arguments, working-directory path resolution and MFS broker behavior. GUI
Terminal also provides application `ps`, `kill`/`wait`, system/time/history
commands, `netstat` and power control through its bounded frame. DNS, curl, SQL
and Mica execution use the serial shell.

Both shells support `service list`, `service status NAME`, `service stop NAME`
and `service restart NAME`. Init supervises service recovery with dependency
ordering and bounded retries. Terminal `sleep`/`wait` run asynchronously and
can be interrupted with Ctrl+C while desktop input remains active. Native
process capacity is 16. User heaps and anonymous regions are demand-backed;
the allocator can grow beyond its initial heap and reclaim large allocations.
Run the AArch64 service/cancellation gate with `make test-recovery`; it uses an
isolated disk copy. The full maturity implementation is tracked in
[the working ledger](.workflow/os-maturity/plan.md).

Accounts, roles, per-user file/process permissions, public-key management,
SSH host-key rotation and local audit are described in [identity.md](docs/identity.md).
Use `user list`, `whoami`, `user add alice operator`, `user key-add alice HEX`,
`user switch alice` and `user audit`. Run its isolated-disk gate with
`make test-identity`. The serial console is trusted root; remote login is key-only.

Mica GUI applications support four independent windows, a shared UTF-8 clipboard
and dictionary Pinyin composition (Ctrl+Space). Ctrl+A then Ctrl+C/X/V edits
text-input selections; Terminal uses Ctrl+Shift+C/V for its current input line.
Run the repeated multi-window, clipboard and Chinese-input gate with
`make test-desktop`. See [desktop contracts](docs/gui.md) for limits and the
editable dictionary.

Install and run an on-disk static ELF:

```text
app install example v1 /.system/examples/native/counter.elf 64 stats
app run example
app update example v2 /.system/examples/native/counter.elf 64 all
app rollback example
app info example
```

Versions carry an ELF checksum and capability policy. `64` allows 64 additional
anonymous pages above the initial 1 MiB heap; `stats` grants system counters,
and `all` grants system counters and randomness. The manager supports 16 native
process slots, checks installed data before launch, and preserves running
images during updates. `app exec /absolute/program.elf` runs an unregistered
static image with the default memory budget and no information caps.

Run the GUI profile. The QEMU command prints the VNC/QMP endpoints; the default VNC port is `5900`.

```sh
make gui
```

Example Mica programs:

```text
mica -e 'print(40 + 2)'
mica --gui --timeout 86400s --allow gui.window /mica/gui-counter.mica
mica --gui --timeout 86400s --allow gui.window --allow net.browse \
  --allow fs.write:/data \
  /.system/examples/mica/browser.mica -- https://example.com/
```

The Reader follows up to eight HTTP(S) redirects. Clicking Save atomically writes
the raw response (up to 32,512 bytes) below `/data`; page links remain same-origin.

## Recorded acceptance evidence

The repository contains the detailed runbook and raw workflow results. Earlier
evidence records the RISC-V QEMU serial/IOMMU/device gates above, AArch64 build
and serial shutdown, and the focused x86_64 QEMU acceptance (including
resident services, Intel VT-d, VirtIO-blk INTx, block/MFS I/O, and shell
shutdown). It also records `81/81` built-in `make test` host suites and a separate
shell-parser run of `6/6` (`87/87` combined recorded host checks). The exact
`make ARCH=aarch64 test`, `make ARCH=riscv64 test`, and `make ARCH=x86_64 test`
matrices have not all passed for the current functionality changes. The latest
AArch64 matrix passed host suites and serial smoke, then failed at power-cut
persistence; mounted-volume restart recovery also remains unresolved. Focused
checks and remaining qualification are recorded in [`docs/runbook.md`](docs/runbook.md).
Older matrix evidence remains in
[`.workflow/microsystem-kernel/results/tests.md`](.workflow/microsystem-kernel/results/tests.md).

## Repository map

```text
crates/kernel/     EL1, S-mode, and x86_64 boot/runtime mechanisms
crates/mfs1/       MFS1 filesystem implementation
crates/sql/        bounded SQL parser, executor, and MSQLDB1 snapshot format
crates/mica/       Mica language, VM, permissions, and standard library
crates/gui/        GUI ABI and command-stream types
services/          Static EL0 services and example applications, including `db`
assets/            CA and font inputs used by the guest image
docs/              Architecture, ABI, GUI, MFS1, network, SSH, and runbook docs
scripts/           OrbStack/QEMU build and acceptance helpers
xtask/             Image generation and QEMU orchestration
```

## Read next

- [Architecture](docs/architecture.md) — boot, address spaces, IPC, capabilities, storage, and GUI boundaries.
- [ABI](docs/abi.md) — protocol 10, database frames, response layout, and service contracts.
- [GUI profile](docs/gui.md) — windowd, retained Present, input, launch policy, and the VNC/QEMU path.
- [Mica Programming Guide](docs/mica-programming.md) — language basics, permissions, filesystem, HTTP, and GUI examples.
- [Mica runtime contract](docs/mica.md) — language, VM limits, permissions, and broker APIs.
- [MFS1](docs/mfs1.md) — transaction format, recovery, fault model, fsck, and repair boundary.
- [Network and `netd`](docs/network.md) — endpoint policy and bounded networking.
- [SSH](docs/ssh.md) — SSH commands and launch policy.
- [Runbook](docs/runbook.md) — reproducible commands and recorded acceptance evidence.

## Project intent

MicroSystem is deliberately small enough to inspect and opinionated enough to expose trade-offs. Its central question is practical: can an AI-assisted engineering loop produce not just isolated kernel code, but a consistent operating-system ecosystem with storage, networking, GUI, applications, failure handling, and documented security boundaries?

The answer is still being explored. Contributions, criticism, and reproducible experiments are welcome.

## License

MicroSystem is released under the [MIT License](LICENSE). Third-party assets
retain their own license terms; see the notices in [`assets/`](assets/),
including the bundled font and certificate materials.
