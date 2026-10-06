#![no_std]
#![no_main]

use core::panic::PanicInfo;
use core::sync::atomic::{AtomicBool, Ordering};
use microsystem_abi::{CapHandle, Message, Status, boot_cap, protocol};
use microsystem_block::Operation;

const COMMON_PAGE: usize = 0x005a_0000;
const DEVICE_PAGE: usize = 0x005a_1000;
const NOTIFY_PAGE: usize = 0x005a_2000;
const ISR_PAGE: usize = 0x005a_3000;
const PCI_CONFIG_PAGE: usize = 0x005e_0000;
const DMA_IOVA: u64 = 0x0010_0000;
const PAGE_MASK: usize = 0xfff;
const VIRTIO_BLOCK_ID: u32 = 0x1042_1af4;
const VIRTIO_GPU_ID: u32 = 0x1050_1af4;
const VIRTIO_INPUT_ID: u32 = 0x1052_1af4;
const VIRTIO_NET_ID: u32 = 0x1041_1af4;
const VIRTIO_RNG_ID: u32 = 0x1044_1af4;
const PCI_COMMAND_MEMORY_BUS_MASTER: u16 = (1 << 1) | (1 << 2);
const PCI_STATUS_CAPABILITIES: u16 = 1 << 4;
const PCI_CAP_VENDOR: u8 = 0x09;
const FEATURE_FLUSH: u32 = 1 << 9;
const FEATURES_HIGH_REQUIRED: u32 = 0b11;
const STATUS_FEATURES_OK: u8 = 1 << 3;
static GUI_PRESENT: AtomicBool = AtomicBool::new(false);
static REUSE_TRANSPORT: AtomicBool = AtomicBool::new(false);

#[unsafe(no_mangle)]
pub extern "C" fn _start(
    mmio_base: u64,
    mmio_bytes: u64,
    ecam_scan: u64,
    reuse: u64,
    gui: u64,
) -> ! {
    REUSE_TRANSPORT.store(reuse != 0, Ordering::Release);
    GUI_PRESENT.store(gui != 0, Ordering::Release);
    let _ = microsystem_user_rt::debug_write(b"[user] devmgr service ELF entered EL0\n");
    let mut request = Message::new(protocol::BLOCK, 0);
    if microsystem_user_rt::ipc_recv(boot_cap::DEVMGR_ENDPOINT, &mut request, 0).is_err() {
        microsystem_user_rt::exit(2);
    }
    let mut first = true;
    loop {
        let mut reply = Message::new(protocol::BLOCK, Operation::Configure as u16);
        reply.words[5] = forward_configuration(
            &mut request,
            first.then_some((mmio_base, mmio_bytes, ecam_scan)),
        ) as i64 as u64;
        reply.words[0] = GUI_PRESENT.load(Ordering::Acquire) as u64;
        if first && reply.words[5] as i64 == Status::Ok as i64 {
            let _ = microsystem_user_rt::debug_write(
                b"[devmgr] resident EL0 accepted root caps=4 forwarded block caps=4\n",
            );
            let _ = microsystem_user_rt::service_online();
            first = false;
        }
        if microsystem_user_rt::ipc_reply_recv(boot_cap::DEVMGR_ENDPOINT, &reply, &mut request, 0)
            .is_err()
        {
            microsystem_user_rt::exit(3);
        }
    }
}

fn forward_configuration(request: &mut Message, platform: Option<(u64, u64, u64)>) -> Status {
    if request.protocol != protocol::BLOCK
        || request.opcode != Operation::Configure as u16
        || request.caps.contains(&CapHandle::INVALID)
    {
        return Status::Invalid;
    }
    if let Some((mmio_base, mmio_bytes, ecam_scan)) =
        platform.filter(|_| !REUSE_TRANSPORT.load(Ordering::Acquire))
    {
        let Ok((slot, function)) = discover_and_configure(mmio_base, mmio_bytes, ecam_scan) else {
            return Status::NotFound;
        };
        let _ = microsystem_user_rt::debug_write(
            b"[devmgr] resident EL0 staged PCI ECAM discovery and BAR assignment\n",
        );
        if microsystem_user_rt::system_activate_pci(boot_cap::DEVICE_SYSTEM_CONTROL, slot, function)
            .is_err()
        {
            return Status::Io;
        }
        if configure_pci_function().is_err() {
            return Status::Invalid;
        }
        let Ok(words) = transport_words() else {
            return Status::Invalid;
        };
        request.words = words;
        let _ = microsystem_user_rt::debug_write(
            b"[devmgr] resident EL0 discovered PCI ECAM function and assigned BARs\n",
        );
    }
    if request.words == [0; 6] {
        let Ok(words) = transport_words() else {
            return Status::Invalid;
        };
        request.words = words;
    }
    if request.words[..=5].contains(&0) {
        return Status::Invalid;
    }
    // Stopping block disables PCI bus mastering. A reused BAR layout still
    // needs its command register restored before restarting DMA queues.
    if configure_pci_function().is_err() {
        return Status::Invalid;
    }
    if validate_transport_layout(request).is_err() || negotiate_transport(request).is_err() {
        return Status::Invalid;
    }
    let _ = microsystem_user_rt::debug_write(
        b"[devmgr] resident EL0 negotiated modern transport features=VERSION_1+ACCESS_PLATFORM+FLUSH queues-positive=true capacity-positive=true\n",
    );
    let _ = microsystem_user_rt::debug_write(
        b"[devmgr] resident EL0 configured PCI function vendor=1af4 device=1042 command=memory+bus-master caps=common+notify+isr+device\n",
    );
    let mut block_reply = Message::new(protocol::BLOCK, 0);
    match microsystem_user_rt::ipc_call(
        boot_cap::BLOCK_CONFIG_ENDPOINT,
        request,
        &mut block_reply,
        0,
    ) {
        Ok(()) if block_reply.protocol == protocol::BLOCK => {
            status_from_wire(block_reply.words[5] as i64)
        }
        Ok(()) => Status::Invalid,
        Err(status) => status,
    }
}

fn discover_and_configure(mmio_base: u64, mmio_bytes: u64, ecam_scan: u64) -> Result<(u8, u8), ()> {
    if mmio_base == 0 || mmio_bytes == 0 || ecam_scan == 0 {
        return Err(());
    }
    let window_end = mmio_base.checked_add(mmio_bytes).ok_or(())?;
    let mut next = mmio_base;
    let mut block = None;
    let mut gui_devices = 0u8;
    for slot in 0..32u8 {
        let config = ecam_scan as usize + slot as usize * 4096;
        let id = read32(config);
        if !matches!(
            id,
            VIRTIO_BLOCK_ID | VIRTIO_GPU_ID | VIRTIO_INPUT_ID | VIRTIO_NET_ID | VIRTIO_RNG_ID
        ) {
            continue;
        }
        configure_bars(config, &mut next, window_end)?;
        let command = read16(config + 4) | PCI_COMMAND_MEMORY_BUS_MASTER;
        write16(config + 4, command);
        if matches!(id, VIRTIO_GPU_ID | VIRTIO_INPUT_ID) {
            gui_devices = gui_devices.saturating_add(1);
        }
        if id == VIRTIO_BLOCK_ID {
            block = Some((slot, 0));
        }
    }
    if gui_devices == 3 {
        GUI_PRESENT.store(true, Ordering::Release);
        let _ = microsystem_user_rt::debug_write(
            b"[devmgr] GUI PCI devices assigned block+gpu+keyboard+tablet=true\n",
        );
    }
    block.ok_or(())
}

fn configure_bars(config: usize, next: &mut u64, window_end: u64) -> Result<(), ()> {
    let mut index = 0usize;
    while index < 6 {
        let offset = 0x10 + index * 4;
        let original = read32(config + offset);
        if original & 1 != 0 {
            index += 1;
            continue;
        }
        write32(config + offset, u32::MAX);
        let low_mask = read32(config + offset);
        let is_64 = original & 0x6 == 0x4 && index + 1 < 6;
        let (size, flags) = if is_64 {
            let upper_original = read32(config + offset + 4);
            write32(config + offset + 4, u32::MAX);
            let upper_mask = read32(config + offset + 4);
            write32(config + offset + 4, upper_original);
            let mask = ((upper_mask as u64) << 32) | (low_mask as u64 & !0xf);
            ((!mask).wrapping_add(1), original & 0xf)
        } else {
            let mask = low_mask & !0xf;
            ((!mask).wrapping_add(1) as u64, original & 0xf)
        };
        if size == 0 {
            write32(config + offset, original);
            index += if is_64 { 2 } else { 1 };
            continue;
        }
        *next = align_up(*next, size).ok_or(())?;
        if next
            .checked_add(size)
            .filter(|end| *end <= window_end)
            .is_none()
        {
            return Err(());
        }
        write32(config + offset, *next as u32 | flags);
        if is_64 {
            write32(config + offset + 4, (*next >> 32) as u32);
        }
        *next += size;
        index += if is_64 { 2 } else { 1 };
    }
    Ok(())
}

fn align_up(value: u64, alignment: u64) -> Option<u64> {
    if alignment == 0 || !alignment.is_power_of_two() {
        return None;
    }
    value
        .checked_add(alignment - 1)
        .map(|value| value & !(alignment - 1))
}

fn transport_words() -> Result<[u64; 6], ()> {
    let mut offsets = [None; 4];
    let mut notify_multiplier = 0u32;
    let mut capability = read8(PCI_CONFIG_PAGE + 0x34) & !3;
    for _ in 0..48 {
        if capability < 0x40 {
            break;
        }
        let address = PCI_CONFIG_PAGE + capability as usize;
        let id = read8(address);
        let next = read8(address + 1) & !3;
        if id == PCI_CAP_VENDOR {
            let length = read8(address + 2);
            let kind = read8(address + 3);
            if matches!(kind, 1..=4) && length >= 16 {
                offsets[kind as usize - 1] = Some(read32(address + 8) as u64 & PAGE_MASK as u64);
                if kind == 2 && length >= 20 {
                    notify_multiplier = read32(address + 16);
                }
            }
        }
        if next == 0 || next == capability {
            break;
        }
        capability = next;
    }
    Ok([
        COMMON_PAGE as u64 + offsets[0].ok_or(())?,
        NOTIFY_PAGE as u64 + offsets[1].ok_or(())?,
        ISR_PAGE as u64 + offsets[2].ok_or(())?,
        DEVICE_PAGE as u64 + offsets[3].ok_or(())?,
        notify_multiplier as u64,
        DMA_IOVA,
    ])
}

fn configure_pci_function() -> Result<(), ()> {
    if read32(PCI_CONFIG_PAGE) != VIRTIO_BLOCK_ID
        || read16(PCI_CONFIG_PAGE + 6) & PCI_STATUS_CAPABILITIES == 0
    {
        return Err(());
    }
    let mut kinds = 0u8;
    let mut capability = read8(PCI_CONFIG_PAGE + 0x34) & !3;
    for _ in 0..48 {
        if capability < 0x40 {
            break;
        }
        let address = PCI_CONFIG_PAGE + capability as usize;
        let id = read8(address);
        let next = read8(address + 1) & !3;
        if id == PCI_CAP_VENDOR {
            let length = read8(address + 2);
            let kind = read8(address + 3);
            let bar = read8(address + 4);
            if matches!(kind, 1..=4) && length >= 16 && bar < 6 {
                kinds |= 1 << (kind - 1);
            }
        }
        if next == 0 || next == capability {
            break;
        }
        capability = next;
    }
    if kinds != 0b1111 {
        return Err(());
    }
    let command = read16(PCI_CONFIG_PAGE + 4) | PCI_COMMAND_MEMORY_BUS_MASTER;
    write16(PCI_CONFIG_PAGE + 4, command);
    if read16(PCI_CONFIG_PAGE + 4) & PCI_COMMAND_MEMORY_BUS_MASTER == PCI_COMMAND_MEMORY_BUS_MASTER
    {
        Ok(())
    } else {
        Err(())
    }
}

fn validate_transport_layout(request: &Message) -> Result<(), ()> {
    let common = request.words[0] as usize;
    let notify = request.words[1] as usize;
    let isr = request.words[2] as usize;
    let device = request.words[3] as usize;
    if common & !PAGE_MASK != COMMON_PAGE
        || device & !PAGE_MASK != DEVICE_PAGE
        || notify & !PAGE_MASK != NOTIFY_PAGE
        || isr & !PAGE_MASK != ISR_PAGE
        || common & PAGE_MASK > PAGE_MASK - 20
        || device & PAGE_MASK > PAGE_MASK - 7
        || request.words[4] == 0
        || request.words[5] & PAGE_MASK as u64 != 0
    {
        return Err(());
    }
    Ok(())
}

fn negotiate_transport(request: &Message) -> Result<(), ()> {
    let common = request.words[0] as usize;
    let device = request.words[3] as usize;
    write8(common + 20, 0);
    write8(common + 20, 1);
    write8(common + 20, 1 | 2);
    write32(common, 0);
    let low = read32(common + 4);
    write32(common, 1);
    let high = read32(common + 4);
    if low & FEATURE_FLUSH == 0 || high & FEATURES_HIGH_REQUIRED != FEATURES_HIGH_REQUIRED {
        write8(common + 20, read8(common + 20) | 0x80);
        return Err(());
    }
    write32(common + 8, 0);
    write32(common + 12, FEATURE_FLUSH);
    write32(common + 8, 1);
    write32(common + 12, FEATURES_HIGH_REQUIRED);
    write8(common + 20, read8(common + 20) | STATUS_FEATURES_OK);
    if read8(common + 20) & STATUS_FEATURES_OK == 0
        || read16(common + 18) == 0
        || read64(device) == 0
    {
        return Err(());
    }
    Ok(())
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

fn read64(address: usize) -> u64 {
    unsafe { core::ptr::read_volatile(address as *const u64) }
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

fn status_from_wire(value: i64) -> Status {
    match value {
        0 => Status::Ok,
        -1 => Status::Invalid,
        -2 => Status::BadCapability,
        -3 => Status::AccessDenied,
        -4 => Status::NotFound,
        -5 => Status::NoMemory,
        -6 => Status::Busy,
        -7 => Status::TimedOut,
        -8 => Status::Fault,
        -9 => Status::NotSupported,
        -10 => Status::Io,
        -11 => Status::NoSpace,
        -12 => Status::Corrupt,
        _ => Status::Invalid,
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo<'_>) -> ! {
    microsystem_user_rt::exit(1)
}
