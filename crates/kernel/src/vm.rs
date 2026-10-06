use crate::memory::PAGE_SIZE;
use microsystem_abi::{Status, virtual_memory as vm};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Region {
    pub start: u64,
    pub bytes: u64,
    pub rights: u64,
}

impl Region {
    const EMPTY: Self = Self {
        start: 0,
        bytes: 0,
        rights: 0,
    };

    pub fn contains(self, address: u64) -> bool {
        self.bytes != 0 && address >= self.start && address - self.start < self.bytes
    }
}

pub struct AddressSpace {
    regions: [Region; vm::MAX_REGIONS],
    budget: u32,
    pub resident_pages: u32,
    pub faults: u64,
    pub allocation_failures: u64,
}

impl AddressSpace {
    pub const fn new() -> Self {
        Self {
            regions: [Region::EMPTY; vm::MAX_REGIONS],
            budget: vm::PAGE_BUDGET,
            resident_pages: 0,
            faults: 0,
            allocation_failures: 0,
        }
    }

    pub fn find(&self, address: u64) -> Option<Region> {
        self.regions
            .iter()
            .copied()
            .find(|region| region.contains(address))
    }

    pub fn regions(&self) -> [Region; vm::MAX_REGIONS] {
        self.regions
    }

    pub fn set_budget(&mut self, pages: u32) -> Result<(), Status> {
        if pages == 0 || pages > vm::PAGE_BUDGET || self.reserved_pages() > pages as u64 {
            return Err(Status::Invalid);
        }
        self.budget = pages;
        Ok(())
    }

    pub fn reserve(&mut self, requested: u64, bytes: u64, rights: u64) -> Result<u64, Status> {
        validate_rights(rights)?;
        validate_bytes(bytes)?;
        if self.reserved_pages() + bytes / PAGE_SIZE > self.budget as u64 {
            return Err(Status::NoMemory);
        }
        let slot = self
            .regions
            .iter()
            .position(|region| region.bytes == 0)
            .ok_or(Status::NoSpace)?;
        let start = if requested == 0 {
            let mut candidate = vm::START;
            loop {
                let end = candidate
                    .checked_add(bytes)
                    .filter(|end| *end <= vm::END)
                    .ok_or(Status::NoMemory)?;
                let conflict = self
                    .regions
                    .iter()
                    .filter(|region| region.bytes != 0)
                    .find(|region| candidate < region.start + region.bytes && region.start < end);
                match conflict {
                    Some(region) => candidate = region.start + region.bytes,
                    None => break candidate,
                }
            }
        } else {
            requested
        };
        validate_range(start, bytes)?;
        if self.overlaps(start, bytes, None) {
            return Err(Status::Busy);
        }
        self.regions[slot] = Region {
            start,
            bytes,
            rights,
        };
        Ok(start)
    }

    /// Changes one complete reservation; existing data is retained when growing.
    pub fn resize(&mut self, start: u64, bytes: u64) -> Result<Region, Status> {
        validate_range(start, bytes)?;
        let slot = self.exact(start)?;
        let old = self.regions[slot];
        if self.reserved_pages() - old.bytes / PAGE_SIZE + bytes / PAGE_SIZE > self.budget as u64 {
            return Err(Status::NoMemory);
        }
        if self.overlaps(start, bytes, Some(slot)) {
            return Err(Status::Busy);
        }
        self.regions[slot].bytes = bytes;
        Ok(old)
    }

    pub fn protect(&mut self, start: u64, rights: u64) -> Result<Region, Status> {
        validate_rights(rights)?;
        let slot = self.exact(start)?;
        self.regions[slot].rights = rights;
        Ok(self.regions[slot])
    }

    pub fn remove(&mut self, start: u64) -> Result<Region, Status> {
        let slot = self.exact(start)?;
        let region = self.regions[slot];
        self.regions[slot] = Region::EMPTY;
        Ok(region)
    }

    pub fn stats(&self) -> vm::StatsV1 {
        vm::StatsV1 {
            version: 1,
            regions: self
                .regions
                .iter()
                .filter(|region| region.bytes != 0)
                .count() as u32,
            reserved_pages: self.reserved_pages() as u32,
            resident_pages: self.resident_pages,
            page_budget: self.budget,
            reserved: 0,
            faults: self.faults,
            allocation_failures: self.allocation_failures,
        }
    }

    fn exact(&self, start: u64) -> Result<usize, Status> {
        self.regions
            .iter()
            .position(|region| region.bytes != 0 && region.start == start)
            .ok_or(Status::NotFound)
    }

    fn reserved_pages(&self) -> u64 {
        self.regions
            .iter()
            .map(|region| region.bytes / PAGE_SIZE)
            .sum()
    }

    fn overlaps(&self, start: u64, bytes: u64, excluded: Option<usize>) -> bool {
        self.regions.iter().enumerate().any(|(index, region)| {
            Some(index) != excluded
                && region.bytes != 0
                && start < region.start + region.bytes
                && region.start < start + bytes
        })
    }
}

fn validate_rights(rights: u64) -> Result<(), Status> {
    if rights & vm::READ == 0 || rights & !(vm::READ | vm::WRITE) != 0 {
        Err(Status::AccessDenied)
    } else {
        Ok(())
    }
}

fn validate_bytes(bytes: u64) -> Result<(), Status> {
    if bytes == 0 || bytes % PAGE_SIZE != 0 || bytes > vm::PAGE_BUDGET as u64 * PAGE_SIZE {
        Err(Status::Invalid)
    } else {
        Ok(())
    }
}

fn validate_range(start: u64, bytes: u64) -> Result<(), Status> {
    validate_bytes(bytes)?;
    if start < vm::START
        || start % PAGE_SIZE != 0
        || start.checked_add(bytes).is_none_or(|end| end > vm::END)
    {
        Err(Status::Invalid)
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn growth_never_overlaps_a_neighbour_and_failed_changes_are_atomic() {
        let mut space = AddressSpace::new();
        let first = space.reserve(0, PAGE_SIZE, vm::READ | vm::WRITE).unwrap();
        let second = space.reserve(0, PAGE_SIZE, vm::READ).unwrap();
        assert_eq!(space.resize(first, PAGE_SIZE * 2), Err(Status::Busy));
        assert_eq!(space.find(first).unwrap().bytes, PAGE_SIZE);
        assert_eq!(space.protect(first, vm::WRITE), Err(Status::AccessDenied));
        space.remove(second).unwrap();
        space.resize(first, PAGE_SIZE * 2).unwrap();
        assert!(space.find(first + PAGE_SIZE).is_some());
        assert_eq!(space.reserve(first, PAGE_SIZE, vm::READ), Err(Status::Busy));
        space.resize(first, PAGE_SIZE).unwrap();
        assert!(space.find(first + PAGE_SIZE).is_none());
        assert_eq!(space.reserve(0, PAGE_SIZE, vm::READ).unwrap(), second);
    }

    #[test]
    fn reservations_are_lazy_bounded_and_release_their_budget() {
        let mut space = AddressSpace::new();
        let bytes = vm::PAGE_BUDGET as u64 * PAGE_SIZE;
        let base = space.reserve(0, bytes, vm::READ).unwrap();
        assert_eq!(space.stats().resident_pages, 0);
        assert_eq!(space.reserve(0, PAGE_SIZE, vm::READ), Err(Status::NoMemory));
        space.remove(base).unwrap();
        assert_eq!(space.stats().reserved_pages, 0);
        assert!(
            space
                .reserve(vm::END - PAGE_SIZE, PAGE_SIZE, vm::READ)
                .is_ok()
        );
        assert_eq!(
            space.reserve(u64::MAX - PAGE_SIZE + 1, PAGE_SIZE, vm::READ),
            Err(Status::Invalid)
        );
    }
}
