# Orchestration

- Main agent owns production code, workspace integration and build diagnostics.
- Luna MAX docs packet owns `README.md` and `docs/` only.
- Luna MAX test packet owns test-only files and executes all tests in OrbStack.
- Concurrent workers must preserve others' edits and adapt to shared interfaces.
- Milestones are integrated in dependency order; test failures return to the owning production module.
- Final completion requires build evidence, QEMU serial evidence, MFS1 fsck evidence and no unresolved workflow packet.

## Mica GUI milestone

- Main agent owns ABI, kernel capability installation, init/windowd/Mica runtime,
  service integration and production build fixes.
- Luna MAX test packet owns `crates/*/tests`, `scripts/*qemu.sh` test harness
  changes and `.workflow/microsystem-kernel/results/tests.md`; it must explain
  the concrete regression protected by each test and run only on OrbStack.
- Luna MAX docs packet owns `README.md`, `docs/` and
  `.workflow/microsystem-kernel/results/docs.md` after production interfaces and
  serial markers stabilize.
- Existing Terminal, Files and Monitor clients remain compatible while dynamic
  Mica clients use isolated command/event memory and a per-session endpoint.
- Integration order is ABI/layout, capability lifecycle, windowd protocol,
  Mica retained widgets, targeted tests, QEMU gate, unified test/fsck, docs.
