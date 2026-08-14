#![no_std]

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;
use mfs1::{BlockDevice, Error, FileSystem, Metadata, Stats};

pub use microsystem_abi::filesystem::Operation;

pub struct FileService<D: BlockDevice> {
    filesystem: FileSystem<D>,
    writeback_interval_ns: u64,
    next_writeback_ns: u64,
}

impl<D: BlockDevice> FileService<D> {
    pub fn mount(device: D, now_ns: u64) -> Result<Self, Error> {
        Ok(Self {
            filesystem: FileSystem::mount(device)?,
            writeback_interval_ns: 1_000_000_000,
            next_writeback_ns: now_ns + 1_000_000_000,
        })
    }
    pub fn read(&self, path: &str) -> Result<Vec<u8>, Error> {
        self.filesystem.read(path)
    }
    pub fn metadata(&self, path: &str) -> Result<Metadata, Error> {
        self.filesystem.metadata(path)
    }
    pub fn write(&mut self, path: &str, data: &[u8]) -> Result<(), Error> {
        self.filesystem.put(path, data)
    }
    pub fn mkdir(&mut self, path: &str) -> Result<(), Error> {
        self.filesystem.mkdir(path)
    }
    pub fn list(&self, path: &str) -> Result<Vec<String>, Error> {
        self.filesystem.list(path)
    }
    pub fn rename(&mut self, from: &str, to: &str) -> Result<(), Error> {
        self.filesystem.rename(from, to)
    }
    pub fn replace_file(&mut self, from: &str, to: &str) -> Result<(), Error> {
        self.filesystem.replace_file(from, to)
    }
    pub fn unlink(&mut self, path: &str) -> Result<(), Error> {
        self.filesystem.unlink(path)
    }
    pub fn fsync(&mut self, path: &str) -> Result<(), Error> {
        self.filesystem.fsync(path)
    }
    pub fn sync(&mut self) -> Result<(), Error> {
        self.filesystem.sync()
    }
    pub fn poll_writeback(&mut self, now_ns: u64) -> Result<bool, Error> {
        if now_ns < self.next_writeback_ns {
            return Ok(false);
        }
        self.filesystem.sync()?;
        self.next_writeback_ns = now_ns.saturating_add(self.writeback_interval_ns);
        Ok(true)
    }
    pub fn collect_once(&mut self) -> Result<bool, Error> {
        if !self.filesystem.needs_gc() {
            return Ok(false);
        }
        self.filesystem.gc()?;
        Ok(true)
    }
    pub fn stats(&self) -> Stats {
        self.filesystem.stats()
    }
}
