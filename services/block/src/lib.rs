#![no_std]

use microsystem_abi::{CapHandle, Status};

pub const SECTOR_SIZE: u32 = 512;

#[repr(C)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Geometry {
    pub sectors: u64,
    pub sector_size: u32,
    pub max_segments: u16,
    pub flush_supported: u8,
    pub read_only: u8,
}

#[repr(u16)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Operation {
    Geometry = 1,
    Read = 2,
    Write = 3,
    Flush = 4,
    Configure = 100,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Request {
    pub operation: Operation,
    pub flags: u16,
    pub sector: u64,
    pub bytes: u32,
    pub frame: CapHandle,
    pub frame_offset: u32,
}

impl Request {
    pub fn validate(&self, geometry: Geometry) -> Result<(), Status> {
        match self.operation {
            Operation::Geometry | Operation::Flush if self.bytes == 0 => Ok(()),
            Operation::Read | Operation::Write => {
                if self.bytes == 0 || self.bytes % geometry.sector_size != 0 {
                    return Err(Status::Invalid);
                }
                let sectors = self.bytes as u64 / geometry.sector_size as u64;
                if self
                    .sector
                    .checked_add(sectors)
                    .filter(|&end| end <= geometry.sectors)
                    .is_none()
                {
                    return Err(Status::Invalid);
                }
                if self.frame == CapHandle::INVALID {
                    return Err(Status::BadCapability);
                }
                Ok(())
            }
            _ => Err(Status::Invalid),
        }
    }
}
