#![no_std]

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;
use mfs1::{Attributes, BlockDevice, Error, Metadata, Stats};

pub mod access;
mod vfs;
mod volume;
use vfs::Vfs;
pub use vfs::{MAX_VOLUMES, MountInfo};

pub use microsystem_abi::filesystem::Operation;

pub struct FileService<D: BlockDevice> {
    vfs: Vfs<D>,
    writeback_interval_ns: u64,
    next_writeback_ns: u64,
}

impl<D: BlockDevice> FileService<D> {
    pub fn mount(device: D, now_ns: u64) -> Result<Self, Error> {
        Ok(Self {
            vfs: Vfs::new(device)?,
            writeback_interval_ns: 1_000_000_000,
            next_writeback_ns: now_ns + 1_000_000_000,
        })
    }
    pub fn read(&self, path: &str) -> Result<Vec<u8>, Error> {
        self.vfs
            .with(path, false, |filesystem, local| filesystem.read(local))
    }
    pub fn read_range(&self, path: &str, offset: usize, maximum: usize) -> Result<Vec<u8>, Error> {
        self.vfs.with(path, false, |filesystem, local| {
            filesystem.read_range(local, offset, maximum)
        })
    }
    pub fn metadata(&self, path: &str) -> Result<Metadata, Error> {
        self.vfs
            .with(path, false, |filesystem, local| filesystem.metadata(local))
    }
    pub fn set_timestamp(&mut self, seconds: u64) {
        self.vfs.set_timestamp(seconds);
    }
    pub fn attributes(&self, path: &str) -> Result<Attributes, Error> {
        self.vfs.with(path, false, |filesystem, local| {
            filesystem.attributes(local)
        })
    }
    pub fn set_attributes(&mut self, path: &str, attributes: Attributes) -> Result<(), Error> {
        self.vfs.with(path, true, |filesystem, local| {
            filesystem.set_attributes(local, attributes)
        })
    }
    pub fn chmod(&mut self, path: &str, mode: u32) -> Result<(), Error> {
        self.vfs.with(path, true, |filesystem, local| {
            let mut attributes = filesystem.attributes(local)?;
            attributes.mode = mode;
            filesystem.set_attributes(local, attributes)
        })
    }
    pub fn chown(&mut self, path: &str, uid: u32, gid: u32) -> Result<(), Error> {
        self.vfs.with(path, true, |filesystem, local| {
            let mut attributes = filesystem.attributes(local)?;
            attributes.uid = uid;
            attributes.gid = gid;
            filesystem.set_attributes(local, attributes)
        })
    }
    pub fn write(&mut self, path: &str, data: &[u8]) -> Result<(), Error> {
        self.vfs
            .with(path, true, |filesystem, local| filesystem.put(local, data))
    }
    pub fn write_range(&mut self, path: &str, offset: usize, data: &[u8]) -> Result<(), Error> {
        self.vfs.with(path, true, |filesystem, local| {
            filesystem.write_range(local, offset, data)
        })
    }
    /// Appends and durably publishes one request without rewriting the file's
    /// previous bytes. Creates a missing file in one durable transaction.
    pub fn append(&mut self, path: &str, data: &[u8]) -> Result<(), Error> {
        self.vfs.with(path, true, |filesystem, local| {
            match filesystem.metadata(local) {
                Ok(Metadata::File { bytes }) => filesystem.write_range(local, bytes, data),
                Ok(Metadata::Directory) => Err(Error::IsDirectory),
                Err(Error::NotFound) => {
                    filesystem.put(local, data)?;
                    filesystem.fsync(local)
                }
                Err(error) => return Err(error),
            }
        })
    }
    pub fn mkdir(&mut self, path: &str) -> Result<(), Error> {
        self.vfs
            .with(path, true, |filesystem, local| filesystem.mkdir(local))
    }
    pub fn list(&self, path: &str) -> Result<Vec<String>, Error> {
        self.vfs.list(path)
    }
    pub fn rename(&mut self, from: &str, to: &str) -> Result<(), Error> {
        self.vfs.rename(from, to, false)
    }
    pub fn replace_file(&mut self, from: &str, to: &str) -> Result<(), Error> {
        self.vfs.rename(from, to, true)
    }
    pub fn unlink(&mut self, path: &str) -> Result<(), Error> {
        self.vfs.unlink(path)
    }
    pub fn fsync(&mut self, path: &str) -> Result<(), Error> {
        self.vfs
            .with(path, false, |filesystem, local| filesystem.fsync(local))
    }
    pub fn sync(&mut self) -> Result<(), Error> {
        self.vfs.sync()
    }
    pub fn poll_writeback(&mut self, now_ns: u64) -> Result<bool, Error> {
        if now_ns < self.next_writeback_ns {
            return Ok(false);
        }
        self.vfs.sync()?;
        self.next_writeback_ns = now_ns.saturating_add(self.writeback_interval_ns);
        Ok(true)
    }
    pub fn collect_once(&mut self) -> Result<bool, Error> {
        self.vfs.collect_once()
    }
    pub fn stats(&self) -> Stats {
        self.vfs.root_stats()
    }
    pub fn stats_path(&self, path: &str) -> Result<Stats, Error> {
        self.vfs.with(path, false, |filesystem, local| {
            filesystem.metadata(local)?;
            Ok(filesystem.stats())
        })
    }

    pub fn format_image(&mut self, image: &str, mib: u32) -> Result<(), Error> {
        self.vfs.format_image(image, mib)
    }
    pub fn mount_image(&mut self, image: &str, point: &str, readonly: bool) -> Result<(), Error> {
        self.vfs.mount_image(image, point, readonly)
    }
    pub fn unmount(&mut self, point: &str) -> Result<(), Error> {
        self.vfs.unmount(point)
    }
    pub fn mounts(&self) -> impl Iterator<Item = &MountInfo> {
        self.vfs.mounts()
    }
    pub fn mount_failures(&self) -> &[(usize, Error)] {
        &self.vfs.failures
    }
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use alloc::rc::Rc;
    use core::cell::RefCell;
    use mfs1::BLOCK_SIZE;

    #[derive(Clone)]
    struct Disk(Rc<RefCell<Vec<[u8; BLOCK_SIZE]>>>);
    impl BlockDevice for Disk {
        fn block_count(&self) -> u64 {
            self.0.borrow().len() as u64
        }
        fn read_block(&mut self, block: u64, output: &mut [u8; BLOCK_SIZE]) -> Result<(), Error> {
            output.copy_from_slice(self.0.borrow().get(block as usize).ok_or(Error::Io)?);
            Ok(())
        }
        fn write_block(&mut self, block: u64, bytes: &[u8; BLOCK_SIZE]) -> Result<(), Error> {
            *self
                .0
                .borrow_mut()
                .get_mut(block as usize)
                .ok_or(Error::Io)? = *bytes;
            Ok(())
        }
        fn flush(&mut self) -> Result<(), Error> {
            Ok(())
        }
    }

    fn prepared() -> (Disk, FileService<Disk>) {
        let disk = Disk(Rc::new(RefCell::new(alloc::vec![[0; BLOCK_SIZE]; 8192])));
        mfs1::format(disk.clone()).unwrap();
        let mut service = FileService::mount(disk.clone(), 0).unwrap();
        service.set_timestamp(1234);
        for path in ["/volumes", "/mnt", "/mnt/a", "/mnt/ab"] {
            service.mkdir(path).unwrap();
        }
        service.sync().unwrap();
        (disk, service)
    }

    #[test]
    fn mounted_images_preserve_namespace_metadata_and_readonly_after_restart() {
        let (disk, mut service) = prepared();
        for name in ["a", "ab"] {
            let image = alloc::format!("/volumes/{name}.img");
            let point = alloc::format!("/mnt/{name}");
            service.format_image(&image, 3).unwrap();
            service.mount_image(&image, &point, false).unwrap();
        }
        service.write("/mnt/a/note", b"first").unwrap();
        service.chmod("/mnt/a/note", 0o600).unwrap();
        service.chown("/mnt/a/note", 42, 7).unwrap();
        service.fsync("/mnt/a/note").unwrap();
        service.write("/mnt/ab/note", b"second").unwrap();
        service.fsync("/mnt/ab/note").unwrap();
        assert_eq!(service.list("/mnt/a").unwrap(), ["/mnt/a/note"]);
        assert_eq!(service.read("/mnt/ab/note").unwrap(), b"second");
        assert_eq!(
            service.stats_path("/mnt/a/note").unwrap().total_blocks,
            3 * 1024 * 1024 / BLOCK_SIZE as u64
        );
        assert_eq!(service.stats_path("/missing"), Err(Error::NotFound));
        assert_eq!(
            service.rename("/mnt/a/note", "/mnt/ab/new"),
            Err(Error::CrossDevice)
        );
        assert_eq!(
            service.write("/volumes/a.img", b"corrupt"),
            Err(Error::Busy)
        );
        assert_eq!(service.rename("/volumes", "/moved"), Err(Error::Busy));
        assert_eq!(service.unlink("/mnt/a"), Err(Error::Busy));
        drop(service);
        let mut service = FileService::mount(disk.clone(), 0).unwrap();
        assert!(service.mount_failures().is_empty());
        assert_eq!(service.mounts().count(), 2);
        assert_eq!(service.read("/mnt/a/note").unwrap(), b"first");
        let attributes = service.attributes("/mnt/a/note").unwrap();
        assert_eq!(
            (
                attributes.uid,
                attributes.gid,
                attributes.mode,
                attributes.created
            ),
            (42, 7, 0o600, 1234)
        );
        service.unmount("/mnt/a").unwrap();
        assert_eq!(service.read("/mnt/a/note"), Err(Error::NotFound));
        service
            .mount_image("/volumes/a.img", "/mnt/a", true)
            .unwrap();
        assert_eq!(
            service.write("/mnt/a/note", b"denied"),
            Err(Error::ReadOnly)
        );
        assert_eq!(
            service.append("/mnt/a/note", b"denied"),
            Err(Error::ReadOnly)
        );
        assert_eq!(service.chmod("/mnt/a/note", 0o777), Err(Error::ReadOnly));
        drop(service);
        let service = FileService::mount(disk, 0).unwrap();
        assert_eq!(service.read("/mnt/a/note").unwrap(), b"first");
        assert!(
            service
                .mounts()
                .any(|mount| mount.point == "/mnt/a" && mount.readonly)
        );
    }

    #[test]
    fn nested_mount_commits_parent_and_refuses_parent_unmount() {
        let (disk, mut service) = prepared();
        service.format_image("/volumes/a.img", 3).unwrap();
        service.format_image("/volumes/b.img", 3).unwrap();
        service
            .mount_image("/volumes/a.img", "/mnt/a", false)
            .unwrap();
        service.mkdir("/mnt/a/nested").unwrap();
        service
            .mount_image("/volumes/b.img", "/mnt/a/nested", false)
            .unwrap();
        service.write("/mnt/a/nested/note", b"nested").unwrap();
        service.fsync("/mnt/a/nested/note").unwrap();
        assert_eq!(service.unmount("/mnt/a"), Err(Error::Busy));
        drop(service);
        let mut restored = FileService::mount(disk, 0).unwrap();
        assert!(restored.mount_failures().is_empty());
        assert_eq!(restored.read("/mnt/a/nested/note").unwrap(), b"nested");
        restored.unmount("/mnt/a/nested").unwrap();
        restored.unmount("/mnt/a").unwrap();
        assert_eq!(restored.mounts().count(), 0);
    }

    #[test]
    fn invalid_stored_mount_preserves_root_and_valid_entries() {
        let (disk, mut service) = prepared();
        service.mkdir("/.system").unwrap();
        service.format_image("/volumes/a.img", 3).unwrap();
        service
            .write(
                "/.system/volumes.mounts",
                b"/missing.img\t/mnt/ab\trw\n/volumes/a.img\t/mnt/a\trw\n",
            )
            .unwrap();
        service.sync().unwrap();
        drop(service);
        let mut service = FileService::mount(disk, 0).unwrap();
        assert_eq!(service.mounts().count(), 1);
        assert_eq!(service.mount_failures(), &[(1, Error::NotFound)]);
        service.write("/root-proof", b"available").unwrap();
        service.fsync("/root-proof").unwrap();
        assert_eq!(service.read("/root-proof").unwrap(), b"available");
        service.unmount("/mnt/a").unwrap();
        assert!(service.mount_failures().is_empty());
    }
    #[test]
    fn user_access_preserves_ancestor_search_ownership_and_reader_restrictions() {
        use microsystem_identity::{Actor, OPERATOR, READER};
        let (_, mut fs) = prepared();
        fs.mkdir("/private").unwrap();
        fs.chown("/private", 1000, 1000).unwrap();
        fs.chmod("/private", 0o700).unwrap();
        fs.write("/private/public", b"secret through private directory")
            .unwrap();
        fs.chmod("/private/public", 0o644).unwrap();
        let alice = Actor {
            uid: 1000,
            gid: 1000,
            role: OPERATOR,
        };
        let bob = Actor {
            uid: 1001,
            gid: 1001,
            role: OPERATOR,
        };
        assert!(
            access::authorize(&fs, alice, Operation::Read as u16, "/private/public", &[]).is_ok()
        );
        assert_eq!(
            access::authorize(&fs, bob, Operation::Read as u16, "/private/public", &[]),
            Err(microsystem_abi::Status::AccessDenied)
        );
        fs.mkdir("/shared").unwrap();
        fs.chmod("/shared", 0o777).unwrap();
        assert!(access::authorize(&fs, bob, Operation::Write as u16, "/shared/new", &[]).is_ok());
        fs.write("/shared/new", b"owned").unwrap();
        access::own_created(&mut fs, bob, "/shared/new").unwrap();
        assert_eq!(fs.attributes("/shared/new").unwrap().uid, 1001);
        assert_eq!(
            access::authorize(&fs, alice, Operation::Write as u16, "/shared/new", &[]),
            Err(microsystem_abi::Status::AccessDenied)
        );
        let reader = Actor {
            role: READER,
            ..bob
        };
        assert_eq!(
            access::authorize(&fs, reader, Operation::Write as u16, "/shared/new", &[]),
            Err(microsystem_abi::Status::AccessDenied)
        );
        assert_eq!(
            access::authorize(&fs, bob, Operation::Chown as u16, "/shared/new", &[]),
            Err(microsystem_abi::Status::AccessDenied)
        );
        assert_eq!(
            access::authorize(
                &fs,
                bob,
                Operation::Rename as u16,
                "/shared/new",
                b"/private/stolen"
            ),
            Err(microsystem_abi::Status::AccessDenied)
        );
        assert!(
            access::authorize(
                &fs,
                Actor::SYSTEM,
                Operation::Read as u16,
                "/private/public",
                &[]
            )
            .is_ok()
        );
    }
}
