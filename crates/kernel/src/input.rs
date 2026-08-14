use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicBool, Ordering};

use microsystem_abi::gui::InputEvent;

use crate::dtb::PlatformInfo;
use crate::{pci, smmu};

const KERNEL_OFFSET: usize = 0xffff_ff80_0000_0000;
const QUEUE_SIZE: u16 = 16;
const DESC_OFFSET: usize = 0;
const AVAIL_OFFSET: usize = 256;
const USED_OFFSET: usize = 320;
const EVENT_OFFSET: usize = 512;
const EVENT_BYTES: usize = core::mem::size_of::<InputEvent>();
const STATUS_ACKNOWLEDGE: u8 = 1;
const STATUS_DRIVER: u8 = 2;
const STATUS_DRIVER_OK: u8 = 4;
const STATUS_FEATURES_OK: u8 = 8;
const STATUS_FAILED: u8 = 0x80;
const REQUIRED_FEATURES: u32 = 0b11;
const VRING_DESC_F_WRITE: u16 = 2;

#[repr(C, align(4096))]
struct QueuePage([u8; 4096]);

struct QueueCell(UnsafeCell<QueuePage>);
unsafe impl Sync for QueueCell {}

static KEYBOARD_QUEUE: QueueCell = QueueCell(UnsafeCell::new(QueuePage([0; 4096])));
static TABLET_QUEUE: QueueCell = QueueCell(UnsafeCell::new(QueuePage([0; 4096])));
static ACTIVE: AtomicBool = AtomicBool::new(false);
static FAULT_REPORTED: AtomicBool = AtomicBool::new(false);
static mut DEVICES: [Option<InputDevice>; 2] = [None, None];

#[derive(Clone, Copy)]
struct InputDevice {
    transport: pci::VirtioTransport,
    queue: *mut u8,
    used_consumer: u16,
}

unsafe impl Send for InputDevice {}

pub fn activate(platform: PlatformInfo) -> Result<(), ()> {
    let keyboard = activate_one(platform, 4, KEYBOARD_QUEUE.0.get(), smmu::KEYBOARD_QUEUE_IOVA)?;
    let tablet = activate_one(platform, 5, TABLET_QUEUE.0.get(), smmu::TABLET_QUEUE_IOVA)?;
    unsafe { DEVICES = [Some(keyboard), Some(tablet)] };
    ACTIVE.store(true, Ordering::Release);
    crate::kprintln!(
        "[input] virtio-keyboard+tablet ready streams=2 queues=2 dma-isolated=true polling=true"
    );
    Ok(())
}

pub fn poll() -> Option<InputEvent> {
    if !ACTIVE.load(Ordering::Acquire) {
        return None;
    }
    for index in 0..2 {
        let mut device = unsafe { DEVICES[index]? };
        let queue = unsafe { core::slice::from_raw_parts_mut(device.queue, 4096) };
        let used = get16(queue, USED_OFFSET + 2);
        if device.used_consumer == used {
            continue;
        }
        dma_barrier();
        let used_slot = (device.used_consumer % QUEUE_SIZE) as usize;
        let descriptor = get32(queue, USED_OFFSET + 4 + used_slot * 8) as u16;
        if descriptor >= QUEUE_SIZE {
            device.used_consumer = device.used_consumer.wrapping_add(1);
            unsafe { DEVICES[index] = Some(device) };
            continue;
        }
        let offset = EVENT_OFFSET + descriptor as usize * EVENT_BYTES;
        let event = InputEvent {
            event_type: get16(queue, offset),
            code: get16(queue, offset + 2),
            value: get32(queue, offset + 4) as i32,
        };
        device.used_consumer = device.used_consumer.wrapping_add(1);
        let available = get16(queue, AVAIL_OFFSET + 2);
        put16(
            queue,
            AVAIL_OFFSET + 4 + (available % QUEUE_SIZE) as usize * 2,
            descriptor,
        );
        dma_barrier();
        put16(queue, AVAIL_OFFSET + 2, available.wrapping_add(1));
        dma_barrier();
        let notify = read16(device.transport.common + 30) as usize
            * device.transport.notify_multiplier as usize;
        write16(device.transport.notify + notify, 0);
        unsafe { DEVICES[index] = Some(device) };
        return Some(event);
    }
    if !FAULT_REPORTED.load(Ordering::Acquire) {
        if let Some(fault) = smmu::take_fault() {
            crate::kprintln!(
                "[input] DMA fault event={:#x} stream-id={:#x} iova={:#x}",
                fault.event_type,
                fault.stream_id,
                fault.address
            );
            FAULT_REPORTED.store(true, Ordering::Release);
        }
    }
    None
}

fn activate_one(
    platform: PlatformInfo,
    slot: u8,
    queue: *mut QueuePage,
    iova: u64,
) -> Result<InputDevice, ()> {
    let device = pci::virtio_device_at(platform.pcie_base, slot, 0, pci::VIRTIO_INPUT_MODERN)
        .ok_or(())?;
    let transport = pci::inspect_configured_virtio(
        platform.pcie_base,
        device,
        platform.pcie_mmio_base,
        platform.pcie_mmio_bytes,
    )
    .map_err(|_| ())?;
    let stream_id = platform.stream_id(device.requester_id()).ok_or(())?;
    let physical = queue as usize as u64 - KERNEL_OFFSET as u64;
    smmu::map_gui_aux_page(iova, physical).map_err(|_| ())?;
    smmu::attach_stream(stream_id).map_err(|_| ())?;
    negotiate(transport)?;
    let bytes = unsafe { &mut (*queue).0 };
    bytes.fill(0);
    for descriptor in 0..QUEUE_SIZE {
        let offset = descriptor as usize * 16;
        put64(bytes, offset, iova + EVENT_OFFSET as u64 + descriptor as u64 * EVENT_BYTES as u64);
        put32(bytes, offset + 8, EVENT_BYTES as u32);
        put16(bytes, offset + 12, VRING_DESC_F_WRITE);
        put16(bytes, AVAIL_OFFSET + 4 + descriptor as usize * 2, descriptor);
    }
    dma_barrier();
    write16(transport.common + 22, 0);
    let maximum = read16(transport.common + 24);
    if maximum < QUEUE_SIZE {
        return Err(());
    }
    write16(transport.common + 24, QUEUE_SIZE);
    write64(transport.common + 32, iova + DESC_OFFSET as u64);
    write64(transport.common + 40, iova + AVAIL_OFFSET as u64);
    write64(transport.common + 48, iova + USED_OFFSET as u64);
    write16(transport.common + 28, 1);
    if read16(transport.common + 28) != 1 {
        return Err(());
    }
    write8(
        transport.common + 20,
        read8(transport.common + 20) | STATUS_DRIVER_OK,
    );
    put16(bytes, AVAIL_OFFSET + 2, QUEUE_SIZE);
    dma_barrier();
    let notify = read16(transport.common + 30) as usize * transport.notify_multiplier as usize;
    write16(transport.notify + notify, 0);
    Ok(InputDevice {
        transport,
        queue: queue.cast::<u8>(),
        used_consumer: 0,
    })
}

fn negotiate(transport: pci::VirtioTransport) -> Result<(), ()> {
    write8(transport.common + 20, 0);
    write8(transport.common + 20, STATUS_ACKNOWLEDGE | STATUS_DRIVER);
    write32(transport.common, 1);
    let high = read32(transport.common + 4);
    if high & REQUIRED_FEATURES != REQUIRED_FEATURES {
        write8(transport.common + 20, STATUS_FAILED);
        return Err(());
    }
    write32(transport.common + 8, 0);
    write32(transport.common + 12, 0);
    write32(transport.common + 8, 1);
    write32(transport.common + 12, REQUIRED_FEATURES);
    write8(
        transport.common + 20,
        read8(transport.common + 20) | STATUS_FEATURES_OK,
    );
    (read8(transport.common + 20) & STATUS_FEATURES_OK != 0)
        .then_some(())
        .ok_or(())
}

fn read8(address: usize) -> u8 { unsafe { core::ptr::read_volatile(address as *const u8) } }
fn read16(address: usize) -> u16 { unsafe { core::ptr::read_volatile(address as *const u16) } }
fn read32(address: usize) -> u32 { unsafe { core::ptr::read_volatile(address as *const u32) } }
fn write8(address: usize, value: u8) { unsafe { core::ptr::write_volatile(address as *mut u8, value) } }
fn write16(address: usize, value: u16) { unsafe { core::ptr::write_volatile(address as *mut u16, value) } }
fn write32(address: usize, value: u32) { unsafe { core::ptr::write_volatile(address as *mut u32, value) } }
fn write64(address: usize, value: u64) { write32(address, value as u32); write32(address + 4, (value >> 32) as u32); }
fn get16(bytes: &[u8], offset: usize) -> u16 { u16::from_le_bytes(bytes[offset..offset + 2].try_into().unwrap()) }
fn get32(bytes: &[u8], offset: usize) -> u32 { u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap()) }
fn put16(bytes: &mut [u8], offset: usize, value: u16) { bytes[offset..offset + 2].copy_from_slice(&value.to_le_bytes()); }
fn put32(bytes: &mut [u8], offset: usize, value: u32) { bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes()); }
fn put64(bytes: &mut [u8], offset: usize, value: u64) { bytes[offset..offset + 8].copy_from_slice(&value.to_le_bytes()); }
fn dma_barrier() { unsafe { core::arch::asm!("dmb osh", options(nostack, preserves_flags)); } }
