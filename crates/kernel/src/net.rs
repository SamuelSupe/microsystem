use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicBool, Ordering};

use crate::dtb::PlatformInfo;
use crate::{pci, smmu};

const KERNEL_OFFSET: usize = 0xffff_ff80_0000_0000;
const RX_QUEUE_SIZE: u16 = 2;
const TX_QUEUE_SIZE: u16 = 1;
const DESC_OFFSET: usize = 0;
const AVAIL_OFFSET: usize = 256;
const USED_OFFSET: usize = 320;
const PACKET_OFFSET: usize = 512;
// Modern virtio-net always includes num_buffers; only the legacy layout may omit it.
const VIRTIO_NET_HEADER: usize = 12;
const MAX_FRAME: usize = 1536;
const PACKET_STRIDE: usize = VIRTIO_NET_HEADER + MAX_FRAME;
const STATUS_ACKNOWLEDGE: u8 = 1;
const STATUS_DRIVER: u8 = 2;
const STATUS_DRIVER_OK: u8 = 4;
const STATUS_FEATURES_OK: u8 = 8;
const STATUS_FAILED: u8 = 0x80;
const REQUIRED_FEATURES_HIGH: u32 = 0b11;
const VRING_DESC_F_WRITE: u16 = 2;

#[repr(C, align(4096))]
struct QueuePage([u8; 4096]);

struct QueueCell(UnsafeCell<QueuePage>);
unsafe impl Sync for QueueCell {}

static RX_QUEUE: QueueCell = QueueCell(UnsafeCell::new(QueuePage([0; 4096])));
static TX_QUEUE: QueueCell = QueueCell(UnsafeCell::new(QueuePage([0; 4096])));
static ACTIVE: AtomicBool = AtomicBool::new(false);
static FIRST_RX_REPORTED: AtomicBool = AtomicBool::new(false);
static FIRST_TX_REPORTED: AtomicBool = AtomicBool::new(false);
static mut DEVICE: Option<NetworkDevice> = None;

#[derive(Clone, Copy)]
struct NetworkDevice {
    transport: pci::VirtioTransport,
    rx_notify: usize,
    tx_notify: usize,
    rx_used: u16,
    tx_used: u16,
    tx_inflight: bool,
}

pub fn activate(platform: PlatformInfo) -> Result<(), ()> {
    let device =
        pci::virtio_device_at(platform.pcie_base, 6, 0, pci::VIRTIO_NET_MODERN).ok_or(())?;
    let transport = pci::inspect_configured_virtio(
        platform.pcie_base,
        device,
        platform.pcie_mmio_base,
        platform.pcie_mmio_bytes,
    )
    .map_err(|_| ())?;
    let stream_id = platform.stream_id(device.requester_id()).ok_or(())?;
    smmu::map_gui_aux_page(smmu::NET_RX_QUEUE_IOVA, physical(RX_QUEUE.0.get())).map_err(|_| ())?;
    smmu::map_gui_aux_page(smmu::NET_TX_QUEUE_IOVA, physical(TX_QUEUE.0.get())).map_err(|_| ())?;
    smmu::attach_stream(stream_id).map_err(|_| ())?;
    negotiate(transport)?;
    let rx_notify = configure_queue(
        transport,
        0,
        smmu::NET_RX_QUEUE_IOVA,
        unsafe { &mut (*RX_QUEUE.0.get()).0 },
        RX_QUEUE_SIZE,
        true,
    )?;
    let tx_notify = configure_queue(
        transport,
        1,
        smmu::NET_TX_QUEUE_IOVA,
        unsafe { &mut (*TX_QUEUE.0.get()).0 },
        TX_QUEUE_SIZE,
        false,
    )?;
    write8(
        transport.common + 20,
        read8(transport.common + 20) | STATUS_DRIVER_OK,
    );
    unsafe {
        DEVICE = Some(NetworkDevice {
            transport,
            rx_notify,
            tx_notify,
            rx_used: 0,
            tx_used: 0,
            tx_inflight: false,
        })
    };
    ACTIVE.store(true, Ordering::Release);
    notify(transport, rx_notify, 0);
    crate::kprintln!(
        "[net] virtio-net ready mac=52:54:00:12:34:56 ipv4=10.0.2.15/24 rx-buffers=2 stream-id={:#x}",
        stream_id
    );
    Ok(())
}

pub fn receive(output: &mut [u8]) -> Result<usize, ()> {
    if !ACTIVE.load(Ordering::Acquire) {
        return Err(());
    }
    let mut device = unsafe { DEVICE.ok_or(())? };
    let queue = unsafe { &mut (*RX_QUEUE.0.get()).0 };
    let used = get16(queue, USED_OFFSET + 2);
    if used == device.rx_used {
        return Ok(0);
    }
    dma_barrier();
    let slot = (device.rx_used % RX_QUEUE_SIZE) as usize;
    let descriptor = get32(queue, USED_OFFSET + 4 + slot * 8) as u16;
    let bytes = get32(queue, USED_OFFSET + 8 + slot * 8) as usize;
    if descriptor >= RX_QUEUE_SIZE || bytes < VIRTIO_NET_HEADER {
        return Err(());
    }
    let payload = bytes - VIRTIO_NET_HEADER;
    let packet_offset = PACKET_OFFSET + descriptor as usize * PACKET_STRIDE;
    if payload > output.len() || packet_offset + bytes > queue.len() {
        return Err(());
    }
    output[..payload].copy_from_slice(
        &queue[packet_offset + VIRTIO_NET_HEADER..packet_offset + bytes],
    );
    if payload >= 14 && !FIRST_RX_REPORTED.swap(true, Ordering::AcqRel) {
        crate::kprintln!(
            "[net] first-rx bytes={} virtio={:02x}{:02x} ethernet={:02x}{:02x}{:02x}{:02x}{:02x}{:02x} ether-type={:02x}{:02x}",
            payload,
            queue[packet_offset],
            queue[packet_offset + 1],
            output[0],
            output[1],
            output[2],
            output[3],
            output[4],
            output[5],
            output[12],
            output[13]
        );
    }
    device.rx_used = device.rx_used.wrapping_add(1);
    let available = get16(queue, AVAIL_OFFSET + 2);
    put16(
        queue,
        AVAIL_OFFSET + 4 + (available % RX_QUEUE_SIZE) as usize * 2,
        descriptor,
    );
    dma_barrier();
    put16(queue, AVAIL_OFFSET + 2, available.wrapping_add(1));
    dma_barrier();
    notify(device.transport, device.rx_notify, 0);
    unsafe { DEVICE = Some(device) };
    Ok(payload)
}

pub fn send(frame: &[u8]) -> Result<bool, ()> {
    if !ACTIVE.load(Ordering::Acquire) || frame.is_empty() || frame.len() > MAX_FRAME {
        return Err(());
    }
    let mut device = unsafe { DEVICE.ok_or(())? };
    let queue = unsafe { &mut (*TX_QUEUE.0.get()).0 };
    let used = get16(queue, USED_OFFSET + 2);
    if device.tx_inflight {
        if used == device.tx_used {
            return Ok(false);
        }
        device.tx_used = used;
        device.tx_inflight = false;
    }
    queue[PACKET_OFFSET..PACKET_OFFSET + VIRTIO_NET_HEADER].fill(0);
    let end = PACKET_OFFSET + VIRTIO_NET_HEADER + frame.len();
    queue[PACKET_OFFSET + VIRTIO_NET_HEADER..end].copy_from_slice(frame);
    put32(
        queue,
        DESC_OFFSET + 8,
        (VIRTIO_NET_HEADER + frame.len()) as u32,
    );
    let available = get16(queue, AVAIL_OFFSET + 2);
    put16(
        queue,
        AVAIL_OFFSET + 4 + (available % TX_QUEUE_SIZE) as usize * 2,
        0,
    );
    dma_barrier();
    put16(queue, AVAIL_OFFSET + 2, available.wrapping_add(1));
    dma_barrier();
    notify(device.transport, device.tx_notify, 1);
    if frame.len() >= 14 && !FIRST_TX_REPORTED.swap(true, Ordering::AcqRel) {
        crate::kprintln!(
            "[net] first-tx bytes={} ethernet={:02x}{:02x}{:02x}{:02x}{:02x}{:02x} ether-type={:02x}{:02x}",
            frame.len(),
            frame[0],
            frame[1],
            frame[2],
            frame[3],
            frame[4],
            frame[5],
            frame[12],
            frame[13]
        );
    }
    device.tx_inflight = true;
    unsafe { DEVICE = Some(device) };
    Ok(true)
}

fn negotiate(transport: pci::VirtioTransport) -> Result<(), ()> {
    write8(transport.common + 20, 0);
    write8(transport.common + 20, STATUS_ACKNOWLEDGE | STATUS_DRIVER);
    write32(transport.common, 1);
    let high = read32(transport.common + 4);
    if high & REQUIRED_FEATURES_HIGH != REQUIRED_FEATURES_HIGH {
        write8(transport.common + 20, STATUS_FAILED);
        return Err(());
    }
    write32(transport.common + 8, 0);
    write32(transport.common + 12, 0);
    write32(transport.common + 8, 1);
    write32(transport.common + 12, REQUIRED_FEATURES_HIGH);
    write8(
        transport.common + 20,
        read8(transport.common + 20) | STATUS_FEATURES_OK,
    );
    (read8(transport.common + 20) & STATUS_FEATURES_OK != 0)
        .then_some(())
        .ok_or(())
}

fn configure_queue(
    transport: pci::VirtioTransport,
    index: u16,
    iova: u64,
    queue: &mut [u8],
    queue_size: u16,
    receive: bool,
) -> Result<usize, ()> {
    queue.fill(0);
    write16(transport.common + 22, index);
    if read16(transport.common + 24) < queue_size {
        return Err(());
    }
    write16(transport.common + 24, queue_size);
    write64(transport.common + 32, iova + DESC_OFFSET as u64);
    write64(transport.common + 40, iova + AVAIL_OFFSET as u64);
    write64(transport.common + 48, iova + USED_OFFSET as u64);
    for descriptor in 0..queue_size {
        let descriptor_offset = DESC_OFFSET + descriptor as usize * 16;
        let packet_offset = PACKET_OFFSET + descriptor as usize * PACKET_STRIDE;
        if packet_offset + PACKET_STRIDE > queue.len() {
            return Err(());
        }
        put64(queue, descriptor_offset, iova + packet_offset as u64);
        put32(queue, descriptor_offset + 8, PACKET_STRIDE as u32);
        put16(
            queue,
            descriptor_offset + 12,
            if receive { VRING_DESC_F_WRITE } else { 0 },
        );
        if receive {
            put16(queue, AVAIL_OFFSET + 4 + descriptor as usize * 2, descriptor);
        }
    }
    if receive {
        put16(queue, AVAIL_OFFSET + 2, queue_size);
    }
    dma_barrier();
    let notify_offset =
        read16(transport.common + 30) as usize * transport.notify_multiplier as usize;
    write16(transport.common + 28, 1);
    (read16(transport.common + 28) == 1)
        .then_some(notify_offset)
        .ok_or(())
}

fn notify(transport: pci::VirtioTransport, offset: usize, queue: u16) {
    write16(transport.notify + offset, queue);
}

fn physical<T>(pointer: *mut T) -> u64 {
    pointer as usize as u64 - KERNEL_OFFSET as u64
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
    unsafe {
        core::arch::asm!("dmb osh", options(nostack, preserves_flags));
    }
}
