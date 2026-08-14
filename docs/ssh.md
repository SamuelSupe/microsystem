# SSH service

`sshd` is an EL0 service on port 22, reached through netd's
`SSH_NETWORK_ENDPOINT` (protocol 9 operations 14–20). It does not receive the
raw network device. The service uses two bounded listener sockets and a
per-connection payload frame; the final readiness marker is:

```text
[ssh] sshd ready address=10.0.2.15 port=22 auth=publickey user=micro
```

## Authentication and connection

The image build creates deterministic test material in `build/ssh/`:

- `build/ssh/id_ed25519` is the client private key;
- `build/ssh-authorized-key.bin` is the only accepted Ed25519 public key;
- `build/ssh-host-key.bin` is the server Ed25519 host key.

Only username `micro` with that public key is accepted. The SSH transport
identifies as `SSH-2.0-MicroSystem_0.1`. Wrong-key and no-key attempts are
expected to fail with exit 255 in the QEMU gate.

Start QEMU with `make run`/`make gui`, then connect with:

```sh
ssh -F /dev/null -T -p "${SSH_PORT:-2222}" \
  -i build/ssh/id_ed25519 -o IdentitiesOnly=yes \
  -o StrictHostKeyChecking=accept-new micro@127.0.0.1
```

The standalone `scripts/ssh-qemu.sh` gate uses host port 2223 by default and
checks its own forwarded connection; it should not be confused with the
Makefile's interactive port 2222 default.

## Commands

The SSH exec and interactive shell command set is intentionally small:

```text
help uptime ps clear echo mica exit
```

`mica` forms are:

```sh
ssh ... micro@127.0.0.1 'mica -e "print(40 + 2)"'
ssh ... micro@127.0.0.1 'mica /data/example.mica -- arg1 arg2'
ssh ... micro@127.0.0.1 'mica --gui --timeout 86400s --allow gui.window /mica/gui-counter.mica'
```

An interactive SSH shell (`ssh ...` without `-T` command mode) provides
`micro>`, line editing, the same commands, and `mica` to start a Mica REPL.
Non-interactive `mica` without a file or `-e` is rejected because a REPL needs
the SSH shell. Mica sessions are allocated with `SpawnScript=6` and launched
with `ThreadStartEx=30`; they are not ordinary `run` applications. GUI Mica
must name a file (GUI `-e` and GUI REPL are rejected) and receives a
ThreadLaunchV2 command frame, event frame and dedicated endpoint.

Desktop icon launches are local GUI actions, not an SSH command extension. The
fixed Reader/Editor icons send only an application id to init, which selects
the bundled path and policy; SSH-launched GUI scripts continue to use the
three-way manifest/`--allow`/SSH-policy intersection. The icon endpoint cannot
be used to request an arbitrary script or widen SSH permissions.

## SSH Mica policy

The maximum policy is read from the exact MFS path
`/.system/ssh/mica-policy` through the dedicated `SSH_FILESYSTEM_FRAME`, not
through block DMA or the generic GUI frame. The checked-in policy is:

```text
fs.read:/data
fs.write:/data
proc.list
proc.spawn:counter
sys.stats
random
gui.window
```

Command-line `--allow` rules are parsed, then intersected with this maximum
before the Mica session is launched. A file script's own `--!allow` manifest
is intersected again by the Mica runtime. This preserves path-prefix,
exact-host/port and named-program boundaries; SSH cannot grant raw devices,
arbitrary process kill, or unrestricted filesystem access. GUI access is still
the three-way intersection of this policy, the command-line `--allow` rules and
the script's `--!allow gui.window` manifest.

## Acceptance and boundaries

The final SSH stage passed uptime, Mica eval/REPL/file/policy/disconnect,
wrong-key/no-key and clean shutdown checks. The dynamic GUI Counter stage also
passed over SSH: registration, isolated Present, button/ASCII/Unicode input,
window actions, close and resource reclamation, with SSH status 0. Additional
GUI sessions were launched through the persistent serial shell while the first
remained on SSH; this proves GUI-session concurrency, not SSH concurrency. The
service remains bounded to two connections and the command subset above. It does
not provide SCP/SFTP,
port-forwarding, agent forwarding, password authentication, arbitrary shell
programs or a general terminal multiplexer. See [docs/mica.md](mica.md) for
the language/VM and trusted HTTPS contract.
