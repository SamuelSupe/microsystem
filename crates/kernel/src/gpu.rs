use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicBool, Ordering};

use crate::dtb::PlatformInfo;
use crate::pci;
use crate::smmu;

const WIDTH: usize = 1024;
const HEIGHT: usize = 768;
const FRAMEBUFFER_BYTES: usize = WIDTH * HEIGHT * 4;
const FRAMEBUFFER_PAGES: usize = FRAMEBUFFER_BYTES / 4096;
const QUEUE_BYTES: usize = 4096;
const QUEUE_SIZE: u16 = 16;
const DESC_OFFSET: usize = 0;
const AVAIL_OFFSET: usize = 256;
const USED_OFFSET: usize = 320;
const REQUEST_OFFSET: usize = 512;
const RESPONSE_OFFSET: usize = 1024;

const VIRTIO_F_VERSION_1_ACCESS_PLATFORM: u32 = 0b11;
const STATUS_ACKNOWLEDGE: u8 = 1;
const STATUS_DRIVER: u8 = 2;
const STATUS_DRIVER_OK: u8 = 4;
const STATUS_FEATURES_OK: u8 = 8;
const STATUS_FAILED: u8 = 0x80;

const CMD_RESOURCE_CREATE_2D: u32 = 0x0101;
const CMD_SET_SCANOUT: u32 = 0x0103;
const CMD_RESOURCE_FLUSH: u32 = 0x0104;
const CMD_TRANSFER_TO_HOST_2D: u32 = 0x0105;
const CMD_RESOURCE_ATTACH_BACKING: u32 = 0x0106;
const RESP_OK_NODATA: u32 = 0x1100;
const FORMAT_B8G8R8X8_UNORM: u32 = 2;
const RESOURCE_ID: u32 = 1;
const VRING_DESC_F_NEXT: u16 = 1;
const VRING_DESC_F_WRITE: u16 = 2;

#[repr(C, align(4096))]
struct Framebuffer([u32; WIDTH * HEIGHT]);

#[repr(C, align(4096))]
struct Queue([u8; QUEUE_BYTES]);

struct FramebufferCell(UnsafeCell<Framebuffer>);
struct QueueCell(UnsafeCell<Queue>);

unsafe impl Sync for FramebufferCell {}
unsafe impl Sync for QueueCell {}

static FRAMEBUFFER: FramebufferCell =
    FramebufferCell(UnsafeCell::new(Framebuffer([0; WIDTH * HEIGHT])));
static QUEUE: QueueCell = QueueCell(UnsafeCell::new(Queue([0; QUEUE_BYTES])));
static ACTIVE: AtomicBool = AtomicBool::new(false);
static mut TRANSPORT: Option<pci::VirtioTransport> = None;

pub fn activate(platform: PlatformInfo) -> Result<(), ()> {
    let device = (0..32u8)
        .find_map(|slot| pci::virtio_device_at(platform.pcie_base, slot, 0, pci::VIRTIO_GPU_MODERN))
        .ok_or_else(|| activation_failed("discover"))?;
    let transport = pci::inspect_configured_virtio(
        platform.pcie_base,
        device,
        platform.pcie_mmio_base,
        platform.pcie_mmio_bytes,
    )
    .map_err(|_| activation_failed("transport"))?;
    let stream_id = platform
        .stream_id(device.requester_id())
        .ok_or_else(|| activation_failed("stream-id"))?;
    smmu::map_gui_pages(framebuffer_physical(), FRAMEBUFFER_PAGES, queue_physical())
        .map_err(|_| activation_failed("dma-map"))?;
    smmu::attach_stream(stream_id).map_err(|_| activation_failed("iommu-attach"))?;
    negotiate(transport).map_err(|_| activation_failed("features"))?;
    configure_queue(transport).map_err(|_| activation_failed("queue"))?;
    create_scanout(transport).map_err(|_| activation_failed("scanout"))?;
    unsafe { TRANSPORT = Some(transport) };
    ACTIVE.store(true, Ordering::Release);
    crate::kprintln!(
        "[gui] virtio-gpu scanout ready resolution=1024x768 format=XRGB8888 stream-id={:#x} iova={:#x}",
        stream_id,
        smmu::GUI_IOVA
    );
    Ok(())
}

fn activation_failed(stage: &str) {
    crate::kprintln!("[gui] virtio-gpu activation failed stage={}", stage);
}

pub fn framebuffer_mut() -> Option<&'static mut [u8]> {
    if !ACTIVE.load(Ordering::Acquire) {
        return None;
    }
    Some(unsafe {
        core::slice::from_raw_parts_mut(FRAMEBUFFER.0.get().cast::<u8>(), FRAMEBUFFER_BYTES)
    })
}

pub fn flush_rect(x: u32, y: u32, width: u32, height: u32) -> Result<(), ()> {
    if !ACTIVE.load(Ordering::Acquire) {
        return Err(());
    }
    if width == 0
        || height == 0
        || x.checked_add(width)
            .is_none_or(|right| right > WIDTH as u32)
        || y.checked_add(height)
            .is_none_or(|bottom| bottom > HEIGHT as u32)
    {
        return Err(());
    }
    let transport = unsafe { TRANSPORT.ok_or(())? };
    command_transfer(transport, x, y, width, height)?;
    command_flush(transport, x, y, width, height)
}

fn negotiate(transport: pci::VirtioTransport) -> Result<(), ()> {
    write8(transport.common + 20, 0);
    write8(transport.common + 20, STATUS_ACKNOWLEDGE | STATUS_DRIVER);
    write32(transport.common, 1);
    let high = read32(transport.common + 4);
    if high & VIRTIO_F_VERSION_1_ACCESS_PLATFORM != VIRTIO_F_VERSION_1_ACCESS_PLATFORM {
        write8(transport.common + 20, STATUS_FAILED);
        return Err(());
    }
    write32(transport.common + 8, 0);
    write32(transport.common + 12, 0);
    write32(transport.common + 8, 1);
    write32(transport.common + 12, VIRTIO_F_VERSION_1_ACCESS_PLATFORM);
    let status = read8(transport.common + 20) | STATUS_FEATURES_OK;
    write8(transport.common + 20, status);
    if read8(transport.common + 20) & STATUS_FEATURES_OK == 0 {
        return Err(());
    }
    Ok(())
}

fn configure_queue(transport: pci::VirtioTransport) -> Result<(), ()> {
    let queue = unsafe { &mut (*QUEUE.0.get()).0 };
    queue.fill(0);
    write16(transport.common + 22, 0);
    let maximum = read16(transport.common + 24);
    if maximum < 2 {
        return Err(());
    }
    write16(transport.common + 24, maximum.min(QUEUE_SIZE));
    write64(
        transport.common + 32,
        smmu::GUI_QUEUE_IOVA + DESC_OFFSET as u64,
    );
    write64(
        transport.common + 40,
        smmu::GUI_QUEUE_IOVA + AVAIL_OFFSET as u64,
    );
    write64(
        transport.common + 48,
        smmu::GUI_QUEUE_IOVA + USED_OFFSET as u64,
    );
    write16(transport.common + 28, 1);
    if read16(transport.common + 28) != 1 {
        return Err(());
    }
    write8(
        transport.common + 20,
        read8(transport.common + 20) | STATUS_DRIVER_OK,
    );
    Ok(())
}

fn create_scanout(transport: pci::VirtioTransport) -> Result<(), ()> {
    let mut create = [0u8; 40];
    put32(&mut create, 0, CMD_RESOURCE_CREATE_2D);
    put32(&mut create, 24, RESOURCE_ID);
    put32(&mut create, 28, FORMAT_B8G8R8X8_UNORM);
    put32(&mut create, 32, WIDTH as u32);
    put32(&mut create, 36, HEIGHT as u32);
    submit(transport, &create).map_err(|_| command_failed("resource-create"))?;

    let mut attach = [0u8; 48];
    put32(&mut attach, 0, CMD_RESOURCE_ATTACH_BACKING);
    put32(&mut attach, 24, RESOURCE_ID);
    put32(&mut attach, 28, 1);
    put64(&mut attach, 32, smmu::GUI_IOVA);
    put32(&mut attach, 40, FRAMEBUFFER_BYTES as u32);
    submit(transport, &attach).map_err(|_| command_failed("attach-backing"))?;

    let mut scanout = [0u8; 48];
    put32(&mut scanout, 0, CMD_SET_SCANOUT);
    put32(&mut scanout, 32, WIDTH as u32);
    put32(&mut scanout, 36, HEIGHT as u32);
    put32(&mut scanout, 40, 0);
    put32(&mut scanout, 44, RESOURCE_ID);
    submit(transport, &scanout).map_err(|_| command_failed("set-scanout"))?;
    command_transfer(transport, 0, 0, WIDTH as u32, HEIGHT as u32)
        .map_err(|_| command_failed("transfer"))?;
    command_flush(transport, 0, 0, WIDTH as u32, HEIGHT as u32).map_err(|_| command_failed("flush"))
}

fn command_failed(stage: &str) {
    crate::kprintln!("[gui] virtio-gpu command failed stage={}", stage);
}

fn command_transfer(
    transport: pci::VirtioTransport,
    x: u32,
    y: u32,
    width: u32,
    height: u32,
) -> Result<(), ()> {
    let mut command = [0u8; 56];
    put32(&mut command, 0, CMD_TRANSFER_TO_HOST_2D);
    put32(&mut command, 24, x);
    put32(&mut command, 28, y);
    put32(&mut command, 32, width);
    put32(&mut command, 36, height);
    put64(&mut command, 40, (y as u64 * WIDTH as u64 + x as u64) * 4);
    put32(&mut command, 48, RESOURCE_ID);
    submit(transport, &command)
}

fn command_flush(
    transport: pci::VirtioTransport,
    x: u32,
    y: u32,
    width: u32,
    height: u32,
) -> Result<(), ()> {
    let mut command = [0u8; 48];
    put32(&mut command, 0, CMD_RESOURCE_FLUSH);
    put32(&mut command, 24, x);
    put32(&mut command, 28, y);
    put32(&mut command, 32, width);
    put32(&mut command, 36, height);
    put32(&mut command, 40, RESOURCE_ID);
    submit(transport, &command)
}

fn submit(transport: pci::VirtioTransport, request: &[u8]) -> Result<(), ()> {
    let queue = unsafe { &mut (*QUEUE.0.get()).0 };
    queue[REQUEST_OFFSET..REQUEST_OFFSET + request.len()].copy_from_slice(request);
    queue[RESPONSE_OFFSET..RESPONSE_OFFSET + 64].fill(0);
    write_descriptor(
        queue,
        0,
        smmu::GUI_QUEUE_IOVA + REQUEST_OFFSET as u64,
        request.len() as u32,
        VRING_DESC_F_NEXT,
        1,
    );
    write_descriptor(
        queue,
        1,
        smmu::GUI_QUEUE_IOVA + RESPONSE_OFFSET as u64,
        64,
        VRING_DESC_F_WRITE,
        0,
    );
    let available = read_u16(queue, AVAIL_OFFSET + 2);
    let used = read_u16(queue, USED_OFFSET + 2);
    put_u16(
        queue,
        AVAIL_OFFSET + 4 + (available % QUEUE_SIZE) as usize * 2,
        0,
    );
    dma_barrier();
    put_u16(queue, AVAIL_OFFSET + 2, available.wrapping_add(1));
    dma_barrier();
    let notify_offset =
        read16(transport.common + 30) as usize * transport.notify_multiplier as usize;
    write16(transport.notify + notify_offset, 0);
    let deadline = crate::arch::clock_nanos().saturating_add(5_000_000_000);
    loop {
        if read_u16(queue, USED_OFFSET + 2) != used {
            dma_barrier();
            let response = read_u32(queue, RESPONSE_OFFSET);
            if response != RESP_OK_NODATA {
                crate::kprintln!("[gui] virtio-gpu response={:#x}", response);
                return Err(());
            }
            return Ok(());
        }
        if crate::arch::clock_nanos() >= deadline {
            break;
        }
        core::hint::spin_loop();
    }
    if let Some(fault) = smmu::take_fault() {
        crate::kprintln!(
            "[gui] virtio-gpu DMA fault event={:#x} stream-id={:#x} iova={:#x}",
            fault.event_type,
            fault.stream_id,
            fault.address
        );
    }
    crate::kprintln!(
        "[gui] virtio-gpu command timeout available={} used-before={} used-now={}",
        available,
        used,
        read_u16(queue, USED_OFFSET + 2)
    );
    Err(())
}

fn write_descriptor(queue: &mut [u8], index: usize, address: u64, len: u32, flags: u16, next: u16) {
    let offset = DESC_OFFSET + index * 16;
    put64(queue, offset, address);
    put32(queue, offset + 8, len);
    put_u16(queue, offset + 12, flags);
    put_u16(queue, offset + 14, next);
}

fn framebuffer_physical() -> u64 {
    crate::arch::virt_to_phys(FRAMEBUFFER.0.get() as usize as u64).unwrap_or(0)
}

fn queue_physical() -> u64 {
    crate::arch::virt_to_phys(QUEUE.0.get() as usize as u64).unwrap_or(0)
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

fn put_u16(bytes: &mut [u8], offset: usize, value: u16) {
    bytes[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
}
fn put32(bytes: &mut [u8], offset: usize, value: u32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}
fn put64(bytes: &mut [u8], offset: usize, value: u64) {
    bytes[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
}
fn read_u16(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes(bytes[offset..offset + 2].try_into().unwrap())
}
fn read_u32(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}
fn dma_barrier() {
    crate::arch::dma_barrier();
}
