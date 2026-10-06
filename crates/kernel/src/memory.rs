use microsystem_abi::Status;

pub const PAGE_SIZE: u64 = 4096;
pub const MAX_TRACKED_FRAMES: usize = 65_536;
pub const MAX_MEMORY_REGIONS: usize = 8;
const BITMAP_WORDS: usize = MAX_TRACKED_FRAMES / 64;

/// A physical RAM interval described by its first byte and total byte length.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhysicalRange {
    pub base: u64,
    pub bytes: u64,
}

const EMPTY_RANGE: PhysicalRange = PhysicalRange { base: 0, bytes: 0 };

pub struct FrameAllocator {
    ranges: [PhysicalRange; MAX_MEMORY_REGIONS],
    range_count: usize,
    frame_count: usize,
    next_free: usize,
    free_count: usize,
    used: [u64; BITMAP_WORDS],
    reserved: [u64; BITMAP_WORDS],
}

impl FrameAllocator {
    pub const fn empty() -> Self {
        Self {
            ranges: [EMPTY_RANGE; MAX_MEMORY_REGIONS],
            range_count: 0,
            frame_count: 0,
            next_free: 0,
            free_count: 0,
            used: [u64::MAX; BITMAP_WORDS],
            reserved: [u64::MAX; BITMAP_WORDS],
        }
    }

    pub fn initialize(&mut self, base: u64, bytes: u64) -> Result<(), Status> {
        self.initialize_regions(&[PhysicalRange { base, bytes }])
    }

    /// Tracks page-aligned, non-overlapping RAM intervals in order, up to the
    /// allocator's fixed frame limit. Later intervals may be left untracked.
    pub fn initialize_regions(&mut self, input: &[PhysicalRange]) -> Result<(), Status> {
        if input.is_empty() || input.len() > MAX_MEMORY_REGIONS {
            return Err(Status::Invalid);
        }
        let mut ranges = [EMPTY_RANGE; MAX_MEMORY_REGIONS];
        let mut range_count = 0;
        let mut count = 0usize;
        for region in input {
            if region.base % PAGE_SIZE != 0 || region.bytes < PAGE_SIZE {
                return Err(Status::Invalid);
            }
            region
                .base
                .checked_add(region.bytes)
                .ok_or(Status::Invalid)?;
            let pages = (region.bytes / PAGE_SIZE) as usize;
            let tracked_pages = pages.min(MAX_TRACKED_FRAMES - count);
            if tracked_pages == 0 {
                break;
            }
            for existing in &ranges[..range_count] {
                let existing_end = existing.base + existing.bytes;
                if region.base < existing_end
                    && existing.base < region.base + tracked_pages as u64 * PAGE_SIZE
                {
                    return Err(Status::Invalid);
                }
            }
            ranges[range_count] = PhysicalRange {
                base: region.base,
                bytes: tracked_pages as u64 * PAGE_SIZE,
            };
            range_count += 1;
            count += tracked_pages;
            if count == MAX_TRACKED_FRAMES {
                break;
            }
        }
        if count == 0 {
            return Err(Status::Invalid);
        }
        self.ranges = ranges;
        self.range_count = range_count;
        self.frame_count = count;
        self.next_free = 0;
        self.free_count = count;
        self.used.fill(0);
        self.reserved.fill(0);
        for index in count..MAX_TRACKED_FRAMES {
            self.set(index, true);
            self.set_reserved(index, true);
        }
        Ok(())
    }

    /// Reserves every tracked frame touched by a physical byte interval.
    pub fn reserve(&mut self, start: u64, bytes: u64) -> Result<(), Status> {
        if bytes == 0 {
            return Ok(());
        }
        let end = start.checked_add(bytes).ok_or(Status::Invalid)?;
        if end <= start {
            return Err(Status::Invalid);
        }
        let mut dense_base = 0usize;
        for range_index in 0..self.range_count {
            let region = self.ranges[range_index];
            let region_end = region.base + region.bytes;
            let overlap_start = start.max(region.base);
            let overlap_end = end.min(region_end);
            if overlap_start < overlap_end {
                let first = ((overlap_start - region.base) / PAGE_SIZE) as usize;
                let last_exclusive =
                    ((overlap_end - region.base + PAGE_SIZE - 1) / PAGE_SIZE) as usize;
                for local_index in first..last_exclusive {
                    let index = dense_base + local_index;
                    if index < self.frame_count {
                        if !self.get(index) {
                            self.free_count -= 1;
                        }
                        self.set(index, true);
                        self.set_reserved(index, true);
                    }
                }
            }
            dense_base += (region.bytes / PAGE_SIZE) as usize;
        }
        Ok(())
    }

    pub fn allocate(&mut self) -> Result<u64, Status> {
        if self.free_count == 0 {
            return Err(Status::NoMemory);
        }
        for range in [self.next_free..self.frame_count, 0..self.next_free] {
            for index in range {
                if !self.get(index) {
                    self.set(index, true);
                    self.free_count -= 1;
                    self.next_free = if index + 1 == self.frame_count {
                        0
                    } else {
                        index + 1
                    };
                    return self.physical_address(index).ok_or(Status::NoMemory);
                }
            }
        }
        Err(Status::NoMemory)
    }

    pub fn release(&mut self, address: u64) -> Result<(), Status> {
        if address % PAGE_SIZE != 0 {
            return Err(Status::Invalid);
        }
        let Some(index) = self.frame_index(address) else {
            return Err(Status::Invalid);
        };
        if !self.get(index) || self.get_reserved(index) {
            return Err(Status::Invalid);
        }
        self.set(index, false);
        self.free_count += 1;
        self.next_free = self.next_free.min(index);
        Ok(())
    }

    pub fn free_frames(&self) -> usize {
        self.free_count
    }

    fn physical_address(&self, index: usize) -> Option<u64> {
        if index >= self.frame_count {
            return None;
        }
        let mut remaining = index;
        for region in &self.ranges[..self.range_count] {
            let pages = (region.bytes / PAGE_SIZE) as usize;
            if remaining < pages {
                return Some(region.base + remaining as u64 * PAGE_SIZE);
            }
            remaining -= pages;
        }
        None
    }

    fn frame_index(&self, address: u64) -> Option<usize> {
        let mut dense_base = 0usize;
        for region in &self.ranges[..self.range_count] {
            let region_end = region.base + region.bytes;
            if address >= region.base && address < region_end {
                return Some(dense_base + ((address - region.base) / PAGE_SIZE) as usize);
            }
            dense_base += (region.bytes / PAGE_SIZE) as usize;
        }
        None
    }

    fn get(&self, index: usize) -> bool {
        self.used[index / 64] & (1u64 << (index % 64)) != 0
    }
    fn get_reserved(&self, index: usize) -> bool {
        self.reserved[index / 64] & (1u64 << (index % 64)) != 0
    }
    fn set(&mut self, index: usize, value: bool) {
        let mask = 1u64 << (index % 64);
        if value {
            self.used[index / 64] |= mask;
        } else {
            self.used[index / 64] &= !mask;
        }
    }
    fn set_reserved(&mut self, index: usize, value: bool) {
        let mask = 1u64 << (index % 64);
        if value {
            self.reserved[index / 64] |= mask;
        } else {
            self.reserved[index / 64] &= !mask;
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Mapping {
    pub virtual_address: u64,
    pub physical_address: u64,
    pub writable: bool,
    pub executable: bool,
    pub user: bool,
}

impl Mapping {
    pub fn validate(self) -> Result<Self, Status> {
        if self.virtual_address % PAGE_SIZE != 0 || self.physical_address % PAGE_SIZE != 0 {
            return Err(Status::Invalid);
        }
        if self.writable && self.executable {
            return Err(Status::AccessDenied);
        }
        Ok(self)
    }
}
