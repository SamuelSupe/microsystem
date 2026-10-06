#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;
use core::fmt;

pub const BLOCK_SIZE: usize = 4096;
pub const SEGMENT_BLOCKS: u64 = 256;
pub const FORMAT_VERSION: u32 = 1;
pub const GC_LOW_WATER_PERCENT: u64 = 15;
pub const GC_HIGH_WATER_PERCENT: u64 = 25;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CommitStage {
    TransactionRecordsWritten,
    TransactionDataFlushed,
    TransactionSuperblockWritten,
    TransactionSuperblockFlushed,
    GcRecordsWritten,
    GcDataFlushed,
    GcCandidateSuperblockWritten,
    GcCandidateSuperblockFlushed,
    GcSealedSuperblockWritten,
    GcSealedSuperblockFlushed,
}

const SUPER_MAGIC: &[u8; 8] = b"MFS1SB\0\0";
const RECORD_MAGIC: u32 = 0x3152_464d;
const SUPERBLOCK_COPIES: u64 = 2;
const SUPERBLOCK_SIZE: usize = 64;
const RECORD_HEADER_SIZE: usize = 64;
const FEATURE_SEGMENT_LAYOUT: u32 = 1 << 0;
const FEATURE_SEGMENT_DIRECTORY: u32 = 1 << 1;
const FEATURE_RANGE_WRITES: u32 = 1 << 2;
const FEATURE_ATTRIBUTES: u32 = 1 << 3;
const KNOWN_FEATURES: u32 =
    FEATURE_SEGMENT_LAYOUT | FEATURE_SEGMENT_DIRECTORY | FEATURE_RANGE_WRITES | FEATURE_ATTRIBUTES;
const MAX_RANGE_FILE_BYTES: usize = 64 * 1024 * 1024;
const MAX_SEGMENTS: usize = 128;
const MAX_PATH_BYTES: usize = 255;
const MAX_MATERIALIZED_PATHS: usize = 4096;
const DIRECTORY_OFFSET: usize = 64;
const DIRECTORY_ENTRY_OFFSET: usize = DIRECTORY_OFFSET + 4;
const DIRECTORY_CRC_OFFSET: usize = BLOCK_SIZE - 4;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct SegmentRef {
    index: u16,
    used: u16,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct SegmentDirectory {
    len: u16,
    entries: [SegmentRef; MAX_SEGMENTS],
}

impl SegmentDirectory {
    const EMPTY: Self = Self {
        len: 0,
        entries: [SegmentRef { index: 0, used: 0 }; MAX_SEGMENTS],
    };

    fn as_slice(&self) -> &[SegmentRef] {
        &self.entries[..self.len as usize]
    }

    fn as_mut_slice(&mut self) -> &mut [SegmentRef] {
        &mut self.entries[..self.len as usize]
    }

    fn push(&mut self, entry: SegmentRef) -> Result<(), Error> {
        if self.len as usize == MAX_SEGMENTS {
            return Err(Error::NoSpace);
        }
        self.entries[self.len as usize] = entry;
        self.len += 1;
        Ok(())
    }

    fn contains(&self, index: u16) -> bool {
        self.as_slice().iter().any(|entry| entry.index == index)
    }
}

pub trait BlockDevice {
    fn block_count(&self) -> u64;
    fn read_block(&mut self, block: u64, out: &mut [u8; BLOCK_SIZE]) -> Result<(), Error>;
    fn write_block(&mut self, block: u64, data: &[u8; BLOCK_SIZE]) -> Result<(), Error>;
    fn flush(&mut self) -> Result<(), Error>;
    fn commit_stage(&mut self, _stage: CommitStage) {}
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    Io,
    Invalid,
    Corrupt,
    NotFound,
    AlreadyExists,
    NotDirectory,
    IsDirectory,
    Busy,
    NoSpace,
    NameTooLong,
    Utf8,
    CrossDevice,
    ReadOnly,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
/// Independent checksum and replay results for both metadata roots.
pub struct MetadataReport {
    pub selected_superblock: u8,
    pub checksum_valid_superblocks: u8,
    pub replay_verified_superblocks: u8,
}

impl MetadataReport {
    pub fn is_healthy(self) -> bool {
        self.checksum_valid_superblocks == SUPERBLOCK_COPIES as u8
            && self.replay_verified_superblocks == SUPERBLOCK_COPIES as u8
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
/// Result of an offline, bounded metadata repair attempt.
pub struct RepairReport {
    pub metadata: MetadataReport,
    pub repaired_superblocks: u8,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}

#[cfg(feature = "std")]
impl std::error::Error for Error {}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Node {
    File(Vec<u8>, Attributes),
    Directory(Attributes),
}

/// Ownership, permission bits and Unix timestamps (seconds). Reads use noatime;
/// a missing realtime source is represented by zero, rather than boot uptime.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Attributes {
    pub uid: u32,
    pub gid: u32,
    pub mode: u32,
    pub created: u64,
    pub modified: u64,
    pub accessed: u64,
    pub changed: u64,
}

impl Attributes {
    fn new(directory: bool, time: u64) -> Self {
        Self {
            uid: 0,
            gid: 0,
            mode: if directory { 0o755 } else { 0o644 },
            created: time,
            modified: time,
            accessed: time,
            changed: time,
        }
    }

    fn encode(self) -> Vec<u8> {
        let mut data = vec![0; 48];
        put_u32(&mut data, 0, self.uid);
        put_u32(&mut data, 4, self.gid);
        put_u32(&mut data, 8, self.mode);
        for (offset, time) in [
            (16, self.created),
            (24, self.modified),
            (32, self.accessed),
            (40, self.changed),
        ] {
            put_u64(&mut data, offset, time);
        }
        data
    }

    fn decode(data: &[u8]) -> Result<Self, Error> {
        if data.len() != 48 || get_u32(data, 8) > 0o777 || get_u32(data, 12) != 0 {
            return Err(Error::Corrupt);
        }
        Ok(Self {
            uid: get_u32(data, 0),
            gid: get_u32(data, 4),
            mode: get_u32(data, 8),
            created: get_u64(data, 16),
            modified: get_u64(data, 24),
            accessed: get_u64(data, 32),
            changed: get_u64(data, 40),
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Metadata {
    File { bytes: usize },
    Directory,
}

#[derive(Clone, Copy)]
enum NodeView<'a> {
    File(&'a [u8]),
    Directory,
}

impl Node {
    pub fn attributes(&self) -> Attributes {
        match self {
            Self::File(_, attributes) | Self::Directory(attributes) => *attributes,
        }
    }
    pub fn is_dir(&self) -> bool {
        matches!(self, Self::Directory(_))
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Stats {
    pub generation: u64,
    pub transaction: u64,
    pub total_blocks: u64,
    pub active_arena: u8,
    pub checkpoint_block: u64,
    pub segments_since_checkpoint: u64,
    pub used_blocks: u64,
    pub free_blocks: u64,
    pub entries: usize,
    pub segment_directory: bool,
    pub active_segments: u16,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Superblock {
    features: u32,
    generation: u64,
    transaction: u64,
    total_blocks: u64,
    arena: u8,
    checkpoint: u64,
    log_head: u64,
    directory: SegmentDirectory,
}

impl Superblock {
    fn encode(self) -> [u8; BLOCK_SIZE] {
        let mut out = [0u8; BLOCK_SIZE];
        out[0..8].copy_from_slice(SUPER_MAGIC);
        put_u32(&mut out, 8, FORMAT_VERSION);
        put_u32(&mut out, 12, self.features);
        put_u64(&mut out, 16, self.generation);
        put_u64(&mut out, 24, self.transaction);
        put_u64(&mut out, 32, self.total_blocks);
        out[40] = self.arena;
        put_u64(&mut out, 48, self.log_head);
        let checkpoint_offset = self
            .checkpoint
            .saturating_sub(arena_start(self.total_blocks, self.arena));
        put_u32(&mut out, 56, checkpoint_offset as u32);
        if self.features & FEATURE_SEGMENT_DIRECTORY != 0 {
            put_u16(&mut out, DIRECTORY_OFFSET, self.directory.len);
            for (slot, entry) in self.directory.as_slice().iter().enumerate() {
                let offset = DIRECTORY_ENTRY_OFFSET + slot * 4;
                put_u16(&mut out, offset, entry.index);
                put_u16(&mut out, offset + 2, entry.used);
            }
            let crc = crc32c(&out[..DIRECTORY_CRC_OFFSET]);
            put_u32(&mut out, DIRECTORY_CRC_OFFSET, crc);
        } else {
            let crc = crc32c(&out[..SUPERBLOCK_SIZE - 4]);
            put_u32(&mut out, SUPERBLOCK_SIZE - 4, crc);
        }
        out
    }

    fn decode(input: &[u8; BLOCK_SIZE]) -> Result<Self, Error> {
        if &input[0..8] != SUPER_MAGIC || get_u32(input, 8) != FORMAT_VERSION {
            return Err(Error::Corrupt);
        }
        let features = get_u32(input, 12);
        if features & !KNOWN_FEATURES != 0
            || features & FEATURE_SEGMENT_DIRECTORY != 0 && features & FEATURE_SEGMENT_LAYOUT == 0
        {
            return Err(Error::Corrupt);
        }
        let crc_offset = if features & FEATURE_SEGMENT_DIRECTORY != 0 {
            DIRECTORY_CRC_OFFSET
        } else {
            SUPERBLOCK_SIZE - 4
        };
        let expected = get_u32(input, crc_offset);
        if crc32c(&input[..crc_offset]) != expected {
            return Err(Error::Corrupt);
        }
        let arena = input[40];
        if arena > 1 {
            return Err(Error::Corrupt);
        }
        let total_blocks = get_u64(input, 32);
        if total_blocks < 2 * SEGMENT_BLOCKS + SUPERBLOCK_COPIES
            || arena_len(total_blocks) > u32::MAX as u64
        {
            return Err(Error::Corrupt);
        }
        let checkpoint = arena_start(total_blocks, arena) + get_u32(input, 56) as u64;
        let mut directory = SegmentDirectory::EMPTY;
        if features & FEATURE_SEGMENT_DIRECTORY != 0 {
            let count = get_u16(input, DIRECTORY_OFFSET) as usize;
            let available = segment_count(total_blocks)?;
            if count > MAX_SEGMENTS || count > available as usize {
                return Err(Error::Corrupt);
            }
            for slot in 0..count {
                let offset = DIRECTORY_ENTRY_OFFSET + slot * 4;
                let entry = SegmentRef {
                    index: get_u16(input, offset),
                    used: get_u16(input, offset + 2),
                };
                if entry.index >= available
                    || entry.used == 0
                    || entry.used as u64 > SEGMENT_BLOCKS
                    || directory.contains(entry.index)
                {
                    return Err(Error::Corrupt);
                }
                directory.push(entry).map_err(|_| Error::Corrupt)?;
            }
        }
        Ok(Self {
            features,
            generation: get_u64(input, 16),
            transaction: get_u64(input, 24),
            total_blocks,
            arena,
            checkpoint,
            log_head: get_u64(input, 48),
            directory,
        })
    }
}

#[derive(Clone, Debug)]
enum Mutation {
    Put(String, Vec<u8>),
    Patch(String, usize, Vec<u8>),
    Attributes(String, Attributes),
    Mkdir(String),
    Remove(String),
    Rename(String, String),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
enum RecordKind {
    Put = 1,
    Mkdir = 2,
    Remove = 3,
    Rename = 4,
    Commit = 5,
    Padding = 6,
    Patch = 7,
    Attributes = 8,
}

#[derive(Clone, Debug)]
struct Record {
    kind: RecordKind,
    txid: u64,
    generation: u64,
    object: u64,
    logical_offset: u64,
    path: String,
    secondary: String,
    data: Vec<u8>,
}

pub struct FileSystem<D: BlockDevice> {
    device: D,
    superblock: Superblock,
    protected: [SegmentDirectory; 2],
    entries: BTreeMap<String, Node>,
    dirty: Vec<Mutation>,
    timestamp: u64,
}

pub fn format<D: BlockDevice>(mut device: D) -> Result<FileSystem<D>, Error> {
    if device.block_count() < 2 * SEGMENT_BLOCKS + SUPERBLOCK_COPIES
        || arena_len(device.block_count()) > u32::MAX as u64
        || segment_count(device.block_count()).is_err()
    {
        return Err(Error::NoSpace);
    }
    let superblock = Superblock {
        features: KNOWN_FEATURES,
        generation: 1,
        transaction: 0,
        total_blocks: device.block_count(),
        arena: 0,
        checkpoint: arena_start(device.block_count(), 0),
        log_head: arena_start(device.block_count(), 0),
        directory: SegmentDirectory::EMPTY,
    };
    let mut entries = BTreeMap::new();
    entries.insert("/".to_string(), Node::Directory(Attributes::new(true, 0)));
    let encoded = superblock.encode();
    device.write_block(0, &encoded)?;
    device.write_block(1, &encoded)?;
    device.flush()?;
    Ok(FileSystem {
        device,
        superblock,
        protected: [superblock.directory; 2],
        entries,
        dirty: Vec::new(),
        timestamp: 0,
    })
}

fn audit_mount<D: BlockDevice>(mut device: D) -> Result<(FileSystem<D>, MetadataReport), Error> {
    let mut copies = [None, None];
    let mut checksum_valid = 0u8;
    for (index, copy) in copies.iter_mut().enumerate() {
        let mut encoded = [0u8; BLOCK_SIZE];
        device.read_block(index as u64, &mut encoded)?;
        if let Ok(superblock) = Superblock::decode(&encoded) {
            checksum_valid += 1;
            *copy = Some(superblock);
        }
    }

    let protected = [
        copies[0]
            .and_then(|superblock| protection_for(superblock).ok())
            .unwrap_or(SegmentDirectory::EMPTY),
        copies[1]
            .and_then(|superblock| protection_for(superblock).ok())
            .unwrap_or(SegmentDirectory::EMPTY),
    ];
    // Validate older state first and release its materialized file bodies before
    // replaying the newer copy. Keeping both caches alive doubles volume-image
    // memory and can make a healthy filesystem fail to mount under its budget.
    let order = if copies[0]
        .zip(copies[1])
        .is_some_and(|(a, b)| a.generation > b.generation)
    {
        [1, 0]
    } else {
        [0, 1]
    };
    let mut selected = None;
    let mut entries = None;
    let mut replay_verified = 0u8;
    for index in order {
        let Some(superblock) = copies[index] else {
            continue;
        };
        if superblock.total_blocks != device.block_count() || validate_head(superblock).is_err() {
            continue;
        }
        drop(entries.take());
        match replay(&mut device, superblock) {
            Ok(decoded) => {
                selected = Some(index);
                entries = Some(decoded);
                replay_verified += 1;
            }
            Err(Error::Io) => return Err(Error::Io),
            Err(error @ (Error::NoSpace | Error::Busy)) => return Err(error),
            Err(_) => {}
        }
    }
    if replay_verified == 2 && matches!((copies[0], copies[1]), (Some(a), Some(b)) if a.generation == b.generation && a != b) { return Err(Error::Corrupt); }
    let selected = selected.ok_or(Error::Corrupt)?;
    let superblock = copies[selected].ok_or(Error::Corrupt)?;
    let entries = match entries {
        Some(entries) => entries,
        None => replay(&mut device, superblock)?,
    };
    Ok((
        FileSystem {
            device,
            superblock,
            protected,
            entries,
            dirty: Vec::new(),
            timestamp: 0,
        },
        MetadataReport {
            selected_superblock: selected as u8,
            checksum_valid_superblocks: checksum_valid,
            replay_verified_superblocks: replay_verified,
        },
    ))
}

impl<D: BlockDevice> FileSystem<D> {
    pub fn set_timestamp(&mut self, unix_seconds: u64) {
        self.timestamp = unix_seconds;
    }

    pub fn attributes(&self, path: &str) -> Result<Attributes, Error> {
        let path = normalize(path)?;
        if self.view(&path).is_none() {
            return Err(Error::NotFound);
        }
        self.attributes_before(&path, self.dirty.len())
            .ok_or(Error::NotFound)
    }

    pub fn set_attributes(&mut self, path: &str, mut attributes: Attributes) -> Result<(), Error> {
        let path = normalize(path)?;
        if attributes.mode > 0o777 {
            return Err(Error::Invalid);
        }
        if self.view(&path).is_none() {
            return Err(Error::NotFound);
        }
        if self.timestamp != 0 {
            attributes.changed = self.timestamp;
        }
        self.dirty.try_reserve(1).map_err(|_| Error::NoSpace)?;
        self.dirty.push(Mutation::Attributes(path, attributes));
        Ok(())
    }
    pub fn mount(mut device: D) -> Result<Self, Error> {
        let mut first = [0u8; BLOCK_SIZE];
        let mut second = [0u8; BLOCK_SIZE];
        device.read_block(0, &mut first)?;
        device.read_block(1, &mut second)?;
        let a = Superblock::decode(&first).ok();
        let b = Superblock::decode(&second).ok();
        let protected = [
            a.and_then(|superblock| protection_for(superblock).ok())
                .unwrap_or(SegmentDirectory::EMPTY),
            b.and_then(|superblock| protection_for(superblock).ok())
                .unwrap_or(SegmentDirectory::EMPTY),
        ];
        let mut candidates = [a, b];
        if matches!((candidates[0], candidates[1]), (Some(a), Some(b)) if b.generation > a.generation)
        {
            candidates.swap(0, 1);
        }
        for superblock in candidates.into_iter().flatten() {
            if superblock.total_blocks != device.block_count() || validate_head(superblock).is_err()
            {
                continue;
            }
            let entries = match replay(&mut device, superblock) {
                Ok(entries) => entries,
                Err(error @ (Error::Io | Error::NoSpace | Error::Busy)) => return Err(error),
                Err(_) => continue,
            };
            return Ok(Self {
                device,
                superblock,
                protected,
                entries,
                dirty: Vec::new(),
                timestamp: 0,
            });
        }
        Err(Error::Corrupt)
    }

    /// Audits both superblock copies without changing the device.
    ///
    /// A returned filesystem may be readable while `is_healthy()` is false;
    /// callers such as fsck should report that loss of redundancy.
    pub fn verify(device: D) -> Result<(Self, MetadataReport), Error> {
        audit_mount(device)
    }

    /// Rebuilds one checksum-invalid superblock from the only replay-verified
    /// copy. Read errors, two damaged copies, and replay ambiguity are never
    /// repaired automatically.
    pub fn repair(device: D) -> Result<(Self, RepairReport), Error> {
        let (mut filesystem, before) = audit_mount(device)?;
        if before.is_healthy() {
            return Ok((
                filesystem,
                RepairReport {
                    metadata: before,
                    repaired_superblocks: 0,
                },
            ));
        }
        if before.checksum_valid_superblocks != 1 || before.replay_verified_superblocks != 1 {
            return Err(Error::Corrupt);
        }
        let selected = filesystem.superblock;
        if has_later_commit(&mut filesystem.device, selected)? {
            return Err(Error::Corrupt);
        }

        let repair_copy = 1 - before.selected_superblock as u64;
        let encoded = filesystem.superblock.encode();
        filesystem.device.write_block(repair_copy, &encoded)?;
        filesystem.device.flush()?;

        let device = filesystem.into_device();
        let (filesystem, after) = audit_mount(device)?;
        if !after.is_healthy() {
            return Err(Error::Corrupt);
        }
        Ok((
            filesystem,
            RepairReport {
                metadata: after,
                repaired_superblocks: 1,
            },
        ))
    }

    pub fn put(&mut self, path: &str, data: &[u8]) -> Result<(), Error> {
        let path = normalize(path)?;
        self.require_parent(&path)?;
        if matches!(self.view(&path), Some(NodeView::Directory)) {
            return Err(Error::IsDirectory);
        }
        let mut contents = Vec::new();
        contents
            .try_reserve_exact(data.len())
            .map_err(|_| Error::NoSpace)?;
        contents.extend_from_slice(data);
        let mut attributes = self
            .attributes(&path)
            .unwrap_or_else(|_| Attributes::new(false, self.timestamp));
        if self.timestamp != 0 {
            attributes.modified = self.timestamp;
            attributes.changed = self.timestamp;
        }
        self.dirty.try_reserve(2).map_err(|_| Error::NoSpace)?;
        self.dirty.push(Mutation::Put(path.clone(), contents));
        self.dirty.push(Mutation::Attributes(path, attributes));
        Ok(())
    }

    pub fn mkdir(&mut self, path: &str) -> Result<(), Error> {
        let path = normalize(path)?;
        if path == "/" || self.view(&path).is_some() {
            return Err(Error::AlreadyExists);
        }
        self.require_parent(&path)?;
        self.dirty.try_reserve(2).map_err(|_| Error::NoSpace)?;
        self.dirty.push(Mutation::Mkdir(path.clone()));
        self.dirty.push(Mutation::Attributes(
            path,
            Attributes::new(true, self.timestamp),
        ));
        Ok(())
    }

    /// Replaces a byte range without rewriting the rest of the file. The offset
    /// may be at EOF, but cannot leave a hole. This operation durably commits
    /// preceding buffered mutations, then publishes one range transaction.
    /// Failure before publication preserves file contents and length.
    pub fn write_range(&mut self, path: &str, offset: usize, data: &[u8]) -> Result<(), Error> {
        self.write_ranges(path, &[(offset, data)])
    }

    /// Applies ordered ranges in one durable transaction. Overlapping ranges
    /// use the last supplied bytes; validation/allocation precede publication.
    pub fn write_ranges(&mut self, path: &str, ranges: &[(usize, &[u8])]) -> Result<(), Error> {
        let path = normalize(path)?;
        let mut size = match self.view(&path) {
            Some(NodeView::File(bytes)) => bytes.len(),
            Some(NodeView::Directory) => return Err(Error::IsDirectory),
            None => return Err(Error::NotFound),
        };
        let mut patches = Vec::new();
        patches
            .try_reserve_exact(ranges.len().checked_add(1).ok_or(Error::NoSpace)?)
            .map_err(|_| Error::NoSpace)?;
        for (offset, data) in ranges {
            if *offset > size {
                return Err(Error::Invalid);
            }
            let end = offset
                .checked_add(data.len())
                .filter(|end| *end <= MAX_RANGE_FILE_BYTES)
                .ok_or(Error::NoSpace)?;
            size = size.max(end);
            if !data.is_empty() {
                patches.push(Mutation::Patch(
                    copy_string(&path)?,
                    *offset,
                    copy_bytes(data)?,
                ));
            }
        }
        if patches.is_empty() {
            return Ok(());
        }
        self.sync()?;
        let Some(Node::File(bytes, _)) = self.entries.get_mut(&path) else {
            return Err(Error::NotFound);
        };
        if size > bytes.len() {
            bytes
                .try_reserve_exact(size - bytes.len())
                .map_err(|_| Error::NoSpace)?;
        }
        let mut attributes = self.attributes(&path)?;
        if self.timestamp != 0 {
            attributes.modified = self.timestamp;
            attributes.changed = self.timestamp;
        }
        patches.push(Mutation::Attributes(path, attributes));
        self.commit(&patches)
    }

    pub fn read(&self, path: &str) -> Result<Vec<u8>, Error> {
        let path = normalize(path)?;
        match self.view(&path) {
            Some(NodeView::File(data)) => copy_bytes(data),
            Some(NodeView::Directory) => Err(Error::IsDirectory),
            None => Err(Error::NotFound),
        }
    }

    pub fn read_range(&self, path: &str, offset: usize, maximum: usize) -> Result<Vec<u8>, Error> {
        let path = normalize(path)?;
        match self.view(&path) {
            Some(NodeView::File(data)) => {
                let start = offset.min(data.len());
                let end = start.saturating_add(maximum).min(data.len());
                copy_bytes(&data[start..end])
            }
            Some(NodeView::Directory) => Err(Error::IsDirectory),
            None => Err(Error::NotFound),
        }
    }

    pub fn metadata(&self, path: &str) -> Result<Metadata, Error> {
        let path = normalize(path)?;
        match self.view(&path) {
            Some(NodeView::File(data)) => Ok(Metadata::File { bytes: data.len() }),
            Some(NodeView::Directory) => Ok(Metadata::Directory),
            None => Err(Error::NotFound),
        }
    }

    pub fn list(&self, path: &str) -> Result<Vec<String>, Error> {
        let path = normalize(path)?;
        if !matches!(self.view(&path), Some(NodeView::Directory)) {
            return Err(Error::NotDirectory);
        }
        let paths = self.materialized_paths()?;
        let prefix = if path == "/" {
            "/".to_string()
        } else {
            path.clone() + "/"
        };
        let mut result = Vec::new();
        for candidate in &paths {
            if candidate == &path || !candidate.starts_with(&prefix) {
                continue;
            }
            let rest = &candidate[prefix.len()..];
            if !rest.is_empty() && !rest.contains('/') {
                result.try_reserve(1).map_err(|_| Error::NoSpace)?;
                result.push(copy_string(candidate)?);
            }
        }
        Ok(result)
    }

    pub fn rename(&mut self, from: &str, to: &str) -> Result<(), Error> {
        let from = normalize(from)?;
        let to = normalize(to)?;
        let source = self.view(&from);
        let moving_directory = matches!(source, Some(NodeView::Directory));
        let moves_into_self = moving_directory
            && to
                .strip_prefix(&from)
                .is_some_and(|suffix| suffix.starts_with('/'));
        if from == "/" || source.is_none() || self.view(&to).is_some() || moves_into_self {
            return Err(Error::Invalid);
        }
        self.require_parent(&to)?;
        self.dirty.push(Mutation::Rename(from, to));
        Ok(())
    }

    pub fn replace_file(&mut self, from: &str, to: &str) -> Result<(), Error> {
        let from = normalize(from)?;
        let to = normalize(to)?;
        if from == "/" || to == "/" || from == to {
            return Err(Error::Invalid);
        }
        if !matches!(self.view(&from), Some(NodeView::File(_))) {
            return Err(Error::NotFound);
        }
        self.require_parent(&to)?;
        match self.view(&to) {
            Some(NodeView::Directory) => return Err(Error::IsDirectory),
            Some(NodeView::File(_)) => self.dirty.push(Mutation::Remove(to.clone())),
            None => {}
        }
        self.dirty.push(Mutation::Rename(from, to));
        Ok(())
    }

    pub fn unlink(&mut self, path: &str) -> Result<(), Error> {
        let path = normalize(path)?;
        if path == "/" || self.view(&path).is_none() {
            return Err(Error::NotFound);
        }
        if matches!(self.view(&path), Some(NodeView::Directory)) && !self.list(&path)?.is_empty() {
            return Err(Error::Busy);
        }
        self.dirty.push(Mutation::Remove(path));
        Ok(())
    }

    pub fn fsync(&mut self, path: &str) -> Result<(), Error> {
        let path = normalize(path)?;
        let Some(last) = self
            .dirty
            .iter()
            .rposition(|mutation| mutation_touches(mutation, &path))
        else {
            return if self.view(&path).is_some() {
                Ok(())
            } else {
                Err(Error::NotFound)
            };
        };
        let retained = self.dirty.split_off(last + 1);
        let selected = core::mem::replace(&mut self.dirty, retained);
        if let Err(error) = self.commit(&selected) {
            let retained = core::mem::take(&mut self.dirty);
            self.dirty = selected;
            self.dirty.extend(retained);
            return Err(error);
        }
        Ok(())
    }

    pub fn sync(&mut self) -> Result<(), Error> {
        let dirty = core::mem::take(&mut self.dirty);
        if let Err(error) = self.commit(&dirty) {
            self.dirty = dirty;
            return Err(error);
        }
        Ok(())
    }

    pub fn gc(&mut self) -> Result<(), Error> {
        self.sync()?;
        if self.superblock.features & FEATURE_SEGMENT_DIRECTORY == 0 {
            return self.migrate_directory();
        }
        if self.superblock.directory.len == 0 {
            return Ok(());
        }
        let capacity = segment_count(self.superblock.total_blocks)? as u64 * SEGMENT_BLOCKS;
        let mut first = true;
        loop {
            let before = self.stats().free_blocks;
            if !first && before * 100 / capacity >= GC_HIGH_WATER_PERCENT {
                return Ok(());
            }
            first = false;
            self.gc_directory_once()?;
            let after = self.stats().free_blocks;
            if after * 100 / capacity >= GC_HIGH_WATER_PERCENT {
                return Ok(());
            }
            if after <= before {
                return Err(Error::NoSpace);
            }
        }
    }

    fn gc_directory_once(&mut self) -> Result<(), Error> {
        let plan = select_victim(&mut self.device, self.superblock, &self.entries)?;
        let txid = self.superblock.transaction + 1;
        let generation = self.superblock.generation + 1;
        let records = records_for(txid, generation, &plan.mutations);
        let mut directory = plan.retained;
        append_directory_records(
            &mut self.device,
            &records,
            &mut directory,
            self.superblock.total_blocks,
            &self.protected,
        )?;
        self.device.commit_stage(CommitStage::GcRecordsWritten);
        // Host validation can afford a second full replay; the EL0 service has
        // already derived this directory from a successfully replayed base.
        #[cfg(feature = "std")]
        if replay_directory(&mut self.device, directory)? != self.entries {
            return Err(Error::Corrupt);
        }
        self.device.flush()?;
        self.device.commit_stage(CommitStage::GcDataFlushed);
        let next = Superblock {
            features: KNOWN_FEATURES,
            generation,
            transaction: txid,
            total_blocks: self.superblock.total_blocks,
            arena: 1 - self.superblock.arena,
            checkpoint: directory
                .as_slice()
                .first()
                .map_or(SUPERBLOCK_COPIES, |entry| segment_start(entry.index)),
            log_head: 0,
            directory,
        };
        self.write_superblock(
            next,
            Some((
                CommitStage::GcCandidateSuperblockWritten,
                CommitStage::GcCandidateSuperblockFlushed,
            )),
        )?;
        self.superblock = next;

        let sealed = Superblock {
            generation: next.generation + 1,
            ..next
        };
        self.write_superblock(
            sealed,
            Some((
                CommitStage::GcSealedSuperblockWritten,
                CommitStage::GcSealedSuperblockFlushed,
            )),
        )?;
        self.superblock = sealed;
        Ok(())
    }

    fn compact_directory_all(&mut self) -> Result<(), Error> {
        let txid = self.superblock.transaction + 1;
        let generation = self.superblock.generation + 1;
        let mutations = snapshot_mutations(&self.entries);
        let records = records_for(txid, generation, &mutations);
        let mut directory = SegmentDirectory::EMPTY;
        append_directory_records(
            &mut self.device,
            &records,
            &mut directory,
            self.superblock.total_blocks,
            &self.protected,
        )?;
        self.device.commit_stage(CommitStage::GcRecordsWritten);
        self.device.flush()?;
        self.device.commit_stage(CommitStage::GcDataFlushed);
        let next = Superblock {
            features: KNOWN_FEATURES,
            generation,
            transaction: txid,
            total_blocks: self.superblock.total_blocks,
            arena: 1 - self.superblock.arena,
            checkpoint: directory
                .as_slice()
                .first()
                .map_or(SUPERBLOCK_COPIES, |entry| segment_start(entry.index)),
            log_head: 0,
            directory,
        };
        self.write_superblock(
            next,
            Some((
                CommitStage::GcCandidateSuperblockWritten,
                CommitStage::GcCandidateSuperblockFlushed,
            )),
        )?;
        self.superblock = next;
        Ok(())
    }

    fn migrate_directory(&mut self) -> Result<(), Error> {
        self.compact_directory_all()
    }

    pub fn check(&mut self) -> Result<Stats, Error> {
        let replayed = replay(&mut self.device, self.superblock)?;
        if replayed != self.entries {
            return Err(Error::Corrupt);
        }
        Ok(self.stats())
    }

    pub fn stats(&self) -> Stats {
        if self.superblock.features & FEATURE_SEGMENT_DIRECTORY != 0 {
            let used_blocks = self
                .superblock
                .directory
                .as_slice()
                .iter()
                .map(|entry| entry.used as u64)
                .sum();
            return Stats {
                generation: self.superblock.generation,
                transaction: self.superblock.transaction,
                total_blocks: self.superblock.total_blocks,
                active_arena: self.superblock.arena,
                checkpoint_block: self
                    .superblock
                    .directory
                    .as_slice()
                    .first()
                    .map_or(SUPERBLOCK_COPIES, |entry| segment_start(entry.index)),
                segments_since_checkpoint: self.superblock.directory.len as u64,
                used_blocks,
                free_blocks: allocatable_blocks(
                    self.superblock.total_blocks,
                    &self.superblock.directory,
                    &self.protected,
                ),
                entries: self.entries.len(),
                segment_directory: true,
                active_segments: self.superblock.directory.len,
            };
        }
        let start = arena_start(self.superblock.total_blocks, self.superblock.arena);
        let end = arena_end(self.superblock.total_blocks, self.superblock.arena);
        Stats {
            generation: self.superblock.generation,
            transaction: self.superblock.transaction,
            total_blocks: self.superblock.total_blocks,
            active_arena: self.superblock.arena,
            checkpoint_block: self.superblock.checkpoint,
            segments_since_checkpoint: self
                .superblock
                .log_head
                .saturating_sub(self.superblock.checkpoint)
                .div_ceil(SEGMENT_BLOCKS),
            used_blocks: self.superblock.log_head - start,
            free_blocks: end - self.superblock.log_head,
            entries: self.entries.len(),
            segment_directory: false,
            active_segments: self
                .superblock
                .log_head
                .saturating_sub(self.superblock.checkpoint)
                .div_ceil(SEGMENT_BLOCKS) as u16,
        }
    }

    pub fn needs_gc(&self) -> bool {
        let stats = self.stats();
        let capacity = if self.superblock.features & FEATURE_SEGMENT_DIRECTORY != 0 {
            segment_count(stats.total_blocks).unwrap_or(1) as u64 * SEGMENT_BLOCKS
        } else {
            arena_len(stats.total_blocks)
        };
        stats.free_blocks * 100 / capacity < GC_LOW_WATER_PERCENT
    }

    pub fn into_device(self) -> D {
        self.device
    }

    fn commit(&mut self, mutations: &[Mutation]) -> Result<(), Error> {
        if mutations.is_empty() {
            return Ok(());
        }
        if self.superblock.features & FEATURE_SEGMENT_DIRECTORY == 0 {
            self.migrate_directory()?;
        }
        if self.needs_gc() {
            self.gc()?;
        }
        let txid = self.superblock.transaction + 1;
        let generation = self.superblock.generation + 1;
        let observed = mutations.iter().any(observe_transaction);
        let records = records_for(txid, generation, mutations);
        let mut directory = self.superblock.directory;
        append_directory_records(
            &mut self.device,
            &records,
            &mut directory,
            self.superblock.total_blocks,
            &self.protected,
        )?;
        if observed {
            self.device
                .commit_stage(CommitStage::TransactionRecordsWritten);
        }
        self.device.flush()?;
        if observed {
            self.device
                .commit_stage(CommitStage::TransactionDataFlushed);
        }
        let next = Superblock {
            features: KNOWN_FEATURES,
            generation,
            transaction: txid,
            total_blocks: self.superblock.total_blocks,
            arena: self.superblock.arena,
            checkpoint: directory
                .as_slice()
                .first()
                .map_or(SUPERBLOCK_COPIES, |entry| segment_start(entry.index)),
            log_head: 0,
            directory,
        };
        self.write_superblock(
            next,
            observed.then_some((
                CommitStage::TransactionSuperblockWritten,
                CommitStage::TransactionSuperblockFlushed,
            )),
        )?;
        for mutation in mutations {
            apply(&mut self.entries, mutation.clone())?;
        }
        self.superblock = next;
        Ok(())
    }

    fn write_superblock(
        &mut self,
        next: Superblock,
        stages: Option<(CommitStage, CommitStage)>,
    ) -> Result<(), Error> {
        let copy = next.generation % 2;
        let next_protection = protection_for(next)?;
        let conservative = merge_protection(self.protected[copy as usize], next_protection)?;
        if let Err(error) = self.device.write_block(copy, &next.encode()) {
            self.protected[copy as usize] = conservative;
            return Err(error);
        }
        if let Some((written, _)) = stages {
            self.device.commit_stage(written);
        }
        if let Err(error) = self.device.flush() {
            self.protected[copy as usize] = conservative;
            return Err(error);
        }
        if let Some((_, flushed)) = stages {
            self.device.commit_stage(flushed);
        }
        self.protected[copy as usize] = next_protection;
        Ok(())
    }

    fn require_parent(&self, path: &str) -> Result<(), Error> {
        let parent = parent(path);
        if matches!(self.view(parent), Some(NodeView::Directory)) {
            Ok(())
        } else {
            Err(Error::NotDirectory)
        }
    }

    fn view(&self, path: &str) -> Option<NodeView<'_>> {
        self.view_before(path, self.dirty.len())
    }

    fn attributes_before(&self, path: &str, end: usize) -> Option<Attributes> {
        for (index, mutation) in self.dirty[..end].iter().enumerate().rev() {
            match mutation {
                Mutation::Attributes(candidate, attributes) if candidate == path => {
                    return Some(*attributes);
                }
                Mutation::Mkdir(candidate) if candidate == path => {
                    return Some(Attributes::new(true, 0));
                }
                Mutation::Put(candidate, _) if candidate == path => {
                    return Some(
                        self.attributes_before(path, index)
                            .unwrap_or_else(|| Attributes::new(false, 0)),
                    );
                }
                Mutation::Remove(candidate) if subtree_suffix(path, candidate).is_some() => {
                    return None;
                }
                Mutation::Rename(from, to) => {
                    if let Some(suffix) = subtree_suffix(path, to) {
                        return self.attributes_before(&(from.clone() + suffix), index);
                    }
                    if subtree_suffix(path, from).is_some() {
                        return None;
                    }
                }
                _ => {}
            }
        }
        self.entries.get(path).map(Node::attributes)
    }

    fn view_before<'a>(&'a self, path: &str, end: usize) -> Option<NodeView<'a>> {
        for (index, mutation) in self.dirty[..end].iter().enumerate().rev() {
            match mutation {
                Mutation::Put(candidate, data) if candidate == path => {
                    return Some(NodeView::File(data));
                }
                Mutation::Mkdir(candidate) if candidate == path => {
                    return Some(NodeView::Directory);
                }
                Mutation::Remove(candidate) if subtree_suffix(path, candidate).is_some() => {
                    return None;
                }
                Mutation::Rename(from, to) => {
                    if let Some(suffix) = subtree_suffix(path, to) {
                        let mut source = from.clone();
                        source.push_str(suffix);
                        return self.view_before(&source, index);
                    }
                    if subtree_suffix(path, from).is_some() {
                        return None;
                    }
                }
                _ => {}
            }
        }
        self.entries.get(path).map(|node| match node {
            Node::File(data, _) => NodeView::File(data),
            Node::Directory(_) => NodeView::Directory,
        })
    }

    fn materialized_paths(&self) -> Result<Vec<String>, Error> {
        if self.entries.len() > MAX_MATERIALIZED_PATHS {
            return Err(Error::NoSpace);
        }
        let mut paths = Vec::new();
        paths
            .try_reserve_exact(self.entries.len())
            .map_err(|_| Error::NoSpace)?;
        for path in self.entries.keys() {
            paths.push(copy_string(path)?);
        }
        for mutation in &self.dirty {
            match mutation {
                Mutation::Attributes(_, _) => {}
                Mutation::Put(path, _) | Mutation::Patch(path, _, _) | Mutation::Mkdir(path) => {
                    if !paths.contains(path) {
                        if paths.len() == MAX_MATERIALIZED_PATHS {
                            return Err(Error::NoSpace);
                        }
                        paths.try_reserve(1).map_err(|_| Error::NoSpace)?;
                        paths.push(copy_string(path)?);
                    }
                }
                Mutation::Remove(path) => {
                    paths.retain(|candidate| subtree_suffix(candidate, path).is_none());
                }
                Mutation::Rename(from, to) => {
                    for candidate in &mut paths {
                        let Some(suffix) = subtree_suffix(candidate, from) else {
                            continue;
                        };
                        let mut destination = copy_string(to)?;
                        destination
                            .try_reserve_exact(suffix.len())
                            .map_err(|_| Error::NoSpace)?;
                        destination.push_str(suffix);
                        *candidate = destination;
                    }
                    paths.sort_unstable();
                    paths.dedup();
                }
            }
        }
        Ok(paths)
    }
}

fn has_later_commit<D: BlockDevice>(device: &mut D, selected: Superblock) -> Result<bool, Error> {
    for index in 0..segment_count(selected.total_blocks)? {
        let mut block = segment_start(index);
        let end = block + SEGMENT_BLOCKS;
        while block < end {
            let mut first = [0u8; BLOCK_SIZE];
            device.read_block(block, &mut first)?;
            if first.iter().all(|byte| *byte == 0) {
                break;
            }
            if get_u32(&first, 0) != RECORD_MAGIC {
                return Ok(true);
            }
            let (record, consumed) = match read_record(device, block, end) {
                Ok(record) => record,
                Err(Error::Io) => return Err(Error::Io),
                Err(error @ (Error::NoSpace | Error::Busy)) => return Err(error),
                Err(_) => return Ok(true),
            };
            if record.kind == RecordKind::Commit
                && (record.generation > selected.generation || record.txid > selected.transaction)
            {
                return Ok(true);
            }
            block += consumed;
        }
    }
    Ok(false)
}

fn observe_transaction(mutation: &Mutation) -> bool {
    match mutation {
        Mutation::Put(path, _)
        | Mutation::Patch(path, _, _)
        | Mutation::Attributes(path, _)
        | Mutation::Mkdir(path)
        | Mutation::Remove(path)
        | Mutation::Rename(path, _) => path == "/faultcut",
    }
}

fn records_for(txid: u64, generation: u64, mutations: &[Mutation]) -> Vec<Record> {
    let mut records = Vec::new();
    for mutation in mutations {
        match mutation {
            Mutation::Put(path, data) => {
                let max_data =
                    SEGMENT_BLOCKS as usize * BLOCK_SIZE - RECORD_HEADER_SIZE - path.len();
                if data.is_empty() {
                    records.push(Record {
                        kind: RecordKind::Put,
                        txid,
                        generation,
                        object: object_number(path),
                        logical_offset: 0,
                        path: path.clone(),
                        secondary: String::new(),
                        data: Vec::new(),
                    });
                } else {
                    for (index, chunk) in data.chunks(max_data).enumerate() {
                        records.push(Record {
                            kind: RecordKind::Put,
                            txid,
                            generation,
                            object: object_number(path),
                            logical_offset: (index * max_data) as u64,
                            path: path.clone(),
                            secondary: String::new(),
                            data: chunk.to_vec(),
                        });
                    }
                }
            }
            Mutation::Patch(path, offset, data) => {
                let max_data =
                    SEGMENT_BLOCKS as usize * BLOCK_SIZE - RECORD_HEADER_SIZE - path.len();
                for (index, chunk) in data.chunks(max_data).enumerate() {
                    records.push(Record {
                        kind: RecordKind::Patch,
                        txid,
                        generation,
                        object: object_number(path),
                        logical_offset: (offset + index * max_data) as u64,
                        path: path.clone(),
                        secondary: String::new(),
                        data: chunk.to_vec(),
                    });
                }
            }
            Mutation::Attributes(path, attributes) => records.push(Record {
                kind: RecordKind::Attributes,
                txid,
                generation,
                object: object_number(path),
                logical_offset: 0,
                path: path.clone(),
                secondary: String::new(),
                data: attributes.encode(),
            }),
            Mutation::Mkdir(path) => records.push(Record {
                kind: RecordKind::Mkdir,
                txid,
                generation,
                object: object_number(path),
                logical_offset: 0,
                path: path.clone(),
                secondary: String::new(),
                data: Vec::new(),
            }),
            Mutation::Remove(path) => records.push(Record {
                kind: RecordKind::Remove,
                txid,
                generation,
                object: object_number(path),
                logical_offset: 0,
                path: path.clone(),
                secondary: String::new(),
                data: Vec::new(),
            }),
            Mutation::Rename(from, to) => records.push(Record {
                kind: RecordKind::Rename,
                txid,
                generation,
                object: object_number(from),
                logical_offset: 0,
                path: from.clone(),
                secondary: to.clone(),
                data: Vec::new(),
            }),
        }
    }
    records.push(Record {
        kind: RecordKind::Commit,
        txid,
        generation,
        object: 0,
        logical_offset: 0,
        path: String::new(),
        secondary: String::new(),
        data: Vec::new(),
    });
    records
}

fn snapshot_mutations(entries: &BTreeMap<String, Node>) -> Vec<Mutation> {
    let mut mutations = Vec::new();
    for (path, node) in entries {
        if path != "/" {
            match node {
                Node::File(data, _) => mutations.push(Mutation::Put(path.clone(), data.clone())),
                Node::Directory(_) => mutations.push(Mutation::Mkdir(path.clone())),
            }
        }
        mutations.push(Mutation::Attributes(path.clone(), node.attributes()));
    }
    mutations
}

fn apply_record(
    entries: &mut BTreeMap<String, Node>,
    record: Record,
    relaxed: bool,
) -> Result<(), Error> {
    match record.kind {
        RecordKind::Put => {
            let offset = usize::try_from(record.logical_offset).map_err(|_| Error::Corrupt)?;
            if offset == 0 {
                let attributes = entries
                    .get(&record.path)
                    .map(Node::attributes)
                    .unwrap_or_else(|| Attributes::new(false, 0));
                entries.insert(record.path, Node::File(record.data, attributes));
                return Ok(());
            }
            let Some(Node::File(data, _)) = entries.get_mut(&record.path) else {
                return Err(Error::Corrupt);
            };
            if data.len() != offset {
                return Err(Error::Corrupt);
            }
            data.try_reserve(record.data.len())
                .map_err(|_| Error::NoSpace)?;
            data.extend_from_slice(&record.data);
            Ok(())
        }
        RecordKind::Mkdir => apply(entries, Mutation::Mkdir(record.path)),
        RecordKind::Attributes => {
            let attributes = Attributes::decode(&record.data)?;
            if relaxed && !entries.contains_key(&record.path) {
                return Ok(());
            }
            apply(entries, Mutation::Attributes(record.path, attributes))
        }
        RecordKind::Patch => {
            let offset = usize::try_from(record.logical_offset).map_err(|_| Error::Corrupt)?;
            let end = offset
                .checked_add(record.data.len())
                .filter(|end| *end <= MAX_RANGE_FILE_BYTES)
                .ok_or(Error::Corrupt)?;
            // Victim planning may remove the base Put before retained patches
            // are replayed. The final state diff restores the complete file.
            if relaxed && !entries.contains_key(&record.path) {
                entries.insert(
                    record.path.clone(),
                    Node::File(Vec::new(), Attributes::new(false, 0)),
                );
            }
            if relaxed && let Some(Node::File(bytes, _)) = entries.get_mut(&record.path) {
                if offset > bytes.len() {
                    bytes
                        .try_reserve_exact(end - bytes.len())
                        .map_err(|_| Error::NoSpace)?;
                    bytes.resize(offset, 0);
                }
            }
            apply(entries, Mutation::Patch(record.path, offset, record.data))
        }
        RecordKind::Remove => apply(entries, Mutation::Remove(record.path)),
        RecordKind::Rename => apply(entries, Mutation::Rename(record.path, record.secondary)),
        RecordKind::Commit | RecordKind::Padding => Err(Error::Corrupt),
    }
}

fn append_directory_records<D: BlockDevice>(
    device: &mut D,
    records: &[Record],
    directory: &mut SegmentDirectory,
    total_blocks: u64,
    protected: &[SegmentDirectory; 2],
) -> Result<(), Error> {
    let mut unavailable = directory_mask(&protected[0]) | directory_mask(&protected[1]);
    unavailable |= directory_mask(directory);
    let transaction_blocks = encoded_record_blocks(records)?;
    if transaction_blocks <= SEGMENT_BLOCKS {
        if let Some(tail) = directory.as_mut_slice().last_mut() {
            let remaining = SEGMENT_BLOCKS - tail.used as u64;
            if remaining != 0 && remaining < transaction_blocks {
                // Keeping a small transaction in one segment avoids linking
                // successive GC victims through boundary-spanning commits.
                write_padding(
                    device,
                    segment_start(tail.index) + tail.used as u64,
                    remaining,
                )?;
                tail.used = SEGMENT_BLOCKS as u16;
            }
        }
    }
    for record in records {
        let bytes = encode_record(record)?;
        let blocks = bytes.len() as u64 / BLOCK_SIZE as u64;
        if blocks == 0 || blocks > SEGMENT_BLOCKS {
            return Err(Error::NoSpace);
        }

        let remaining = directory
            .as_slice()
            .last()
            .map_or(0, |entry| SEGMENT_BLOCKS - entry.used as u64);
        if directory.len == 0 || blocks > remaining {
            if let Some(tail) = directory.as_mut_slice().last_mut() {
                let padding = SEGMENT_BLOCKS - tail.used as u64;
                if padding > 0 {
                    write_padding(
                        device,
                        segment_start(tail.index) + tail.used as u64,
                        padding,
                    )?;
                    tail.used = SEGMENT_BLOCKS as u16;
                }
            }
            let index = allocate_segment(total_blocks, unavailable)?;
            unavailable |= 1u128 << index;
            directory.push(SegmentRef { index, used: 0 })?;
        }

        let tail = directory.as_mut_slice().last_mut().ok_or(Error::NoSpace)?;
        let start = segment_start(tail.index) + tail.used as u64;
        for (offset, chunk) in bytes.chunks_exact(BLOCK_SIZE).enumerate() {
            let mut block = [0u8; BLOCK_SIZE];
            block.copy_from_slice(chunk);
            device.write_block(start + offset as u64, &block)?;
        }
        tail.used += blocks as u16;
    }
    Ok(())
}

fn directory_mask(directory: &SegmentDirectory) -> u128 {
    directory
        .as_slice()
        .iter()
        .fold(0, |mask, entry| mask | (1u128 << entry.index))
}

fn allocate_segment(total_blocks: u64, unavailable: u128) -> Result<u16, Error> {
    for index in 0..segment_count(total_blocks)? {
        if unavailable & (1u128 << index) == 0 {
            return Ok(index);
        }
    }
    Err(Error::NoSpace)
}

fn allocatable_blocks(
    total_blocks: u64,
    current: &SegmentDirectory,
    protected: &[SegmentDirectory; 2],
) -> u64 {
    let protected_mask = directory_mask(&protected[0]) | directory_mask(&protected[1]);
    let current_mask = directory_mask(current);
    let mut free = current
        .as_slice()
        .last()
        .map_or(0, |entry| SEGMENT_BLOCKS - entry.used as u64);
    let Ok(count) = segment_count(total_blocks) else {
        return 0;
    };
    for index in 0..count {
        let bit = 1u128 << index;
        if protected_mask & bit == 0 && current_mask & bit == 0 {
            free += SEGMENT_BLOCKS;
        }
    }
    free
}

fn write_padding<D: BlockDevice>(device: &mut D, start: u64, blocks: u64) -> Result<(), Error> {
    if blocks == 0 || blocks > SEGMENT_BLOCKS {
        return Err(Error::Invalid);
    }
    let mut first = [0u8; BLOCK_SIZE];
    put_u32(&mut first, 0, RECORD_MAGIC);
    first[4] = RecordKind::Padding as u8;
    put_u32(&mut first, 32, blocks as u32);
    let zero = [0u8; BLOCK_SIZE];
    let mut crc = crc32c_update(!0u32, &first);
    for _ in 1..blocks {
        crc = crc32c_update(crc, &zero);
    }
    put_u32(&mut first, 36, !crc);
    device.write_block(start, &first)?;
    for index in 1..blocks {
        device.write_block(start + index, &zero)?;
    }
    Ok(())
}

fn replay<D: BlockDevice>(
    device: &mut D,
    superblock: Superblock,
) -> Result<BTreeMap<String, Node>, Error> {
    if superblock.features & FEATURE_SEGMENT_DIRECTORY != 0 {
        return replay_directory(device, superblock.directory);
    }
    let mut replay = ReplayState::new(false);
    let mut block = superblock.checkpoint;
    let segment_origin = arena_start(superblock.total_blocks, superblock.arena);
    let strict_segment_layout = superblock.features & FEATURE_SEGMENT_LAYOUT != 0;
    while block < superblock.log_head {
        let (record, consumed) = read_record(device, block, superblock.log_head)?;
        let segment_offset = block.saturating_sub(segment_origin) % SEGMENT_BLOCKS;
        let remaining = SEGMENT_BLOCKS - segment_offset;
        if strict_segment_layout && consumed > remaining {
            return Err(Error::Corrupt);
        }
        if record.kind == RecordKind::Padding {
            if segment_offset == 0 || consumed != remaining {
                return Err(Error::Corrupt);
            }
            block += consumed;
            continue;
        }
        replay.accept(record)?;
        block += consumed;
    }
    replay.finish()
}

fn replay_directory<D: BlockDevice>(
    device: &mut D,
    directory: SegmentDirectory,
) -> Result<BTreeMap<String, Node>, Error> {
    replay_directory_mode(device, directory, false)
}

fn replay_directory_relaxed<D: BlockDevice>(
    device: &mut D,
    directory: SegmentDirectory,
) -> Result<BTreeMap<String, Node>, Error> {
    replay_directory_mode(device, directory, true)
}

fn replay_directory_mode<D: BlockDevice>(
    device: &mut D,
    directory: SegmentDirectory,
    relaxed: bool,
) -> Result<BTreeMap<String, Node>, Error> {
    let mut replay = ReplayState::new(relaxed);
    for segment in directory.as_slice() {
        let start = segment_start(segment.index);
        let limit = start + segment.used as u64;
        let mut block = start;
        while block < limit {
            let (record, consumed) = read_record(device, block, limit)?;
            let remaining = SEGMENT_BLOCKS - (block - start);
            if consumed > remaining {
                return Err(Error::Corrupt);
            }
            if record.kind == RecordKind::Padding {
                if block == start || consumed != remaining {
                    return Err(Error::Corrupt);
                }
            } else {
                replay.accept(record)?;
            }
            block += consumed;
        }
    }
    replay.finish()
}

fn segment_transaction_sets<D: BlockDevice>(
    device: &mut D,
    directory: SegmentDirectory,
) -> Result<Vec<Vec<u64>>, Error> {
    let mut result = Vec::new();
    for segment in directory.as_slice() {
        let start = segment_start(segment.index);
        let limit = start + segment.used as u64;
        let mut block = start;
        let mut transactions = Vec::new();
        while block < limit {
            let (kind, transaction, consumed) = read_record_identity(device, block, limit)?;
            if kind != RecordKind::Padding && !transactions.contains(&transaction) {
                transactions.push(transaction);
            }
            block += consumed;
        }
        result.push(transactions);
    }
    Ok(result)
}

fn read_record_identity<D: BlockDevice>(
    device: &mut D,
    start: u64,
    limit: u64,
) -> Result<(RecordKind, u64, u64), Error> {
    let mut first = [0u8; BLOCK_SIZE];
    device.read_block(start, &mut first)?;
    if get_u32(&first, 0) != RECORD_MAGIC {
        return Err(Error::Corrupt);
    }
    let blocks = get_u32(&first, 32) as u64;
    if blocks == 0 || start.checked_add(blocks).is_none_or(|end| end > limit) {
        return Err(Error::Corrupt);
    }
    let kind = match first[4] {
        1 => RecordKind::Put,
        2 => RecordKind::Mkdir,
        3 => RecordKind::Remove,
        4 => RecordKind::Rename,
        5 => RecordKind::Commit,
        6 => RecordKind::Padding,
        7 => RecordKind::Patch,
        8 => RecordKind::Attributes,
        _ => return Err(Error::Corrupt),
    };
    if kind == RecordKind::Padding && blocks > SEGMENT_BLOCKS {
        return Err(Error::Corrupt);
    }
    // Mount already verified every payload and CRC in this immutable
    // directory. Victim grouping only needs transaction identity and length.
    Ok((kind, get_u64(&first, 8), blocks))
}

fn segment_components(transaction_sets: &[Vec<u64>]) -> Vec<Vec<usize>> {
    let mut parent: Vec<usize> = (0..transaction_sets.len()).collect();
    let mut owners = BTreeMap::new();
    for (segment, transactions) in transaction_sets.iter().enumerate() {
        for transaction in transactions {
            if let Some(previous) = owners.insert(*transaction, segment) {
                union_components(&mut parent, previous, segment);
            }
        }
    }
    let mut components: Vec<Vec<usize>> = Vec::new();
    let mut component_slots: BTreeMap<usize, usize> = BTreeMap::new();
    for segment in 0..transaction_sets.len() {
        let root = component_root(&mut parent, segment);
        if let Some(slot) = component_slots.get(&root).copied() {
            components[slot].push(segment);
        } else {
            component_slots.insert(root, components.len());
            components.push(vec![segment]);
        }
    }
    components
}

fn component_root(parent: &mut [usize], mut node: usize) -> usize {
    while parent[node] != node {
        parent[node] = parent[parent[node]];
        node = parent[node];
    }
    node
}

fn union_components(parent: &mut [usize], left: usize, right: usize) {
    let left = component_root(parent, left);
    let right = component_root(parent, right);
    if left != right {
        parent[right] = left;
    }
}

fn directory_without(
    directory: SegmentDirectory,
    removed: &[usize],
) -> Result<SegmentDirectory, Error> {
    let mut filtered = SegmentDirectory::EMPTY;
    for (position, entry) in directory.as_slice().iter().copied().enumerate() {
        if !removed.contains(&position) {
            filtered.push(entry)?;
        }
    }
    Ok(filtered)
}

fn state_diff(base: &BTreeMap<String, Node>, target: &BTreeMap<String, Node>) -> Vec<Mutation> {
    let mut removed: Vec<String> = base
        .keys()
        .filter(|path| path.as_str() != "/" && !target.contains_key(*path))
        .cloned()
        .collect();
    removed.sort_by_key(|path| core::cmp::Reverse(path.len()));
    let mut mutations: Vec<Mutation> = removed.into_iter().map(Mutation::Remove).collect();
    for (path, node) in target {
        if base.get(path) == Some(node) {
            continue;
        }
        let data_changed = match (base.get(path), node) {
            (Some(Node::File(previous, _)), Node::File(current, _)) => previous != current,
            (Some(Node::Directory(_)), Node::Directory(_)) => false,
            _ => true,
        };
        if data_changed && path != "/" {
            match node {
                Node::File(data, _) => mutations.push(Mutation::Put(path.clone(), data.clone())),
                Node::Directory(_) => mutations.push(Mutation::Mkdir(path.clone())),
            }
        }
        mutations.push(Mutation::Attributes(path.clone(), node.attributes()));
    }
    mutations
}

fn encoded_record_blocks(records: &[Record]) -> Result<u64, Error> {
    let mut blocks = 0u64;
    for record in records {
        let payload = RECORD_HEADER_SIZE
            .checked_add(record.path.len())
            .and_then(|value| value.checked_add(record.secondary.len()))
            .and_then(|value| value.checked_add(record.data.len()))
            .ok_or(Error::NoSpace)?;
        blocks = blocks
            .checked_add(payload.div_ceil(BLOCK_SIZE) as u64)
            .ok_or(Error::NoSpace)?;
    }
    Ok(blocks)
}

struct VictimPlan {
    retained: SegmentDirectory,
    mutations: Vec<Mutation>,
    live_blocks: u64,
    capacity_blocks: u64,
}

fn select_victim<D: BlockDevice>(
    device: &mut D,
    superblock: Superblock,
    target: &BTreeMap<String, Node>,
) -> Result<VictimPlan, Error> {
    let transaction_sets = segment_transaction_sets(device, superblock.directory)?;
    let components = segment_components(&transaction_sets);
    let mut best: Option<VictimPlan> = None;
    for removed in components {
        let retained = directory_without(superblock.directory, &removed)?;
        let Ok(base) = replay_directory_relaxed(device, retained) else {
            continue;
        };
        let mutations = state_diff(&base, target);
        let records = records_for(
            superblock.transaction + 1,
            superblock.generation + 1,
            &mutations,
        );
        let live_blocks = encoded_record_blocks(&records)?;
        let capacity_blocks = removed.len() as u64 * SEGMENT_BLOCKS;
        let candidate = VictimPlan {
            retained,
            mutations,
            live_blocks,
            capacity_blocks,
        };
        // Removing this component leaves the target state unchanged, so it
        // has zero live payload and no later candidate can improve its ratio.
        if candidate.mutations.is_empty() {
            return Ok(candidate);
        }
        let replace = best.as_ref().is_none_or(|current| {
            candidate.live_blocks * current.capacity_blocks
                < current.live_blocks * candidate.capacity_blocks
                || (candidate.live_blocks * current.capacity_blocks
                    == current.live_blocks * candidate.capacity_blocks
                    && candidate.capacity_blocks > current.capacity_blocks)
        });
        if replace {
            best = Some(candidate);
        }
    }
    best.ok_or(Error::NoSpace)
}

struct ReplayState {
    entries: BTreeMap<String, Node>,
    pending: Vec<Record>,
    pending_tx: u64,
    relaxed: bool,
}

impl ReplayState {
    fn new(relaxed: bool) -> Self {
        let mut entries = BTreeMap::new();
        entries.insert("/".to_string(), Node::Directory(Attributes::new(true, 0)));
        Self {
            entries,
            pending: Vec::new(),
            pending_tx: 0,
            relaxed,
        }
    }

    fn accept(&mut self, record: Record) -> Result<(), Error> {
        if self.pending_tx != 0 && record.txid != self.pending_tx {
            return Err(Error::Corrupt);
        }
        self.pending_tx = record.txid;
        match record.kind {
            RecordKind::Attributes
            | RecordKind::Put
            | RecordKind::Patch
            | RecordKind::Mkdir
            | RecordKind::Remove
            | RecordKind::Rename => {
                self.pending.push(record);
            }
            RecordKind::Commit => {
                for record in self.pending.drain(..) {
                    let kind = record.kind;
                    if let Err(error) = apply_record(&mut self.entries, record, self.relaxed) {
                        if !(self.relaxed && kind == RecordKind::Rename && error == Error::NotFound)
                        {
                            return Err(error);
                        }
                    }
                }
                self.pending_tx = 0;
            }
            RecordKind::Padding => return Err(Error::Corrupt),
        }
        Ok(())
    }

    fn finish(self) -> Result<BTreeMap<String, Node>, Error> {
        if !self.pending.is_empty() || self.pending_tx != 0 {
            return Err(Error::Corrupt);
        }
        Ok(self.entries)
    }
}

fn encode_record(record: &Record) -> Result<Vec<u8>, Error> {
    if record.path.len() > 255 || record.secondary.len() > 255 {
        return Err(Error::NameTooLong);
    }
    let payload_len = record.path.len() + record.secondary.len() + record.data.len();
    let blocks = (RECORD_HEADER_SIZE + payload_len).div_ceil(BLOCK_SIZE);
    let mut out = vec![0u8; blocks * BLOCK_SIZE];
    put_u32(&mut out, 0, RECORD_MAGIC);
    out[4] = record.kind as u8;
    put_u64(&mut out, 8, record.txid);
    put_u64(&mut out, 16, record.generation);
    put_u16(&mut out, 24, record.path.len() as u16);
    put_u16(&mut out, 26, record.secondary.len() as u16);
    put_u32(&mut out, 28, record.data.len() as u32);
    put_u32(&mut out, 32, blocks as u32);
    put_u64(&mut out, 40, record.object);
    put_u64(&mut out, 48, record.logical_offset);
    let mut cursor = RECORD_HEADER_SIZE;
    out[cursor..cursor + record.path.len()].copy_from_slice(record.path.as_bytes());
    cursor += record.path.len();
    out[cursor..cursor + record.secondary.len()].copy_from_slice(record.secondary.as_bytes());
    cursor += record.secondary.len();
    out[cursor..cursor + record.data.len()].copy_from_slice(&record.data);
    put_u32(&mut out, 36, 0);
    let crc = crc32c(&out);
    put_u32(&mut out, 36, crc);
    Ok(out)
}

fn read_record<D: BlockDevice>(
    device: &mut D,
    start: u64,
    limit: u64,
) -> Result<(Record, u64), Error> {
    let mut first = [0u8; BLOCK_SIZE];
    device.read_block(start, &mut first)?;
    if get_u32(&first, 0) != RECORD_MAGIC {
        return Err(Error::Corrupt);
    }
    let blocks = get_u32(&first, 32) as u64;
    if blocks == 0 || start.checked_add(blocks).is_none_or(|end| end > limit) {
        return Err(Error::Corrupt);
    }
    if first[4] == RecordKind::Padding as u8 {
        if blocks > SEGMENT_BLOCKS
            || get_u16(&first, 24) != 0
            || get_u16(&first, 26) != 0
            || get_u32(&first, 28) != 0
        {
            return Err(Error::Corrupt);
        }
        let expected = get_u32(&first, 36);
        put_u32(&mut first, 36, 0);
        let mut crc = crc32c_update(!0u32, &first);
        for index in 1..blocks {
            let mut block = [0u8; BLOCK_SIZE];
            device.read_block(start + index, &mut block)?;
            crc = crc32c_update(crc, &block);
        }
        if (!crc) != expected {
            return Err(Error::Corrupt);
        }
        return Ok((
            Record {
                kind: RecordKind::Padding,
                txid: 0,
                generation: 0,
                object: 0,
                logical_offset: 0,
                path: String::new(),
                secondary: String::new(),
                data: Vec::new(),
            },
            blocks,
        ));
    }
    let mut bytes = vec![0u8; blocks as usize * BLOCK_SIZE];
    bytes[..BLOCK_SIZE].copy_from_slice(&first);
    for index in 1..blocks {
        let mut block = [0u8; BLOCK_SIZE];
        device.read_block(start + index, &mut block)?;
        let offset = index as usize * BLOCK_SIZE;
        bytes[offset..offset + BLOCK_SIZE].copy_from_slice(&block);
    }
    let expected = get_u32(&bytes, 36);
    put_u32(&mut bytes, 36, 0);
    if crc32c(&bytes) != expected {
        return Err(Error::Corrupt);
    }
    let kind = match bytes[4] {
        1 => RecordKind::Put,
        2 => RecordKind::Mkdir,
        3 => RecordKind::Remove,
        4 => RecordKind::Rename,
        5 => RecordKind::Commit,
        6 => RecordKind::Padding,
        7 => RecordKind::Patch,
        8 => RecordKind::Attributes,
        _ => return Err(Error::Corrupt),
    };
    let path_len = get_u16(&bytes, 24) as usize;
    let secondary_len = get_u16(&bytes, 26) as usize;
    let data_len = get_u32(&bytes, 28) as usize;
    let end = RECORD_HEADER_SIZE + path_len + secondary_len + data_len;
    if end > bytes.len() {
        return Err(Error::Corrupt);
    }
    let mut cursor = RECORD_HEADER_SIZE;
    let path = core::str::from_utf8(&bytes[cursor..cursor + path_len])
        .map_err(|_| Error::Utf8)?
        .to_string();
    cursor += path_len;
    let secondary = core::str::from_utf8(&bytes[cursor..cursor + secondary_len])
        .map_err(|_| Error::Utf8)?
        .to_string();
    cursor += secondary_len;
    let data = bytes[cursor..cursor + data_len].to_vec();
    Ok((
        Record {
            kind,
            txid: get_u64(&bytes, 8),
            generation: get_u64(&bytes, 16),
            object: get_u64(&bytes, 40),
            logical_offset: get_u64(&bytes, 48),
            path,
            secondary,
            data,
        },
        blocks,
    ))
}

fn apply(entries: &mut BTreeMap<String, Node>, mutation: Mutation) -> Result<(), Error> {
    match mutation {
        Mutation::Put(path, data) => {
            let attributes = entries
                .get(&path)
                .map(Node::attributes)
                .unwrap_or_else(|| Attributes::new(false, 0));
            entries.insert(path, Node::File(data, attributes));
        }
        Mutation::Patch(path, offset, patch) => {
            let Some(Node::File(data, _)) = entries.get_mut(&path) else {
                return Err(Error::Corrupt);
            };
            if offset > data.len() {
                return Err(Error::Corrupt);
            }
            let end = offset
                .checked_add(patch.len())
                .filter(|end| *end <= MAX_RANGE_FILE_BYTES)
                .ok_or(Error::Corrupt)?;
            if end > data.len() {
                data.try_reserve_exact(end - data.len())
                    .map_err(|_| Error::NoSpace)?;
                data.resize(end, 0);
            }
            data[offset..end].copy_from_slice(&patch);
        }
        Mutation::Attributes(path, attributes) => {
            match entries.get_mut(&path).ok_or(Error::Corrupt)? {
                Node::File(_, current) | Node::Directory(current) => *current = attributes,
            }
        }
        Mutation::Mkdir(path) => {
            entries.insert(path, Node::Directory(Attributes::new(true, 0)));
        }
        Mutation::Remove(path) => {
            let prefix = path.clone() + "/";
            entries.retain(|candidate, _| candidate != &path && !candidate.starts_with(&prefix));
        }
        Mutation::Rename(from, to) => {
            let mut moved = Vec::new();
            let prefix = from.clone() + "/";
            for (candidate, node) in entries.iter() {
                if candidate == &from || candidate.starts_with(&prefix) {
                    let suffix = &candidate[from.len()..];
                    moved.push((candidate.clone(), to.clone() + suffix, node.clone()));
                }
            }
            if moved.is_empty() {
                return Err(Error::NotFound);
            }
            for (old, _, _) in &moved {
                entries.remove(old);
            }
            for (_, new, node) in moved {
                entries.insert(new, node);
            }
        }
    }
    Ok(())
}

fn mutation_touches(mutation: &Mutation, path: &str) -> bool {
    match mutation {
        Mutation::Put(candidate, _)
        | Mutation::Patch(candidate, _, _)
        | Mutation::Attributes(candidate, _)
        | Mutation::Mkdir(candidate)
        | Mutation::Remove(candidate) => candidate == path,
        Mutation::Rename(from, to) => {
            subtree_suffix(path, from).is_some() || subtree_suffix(path, to).is_some()
        }
    }
}

fn subtree_suffix<'a>(path: &'a str, root: &str) -> Option<&'a str> {
    path.strip_prefix(root)
        .filter(|suffix| suffix.is_empty() || suffix.starts_with('/'))
}

fn copy_bytes(bytes: &[u8]) -> Result<Vec<u8>, Error> {
    let mut output = Vec::new();
    output
        .try_reserve_exact(bytes.len())
        .map_err(|_| Error::NoSpace)?;
    output.extend_from_slice(bytes);
    Ok(output)
}

fn copy_string(value: &str) -> Result<String, Error> {
    let mut output = String::new();
    output
        .try_reserve_exact(value.len())
        .map_err(|_| Error::NoSpace)?;
    output.push_str(value);
    Ok(output)
}

pub fn normalize(path: &str) -> Result<String, Error> {
    if !path.starts_with('/') || path.as_bytes().contains(&0) {
        return Err(Error::Invalid);
    }
    let mut result = String::new();
    for component in path.split('/').filter(|part| !part.is_empty()) {
        if component == "." {
            continue;
        }
        if component == ".." {
            return Err(Error::Invalid);
        }
        if component.len() > 255 {
            return Err(Error::NameTooLong);
        }
        if result
            .len()
            .checked_add(component.len() + 1)
            .is_none_or(|length| length > MAX_PATH_BYTES)
        {
            return Err(Error::NameTooLong);
        }
        result.push('/');
        result.push_str(component);
    }
    if result.is_empty() {
        result.push('/');
    }
    Ok(result)
}

fn parent(path: &str) -> &str {
    match path.rfind('/') {
        Some(0) => "/",
        Some(index) => &path[..index],
        None => "/",
    }
}

fn arena_len(total_blocks: u64) -> u64 {
    (total_blocks - SUPERBLOCK_COPIES) / 2
}
fn arena_start(total_blocks: u64, arena: u8) -> u64 {
    SUPERBLOCK_COPIES + arena as u64 * arena_len(total_blocks)
}
fn arena_end(total_blocks: u64, arena: u8) -> u64 {
    arena_start(total_blocks, arena) + arena_len(total_blocks)
}

fn segment_count(total_blocks: u64) -> Result<u16, Error> {
    let count = total_blocks
        .saturating_sub(SUPERBLOCK_COPIES)
        .checked_div(SEGMENT_BLOCKS)
        .ok_or(Error::Corrupt)?;
    if count < 2 || count > MAX_SEGMENTS as u64 {
        return Err(Error::Corrupt);
    }
    Ok(count as u16)
}

fn segment_start(index: u16) -> u64 {
    SUPERBLOCK_COPIES + index as u64 * SEGMENT_BLOCKS
}

fn protection_for(superblock: Superblock) -> Result<SegmentDirectory, Error> {
    if superblock.features & FEATURE_SEGMENT_DIRECTORY != 0 {
        return Ok(superblock.directory);
    }
    let mut protected = SegmentDirectory::EMPTY;
    if superblock.log_head == superblock.checkpoint {
        return Ok(protected);
    }
    for index in 0..segment_count(superblock.total_blocks)? {
        let start = segment_start(index);
        let end = start + SEGMENT_BLOCKS;
        if start < superblock.log_head && end > superblock.checkpoint {
            protected.push(SegmentRef {
                index,
                used: SEGMENT_BLOCKS as u16,
            })?;
        }
    }
    Ok(protected)
}

fn merge_protection(
    left: SegmentDirectory,
    right: SegmentDirectory,
) -> Result<SegmentDirectory, Error> {
    let mut merged = left;
    for entry in right.as_slice().iter().copied() {
        if !merged.contains(entry.index) {
            merged.push(SegmentRef {
                index: entry.index,
                used: SEGMENT_BLOCKS as u16,
            })?;
        }
    }
    Ok(merged)
}

fn validate_head(superblock: Superblock) -> Result<(), Error> {
    if superblock.features & FEATURE_SEGMENT_DIRECTORY != 0 {
        segment_count(superblock.total_blocks)?;
        return Ok(());
    }
    let start = arena_start(superblock.total_blocks, superblock.arena);
    let end = arena_end(superblock.total_blocks, superblock.arena);
    if superblock.checkpoint < start
        || superblock.checkpoint > superblock.log_head
        || superblock.log_head > end
    {
        Err(Error::Corrupt)
    } else {
        Ok(())
    }
}

fn object_number(path: &str) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in path.bytes() {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x100_0000_01b3);
    }
    hash
}

pub fn crc32c(bytes: &[u8]) -> u32 {
    !crc32c_update(!0u32, bytes)
}

fn crc32c_update(mut crc: u32, bytes: &[u8]) -> u32 {
    for &byte in bytes {
        crc ^= byte as u32;
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0x82f6_3b78 & mask);
        }
    }
    crc
}

fn put_u16(out: &mut [u8], offset: usize, value: u16) {
    out[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
}
fn put_u32(out: &mut [u8], offset: usize, value: u32) {
    out[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}
fn put_u64(out: &mut [u8], offset: usize, value: u64) {
    out[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
}
fn get_u16(input: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes(input[offset..offset + 2].try_into().unwrap())
}
fn get_u32(input: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(input[offset..offset + 4].try_into().unwrap())
}
fn get_u64(input: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes(input[offset..offset + 8].try_into().unwrap())
}
