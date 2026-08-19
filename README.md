# MicroSystem

![AArch64](https://img.shields.io/badge/target-AArch64%20%2B%20QEMU-2563eb)
![Rust](https://img.shields.io/badge/implementation-Rust%202024-f97316)
![Status](https://img.shields.io/badge/status-research%20prototype-7c3aed)

> An AI-assisted attempt to implement a complete operating-system ecosystem — from an AArch64 microkernel to storage, networking, SSH, GUI, and a capability-aware scripting runtime.

[简体中文](README.zh-CN.md) · **English**

MicroSystem is a small, explicit, end-to-end operating-system experiment. The goal is not to recreate Linux in miniature; it is to explore whether AI can help carry a coherent systems design all the way from boot code and hardware isolation to user-facing applications.

## See it running

The image below is a real GUI frame captured from the repository's QEMU GUI profile, which exposes the guest through VNC on the default port `5900`. It shows the code-drawn desktop, Terminal, Mica Counter, Monitor, Files, Reader, and Editor launchers.

![MicroSystem desktop captured through the GUI/VNC path](docs/assets/microsystem-vnc.png)

*This PNG is a QMP screendump from an earlier VNC-backed GUI run, not a mockup or a screenshot of the VNC client chrome; it was not captured live during this documentation-only publishing pass.*

## System architecture

The design keeps hardware mechanisms in EL1 and pushes policy into isolated EL0 services. Applications use brokered IPC and capabilities rather than receiving raw framebuffer, block, network, DMA, or IRQ access.

![MicroSystem system architecture](docs/assets/microsystem-architecture.svg)

The main path is:

```text
QEMU virt-7.2 hardware
          ↓
EL1 microkernel: MMU · scheduling · IPC · capabilities · IRQ/IOMMU
          ↓
EL0 services: init · devmgr · block · MFS1 · netd · sshd · windowd
          ↓
Mica scripts and GUI applications: Counter · Reader · Editor · Terminal
```

## What is implemented

| Layer | Current experiment |
| --- | --- |
| Kernel | AArch64 EL1 boot, high-half MMU, GICv3 timer/interrupts, scheduling, ASIDs, W^X ELF loading, capability derivation/revoke, and bounded resource cleanup |
| Isolation | DTB-driven VirtIO discovery, SMMUv3 domain setup, device grants, and a strict EL1 mechanism / EL0 policy boundary |
| Storage | MFS1 transactional filesystem, metadata mirrors, fsck, offline narrow repair, power-cut and deterministic fault-injection paths |
| Network and access | EL0 `netd`, endpoint-brokered networking, bounded HTTP/HTTPS access, SSH sessions, and policy intersection for scripts |
| Desktop | VirtIO-GPU/input profile, code-drawn 1024×768 desktop, retained GUI command stream, window management, input batching, damage culling, Terminal, Files, Monitor, Reader, and Editor |
| Runtime | Mica lexer/compiler/VM, capability-aware permissions, filesystem/network/GUI brokers, and bounded application slots |

The repository currently targets the QEMU `virt-7.2` AArch64 machine: two CPUs, 4 KiB pages, and 256 MiB RAM. It is a research prototype, not a Linux/POSIX distribution, general-purpose desktop, or promise of arbitrary real-hardware and ELF compatibility.

## Quick start

The supported development path uses the pinned Rust toolchain and an OrbStack Docker context:

```sh
docker context show                         # expected: orbstack
make build
make test
make fsck
```

Run the serial shell:

```sh
make run
```

Run the GUI profile. The QEMU command prints the VNC/QMP endpoints; the default VNC port is `5900`.

```sh
make gui
```

Example Mica programs:

```text
mica -e 'print(40 + 2)'
mica --gui --timeout 86400s --allow gui.window /mica/gui-counter.mica
mica --gui --timeout 86400s --allow gui.window --allow net.browse \
  /.system/examples/mica/browser.mica -- https://example.com/
```

## Recorded acceptance evidence

The repository contains the detailed runbook and raw workflow results. The recorded acceptance matrix reports host tests of MFS recovery `22/22`, ABI `9/9`, GUI command stream `6/6`, GUI Desktop `9/9`, kernel boundaries `11/11`, and Mica `24/24` — `81/81` host tests in total — together with QEMU smoke, SSH, GUI, TLS, power-cut, and MFS1 fault/recovery gates.

Those numbers are historical evidence retained in the repository. This GitHub publishing pass intentionally does not compile or rerun the system; see [`docs/runbook.md`](docs/runbook.md) and [`.workflow/microsystem-kernel/results/tests.md`](.workflow/microsystem-kernel/results/tests.md) for provenance and limits.

## Repository map

```text
crates/kernel/     EL1 kernel and AArch64 boot/runtime mechanisms
crates/mfs1/       MFS1 filesystem implementation
crates/mica/       Mica language, VM, permissions, and standard library
crates/gui/        GUI ABI and command-stream types
services/          Static EL0 services and example applications
assets/            CA and font inputs used by the guest image
docs/              Architecture, ABI, GUI, MFS1, network, SSH, and runbook docs
scripts/           OrbStack/QEMU build and acceptance helpers
xtask/             Image generation and QEMU orchestration
```

## Read next

- [Architecture](docs/architecture.md) — boot, address spaces, IPC, capabilities, storage, and GUI boundaries.
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

The Rust workspace declares `MIT OR Apache-2.0`. See the individual source headers and package metadata for the applicable license terms.
