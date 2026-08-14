use microsystem_abi::Status;

pub const VIRTIO_F_VERSION_1: u64 = 1 << 32;
pub const VIRTIO_BLK_F_FLUSH: u64 = 1 << 9;
pub const VRING_DESC_F_NEXT: u16 = 1;
pub const VRING_DESC_F_WRITE: u16 = 2;
pub const MAX_QUEUE_SIZE: usize = 128;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Descriptor {
    pub address: u64,
    pub length: u32,
    pub flags: u16,
    pub next: u16,
}

pub struct SplitQueue {
    size: u16,
    descriptors: [Descriptor; MAX_QUEUE_SIZE],
    free: [bool; MAX_QUEUE_SIZE],
    available_index: u16,
    used_index: u16,
}

impl SplitQueue {
    pub fn new(size: u16) -> Result<Self, Status> {
        if size == 0 || size as usize > MAX_QUEUE_SIZE || !size.is_power_of_two() {
            return Err(Status::Invalid);
        }
        Ok(Self {
            size,
            descriptors: [Descriptor::default(); MAX_QUEUE_SIZE],
            free: [true; MAX_QUEUE_SIZE],
            available_index: 0,
            used_index: 0,
        })
    }

    pub fn allocate_chain(&mut self, descriptors: &[Descriptor]) -> Result<u16, Status> {
        if descriptors.is_empty() || descriptors.len() > self.size as usize {
            return Err(Status::Invalid);
        }
        let mut indexes = [0u16; MAX_QUEUE_SIZE];
        let mut found = 0;
        for index in 0..self.size as usize {
            if self.free[index] {
                indexes[found] = index as u16;
                found += 1;
                if found == descriptors.len() {
                    break;
                }
            }
        }
        if found != descriptors.len() {
            return Err(Status::Busy);
        }
        for position in 0..found {
            let index = indexes[position] as usize;
            let mut descriptor = descriptors[position];
            if position + 1 < found {
                descriptor.flags |= VRING_DESC_F_NEXT;
                descriptor.next = indexes[position + 1];
            } else {
                descriptor.flags &= !VRING_DESC_F_NEXT;
                descriptor.next = 0;
            }
            self.descriptors[index] = descriptor;
            self.free[index] = false;
        }
        self.available_index = self.available_index.wrapping_add(1);
        Ok(indexes[0])
    }

    pub fn release_chain(&mut self, head: u16) -> Result<(), Status> {
        let mut index = head;
        for _ in 0..self.size {
            if index >= self.size || self.free[index as usize] {
                return Err(Status::Invalid);
            }
            let descriptor = self.descriptors[index as usize];
            self.free[index as usize] = true;
            self.descriptors[index as usize] = Descriptor::default();
            if descriptor.flags & VRING_DESC_F_NEXT == 0 {
                self.used_index = self.used_index.wrapping_add(1);
                return Ok(());
            }
            index = descriptor.next;
        }
        Err(Status::Corrupt)
    }

    pub const fn available_index(&self) -> u16 {
        self.available_index
    }
    pub const fn used_index(&self) -> u16 {
        self.used_index
    }
}

pub fn negotiate_block_features(device: u64) -> Result<u64, Status> {
    let required = VIRTIO_F_VERSION_1 | VIRTIO_BLK_F_FLUSH;
    if device & required != required {
        Err(Status::NotSupported)
    } else {
        Ok(device & required)
    }
}
