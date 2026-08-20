const FDT_MAGIC: u32 = 0xd00d_feed;

#[derive(Clone, Copy)]
pub struct PlatformInfo {
    pub valid: bool,
    pub bytes: usize,
    pub cpus: usize,
    pub ram_base: usize,
    pub ram_bytes: usize,
    pub uart_base: usize,
    pub rtc_base: usize,
    pub clint_base: usize,
    pub plic_base: usize,
    pub plic_bytes: usize,
    pub gicd_base: usize,
    pub gicr_base: usize,
    pub smmu_base: usize,
    pub riscv_iommu_base: usize,
    pub riscv_iommu_bytes: usize,
    pub pcie_base: usize,
    pub pcie_mmio_base: usize,
    pub pcie_mmio_bytes: usize,
    pub iommu_rid_base: u32,
    pub iommu_sid_base: u32,
    pub iommu_map_length: u32,
    pub iommu_map_mask: u32,
    pcie_intx: [u32; 16],
    pub has_pcie: bool,
}

pub fn discover(physical: usize) -> PlatformInfo {
    let pointer = crate::arch::phys_to_virt(physical as u64) as *const u8;
    let header = unsafe { core::slice::from_raw_parts(pointer, 40) };
    if be_u32(header, 0) != FDT_MAGIC {
        return invalid(0);
    }
    let bytes = be_u32(header, 4) as usize;
    let blob = unsafe { core::slice::from_raw_parts(pointer, bytes) };
    let Ok(tree) = fdt::Fdt::new(blob) else {
        return invalid(bytes);
    };
    let memory = tree.memory().regions().next();
    let uart_base = first_region(&tree, &["arm,pl011", "ns16550a"]);
    let rtc_base = first_region(&tree, &["arm,pl031", "google,goldfish-rtc"]);
    let clint_base = first_region(
        &tree,
        &["riscv,clint0", "riscv,aclint-mtimer", "sifive,clint0"],
    );
    let plic_node = tree.find_compatible(&["riscv,plic0", "sifive,plic-1.0.0"]);
    let plic_base = plic_node
        .as_ref()
        .and_then(|node| node.reg())
        .and_then(|mut regions| regions.next())
        .map(|region| region.starting_address as usize)
        .unwrap_or(0);
    let plic_bytes = plic_node
        .as_ref()
        .and_then(|node| node.reg())
        .and_then(|mut regions| regions.next())
        .and_then(|region| region.size)
        .unwrap_or(0);
    let (gicd_base, gicr_base) = first_two_regions(&tree, &["arm,gic-v3"]);
    let smmu_base = first_region(&tree, &["arm,smmu-v3"]);
    let riscv_iommu = tree.find_compatible(&["riscv,pci-iommu", "riscv,iommu"]);
    let riscv_iommu_base = riscv_iommu
        .as_ref()
        .and_then(|node| node.reg())
        .and_then(|mut regions| regions.next())
        .map(|region| region.starting_address as usize)
        .unwrap_or(0);
    let riscv_iommu_bytes = riscv_iommu
        .as_ref()
        .and_then(|node| node.reg())
        .and_then(|mut regions| regions.next())
        .and_then(|region| region.size)
        .unwrap_or(0);
    let pcie_base = first_region(&tree, &["pci-host-ecam-generic"]);
    let (pcie_mmio_base, pcie_mmio_bytes) = pcie_mmio_region(&tree);
    let (iommu_rid_base, iommu_sid_base, iommu_map_length, iommu_map_mask) = pcie_iommu_map(&tree);
    let pcie_intx = pcie_interrupt_map(&tree);
    PlatformInfo {
        valid: true,
        bytes,
        cpus: tree.cpus().count(),
        ram_base: memory
            .map(|region| region.starting_address as usize)
            .unwrap_or(0),
        ram_bytes: memory.and_then(|region| region.size).unwrap_or(0),
        uart_base,
        rtc_base,
        clint_base,
        plic_base,
        plic_bytes,
        gicd_base,
        gicr_base,
        smmu_base,
        riscv_iommu_base,
        riscv_iommu_bytes,
        pcie_base,
        pcie_mmio_base,
        pcie_mmio_bytes,
        iommu_rid_base,
        iommu_sid_base,
        iommu_map_length,
        iommu_map_mask,
        pcie_intx,
        has_pcie: pcie_base != 0,
    }
}

fn invalid(bytes: usize) -> PlatformInfo {
    PlatformInfo {
        valid: false,
        bytes,
        cpus: 0,
        ram_base: 0,
        ram_bytes: 0,
        uart_base: 0,
        rtc_base: 0,
        clint_base: 0,
        plic_base: 0,
        plic_bytes: 0,
        gicd_base: 0,
        gicr_base: 0,
        smmu_base: 0,
        riscv_iommu_base: 0,
        riscv_iommu_bytes: 0,
        pcie_base: 0,
        pcie_mmio_base: 0,
        pcie_mmio_bytes: 0,
        iommu_rid_base: 0,
        iommu_sid_base: 0,
        iommu_map_length: 0,
        iommu_map_mask: 0,
        pcie_intx: [0; 16],
        has_pcie: false,
    }
}

impl PlatformInfo {
    pub fn stream_id(&self, requester_id: u32) -> Option<u32> {
        let requester_id = requester_id & self.iommu_map_mask;
        let offset = requester_id.checked_sub(self.iommu_rid_base)?;
        if offset >= self.iommu_map_length {
            return None;
        }
        self.iommu_sid_base.checked_add(offset)
    }

    pub fn intx_irq(&self, slot: u8, pin: u8) -> Option<u32> {
        if slot >= 4 || !(1..=4).contains(&pin) {
            return None;
        }
        let irq = self.pcie_intx[slot as usize * 4 + pin as usize - 1];
        (irq != 0).then_some(irq)
    }
}

fn pcie_mmio_region(tree: &fdt::Fdt<'_>) -> (usize, usize) {
    let Some(node) = tree.find_compatible(&["pci-host-ecam-generic"]) else {
        return (0, 0);
    };
    let Some(ranges) = node.property("ranges") else {
        return (0, 0);
    };
    for entry in ranges.value.chunks_exact(28) {
        let space = be_u32(entry, 0) & 0x0300_0000;
        if space != 0x0200_0000 {
            continue;
        }
        let parent = ((be_u32(entry, 12) as u64) << 32) | be_u32(entry, 16) as u64;
        let bytes = ((be_u32(entry, 20) as u64) << 32) | be_u32(entry, 24) as u64;
        return (parent as usize, bytes as usize);
    }
    (0, 0)
}

fn pcie_iommu_map(tree: &fdt::Fdt<'_>) -> (u32, u32, u32, u32) {
    let Some(node) = tree.find_compatible(&["pci-host-ecam-generic"]) else {
        return (0, 0, 0, 0);
    };
    let Some(mapping) = node.property("iommu-map") else {
        return (0, 0, 0, 0);
    };
    if mapping.value.len() < 16 {
        return (0, 0, 0, 0);
    }
    let mask = node
        .property("iommu-map-mask")
        .filter(|property| property.value.len() >= 4)
        .map(|property| be_u32(property.value, 0))
        .unwrap_or(u32::MAX);
    for entry in mapping.value.chunks_exact(16) {
        let length = be_u32(entry, 12);
        if length != 0 {
            return (be_u32(entry, 0), be_u32(entry, 8), length, mask);
        }
    }
    (0, 0, 0, mask)
}

fn pcie_interrupt_map(tree: &fdt::Fdt<'_>) -> [u32; 16] {
    let mut result = [0u32; 16];
    let Some(node) = tree.find_compatible(&["pci-host-ecam-generic"]) else {
        return result;
    };
    let Some(mapping) = node.property("interrupt-map") else {
        return result;
    };
    let mask = node
        .property("interrupt-map-mask")
        .filter(|property| property.value.len() >= 16)
        .map(|property| {
            [
                be_u32(property.value, 0),
                be_u32(property.value, 4),
                be_u32(property.value, 8),
                be_u32(property.value, 12),
            ]
        })
        .unwrap_or([u32::MAX; 4]);
    let (stride, interrupt_type_offset, interrupt_offset, interrupt_base) =
        if cfg!(target_arch = "riscv64") {
            // QEMU virt PLIC entries contain PCI address (3 cells), pin,
            // parent phandle and one PLIC interrupt cell.
            (24, None, 20, 0)
        } else {
            // QEMU Arm virt GIC entries additionally contain the parent
            // address and the three-cell GIC interrupt specifier.
            (40, Some(28), 32, 32)
        };
    for entry in mapping.value.chunks_exact(stride) {
        let address = be_u32(entry, 0) & mask[0];
        let slot = ((address >> 11) & 0x1f) as usize;
        let pin = be_u32(entry, 12) & mask[3];
        if slot >= 4
            || !(1..=4).contains(&pin)
            || interrupt_type_offset.is_some_and(|offset| be_u32(entry, offset) != 0)
        {
            continue;
        }
        result[slot * 4 + pin as usize - 1] = interrupt_base + be_u32(entry, interrupt_offset);
    }
    result
}

fn first_region(tree: &fdt::Fdt<'_>, compatible: &[&str]) -> usize {
    tree.find_compatible(compatible)
        .and_then(|node| node.reg())
        .and_then(|mut regions| regions.next())
        .map(|region| region.starting_address as usize)
        .unwrap_or(0)
}

fn first_two_regions(tree: &fdt::Fdt<'_>, compatible: &[&str]) -> (usize, usize) {
    let Some(mut regions) = tree.find_compatible(compatible).and_then(|node| node.reg()) else {
        return (0, 0);
    };
    let first = regions
        .next()
        .map(|region| region.starting_address as usize)
        .unwrap_or(0);
    let second = regions
        .next()
        .map(|region| region.starting_address as usize)
        .unwrap_or(0);
    (first, second)
}

fn be_u32(input: &[u8], offset: usize) -> u32 {
    u32::from_be_bytes(input[offset..offset + 4].try_into().unwrap())
}
