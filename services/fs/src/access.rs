use crate::FileService;
use alloc::string::{String, ToString};
use mfs1::{BlockDevice, Error, Metadata};
use microsystem_abi::{Status, filesystem::Operation};
use microsystem_identity::{Actor, READER};

fn error(error: Error) -> Status {
    match error {
        Error::NotFound => Status::NotFound,
        Error::NoSpace => Status::NoSpace,
        Error::Io => Status::Io,
        _ => Status::Invalid,
    }
}
fn parent(path: &str) -> &str {
    path.rsplit_once('/')
        .map(|(parent, _)| if parent.is_empty() { "/" } else { parent })
        .unwrap_or("/")
}
fn allowed<D: BlockDevice>(
    fs: &FileService<D>,
    actor: Actor,
    path: &str,
    bits: u32,
) -> Result<(), Status> {
    let attributes = fs.attributes(path).map_err(error)?;
    if actor.access(attributes.uid, attributes.gid, attributes.mode, bits) {
        Ok(())
    } else {
        Err(Status::AccessDenied)
    }
}
fn search<D: BlockDevice>(fs: &FileService<D>, actor: Actor, path: &str) -> Result<(), Status> {
    let mut directory = parent(path);
    loop {
        if fs.metadata(directory).map_err(error)? != Metadata::Directory {
            return Err(Status::Invalid);
        }
        allowed(fs, actor, directory, 1)?;
        if directory == "/" {
            return Ok(());
        }
        directory = parent(directory);
    }
}
fn writable<D: BlockDevice>(fs: &FileService<D>, actor: Actor, path: &str) -> Result<(), Status> {
    search(fs, actor, path)?;
    match fs.metadata(path) {
        Ok(_) => allowed(fs, actor, path, 2),
        Err(Error::NotFound) => allowed(fs, actor, parent(path), 3),
        Err(e) => Err(error(e)),
    }
}

/// Authorizes a normalized VFS path, including search permission on every
/// ancestor. Descriptor requests must first resolve their stored path here.
pub fn authorize<D: BlockDevice>(
    fs: &FileService<D>,
    actor: Actor,
    operation: u16,
    path: &str,
    data: &[u8],
) -> Result<String, Status> {
    let operation = Operation::from_u64(operation as u64).ok_or(Status::Invalid)?;
    if matches!(operation, Operation::Close | Operation::MountList) {
        return Ok(path.to_string());
    }
    if matches!(
        operation,
        Operation::MountImage
            | Operation::Unmount
            | Operation::FormatImage
            | Operation::Sync
            | Operation::Chown
    ) && !actor.administrator()
    {
        return Err(Status::AccessDenied);
    }
    let path = mfs1::normalize(if path.is_empty() { "/" } else { path }).map_err(error)?;
    search(fs, actor, &path)?;
    match operation {
        Operation::Read | Operation::ReadRange | Operation::Open => allowed(fs, actor, &path, 4)?,
        Operation::List => allowed(fs, actor, &path, 5)?,
        Operation::Write | Operation::WriteRange | Operation::WriteAtomic | Operation::Append => {
            writable(fs, actor, &path)?
        }
        Operation::Mkdir => {
            if fs.metadata(&path).is_err() {
                allowed(fs, actor, parent(&path), 3)?;
            }
        }
        Operation::Unlink => allowed(fs, actor, parent(&path), 3)?,
        Operation::Rename | Operation::Replace => {
            allowed(fs, actor, parent(&path), 3)?;
            let destination = core::str::from_utf8(data).map_err(|_| Status::Invalid)?;
            let destination = mfs1::normalize(destination).map_err(error)?;
            search(fs, actor, &destination)?;
            allowed(fs, actor, parent(&destination), 3)?;
        }
        Operation::Copy => {
            allowed(fs, actor, &path, 4)?;
            let destination = core::str::from_utf8(data).map_err(|_| Status::Invalid)?;
            let destination = mfs1::normalize(destination).map_err(error)?;
            writable(fs, actor, &destination)?;
        }
        Operation::Chmod => {
            let attributes = fs.attributes(&path).map_err(error)?;
            if actor.role == READER || (!actor.administrator() && actor.uid != attributes.uid) {
                return Err(Status::AccessDenied);
            }
        }
        _ => {}
    }
    Ok(path)
}

/// Assigns creation ownership before the caller can publish the new file.
pub fn own_created<D: BlockDevice>(
    fs: &mut FileService<D>,
    actor: Actor,
    path: &str,
) -> Result<(), Error> {
    fs.chown(path, actor.uid, actor.gid)
}
