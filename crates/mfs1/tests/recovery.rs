use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use mfs1::{BLOCK_SIZE, BlockDevice, Error, FileSystem, Metadata, SEGMENT_BLOCKS, format};

const BLOCKS: u64 = 1024;

#[derive(Clone)]
struct MemDisk {
    state: Arc<Mutex<DiskState>>,
}

struct DiskState {
    blocks: Vec<[u8; BLOCK_SIZE]>,
    reads: u64,
    flushes: u64,
    writes: u64,
    superblock_writes: u64,
    superblock_flushes: u64,
    pending_superblock_flush: bool,
    fail_write_at: Option<u64>,
    fail_read_at: Option<u64>,
    fail_flush_at: Option<u64>,
    fail_superblock_write_at: Option<u64>,
    fail_superblock_flush_at: Option<u64>,
}

impl MemDisk {
    fn new(blocks: u64) -> Self {
        Self {
            state: Arc::new(Mutex::new(DiskState {
                blocks: vec![[0; BLOCK_SIZE]; blocks as usize],
                reads: 0,
                flushes: 0,
                writes: 0,
                superblock_writes: 0,
                superblock_flushes: 0,
                pending_superblock_flush: false,
                fail_write_at: None,
                fail_read_at: None,
                fail_flush_at: None,
                fail_superblock_write_at: None,
                fail_superblock_flush_at: None,
            })),
        }
    }

    fn fail_next_write(&self) {
        let mut state = self.state.lock().unwrap();
        state.fail_write_at = Some(state.writes + 1);
    }

    fn fail_next_read(&self) {
        let mut state = self.state.lock().unwrap();
        state.fail_read_at = Some(state.reads + 1);
    }

    fn clear_failures(&self) {
        let mut state = self.state.lock().unwrap();
        state.fail_write_at = None;
        state.fail_read_at = None;
        state.fail_flush_at = None;
        state.fail_superblock_write_at = None;
        state.fail_superblock_flush_at = None;
    }

    fn fail_next_flush(&self) {
        let mut state = self.state.lock().unwrap();
        state.fail_flush_at = Some(state.flushes + 1);
    }

    fn fail_second_next_flush(&self) {
        let mut state = self.state.lock().unwrap();
        state.fail_flush_at = Some(state.flushes + 2);
    }

    fn fail_next_superblock_write(&self) {
        let mut state = self.state.lock().unwrap();
        state.fail_superblock_write_at = Some(state.superblock_writes + 1);
    }

    fn fail_second_superblock_write(&self) {
        let mut state = self.state.lock().unwrap();
        state.fail_superblock_write_at = Some(state.superblock_writes + 2);
    }

    fn fail_next_superblock_flush(&self) {
        let mut state = self.state.lock().unwrap();
        state.fail_superblock_flush_at = Some(state.superblock_flushes + 1);
    }

    fn fail_second_superblock_flush(&self) {
        let mut state = self.state.lock().unwrap();
        state.fail_superblock_flush_at = Some(state.superblock_flushes + 2);
    }

    fn corrupt_byte(&self, block: u64, offset: usize) {
        let mut state = self.state.lock().unwrap();
        state.blocks[block as usize][offset] ^= 0xa5;
    }

    fn flush_count(&self) -> u64 {
        self.state.lock().unwrap().flushes
    }

    fn raw_block(&self, block: u64) -> [u8; BLOCK_SIZE] {
        self.state.lock().unwrap().blocks[block as usize]
    }

    fn replace_raw_block(&self, block: u64, data: &[u8; BLOCK_SIZE]) {
        self.state.lock().unwrap().blocks[block as usize] = *data;
    }
}

fn raw_u32(block: &[u8; BLOCK_SIZE], offset: usize) -> u32 {
    u32::from_le_bytes(block[offset..offset + 4].try_into().unwrap())
}

fn raw_u16(block: &[u8; BLOCK_SIZE], offset: usize) -> u16 {
    u16::from_le_bytes(block[offset..offset + 2].try_into().unwrap())
}

fn raw_u64(block: &[u8; BLOCK_SIZE], offset: usize) -> u64 {
    u64::from_le_bytes(block[offset..offset + 8].try_into().unwrap())
}

fn directory_entries(disk: &MemDisk, copy: u64) -> Vec<(u16, u16)> {
    let superblock = disk.raw_block(copy);
    let count = raw_u16(&superblock, 64) as usize;
    (0..count)
        .map(|slot| {
            let offset = 68 + slot * 4;
            (
                raw_u16(&superblock, offset),
                raw_u16(&superblock, offset + 2),
            )
        })
        .collect()
}

impl BlockDevice for MemDisk {
    fn block_count(&self) -> u64 {
        self.state.lock().unwrap().blocks.len() as u64
    }

    fn read_block(&mut self, block: u64, out: &mut [u8; BLOCK_SIZE]) -> Result<(), Error> {
        let mut state = self.state.lock().unwrap();
        state.reads += 1;
        if state.fail_read_at == Some(state.reads) {
            state.fail_read_at = None;
            return Err(Error::Io);
        }
        let source = state.blocks.get(block as usize).ok_or(Error::Io)?;
        out.copy_from_slice(source);
        Ok(())
    }

    fn write_block(&mut self, block: u64, data: &[u8; BLOCK_SIZE]) -> Result<(), Error> {
        let mut state = self.state.lock().unwrap();
        state.writes += 1;
        if state.fail_write_at == Some(state.writes) {
            state.fail_write_at = None;
            return Err(Error::Io);
        }
        if block < 2 {
            state.superblock_writes += 1;
            if state.fail_superblock_write_at == Some(state.superblock_writes) {
                state.fail_superblock_write_at = None;
                return Err(Error::Io);
            }
            state.pending_superblock_flush = true;
        }
        let target = state.blocks.get_mut(block as usize).ok_or(Error::Io)?;
        target.copy_from_slice(data);
        Ok(())
    }

    fn flush(&mut self) -> Result<(), Error> {
        let mut state = self.state.lock().unwrap();
        state.flushes += 1;
        if state.pending_superblock_flush {
            state.pending_superblock_flush = false;
            state.superblock_flushes += 1;
            if state.fail_superblock_flush_at == Some(state.superblock_flushes) {
                state.fail_superblock_flush_at = None;
                return Err(Error::Io);
            }
        }
        if state.fail_flush_at == Some(state.flushes) {
            state.fail_flush_at = None;
            return Err(Error::Io);
        }
        Ok(())
    }
}

const POWER_CUT_BLOCKS: u64 = 1024;
const RANDOM_CAMPAIGN_CASES: usize = 10_024;
const RANDOM_CAMPAIGN_SEED: u64 = 0x4d46_5331_4352_4153;
const SECTOR_SIZE: usize = 512;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum FaultOp {
    Read,
    Write,
    Flush,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum FaultMode {
    Io,
    ShortWrite { bytes: usize },
    TornSectors { mask: u8 },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct FaultSpec {
    op: FaultOp,
    at: u64,
    mode: FaultMode,
}

struct PowerCutState {
    blocks: u64,
    durable: BTreeMap<u64, Box<[u8; BLOCK_SIZE]>>,
    volatile: BTreeMap<u64, Box<[u8; BLOCK_SIZE]>>,
    torn_on_powercut: BTreeMap<u64, Box<[u8; BLOCK_SIZE]>>,
    reads: u64,
    writes: u64,
    flushes: u64,
    fault: Option<FaultSpec>,
    fault_triggered: bool,
}

#[derive(Clone)]
struct PowerCutDisk {
    state: Arc<Mutex<PowerCutState>>,
}

impl PowerCutDisk {
    fn new(blocks: u64) -> Self {
        Self {
            state: Arc::new(Mutex::new(PowerCutState {
                blocks,
                durable: BTreeMap::new(),
                volatile: BTreeMap::new(),
                torn_on_powercut: BTreeMap::new(),
                reads: 0,
                writes: 0,
                flushes: 0,
                fault: None,
                fault_triggered: false,
            })),
        }
    }

    fn fork(&self) -> Self {
        let state = self.state.lock().unwrap();
        Self {
            state: Arc::new(Mutex::new(PowerCutState {
                blocks: state.blocks,
                durable: state.durable.clone(),
                volatile: state.durable.clone(),
                torn_on_powercut: BTreeMap::new(),
                reads: 0,
                writes: 0,
                flushes: 0,
                fault: None,
                fault_triggered: false,
            })),
        }
    }

    fn crash(&self) {
        let mut state = self.state.lock().unwrap();
        let torn = core::mem::take(&mut state.torn_on_powercut);
        for (block, data) in torn {
            PowerCutDisk::write_to(&mut state.durable, block, &data);
        }
        state.volatile = state.durable.clone();
        state.fault = None;
    }

    fn clear_fault(&self) {
        self.state.lock().unwrap().fault = None;
    }

    fn fault_triggered(&self) -> bool {
        self.state.lock().unwrap().fault_triggered
    }

    fn fault_consumed(&self) -> bool {
        self.state.lock().unwrap().fault.is_none()
    }

    fn reset_counters(&self) {
        let mut state = self.state.lock().unwrap();
        state.reads = 0;
        state.writes = 0;
        state.flushes = 0;
        state.fault_triggered = false;
    }

    fn arm_relative(&self, op: FaultOp, offset: u64, mode: FaultMode) {
        let mut state = self.state.lock().unwrap();
        let base = match op {
            FaultOp::Read => state.reads,
            FaultOp::Write => state.writes,
            FaultOp::Flush => state.flushes,
        };
        state.fault = Some(FaultSpec {
            op,
            at: base.saturating_add(offset),
            mode,
        });
        state.fault_triggered = false;
    }

    fn read_from(map: &BTreeMap<u64, Box<[u8; BLOCK_SIZE]>>, block: u64) -> [u8; BLOCK_SIZE] {
        map.get(&block)
            .map(|block| **block)
            .unwrap_or([0; BLOCK_SIZE])
    }

    fn write_to(
        map: &mut BTreeMap<u64, Box<[u8; BLOCK_SIZE]>>,
        block: u64,
        data: &[u8; BLOCK_SIZE],
    ) {
        if data.iter().all(|byte| *byte == 0) {
            map.remove(&block);
        } else {
            map.insert(block, Box::new(*data));
        }
    }

    fn take_fault(state: &mut PowerCutState, op: FaultOp, at: u64) -> Option<FaultSpec> {
        if state
            .fault
            .map(|fault| fault.op == op && fault.at == at)
            .unwrap_or(false)
        {
            state.fault.take()
        } else {
            None
        }
    }
}

impl BlockDevice for PowerCutDisk {
    fn block_count(&self) -> u64 {
        self.state.lock().unwrap().blocks
    }

    fn read_block(&mut self, block: u64, out: &mut [u8; BLOCK_SIZE]) -> Result<(), Error> {
        let mut state = self.state.lock().unwrap();
        if block >= state.blocks {
            return Err(Error::Io);
        }
        let at = state.reads;
        state.reads += 1;
        if PowerCutDisk::take_fault(&mut state, FaultOp::Read, at).is_some() {
            state.fault_triggered = true;
            return Err(Error::Io);
        }
        out.copy_from_slice(&PowerCutDisk::read_from(&state.volatile, block));
        Ok(())
    }

    fn write_block(&mut self, block: u64, data: &[u8; BLOCK_SIZE]) -> Result<(), Error> {
        let mut state = self.state.lock().unwrap();
        if block >= state.blocks {
            return Err(Error::Io);
        }
        let at = state.writes;
        state.writes += 1;
        let fault = PowerCutDisk::take_fault(&mut state, FaultOp::Write, at);
        if fault.is_some() {
            state.fault_triggered = true;
        }
        let mut target = PowerCutDisk::read_from(&state.volatile, block);
        match fault.map(|fault| fault.mode) {
            None => target.copy_from_slice(data),
            Some(FaultMode::Io) => return Err(Error::Io),
            Some(FaultMode::ShortWrite { bytes }) => {
                let bytes = bytes.clamp(1, BLOCK_SIZE - 1);
                target[..bytes].copy_from_slice(&data[..bytes]);
                PowerCutDisk::write_to(&mut state.torn_on_powercut, block, &target);
                PowerCutDisk::write_to(&mut state.volatile, block, &target);
                return Err(Error::Io);
            }
            Some(FaultMode::TornSectors { mask }) => {
                for sector in 0..(BLOCK_SIZE / SECTOR_SIZE) {
                    if mask & (1 << sector) != 0 {
                        let start = sector * SECTOR_SIZE;
                        target[start..start + SECTOR_SIZE]
                            .copy_from_slice(&data[start..start + SECTOR_SIZE]);
                    }
                }
                PowerCutDisk::write_to(&mut state.torn_on_powercut, block, &target);
                PowerCutDisk::write_to(&mut state.volatile, block, &target);
                return Err(Error::Io);
            }
        }
        PowerCutDisk::write_to(&mut state.volatile, block, &target);
        Ok(())
    }

    fn flush(&mut self) -> Result<(), Error> {
        let mut state = self.state.lock().unwrap();
        let at = state.flushes;
        state.flushes += 1;
        if let Some(fault) = PowerCutDisk::take_fault(&mut state, FaultOp::Flush, at) {
            state.fault_triggered = true;
            assert_eq!(fault.mode, FaultMode::Io);
            return Err(Error::Io);
        }
        state.durable = state.volatile.clone();
        Ok(())
    }
}

#[derive(Clone, Copy)]
struct XorShift64(u64);

impl XorShift64 {
    fn next(&mut self) -> u64 {
        let mut value = self.0;
        value ^= value << 7;
        value ^= value >> 9;
        value ^= value << 8;
        self.0 = value;
        value
    }
}

fn seeded_bytes(seed: u64, tag: u64, min_len: usize, max_len: usize) -> Vec<u8> {
    let mut rng = XorShift64(seed ^ tag.rotate_left(17));
    let span = max_len.saturating_sub(min_len).saturating_add(1);
    let length = min_len + (rng.next() as usize % span);
    (0..length)
        .map(|index| {
            let value = rng.next().wrapping_add(index as u64 * 0x9e37_79b9);
            (value as u8).wrapping_add(tag as u8)
        })
        .collect()
}

fn powercut_seeded_pair() -> PowerCutDisk {
    let disk = PowerCutDisk::new(POWER_CUT_BLOCKS);
    let mut fs = format(disk.clone()).unwrap();
    fs.put("/a", b"old-a").unwrap();
    fs.put("/b", b"old-b").unwrap();
    fs.sync().unwrap();
    disk.crash();
    disk
}

#[test]
fn fsync_survives_remount_but_unsynced_write_does_not() {
    let disk = MemDisk::new(BLOCKS);
    let mut fs = format(disk.clone()).unwrap();

    fs.put("/durable", b"committed").unwrap();
    fs.fsync("/durable").unwrap();
    fs.put("/volatile", b"not committed").unwrap();

    let remounted = FileSystem::mount(disk.clone()).unwrap();
    assert_eq!(remounted.read("/durable").unwrap(), b"committed");
    assert_eq!(remounted.read("/volatile"), Err(Error::NotFound));
    assert!(
        disk.flush_count() >= 3,
        "format + commit must flush metadata"
    );
}

#[test]
fn mount_falls_back_to_previous_valid_superblock() {
    let disk = MemDisk::new(BLOCKS);
    let mut fs = format(disk.clone()).unwrap();

    fs.put("/old", b"old generation").unwrap();
    fs.fsync("/old").unwrap();
    fs.put("/new", b"new generation").unwrap();
    fs.fsync("/new").unwrap();

    // Generation three is written to copy one.  Destroying it must leave a
    // mountable generation-two view rather than accepting a mixed superblock.
    disk.corrupt_byte(1, 0);
    let remounted = FileSystem::mount(disk).unwrap();
    assert_eq!(remounted.read("/old").unwrap(), b"old generation");
    assert_eq!(remounted.read("/new"), Err(Error::NotFound));
}

#[test]
fn corrupt_latest_transaction_falls_back_without_mixing_generations() {
    let disk = MemDisk::new(BLOCKS);
    let mut fs = format(disk.clone()).unwrap();
    fs.put("/old", b"old generation").unwrap();
    fs.fsync("/old").unwrap();
    fs.put("/new", b"new generation").unwrap();
    fs.fsync("/new").unwrap();

    let latest = fs.stats();
    assert!(latest.generation >= 3);
    let latest_record = latest.checkpoint_block + latest.used_blocks - 2;
    // Corrupt the latest transaction's payload.  The newest superblock must
    // be rejected and mount must replay the prior generation as a whole.
    disk.corrupt_byte(latest_record, 64);

    let mut remounted = FileSystem::mount(disk).unwrap();
    let recovered = remounted.stats();
    assert!(recovered.generation < latest.generation);
    assert_eq!(remounted.read("/old").unwrap(), b"old generation");
    assert_eq!(remounted.read("/new"), Err(Error::NotFound));
    remounted.check().unwrap();
}

#[test]
fn failed_record_write_leaves_previous_transaction_mountable() {
    let disk = MemDisk::new(BLOCKS);
    let mut fs = format(disk.clone()).unwrap();
    disk.fail_next_write();
    fs.put("/crash", b"must not appear").unwrap();
    assert_eq!(fs.fsync("/crash"), Err(Error::Io));

    // The failed record may have been attempted, but the previous superblock
    // still bounds replay, so a remount cannot expose a partial transaction.
    disk.clear_failures();
    let remounted = FileSystem::mount(disk).unwrap();
    assert_eq!(remounted.read("/crash"), Err(Error::NotFound));
}

#[test]
fn failed_fsync_retry_preserves_rename_then_unlink_order() {
    let disk = MemDisk::new(BLOCKS);
    let mut fs = format(disk.clone()).unwrap();

    fs.put("/before", b"rename then remove").unwrap();
    fs.fsync("/before").unwrap();
    fs.rename("/before", "/after").unwrap();
    fs.unlink("/after").unwrap();

    // The failed commit must restore both dirty mutations in their original
    // order; retrying fsync must replay rename before unlink.
    disk.fail_next_write();
    assert_eq!(fs.fsync("/after"), Err(Error::Io));
    disk.clear_failures();
    fs.fsync("/after").unwrap();

    let remounted = FileSystem::mount(disk).unwrap();
    assert_eq!(remounted.read("/before"), Err(Error::NotFound));
    assert_eq!(remounted.read("/after"), Err(Error::NotFound));
}

#[test]
fn rename_rejects_directory_descendant_without_dirty_mutation() {
    let disk = MemDisk::new(BLOCKS);
    let mut fs = format(disk.clone()).unwrap();
    fs.mkdir("/parent").unwrap();
    fs.mkdir("/parent/child").unwrap();
    fs.fsync("/parent/child").unwrap();

    assert_eq!(
        fs.rename("/parent", "/parent/child/grandchild"),
        Err(Error::Invalid)
    );
    fs.sync().unwrap();

    let remounted = FileSystem::mount(disk).unwrap();
    assert!(remounted.list("/parent").is_ok());
    assert!(remounted.list("/parent/child").is_ok());
    assert_eq!(
        remounted.list("/parent/child/grandchild"),
        Err(Error::NotDirectory)
    );
}

#[test]
fn chained_directory_rename_recreate_remove_views_stay_consistent() {
    let disk = MemDisk::new(BLOCKS);
    let mut fs = format(disk.clone()).unwrap();
    fs.mkdir("/tree").unwrap();
    fs.mkdir("/tree/child").unwrap();
    fs.put("/tree/child/file", b"payload").unwrap();
    fs.sync().unwrap();

    fs.rename("/tree", "/moved").unwrap();
    assert_eq!(fs.metadata("/tree"), Err(Error::NotFound));
    assert_eq!(fs.metadata("/moved"), Ok(Metadata::Directory));
    assert_eq!(fs.list("/moved").unwrap(), vec!["/moved/child".to_string()]);
    assert_eq!(fs.read("/moved/child/file").unwrap(), b"payload");
    assert_eq!(
        fs.metadata("/moved/child/file"),
        Ok(Metadata::File { bytes: 7 })
    );

    // Recreate the old directory name while the original subtree is still
    // reachable through the rename destination.
    fs.mkdir("/tree").unwrap();
    fs.mkdir("/tree/new").unwrap();
    fs.put("/tree/new/file", b"fresh").unwrap();
    assert_eq!(
        fs.list("/").unwrap(),
        vec!["/moved".to_string(), "/tree".to_string()]
    );

    // Chain the first rename and remove the old subtree through its new name.
    fs.rename("/moved", "/final").unwrap();
    assert_eq!(fs.read("/final/child/file").unwrap(), b"payload");
    fs.unlink("/final/child/file").unwrap();
    assert_eq!(fs.metadata("/final/child/file"), Err(Error::NotFound));
    assert!(fs.list("/final/child").unwrap().is_empty());
    fs.unlink("/final/child").unwrap();
    assert_eq!(fs.metadata("/final/child"), Err(Error::NotFound));

    fs.rename("/tree", "/again").unwrap();
    assert_eq!(fs.read("/again/new/file").unwrap(), b"fresh");
    fs.sync().unwrap();

    let mut remounted = FileSystem::mount(disk).unwrap();
    assert_eq!(remounted.list("/").unwrap(), vec!["/again", "/final"]);
    assert_eq!(remounted.metadata("/final"), Ok(Metadata::Directory));
    assert!(remounted.list("/final").unwrap().is_empty());
    assert_eq!(remounted.read("/again/new/file").unwrap(), b"fresh");
    assert_eq!(
        remounted.metadata("/again/new/file"),
        Ok(Metadata::File { bytes: 5 })
    );
    assert_eq!(remounted.metadata("/tree"), Err(Error::NotFound));
    assert_eq!(remounted.metadata("/moved"), Err(Error::NotFound));
    remounted.check().unwrap();
}

#[test]
fn replace_file_failure_matrix_is_atomic_and_retryable() {
    let failures: [(&str, fn(&MemDisk)); 4] = [
        ("record write", |disk: &MemDisk| disk.fail_next_write()),
        ("data flush", |disk: &MemDisk| disk.fail_next_flush()),
        ("superblock write", |disk: &MemDisk| {
            disk.fail_next_superblock_write()
        }),
        ("superblock flush", |disk: &MemDisk| {
            disk.fail_next_superblock_flush()
        }),
    ];

    for (name, arm_failure) in failures {
        let disk = MemDisk::new(BLOCKS);
        let mut fs = format(disk.clone()).unwrap();
        fs.put("/target", b"old").unwrap();
        fs.fsync("/target").unwrap();
        fs.put("/temp", b"new").unwrap();
        fs.fsync("/temp").unwrap();

        fs.replace_file("/temp", "/target").unwrap();
        arm_failure(&disk);
        assert_eq!(fs.fsync("/target"), Err(Error::Io), "failure point {name}");

        // Before retrying the in-memory transaction, remount the device as a
        // crash would.  The committed snapshot may be the old pair or the new
        // pair, but it must never expose a missing target or mixed state.
        let mut remounted = FileSystem::mount(disk.clone()).unwrap();
        match remounted.read("/target") {
            Ok(data) if data == b"old" => {
                assert_eq!(
                    remounted.read("/temp").unwrap(),
                    b"new",
                    "failure point {name}"
                );
            }
            Ok(data) if data == b"new" => {
                assert_eq!(
                    remounted.read("/temp"),
                    Err(Error::NotFound),
                    "failure point {name}"
                );
            }
            other => panic!("failure point {name} lost target: {other:?}"),
        }
        remounted.check().unwrap();

        // A failed fsync must leave the complete mutation sequence queued so
        // the in-memory retry commits both Remove(target) and Rename(temp,target).
        disk.clear_failures();
        fs.fsync("/target").unwrap();
        assert_eq!(fs.read("/target").unwrap(), b"new", "failure point {name}");
        assert_eq!(
            fs.read("/temp"),
            Err(Error::NotFound),
            "failure point {name}"
        );
        let mut final_remounted = FileSystem::mount(disk).unwrap();
        assert_eq!(
            final_remounted.read("/target").unwrap(),
            b"new",
            "failure point {name}"
        );
        assert_eq!(
            final_remounted.read("/temp"),
            Err(Error::NotFound),
            "failure point {name}"
        );
        final_remounted.check().unwrap();
    }
}

fn seeded_pair(disk: &MemDisk) -> mfs1::FileSystem<MemDisk> {
    let mut fs = format(disk.clone()).unwrap();
    fs.put("/a", b"old-a").unwrap();
    fs.put("/b", b"old-b").unwrap();
    fs.sync().unwrap();
    fs
}

fn stage_new_pair(fs: &mut mfs1::FileSystem<MemDisk>) {
    fs.put("/a", b"new-a").unwrap();
    fs.put("/b", b"new-b").unwrap();
}

fn assert_old_or_new_pair(disk: MemDisk) {
    let mut remounted = mfs1::FileSystem::mount(disk).unwrap();
    let first = remounted.read("/a").unwrap();
    let second = remounted.read("/b").unwrap();
    assert!(
        (first == b"old-a" && second == b"old-b") || (first == b"new-a" && second == b"new-b"),
        "recovery exposed a mixed transaction: {first:?} / {second:?}"
    );
    remounted.check().unwrap();
}

#[test]
fn commit_failure_plan_never_exposes_a_mixed_snapshot() {
    let failures: [fn(&MemDisk); 4] = [
        |disk: &MemDisk| disk.fail_next_write(),
        |disk: &MemDisk| disk.fail_next_flush(),
        |disk: &MemDisk| disk.fail_next_superblock_write(),
        |disk: &MemDisk| disk.fail_second_next_flush(),
    ];
    for arm_failure in failures {
        let disk = MemDisk::new(BLOCKS);
        let mut fs = seeded_pair(&disk);
        stage_new_pair(&mut fs);
        arm_failure(&disk);
        assert_eq!(fs.sync(), Err(Error::Io));
        assert_old_or_new_pair(disk);
    }
}

#[test]
fn gc_failure_matrix_never_reuses_old_or_candidate_segments() {
    let failures: [(&str, fn(&MemDisk)); 6] = [
        ("record write", |disk: &MemDisk| disk.fail_next_write()),
        ("data flush", |disk: &MemDisk| disk.fail_next_flush()),
        ("first superblock write", |disk: &MemDisk| {
            disk.fail_next_superblock_write()
        }),
        ("first superblock flush", |disk: &MemDisk| {
            disk.fail_next_superblock_flush()
        }),
        ("sealed second superblock write", |disk: &MemDisk| {
            disk.fail_second_superblock_write()
        }),
        ("sealed second superblock flush", |disk: &MemDisk| {
            disk.fail_second_superblock_flush()
        }),
    ];

    for (name, arm_failure) in failures {
        let disk = MemDisk::new(BLOCKS);
        let mut fs = format(disk.clone()).unwrap();
        fs.put("/live-a", b"a").unwrap();
        fs.put("/live-b", b"b").unwrap();
        fs.sync().unwrap();

        arm_failure(&disk);
        assert_eq!(fs.gc(), Err(Error::Io), "failure point {name}");
        disk.clear_failures();
        fs.put("/after", b"after").unwrap();
        fs.sync().unwrap();

        let mut remounted = mfs1::FileSystem::mount(disk).unwrap();
        assert_eq!(remounted.read("/live-a").unwrap(), b"a", "{name}");
        assert_eq!(remounted.read("/live-b").unwrap(), b"b", "{name}");
        assert_eq!(remounted.read("/after").unwrap(), b"after", "{name}");
        remounted.check().unwrap();
    }
}

#[test]
fn gc_switches_arena_and_keeps_live_entries() {
    let disk = MemDisk::new(BLOCKS);
    let mut fs = format(disk.clone()).unwrap();
    fs.mkdir("/etc").unwrap();
    fs.put("/etc/config", b"v1").unwrap();
    fs.fsync("/etc/config").unwrap();

    let before = fs.stats();
    let before_directory = directory_entries(&disk, before.generation % 2);
    fs.gc().unwrap();
    let after = fs.stats();
    let after_directory = directory_entries(&disk, after.generation % 2);
    assert_ne!(before.active_arena, after.active_arena);
    assert!(after.generation > before.generation);
    assert_eq!(before_directory.len(), 1);
    assert_eq!(after_directory.len(), 1);
    assert_ne!(before_directory[0].0, after_directory[0].0);
    assert_eq!(
        before.checkpoint_block,
        2 + before_directory[0].0 as u64 * SEGMENT_BLOCKS
    );
    assert_eq!(
        after.checkpoint_block,
        2 + after_directory[0].0 as u64 * SEGMENT_BLOCKS
    );
    assert_ne!(before.checkpoint_block, after.checkpoint_block);
    assert!(before.segments_since_checkpoint >= 1);
    assert!(after.segments_since_checkpoint >= 1);
    assert_eq!(fs.read("/etc/config").unwrap(), b"v1");
    assert_eq!(fs.check().unwrap().entries, after.entries);

    let remounted = FileSystem::mount(disk).unwrap();
    assert_eq!(remounted.read("/etc/config").unwrap(), b"v1");
}

#[test]
fn repeated_overwrites_trigger_gc_and_preserve_latest_data() {
    // A directory with several near-full segments gives GC a low-live-ratio
    // victim while leaving enough free capacity for the compacted transaction.
    let disk = MemDisk::new(6 * SEGMENT_BLOCKS + 2);
    let mut fs = format(disk.clone()).unwrap();
    let mut latest = Vec::new();

    for version in 0..500u32 {
        latest = format!("value-{version:03}").into_bytes();
        fs.put("/hot", &latest).unwrap();
        fs.sync().unwrap();
    }

    assert_eq!(fs.read("/hot").unwrap(), latest);
    let before = fs.stats();
    let before_directory = directory_entries(&disk, before.generation % 2);
    assert!(before_directory.len() >= 2);
    assert!(
        before_directory
            .iter()
            .filter(|(_, used)| *used as u64 >= SEGMENT_BLOCKS - 8)
            .count()
            >= 2,
        "overwrite workload did not fill two segments"
    );
    fs.check().unwrap();

    fs.gc().unwrap();
    let after = fs.stats();
    let after_directory = directory_entries(&disk, after.generation % 2);
    assert!(after.free_blocks > before.free_blocks);
    assert!(after_directory.len() < before_directory.len());
    let latest_segment = before_directory.last().unwrap().0;
    assert!(
        after_directory
            .iter()
            .any(|(index, _)| *index == latest_segment)
    );
    let capacity = (after.total_blocks - 2) / SEGMENT_BLOCKS * SEGMENT_BLOCKS;
    assert!(after.free_blocks * 100 / capacity >= 25);
    assert_eq!(fs.read("/hot").unwrap(), latest);
    fs.check().unwrap();

    let mut remounted = FileSystem::mount(disk).unwrap();
    assert_eq!(remounted.read("/hot").unwrap(), latest);
    remounted.check().unwrap();
}

#[test]
fn records_pad_at_segment_boundary_and_padding_crc_falls_back() {
    let disk = MemDisk::new(BLOCKS);
    let mut fs = format(disk.clone()).unwrap();

    let fill_len = (SEGMENT_BLOCKS as usize - 6) * BLOCK_SIZE - 64 - "/fill".len();
    let fill = vec![0x5a; fill_len];
    fs.put("/fill", &fill).unwrap();
    fs.fsync("/fill").unwrap();

    let before = fs.stats();
    let segment_boundary = before.checkpoint_block + SEGMENT_BLOCKS;
    let before_head = before.checkpoint_block + before.used_blocks;
    assert!(before_head < segment_boundary);
    let padding_blocks = segment_boundary - before_head;
    assert!(padding_blocks > 0 && padding_blocks < SEGMENT_BLOCKS);

    let tail = vec![0xa7; 7 * BLOCK_SIZE];
    fs.put("/tail", &tail).unwrap();
    fs.fsync("/tail").unwrap();

    let padding = disk.raw_block(before_head);
    assert_eq!(raw_u32(&padding, 0), 0x3152_464d);
    assert_eq!(padding[4], 6, "segment tail must be a Padding record");
    assert_eq!(raw_u32(&padding, 32), padding_blocks as u32);
    for block in before_head + 1..segment_boundary {
        assert_eq!(disk.raw_block(block), [0; BLOCK_SIZE]);
    }

    let next = disk.raw_block(segment_boundary);
    assert_eq!(raw_u32(&next, 0), 0x3152_464d);
    assert_eq!(
        next[4], 1,
        "the next real record must start at the boundary"
    );
    let next_blocks = raw_u32(&next, 32) as u64;
    assert!(next_blocks > 0 && segment_boundary + next_blocks <= segment_boundary + SEGMENT_BLOCKS);

    let mut remounted = FileSystem::mount(disk.clone()).unwrap();
    assert_eq!(remounted.read("/tail").unwrap(), tail);
    remounted.check().unwrap();

    disk.corrupt_byte(before_head, 36);
    let mut fallback = FileSystem::mount(disk).unwrap();
    assert_eq!(fallback.read("/fill").unwrap(), fill);
    assert_eq!(fallback.read("/tail"), Err(Error::NotFound));
    fallback.check().unwrap();
}

#[test]
fn multi_extent_put_roundtrips_through_remount_and_gc() {
    let disk = MemDisk::new(8 * SEGMENT_BLOCKS + 2);
    let mut fs = format(disk.clone()).unwrap();
    let size = 2 * 1024 * 1024 + 12_345;
    let data: Vec<u8> = (0..size)
        .map(|index| (index as u8).wrapping_mul(37).wrapping_add(11))
        .collect();

    fs.put("/big", &data).unwrap();
    fs.sync().unwrap();
    assert_eq!(fs.read("/big").unwrap(), data);
    fs.check().unwrap();

    let first = disk.raw_block(fs.stats().checkpoint_block);
    let second = disk.raw_block(fs.stats().checkpoint_block + SEGMENT_BLOCKS);
    assert_eq!(first[4], 1, "large file must start with a Put extent");
    assert_eq!(raw_u32(&first, 32), SEGMENT_BLOCKS as u32);
    assert_eq!(second[4], 1, "second extent must remain a Put record");
    assert!(raw_u64(&second, 48) > 0, "extent offset must advance");

    let mut remounted = FileSystem::mount(disk.clone()).unwrap();
    assert_eq!(remounted.read("/big").unwrap(), data);
    remounted.check().unwrap();

    let before_gc = remounted.stats();
    remounted.gc().unwrap();
    assert_ne!(before_gc.active_arena, remounted.stats().active_arena);
    assert_eq!(remounted.read("/big").unwrap(), data);
    remounted.check().unwrap();

    let mut compacted = FileSystem::mount(disk).unwrap();
    assert_eq!(compacted.read("/big").unwrap(), data);
    compacted.check().unwrap();
}

#[test]
fn legacy_zero_feature_superblocks_mount_and_gc_upgrades_layout_feature() {
    let disk = MemDisk::new(BLOCKS);
    let mut fs = format(disk.clone()).unwrap();
    fs.put("/legacy", b"v1 payload").unwrap();
    fs.fsync("/legacy").unwrap();
    let stats = fs.stats();
    let legacy_log_head = stats.checkpoint_block + stats.used_blocks;

    for copy in 0..2 {
        let mut superblock = disk.raw_block(copy);
        superblock[12..16].copy_from_slice(&0u32.to_le_bytes());
        superblock[48..56].copy_from_slice(&legacy_log_head.to_le_bytes());
        let crc = mfs1::crc32c(&superblock[..60]);
        superblock[60..64].copy_from_slice(&crc.to_le_bytes());
        disk.replace_raw_block(copy, &superblock);
    }

    let mut legacy = FileSystem::mount(disk.clone()).unwrap();
    assert_eq!(legacy.read("/legacy").unwrap(), b"v1 payload");
    legacy.check().unwrap();
    legacy.gc().unwrap();

    let active = disk.raw_block(legacy.stats().generation % 2);
    assert_eq!(
        raw_u32(&active, 12),
        0x3,
        "GC must upgrade the segment layout and directory features"
    );
    let mut upgraded = FileSystem::mount(disk).unwrap();
    assert_eq!(upgraded.read("/legacy").unwrap(), b"v1 payload");
    upgraded.check().unwrap();
}

#[test]
fn path_validation_and_directory_boundaries_are_enforced() {
    let disk = MemDisk::new(BLOCKS);
    let mut fs = format(disk).unwrap();
    assert_eq!(fs.put("relative", b"x"), Err(Error::Invalid));
    assert_eq!(fs.put("/../escape", b"x"), Err(Error::Invalid));
    assert_eq!(fs.read("/missing"), Err(Error::NotFound));
    assert_eq!(fs.put("/child/file", b"x"), Err(Error::NotDirectory));
    fs.mkdir("/child").unwrap();
    fs.put("/child/file", b"x").unwrap();
    assert_eq!(fs.read("/child"), Err(Error::IsDirectory));
    assert_eq!(fs.unlink("/child"), Err(Error::Busy));
}

#[test]
fn randomized_powercut_campaign_only_recovers_complete_snapshots() {
    println!(
        "mfs1-crash-campaign cases={} seed=0x{RANDOM_CAMPAIGN_SEED:016x}",
        RANDOM_CAMPAIGN_CASES
    );
    assert!(RANDOM_CAMPAIGN_CASES >= 10_000);

    let baseline = powercut_seeded_pair();
    let mut selected_modes = [0u64; 4];
    let mut triggered_modes = [0u64; 4];
    for case_id in 0..RANDOM_CAMPAIGN_CASES {
        let case_seed = RANDOM_CAMPAIGN_SEED ^ (case_id as u64).wrapping_mul(0x9e37_79b9);
        let mut rng = XorShift64(case_seed | 1);
        let disk = baseline.fork();
        let mut fs = FileSystem::mount(disk.clone()).unwrap_or_else(|error| {
            panic!("seed=0x{case_seed:016x} case={case_id} baseline mount: {error:?}")
        });
        let new_a = seeded_bytes(case_seed, 0xa, 1, 12 * 1024);
        let new_b = seeded_bytes(case_seed, 0xb, 1, 12 * 1024);
        fs.put("/a", &new_a).unwrap();
        fs.put("/b", &new_b).unwrap();

        disk.reset_counters();
        let op = if case_id == 3 {
            FaultOp::Flush
        } else if case_id < 3 {
            FaultOp::Write
        } else if rng.next() & 1 == 0 {
            FaultOp::Write
        } else {
            FaultOp::Flush
        };
        let offset = if case_id < 4 {
            0
        } else {
            match op {
                FaultOp::Write => rng.next() % 4,
                FaultOp::Flush => rng.next() % 2,
                FaultOp::Read => unreachable!(),
            }
        };
        let mode = match case_id {
            0 => FaultMode::Io,
            1 => FaultMode::ShortWrite {
                bytes: 1 + (rng.next() as usize % (BLOCK_SIZE - 1)),
            },
            2 => FaultMode::TornSectors {
                mask: (rng.next() as u8) | 1,
            },
            _ => match op {
                FaultOp::Write => match rng.next() % 3 {
                    0 => FaultMode::Io,
                    1 => FaultMode::ShortWrite {
                        bytes: 1 + (rng.next() as usize % (BLOCK_SIZE - 1)),
                    },
                    _ => FaultMode::TornSectors {
                        mask: (rng.next() as u8) | 1,
                    },
                },
                FaultOp::Flush => FaultMode::Io,
                FaultOp::Read => unreachable!(),
            },
        };
        let slot = match (op, mode) {
            (FaultOp::Write, FaultMode::Io) => 0,
            (FaultOp::Write, FaultMode::ShortWrite { .. }) => 1,
            (FaultOp::Write, FaultMode::TornSectors { .. }) => 2,
            (FaultOp::Flush, FaultMode::Io) => 3,
            _ => unreachable!(),
        };
        selected_modes[slot] += 1;
        disk.arm_relative(op, offset, mode);
        let _ = fs.sync();
        assert!(
            disk.fault_consumed(),
            "seed=0x{case_seed:016x} case={case_id} op={op:?} offset={offset} mode={mode:?} fault was not reached"
        );
        if disk.fault_triggered() {
            triggered_modes[slot] += 1;
        }
        disk.crash();

        let mut recovered = FileSystem::mount(disk.clone()).unwrap_or_else(|error| {
            panic!(
                "seed=0x{case_seed:016x} case={case_id} op={op:?} offset={offset} mode={mode:?} mount: {error:?}"
            )
        });
        let first = recovered.read("/a").unwrap_or_else(|error| {
            panic!(
                "seed=0x{case_seed:016x} case={case_id} op={op:?} offset={offset} mode={mode:?} read /a: {error:?}"
            )
        });
        let second = recovered.read("/b").unwrap_or_else(|error| {
            panic!(
                "seed=0x{case_seed:016x} case={case_id} op={op:?} offset={offset} mode={mode:?} read /b: {error:?}"
            )
        });
        assert!(
            (first == b"old-a" && second == b"old-b") || (first == new_a && second == new_b),
            "seed=0x{case_seed:016x} case={case_id} op={op:?} offset={offset} mode={mode:?} exposed mixed snapshot: {} / {}",
            first.len(),
            second.len()
        );
        recovered.check().unwrap_or_else(|error| {
            panic!(
                "seed=0x{case_seed:016x} case={case_id} op={op:?} offset={offset} mode={mode:?} check: {error:?}"
            )
        });
    }
    println!("mfs1-crash-campaign selected={selected_modes:?} triggered={triggered_modes:?}");
    assert!(selected_modes.iter().all(|count| *count > 0));
    assert!(triggered_modes.iter().all(|count| *count > 0));
}

#[test]
fn metadata_repair_heals_only_one_checksum_invalid_superblock() {
    let disk = MemDisk::new(BLOCKS);
    let mut fs = format(disk.clone()).unwrap();
    fs.put("/repair", b"safe metadata repair").unwrap();
    fs.sync().unwrap();
    let valid = disk.raw_block(0);
    disk.corrupt_byte(1, 0);

    let (mut repaired, report) = FileSystem::repair(disk.clone()).unwrap();
    assert_eq!(report.repaired_superblocks, 1);
    assert_eq!(report.metadata.checksum_valid_superblocks, 2);
    assert_eq!(report.metadata.replay_verified_superblocks, 2);
    assert!(report.metadata.is_healthy());
    assert_eq!(disk.raw_block(0), valid);
    assert_eq!(disk.raw_block(1), valid);
    assert_eq!(repaired.read("/repair").unwrap(), b"safe metadata repair");
    repaired.check().unwrap();

    let (mut verified, metadata) = FileSystem::verify(disk).unwrap();
    assert!(metadata.is_healthy());
    assert_eq!(metadata.checksum_valid_superblocks, 2);
    assert_eq!(metadata.replay_verified_superblocks, 2);
    verified.check().unwrap();
}

#[test]
fn metadata_repair_refuses_double_corruption_and_read_io() {
    let disk = MemDisk::new(BLOCKS);
    let mut fs = format(disk.clone()).unwrap();
    fs.put("/repair", b"must stay intact").unwrap();
    fs.sync().unwrap();
    disk.corrupt_byte(0, 0);
    disk.corrupt_byte(1, 0);
    let corrupt0 = disk.raw_block(0);
    let corrupt1 = disk.raw_block(1);
    assert!(matches!(
        FileSystem::repair(disk.clone()),
        Err(Error::Corrupt)
    ));
    assert_eq!(disk.raw_block(0), corrupt0);
    assert_eq!(disk.raw_block(1), corrupt1);

    let readable = MemDisk::new(BLOCKS);
    let _ = format(readable.clone()).unwrap();
    let before0 = readable.raw_block(0);
    let before1 = readable.raw_block(1);
    readable.fail_next_read();
    assert!(matches!(
        FileSystem::repair(readable.clone()),
        Err(Error::Io)
    ));
    assert_eq!(readable.raw_block(0), before0);
    assert_eq!(readable.raw_block(1), before1);
}

#[test]
fn metadata_repair_refuses_ambiguous_latest_superblock_corruption() {
    let disk = MemDisk::new(BLOCKS);
    let mut fs = format(disk.clone()).unwrap();
    fs.put("/latest", b"newest committed data").unwrap();
    fs.sync().unwrap();
    // Generation two is the active copy at block zero.  Once its checksum is
    // destroyed, block one can only prove the older generation; repairing it
    // from that stale mirror would silently discard /latest.
    disk.corrupt_byte(0, 0);
    let damaged0 = disk.raw_block(0);
    let intact1 = disk.raw_block(1);
    assert!(matches!(
        FileSystem::repair(disk.clone()),
        Err(Error::Corrupt)
    ));
    assert_eq!(disk.raw_block(0), damaged0);
    assert_eq!(disk.raw_block(1), intact1);
}

#[test]
fn metadata_repair_refuses_record_corruption_and_preserves_valid_mirrors() {
    let disk = MemDisk::new(BLOCKS);
    let mut fs = format(disk.clone()).unwrap();
    fs.put("/old", b"old generation").unwrap();
    fs.fsync("/old").unwrap();
    fs.put("/new", b"new generation").unwrap();
    fs.fsync("/new").unwrap();
    let before0 = disk.raw_block(0);
    let before1 = disk.raw_block(1);
    let latest = fs.stats();
    let latest_record = latest.checkpoint_block + latest.used_blocks - 2;
    disk.corrupt_byte(latest_record, 64);

    assert!(matches!(
        FileSystem::repair(disk.clone()),
        Err(Error::Corrupt)
    ));
    assert_eq!(disk.raw_block(0), before0);
    assert_eq!(disk.raw_block(1), before1, "only the record was corrupted");

    // With both checksum-valid copies, repair is a no-op rather than treating
    // the older valid generation as a damaged mirror.
    let healthy = MemDisk::new(BLOCKS);
    let mut healthy_fs = format(healthy.clone()).unwrap();
    healthy_fs.put("/old", b"old generation").unwrap();
    healthy_fs.fsync("/old").unwrap();
    healthy_fs.put("/new", b"new generation").unwrap();
    healthy_fs.fsync("/new").unwrap();
    let healthy0 = healthy.raw_block(0);
    let healthy1 = healthy.raw_block(1);
    let (_, report) = FileSystem::repair(healthy.clone()).unwrap();
    assert_eq!(report.repaired_superblocks, 0);
    assert!(report.metadata.is_healthy());
    assert_eq!(healthy.raw_block(0), healthy0);
    assert_eq!(healthy.raw_block(1), healthy1);
}

#[test]
fn randomized_campaign_read_errors_are_observable_and_non_mutating() {
    let baseline = powercut_seeded_pair();
    for offset in 0..4 {
        let disk = baseline.fork();
        disk.arm_relative(FaultOp::Read, offset, FaultMode::Io);
        assert!(matches!(FileSystem::verify(disk.clone()), Err(Error::Io)));
        disk.clear_fault();
        let mut recovered = FileSystem::mount(disk).unwrap();
        assert_eq!(recovered.read("/a").unwrap(), b"old-a");
        assert_eq!(recovered.read("/b").unwrap(), b"old-b");
        recovered.check().unwrap();
    }
}
