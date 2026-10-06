use crate::volume::{SharedFilesystem, VolumeDevice};
use alloc::rc::Rc;
use alloc::string::String;
use alloc::vec::Vec;
use core::cell::RefCell;
use mfs1::{BlockDevice, Error, FileSystem, Metadata, normalize};

pub const MAX_VOLUMES: usize = 4;
const CONFIG: &str = "/.system/volumes.mounts";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MountInfo {
    pub image: String,
    pub point: String,
    pub readonly: bool,
}

struct Mount<D: BlockDevice> {
    info: MountInfo,
    filesystem: SharedFilesystem<D>,
}

pub struct Vfs<D: BlockDevice> {
    root: SharedFilesystem<D>,
    mounts: Vec<Mount<D>>,
    pub failures: Vec<(usize, Error)>,
    timestamp: u64,
}

impl<D: BlockDevice> Vfs<D> {
    pub fn new(device: D) -> Result<Self, Error> {
        let root = Rc::new(RefCell::new(FileSystem::mount(VolumeDevice::Root(device))?));
        #[cfg(feature = "user-bin")]
        let _ = microsystem_user_rt::debug_write(b"[vfs] root replay complete\n");
        let mut vfs = Self {
            root,
            mounts: Vec::new(),
            failures: Vec::new(),
            timestamp: 0,
        };
        let config_result = vfs.root.borrow().read(CONFIG);
        let config = match config_result {
            Ok(bytes) => bytes,
            Err(Error::NotFound) => return Ok(vfs),
            Err(error) => {
                vfs.failures.push((0, error));
                return Ok(vfs);
            }
        };
        if config.len() > 4096 {
            vfs.failures.push((0, Error::NoSpace));
            return Ok(vfs);
        }
        let text = match core::str::from_utf8(&config) {
            Ok(text) => text,
            Err(_) => {
                vfs.failures.push((0, Error::Utf8));
                return Ok(vfs);
            }
        };
        for (index, row) in text.lines().filter(|row| !row.is_empty()).enumerate() {
            if index >= MAX_VOLUMES {
                vfs.failures.push((index + 1, Error::NoSpace));
                break;
            }
            let mut fields = row.split('\t');
            let image = fields.next().unwrap_or("");
            let point = fields.next().unwrap_or("");
            let mode = fields.next().unwrap_or("");
            let result = if fields.next().is_some() || !matches!(mode, "rw" | "ro") {
                Err(Error::Invalid)
            } else {
                vfs.prepare_mount(image, point, mode == "ro")
                    .and_then(|mount| {
                        vfs.mounts.try_reserve(1).map_err(|_| Error::NoSpace)?;
                        vfs.mounts.push(mount);
                        Ok(())
                    })
            };
            if let Err(error) = result {
                vfs.failures.push((index + 1, error));
            }
        }
        Ok(vfs)
    }

    pub fn set_timestamp(&mut self, timestamp: u64) {
        self.timestamp = timestamp;
        self.root.borrow_mut().set_timestamp(timestamp);
        for mount in &self.mounts {
            mount.filesystem.borrow_mut().set_timestamp(timestamp);
        }
    }

    pub fn mounts(&self) -> impl Iterator<Item = &MountInfo> {
        self.mounts.iter().map(|mount| &mount.info)
    }

    pub fn with<R>(
        &self,
        path: &str,
        write: bool,
        operation: impl FnOnce(&mut FileSystem<VolumeDevice<D>>, &str) -> Result<R, Error>,
    ) -> Result<R, Error> {
        let path = normalize(path)?;
        if write && self.mounts.iter().any(|mount| mount.info.image == path) {
            return Err(Error::Busy);
        }
        let (filesystem, local, readonly, _) = self.resolve(&path);
        if write && readonly {
            return Err(Error::ReadOnly);
        }
        let mut filesystem = filesystem.try_borrow_mut().map_err(|_| Error::Busy)?;
        operation(&mut filesystem, &local)
    }

    pub fn list(&self, path: &str) -> Result<Vec<String>, Error> {
        let path = normalize(path)?;
        let (filesystem, local, _, point) = self.resolve(&path);
        let mut entries = filesystem
            .try_borrow()
            .map_err(|_| Error::Busy)?
            .list(&local)?;
        if !point.is_empty() {
            for entry in &mut entries {
                *entry = point.clone() + entry;
            }
        }
        Ok(entries)
    }

    pub fn rename(&self, from: &str, to: &str, replace: bool) -> Result<(), Error> {
        let from = normalize(from)?;
        let to = normalize(to)?;
        if self.namespace_busy(&from) || self.namespace_busy(&to) {
            return Err(Error::Busy);
        }
        let (source, from, source_ro, _) = self.resolve(&from);
        let (target, to, target_ro, _) = self.resolve(&to);
        if !Rc::ptr_eq(&source, &target) {
            return Err(Error::CrossDevice);
        }
        if source_ro || target_ro {
            return Err(Error::ReadOnly);
        }
        let mut filesystem = source.try_borrow_mut().map_err(|_| Error::Busy)?;
        if replace {
            filesystem.replace_file(&from, &to)
        } else {
            filesystem.rename(&from, &to)
        }
    }

    pub fn unlink(&self, path: &str) -> Result<(), Error> {
        let path = normalize(path)?;
        if self.namespace_busy(&path) {
            return Err(Error::Busy);
        }
        self.with(&path, true, |filesystem, local| filesystem.unlink(local))
    }

    pub fn mount_image(&mut self, image: &str, point: &str, readonly: bool) -> Result<(), Error> {
        self.mounts.try_reserve(1).map_err(|_| Error::NoSpace)?;
        let mount = self.prepare_mount(image, point, readonly)?;
        self.with(point, false, |filesystem, local| filesystem.fsync(local))?;
        let config = self.configuration(Some(&mount.info), None)?;
        self.stage_configuration(&config)?;
        self.mounts.push(mount);
        self.failures.clear();
        // If the final flush fails, runtime state matches the staged config;
        // disk recovery may select either complete configuration snapshot.
        self.root.borrow_mut().fsync(CONFIG)
    }

    pub fn unmount(&mut self, point: &str) -> Result<(), Error> {
        let point = normalize(point)?;
        let index = self
            .mounts
            .iter()
            .position(|mount| mount.info.point == point)
            .ok_or(Error::NotFound)?;
        if self
            .mounts
            .iter()
            .enumerate()
            .any(|(other, mount)| other != index && within(&mount.info.point, &point))
        {
            return Err(Error::Busy);
        }
        self.mounts[index].filesystem.borrow_mut().sync()?;
        let config = self.configuration(None, Some(index))?;
        self.stage_configuration(&config)?;
        self.mounts.remove(index);
        self.failures.clear();
        self.root.borrow_mut().fsync(CONFIG)
    }

    pub fn format_image(&mut self, image: &str, mib: u32) -> Result<(), Error> {
        let image = normalize(image)?;
        if !(3..=8).contains(&mib)
            || image == CONFIG
            || self.resolve(&image).3 != ""
            || self.namespace_busy(&image)
        {
            return Err(Error::Invalid);
        }
        if self.root.borrow().metadata(&image) != Err(Error::NotFound) {
            return Err(Error::AlreadyExists);
        }
        let mut data = Vec::new();
        data.try_reserve_exact(mib as usize * 1024 * 1024)
            .map_err(|_| Error::NoSpace)?;
        data.resize(mib as usize * 1024 * 1024, 0);
        self.root.borrow_mut().put(&image, &data)?;
        self.root.borrow_mut().fsync(&image)?;
        drop(data);
        let device = VolumeDevice::image(self.root.clone(), image.clone())?;
        let mut filesystem = mfs1::format(device)?;
        filesystem.set_timestamp(self.timestamp);
        let mut attributes = filesystem.attributes("/")?;
        attributes.created = self.timestamp;
        attributes.modified = self.timestamp;
        attributes.accessed = self.timestamp;
        filesystem.set_attributes("/", attributes)?;
        filesystem.sync()
    }

    pub fn sync(&self) -> Result<(), Error> {
        for mount in &self.mounts {
            mount.filesystem.borrow_mut().sync()?;
        }
        self.root.borrow_mut().sync()
    }

    pub fn collect_once(&self) -> Result<bool, Error> {
        for mount in &self.mounts {
            let mut filesystem = mount.filesystem.borrow_mut();
            if !mount.info.readonly && filesystem.needs_gc() {
                filesystem.gc()?;
                return Ok(true);
            }
        }
        let mut root = self.root.borrow_mut();
        if root.needs_gc() {
            root.gc()?;
            return Ok(true);
        }
        Ok(false)
    }

    pub fn root_stats(&self) -> mfs1::Stats {
        self.root.borrow().stats()
    }

    fn prepare_mount(&self, image: &str, point: &str, readonly: bool) -> Result<Mount<D>, Error> {
        if self.mounts.len() == MAX_VOLUMES {
            return Err(Error::NoSpace);
        }
        let image = normalize(image)?;
        let point = normalize(point)?;
        if point == "/"
            || image == CONFIG
            || image.contains(['\t', '\n'])
            || point.contains(['\t', '\n'])
            || self.resolve(&image).3 != ""
        {
            return Err(Error::Invalid);
        }
        if self
            .mounts
            .iter()
            .any(|mount| mount.info.point == point || mount.info.image == image)
        {
            return Err(Error::Busy);
        }
        self.with(&point, false, |filesystem, local| {
            if filesystem.metadata(local)? != Metadata::Directory {
                return Err(Error::NotDirectory);
            }
            Ok(())
        })?;
        let device = VolumeDevice::image(self.root.clone(), image.clone())?;
        let mut filesystem = FileSystem::mount(device)?;
        #[cfg(feature = "user-bin")]
        let _ = microsystem_user_rt::debug_write(alloc::format!("[vfs] image replay complete point={point}\n").as_bytes());
        filesystem.set_timestamp(self.timestamp);
        Ok(Mount {
            info: MountInfo {
                image,
                point,
                readonly,
            },
            filesystem: Rc::new(RefCell::new(filesystem)),
        })
    }

    fn resolve(&self, path: &str) -> (SharedFilesystem<D>, String, bool, String) {
        if let Some(mount) = self
            .mounts
            .iter()
            .filter(|mount| within(path, &mount.info.point))
            .max_by_key(|mount| mount.info.point.len())
        {
            let local = &path[mount.info.point.len()..];
            return (
                mount.filesystem.clone(),
                if local.is_empty() {
                    "/".into()
                } else {
                    local.into()
                },
                mount.info.readonly,
                mount.info.point.clone(),
            );
        }
        (self.root.clone(), path.into(), false, String::new())
    }

    fn backing_affected(&self, path: &str) -> bool {
        self.mounts
            .iter()
            .any(|mount| within(&mount.info.image, path))
    }
    fn namespace_busy(&self, path: &str) -> bool {
        self.backing_affected(path)
            || self
                .mounts
                .iter()
                .any(|mount| within(&mount.info.point, path))
    }

    fn configuration(
        &self,
        extra: Option<&MountInfo>,
        skip: Option<usize>,
    ) -> Result<String, Error> {
        let mut config = String::new();
        config.try_reserve_exact(4096).map_err(|_| Error::NoSpace)?;
        for info in self
            .mounts
            .iter()
            .enumerate()
            .filter(|(index, _)| Some(*index) != skip)
            .map(|(_, mount)| &mount.info)
            .chain(extra)
        {
            config.push_str(&info.image);
            config.push('\t');
            config.push_str(&info.point);
            config.push_str(if info.readonly { "\tro\n" } else { "\trw\n" });
        }
        Ok(config)
    }

    fn stage_configuration(&self, config: &str) -> Result<(), Error> {
        let mut root = self.root.borrow_mut();
        match root.metadata("/.system") {
            Err(Error::NotFound) => root.mkdir("/.system")?,
            Ok(Metadata::Directory) => {}
            _ => return Err(Error::NotDirectory),
        }
        root.sync()?;
        root.put(CONFIG, config.as_bytes())
    }
}

pub fn within(path: &str, parent: &str) -> bool {
    path == parent
        || parent == "/"
        || path
            .strip_prefix(parent)
            .is_some_and(|suffix| suffix.starts_with('/'))
}
