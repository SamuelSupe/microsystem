# ABI reference

This page describes the current `crates/abi` contract. Numbers are part of the
wire ABI; changing one requires updating both the service and its client.

## Common message and rights contract

`ABI_VERSION = 1`. Every `Message` contains six `u64` words and four
capability handles. `flags` bits 12–15 are the four-cap move mask
(`message_cap_move(index)`). A moved capability is removed from the sender only
after all destination slots and rights have been pre-validated; failure rolls
back the whole call. Handles encode a 16-bit slot and 16-bit generation.

The rights bits are `READ`, `WRITE`, `EXECUTE`, `GRANT`, `MANAGE`, `MAP` and
`ACK`. Capability derivation is stable across task tables, so revoke invalidates
stale descendants rather than relying on a slot number alone.

## Protocol numbers

| Protocol | Number | Main owner |
| --- | ---: | --- |
| `CONSOLE` | 1 | PL011 console service |
| `BLOCK` | 2 | VirtIO block service |
| `FILESYSTEM` | 3 | MFS1 service |
| `PROCESS` | 4 | init/procman |
| `TIME` | 5 | init/proc/time endpoint |
| `GUI` | 6 | windowd, built-in clients and Mica GUI |
| `SSH` | 7 | reserved protocol namespace |
| `SCRIPT` | **8** | Mica script broker |
| `NETWORK` | **9** | netd and its clients |
| `DATABASE` | **10** | resident EL0 `db` service |

The script, network and database numbers are intentionally distinct: Mica sends
process operations through `SCRIPT`, data-plane DNS/TCP/UDP operations go
through `NETWORK` to netd, and SQL requests go through `DATABASE` to `db`.

## Syscalls and process operations

The stable syscall numbers relevant to this release are:

```text
ThreadStartEx   30   structured launch from init
ClockRealtime   31   PL031-backed Unix seconds
```

The earlier syscall range includes `ClockNow=18`, `ThreadStart=13`,
`ThreadStatus=20`, `ThreadKill=21`, `SystemStats=24`, `NetReceive=27`,
`NetSend=28`, and `RandomFill=29`. `NetReceive`/`NetSend` are capability- and
task-gated; only netd may use the raw network-device path.

`process::Operation` is:

```text
Spawn=1  Wait=2  List=3  Kill=4  MemoryPool=5  SpawnScript=6
```

`SpawnScript=6` allocates a 16-page script session region and prepares the
Mica launch. `ThreadStartEx=30` accepts both the original `ThreadLaunchV1`
(six caps) and `ThreadLaunchV2` (the same six entries plus command frame, event
frame and dedicated GUI endpoint). Init is the only caller; the accepted
profile is `THREAD_PROFILE_MICA` and the program must be the bootfs `mica`
image. V2 is transactional: validation, capability installation and rollback
cover all nine entries. Ordinary application slots begin at
`process::FIRST_APPLICATION_PID = 14` and there are eight slots.

`time::Operation` is `Sleep=1`, `Uptime=2`, `Realtime=3`. The TIME endpoint
currently serves Sleep/Uptime; `Realtime=3` is represented in the ABI and
Mica/TLS obtain the trusted wall clock directly with syscall 31. The PL031
driver returns seconds only when the value is at least the kernel's minimum
valid Unix time; otherwise `ClockRealtime` returns `NotSupported`.

## Filesystem and script sessions

Filesystem operations are `Stat=1`, `List=2`, `Read=3`, `Write=4`, `Mkdir=5`,
`Sync=6`, `Open=7`, `Fsync=8`, `Rename=9`, `Unlink=10`, `Close=11`,
`WriteAtomic=12`, `ReadRange=13`, `Stats=14`, `WriteRange=15`, and
`Replace=16`. Script registration uses `0x100` and unregistration `0x101`;
requests carry `MICAFS01`, and the exact trusted CA read carries `MICACA01`.

`script::SessionHeaderV1` is version 1 with magic `MICA`:

```text
region: 64 KiB / 16 pages at VA 0x0058_0000
inline source: 16 KiB (file mode carries only manifest prefix/path)
policy area: 3,840 bytes
stdin/argv ring: 8 KiB
stdout and broker scratch: 32 KiB
modes: EVAL=1, FILE=2, REPL=3
events: INPUT=1, OUTPUT=2, EXIT=4, GUI=8
GUI flag: FLAG_GUI_SESSION=8 (file mode only)
```

The session region is per process. MFS and netd map it only while servicing a
token-authenticated request, then unmap it. The CA bundle is fetched from the
exact MFS path `/.system/certs/ca-bundle.derpack` with `MICACA01`; it is not a
shared GUI/FrameRegion data source.

## Database protocol 10

The database protocol is a deliberately small MicroSystem contract. It is
served by the resident EL0 `db` service through `DATABASE_ENDPOINT` (86/1),
with the shell as the current client. Requests use protocol `DATABASE=10` and
carry `SHARED_FILESYSTEM_FRAME` (25/1) in capability slot 0. The frame is a
single 4 KiB request/response area; request word 0 is the UTF-8 SQL byte length
and `Execute` accepts at most 4,096 bytes. `Ping=1` checks the endpoint and
`Execute=2` executes one statement. One optional trailing semicolon is allowed;
additional statements are rejected.

The supported SQL subset is:

```text
CREATE TABLE name (column INTEGER|TEXT|BOOL|BOOLEAN [PRIMARY KEY|NOT NULL], ...)
DROP TABLE name
INSERT INTO name [(column, ...)] VALUES (literal, ...)
SELECT *|column, ... FROM name [WHERE predicate [AND predicate ...]]
UPDATE name SET column=literal [, ...] [WHERE predicate [AND predicate ...]]
DELETE FROM name [WHERE predicate [AND predicate ...]]
```

Identifiers are ASCII letters/underscore followed by ASCII letters, digits or
underscore, with a 63-byte limit. Keywords are case-insensitive. Literals are
signed decimal integers, single-quoted text (a doubled quote escapes a quote),
`TRUE`, `FALSE` and `NULL`. Predicates support `=`, `!=`, `<>`, `<`, `<=`, `>`
and `>=`, plus `IS NULL` and `IS NOT NULL`; only conjunction with `AND` is
supported. A table may have at most one primary-key column; `PRIMARY KEY`
implies `NOT NULL`, and explicit `NOT NULL` is supported. The limits are 32
tables, 32 columns per table, 1,024 bytes per text value, 256 KiB per snapshot,
and 4 KiB for the complete response.

Joins, expressions, functions, `OR`, aggregates, ordering, grouping, limits,
offsets, subqueries, indexes, `ALTER TABLE`, foreign keys and explicit
transaction statements are outside this subset. This is not SQLite syntax or
API compatibility: the on-disk snapshot uses the `MSQLDB1\0` magic, version 1
and a CRC32C payload, not SQLite's file format.

### Request, response and tagged values

`DbResponseHeaderV1` is a 24-byte `repr(C)` header at the start of the shared
frame:

```text
magic=SQL1  version=1  kind  columns  rows  reserved
affected_rows  payload_bytes
```

`kind` is `Command=1`, `Rows=2` or `Error=3`. `reply.words[0]` is the total
response bytes and `reply.words[1]` is the affected-row or row count; the
status is in `reply.words[5]`. A command response has no payload and reports
`affected_rows`. An error response carries its UTF-8 message in the payload
and returns a non-OK status.

For a row response, the payload first contains one column descriptor per
column: `u8 name_bytes`, the UTF-8 name, then the `SqlType` byte (`1=Integer`,
`2=Text`, `3=Bool`). Values follow in row-major order, each beginning with a
tagged value byte:

| Tag | Encoding |
| ---: | --- |
| `0` | `Null`, no payload |
| `1` | `Integer`, signed little-endian `i64` |
| `2` | `Text`, little-endian `u16` byte length followed by UTF-8 bytes |
| `3` | `Bool`, one byte `0=false` or `1=true` |

The header and payload together must fit in the 4 KiB frame. A mutating
statement executes against a cloned database, encodes a complete snapshot, and
is committed only after MFS1 has written and fsynced
`/.system/db/main.db.tmp`, replaced it with `/.system/db/main.db`, and fsynced
the final path. `SELECT` is read-only. There is no multi-statement transaction
or SQLite transaction compatibility.

At startup, a missing snapshot creates an empty database, but a CRC failure or
unknown snapshot version returns `Corrupt` and does not reset the database.

## Boot capabilities used by Mica, netd, SSH and DB

The fixed boot handles are:

| Handle | Slot/gen | Object | Rights in client |
| --- | ---: | --- | --- |
| `NETWORK_DEVICE` | 58/1 | raw `NetworkDevice` | netd `READ|WRITE` only |
| `NETWORK_ENDPOINT` | 60/1 | netd endpoint | Mica/init `WRITE`; netd `READ` |
| `NETWORK_MEMORY_POOL` | 61/1 | netd pool | netd `MANAGE` |
| `SCRIPT_SESSION_REGION` | 63/1 | script region authority | init/Mica launch |
| `SCRIPT_IO_NOTIFICATION` | 64/1 | script notification | Mica `READ|WRITE` |
| `SCRIPT_FILESYSTEM_ENDPOINT` | 65/1 | script FS endpoint | broker wiring |
| `SCRIPT_NETWORK_ENDPOINT` | 66/1 | script net endpoint | broker wiring |
| `SCRIPT_RANDOM_SOURCE` | 67/1 | random source | Mica `READ` |
| `SCRIPT_SYSTEM_DATA` | 68/1 | system info | optional Mica `READ` |
| `SSH_NETWORK_ENDPOINT` | 69/1 | SSH↔netd endpoint | sshd `WRITE`, netd `READ` |
| `SCRIPT_BROKER_ENDPOINT` | 70/1 | init script broker | Mica process calls |
| `SSH_FILESYSTEM_FRAME` | 71/1 | SSH/MFS payload frame | MFS/sshd only |
| `SHARED_FILESYSTEM_FRAME` | 25/1 | MFS filesystem payload frame | MFS/shell/db |
| `DATABASE_FILESYSTEM_FRAME` | 87/1 | database filesystem payload frame | MFS/db only |
| `GUI_CONFIG_ENDPOINT` | 54/1 | init↔windowd client registry | init `WRITE`; windowd `READ` |
| `SCRIPT_GUI_COMMANDS` | 72/1 | 64 KiB GUI command FrameRegion | Mica GUI `READ|WRITE|MAP` |
| `SCRIPT_GUI_EVENTS` | 73/1 | 4 KiB GUI event Frame | Mica GUI `READ|WRITE|MAP` |
| `SCRIPT_GUI_ENDPOINT` | 74/1 | dedicated windowd endpoint | Mica GUI `WRITE` |
| `GUI_DYNAMIC_ENDPOINT_BASE` | 75..82 | windowd endpoint slots | one slot per dynamic client |
| `DATABASE_ENDPOINT` | 86/1 | resident `db` endpoint | shell `WRITE`; db `READ` |

The Mica task itself receives the session region, notification,
`FILESYSTEM_ENDPOINT` (write), `NETWORK_ENDPOINT` (write), `RANDOM_SOURCE`
(read), and an optional `SYSTEM_INFO` (read). A GUI session additionally
receives the command frame, event frame and dedicated endpoint above. No Mica
session receives `NETWORK_DEVICE`, MMIO, block, IRQ, framebuffer, GPU BAR or
input-device capabilities. MFS/Shell payload and SSH payload frames are
separate from block DMA frames.

## GUI protocol 6

`gui::VERSION=1` fixes a 1024×768 XRGB8888 desktop. A command FrameRegion is
64 KiB and an event Frame is 4 KiB. `PresentHeaderV1` carries a sequence,
command length, payload hash and up to 16 damage rectangles. Windowd validates
UTF-8, bounds, clip, command count (4,096), text bytes (48 KiB) and all payload
bytes before atomically replacing the current display list; an invalid request
returns `Invalid` without changing the previous list. Validated damage
rectangles bound redraws, merged at up to 60 Hz. The six command kinds are
`Clear`, `FillRect`, `StrokeRect`, `Text`,
`Icon` and `SetClip`.

The event ring has `PointerMove`, `PointerButton`, `PointerWheel`, `Key`,
`TextInput`, `Focus`, `Configure`, `Expose` and `CloseRequested`. Pointer moves
may be coalesced when full; button, key, configure, expose and close events use
bounded backpressure. `script::EVENT_GUI` signals the ring. The GUI config
endpoint (54/1) is write-owned by init and read-owned by windowd; each dynamic
client gets one identity-bound endpoint, and the desktop supports eight dynamic
clients in addition to the three built-in windows (11 total).

The partial-Present contract is bounded through user-rt, kernel, GPU and windowd;
pointer movement contributes damage rectangles and input is batched at 16. The
final marker is `partial-present=true rect=1024x720+0,48`; the renderer marker
is `damage-merge=true local-window-damage=true command-cull=true glyph-cache=2way
input-batch=16`. The network driver marker records
`virtio-net ... rx-buffers=2`. These changes do not alter the ABI or permissions.

## Topology and address-space numbers

The generated bootfs has 25 entries: 24 static ELF files and `etc/services`.
`SERVICE_COUNT` is 13 and the service order is:

```text
init console block mfs shell devmgr windowd terminal files monitor sshd netd db
```

The serial manifest starts `devmgr,console,block,mfs,db,shell,netd,sshd` and
the serial readiness mask reports `ready=8/8 online=8/8`. A detected
VirtIO-GPU/input device additionally starts `windowd,terminal,files,monitor`,
enables all 13 service slots and reports `ready=13/13 online=13/13`. Service
address spaces use ASIDs `0x20..0x2c`. Dynamic application slots begin at PID
14 and are separate from these service slots.

The kernel heap is 128 MiB. Each ordinary task has a 64 KiB stack and 1 MiB
user heap. MFS has the explicitly provisioned 32 MiB large-heap backing;
windowd may map the large-window VA for GUI `FrameRegion` mappings without
receiving that heap backing. The Mica/task image allocation is `0xd0000`
bytes.

## Compatibility boundary

The ABI has no stable contract for dynamic linking, arbitrary on-disk ELF
execution, unrestricted network sockets, or low-level TLS. In particular,
`network::Operation::TlsConnect=12` and Mica `net.tls_connect` remain
`NotSupported`; trusted HTTPS is implemented by the Mica runtime above netd
TCP. See [docs/mica.md](mica.md) for language, policy, TLS and command details.
