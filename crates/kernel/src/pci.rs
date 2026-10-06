use core::cell::UnsafeCell;

const VIRTIO_VENDOR: u16 = 0x1af4;
const VIRTIO_BLOCK_MODERN: u16 = 0x1042;
pub const VIRTIO_GPU_MODERN: u16 = 0x1050;
pub const VIRTIO_INPUT_MODERN: u16 = 0x1052;
pub const VIRTIO_NET_MODERN: u16 = 0x1041;
pub const VIRTIO_RNG_MODERN: u16 = 0x1044;
const QUEUE_SIZE: u16 = 16;
const QUEUE_PAGE_BYTES: usize = 4096;
const DESC_OFFSET: usize = 0;
const AVAIL_OFFSET: usize = 256;
const USED_OFFSET: usize = 320;
const HEADER_OFFSET: usize = 512;
const DATA_OFFSET: usize = 1024;
const STATUS_OFFSET: usize = DATA_OFFSET + 512;
const VIRTIO_BLK_T_IN: u32 = 0;
const VRING_DESC_F_NEXT: u16 = 1;
const VRING_DESC_F_WRITE: u16 = 2;
const SENTINEL_VALUE: u64 = 0x51a7_1e1d_d15c_a11e;

#[derive(Clone, Copy)]
pub struct Device {
    pub bus: u8,
    pub slot: u8,
    pub function: u8,
}

impl Device {
    pub const fn requester_id(self) -> u32 {
        ((self.bus as u32) << 8) | ((self.slot as u32) << 3) | self.function as u32
    }

    pub const fn config_physical(self, ecam_physical: usize) -> u64 {
        (ecam_physical
            + ((self.bus as usize) << 20)
            + ((self.slot as usize) << 15)
            + ((self.function as usize) << 12)) as u64
    }
}

#[derive(Clone, Copy)]
pub struct VirtioTransport {
    pub common: usize,
    pub notify: usize,
    pub notify_bytes: u32,
    pub isr: usize,
    pub device_config: usize,
    pub device_config_bytes: u32,
    pub notify_multiplier: u32,
    pub queues: u16,
    pub capacity_sectors: u64,
}

#[derive(Clone, Copy)]
struct BarRange {
    base: u64,
    bytes: u64,
}

#[derive(Clone, Copy)]
struct BarCache {
    ecam: usize,
    requester: u32,
    id: u32,
    raw: [u32; 6],
    bars: [Option<BarRange>; 6],
}
static mut BAR_CACHE: [Option<BarCache>; 64] = [None; 64];

pub fn forget_device(ecam: usize, device: Device) {
    for index in 0..64 {
        unsafe {
            if BAR_CACHE[index].is_some_and(|cached| {
                cached.ecam == ecam && cached.requester == device.requester_id()
            }) {
                BAR_CACHE[index] = None;
            }
        }
    }
}

#[derive(Clone, Copy)]
pub struct UserTransportGrant {
    pub pci_function_physical: u64,
    pub common_physical: u64,
    pub notify_physical: u64,
    pub isr_physical: u64,
    pub device_physical: u64,
    pub queue_physical: u64,
    pub data_physical: u64,
    pub dma_iova: u64,
}

#[allow(dead_code)]
#[derive(Clone, Copy, Debug)]
pub enum VirtioError {
    MissingWindow,
    BarSize {
        index: u8,
        original: u32,
        mask: u64,
        size: u64,
    },
    BarWindow {
        index: u8,
        address: u64,
        size: u64,
        end: u64,
    },
    CapabilityList,
    CapabilityBar,
    CapabilityLength,
    CommonConfig,
    NotifyConfig,
    IsrConfig,
    DeviceConfig,
    RequiredFeatures,
    FeaturesRejected,
}

#[allow(dead_code)]
#[derive(Clone, Copy, Debug)]
pub enum BlockIoError {
    QueueUnavailable,
    QueueTooSmall,
    QueueRejected,
    TimedOut,
    Device(u8),
}

pub struct DmaFaultProbe {
    pub completion_error: bool,
    pub sentinel_intact: bool,
    pub attempted_iova: u64,
}

#[repr(C, align(4096))]
struct QueuePage([u8; QUEUE_PAGE_BYTES]);

struct QueueStorage(UnsafeCell<QueuePage>);

unsafe impl Sync for QueueStorage {}

static BLOCK_QUEUE: QueueStorage = QueueStorage(UnsafeCell::new(QueuePage([0; QUEUE_PAGE_BYTES])));
static BLOCK_DATA: QueueStorage = QueueStorage(UnsafeCell::new(QueuePage([0; QUEUE_PAGE_BYTES])));
static DMA_SENTINEL: QueueStorage = QueueStorage(UnsafeCell::new(QueuePage([0; QUEUE_PAGE_BYTES])));

pub fn virtio_block_at(ecam_physical: usize, slot: u8, function: u8) -> Option<Device> {
    virtio_device_at(ecam_physical, slot, function, VIRTIO_BLOCK_MODERN)
}

pub fn virtio_device_at(
    ecam_physical: usize,
    slot: u8,
    function: u8,
    expected_device: u16,
) -> Option<Device> {
    virtio_device_on_bus(ecam_physical, 0, slot, function, expected_device)
}

pub fn virtio_device_on_bus(
    ecam_physical: usize,
    bus: u8,
    slot: u8,
    function: u8,
    expected_device: u16,
) -> Option<Device> {
    if ecam_physical == 0 || slot >= 32 || function >= 8 {
        return None;
    }
    let id = read32(ecam_physical + ((bus as usize) << 20), slot, function, 0);
    let vendor = id as u16;
    let device = (id >> 16) as u16;
    (vendor == VIRTIO_VENDOR && device == expected_device).then_some(Device {
        bus,
        slot,
        function,
    })
}

pub fn virtio_devices(ecam: usize, expected: u16) -> impl Iterator<Item = Device> {
    (0..crate::pci_bridges::MAX_BUSES as u8)
        .filter(|bus| crate::pci_bridges::active(*bus))
        .flat_map(move |bus| {
            (0..32).filter_map(move |slot| virtio_device_on_bus(ecam, bus, slot, 0, expected))
        })
}

pub fn inspect_configured_virtio(
    ecam_physical: usize,
    device: Device,
    mmio_base: usize,
    mmio_bytes: usize,
) -> Result<VirtioTransport, VirtioError> {
    if mmio_base == 0 || mmio_bytes == 0 {
        return Err(VirtioError::MissingWindow);
    }
    transport_from_bars(
        ecam_physical + ((device.bus as usize) << 20),
        device,
        configured_bars(ecam_physical, device)?,
        mmio_base,
        mmio_bytes,
    )
}

pub fn reserve_hotplug_window(ecam: usize, base: usize, bytes: usize) -> Result<(), VirtioError> {
    let end = (base as u64)
        .checked_add(bytes as u64)
        .ok_or(VirtioError::MissingWindow)?;
    let mut next = base as u64;
    for slot in 0..32 {
        let id = read32(ecam, slot, 0, 0);
        if id as u16 == 0xffff {
            continue;
        }
        let device = Device {
            bus: 0,
            slot,
            function: 0,
        };
        for bar in configured_bars(ecam, device)?.iter().flatten() {
            let limit = bar
                .base
                .checked_add(bar.bytes)
                .ok_or(VirtioError::MissingWindow)?;
            if bar.base >= base as u64 && limit <= end {
                next = next.max(limit);
            }
        }
    }
    crate::pci_bridges::reset(base as u64, end, next);
    crate::pci_bridges::prepare(ecam)
}

pub(crate) fn bar_ranges(
    ecam: usize,
    device: Device,
) -> Result<impl Iterator<Item = (u64, u64)>, VirtioError> {
    Ok(configured_bars(ecam, device)?
        .into_iter()
        .flatten()
        .map(|bar| (bar.base, bar.bytes)))
}

/// Assigns BARs only for a newly discovered function. Existing live functions
/// retain their addresses; the boot-discovered high-water mark reserves them.
pub fn configure_hotplug_virtio(
    ecam: usize,
    device: Device,
    base: usize,
    bytes: usize,
) -> Result<(), VirtioError> {
    let _ = (base, bytes);
    let (mut next, end) = crate::pci_bridges::allocation(device.bus)?;
    let ecam = ecam + ((device.bus as usize) << 20);
    let command = read16(ecam, device.slot, device.function, 4);
    write16(ecam, device.slot, device.function, 4, command & !7);
    let mut index = 0;
    while index < 6 {
        let offset = 0x10 + index * 4;
        let original = read32(ecam, device.slot, device.function, offset);
        if original & 1 != 0 {
            index += 1;
            continue;
        }
        let is_64 = original & 6 == 4 && index < 5;
        let upper = if is_64 {
            read32(ecam, device.slot, device.function, offset + 4)
        } else {
            0
        };
        write32(ecam, device.slot, device.function, offset, u32::MAX);
        if is_64 {
            write32(ecam, device.slot, device.function, offset + 4, u32::MAX);
        }
        let low_mask = read32(ecam, device.slot, device.function, offset);
        let high_mask = if is_64 {
            read32(ecam, device.slot, device.function, offset + 4)
        } else {
            0
        };
        write32(ecam, device.slot, device.function, offset, original);
        if is_64 {
            write32(ecam, device.slot, device.function, offset + 4, upper);
        }
        let mask = if is_64 {
            ((high_mask as u64) << 32) | (low_mask as u64 & !15)
        } else {
            low_mask as u64 & !15
        };
        let size = if is_64 {
            (!mask).wrapping_add(1)
        } else {
            (!(mask as u32)).wrapping_add(1) as u64
        };
        if size != 0 {
            if !size.is_power_of_two() {
                return Err(VirtioError::MissingWindow);
            }
            let address = next
                .checked_add(size - 1)
                .map(|value| value & !(size - 1))
                .ok_or(VirtioError::MissingWindow)?;
            let limit = address
                .checked_add(size)
                .ok_or(VirtioError::MissingWindow)?;
            if limit > end || (!is_64 && limit > 0x1_0000_0000) {
                return Err(VirtioError::MissingWindow);
            }
            write32(
                ecam,
                device.slot,
                device.function,
                offset,
                address as u32 | (original & 15),
            );
            if is_64 {
                write32(
                    ecam,
                    device.slot,
                    device.function,
                    offset + 4,
                    (address >> 32) as u32,
                );
            }
            next = limit;
            crate::pci_bridges::advance(device.bus, next);
        }
        index += if is_64 { 2 } else { 1 };
    }
    crate::pci_bridges::advance(device.bus, next);
    write16(ecam, device.slot, device.function, 4, command | 6);
    Ok(())
}

pub fn interrupt_pin(ecam_physical: usize, device: Device) -> u8 {
    read8(ecam_physical, device.slot, device.function, 0x3d)
}

#[cfg(target_arch = "x86_64")]
pub fn interrupt_line(ecam_physical: usize, device: Device) -> Option<u32> {
    let line = read8(ecam_physical, device.slot, device.function, 0x3c);
    (line < 24).then_some(u32::from(line))
}

pub fn inspect_configured_virtio_block(
    ecam_physical: usize,
    device: Device,
    mmio_base: usize,
    mmio_bytes: usize,
) -> Result<VirtioTransport, VirtioError> {
    if mmio_base == 0 || mmio_bytes == 0 {
        return Err(VirtioError::MissingWindow);
    }
    let transport = transport_from_bars(
        ecam_physical,
        device,
        configured_bars(ecam_physical, device)?,
        mmio_base,
        mmio_bytes,
    )?;
    if transport.device_config == 0 || transport.device_config_bytes < 8 {
        return Err(VirtioError::DeviceConfig);
    }
    Ok(transport)
}

fn transport_from_bars(
    ecam_physical: usize,
    device: Device,
    bars: [Option<BarRange>; 6],
    mmio_base: usize,
    mmio_bytes: usize,
) -> Result<VirtioTransport, VirtioError> {
    let window_end = (mmio_base as u64)
        .checked_add(mmio_bytes as u64)
        .ok_or(VirtioError::MissingWindow)?;
    let mut common = 0usize;
    let mut notify = 0usize;
    let mut notify_bytes = 0u32;
    let mut isr = 0usize;
    let mut device_config = 0usize;
    let mut device_config_bytes = 0u32;
    let mut notify_multiplier = 0u32;
    let status = read16(ecam_physical, device.slot, device.function, 0x06);
    if status & (1 << 4) == 0 {
        return Err(VirtioError::CapabilityList);
    }
    let mut capability = read8(ecam_physical, device.slot, device.function, 0x34) & !3;
    for _ in 0..48 {
        if capability < 0x40 {
            break;
        }
        let id = read8(
            ecam_physical,
            device.slot,
            device.function,
            capability as usize,
        );
        let next = read8(
            ecam_physical,
            device.slot,
            device.function,
            capability as usize + 1,
        ) & !3;
        if id == 0x09 {
            let kind = read8(
                ecam_physical,
                device.slot,
                device.function,
                capability as usize + 3,
            );
            if matches!(kind, 1..=4) {
                if capability as usize + 5 > 256 {
                    return Err(VirtioError::CapabilityLength);
                }
                let bar = read8(
                    ecam_physical,
                    device.slot,
                    device.function,
                    capability as usize + 4,
                ) as usize;
                if bar >= 6 {
                    return Err(VirtioError::CapabilityBar);
                }
                let Some(bar_range) = bars.get(bar).copied().flatten() else {
                    // Virtio requires drivers to ignore capabilities on reserved BARs.
                    if next == 0 || next == capability {
                        break;
                    }
                    capability = next;
                    continue;
                };
                let cap_len = read8(
                    ecam_physical,
                    device.slot,
                    device.function,
                    capability as usize + 2,
                );
                if cap_len < 16 || capability as usize + cap_len as usize > 256 {
                    return Err(VirtioError::CapabilityLength);
                }
                if (kind == 1 && common != 0)
                    || (kind == 2 && notify != 0)
                    || (kind == 3 && isr != 0)
                    || (kind == 4 && device_config != 0)
                {
                    if next == 0 || next == capability {
                        break;
                    }
                    capability = next;
                    continue;
                }
                if kind == 2 && cap_len < 20 {
                    return Err(VirtioError::CapabilityLength);
                }
                let offset = read32(
                    ecam_physical,
                    device.slot,
                    device.function,
                    capability as usize + 8,
                ) as usize;
                let region_bytes = read32(
                    ecam_physical,
                    device.slot,
                    device.function,
                    capability as usize + 12,
                );
                let minimum_region_bytes = match kind {
                    1 => 56,
                    2 => 2,
                    3 => 1,
                    4 => 0,
                    _ => unreachable!(),
                };
                if region_bytes < minimum_region_bytes {
                    return Err(VirtioError::CapabilityLength);
                }
                let end_offset = (offset as u64)
                    .checked_add(region_bytes as u64)
                    .ok_or(VirtioError::CapabilityBar)?;
                if end_offset > bar_range.bytes {
                    return Err(VirtioError::CapabilityBar);
                }
                let physical = bar_range
                    .base
                    .checked_add(offset as u64)
                    .filter(|address| *address >= mmio_base as u64)
                    .ok_or(VirtioError::CapabilityBar)?;
                let region_end = physical
                    .checked_add(region_bytes as u64)
                    .ok_or(VirtioError::CapabilityBar)?;
                if region_end > window_end {
                    return Err(VirtioError::CapabilityBar);
                }
                let address = crate::arch::phys_to_virt(physical);
                match kind {
                    1 => common = address,
                    2 => {
                        notify = address;
                        notify_bytes = region_bytes;
                        notify_multiplier = read32(
                            ecam_physical,
                            device.slot,
                            device.function,
                            capability as usize + 16,
                        );
                    }
                    3 => isr = address,
                    4 => {
                        device_config = address;
                        device_config_bytes = region_bytes;
                    }
                    _ => unreachable!(),
                }
            }
        }
        if next == 0 || next == capability {
            break;
        }
        capability = next;
    }
    if common == 0 {
        return Err(VirtioError::CommonConfig);
    }
    if notify == 0 {
        return Err(VirtioError::NotifyConfig);
    }
    if isr == 0 {
        return Err(VirtioError::IsrConfig);
    }
    Ok(VirtioTransport {
        common,
        notify,
        notify_bytes,
        isr,
        device_config,
        device_config_bytes,
        notify_multiplier,
        queues: read_mmio16(common + 18),
        capacity_sectors: if device_config_bytes >= 8 {
            read_mmio64(device_config)
        } else {
            0
        },
    })
}

impl VirtioTransport {
    pub fn queue_notify_offset(&self) -> Option<usize> {
        let offset = (read_mmio16(self.common + 30) as usize)
            .checked_mul(self.notify_multiplier as usize)?;
        let end = offset.checked_add(2)?;
        (end <= self.notify_bytes as usize && self.notify.checked_add(end).is_some())
            .then_some(offset)
    }

    pub fn negotiate_features(&self) -> Result<(), VirtioError> {
        write_mmio8(self.common + 20, 0);
        write_mmio8(self.common + 20, 1);
        write_mmio8(self.common + 20, 1 | 2);
        write_mmio32(self.common, 0);
        let low = read_mmio32(self.common + 4);
        write_mmio32(self.common, 1);
        let high = read_mmio32(self.common + 4);
        if low & (1 << 9) == 0 || high & 0b11 != 0b11 {
            write_mmio8(self.common + 20, read_mmio8(self.common + 20) | 0x80);
            return Err(VirtioError::RequiredFeatures);
        }
        write_mmio32(self.common + 8, 0);
        write_mmio32(self.common + 12, 1 << 9);
        write_mmio32(self.common + 8, 1);
        write_mmio32(self.common + 12, 0b11);
        let status = read_mmio8(self.common + 20) | 8;
        write_mmio8(self.common + 20, status);
        if read_mmio8(self.common + 20) & 8 == 0 {
            return Err(VirtioError::FeaturesRejected);
        }
        Ok(())
    }

    pub fn queue_physical(&self) -> u64 {
        crate::arch::virt_to_phys(BLOCK_QUEUE.0.get() as usize as u64).unwrap_or(0)
    }

    pub fn user_grant(&self, dma_iova: u64, pci_function_physical: u64) -> UserTransportGrant {
        UserTransportGrant {
            pci_function_physical,
            common_physical: physical_page(self.common),
            notify_physical: physical_page(self.notify),
            isr_physical: physical_page(self.isr),
            device_physical: physical_page(self.device_config),
            queue_physical: self.queue_physical(),
            data_physical: self.data_physical(),
            dma_iova,
        }
    }

    pub fn data_physical(&self) -> u64 {
        crate::arch::virt_to_phys(BLOCK_DATA.0.get() as usize as u64).unwrap_or(0)
    }

    pub fn reset_for_user(&self, ecam_physical: usize, device: Device) -> Result<(), BlockIoError> {
        write_mmio8(self.common + 20, 0);
        let mut acknowledged = false;
        for _ in 0..100_000 {
            if read_mmio8(self.common + 20) == 0 {
                acknowledged = true;
                break;
            }
            core::hint::spin_loop();
        }
        if !acknowledged {
            return Err(BlockIoError::TimedOut);
        }
        let command = read16(ecam_physical, device.slot, device.function, 0x04);
        write16(
            ecam_physical,
            device.slot,
            device.function,
            0x04,
            command & !((1 << 1) | (1 << 2)),
        );
        crate::arch::dma_barrier();
        Ok(())
    }

    pub fn prepare_fault_probe(&self, dma_base: u64) -> Result<(), BlockIoError> {
        write_mmio16(self.common + 22, 0);
        let maximum = read_mmio16(self.common + 24);
        if maximum == 0 {
            return Err(BlockIoError::QueueUnavailable);
        }
        if maximum < 3 {
            return Err(BlockIoError::QueueTooSmall);
        }
        let size = maximum.min(QUEUE_SIZE);
        write_mmio16(self.common + 24, size);

        let queue = BLOCK_QUEUE.0.get().cast::<u8>();
        write_mmio64_split(self.common + 32, dma_base + DESC_OFFSET as u64);
        write_mmio64_split(self.common + 40, dma_base + AVAIL_OFFSET as u64);
        write_mmio64_split(self.common + 48, dma_base + USED_OFFSET as u64);
        write_mmio16(self.common + 28, 1);
        if read_mmio16(self.common + 28) != 1 {
            return Err(BlockIoError::QueueRejected);
        }

        let status = read_mmio8(self.common + 20) | 4;
        write_mmio8(self.common + 20, status);
        if read_mmio8(self.common + 20) & 4 == 0 {
            return Err(BlockIoError::QueueRejected);
        }

        unsafe {
            core::ptr::write_volatile(queue.add(AVAIL_OFFSET) as *mut u16, 0);
            core::ptr::write_volatile(queue.add(AVAIL_OFFSET + 2) as *mut u16, 0);
            core::ptr::write_volatile(queue.add(USED_OFFSET) as *mut u16, 0);
            core::ptr::write_volatile(queue.add(USED_OFFSET + 2) as *mut u16, 0);
        }

        Ok(())
    }

    pub fn probe_dma_fault(&self, dma_base: u64) -> DmaFaultProbe {
        let sentinel = DMA_SENTINEL.0.get().cast::<u8>();
        let attempted_iova = crate::arch::virt_to_phys(sentinel as usize as u64).unwrap_or(0);
        unsafe {
            core::ptr::write_volatile(sentinel as *mut u64, SENTINEL_VALUE);
            dma_write_barrier();
        }
        let completion_error = self
            .submit(
                BLOCK_QUEUE.0.get().cast::<u8>(),
                dma_base,
                read_mmio16(self.common + 24),
                VIRTIO_BLK_T_IN,
                Some(attempted_iova),
                5_000_000,
            )
            .is_err();
        let sentinel_intact =
            unsafe { core::ptr::read_volatile(sentinel as *const u64) } == SENTINEL_VALUE;
        DmaFaultProbe {
            completion_error,
            sentinel_intact,
            attempted_iova,
        }
    }

    fn submit(
        &self,
        queue: *mut u8,
        dma_base: u64,
        queue_size: u16,
        request_type: u32,
        data_address: Option<u64>,
        spin_limit: u32,
    ) -> Result<(), BlockIoError> {
        unsafe {
            core::ptr::write_volatile(queue.add(HEADER_OFFSET) as *mut u32, request_type);
            core::ptr::write_volatile(queue.add(HEADER_OFFSET + 4) as *mut u32, 0);
            core::ptr::write_volatile(queue.add(HEADER_OFFSET + 8) as *mut u64, 0);
            core::ptr::write_volatile(queue.add(STATUS_OFFSET), u8::MAX);

            write_descriptor(
                queue,
                0,
                dma_base + HEADER_OFFSET as u64,
                16,
                VRING_DESC_F_NEXT,
                1,
            );
            if let Some(data_address) = data_address {
                let data_flags = if request_type == VIRTIO_BLK_T_IN {
                    VRING_DESC_F_NEXT | VRING_DESC_F_WRITE
                } else {
                    VRING_DESC_F_NEXT
                };
                write_descriptor(queue, 1, data_address, 512, data_flags, 2);
                write_descriptor(
                    queue,
                    2,
                    dma_base + STATUS_OFFSET as u64,
                    1,
                    VRING_DESC_F_WRITE,
                    0,
                );
            } else {
                write_descriptor(
                    queue,
                    1,
                    dma_base + STATUS_OFFSET as u64,
                    1,
                    VRING_DESC_F_WRITE,
                    0,
                );
            }

            let available = queue.add(AVAIL_OFFSET);
            let available_index = core::ptr::read_volatile(available.add(2) as *const u16);
            let used_index = core::ptr::read_volatile(queue.add(USED_OFFSET + 2) as *const u16);
            let ring_offset = 4 + (available_index % queue_size) as usize * 2;
            core::ptr::write_volatile(available.add(ring_offset) as *mut u16, 0);
            dma_write_barrier();
            core::ptr::write_volatile(
                available.add(2) as *mut u16,
                available_index.wrapping_add(1),
            );
            dma_write_barrier();

            let notify_offset = self
                .queue_notify_offset()
                .ok_or(BlockIoError::QueueUnavailable)?;
            write_mmio16(self.notify + notify_offset, 0);

            for _ in 0..spin_limit {
                if core::ptr::read_volatile(queue.add(USED_OFFSET + 2) as *const u16) != used_index
                {
                    dma_read_barrier();
                    let status = core::ptr::read_volatile(queue.add(STATUS_OFFSET));
                    return if status == 0 {
                        Ok(())
                    } else {
                        Err(BlockIoError::Device(status))
                    };
                }
                core::hint::spin_loop();
            }
        }
        Err(BlockIoError::TimedOut)
    }
}

fn physical_page(high_address: usize) -> u64 {
    crate::arch::virt_to_phys(high_address as u64).unwrap_or(0) & !0xfff
}

fn configured_bars(ecam: usize, device: Device) -> Result<[Option<BarRange>; 6], VirtioError> {
    let origin = ecam;
    let ecam = ecam + ((device.bus as usize) << 20);
    let id = read32(ecam, device.slot, device.function, 0);
    let count = if read8(ecam, device.slot, device.function, 0x0e) & 0x7f == 1 {
        2
    } else {
        6
    };
    let raw = core::array::from_fn(|index| {
        if index < count {
            read32(ecam, device.slot, device.function, 0x10 + index * 4)
        } else {
            0
        }
    });
    // Probing BAR sizes changes QEMU's address-space topology. Reuse validated
    // sizes while addresses/identity are unchanged, especially after DMA starts.
    for index in 0..64 {
        unsafe {
            if let Some(cached) = BAR_CACHE[index] {
                if cached.ecam == origin
                    && cached.requester == device.requester_id()
                    && cached.id == id
                    && cached.raw == raw
                {
                    return Ok(cached.bars);
                }
            }
        }
    }
    let mut bars = [None; 6];
    let command = read16(ecam, device.slot, device.function, 0x04);
    write16(ecam, device.slot, device.function, 0x04, command & !0x7);
    let mut index = 0usize;
    while index < count {
        let low = read32(ecam, device.slot, device.function, 0x10 + index * 4);
        if low == 0 || low == u32::MAX || low & 1 != 0 {
            index += 1;
            continue;
        }
        let bar_type = low & 0x6;
        let is_64 = bar_type == 0x4;
        if bar_type == 0x6 || (is_64 && index + 1 == bars.len()) {
            index += 1;
            continue;
        }
        let high = if is_64 {
            read32(ecam, device.slot, device.function, 0x14 + index * 4)
        } else {
            0
        };
        write32(
            ecam,
            device.slot,
            device.function,
            0x10 + index * 4,
            u32::MAX,
        );
        if is_64 {
            write32(
                ecam,
                device.slot,
                device.function,
                0x14 + index * 4,
                u32::MAX,
            );
        }
        let mask_low = read32(ecam, device.slot, device.function, 0x10 + index * 4);
        let mask_high = if is_64 {
            read32(ecam, device.slot, device.function, 0x14 + index * 4)
        } else {
            0
        };
        write32(ecam, device.slot, device.function, 0x10 + index * 4, low);
        if is_64 {
            write32(ecam, device.slot, device.function, 0x14 + index * 4, high);
        }

        let base = ((high as u64) << 32) | (low & !0xf) as u64;
        let size = if is_64 {
            let mask = ((mask_high as u64) << 32) | (mask_low & !0xf) as u64;
            (!mask).wrapping_add(1)
        } else {
            (!(mask_low & !0xf)).wrapping_add(1) as u64
        };
        if base != 0 && size >= 16 && size.is_power_of_two() && base & (size - 1) == 0 {
            bars[index] = Some(BarRange { base, bytes: size });
        }
        index += if is_64 { 2 } else { 1 };
    }
    write16(ecam, device.slot, device.function, 0x04, command);
    let mut slot = None;
    for index in 0..64 {
        unsafe {
            if BAR_CACHE[index].is_some_and(|cached| {
                cached.ecam == origin && cached.requester == device.requester_id()
            }) {
                slot = Some(index);
                break;
            }
            if BAR_CACHE[index].is_none() && slot.is_none() {
                slot = Some(index);
            }
        }
    }
    if let Some(index) = slot {
        unsafe {
            BAR_CACHE[index] = Some(BarCache {
                ecam: origin,
                requester: device.requester_id(),
                id,
                raw,
                bars,
            });
        }
    }
    Ok(bars)
}

fn config_address(ecam_physical: usize, slot: u8, function: u8, offset: usize) -> usize {
    crate::arch::phys_to_virt(
        ecam_physical as u64 + ((slot as u64) << 15) + ((function as u64) << 12) + offset as u64,
    )
}

pub(crate) fn read8(ecam_physical: usize, slot: u8, function: u8, offset: usize) -> u8 {
    unsafe {
        core::ptr::read_volatile(config_address(ecam_physical, slot, function, offset) as *const u8)
    }
}

pub(crate) fn read16(ecam_physical: usize, slot: u8, function: u8, offset: usize) -> u16 {
    unsafe {
        core::ptr::read_volatile(config_address(ecam_physical, slot, function, offset) as *const u16)
    }
}

pub(crate) fn write32(ecam_physical: usize, slot: u8, function: u8, offset: usize, value: u32) {
    unsafe {
        core::ptr::write_volatile(
            config_address(ecam_physical, slot, function, offset) as *mut u32,
            value,
        )
    }
}

pub(crate) fn read32(ecam_physical: usize, slot: u8, function: u8, offset: usize) -> u32 {
    unsafe {
        core::ptr::read_volatile(config_address(ecam_physical, slot, function, offset) as *const u32)
    }
}

pub(crate) fn write16(ecam_physical: usize, slot: u8, function: u8, offset: usize, value: u16) {
    unsafe {
        core::ptr::write_volatile(
            config_address(ecam_physical, slot, function, offset) as *mut u16,
            value,
        )
    }
}

fn read_mmio8(address: usize) -> u8 {
    unsafe { core::ptr::read_volatile(address as *const u8) }
}
fn read_mmio16(address: usize) -> u16 {
    unsafe { core::ptr::read_volatile(address as *const u16) }
}
fn read_mmio32(address: usize) -> u32 {
    unsafe { core::ptr::read_volatile(address as *const u32) }
}
fn read_mmio64(address: usize) -> u64 {
    unsafe { core::ptr::read_volatile(address as *const u64) }
}
fn write_mmio8(address: usize, value: u8) {
    unsafe { core::ptr::write_volatile(address as *mut u8, value) }
}
fn write_mmio16(address: usize, value: u16) {
    unsafe { core::ptr::write_volatile(address as *mut u16, value) }
}
fn write_mmio32(address: usize, value: u32) {
    unsafe { core::ptr::write_volatile(address as *mut u32, value) }
}

fn write_mmio64_split(address: usize, value: u64) {
    write_mmio32(address, value as u32);
    write_mmio32(address + 4, (value >> 32) as u32);
}

unsafe fn write_descriptor(
    queue: *mut u8,
    index: usize,
    address: u64,
    length: u32,
    flags: u16,
    next: u16,
) {
    let descriptor = unsafe { queue.add(DESC_OFFSET + index * 16) };
    unsafe {
        core::ptr::write_volatile(descriptor as *mut u64, address);
        core::ptr::write_volatile(descriptor.add(8) as *mut u32, length);
        core::ptr::write_volatile(descriptor.add(12) as *mut u16, flags);
        core::ptr::write_volatile(descriptor.add(14) as *mut u16, next);
    }
}

unsafe fn dma_write_barrier() {
    crate::arch::dma_write_barrier()
}

unsafe fn dma_read_barrier() {
    crate::arch::dma_read_barrier()
}
