# Accounts, roles and SSH keys

Init owns up to eight accounts and publishes an atomic readonly authorization
snapshot to core services. Apps cannot map or change it. IPC caller PIDs come from
the kernel, and SSH cookies are bound to SSHD, user epoch and a one-hour expiry.
Account changes are checksummed and atomically persisted in `/.system/accounts`;
credentials and current desktop identity are never restored from disk. Invalid
account files fail closed with a console diagnostic.

The physical serial console is the trusted root administration path. The initial
desktop also runs as root; switching a desktop user requires an administrator.
Logout switches to the readonly guest and restarts the GUI, clearing its clipboard,
composition, command history and windows. This profile uses public-key SSH login;
there is no password login or encrypted home directory.

## Commands

The serial shell, Terminal and SSH accept the account commands below. Desktop logout is local-only:

```text
whoami
user list
user whoami
user add alice operator
user role alice reader
user disable alice
user enable alice
user keys alice
user key-add alice ED25519_PUBLIC_KEY_HEX
user key-remove alice 0
user switch alice
user logout
user host-key rotate
user audit
```

Roles are `admin`, `operator`, `reader`. Administrators manage accounts, network
configuration, services, volume mounts, native installation/rollback/direct ELF
execution and SQL. Operators may run installed native apps and authorized Mica
scripts. Readers cannot launch processes or modify files. Native app capabilities
and memory quotas still come from its installation manifest. Killing another
user's process requires an administrator. A user can inspect their own public
keys; changing keys requires an administrator. Each account has up to four
Ed25519 keys. Key removal, role changes and disabling an account invalidate all
its credentials and terminate its applications. SSHD checks revocation while
waiting for channel I/O as well as on new commands. SSH handshake completion is bounded to 30 seconds; accepted-channel read inactivity is bounded to five minutes. Restarting SSHD resets its old listener connections.

A key is 64 hexadecimal characters containing the raw 32-byte public key, not an
OpenSSH base64 line. For an OpenSSH `.pub` file:

```sh
python3 -c 'import base64,sys; print(base64.b64decode(open(sys.argv[1]).read().split()[1])[-32:].hex())' ~/.ssh/id_ed25519.pub
```

On first boot, `micro` is an operator with the locally generated build key
`build/ssh/id_ed25519`; root is an administrator with no SSH key; guest is a
permanent readonly account. New users receive monotonically allocated UIDs and
private mode-700 home directories. `/data` is a shared mode-777 workspace; use a
private home for confidential files. `/data` has no sticky-directory policy.
User disable is the supported account removal path; UIDs are not recycled.

## Filesystem and recovery

MFS checks owner/group/other read/write bits and search permission on every
ancestor after VFS path normalization, including mounted volumes. File creation
uses the caller's UID/GID. Read/write/fsync descriptors retain caller ownership;
permissions are checked again after chmod or role changes. Script manifest scope
is an additional boundary, and the registered UID comes only from init. chmod
requires file ownership or an administrator; chown and mounts require an
administrator. Atomic script replacement preserves an existing file's owner and
mode. Core storage/network/database maintenance runs as system authority.

A unique host signing seed is generated from the guest hardware RNG on first boot and persisted privately at `/.system/ssh/host.key` and is
loaded on each SSH connection. Rotation closes SSHD and publishes a new key;
SSHD prints the raw Ed25519 public key in hexadecimal on the trusted serial console at startup. Compare that key with the final 32 bytes of the client’s OpenSSH host-key blob before accepting its new fingerprint. It survives service restart
and reboot. Private account/key files and pending files are mode 600 before
sensitive bytes are written. There is no disk encryption or hardware keystore.

Account administration, accepted/rejected SSH authentication and host-key rotation
are recorded with realtime, UID, action and account name in `/.system/audit.log`.
No private keys or credential cookies are logged. Logs rotate into
`audit.previous` after about 60 KiB. This is local bounded audit storage, not a
remote or tamper-proof audit service. An audit-write failure is reported; a
committed revocation is still published so an old key cannot remain authorized.

## Qualification

`MICROSYSTEM_RECOVERY_IDENTITY_ONLY=1` runs the existing isolated-disk QEMU gate
with account creation, role/private-file denial, disable/enable, key removal and
replacement, host-key rotation, MFS recovery and two cold boots. Host tests cover
UID uniqueness, epoch/peer/expiry revocation, atomic publication and ancestor
permissions. The implementation ledger records which gate version has passed.
