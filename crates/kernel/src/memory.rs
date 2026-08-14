use microsystem_abi::Status;

pub const PAGE_SIZE: u64 = 4096;
pub const MAX_TRACKED_FRAMES: usize = 65_536;
const BITMAP_WORDS: usize = MAX_TRACKED_FRAMES / 64;

pub struct FrameAllocator {
    base: u64,
    frame_count: usize,
    used: [u64; BITMAP_WORDS],
    reserved: [u64; BITMAP_WORDS],
}

impl FrameAllocator {
    pub const fn empty() -> Self {
        Self {
            base: 0,
            frame_count: 0,
            used: [u64::MAX; BITMAP_WORDS],
            reserved: [u64::MAX; BITMAP_WORDS],
        }
    }

    pub fn initialize(&mut self, base: u64, bytes: u64) -> Result<(), Status> {
        if base % PAGE_SIZE != 0 || bytes < PAGE_SIZE {
            return Err(Status::Invalid);
        }
        let count = (bytes / PAGE_SIZE).min(MAX_TRACKED_FRAMES as u64) as usize;
        self.base = base;
        self.frame_count = count;
        self.used.fill(0);
        self.reserved.fill(0);
        for index in count..MAX_TRACKED_FRAMES {
            self.set(index, true);
            self.set_reserved(index, true);
        }
        Ok(())
    }

    pub fn reserve(&mut self, start: u64, bytes: u64) -> Result<(), Status> {
        if start < self.base || bytes == 0 {
            return Err(Status::Invalid);
        }
        let first = ((start - self.base) / PAGE_SIZE) as usize;
        let last = ((start + bytes - 1 - self.base) / PAGE_SIZE) as usize;
        if last >= self.frame_count {
            return Err(Status::Invalid);
        }
        for index in first..=last {
            self.set(index, true);
            self.set_reserved(index, true);
        }
        Ok(())
    }

    pub fn allocate(&mut self) -> Result<u64, Status> {
        for index in 0..self.frame_count {
            if !self.get(index) {
                self.set(index, true);
                return Ok(self.base + index as u64 * PAGE_SIZE);
            }
        }
        Err(Status::NoMemory)
    }

    pub fn release(&mut self, address: u64) -> Result<(), Status> {
        if address < self.base || address % PAGE_SIZE != 0 {
            return Err(Status::Invalid);
        }
        let index = ((address - self.base) / PAGE_SIZE) as usize;
        if index >= self.frame_count || !self.get(index) || self.get_reserved(index) {
            return Err(Status::Invalid);
        }
        self.set(index, false);
        Ok(())
    }

    pub fn free_frames(&self) -> usize {
        (0..self.frame_count)
            .filter(|&index| !self.get(index))
            .count()
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
