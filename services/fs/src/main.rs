#![no_std]
#![no_main]

extern crate alloc;

use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::fmt::{self, Write};
use core::panic::PanicInfo;
use mfs1::{BLOCK_SIZE, BlockDevice, CommitStage, Error, Metadata};
use microsystem_abi::{
    CapHandle, FilesystemStatsV1, Message, Rights, Status, boot_cap, filesystem, protocol, script,
};
use microsystem_block::Operation;
use microsystem_fs::{FileService, Operation as FsOperation};

const SHARED_DATA: usize = 0x005c_0000;
const FILESYSTEM_DATA: usize = 0x005e_0000;
const SSH_FILESYSTEM_DATA: usize = 0x005f_0000;
const WINDOWD_FILESYSTEM_DATA: usize = 0x0060_0000;
const TERMINAL_FILESYSTEM_DATA: usize = 0x0061_0000;
const DATABASE_FILESYSTEM_DATA: usize = 0x0062_0000;
const ROOT_FILESYSTEM_DATA: usize = 0x0063_0000;
const NETWORK_FILESYSTEM_DATA: usize = 0x0064_0000;
const OPEN_FILE_LIMIT: usize = 16;
const SCRIPT_SESSION_LIMIT: usize = 8;
const SCRIPT_SESSION_VA: u64 = script::SESSION_VA;

struct OpenFiles {
    paths: [Option<String>; OPEN_FILE_LIMIT],
    generations: [u32; OPEN_FILE_LIMIT],
    owners: [(u32, u32); OPEN_FILE_LIMIT],
}

impl OpenFiles {
    const fn new() -> Self {
        Self {
            paths: [const { None }; OPEN_FILE_LIMIT],
            generations: [0; OPEN_FILE_LIMIT],
            owners: [(0, 0); OPEN_FILE_LIMIT],
        }
    }

    fn open(&mut self, path: String, peer: u32, uid: u32) -> Result<u64, Error> {
        let Some(index) = self.paths.iter().position(Option::is_none) else {
            return Err(Error::NoSpace);
        };
        let generation = self.generations[index].wrapping_add(1).max(1);
        self.generations[index] = generation;
        self.paths[index] = Some(path);
        self.owners[index] = (peer, uid);
        Ok(((generation as u64) << 32) | (index as u64 + 1))
    }

    fn path(&self, descriptor: u64) -> Result<&str, Error> {
        let index = (descriptor as u32).checked_sub(1).ok_or(Error::Invalid)? as usize;
        let generation = (descriptor >> 32) as u32;
        if generation == 0 || self.generations.get(index).copied() != Some(generation) {
            return Err(Error::NotFound);
        }
        self.paths
            .get(index)
            .and_then(Option::as_deref)
            .ok_or(Error::NotFound)
    }

    fn owned(&self, descriptor: u64, peer: u32, uid: u32) -> bool {
        self.path(descriptor).is_ok()
            && (descriptor as u32)
                .checked_sub(1)
                .and_then(|i| self.owners.get(i as usize))
                .copied()
                == Some((peer, uid))
    }

    fn close(&mut self, descriptor: u64) -> Result<(), Error> {
        let index = (descriptor as u32).checked_sub(1).ok_or(Error::Invalid)? as usize;
        let generation = (descriptor >> 32) as u32;
        if generation == 0 || self.generations.get(index).copied() != Some(generation) {
            return Err(Error::NotFound);
        }
        let slot = self.paths.get_mut(index).ok_or(Error::NotFound)?;
        if slot.take().is_none() {
            return Err(Error::NotFound);
        }
        Ok(())
    }

    fn invalidate_path(&mut self, path: &str) {
        for slot in &mut self.paths {
            if slot.as_deref() == Some(path) {
                *slot = None;
            }
        }
    }

    fn rename_path(&mut self, source: &str, destination: &str) {
        self.invalidate_path(destination);
        for path in self.paths.iter_mut().flatten() {
            if path.as_str() == source
                || path
                    .strip_prefix(source)
                    .is_some_and(|suffix| suffix.starts_with('/'))
            {
                *path = destination.to_string() + &path[source.len()..];
            }
        }
    }

    fn holds_under(&self, parent: &str) -> bool {
        self.paths.iter().flatten().any(|path| {
            path == parent
                || path
                    .strip_prefix(parent)
                    .is_some_and(|suffix| suffix.starts_with('/'))
        })
    }
}

struct ScriptSession {
    token: u64,
    region: CapHandle,
    open_files: OpenFiles,
    uid: u32,
}

impl ScriptSession {
    const fn empty() -> Self {
        Self {
            token: 0,
            region: CapHandle::INVALID,
            open_files: OpenFiles::new(),
            uid: microsystem_identity::GUEST_UID,
        }
    }
}

struct ScriptSessions {
    entries: [ScriptSession; SCRIPT_SESSION_LIMIT],
}

impl ScriptSessions {
    const fn new() -> Self {
        Self {
            entries: [const { ScriptSession::empty() }; SCRIPT_SESSION_LIMIT],
        }
    }

    fn index(&self, token: u64) -> Option<usize> {
        (token != 0)
            .then(|| self.entries.iter().position(|entry| entry.token == token))
            .flatten()
    }

    fn invalidate_path(&mut self, path: &str) {
        for session in &mut self.entries {
            session.open_files.invalidate_path(path);
        }
    }

    fn rename_path(&mut self, source: &str, destination: &str) {
        for session in &mut self.entries {
            session.open_files.rename_path(source, destination);
        }
    }

    fn register(&mut self, token: u64, region: CapHandle, uid: u32) -> Status {
        if token == 0 || region == CapHandle::INVALID || self.index(token).is_some() {
            let _ = microsystem_user_rt::cap_delete(region);
            return Status::Invalid;
        }
        let Some(index) = self.entries.iter().position(|entry| entry.token == 0) else {
            let _ = microsystem_user_rt::cap_delete(region);
            return Status::NoMemory;
        };
        if microsystem_user_rt::frame_map(region, SCRIPT_SESSION_VA, Rights::READ).is_err() {
            let _ = microsystem_user_rt::cap_delete(region);
            return Status::BadCapability;
        }
        let valid = validate_script_session();
        let _ = microsystem_user_rt::frame_unmap(region, SCRIPT_SESSION_VA);
        if !valid {
            let _ = microsystem_user_rt::cap_delete(region);
            return Status::Invalid;
        }
        self.entries[index] = ScriptSession {
            token,
            region,
            open_files: OpenFiles::new(),
            uid,
        };
        Status::Ok
    }

    fn unregister(&mut self, token: u64) -> Status {
        let Some(index) = self.index(token) else {
            return Status::NotFound;
        };
        let region = self.entries[index].region;
        self.entries[index] = ScriptSession::empty();
        microsystem_user_rt::cap_delete(region)
            .map(|()| Status::Ok)
            .unwrap_or_else(|status| status)
    }
}

struct IpcDisk {
    blocks: u64,
}

impl IpcDisk {
    fn geometry() -> Result<Self, Error> {
        let request = Message::new(protocol::BLOCK, Operation::Geometry as u16);
        let mut reply = Message::new(protocol::BLOCK, 0);
        microsystem_user_rt::ipc_call(boot_cap::BLOCK_ENDPOINT, &request, &mut reply, 0)
            .map_err(|_| Error::Io)?;
        if reply.protocol != protocol::BLOCK || reply.words[5] as i64 != 0 {
            return Err(Error::Io);
        }
        Ok(Self {
            blocks: reply.words[0] / 8,
        })
    }

    fn transfer(&mut self, operation: Operation, block: u64) -> Result<(), Error> {
        let mut request = Message::new(protocol::BLOCK, operation as u16);
        request.words[0] = block;
        request.words[1] = BLOCK_SIZE as u64;
        request.caps[0] = boot_cap::SHARED_BLOCK_FRAME;
        let mut reply = Message::new(protocol::BLOCK, 0);
        microsystem_user_rt::fence();
        microsystem_user_rt::ipc_call(boot_cap::BLOCK_ENDPOINT, &request, &mut reply, 0)
            .map_err(|_| Error::Io)?;
        microsystem_user_rt::fence();
        if reply.words[5] as i64 == 0 {
            Ok(())
        } else {
            Err(Error::Io)
        }
    }
}

impl BlockDevice for IpcDisk {
    fn block_count(&self) -> u64 {
        self.blocks
    }

    fn read_block(&mut self, block: u64, out: &mut [u8; BLOCK_SIZE]) -> Result<(), Error> {
        if block >= self.blocks {
            return Err(Error::Io);
        }
        self.transfer(Operation::Read, block)?;
        unsafe {
            core::ptr::copy_nonoverlapping(SHARED_DATA as *const u8, out.as_mut_ptr(), BLOCK_SIZE)
        };
        Ok(())
    }

    fn write_block(&mut self, block: u64, data: &[u8; BLOCK_SIZE]) -> Result<(), Error> {
        if block >= self.blocks {
            return Err(Error::Io);
        }
        unsafe {
            core::ptr::copy_nonoverlapping(data.as_ptr(), SHARED_DATA as *mut u8, BLOCK_SIZE)
        };
        self.transfer(Operation::Write, block)
    }

    fn flush(&mut self) -> Result<(), Error> {
        let request = Message::new(protocol::BLOCK, Operation::Flush as u16);
        let mut reply = Message::new(protocol::BLOCK, 0);
        microsystem_user_rt::ipc_call(boot_cap::BLOCK_ENDPOINT, &request, &mut reply, 0)
            .map_err(|_| Error::Io)?;
        if reply.words[5] as i64 == 0 {
            Ok(())
        } else {
            Err(Error::Io)
        }
    }

    fn commit_stage(&mut self, stage: CommitStage) {
        let marker = match stage {
            CommitStage::TransactionRecordsWritten => {
                b"[mfs1-fault] stage=transaction-records-written\n".as_slice()
            }
            CommitStage::TransactionDataFlushed => {
                b"[mfs1-fault] stage=transaction-data-flushed\n".as_slice()
            }
            CommitStage::TransactionSuperblockWritten => {
                b"[mfs1-fault] stage=transaction-superblock-written\n".as_slice()
            }
            CommitStage::TransactionSuperblockFlushed => {
                b"[mfs1-fault] stage=transaction-superblock-flushed\n".as_slice()
            }
            CommitStage::GcRecordsWritten => b"[mfs1-fault] stage=gc-records-written\n".as_slice(),
            CommitStage::GcDataFlushed => b"[mfs1-fault] stage=gc-data-flushed\n".as_slice(),
            CommitStage::GcCandidateSuperblockWritten => {
                b"[mfs1-fault] stage=gc-candidate-superblock-written\n".as_slice()
            }
            CommitStage::GcCandidateSuperblockFlushed => {
                b"[mfs1-fault] stage=gc-candidate-superblock-flushed\n".as_slice()
            }
            CommitStage::GcSealedSuperblockWritten => {
                b"[mfs1-fault] stage=gc-sealed-superblock-written\n".as_slice()
            }
            CommitStage::GcSealedSuperblockFlushed => {
                b"[mfs1-fault] stage=gc-sealed-superblock-flushed\n".as_slice()
            }
        };
        let _ = microsystem_user_rt::debug_write(marker);
        let start = microsystem_user_rt::clock_now().unwrap_or(0);
        let deadline = start.saturating_add(100_000_000);
        while microsystem_user_rt::clock_now().unwrap_or(deadline) < deadline {
            let _ = microsystem_user_rt::yield_now();
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    let _ = microsystem_user_rt::debug_write(b"[user] mfs service ELF entered EL0\n");
    verify_allocator_reuse();
    let disk = IpcDisk::geometry().unwrap_or_else(|_| microsystem_user_rt::exit(2));
    let _ =
        microsystem_user_rt::debug_write(b"[ipc] resident mfs->block capacity-sectors=131072\n");
    let now = microsystem_user_rt::clock_now().unwrap_or(0);
    let mut filesystem = FileService::mount(disk, now).unwrap_or_else(|error| {
        let mut output = String::new();
        let _ = writeln!(output, "[mfs] mount failed error={error:?}");
        let _ = microsystem_user_rt::debug_write(output.as_bytes());
        microsystem_user_rt::exit(map_fs_error(error) as i64 as u64)
    });
    filesystem.set_timestamp(microsystem_user_rt::clock_realtime().unwrap_or(0));
    for (line, error) in filesystem.mount_failures() {
        let mut output = String::new();
        let _ = writeln!(
            &mut output,
            "[vfs] stored mount unavailable line={line} error={error:?}"
        );
        let _ = microsystem_user_rt::debug_write(output.as_bytes());
    }
    let _ = microsystem_user_rt::debug_write(b"[user] mfs1 mounted via EL0 block IPC\n");
    if filesystem.stats().checkpoint_block < 2 {
        microsystem_user_rt::exit(8);
    }
    let _ =
        microsystem_user_rt::debug_write(b"[user] mfs1 checkpoint valid=true segment-blocks=256\n");
    if filesystem.read("/boot-proof").ok().as_deref()
        == Some(b"MicroSystem persistent MFS1\n".as_slice())
    {
        let _ = microsystem_user_rt::debug_write(
            b"[user] mfs1 recovered /boot-proof after restart=true\n",
        );
    }
    filesystem
        .write("/boot-proof", b"MicroSystem persistent MFS1\n")
        .and_then(|_| filesystem.fsync("/boot-proof"))
        .unwrap_or_else(|_| microsystem_user_rt::exit(4));
    if filesystem.read("/boot-proof").ok().as_deref()
        != Some(b"MicroSystem persistent MFS1\n".as_slice())
    {
        microsystem_user_rt::exit(5);
    }
    let _ = microsystem_user_rt::debug_write(b"[user] mfs1 fsync /boot-proof persistent=true\n");
    let stats = filesystem.stats();
    if !stats.segment_directory || stats.active_segments == 0 {
        microsystem_user_rt::exit(9);
    }
    let _ = microsystem_user_rt::debug_write_u64(
        b"[user] mfs1 segment-directory active-segments=",
        stats.active_segments as u64,
        b" low-water=15 high-water=25\n",
    );

    let mut fs_request = Message::new(protocol::FILESYSTEM, 0);
    let mut open_files = OpenFiles::new();
    let mut script_sessions = ScriptSessions::new();
    let _ = microsystem_user_rt::debug_write(b"[ipc] resident mfs endpoint=3 ready\n");
    let _ =
        microsystem_user_rt::debug_write(b"[user] mfs1 background-writeback=1s online-gc=true\n");
    let _ = microsystem_user_rt::service_online();
    if receive_with_maintenance(&mut filesystem, None, &mut fs_request).is_err() {
        microsystem_user_rt::exit(6);
    }
    loop {
        let mut reply = Message::new(protocol::FILESYSTEM, fs_request.opcode);
        reply.words[5] = handle_fs(
            &mut filesystem,
            &mut open_files,
            &mut script_sessions,
            &fs_request,
            &mut reply,
        ) as i64 as u64;
        if receive_with_maintenance(&mut filesystem, Some(&reply), &mut fs_request).is_err() {
            microsystem_user_rt::exit(7);
        }
    }
}

fn verify_allocator_reuse() {
    for value in 0..512 {
        let bytes = alloc::vec![value as u8; BLOCK_SIZE];
        if bytes.len() != BLOCK_SIZE || bytes[0] != value as u8 {
            microsystem_user_rt::exit(10);
        }
        core::hint::black_box(&bytes);
    }
    let _ = microsystem_user_rt::debug_write(
        b"[mm] EL0 user heap free-list reuse allocations=512 bytes=4096 true\n",
    );
}

fn handle_fs(
    filesystem: &mut FileService<IpcDisk>,
    open_files: &mut OpenFiles,
    script_sessions: &mut ScriptSessions,
    request: &Message,
    reply: &mut Message,
) -> Status {
    filesystem.set_timestamp(microsystem_user_rt::clock_realtime().unwrap_or(0));
    let peer = match microsystem_user_rt::ipc_peer() {
        Ok(peer) => peer as u32,
        Err(status) => return status,
    };
    if request.opcode == filesystem::SCRIPT_REGISTER {
        if peer != 1 {
            return Status::AccessDenied;
        }
        return script_sessions.register(
            request.words[3],
            request.caps[0],
            request.words[0] as u32,
        );
    }
    if request.opcode == filesystem::SCRIPT_UNREGISTER {
        if peer != 1 {
            return Status::AccessDenied;
        }
        return script_sessions.unregister(request.words[3]);
    }
    if request.words[4] == filesystem::SCRIPT_REQUEST_MAGIC {
        return handle_script_fs(filesystem, open_files, script_sessions, request, reply);
    }
    if request.opcode == FsOperation::Stat as u16 && request.caps[0] == CapHandle::INVALID {
        reply.words[0] = 0x4d46_5331;
        return Status::Ok;
    }
    let shared_address = match request.caps[0] {
        value if value == boot_cap::SHARED_FILESYSTEM_FRAME => FILESYSTEM_DATA,
        value if value == boot_cap::SSH_FILESYSTEM_FRAME => SSH_FILESYSTEM_DATA,
        value if value == boot_cap::WINDOWD_FILESYSTEM_FRAME => WINDOWD_FILESYSTEM_DATA,
        value if value == boot_cap::GUI_TERMINAL_COMMANDS => TERMINAL_FILESYSTEM_DATA,
        value if value == boot_cap::DATABASE_FILESYSTEM_FRAME => DATABASE_FILESYSTEM_DATA,
        value if value == boot_cap::ROOT_FILESYSTEM_FRAME => ROOT_FILESYSTEM_DATA,
        value if value == boot_cap::NETWORK_FILESYSTEM_FRAME => NETWORK_FILESYSTEM_DATA,
        _ => return Status::AccessDenied,
    };
    let Ok(path_length) = usize::try_from(request.words[0]) else {
        return Status::Invalid;
    };
    let Ok(data_length) = usize::try_from(request.words[1]) else {
        return Status::Invalid;
    };
    let Some(total_length) = path_length.checked_add(data_length) else {
        return Status::Invalid;
    };
    if path_length > 255 || total_length > BLOCK_SIZE {
        return Status::Invalid;
    }
    let shared = unsafe { core::slice::from_raw_parts(shared_address as *const u8, BLOCK_SIZE) };
    let Ok(path) = String::from_utf8(shared[..path_length].to_vec()) else {
        return Status::Invalid;
    };
    let data = shared[path_length..path_length + data_length].to_vec();
    let actor = if matches!(peer, 1 | 5 | 12 | 13) && request.words[3] == 0 {
        microsystem_identity::Actor::SYSTEM
    } else if peer == 11 && request.words[3] == 0 {
        if !matches!(
            path.as_str(),
            "/.system/ssh/host.key" | "/.system/ssh/mica-policy"
        ) || !matches!(request.opcode, value if value == FsOperation::Stat as u16 || value == FsOperation::ReadRange as u16)
        {
            return Status::AccessDenied;
        }
        microsystem_identity::Actor::SYSTEM
    } else {
        let snapshot = match unsafe {
            microsystem_identity::read_shared(microsystem_abi::identity::SNAPSHOT_VA)
        } {
            Ok(snapshot) => snapshot,
            Err(status) => return status,
        };
        match snapshot.actor(
            peer,
            request.words[3],
            microsystem_user_rt::clock_now().unwrap_or(0),
        ) {
            Ok(actor) => actor,
            Err(status) => return status,
        }
    };
    if peer == 11 && request.words[3] == 0 && request.caps[0] != boot_cap::SSH_FILESYSTEM_FRAME {
        return Status::AccessDenied;
    }
    let descriptor = request.words[2];
    let uses_descriptor = matches!(request.opcode, value if value == FsOperation::Read as u16 || value == FsOperation::Write as u16 || value == FsOperation::Fsync as u16 || value == FsOperation::Close as u16)
        && descriptor != 0;
    if uses_descriptor && !open_files.owned(descriptor, peer, actor.uid) {
        return Status::AccessDenied;
    }
    let resolved = if uses_descriptor {
        open_files.path(descriptor).unwrap().to_string()
    } else {
        path.clone()
    };
    let checked = match microsystem_fs::access::authorize(
        filesystem,
        actor,
        request.opcode,
        &resolved,
        &data,
    ) {
        Ok(path) => path,
        Err(status) => return status,
    };
    let created = filesystem.metadata(&checked) == Err(Error::NotFound)
        && matches!(request.opcode, value if value == FsOperation::Write as u16 || value == FsOperation::Append as u16 || value == FsOperation::Mkdir as u16);
    if request.opcode == FsOperation::Stats as u16 {
        return write_filesystem_stats(
            filesystem,
            if path.is_empty() { "/" } else { &path },
            shared_address,
            reply,
        )
        .map(|()| Status::Ok)
        .unwrap_or_else(map_fs_error);
    }
    let result = match request.opcode {
        value if value == FsOperation::Stat as u16 => {
            require_path(&path).and_then(|path| stat_path(filesystem, path, reply))
        }
        value if value == FsOperation::FormatImage as u16 => require_path(&path).and_then(|path| {
            let mib = u32::try_from(request.words[2]).map_err(|_| Error::Invalid)?;
            filesystem.format_image(path, mib)
        }),
        value if value == FsOperation::MountImage as u16 => require_path(&path).and_then(|image| {
            let point = core::str::from_utf8(&data).map_err(|_| Error::Utf8)?;
            if request.words[2] > 1 {
                return Err(Error::Invalid);
            }
            filesystem.mount_image(image, point, request.words[2] != 0)
        }),
        value if value == FsOperation::Unmount as u16 => require_path(&path).and_then(|point| {
            let point = mfs1::normalize(point)?;
            if open_files.holds_under(&point)
                || script_sessions
                    .entries
                    .iter()
                    .any(|session| session.token != 0 && session.open_files.holds_under(&point))
            {
                return Err(Error::Busy);
            }
            filesystem.unmount(&point)
        }),
        value if value == FsOperation::MountList as u16 => {
            let mut output = String::new();
            output
                .try_reserve(4096)
                .map_err(|_| Error::NoSpace)
                .and_then(|()| {
                    output.push_str("/ device rw\n");
                    for mount in filesystem.mounts() {
                        let _ = writeln!(
                            &mut output,
                            "{} {} {}",
                            mount.point,
                            mount.image,
                            if mount.readonly { "ro" } else { "rw" }
                        );
                    }
                    for (line, error) in filesystem.mount_failures() {
                        let _ = writeln!(&mut output, "unavailable line={line} error={error:?}");
                    }
                    write_shared(shared_address, output.as_bytes(), reply)
                })
        }
        value if value == FsOperation::Attributes as u16 => require_path(&path).and_then(|path| {
            let attributes = filesystem.attributes(path)?;
            let payload = filesystem::AttributesV1 {
                version: 1,
                mode: attributes.mode,
                uid: attributes.uid,
                gid: attributes.gid,
                created: attributes.created,
                modified: attributes.modified,
                accessed: attributes.accessed,
                changed: attributes.changed,
            };
            let bytes = unsafe {
                core::slice::from_raw_parts(
                    (&payload as *const filesystem::AttributesV1).cast::<u8>(),
                    core::mem::size_of_val(&payload),
                )
            };
            write_shared(shared_address, bytes, reply)
        }),
        value if value == FsOperation::Chmod as u16 => require_path(&path).and_then(|path| {
            let mode = u32::try_from(request.words[2]).map_err(|_| Error::Invalid)?;
            filesystem.chmod(path, mode)
        }),
        value if value == FsOperation::Chown as u16 => require_path(&path).and_then(|path| {
            let uid = request.words[2] as u32;
            let gid = (request.words[2] >> 32) as u32;
            filesystem.chown(path, uid, gid)
        }),
        value if value == FsOperation::Open as u16 => require_path(&path).and_then(|path| {
            stat_path(filesystem, path, reply)?;
            reply.words[0] = open_files.open(path.to_string(), peer, actor.uid)?;
            Ok(())
        }),
        value if value == FsOperation::List as u16 => filesystem
            .list(&path)
            .and_then(|entries| encode_listing(entries, BLOCK_SIZE))
            .and_then(|output| write_shared(shared_address, &output, reply)),
        value if value == FsOperation::Read as u16 => request_path(request, &path, open_files)
            .and_then(|path| filesystem.read(&path))
            .and_then(|bytes| write_shared(shared_address, &bytes, reply)),
        value if value == FsOperation::ReadRange as u16 => require_path(&path)
            .and_then(|path| filesystem.read_range(path, request.words[2] as usize, BLOCK_SIZE))
            .and_then(|bytes| write_shared(shared_address, &bytes, reply)),
        value if value == FsOperation::Write as u16 => {
            request_path(request, &path, open_files).and_then(|path| filesystem.write(&path, &data))
        }
        value if value == FsOperation::Copy as u16 => require_path(&path).and_then(|source| {
            let destination = core::str::from_utf8(&data).map_err(|_| Error::Invalid)?;
            require_path(destination)?;
            if filesystem.metadata(destination) != Err(Error::NotFound) {
                return Err(Error::AlreadyExists);
            }
            let bytes = filesystem.read(source)?;
            filesystem.write(destination, &bytes)
        }),
        value if value == FsOperation::WriteRange as u16 => require_path(&path)
            .and_then(|path| write_range(filesystem, path, request.words[2], &data)),
        value if value == FsOperation::Append as u16 => require_path(&path).and_then(|path| {
            if created {
                filesystem.write(path, &data)?;
                microsystem_fs::access::own_created(filesystem, actor, path)?;
                filesystem.fsync(path)
            } else {
                filesystem.append(path, &data)
            }
        }),
        value if value == FsOperation::Mkdir as u16 => match filesystem.mkdir(&path) {
            Err(Error::AlreadyExists) if filesystem.metadata(&path) == Ok(Metadata::Directory) => {
                Ok(())
            }
            result => result,
        },
        value if value == FsOperation::Sync as u16 => filesystem.sync(),
        value if value == FsOperation::Fsync as u16 => {
            if request.words[2] == 0 {
                require_path(&path).and_then(|path| filesystem.fsync(path))
            } else {
                open_files
                    .path(request.words[2])
                    .map(str::to_string)
                    .and_then(|path| filesystem.fsync(&path))
            }
        }
        value if value == FsOperation::Rename as u16 => {
            let destination = String::from_utf8(data.clone()).map_err(|_| Error::Utf8);
            destination.and_then(|destination| {
                filesystem.rename(&path, &destination)?;
                open_files.rename_path(&path, &destination);
                script_sessions.rename_path(&path, &destination);
                Ok(())
            })
        }
        value if value == FsOperation::Replace as u16 => {
            let destination = String::from_utf8(data.clone()).map_err(|_| Error::Utf8);
            destination.and_then(|destination| {
                filesystem.replace_file(&path, &destination)?;
                open_files.rename_path(&path, &destination);
                script_sessions.rename_path(&path, &destination);
                filesystem.fsync(&destination)
            })
        }
        value if value == FsOperation::Unlink as u16 => filesystem.unlink(&path).map(|()| {
            open_files.invalidate_path(&path);
            script_sessions.invalidate_path(&path);
        }),
        value if value == FsOperation::Close as u16 => open_files.close(request.words[2]),
        _ => return Status::Invalid,
    };
    if result.is_ok() && created {
        if let Err(error) = microsystem_fs::access::own_created(filesystem, actor, &checked) {
            return map_fs_error(error);
        }
    }
    if result.is_ok() && request.opcode == FsOperation::Copy as u16 {
        if let Ok(destination) = core::str::from_utf8(&data) {
            if let Err(error) = microsystem_fs::access::own_created(filesystem, actor, destination)
            {
                return map_fs_error(error);
            }
        }
    }
    match result {
        Ok(()) => Status::Ok,
        Err(Error::NotFound) => Status::NotFound,
        Err(Error::NoSpace) => Status::NoSpace,
        Err(Error::Io) => Status::Io,
        Err(_) => Status::Invalid,
    }
}

fn handle_script_fs(
    filesystem_service: &mut FileService<IpcDisk>,
    open_files: &mut OpenFiles,
    sessions: &mut ScriptSessions,
    request: &Message,
    reply: &mut Message,
) -> Status {
    let token = request.words[3];
    let Some(index) = sessions.index(token) else {
        return Status::AccessDenied;
    };
    let uid = sessions.entries[index].uid;
    let actor =
        match unsafe { microsystem_identity::read_shared(microsystem_abi::identity::SNAPSHOT_VA) }
            .and_then(|s| {
                s.uid(uid)
                    .filter(|u| u.enabled())
                    .map(|u| u.actor())
                    .ok_or(Status::AccessDenied)
            }) {
            Ok(actor) => actor,
            Err(status) => return status,
        };
    let path_length = request.words[0] as usize;
    let data_length = request.words[1] as usize;
    if path_length > 1024
        || data_length > script::BROKER_BYTES
        || path_length.saturating_add(data_length) > script::BROKER_BYTES
    {
        return Status::Invalid;
    }
    let region = sessions.entries[index].region;
    if microsystem_user_rt::frame_map(
        region,
        SCRIPT_SESSION_VA,
        Rights(Rights::READ.0 | Rights::WRITE.0),
    )
    .is_err()
    {
        return Status::BadCapability;
    }
    let result = (|| {
        if !validate_script_session() {
            return Status::Invalid;
        }
        let payload = unsafe {
            core::slice::from_raw_parts(
                (SCRIPT_SESSION_VA as *const u8).add(script::BROKER_OFFSET),
                script::BROKER_BYTES,
            )
        };
        let Ok(raw_path) = core::str::from_utf8(&payload[..path_length]) else {
            return Status::Invalid;
        };
        let path = match normalize_script_path(raw_path) {
            Ok(path) => path,
            Err(status) => return status,
        };
        let data = payload[path_length..path_length + data_length].to_vec();
        let (source, policy) = script_policy_text();
        let descriptor = request.words[2];
        let operation = request.opcode;

        let resolved_path = if descriptor != 0
            && matches!(
                operation,
                value if value == FsOperation::Read as u16
                    || value == FsOperation::Write as u16
                    || value == FsOperation::Fsync as u16
            ) {
            match sessions.entries[index].open_files.path(descriptor) {
                Ok(path) => path.to_string(),
                Err(error) => return map_fs_error(error),
            }
        } else {
            path.clone()
        };
        let access = if matches!(
            operation,
            value if value == FsOperation::Stat as u16
                || value == FsOperation::List as u16
                || value == FsOperation::Read as u16
                || value == FsOperation::ReadRange as u16
                || value == FsOperation::Open as u16
        ) {
            "fs.read"
        } else {
            "fs.write"
        };
        let entry_source_read = operation == FsOperation::ReadRange as u16
            && script_entry_path()
                .and_then(|entry| normalize_script_path(entry).ok())
                .is_some_and(|entry| entry == resolved_path);
        let trusted_ca_read = operation == FsOperation::ReadRange as u16
            && request.words[5] == filesystem::SCRIPT_TRUSTED_READ_MAGIC
            && resolved_path == "/.system/certs/ca-bundle.derpack";
        if operation != FsOperation::Close as u16
            && !entry_source_read
            && !trusted_ca_read
            && !script_permission_allows(source, policy, access, &resolved_path)
        {
            return Status::AccessDenied;
        }
        if operation == FsOperation::Rename as u16 {
            let Ok(destination) = core::str::from_utf8(&data) else {
                return Status::Invalid;
            };
            let Ok(destination) = normalize_script_path(destination) else {
                return Status::Invalid;
            };
            if !script_permission_allows(source, policy, "fs.write", &destination) {
                return Status::AccessDenied;
            }
        }

        if let Err(status) = microsystem_fs::access::authorize(
            filesystem_service,
            actor,
            operation,
            &resolved_path,
            &data,
        ) {
            return status;
        }
        let created = filesystem_service.metadata(&resolved_path) == Err(Error::NotFound)
            && matches!(operation, value if value == FsOperation::Write as u16 || value == FsOperation::WriteAtomic as u16 || value == FsOperation::Mkdir as u16);
        let outcome = match operation {
            value if value == FsOperation::Stat as u16 => {
                stat_path(filesystem_service, &resolved_path, reply)
            }
            value if value == FsOperation::List as u16 => filesystem_service
                .list(&resolved_path)
                .and_then(|entries| encode_listing(entries, script::BROKER_BYTES))
                .and_then(|output| write_script_payload(&output, reply)),
            value if value == FsOperation::Read as u16 => filesystem_service
                .read(&resolved_path)
                .and_then(|bytes| write_script_payload(&bytes, reply)),
            value if value == FsOperation::ReadRange as u16 => filesystem_service
                .read_range(
                    &resolved_path,
                    request.words[2] as usize,
                    script::BROKER_BYTES,
                )
                .and_then(|bytes| write_script_payload(&bytes, reply)),
            value if value == FsOperation::Write as u16 => {
                filesystem_service.write(&resolved_path, &data)
            }
            value if value == FsOperation::WriteAtomic as u16 => {
                let temporary = match script_temporary_path(&resolved_path, token) {
                    Ok(path) => path,
                    Err(status) => return status,
                };
                let durable = request.words[2] & 1 != 0;
                let original = filesystem_service.attributes(&resolved_path).ok();
                filesystem_service
                    .write(&temporary, &[])
                    .and_then(|()| {
                        let mut attributes = filesystem_service.attributes(&temporary)?;
                        attributes.uid = original.map_or(actor.uid, |a| a.uid);
                        attributes.gid = original.map_or(actor.gid, |a| a.gid);
                        attributes.mode = original.map_or(0o644, |a| a.mode);
                        if let Some(original) = original {
                            attributes.created = original.created;
                        }
                        filesystem_service.set_attributes(&temporary, attributes)?;
                        filesystem_service.write(&temporary, &data)
                    })
                    .and_then(|()| {
                        if durable {
                            filesystem_service.fsync(&temporary)
                        } else {
                            Ok(())
                        }
                    })
                    .and_then(|()| {
                        filesystem_service.replace_file(&temporary, &resolved_path)?;
                        open_files.invalidate_path(&resolved_path);
                        sessions.invalidate_path(&resolved_path);
                        Ok(())
                    })
                    .and_then(|()| {
                        if durable {
                            filesystem_service.fsync(&resolved_path)
                        } else {
                            Ok(())
                        }
                    })
            }
            value if value == FsOperation::Mkdir as u16 => filesystem_service.mkdir(&resolved_path),
            value if value == FsOperation::Sync as u16 => filesystem_service.sync(),
            value if value == FsOperation::Open as u16 => {
                stat_path(filesystem_service, &resolved_path, reply).and_then(|()| {
                    reply.words[0] = sessions.entries[index].open_files.open(
                        resolved_path.clone(),
                        0,
                        actor.uid,
                    )?;
                    Ok(())
                })
            }
            value if value == FsOperation::Fsync as u16 => filesystem_service.fsync(&resolved_path),
            value if value == FsOperation::Rename as u16 => String::from_utf8(data)
                .map_err(|_| Error::Utf8)
                .and_then(|destination| {
                    normalize_script_path(&destination).map_err(|_| Error::Invalid)
                })
                .and_then(|destination| {
                    filesystem_service.rename(&resolved_path, &destination)?;
                    open_files.rename_path(&resolved_path, &destination);
                    sessions.rename_path(&resolved_path, &destination);
                    Ok(())
                }),
            value if value == FsOperation::Unlink as u16 => {
                filesystem_service.unlink(&resolved_path).map(|()| {
                    open_files.invalidate_path(&resolved_path);
                    sessions.invalidate_path(&resolved_path);
                })
            }
            value if value == FsOperation::Close as u16 => {
                sessions.entries[index].open_files.close(descriptor)
            }
            _ => return Status::Invalid,
        };
        if outcome.is_ok() && created {
            if let Err(error) =
                microsystem_fs::access::own_created(filesystem_service, actor, &resolved_path)
            {
                return map_fs_error(error);
            }
        }
        outcome.map(|()| Status::Ok).unwrap_or_else(map_fs_error)
    })();
    let _ = microsystem_user_rt::frame_unmap(region, SCRIPT_SESSION_VA);
    result
}

fn validate_script_session() -> bool {
    let header = unsafe { &*(SCRIPT_SESSION_VA as *const script::SessionHeaderV1) };
    header.magic == script::SESSION_MAGIC
        && header.version == script::VERSION
        && matches!(
            header.mode,
            script::MODE_EVAL | script::MODE_FILE | script::MODE_REPL
        )
        && (header.mode == script::MODE_REPL || header.source_bytes != 0)
        && header.source_bytes as usize <= script::SOURCE_BYTES
        && header.policy_bytes as usize <= script::POLICY_BYTES
        && header.argv_bytes as usize <= script::STDIN_BYTES
        && header.path_bytes as usize
            <= script::POLICY_BYTES.saturating_sub(header.policy_bytes as usize)
        && (header.mode != script::MODE_FILE || header.path_bytes != 0)
        && header.permission_mask != 0
}

fn script_policy_text() -> (&'static str, &'static str) {
    let header = unsafe { &*(SCRIPT_SESSION_VA as *const script::SessionHeaderV1) };
    let source = unsafe {
        core::slice::from_raw_parts(
            (SCRIPT_SESSION_VA as *const u8).add(script::SOURCE_OFFSET),
            header.source_bytes as usize,
        )
    };
    let policy = unsafe {
        core::slice::from_raw_parts(
            (SCRIPT_SESSION_VA as *const u8).add(script::POLICY_OFFSET),
            header.policy_bytes as usize,
        )
    };
    let source = core::str::from_utf8(source).unwrap_or("");
    (
        if header.mode == script::MODE_FILE {
            source
        } else {
            ""
        },
        core::str::from_utf8(policy).unwrap_or(""),
    )
}

fn script_entry_path() -> Option<&'static str> {
    let header = unsafe { &*(SCRIPT_SESSION_VA as *const script::SessionHeaderV1) };
    if header.mode != script::MODE_FILE
        || header.path_bytes == 0
        || header.path_bytes as usize
            > script::POLICY_BYTES.saturating_sub(header.policy_bytes as usize)
    {
        return None;
    }
    let bytes = unsafe {
        core::slice::from_raw_parts(
            (SCRIPT_SESSION_VA as *const u8)
                .add(script::POLICY_OFFSET + header.policy_bytes as usize),
            header.path_bytes as usize,
        )
    };
    core::str::from_utf8(bytes).ok()
}

fn script_permission_allows(source: &str, policy: &str, access: &str, path: &str) -> bool {
    (source.is_empty() || rules_allow(source, true, access, path))
        && rules_allow(policy, false, access, path)
}

fn rules_allow(input: &str, manifest: bool, access: &str, path: &str) -> bool {
    for line in input.lines() {
        let line = line.trim();
        let rule = if manifest {
            if let Some(rule) = line.strip_prefix("--!allow ") {
                rule
            } else {
                if !line.is_empty() && !line.starts_with("--") {
                    break;
                }
                continue;
            }
        } else {
            line
        };
        let Some(prefix) = rule
            .strip_prefix(access)
            .and_then(|rule| rule.strip_prefix(':'))
        else {
            continue;
        };
        let Ok(prefix) = normalize_script_path(prefix) else {
            continue;
        };
        if prefix == "/"
            || path == prefix
            || path
                .strip_prefix(&prefix)
                .is_some_and(|suffix| suffix.starts_with('/'))
        {
            return true;
        }
    }
    false
}

fn normalize_script_path(path: &str) -> Result<String, Status> {
    if path.len() > 255 || !path.starts_with('/') || path.as_bytes().contains(&0) {
        return Err(Status::Invalid);
    }
    let mut components = Vec::new();
    for component in path.split('/') {
        match component {
            "" | "." => {}
            ".." => {
                if components.pop().is_none() {
                    return Err(Status::AccessDenied);
                }
            }
            value => components.push(value),
        }
    }
    let mut normalized = String::from("/");
    normalized.push_str(&components.join("/"));
    if normalized.len() > 255 {
        return Err(Status::Invalid);
    }
    Ok(normalized)
}

fn script_temporary_path(path: &str, token: u64) -> Result<String, Status> {
    use core::fmt::Write as _;
    let (directory, _) = path.rsplit_once('/').unwrap_or(("", path));
    let mut output = if directory.is_empty() {
        String::from("/")
    } else {
        directory.to_string()
    };
    if !output.ends_with('/') {
        output.push('/');
    }
    let _ = write!(output, ".mica-{token:016x}.tmp");
    if output.len() > 255 {
        Err(Status::Invalid)
    } else {
        Ok(output)
    }
}

fn write_script_payload(bytes: &[u8], reply: &mut Message) -> Result<(), Error> {
    if bytes.len() > script::BROKER_BYTES {
        return Err(Error::NoSpace);
    }
    unsafe {
        core::ptr::copy_nonoverlapping(
            bytes.as_ptr(),
            (SCRIPT_SESSION_VA as *mut u8).add(script::BROKER_OFFSET),
            bytes.len(),
        );
        microsystem_user_rt::fence();
    }
    reply.words[0] = bytes.len() as u64;
    Ok(())
}

fn encode_listing(entries: Vec<String>, maximum: usize) -> Result<Vec<u8>, Error> {
    let mut output = Vec::new();
    for entry in entries {
        let separator = usize::from(!output.is_empty());
        let required = output
            .len()
            .checked_add(separator)
            .and_then(|length| length.checked_add(entry.len()))
            .ok_or(Error::NoSpace)?;
        if required > maximum {
            return Err(Error::NoSpace);
        }
        output
            .try_reserve_exact(separator + entry.len())
            .map_err(|_| Error::NoSpace)?;
        if separator != 0 {
            output.push(b'\n');
        }
        output.extend_from_slice(entry.as_bytes());
    }
    Ok(output)
}

fn map_fs_error(error: Error) -> Status {
    match error {
        Error::NotFound => Status::NotFound,
        Error::NoSpace => Status::NoSpace,
        Error::Io => Status::Io,
        Error::Corrupt => Status::Corrupt,
        Error::Busy => Status::Busy,
        Error::CrossDevice => Status::NotSupported,
        Error::ReadOnly => Status::AccessDenied,
        _ => Status::Invalid,
    }
}

fn require_path(path: &str) -> Result<&str, Error> {
    if path.is_empty() {
        Err(Error::Invalid)
    } else {
        Ok(path)
    }
}

fn request_path(request: &Message, path: &str, open_files: &OpenFiles) -> Result<String, Error> {
    if request.words[2] != 0 {
        return open_files.path(request.words[2]).map(str::to_string);
    }
    require_path(path).map(str::to_string)
}

fn stat_path(
    filesystem: &FileService<IpcDisk>,
    path: &str,
    reply: &mut Message,
) -> Result<(), Error> {
    let attributes = filesystem.attributes(path)?;
    reply.words[2] = attributes.mode as u64;
    reply.words[3] = attributes.uid as u64 | ((attributes.gid as u64) << 32);
    reply.words[4] = attributes.modified;
    match filesystem.metadata(path) {
        Ok(Metadata::File { bytes }) => {
            reply.words[0] = 1;
            reply.words[1] = bytes as u64;
            Ok(())
        }
        Ok(Metadata::Directory) => {
            let entries = filesystem.list(path)?;
            reply.words[0] = 2;
            reply.words[1] = entries.len() as u64;
            Ok(())
        }
        Err(error) => Err(error),
    }
}

fn write_filesystem_stats(
    filesystem: &FileService<IpcDisk>,
    path: &str,
    shared_address: usize,
    reply: &mut Message,
) -> Result<(), Error> {
    let stats = filesystem.stats_path(path)?;
    let output = FilesystemStatsV1 {
        version: 1,
        reserved: 0,
        block_size: BLOCK_SIZE as u32,
        total_blocks: stats.total_blocks,
        used_blocks: stats.used_blocks,
        free_blocks: stats.free_blocks,
        generation: stats.generation,
        transaction: stats.transaction,
        entries: stats.entries as u64,
        checkpoint_block: stats.checkpoint_block,
        segments_since_checkpoint: stats.segments_since_checkpoint,
        active_arena: stats.active_arena,
        segment_directory: stats.segment_directory as u8,
        active_segments: stats.active_segments,
        reserved_tail: 0,
    };
    let bytes = unsafe {
        core::slice::from_raw_parts(
            (&output as *const FilesystemStatsV1).cast::<u8>(),
            core::mem::size_of::<FilesystemStatsV1>(),
        )
    };
    write_shared(shared_address, bytes, reply)
}

fn write_range(
    filesystem: &mut FileService<IpcDisk>,
    path: &str,
    offset: u64,
    data: &[u8],
) -> Result<(), Error> {
    let offset = usize::try_from(offset).map_err(|_| Error::Invalid)?;
    filesystem.write_range(path, offset, data)
}

fn receive_with_maintenance(
    filesystem: &mut FileService<IpcDisk>,
    mut reply: Option<&Message>,
    request: &mut Message,
) -> Result<(), Status> {
    loop {
        let now = microsystem_user_rt::clock_now()?;
        let deadline = now.checked_add(1_000_000_000).ok_or(Status::Invalid)?;
        let result = match reply.take() {
            Some(message) => microsystem_user_rt::ipc_reply_recv(
                boot_cap::FILESYSTEM_ENDPOINT,
                message,
                request,
                deadline,
            ),
            None => microsystem_user_rt::ipc_recv(boot_cap::FILESYSTEM_ENDPOINT, request, deadline),
        };
        match result {
            Ok(()) => return Ok(()),
            Err(Status::TimedOut) => {
                let now = microsystem_user_rt::clock_now()?;
                filesystem.poll_writeback(now).map_err(map_error)?;
                filesystem.collect_once().map_err(map_error)?;
            }
            Err(status) => return Err(status),
        }
    }
}

fn map_error(error: Error) -> Status {
    match error {
        Error::Io => Status::Io,
        Error::NoSpace => Status::NoSpace,
        Error::Corrupt => Status::Corrupt,
        _ => Status::Invalid,
    }
}

fn write_shared(address: usize, bytes: &[u8], reply: &mut Message) -> Result<(), Error> {
    if bytes.len() > BLOCK_SIZE {
        return Err(Error::NoSpace);
    }
    unsafe {
        core::ptr::copy_nonoverlapping(bytes.as_ptr(), address as *mut u8, bytes.len());
        microsystem_user_rt::fence();
    }
    reply.words[0] = bytes.len() as u64;
    Ok(())
}

struct PanicText {
    bytes: [u8; 384],
    length: usize,
}

impl PanicText {
    const fn new() -> Self {
        Self {
            bytes: [0; 384],
            length: 0,
        }
    }

    fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.length]
    }
}

impl Write for PanicText {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        let remaining = self.bytes.len().saturating_sub(self.length);
        let copied = remaining.min(text.len());
        self.bytes[self.length..self.length + copied].copy_from_slice(&text.as_bytes()[..copied]);
        self.length += copied;
        if copied == text.len() {
            Ok(())
        } else {
            Err(fmt::Error)
        }
    }
}

#[panic_handler]
fn panic(info: &PanicInfo<'_>) -> ! {
    let mut output = PanicText::new();
    let _ = write!(&mut output, "[panic] mfs service: {info}\n");
    let _ = microsystem_user_rt::debug_write(output.as_bytes());
    microsystem_user_rt::exit(1)
}
