use mfs1::{BLOCK_SIZE, BlockDevice, Error, FileSystem, format};
use std::env;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::process::ExitCode;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("mfsctl: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    let args: Vec<String> = env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("mkfs") if args.len() == 4 => {
            let mib: u64 = args[3].parse().map_err(|_| "invalid MiB size")?;
            let device = FileDevice::create(&args[2], mib)?;
            let fs = format(device).map_err(display)?;
            println!("formatted {} blocks generation={}", fs.stats().total_blocks, fs.stats().generation);
            Ok(())
        }
        Some("put") if args.len() == 5 => {
            let data = fs::read(&args[3]).map_err(|e| e.to_string())?;
            let mut fs = FileSystem::mount(FileDevice::open(&args[2])?).map_err(display)?;
            fs.put(&args[4], &data).map_err(display)?;
            fs.fsync(&args[4]).map_err(display)?;
            println!("wrote {} bytes to {}", data.len(), args[4]);
            Ok(())
        }
        Some("mkdir") if args.len() == 4 => {
            let mut fs = FileSystem::mount(FileDevice::open(&args[2])?).map_err(display)?;
            match fs.mkdir(&args[3]) {
                Ok(()) | Err(Error::AlreadyExists) => {}
                Err(error) => return Err(display(error)),
            }
            fs.sync().map_err(display)?;
            println!("directory {} ready", args[3]);
            Ok(())
        }
        Some("ls") if args.len() == 3 || args.len() == 4 => {
            let fs = FileSystem::mount(FileDevice::open(&args[2])?).map_err(display)?;
            for entry in fs.list(args.get(3).map(String::as_str).unwrap_or("/")).map_err(display)? {
                println!("{entry}");
            }
            Ok(())
        }
        Some("inspect") if args.len() == 3 => {
            let fs = FileSystem::mount(FileDevice::open(&args[2])?).map_err(display)?;
            println!("{:#?}", fs.stats());
            Ok(())
        }
        Some("fsck") if args.len() == 3 => {
            let (mut fs, metadata) =
                FileSystem::verify(FileDevice::open(&args[2])?).map_err(display)?;
            if !metadata.is_healthy() {
                return Err(format!(
                    "metadata redundancy degraded: checksum-valid={}/2 replay-verified={}/2; run mfsctl repair {}",
                    metadata.checksum_valid_superblocks,
                    metadata.replay_verified_superblocks,
                    args[2]
                ));
            }
            let stats = fs.check().map_err(display)?;
            println!("MFS1 clean generation={} transactions={} entries={} used_blocks={}", stats.generation, stats.transaction, stats.entries, stats.used_blocks);
            Ok(())
        }
        Some("repair") if args.len() == 3 => {
            let (mut fs, report) =
                FileSystem::repair(FileDevice::open(&args[2])?).map_err(display)?;
            let stats = fs.check().map_err(display)?;
            println!(
                "MFS1 repaired superblocks={} verified={}/2 generation={} transactions={} entries={} used_blocks={}",
                report.repaired_superblocks,
                report.metadata.replay_verified_superblocks,
                stats.generation,
                stats.transaction,
                stats.entries,
                stats.used_blocks
            );
            Ok(())
        }
        _ => Err("usage: mfsctl <mkfs IMAGE MIB|mkdir IMAGE PATH|put IMAGE SOURCE DEST|ls IMAGE [PATH]|inspect IMAGE|fsck IMAGE|repair IMAGE>".into()),
    }
}

fn display(error: Error) -> String {
    error.to_string()
}

struct FileDevice {
    file: File,
    blocks: u64,
}

impl FileDevice {
    fn create(path: &str, mib: u64) -> Result<Self, String> {
        if let Some(parent) = Path::new(path).parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .read(true)
            .write(true)
            .open(path)
            .map_err(|e| e.to_string())?;
        file.set_len(mib * 1024 * 1024).map_err(|e| e.to_string())?;
        Ok(Self {
            file,
            blocks: mib * 1024 * 1024 / BLOCK_SIZE as u64,
        })
    }

    fn open(path: &str) -> Result<Self, String> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)
            .map_err(|e| e.to_string())?;
        let len = file.metadata().map_err(|e| e.to_string())?.len();
        if len % BLOCK_SIZE as u64 != 0 {
            return Err("image size is not block aligned".into());
        }
        Ok(Self {
            file,
            blocks: len / BLOCK_SIZE as u64,
        })
    }
}

impl BlockDevice for FileDevice {
    fn block_count(&self) -> u64 {
        self.blocks
    }

    fn read_block(&mut self, block: u64, out: &mut [u8; BLOCK_SIZE]) -> Result<(), Error> {
        if block >= self.blocks {
            return Err(Error::Io);
        }
        self.file
            .seek(SeekFrom::Start(block * BLOCK_SIZE as u64))
            .map_err(|_| Error::Io)?;
        self.file.read_exact(out).map_err(|_| Error::Io)
    }

    fn write_block(&mut self, block: u64, data: &[u8; BLOCK_SIZE]) -> Result<(), Error> {
        if block >= self.blocks {
            return Err(Error::Io);
        }
        self.file
            .seek(SeekFrom::Start(block * BLOCK_SIZE as u64))
            .map_err(|_| Error::Io)?;
        self.file.write_all(data).map_err(|_| Error::Io)
    }

    fn flush(&mut self) -> Result<(), Error> {
        self.file.sync_data().map_err(|_| Error::Io)
    }
}
