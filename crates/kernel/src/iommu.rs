use microsystem_abi::Status;

pub const MAX_DMA_MAPPINGS: usize = 128;
pub const DMA_PAGE_SIZE: u64 = 4096;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DmaMapping {
    pub iova: u64,
    pub physical: u64,
    pub pages: u32,
    pub writable: bool,
}

pub struct DmaDomain {
    pub stream_id: u32,
    mappings: [Option<DmaMapping>; MAX_DMA_MAPPINGS],
}

impl DmaDomain {
    pub const fn new(stream_id: u32) -> Self {
        Self {
            stream_id,
            mappings: [None; MAX_DMA_MAPPINGS],
        }
    }

    pub fn map(&mut self, mapping: DmaMapping) -> Result<(), Status> {
        if mapping.pages == 0
            || mapping.iova % DMA_PAGE_SIZE != 0
            || mapping.physical % DMA_PAGE_SIZE != 0
        {
            return Err(Status::Invalid);
        }
        let length = mapping.pages as u64 * DMA_PAGE_SIZE;
        let end = mapping.iova.checked_add(length).ok_or(Status::Invalid)?;
        for current in self.mappings.iter().flatten() {
            let current_end = current.iova + current.pages as u64 * DMA_PAGE_SIZE;
            if mapping.iova < current_end && current.iova < end {
                return Err(Status::Busy);
            }
        }
        let slot = self
            .mappings
            .iter_mut()
            .find(|slot| slot.is_none())
            .ok_or(Status::NoMemory)?;
        *slot = Some(mapping);
        Ok(())
    }

    pub fn unmap(&mut self, iova: u64) -> Result<DmaMapping, Status> {
        let slot = self
            .mappings
            .iter_mut()
            .find(|slot| slot.map(|mapping| mapping.iova) == Some(iova))
            .ok_or(Status::NotFound)?;
        slot.take().ok_or(Status::NotFound)
    }

    pub fn translate(&self, iova: u64, write: bool) -> Result<u64, Status> {
        for mapping in self.mappings.iter().flatten() {
            let length = mapping.pages as u64 * DMA_PAGE_SIZE;
            if iova >= mapping.iova && iova < mapping.iova + length {
                if write && !mapping.writable {
                    return Err(Status::AccessDenied);
                }
                return Ok(mapping.physical + (iova - mapping.iova));
            }
        }
        Err(Status::Fault)
    }

    pub fn len(&self) -> usize {
        self.mappings
            .iter()
            .filter(|mapping| mapping.is_some())
            .count()
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}
