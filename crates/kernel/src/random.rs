use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicBool, Ordering};

use crate::dtb::PlatformInfo;
use crate::pci;
use crate::smmu;

const DESC_OFFSET: usize = 0;
const AVAIL_OFFSET: usize = 256;
const USED_OFFSET: usize = 320;
const DATA_OFFSET: usize = 512;
const STATUS_ACKNOWLEDGE: u8 = 1;
const STATUS_DRIVER: u8 = 2;
const STATUS_DRIVER_OK: u8 = 4;
const STATUS_FEATURES_OK: u8 = 8;
const STATUS_FAILED: u8 = 0x80;
const VRING_DESC_F_WRITE: u16 = 2;

#[repr(C, align(4096))]
struct QueuePage([u8; 4096]);
struct QueueCell(UnsafeCell<QueuePage>);
unsafe impl Sync for QueueCell {}

static QUEUE: QueueCell = QueueCell(UnsafeCell::new(QueuePage([0; 4096])));
static ACTIVE: AtomicBool = AtomicBool::new(false);
static FIRST_FILL_REPORTED: AtomicBool = AtomicBool::new(false);
static mut DEVICE: Option<RandomDevice> = None;

#[derive(Clone, Copy)]
struct RandomDevice {
    transport: pci::VirtioTransport,
    stream_id: u32,
    notify: usize,
    available: u16,
    used: u16,
}

pub fn activate(platform: PlatformInfo) -> Result<(), ()> {
    let device =
        pci::virtio_device_at(platform.pcie_base, 7, 0, pci::VIRTIO_RNG_MODERN).ok_or(())?;
    let transport = pci::inspect_configured_virtio(
        platform.pcie_base,
        device,
        platform.pcie_mmio_base,
        platform.pcie_mmio_bytes,
    )
    .map_err(|_| ())?;
    let stream_id = platform.stream_id(device.requester_id()).ok_or(())?;
    let physical = crate::arch::virt_to_phys(QUEUE.0.get() as usize as u64).ok_or(())?;
    smmu::map_gui_aux_page(stream_id, smmu::RNG_QUEUE_IOVA, physical).map_err(|_| ())?;
    smmu::attach_stream(stream_id).map_err(|_| ())?;
    negotiate(transport)?;
    let queue = unsafe { &mut (*QUEUE.0.get()).0 };
    queue.fill(0);
    write16(transport.common + 22, 0);
    if read16(transport.common + 24) == 0 {
        return Err(());
    }
    write16(transport.common + 24, 1);
    write64(
        transport.common + 32,
        smmu::RNG_QUEUE_IOVA + DESC_OFFSET as u64,
    );
    write64(
        transport.common + 40,
        smmu::RNG_QUEUE_IOVA + AVAIL_OFFSET as u64,
    );
    write64(
        transport.common + 48,
        smmu::RNG_QUEUE_IOVA + USED_OFFSET as u64,
    );
    put64(
        queue,
        DESC_OFFSET,
        smmu::RNG_QUEUE_IOVA + DATA_OFFSET as u64,
    );
    put32(queue, DESC_OFFSET + 8, 256);
    put16(queue, DESC_OFFSET + 12, VRING_DESC_F_WRITE);
    let notify = transport.queue_notify_offset().ok_or(())?;
    write16(transport.common + 28, 1);
    write8(
        transport.common + 20,
        read8(transport.common + 20) | STATUS_DRIVER_OK,
    );
    unsafe {
        DEVICE = Some(RandomDevice {
            transport,
            stream_id,
            notify,
            available: 0,
            used: 0,
        })
    };
    ACTIVE.store(true, Ordering::Release);
    crate::kprintln!("[rng] virtio-rng ready entropy-source=host");
    Ok(())
}

pub fn fill(output: &mut [u8]) -> Result<(), ()> {
    if !ACTIVE.load(Ordering::Acquire) || output.is_empty() || output.len() > 256 {
        return Err(());
    }
    let mut device = unsafe { DEVICE.ok_or(())? };
    let queue = unsafe { &mut (*QUEUE.0.get()).0 };
    put32(queue, DESC_OFFSET + 8, output.len() as u32);
    put16(
        queue,
        AVAIL_OFFSET + 4 + (device.available as usize % 1) * 2,
        0,
    );
    device.available = device.available.wrapping_add(1);
    dma_barrier();
    put16(queue, AVAIL_OFFSET + 2, device.available);
    dma_barrier();
    write16(device.transport.notify + device.notify, 0);
    let deadline = crate::arch::clock_nanos().saturating_add(1_000_000_000);
    while crate::arch::clock_nanos() < deadline {
        let used = get16(queue, USED_OFFSET + 2);
        if used != device.used {
            dma_barrier();
            let bytes = get32(queue, USED_OFFSET + 8) as usize;
            if bytes < output.len() {
                return Err(());
            }
            output.copy_from_slice(&queue[DATA_OFFSET..DATA_OFFSET + output.len()]);
            device.used = used;
            unsafe { DEVICE = Some(device) };
            if !FIRST_FILL_REPORTED.swap(true, Ordering::AcqRel) {
                crate::kprintln!("[rng] first-fill bytes={} dma=true", output.len());
            }
            return Ok(());
        }
        core::hint::spin_loop();
    }
    let available = get16(queue, AVAIL_OFFSET + 2);
    let used = get16(queue, USED_OFFSET + 2);
    let status = read8(device.transport.common + 20);
    let queue_enabled = read16(device.transport.common + 28);
    #[cfg(target_arch = "riscv64")]
    let stream = smmu::stream_diagnostics(device.stream_id, smmu::RNG_QUEUE_IOVA);
    if let Some(fault) = smmu::take_fault() {
        crate::kprintln!(
            "[rng] fill timeout avail={} used={} status={:#x} queue-enabled={} event={:#x} stream-id={:#x} iova={:#x}",
            available,
            used,
            status,
            queue_enabled,
            fault.event_type,
            fault.stream_id,
            fault.address
        );
    } else {
        #[cfg(any(target_arch = "aarch64", target_arch = "x86_64"))]
        crate::kprintln!(
            "[rng] fill timeout avail={} used={} status={:#x} queue-enabled={} event=none",
            available,
            used,
            status,
            queue_enabled
        );
        #[cfg(target_arch = "riscv64")]
        crate::kprintln!(
            "[rng] fill timeout avail={} used={} status={:#x} queue-enabled={} event=none stream-id={:#x} tc={:#x} fsc={:#x} pte={:#x}",
            available,
            used,
            status,
            queue_enabled,
            device.stream_id,
            stream.tc,
            stream.fsc,
            stream.pte
        );
    }
    Err(())
}

fn negotiate(transport: pci::VirtioTransport) -> Result<(), ()> {
    write8(transport.common + 20, 0);
    write8(transport.common + 20, STATUS_ACKNOWLEDGE | STATUS_DRIVER);
    write32(transport.common, 1);
    if read32(transport.common + 4) & 0b11 != 0b11 {
        write8(transport.common + 20, STATUS_FAILED);
        return Err(());
    }
    write32(transport.common + 8, 0);
    write32(transport.common + 12, 0);
    write32(transport.common + 8, 1);
    write32(transport.common + 12, 0b11);
    write8(
        transport.common + 20,
        read8(transport.common + 20) | STATUS_FEATURES_OK,
    );
    (read8(transport.common + 20) & STATUS_FEATURES_OK != 0)
        .then_some(())
        .ok_or(())
}

fn read8(address: usize) -> u8 {
    unsafe { core::ptr::read_volatile(address as *const u8) }
}
fn read16(address: usize) -> u16 {
    unsafe { core::ptr::read_volatile(address as *const u16) }
}
fn read32(address: usize) -> u32 {
    unsafe { core::ptr::read_volatile(address as *const u32) }
}
fn write8(address: usize, value: u8) {
    unsafe { core::ptr::write_volatile(address as *mut u8, value) }
}
fn write16(address: usize, value: u16) {
    unsafe { core::ptr::write_volatile(address as *mut u16, value) }
}
fn write32(address: usize, value: u32) {
    unsafe { core::ptr::write_volatile(address as *mut u32, value) }
}
fn write64(address: usize, value: u64) {
    write32(address, value as u32);
    write32(address + 4, (value >> 32) as u32);
}
fn get16(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes(bytes[offset..offset + 2].try_into().unwrap())
}
fn get32(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}
fn put16(bytes: &mut [u8], offset: usize, value: u16) {
    bytes[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
}
fn put32(bytes: &mut [u8], offset: usize, value: u32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}
fn put64(bytes: &mut [u8], offset: usize, value: u64) {
    bytes[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
}
fn dma_barrier() {
    crate::arch::dma_barrier();
}
