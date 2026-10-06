# Operating-system maturity implementation

Requested 2026-10-06: implement every item in the functionality assessment.
User selected software/QEMU first; physical-machine acceptance is deferred
until a target machine is provided. This is an implementation ledger, not
evidence of completion.

## Scope and acceptance

- [ ] Service supervision: classified failures, restart/backoff, dependency
  recovery, abandoned IPC completion, bounded readiness and diagnostics.
- [ ] Native applications: filesystem loading, install/update/rollback,
  launch permissions and resource budgets beyond eight fixed applications.
- [ ] Memory: virtual-region management, anonymous demand allocation,
  growth/reclamation and observable memory-pressure failure handling.
- [ ] Storage: VFS/multiple volumes, file ownership/mode/timestamps and
  efficient range I/O with crash-safe persistence.
- [ ] Desktop: asynchronous long commands, clipboard, input composition and
  application-owned multiple windows.
- [ ] Platform: firmware-provided memory/topology/resource discovery and
  QEMU device discovery/hotplug recovery. Physical-device qualification is
  deferred by user choice.
- [ ] Network: persisted configuration, DHCP, multiple interfaces and IPv6.
- [ ] Identity: accounts/roles, authorized-key management/rotation and audit.
- [ ] Qualification: full architecture matrices, service faults, resource
  exhaustion, repeated lifecycles, soak and recorded performance baselines.

## Working rules

Preserve the existing dirty worktree. Baseline diff is stored locally at
`target/maturity-baseline.patch`. Implement related contracts together; use
existing host and OrbStack/QEMU validation. Do not report an item complete
from scaffolding, build-only checks or earlier evidence.

## Current phase

All nine areas have implementation work in this tree; full qualification remains
incomplete. AArch64 service/native/VM/desktop/VFS/network/account gates have passed
individual slices. The latest AArch64 full matrix failed at power-cut persistence,
and mounted-volume recovery after block restart timed out. These remain open
defects in this published snapshot. Physical acceptance is deferred by user choice.

### Implemented software slices

- Service state/epochs, init-only stop/status, failed IPC completion, fixed-grant
  restoration, dependency restart/backoff, administrative hold/resume, controller
  reset/IOMMU quiescence, shell degraded mode and GUI client reconnection.
- Asynchronous Terminal commands and Ctrl+C for sleep/wait, including cancellation
  after a complete GUI restart.
- Filesystem static ELF snapshots; versioned install/update/rollback, SHA-256
  checking, memory budgets, random/stat capability selection and 16 concurrent
  application slots. Native images do not inherit process-control/FS/network caps.
- Anonymous regions, demand-backed heaps, grow/shrink/protect/unmap, physical
  commit preflight, recoverable allocation failure and exit/fault/kill reclamation.
  Kernel heap reservation reduced to 64 MiB. A former 57 KiB persistent scheduler
  initializer frame was removed; AArch64 disassembly now shows about 12 KiB.
- Whole-file Copy broker operation avoids the installer's repeated whole-file
  rewrite. Range I/O and bounded VFS volumes are implemented below; file bodies
  are still materialized in memory rather than loaded from disk on demand.
- Range-write Patch records commit changed bytes, preserving surrounding data;
  small transactions stay within one segment so GC victims stay independent.
  Ownership/mode and four Unix-second timestamps have checksummed attribute
  records, legacy defaults, noatime, RTC-based updates and GC/rename preservation.
  Both shells expose chmod/chown and enriched stat. Account enforcement is
  implemented below; physical multiple-disk support remains outstanding.
- VFS normalized longest-prefix routing; four persistent file-backed volumes,
  readonly mounts, namespace/backing-file protection, cross-volume rename
  rejection, descriptor/child unmount guards and per-volume df. Image block
  flushes use batched range transactions; malformed stored mounts keep root
  available and report their configuration line.
- Four windows per Mica client, independent scenes/focus/sequence/widget ownership,
  reusable window/widget resources and stale-handle rejection; shared 4 KiB UTF-8
  clipboard, input Ctrl+C/X/V and Terminal Ctrl+Shift+C/V; bounded dictionary
  Pinyin preedit/candidates/selection/cancel and UTF-8 Terminal rendering/deletion.
  Cold-start font loading now retries within the service readiness budget.

### Current evidence

- `target/native-pressure-64m-runtime.log`: AArch64 full gate PASS, kernel SHA-256
  `339300f3137c32b0f300008347d5d9679a47e7b8bfa06219a4a1a3fec06db96d`.
  The gate observes all 16 pressure outcomes, NoMemory with zero partial pages,
  kills/waits successful allocations, allocates 16 MiB again, verifies native
  update/corruption/rollback and recovers terminal/netd/mfs/block/devmgr.
- `target/maturity-native-host.log`: 31 meaningful host tests PASS (2 VM,
  2 recovery policy, 14 kernel boundaries, 13 shell/parser).
- Three target checks passed during implementation; current native checks are
  `target/maturity-native-{riscv64,x86_64}-check.log`.
- `target/maturity-riscv64-smoke-check.log`: full serial smoke PASS after console
  byte writes adopted the shared print lock and all profiles normalized CRLF.
- Native OrbStack lacks grub-mkrescue. x86-64 ELF build completed; boot ISO was
  produced using the existing OrbStack Docker image. x86 disk was seeded with
  the same assets and target-native fixtures. `target/maturity-x86_64-smoke-check.log`
  records a full serial smoke PASS.
- `target/maturity-desktop-runtime.log`: AArch64 desktop gate PASS over two complete
  lifecycles, kernel SHA-256
  `b76275e0a72da25160c0cb8373ca7c4f5c81cb459624238690d53132812bada2`.
  Both fields' observed callbacks match clipboard/Chinese edits; window quota,
  cross-window rejection, single-window close survival and stale widget rejection
  pass. Chinese glyphs, preedit/candidate overlay and inactive-window focus were
  inspected in `target/desktop-{two-windows,composition,one-window}.png`.
- `target/maturity-desktop-host.log`: GUI host behavior suites PASS (6 command
  stream and 11 desktop/composition tests). Full AArch64 build PASS.
- `target/maturity-desktop-recovery.log`: the complete native/pressure/service
  recovery gate also PASS on kernel `b76275e0...12bada2` after desktop changes.
- `target/maturity-storage-host.log`: 25 MFS tests PASS, including the existing
  10,024-case crash campaign, range power cuts, 4 KiB versus 4 MiB write cost,
  metadata persistence/GC and child fsync after directory rename.
- `target/maturity-storage-parser-host.log`: 13 shell/parser tests PASS, including
  quoted chmod/chown, invalid octal modes and UID bounds. AArch64 service checks
  PASS; full AArch64 build PASS.
- `target/maturity-storage-runtime.log`: full native/physical-pressure/async-cancel/
  dependency-recovery gate PASS, kernel SHA-256
  `d0d7d712ad2cbf736203946a0a8f9e39d8c41a79b49d588a3f3601f496e5bdaa`.
  Mode 600 and uid/gid 1000 remain after mfs restart. VFS qualification is recorded
  below; account-based permission enforcement was added in the later identity slice.
- `target/maturity-vfs-runtime.log`: AArch64 volume gate PASS with kernel SHA-256
  `cbf5797eadbcc4328a0d6a7eb0fad88db1de3123647f6e914357a8d31882448d`.
  Two mounted volumes remain isolated and retain metadata/readonly configuration
  across two mfs/GUI restart rounds. Per-volume df reports 768 blocks for the
  3 MiB image.
- `target/maturity-vfs-host.log`: three filesystem/VFS behavior tests and 13 shared
  shell parser tests PASS. Native incremental Rust metadata briefly failed with
  a compiler ICE; the rerun with CARGO_INCREMENTAL=0 passed. No toolchain change.

- `target/maturity-network-runtime.log`: dual-interface AArch64 gate PASS, kernel
  SHA-256 `e961f2c96860fa85b8a90a62c8099b31be3c890b89f23a0b93fb3dbb11bc57ca`.
  Both DHCP leases, two connected IPv4 routes, SLAAC prefixes, IPv6 TCP and UDP,
  stale connection rejection, saved static configuration after netd restart,
  preferred-interface link failover and invalid gateway rejection pass. The
  corresponding serial transcript is `target/maturity-network-serial.log`.
- `target/maturity-network-storage-host.log`: FS 3, init recovery 2, network
  configuration 3 and shell parser 13 tests PASS. This invocation used --lib;
  the separate `target/maturity-storage-current-host.log` reruns all 25 MFS tests,
  including the complete crash campaign after batched image writes.
- `target/maturity-firmware-kernel-host.log`: firmware discovery 3, existing VM 2
  and kernel boundary 15 tests PASS. New coverage preserves real CPU IDs,
  ACPI checksums/length rejection, Multiboot memory holes/modules, and exhausted
  primary RAM cannot expose reserved low boot memory.
- AArch64's EL0 transition now masks interrupts while setting ELR/SPSR: the
  reproduced timer race stalled at eret. Expanding the SMMUv3 linear stream
  table also required 16 KiB alignment; DMA isolation is restored in the
  network gate. Firmware x86 runtime and device hotplug remain in progress.
- `target/maturity-firmware-x86-smoke-check.log`: complete two-boot x86 serial
  smoke PASS, kernel SHA-256
  `6236b09184eaa5595ca5d7367042b1a36cc69bb59e15e68a360cd24293f6ea67`.
  Multiboot memory holes/reservations, ACPI CPU/APIC/MCFG/DMAR and static root
  _CRS allocation feed actual memory, interrupts, VirtIO DMA isolation and MFS
  recovery. Probed BAR sizes are cached while identity/addresses are unchanged,
  preventing live-DMA address-space reprobes that triggered QEMU VT-d assertions.
  Native OrbStack still lacks grub-mkrescue; the existing Docker tool created
  the ISO. The disk used the same assets and target-native fixtures.

### Additional implemented slices and evidence

- Atomic readonly account snapshots, eight persistent users, admin/operator/reader
  roles, UID/GID/search/mode checks, descriptor ownership, process ownership,
  native launch policy, public-key registration/removal, epoch/expiry/PID-bound SSH
  cookies, GUI switch/logout, private host-key generation/rotation and bounded audit.
  SSHD prints its public key, bounds handshake/read inactivity, and clears old
  listeners on restart. Account corruption is fail-closed; the serial console
  retains trusted root repair access. Contracts are in `docs/identity.md`.
- IPv6 A/AAAA DNS, explicit resolve family and A-to-AAAA fallback; original-host
  authorization survives resolved IPv6 address transfer. Source-port rotation
  fixes repeated short TCP connect collisions. HTTP responses no longer index
  native socket IDs as tiny array positions.
- Multiboot/ACPI/FDT memory/resource discovery, cached BAR sizes, eight PCI buses,
  ancestor forwarding, and multi-bus SMMUv3/RISC-V/VT-d device tables. Network
  queues have independent stream state and bounded PCIe hotplug. The RISC-V
  cross-target gate caught shared PSCID aliases; contexts now use unique IDs.
- SSH revoke first appeared to close server processing but left the client alive:
  abort() was followed by listen() before RST dispatch. Re-listen now waits until
  the remote tuple has cleared. Probe/signature authentication also replaces the
  old credential rather than exhausting 32 slots. Login audit follows verified
  channel acceptance. These defects have concrete QEMU regressions.
- Console formatted administrative rows and supervisor messages are emitted in
  one UART transaction; SMP entry order is no longer used to identify page-fault
  probes. Mount I/O/resource errors propagate, avoiding rollback to an old copy;
  audited replay releases old materialized bodies before validating the newer one,
  and multipart file replay grows buffers geometrically.
- `target/maturity-final-host.log`: 66 host behavior tests PASS; subsequent storage
  changes are rerun separately. `target/maturity-final-mfs-host.log`: all 25 MFS
  tests PASS. `target/maturity-remediation-storage-host.log` records the mount-error
  regression rerun. Exact latest results still need checking before completion.
- `target/maturity-final-network-runtime.log`: AArch64 network PASS, kernel SHA
  `0eb4c9cf4ea86155ee3ec7a95675f148d57d423fbf1a58ef61a18288853d3912`.
  Includes AAAA-only HTTP, IPv6 UDP/TCP, both DHCP routes, saved configuration,
  link failover and three PCIe insertion/removal cycles.
- `target/maturity-final-identity-runtime.log`: accounts, remote whoami/elevation
  denial, creation ownership, private file/reader/disabled/key rejection, live SSH
  revocation, host rotation, MFS recovery and two boots PASS on `0eb4c9cf...d3912`.
  New RNG host-key/startup/listener/deadline and 36-login regression are being rerun.
- `target/maturity-final-vfs-runtime.log`: two volumes and two restart rounds PASS
  on `0eb4c9cf...d3912`. Later mounted-volume pressure is in progress.
- `target/maturity-soak-fixed-runtime.log`: 12 write/fsync + native launch/wait +
  netd/SSHD lifecycle rounds PASS, zero physical-page drift after warmup; all
  samples have 41,712 free pages. Each round took 0.716–1.343 s; this is a measured
  baseline in this QEMU/container run, not a comparison against different host load.
  Kernel SHA `a84adb868526d69aff703d4956139340c26170cf2e99d796dae83deacfe1d4be`.
- `target/maturity-final-riscv64-smoke-check.log`: complete serial two-boot PASS
  before the PSCID fix. `target/maturity-riscv64-network-current-runtime.log`
  subsequently passes the full dual-interface IPv6/DNS/failover gate and three
  hotplug rounds on kernel SHA-256
  `f9dd22bf9a1bbd099725a7465348f0c39fa3aaf9343584bb65da9130967b78d9`.
  x86 runtime reached shutdown but its parallel page-probe association was falsely
  rejected; validation now binds the PID to init's observed wait record.

### Remaining work

The latest storage host rerun (`target/maturity-remediation-storage-host.log`)
passes all 25 MFS and four filesystem tests, including transient mounted-image
I/O errors. `target/maturity-current-host.log` passes the GUI, identity, init,
kernel, Mica, network and shell suites. Logs under `target/` are retained locally
and ignored by Git; their names here identify recorded runs, not shipped artifacts.

- `target/maturity-matrix-aarch64.log`: host suites and two-boot serial smoke
  PASS, power-cut stage FAIL because write/sync were unavailable and MFS startup
  timed out. Later fault/GUI/SSH/TLS stages were not reached.
- `target/maturity-pressure-diag-runtime.log`: initial physical-pressure and block
  restart stages progressed, but mounted MFS recovery timed out on kernel SHA-256
  `e30cf5db0f661bab1305fb842d46b238406bda346630c2fae80a9f8529a0c5c6`.
  Mounted cold startup is also intermittent; one run reached desktop in 14.756 s
  while another exceeded 120 s. No maximum startup time is established.
- New host-key/startup/listener/deadline changes and the 36-login credential gate
  still need full runtime qualification. Complete mounted-volume pressure/GC,
  cross-target desktop, all architecture fault/GUI/SSH/TLS matrices and a long
  soak. The 12 lifecycle rounds above are a short baseline, not a long soak.

Parent acceptance boxes above remain open until qualification completes. Physical
acceptance stays deferred as requested. Limits (two active CPUs, bounded file
caches/volumes, static AML resources, key-only SSH and local bounded audit) are
documented.
