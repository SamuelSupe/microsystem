use alloc::rc::Rc;
use alloc::string::String;
use alloc::vec::Vec;
use core::cell::RefCell;
use mfs1::{BLOCK_SIZE, BlockDevice, Error, FileSystem, Metadata};

pub type SharedFilesystem<D> = Rc<RefCell<FileSystem<VolumeDevice<D>>>>;

pub enum VolumeDevice<D: BlockDevice> {
    Root(D),
    Image {
        root: SharedFilesystem<D>,
        path: String,
        blocks: u64,
        pending: Vec<(u64, Vec<u8>)>,
    },
}

impl<D: BlockDevice> VolumeDevice<D> {
    pub fn image(root: SharedFilesystem<D>, path: String) -> Result<Self, Error> {
        let bytes = match root
            .try_borrow()
            .map_err(|_| Error::Busy)?
            .metadata(&path)?
        {
            Metadata::File { bytes } => bytes,
            Metadata::Directory => return Err(Error::IsDirectory),
        };
        if bytes % BLOCK_SIZE != 0 || bytes < (2 * mfs1::SEGMENT_BLOCKS + 2) as usize * BLOCK_SIZE {
            return Err(Error::Invalid);
        }
        Ok(Self::Image {
            root,
            path,
            blocks: (bytes / BLOCK_SIZE) as u64,
            pending: Vec::new(),
        })
    }
}

impl<D: BlockDevice> BlockDevice for VolumeDevice<D> {
    fn block_count(&self) -> u64 {
        match self {
            Self::Root(device) => device.block_count(),
            Self::Image { blocks, .. } => *blocks,
        }
    }

    fn read_block(&mut self, block: u64, out: &mut [u8; BLOCK_SIZE]) -> Result<(), Error> {
        match self {
            Self::Root(device) => device.read_block(block, out),
            Self::Image {
                root,
                path,
                blocks,
                pending,
            } => {
                if block >= *blocks {
                    return Err(Error::Io);
                }
                if let Some((_, data)) = pending.iter().find(|(index, _)| *index == block) {
                    out.copy_from_slice(data);
                    return Ok(());
                }
                let data = root.try_borrow().map_err(|_| Error::Busy)?.read_range(
                    path,
                    block as usize * BLOCK_SIZE,
                    BLOCK_SIZE,
                )?;
                if data.len() != BLOCK_SIZE {
                    return Err(Error::Io);
                }
                out.copy_from_slice(&data);
                Ok(())
            }
        }
    }

    fn write_block(&mut self, block: u64, data: &[u8; BLOCK_SIZE]) -> Result<(), Error> {
        match self {
            Self::Root(device) => device.write_block(block, data),
            Self::Image {
                blocks, pending, ..
            } => {
                if block >= *blocks {
                    return Err(Error::Io);
                }
                if let Some((_, bytes)) = pending.iter_mut().find(|(index, _)| *index == block) {
                    bytes.copy_from_slice(data);
                    return Ok(());
                }
                let mut bytes = Vec::new();
                bytes
                    .try_reserve_exact(BLOCK_SIZE)
                    .map_err(|_| Error::NoSpace)?;
                bytes.extend_from_slice(data);
                pending.try_reserve(1).map_err(|_| Error::NoSpace)?;
                pending.push((block, bytes));
                Ok(())
            }
        }
    }

    fn flush(&mut self) -> Result<(), Error> {
        match self {
            Self::Root(device) => device.flush(),
            Self::Image {
                root,
                path,
                pending,
                ..
            } => {
                if pending.is_empty() {
                    return Ok(());
                }
                let mut ranges = Vec::new();
                ranges
                    .try_reserve_exact(pending.len())
                    .map_err(|_| Error::NoSpace)?;
                for (block, bytes) in pending.iter() {
                    ranges.push((*block as usize * BLOCK_SIZE, bytes.as_slice()));
                }
                root.try_borrow_mut()
                    .map_err(|_| Error::Busy)?
                    .write_ranges(path, &ranges)?;
                pending.clear();
                Ok(())
            }
        }
    }

    fn commit_stage(&mut self, stage: mfs1::CommitStage) {
        if let Self::Root(device) = self {
            device.commit_stage(stage);
        }
    }
}
