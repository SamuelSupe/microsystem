//! Bounded Multiboot2 and ACPI discovery. Unknown entries are skipped; malformed
//! lengths/checksums and truncated topology fail before resources are published.
use crate::memory::{MAX_MEMORY_REGIONS, PhysicalRange};

pub const MAX_CPUS: usize = 64;
pub const MAX_RESERVED: usize = 32;
const EMPTY: PhysicalRange = PhysicalRange { base: 0, bytes: 0 };

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    Truncated,
    Invalid,
    Capacity,
    Missing,
}

pub struct BootInfo<'a> {
    pub memory: [PhysicalRange; MAX_MEMORY_REGIONS],
    pub memory_count: usize,
    pub reserved: [PhysicalRange; MAX_RESERVED],
    pub reserved_count: usize,
    pub rsdp: Option<&'a [u8]>,
}

pub struct AcpiInfo {
    pub cpu_ids: [u32; MAX_CPUS],
    pub cpu_count: usize,
    pub local_apic: u64,
    pub io_apic: u64,
    pub io_apic_gsi_base: u32,
    pub ecam: u64,
    pub ecam_start_bus: u8,
    pub ecam_end_bus: u8,
    pub iommu: u64,
    pub dmar_units: usize,
    pub pci_mmio: PhysicalRange,
    pub iommu_requesters: [u16; 32],
    pub iommu_requester_count: usize,
    pub iommu_include_all: bool,
}

impl Default for AcpiInfo {
    fn default() -> Self {
        Self {
            cpu_ids: [0; MAX_CPUS],
            cpu_count: 0,
            local_apic: 0,
            io_apic: 0,
            io_apic_gsi_base: 0,
            ecam: 0,
            ecam_start_bus: 0,
            ecam_end_bus: 0,
            iommu: 0,
            dmar_units: 0,
            pci_mmio: EMPTY,
            iommu_requesters: [0; 32],
            iommu_requester_count: 0,
            iommu_include_all: false,
        }
    }
}

pub fn multiboot(bytes: &[u8]) -> Result<BootInfo<'_>, Error> {
    let total = u32_at(bytes, 0)? as usize;
    if total < 16 || total > bytes.len() || total % 8 != 0 {
        return Err(Error::Invalid);
    }
    let bytes = &bytes[..total];
    let mut result = BootInfo {
        memory: [EMPTY; MAX_MEMORY_REGIONS],
        memory_count: 0,
        reserved: [EMPTY; MAX_RESERVED],
        reserved_count: 0,
        rsdp: None,
    };
    let mut cursor = 8;
    let mut ended = false;
    while cursor < total {
        let kind = u32_at(bytes, cursor)?;
        let length = u32_at(bytes, cursor + 4)? as usize;
        if length < 8 {
            return Err(Error::Invalid);
        }
        let end = cursor
            .checked_add(length)
            .filter(|end| *end <= total)
            .ok_or(Error::Truncated)?;
        let tag = &bytes[cursor..end];
        match kind {
            0 => {
                if length != 8 {
                    return Err(Error::Invalid);
                }
                ended = true;
                break;
            }
            3 => {
                let base = u32_at(tag, 8)? as u64;
                let end = u32_at(tag, 12)? as u64;
                if end < base {
                    return Err(Error::Invalid);
                }
                append(
                    &mut result.reserved,
                    &mut result.reserved_count,
                    PhysicalRange {
                        base,
                        bytes: end - base,
                    },
                )?;
            }
            6 => {
                let stride = u32_at(tag, 8)? as usize;
                if stride < 24
                    || length < 16
                    || (length - 16) % stride != 0
                    || u32_at(tag, 12)? != 0
                {
                    return Err(Error::Invalid);
                }
                for entry in tag[16..].chunks_exact(stride) {
                    let range = PhysicalRange {
                        base: u64_at(entry, 0)?,
                        bytes: u64_at(entry, 8)?,
                    };
                    if range.base.checked_add(range.bytes).is_none() {
                        return Err(Error::Invalid);
                    }
                    if u32_at(entry, 16)? == 1 {
                        append(&mut result.memory, &mut result.memory_count, range)?;
                    } else {
                        append(&mut result.reserved, &mut result.reserved_count, range)?;
                    }
                }
            }
            14 if result.rsdp.is_none() => {
                result.rsdp = Some(&tag[8..]);
            }
            15 => {
                result.rsdp = Some(&tag[8..]);
            }
            _ => {}
        }
        cursor = end.checked_add(7).ok_or(Error::Invalid)? & !7;
    }
    if !ended || result.memory_count == 0 {
        return Err(Error::Missing);
    }
    Ok(result)
}

pub fn acpi<'a>(
    rsdp: &[u8],
    mut read: impl FnMut(u64, usize) -> Option<&'a [u8]>,
) -> Result<AcpiInfo, Error> {
    if rsdp.get(..8) != Some(b"RSD PTR ") || !checksum(rsdp.get(..20).ok_or(Error::Truncated)?) {
        return Err(Error::Invalid);
    }
    let extended = *rsdp.get(15).ok_or(Error::Truncated)? >= 2;
    let root_address = if extended {
        let length = u32_at(rsdp, 20)? as usize;
        if !(36..=4096).contains(&length) || !checksum(rsdp.get(..length).ok_or(Error::Truncated)?)
        {
            return Err(Error::Invalid);
        }
        u64_at(rsdp, 24)?
    } else {
        u32_at(rsdp, 16)? as u64
    };
    let root = table(&mut read, root_address)?;
    let width = if extended { 8 } else { 4 };
    if root.get(..4) != Some(if extended { b"XSDT" } else { b"RSDT" })
        || (root.len() - 36) % width != 0
    {
        return Err(Error::Invalid);
    }
    let mut result = AcpiInfo {
        cpu_ids: [0; MAX_CPUS],
        ..Default::default()
    };
    for pointer in root[36..].chunks_exact(width) {
        let address = if extended {
            u64_at(pointer, 0)?
        } else {
            u32_at(pointer, 0)? as u64
        };
        let child = table(&mut read, address)?;
        match &child[..4] {
            b"APIC" => parse_madt(child, &mut result)?,
            b"FACP" => {
                let extended = if child.len() >= 148 {
                    u64_at(child, 140)?
                } else {
                    0
                };
                let address = if extended != 0 {
                    extended
                } else {
                    u32_at(child, 40)? as u64
                };
                let dsdt = table(&mut read, address)?;
                if dsdt.get(..4) != Some(b"DSDT") {
                    return Err(Error::Invalid);
                }
                result.pci_mmio = pci_root_resources(&dsdt[36..])?.unwrap_or(EMPTY);
            }
            b"MCFG" => {
                if child.len() < 44 || (child.len() - 44) % 16 != 0 {
                    return Err(Error::Invalid);
                }
                for entry in child[44..].chunks_exact(16) {
                    if u16_at(entry, 8)? == 0 && entry[10] == 0 {
                        let address = u64_at(entry, 0)?;
                        if address == 0 || address & 0xfffff != 0 || entry[11] < entry[10] {
                            return Err(Error::Invalid);
                        }
                        result.ecam = address;
                        result.ecam_start_bus = entry[10];
                        result.ecam_end_bus = entry[11];
                        break;
                    }
                }
            }
            b"DMAR" => {
                if child.len() < 48 {
                    return Err(Error::Truncated);
                }
                let mut offset = 48;
                while offset < child.len() {
                    let length = u16_at(child, offset + 2)? as usize;
                    if length < 4 || offset + length > child.len() {
                        return Err(Error::Invalid);
                    }
                    if u16_at(child, offset)? == 0 {
                        if length < 16 {
                            return Err(Error::Invalid);
                        }
                        result.dmar_units += 1;
                        if u16_at(child, offset + 6)? == 0 {
                            if result.iommu != 0 {
                                return Err(Error::Capacity);
                            }
                            result.iommu = u64_at(child, offset + 8)?;
                            result.iommu_include_all = child[offset + 4] & 1 != 0;
                            if child[offset + 4] & 1 == 0 {
                                let mut scope = offset + 16;
                                while scope < offset + length {
                                    let size =
                                        *child.get(scope + 1).ok_or(Error::Truncated)? as usize;
                                    if size < 8 || scope + size > offset + length {
                                        return Err(Error::Invalid);
                                    }
                                    if child[scope] == 1 && size == 8 {
                                        if result.iommu_requester_count
                                            == result.iommu_requesters.len()
                                        {
                                            return Err(Error::Capacity);
                                        }
                                        let rid = (u16::from(child[scope + 5]) << 8)
                                            | (u16::from(child[scope + 6]) << 3)
                                            | u16::from(child[scope + 7]);
                                        result.iommu_requesters[result.iommu_requester_count] = rid;
                                        result.iommu_requester_count += 1;
                                    }
                                    scope += size;
                                }
                            }
                        }
                    }
                    offset += length;
                }
            }
            _ => {}
        }
    }
    if result.cpu_count == 0 || result.local_apic == 0 || result.io_apic == 0 {
        return Err(Error::Missing);
    }
    Ok(result)
}

fn parse_madt(bytes: &[u8], result: &mut AcpiInfo) -> Result<(), Error> {
    if bytes.len() < 44 {
        return Err(Error::Truncated);
    }
    result.local_apic = u32_at(bytes, 36)? as u64;
    let mut cursor = 44;
    while cursor < bytes.len() {
        let kind = bytes[cursor];
        let length = *bytes.get(cursor + 1).ok_or(Error::Truncated)? as usize;
        if length < 2 || cursor + length > bytes.len() {
            return Err(Error::Invalid);
        }
        let entry = &bytes[cursor..cursor + length];
        match kind {
            0 => {
                if length < 8 {
                    return Err(Error::Invalid);
                }
                if u32_at(entry, 4)? & 1 != 0 {
                    add_cpu(result, entry[3] as u32)?;
                }
            }
            1 => {
                if length < 12 {
                    return Err(Error::Invalid);
                }
                let gsi = u32_at(entry, 8)?;
                if result.io_apic == 0 || gsi == 0 {
                    result.io_apic = u32_at(entry, 4)? as u64;
                    result.io_apic_gsi_base = gsi;
                }
            }
            5 => {
                if length < 12 {
                    return Err(Error::Invalid);
                }
                result.local_apic = u64_at(entry, 4)?;
            }
            9 => {
                if length < 16 {
                    return Err(Error::Invalid);
                }
                if u32_at(entry, 8)? & 1 != 0 {
                    add_cpu(result, u32_at(entry, 4)?)?;
                }
            }
            _ => {}
        }
        cursor += length;
    }
    Ok(())
}

fn add_cpu(info: &mut AcpiInfo, id: u32) -> Result<(), Error> {
    if info.cpu_ids[..info.cpu_count].contains(&id) {
        return Err(Error::Invalid);
    }
    if info.cpu_count == MAX_CPUS {
        return Err(Error::Capacity);
    }
    info.cpu_ids[info.cpu_count] = id;
    info.cpu_count += 1;
    Ok(())
}

// This reads static root-bridge resource buffers. It deliberately does not
// execute AML methods or guess the result of dynamic _CRS implementations.
fn pci_root_resources(aml: &[u8]) -> Result<Option<PhysicalRange>, Error> {
    let mut selected: Option<PhysicalRange> = None;
    for start in 0..aml.len().saturating_sub(12) {
        if aml[start] != 0x10 {
            continue;
        }
        let Ok((length, prefix)) = aml_package_length(&aml[start + 1..]) else {
            continue;
        };
        let end = match (start + 1)
            .checked_add(length)
            .filter(|end| *end <= aml.len())
        {
            Some(end) => end,
            None => continue,
        };
        let name = start + 1 + prefix;
        if aml.get(name..name + 10)
            != Some(&[0x5c, 0x2e, b'_', b'S', b'B', b'_', b'P', b'C', b'I', b'0'])
        {
            continue;
        }
        let body = name + 10;
        if aml.get(body..body + 6) != Some(&[0x08, b'_', b'C', b'R', b'S', 0x11]) {
            continue;
        }
        let package = body + 6;
        let (length, prefix) = aml_package_length(&aml[package..end])?;
        let buffer_end = package
            .checked_add(length)
            .filter(|buffer_end| *buffer_end <= end)
            .ok_or(Error::Truncated)?;
        let (bytes, integer_bytes) = aml_integer(&aml[package + prefix..buffer_end])?;
        let data = package + prefix + integer_bytes;
        let data_end = data
            .checked_add(bytes as usize)
            .filter(|data_end| *data_end <= buffer_end)
            .ok_or(Error::Truncated)?;
        let resources = &aml[data..data_end];
        let mut cursor = 0;
        while cursor < resources.len() {
            let kind = resources[cursor];
            let length = if kind & 0x80 != 0 {
                3 + u16_at(resources, cursor + 1)? as usize
            } else {
                1 + (kind & 7) as usize
            };
            let end = cursor
                .checked_add(length)
                .filter(|end| *end <= resources.len())
                .ok_or(Error::Truncated)?;
            let entry = &resources[cursor..end];
            let range = if kind == 0x87
                && length >= 26
                && entry[3] == 0
                && entry[4] & 1 == 0
                && entry[5] & 1 != 0
            {
                Some((
                    u32_at(entry, 10)? as u64,
                    u32_at(entry, 14)? as u64,
                    u32_at(entry, 18)? as u64,
                    u32_at(entry, 22)? as u64,
                ))
            } else if kind == 0x8a
                && length >= 46
                && entry[3] == 0
                && entry[4] & 1 == 0
                && entry[5] & 1 != 0
            {
                Some((
                    u64_at(entry, 14)?,
                    u64_at(entry, 22)?,
                    u64_at(entry, 30)?,
                    u64_at(entry, 38)?,
                ))
            } else {
                None
            };
            if let Some((minimum, maximum, translation, bytes)) = range {
                if maximum < minimum
                    || bytes == 0
                    || bytes != maximum - minimum + 1
                    || translation != 0
                {
                    return Err(Error::Invalid);
                }
                if maximum < 0x1_0000_0000 && selected.is_none_or(|range| bytes > range.bytes) {
                    selected = Some(PhysicalRange {
                        base: minimum,
                        bytes,
                    });
                }
            }
            if kind == 0x79 {
                break;
            }
            cursor = end;
        }
    }
    Ok(selected)
}

fn aml_package_length(bytes: &[u8]) -> Result<(usize, usize), Error> {
    let first = *bytes.first().ok_or(Error::Truncated)?;
    let following = (first >> 6) as usize;
    let mut length = (first & if following == 0 { 0x3f } else { 0xf }) as usize;
    for index in 0..following {
        length |= (*bytes.get(index + 1).ok_or(Error::Truncated)? as usize) << (4 + index * 8);
    }
    if length < following + 1 {
        return Err(Error::Invalid);
    }
    Ok((length, following + 1))
}

fn aml_integer(bytes: &[u8]) -> Result<(u64, usize), Error> {
    match bytes.first().copied().ok_or(Error::Truncated)? {
        0 => Ok((0, 1)),
        1 => Ok((1, 1)),
        0x0a => Ok((*bytes.get(1).ok_or(Error::Truncated)? as u64, 2)),
        0x0b => Ok((u16_at(bytes, 1)? as u64, 3)),
        0x0c => Ok((u32_at(bytes, 1)? as u64, 5)),
        0x0e => Ok((u64_at(bytes, 1)?, 9)),
        _ => Err(Error::Invalid),
    }
}
fn table<'a>(
    read: &mut impl FnMut(u64, usize) -> Option<&'a [u8]>,
    address: u64,
) -> Result<&'a [u8], Error> {
    if address == 0 {
        return Err(Error::Invalid);
    }
    let header = read(address, 36).ok_or(Error::Truncated)?;
    let length = u32_at(header, 4)? as usize;
    if !(36..=1024 * 1024).contains(&length) {
        return Err(Error::Invalid);
    }
    let bytes = read(address, length)
        .and_then(|bytes| bytes.get(..length))
        .ok_or(Error::Truncated)?;
    if !checksum(bytes) {
        return Err(Error::Invalid);
    }
    Ok(bytes)
}
fn append<const N: usize>(
    ranges: &mut [PhysicalRange; N],
    count: &mut usize,
    range: PhysicalRange,
) -> Result<(), Error> {
    if range.bytes == 0 {
        return Ok(());
    }
    if *count == N {
        return Err(Error::Capacity);
    }
    ranges[*count] = range;
    *count += 1;
    Ok(())
}
fn checksum(bytes: &[u8]) -> bool {
    bytes.iter().fold(0u8, |sum, byte| sum.wrapping_add(*byte)) == 0
}
fn u16_at(bytes: &[u8], index: usize) -> Result<u16, Error> {
    Ok(u16::from_le_bytes(
        bytes
            .get(index..index + 2)
            .ok_or(Error::Truncated)?
            .try_into()
            .unwrap(),
    ))
}
fn u32_at(bytes: &[u8], index: usize) -> Result<u32, Error> {
    Ok(u32::from_le_bytes(
        bytes
            .get(index..index + 4)
            .ok_or(Error::Truncated)?
            .try_into()
            .unwrap(),
    ))
}
fn u64_at(bytes: &[u8], index: usize) -> Result<u64, Error> {
    Ok(u64::from_le_bytes(
        bytes
            .get(index..index + 8)
            .ok_or(Error::Truncated)?
            .try_into()
            .unwrap(),
    ))
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use std::{vec, vec::Vec};

    fn sdt(signature: &[u8; 4], payload: &[u8]) -> Vec<u8> {
        let mut bytes = vec![0; 36];
        bytes[..4].copy_from_slice(signature);
        bytes.extend_from_slice(payload);
        let length = bytes.len() as u32;
        bytes[4..8].copy_from_slice(&length.to_le_bytes());
        bytes[9] = 0u8.wrapping_sub(bytes.iter().fold(0u8, |sum, byte| sum.wrapping_add(*byte)));
        bytes
    }
    fn rsdp(root: u64) -> Vec<u8> {
        let mut bytes = vec![0; 36];
        bytes[..8].copy_from_slice(b"RSD PTR ");
        bytes[15] = 2;
        bytes[20..24].copy_from_slice(&36u32.to_le_bytes());
        bytes[24..32].copy_from_slice(&root.to_le_bytes());
        bytes[8] = 0u8.wrapping_sub(
            bytes[..20]
                .iter()
                .fold(0u8, |sum, byte| sum.wrapping_add(*byte)),
        );
        bytes[32] = 0u8.wrapping_sub(bytes.iter().fold(0u8, |sum, byte| sum.wrapping_add(*byte)));
        bytes
    }
    fn fixture() -> Vec<(u64, Vec<u8>)> {
        let mut madt = Vec::from(0xfee0_0000u32.to_le_bytes());
        madt.extend_from_slice(&1u32.to_le_bytes());
        for id in [0, 7] {
            madt.extend_from_slice(&[0, 8, id, id, 1, 0, 0, 0]);
        }
        madt.extend_from_slice(&[1, 12, 0, 0]);
        madt.extend_from_slice(&0xfec0_0000u32.to_le_bytes());
        madt.extend_from_slice(&0u32.to_le_bytes());
        let mut mcfg = vec![0; 8];
        mcfg.extend_from_slice(&0xb000_0000u64.to_le_bytes());
        mcfg.extend_from_slice(&[0, 0, 0, 255, 0, 0, 0, 0]);
        let mut dmar = vec![0; 12];
        dmar.extend_from_slice(&[0, 0, 16, 0, 1, 0, 0, 0]);
        dmar.extend_from_slice(&0xfed9_0000u64.to_le_bytes());
        let mut pointers = Vec::new();
        for address in [0x2000u64, 0x3000, 0x4000] {
            pointers.extend_from_slice(&address.to_le_bytes());
        }
        vec![
            (0x1000, sdt(b"XSDT", &pointers)),
            (0x2000, sdt(b"APIC", &madt)),
            (0x3000, sdt(b"MCFG", &mcfg)),
            (0x4000, sdt(b"DMAR", &dmar)),
        ]
    }
    #[test]
    fn topology_and_resources_follow_firmware_instead_of_assumed_cpu_ids() {
        let tables = fixture();
        let info = acpi(&rsdp(0x1000), |address, bytes| {
            tables
                .iter()
                .find(|(base, _)| *base == address)
                .and_then(|(_, data)| data.get(..bytes))
        })
        .unwrap();
        assert_eq!(&info.cpu_ids[..info.cpu_count], &[0, 7]);
        assert_eq!(
            (info.local_apic, info.io_apic, info.ecam, info.iommu),
            (0xfee0_0000, 0xfec0_0000, 0xb000_0000, 0xfed9_0000)
        );
    }
    #[test]
    fn corrupted_or_truncated_acpi_cannot_publish_resources() {
        let mut tables = fixture();
        tables[1].1[36] ^= 1;
        assert!(matches!(
            acpi(&rsdp(0x1000), |address, bytes| tables
                .iter()
                .find(|(base, _)| *base == address)
                .and_then(|(_, data)| data.get(..bytes))),
            Err(Error::Invalid)
        ));
        assert!(matches!(
            acpi(&rsdp(0x1000)[..20], |_, _| None),
            Err(Error::Truncated)
        ));
    }
    #[test]
    fn static_root_bridge_crs_uses_declared_window_and_rejects_bad_lengths() {
        let mut descriptor = vec![0x87, 23, 0, 0, 0x0c, 1];
        for word in [0u32, 0xc000_0000, 0xfebf_ffff, 0, 0x3ec0_0000] {
            descriptor.extend_from_slice(&word.to_le_bytes());
        }
        descriptor.extend_from_slice(&[0x79, 0]);
        let mut aml = vec![
            0x10, 48, 0x5c, 0x2e, b'_', b'S', b'B', b'_', b'P', b'C', b'I', b'0', 0x08, b'_', b'C',
            b'R', b'S', 0x11, 31, 0x0a, 28,
        ];
        aml.extend_from_slice(&descriptor);
        assert_eq!(
            pci_root_resources(&aml).unwrap(),
            Some(PhysicalRange {
                base: 0xc000_0000,
                bytes: 0x3ec0_0000
            })
        );
        aml[20] = 29;
        assert_eq!(pci_root_resources(&aml), Err(Error::Truncated));
    }

    #[test]
    fn scoped_dmar_preserves_requester_ids_instead_of_treating_unit_as_missing() {
        let mut tables = fixture();
        let mut payload = vec![0; 12];
        payload.extend_from_slice(&[0, 0, 24, 0, 0, 0, 0, 0]);
        payload.extend_from_slice(&0xfed9_0000u64.to_le_bytes());
        payload.extend_from_slice(&[1, 8, 0, 0, 0, 0, 2, 0]);
        tables[3].1 = sdt(b"DMAR", &payload);
        let info = acpi(&rsdp(0x1000), |address, bytes| {
            tables
                .iter()
                .find(|(base, _)| *base == address)
                .and_then(|(_, data)| data.get(..bytes))
        })
        .unwrap();
        assert_eq!(info.iommu, 0xfed9_0000);
        assert!(!info.iommu_include_all);
        assert_eq!(
            &info.iommu_requesters[..info.iommu_requester_count],
            &[0x10]
        );
    }
    #[test]
    fn multiboot_preserves_memory_holes_and_module_reservations() {
        let mut bytes = vec![0; 8];
        let mut map = vec![0; 16];
        map[..4].copy_from_slice(&6u32.to_le_bytes());
        map[4..8].copy_from_slice(&88u32.to_le_bytes());
        map[8..12].copy_from_slice(&24u32.to_le_bytes());
        for (base, length, kind) in [
            (0u64, 0x9f000u64, 1u32),
            (0x9f000, 0x61000, 2),
            (0x100000, 0x17f00000, 1),
        ] {
            map.extend_from_slice(&base.to_le_bytes());
            map.extend_from_slice(&length.to_le_bytes());
            map.extend_from_slice(&kind.to_le_bytes());
            map.extend_from_slice(&0u32.to_le_bytes());
        }
        bytes.extend_from_slice(&map);
        bytes.extend_from_slice(&[3, 0, 0, 0, 16, 0, 0, 0]);
        bytes.extend_from_slice(&0x2000000u32.to_le_bytes());
        bytes.extend_from_slice(&0x2100000u32.to_le_bytes());
        bytes.extend_from_slice(&[0, 0, 0, 0, 8, 0, 0, 0]);
        let length = bytes.len() as u32;
        bytes[..4].copy_from_slice(&length.to_le_bytes());
        let parsed = multiboot(&bytes).unwrap();
        assert_eq!(parsed.memory_count, 2);
        assert_eq!(
            parsed.memory[1],
            PhysicalRange {
                base: 0x100000,
                bytes: 0x17f00000
            }
        );
        assert_eq!(parsed.reserved_count, 2);
        assert_eq!(
            parsed.reserved[1],
            PhysicalRange {
                base: 0x2000000,
                bytes: 0x100000
            }
        );
        bytes[16..20].copy_from_slice(&0u32.to_le_bytes());
        assert!(matches!(multiboot(&bytes), Err(Error::Invalid)));
    }
}
