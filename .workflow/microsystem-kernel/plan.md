# MicroSystem implementation workflow

Goal: implement and validate the requested AArch64 dual-core capability microkernel on OrbStack/QEMU.

Success criteria:

- `make build`, `make run`, `make test`, and `make fsck` are backed by the OrbStack Docker context.
- QEMU boots two CPUs and reaches an interactive shell.
- Kernel capability/IPC/memory/SMP boundaries and the MFS1 persistence contract have executable validation.
- User-space service boundaries and current hardware limitations are represented honestly in code and documentation.

Constraints:

- Keep implementation direct and modular; avoid unrelated abstraction.
- Test execution and documentation production are owned by Luna MAX subagents.
- Do not claim a hardware boundary without QEMU/OrbStack evidence.

Milestones:

1. Reproducible workspace, Docker toolchain, AArch64 boot and UART.
2. MMU, exception handling, frame allocator, SMP and scheduling core.
3. Capability/IPC ABI, EL0 entry, ELF loading and service model.
4. PCI/SMMUv3/VirtIO service boundary and block I/O.
5. MFS1 format, tools, recovery, buffered persistence and GC.
6. Shell integration, tests, docs and final OrbStack acceptance.
7. Mica GUI retained-widget applications: dynamic per-session GUI capabilities,
   validated display lists, event rings, one-window script lifecycle, widget
   library, runtime font fallback, QMP interaction gate and unified regression.
