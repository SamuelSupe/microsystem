# MFS1

MFS1 is the project filesystem served by the resident EL0 `mfs` service over
the EL0 block service. The format remains version 1. The final image is a
64 MiB test disk; the acceptance `make fsck` result is:

```text
MFS1 clean generation=2544 transactions=2381 entries=24 used_blocks=11898
```

The tuple is from `target/unified-mfs1-resilience-fsck.log`; the corresponding
unified acceptance log is `target/unified-mfs1-resilience-final.log`.

## On-disk contract

- block size is 4 KiB and a segment is 256 blocks;
- feature bit 0 is segment layout and bit 1 is segment directory (`0x3`); any
  unknown feature bit is rejected;
- metadata has two superblock copies. Each copy has an independent CRC32C,
  generation/head validation and a complete replay check; the newest
  replay-verified copy is selected;
- records also carry CRC32C over their complete encoded record blocks;
- directory entries are ordered `(segment index, used blocks)` records, with up
  to 128 complete 1 MiB segments represented;
- records do not cross segment boundaries; `Padding=6` fills a segment tail;
- old feature-0/feature-1 images migrate to the directory layout on the first
  write, while the mount path can replay them read-only;
- operations are ordered through record/data/superblock stages and `fsync`
  commits the named path before returning.

The dual superblocks are metadata redundancy, not data-block error correction.
`fsck` independently audits both checksum-valid copies and their full replays.
If only one copy is healthy it reports degraded metadata and exits non-zero;
the payload and records are not silently treated as repaired.

`fsync` failure returns `Err` without losing or reordering the dirty mutation
list; retry replays the original sequence. `WriteAtomic` stages the temporary
file, then uses one `replace_file` transaction containing `Remove(target)` and
`Rename(temp,target)`, followed by a durable final fsync. At each of the four
fault points (write/data flush/superblock write/superblock flush), remount sees
either `target=old,temp=new` or `target=new,temp=absent`; retry ends at
`target=new,temp=absent`. Renaming a directory into its own descendant is
rejected before any dirty mutation is queued.

The materialized-paths review keeps single-path `view_before` operations
(`read`, metadata, `put`, `mkdir` and `rename`) from cloning the whole
`BTreeMap` or file bodies. `list` materializes only `BTreeSet` paths, and
`stat` reads `Metadata` without copying file data. Chained directory rename,
old-name recreation, nested remove and a second rename are resolved through
the same ordered view and remain correct after sync/remount. These bounded
changes do not alter the MFS1 transaction, security or resource limits.

The public FileService operation numbers are `Stat=1`, `List=2`, `Read=3`,
`Write=4`, `Mkdir=5`, `Sync=6`, `Open=7`, `Fsync=8`, `Rename=9`, `Unlink=10`,
`Close=11`, `WriteAtomic=12`, `ReadRange=13`. Script sessions have eight
session slots and sixteen file-descriptor slots per session. Script requests
are token-authenticated and capped at 32 KiB per broker transfer.

## Crash and I/O campaign

The host resilience campaign executes 10,024 seeded-randomized, deterministic
cases from seed `0x4d46533143524153`. It uses a sparse volatile/durable block
model: writes
first affect volatile state, flush copies the selected state to durable state,
and a crash discards all remaining volatile writes. Every result records the
seed, case and event, so a failure is directly reproducible rather than being
an aggregate-only random result.

The four injected classes and triggered-case counts are:

| fault class | cases |
| --- | ---: |
| write I/O error | 1,668 |
| short write | 1,711 |
| 512-byte torn sector | 1,626 |
| flush I/O error | 5,019 |

Short writes and torn sectors are injected at the block-device write boundary;
the latter applies only a subset of 512-byte sectors. Read errors are also
injected and must be observable. The campaign checks that remount exposes only
a complete old or new snapshot, never a mixed record/data state. This is a
deterministic host fault model; it is not a claim that QEMU emulates every
physical disk failure mode.

## Offline metadata verification and repair

`mfsctl fsck IMAGE` runs the dual-copy checksum and full-replay audit. A
degraded `1/2` result is an error and points to `mfsctl repair IMAGE`.

`mfsctl repair IMAGE` is offline and intentionally limited to one action: copy
the only fully replay-verified superblock over one checksum-invalid stale
mirror, flush it, then re-audit both copies. Before writing, the scanner covers
the whole image and rejects any later `Commit` or ambiguous record. It also
rejects latest-superblock corruption, double corruption, read I/O errors and
record ambiguity; those paths do not write anything. A healthy `2/2` image is
left unchanged.

The CLI repair gate preserves payload bytes. Active-latest corruption and
double-corruption rejection both keep the original payload SHA unchanged. The
repair operation is therefore metadata mirror healing only; it is not general
data recovery and does not reconstruct corrupted data blocks or ambiguous
records.

## GC and recovery

GC starts below 15% free space and runs until at least 25% free. The directory
GC chooses a low live-block/capacity victim, writes a new candidate directory
and seals it with the dual-superblock protocol. Zero-live components return
without a full copy. Normal service operation uses a 1 s writeback interval
and online GC; block DMA and filesystem payload frames are physically
separate.

The final `make test` sequence passed normal migration, powercut token
recovery, the transaction/GC QEMU cuts, and the 10,024-case host campaign.
Those QEMU KILL stages are deliberate fault-injection fixtures; they do not
claim coverage of real hardware power loss, every sealed GC stage, arbitrary
extent interleavings or unbounded directory sizes. The final evidence is in
`target/unified-mfs1-resilience-final.log` and
`target/unified-mfs1-resilience-fsck.log`.

## Shell-visible chain

The resident shell exercises `mkdir`, `write`, `open`, `read`, `fsync`, `sync`,
`stat`, `readdir`, `rename`, `unlink` and `close`, then reads the persistent
`/boot-proof` value after restart. Mica's `fs.*` wrappers use the same endpoint
with path permissions; the TLS trust bundle is read through the exact MFS path
`/.system/certs/ca-bundle.derpack` using the trusted-read marker.

MFS remains a bounded research format, not a POSIX filesystem: there is no
permissions/ownership model, journaling API beyond the fixed transaction
stages, arbitrary disk expansion, or general concurrent client contract.
