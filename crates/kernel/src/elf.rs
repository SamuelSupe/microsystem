use microsystem_abi::Status;

pub const EM_AARCH64: u16 = 183;
pub const EM_RISCV: u16 = 243;
pub const EM_X86_64: u16 = 62;
pub const ET_EXEC: u16 = 2;
pub const PT_LOAD: u32 = 1;
pub const PF_X: u32 = 1;
pub const PF_W: u32 = 2;
pub const USER_MIN: u64 = 0x0040_0000;
pub const USER_MAX: u64 = 0x0000_007f_ffff_0000;

#[cfg(target_arch = "aarch64")]
const EXPECTED_MACHINE: u16 = EM_AARCH64;
#[cfg(target_arch = "riscv64")]
const EXPECTED_MACHINE: u16 = EM_RISCV;
#[cfg(target_arch = "x86_64")]
const EXPECTED_MACHINE: u16 = EM_X86_64;
#[cfg(not(any(
    target_arch = "aarch64",
    target_arch = "riscv64",
    target_arch = "x86_64"
)))]
const EXPECTED_MACHINE: u16 = EM_AARCH64;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LoadSegment {
    pub file_offset: u64,
    pub file_size: u64,
    pub memory_size: u64,
    pub virtual_address: u64,
    pub writable: bool,
    pub executable: bool,
}

pub struct ElfImage<'a> {
    bytes: &'a [u8],
    entry: u64,
    ph_offset: usize,
    ph_count: usize,
}

impl<'a> ElfImage<'a> {
    pub fn parse(bytes: &'a [u8]) -> Result<Self, Status> {
        if bytes.len() < 64 || &bytes[0..4] != b"\x7fELF" || bytes[4] != 2 || bytes[5] != 1 {
            return Err(Status::Invalid);
        }
        if u16_at(bytes, 16)? != ET_EXEC || u16_at(bytes, 18)? != EXPECTED_MACHINE {
            return Err(Status::NotSupported);
        }
        let ph_offset = u64_at(bytes, 32)? as usize;
        let ph_entry_size = u16_at(bytes, 54)? as usize;
        let ph_count = u16_at(bytes, 56)? as usize;
        if ph_entry_size != 56
            || ph_count == 0
            || ph_offset
                .checked_add(ph_entry_size.checked_mul(ph_count).ok_or(Status::Invalid)?)
                .filter(|&end| end <= bytes.len())
                .is_none()
        {
            return Err(Status::Invalid);
        }
        let entry = u64_at(bytes, 24)?;
        if !(USER_MIN..USER_MAX).contains(&entry) {
            return Err(Status::AccessDenied);
        }
        let image = Self {
            bytes,
            entry,
            ph_offset,
            ph_count,
        };
        let mut seen = [None; 32];
        let mut entry_is_executable = false;
        if ph_count > seen.len() {
            return Err(Status::NotSupported);
        }
        for (index, segment) in image.segments().enumerate() {
            let segment = segment?;
            if segment.writable && segment.executable {
                return Err(Status::AccessDenied);
            }
            let end = segment
                .virtual_address
                .checked_add(segment.memory_size)
                .ok_or(Status::Invalid)?;
            if segment.virtual_address < USER_MIN
                || end > USER_MAX
                || segment.file_size > segment.memory_size
            {
                return Err(Status::AccessDenied);
            }
            entry_is_executable |=
                segment.executable && segment.virtual_address <= entry && entry < end;
            let file_end = segment
                .file_offset
                .checked_add(segment.file_size)
                .ok_or(Status::Invalid)? as usize;
            if file_end > bytes.len() {
                return Err(Status::Invalid);
            }
            for previous in seen.iter().flatten() {
                let previous: &LoadSegment = previous;
                let previous_end = previous.virtual_address + previous.memory_size;
                if segment.virtual_address < previous_end && previous.virtual_address < end {
                    return Err(Status::Invalid);
                }
                if segment.memory_size != 0 && previous.memory_size != 0 {
                    let segment_page_end = end.checked_add(4095).ok_or(Status::Invalid)? & !4095;
                    let previous_page_end =
                        previous_end.checked_add(4095).ok_or(Status::Invalid)? & !4095;
                    let segment_page_start = segment.virtual_address & !4095;
                    let previous_page_start = previous.virtual_address & !4095;
                    if segment_page_start < previous_page_end
                        && previous_page_start < segment_page_end
                    {
                        return Err(Status::Invalid);
                    }
                }
            }
            seen[index] = Some(segment);
        }
        if !entry_is_executable {
            return Err(Status::Invalid);
        }
        Ok(image)
    }

    pub const fn entry(&self) -> u64 {
        self.entry
    }

    pub fn segments(&self) -> impl Iterator<Item = Result<LoadSegment, Status>> + '_ {
        (0..self.ph_count).filter_map(move |index| {
            let offset = self.ph_offset + index * 56;
            match u32_at(self.bytes, offset) {
                Ok(PT_LOAD) => Some(parse_segment(self.bytes, offset)),
                Ok(2 | 3) => Some(Err(Status::NotSupported)),
                Ok(_) => None,
                Err(error) => Some(Err(error)),
            }
        })
    }
}

fn parse_segment(bytes: &[u8], offset: usize) -> Result<LoadSegment, Status> {
    let flags = u32_at(bytes, offset + 4)?;
    Ok(LoadSegment {
        file_offset: u64_at(bytes, offset + 8)?,
        virtual_address: u64_at(bytes, offset + 16)?,
        file_size: u64_at(bytes, offset + 32)?,
        memory_size: u64_at(bytes, offset + 40)?,
        writable: flags & PF_W != 0,
        executable: flags & PF_X != 0,
    })
}

fn u16_at(bytes: &[u8], offset: usize) -> Result<u16, Status> {
    let value = bytes.get(offset..offset + 2).ok_or(Status::Invalid)?;
    Ok(u16::from_le_bytes(value.try_into().unwrap()))
}
fn u32_at(bytes: &[u8], offset: usize) -> Result<u32, Status> {
    let value = bytes.get(offset..offset + 4).ok_or(Status::Invalid)?;
    Ok(u32::from_le_bytes(value.try_into().unwrap()))
}
fn u64_at(bytes: &[u8], offset: usize) -> Result<u64, Status> {
    let value = bytes.get(offset..offset + 8).ok_or(Status::Invalid)?;
    Ok(u64::from_le_bytes(value.try_into().unwrap()))
}
