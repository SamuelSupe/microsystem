use crate::dtb::PlatformInfo;
use microsystem_kernel::firmware;
use microsystem_kernel::memory::PhysicalRange;

pub fn discover(physical: usize) -> PlatformInfo {
    let Some(header) = read(physical as u64, 8) else {
        return crate::dtb::invalid(0);
    };
    let bytes = u32::from_le_bytes(header[..4].try_into().unwrap()) as usize;
    if !(16..=1024 * 1024).contains(&bytes) {
        return crate::dtb::invalid(bytes);
    }
    let Some(blob) = read(physical as u64, bytes) else {
        return crate::dtb::invalid(bytes);
    };
    let Ok(boot) = firmware::multiboot(blob) else {
        return crate::dtb::invalid(bytes);
    };
    let mut platform = crate::dtb::invalid(bytes);
    platform.valid = true;
    platform.memory_map_overflow = false;
    platform.ram_regions = boot.memory;
    platform.ram_region_count = boot.memory_count;
    platform.reserved_regions = boot.reserved;
    platform.reserved_region_count = boot.reserved_count;
    if platform.reserved_region_count == platform.reserved_regions.len() {
        platform.memory_map_overflow = true;
    } else {
        platform.reserved_regions[platform.reserved_region_count] = PhysicalRange {
            base: physical as u64,
            bytes: bytes as u64,
        };
        platform.reserved_region_count += 1;
    }
    platform.ram_base = boot.memory[0].base as usize;
    platform.ram_bytes = boot.memory[..boot.memory_count]
        .iter()
        .map(|range| range.bytes as usize)
        .sum();
    // Early boot maps the first 4 GiB. Higher RAM cannot enter the allocator
    // until a wider direct map is installed.
    let mut count = 0;
    for index in 0..platform.ram_region_count {
        let mut region = platform.ram_regions[index];
        let start = region.base.saturating_add(4095) & !4095;
        let end = region.base.saturating_add(region.bytes).min(0x1_0000_0000) & !4095;
        if start >= end {
            continue;
        }
        region.base = start;
        region.bytes = end - start;
        platform.ram_regions[count] = region;
        count += 1;
    }
    platform.ram_region_count = count;
    for range in &platform.ram_regions[..count] {
        crate::kprintln!("[firmware] usable-ram={:#x}+{:#x}", range.base, range.bytes);
    }
    platform.uart_base = 0x3f8;
    let Some(rsdp) = boot.rsdp else {
        crate::kprintln!("[firmware] ACPI RSDP missing");
        return platform;
    };
    let acpi = match firmware::acpi(rsdp, read) {
        Ok(acpi) => acpi,
        Err(error) => {
            crate::kprintln!("[firmware] ACPI rejected {:?}", error);
            return platform;
        }
    };
    platform.cpus = acpi.cpu_count;
    platform.gicd_base = acpi.local_apic as usize;
    platform.gicr_base = acpi.io_apic as usize;
    platform.smmu_base = acpi.iommu as usize;
    crate::kprintln!(
        "[firmware] DMAR units={} selected={:#x}",
        acpi.dmar_units,
        acpi.iommu
    );
    platform.pcie_base = acpi.ecam as usize;
    platform.has_pcie = acpi.ecam != 0;
    platform.iommu_map_length = 0x10000;
    platform.iommu_map_mask = 0xffff;
    let bsp = crate::arch::selected::hardware_cpu_id(acpi.local_apic);
    let second = acpi.cpu_ids[..acpi.cpu_count]
        .iter()
        .copied()
        .find(|id| *id != bsp);
    crate::arch::selected::configure_topology(acpi.local_apic, bsp, second);
    platform.pcie_mmio_base = acpi.pci_mmio.base as usize;
    platform.pcie_mmio_bytes = acpi.pci_mmio.bytes as usize;
    // PCI INTx swizzling follows this Q35 profile; resource windows come from
    // the root bridge's firmware _CRS buffer, not a chipset register guess.
    let bridge = host_config(0);
    crate::kprintln!(
        "[firmware] PCI host id={:#x} resource-window={:#x}+{:#x}",
        bridge,
        acpi.pci_mmio.base,
        acpi.pci_mmio.bytes
    );
    if bridge == 0x29c0_8086 {
        for slot in 0..4 {
            for pin in 0..4 {
                platform.pcie_intx[slot * 4 + pin] = 16 + ((slot + pin) & 3) as u32;
            }
        }
    }
    if acpi.io_apic_gsi_base != 0 {
        platform.gicr_base = 0;
    }
    crate::kprintln!(
        "[firmware] Multiboot2 memory-regions={} reserved={} ACPI cpus={} apic={:#x} ioapic={:#x} MCFG={:#x} DMAR={:#x}",
        platform.ram_region_count,
        platform.reserved_region_count,
        platform.cpus,
        acpi.local_apic,
        acpi.io_apic,
        acpi.ecam,
        acpi.iommu
    );
    platform
}

fn read(address: u64, bytes: usize) -> Option<&'static [u8]> {
    if address == 0 || address.checked_add(bytes as u64)? > 0x1_0000_0000 {
        return None;
    }
    Some(unsafe {
        core::slice::from_raw_parts(crate::arch::phys_to_virt(address) as *const u8, bytes)
    })
}

fn host_config(register: u32) -> u32 {
    let value: u32;
    unsafe {
        core::arch::asm!("out dx, eax", in("dx") 0xcf8u16, in("eax") 0x8000_0000u32 | register, options(nomem, nostack, preserves_flags));
        core::arch::asm!("in eax, dx", in("dx") 0xcfcu16, out("eax") value, options(nomem, nostack, preserves_flags));
    }
    value
}
