use core::cell::UnsafeCell;

const KERNEL_OFFSET: usize = 0xffffff8000000000;
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
    pub isr: usize,
    pub device_config: usize,
    pub notify_multiplier: u32,
    pub queues: u16,
    pub capacity_sectors: u64,
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
    if ecam_physical == 0 || slot >= 32 || function >= 8 {
        return None;
    }
    let id = read32(ecam_physical, slot, function, 0);
    let vendor = id as u16;
    let device = (id >> 16) as u16;
    (vendor == VIRTIO_VENDOR && device == expected_device).then_some(Device {
        bus: 0,
        slot,
        function,
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
        ecam_physical,
        device,
        configured_bars(ecam_physical, device),
        mmio_base,
        mmio_bytes,
    )
}

pub fn interrupt_pin(ecam_physical: usize, device: Device) -> u8 {
    read8(ecam_physical, device.slot, device.function, 0x3d)
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
    transport_from_bars(
        ecam_physical,
        device,
        configured_bars(ecam_physical, device),
        mmio_base,
        mmio_bytes,
    )
}

fn transport_from_bars(
    ecam_physical: usize,
    device: Device,
    bars: [u64; 6],
    mmio_base: usize,
    mmio_bytes: usize,
) -> Result<VirtioTransport, VirtioError> {
    let window_end = (mmio_base as u64)
        .checked_add(mmio_bytes as u64)
        .ok_or(VirtioError::MissingWindow)?;
    let mut common = 0usize;
    let mut notify = 0usize;
    let mut isr = 0usize;
    let mut device_config = 0usize;
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
            let length = read8(
                ecam_physical,
                device.slot,
                device.function,
                capability as usize + 2,
            );
            let kind = read8(
                ecam_physical,
                device.slot,
                device.function,
                capability as usize + 3,
            );
            let bar = read8(
                ecam_physical,
                device.slot,
                device.function,
                capability as usize + 4,
            ) as usize;
            if matches!(kind, 1..=4) {
                if bar >= bars.len() || bars[bar] == 0 {
                    return Err(VirtioError::CapabilityBar);
                }
                let offset = read32(
                    ecam_physical,
                    device.slot,
                    device.function,
                    capability as usize + 8,
                ) as usize;
                let physical = bars[bar]
                    .checked_add(offset as u64)
                    .filter(|address| *address >= mmio_base as u64 && *address < window_end)
                    .ok_or(VirtioError::CapabilityBar)?;
                let address = KERNEL_OFFSET + physical as usize;
                match kind {
                    1 => common = address,
                    2 => {
                        notify = address;
                        if length >= 20 {
                            notify_multiplier = read32(
                                ecam_physical,
                                device.slot,
                                device.function,
                                capability as usize + 16,
                            );
                        }
                    }
                    3 => isr = address,
                    4 => device_config = address,
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
    if device_config == 0 {
        return Err(VirtioError::DeviceConfig);
    }

    Ok(VirtioTransport {
        common,
        notify,
        isr,
        device_config,
        notify_multiplier,
        queues: read_mmio16(common + 18),
        capacity_sectors: read_mmio64(device_config),
    })
}

impl VirtioTransport {
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
        BLOCK_QUEUE.0.get() as usize as u64 - KERNEL_OFFSET as u64
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
        BLOCK_DATA.0.get() as usize as u64 - KERNEL_OFFSET as u64
    }

    pub fn reset_for_user(&self, ecam_physical: usize, device: Device) {
        write_mmio8(self.common + 20, 0);
        let command = read16(ecam_physical, device.slot, device.function, 0x04);
        write16(
            ecam_physical,
            device.slot,
            device.function,
            0x04,
            command & !((1 << 1) | (1 << 2)),
        );
        unsafe { core::arch::asm!("dsb sy", options(nostack, preserves_flags)) };
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
        let attempted_iova = sentinel as usize as u64 - KERNEL_OFFSET as u64;
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

            write_mmio16(
                self.notify
                    + read_mmio16(self.common + 30) as usize * self.notify_multiplier as usize,
                0,
            );

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
    ((high_address - KERNEL_OFFSET) as u64) & !0xfff
}

fn configured_bars(ecam: usize, device: Device) -> [u64; 6] {
    let mut bars = [0u64; 6];
    let mut index = 0usize;
    while index < bars.len() {
        let low = read32(ecam, device.slot, device.function, 0x10 + index * 4);
        if low & 1 != 0 {
            index += 1;
            continue;
        }
        let is_64 = low & 0x6 == 0x4 && index + 1 < bars.len();
        let mut address = (low & !0xf) as u64;
        if is_64 {
            address |= (read32(ecam, device.slot, device.function, 0x14 + index * 4) as u64) << 32;
        }
        bars[index] = address;
        index += if is_64 { 2 } else { 1 };
    }
    bars
}

fn config_address(ecam_physical: usize, slot: u8, function: u8, offset: usize) -> usize {
    KERNEL_OFFSET + ecam_physical + ((slot as usize) << 15) + ((function as usize) << 12) + offset
}

fn read8(ecam_physical: usize, slot: u8, function: u8, offset: usize) -> u8 {
    unsafe {
        core::ptr::read_volatile(config_address(ecam_physical, slot, function, offset) as *const u8)
    }
}

fn read16(ecam_physical: usize, slot: u8, function: u8, offset: usize) -> u16 {
    unsafe {
        core::ptr::read_volatile(config_address(ecam_physical, slot, function, offset) as *const u16)
    }
}

fn read32(ecam_physical: usize, slot: u8, function: u8, offset: usize) -> u32 {
    unsafe {
        core::ptr::read_volatile(config_address(ecam_physical, slot, function, offset) as *const u32)
    }
}

fn write16(ecam_physical: usize, slot: u8, function: u8, offset: usize, value: u16) {
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
    unsafe { core::arch::asm!("dmb oshst", options(nostack, preserves_flags)) }
}

unsafe fn dma_read_barrier() {
    unsafe { core::arch::asm!("dmb oshld", options(nostack, preserves_flags)) }
}
