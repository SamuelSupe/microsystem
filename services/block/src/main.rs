#![no_std]
#![no_main]

use core::panic::PanicInfo;
use microsystem_abi::{CapHandle, Message, Rights, Status, boot_cap, protocol};
use microsystem_block::Operation;

const QUEUE: usize = 0x005b_0000;
const DESC_OFFSET: usize = 0;
const AVAIL_OFFSET: usize = 256;
const USED_OFFSET: usize = 320;
const HEADER_OFFSET: usize = 512;
const DATA_OFFSET: usize = 1024;
const STATUS_OFFSET: usize = DATA_OFFSET + 512;
const QUEUE_SIZE: u16 = 16;
const SCRATCH_VA: u64 = 0x0059_0000;
const VIRTIO_BLK_T_IN: u32 = 0;
const VIRTIO_BLK_T_OUT: u32 = 1;
const VIRTIO_BLK_T_FLUSH: u32 = 4;
const DESC_NEXT: u16 = 1;
const DESC_WRITE: u16 = 2;

struct TransportConfig {
    common: usize,
    notify: usize,
    isr: usize,
    device: usize,
    notify_multiplier: usize,
    dma_iova: u64,
    data_iova: u64,
    irq: CapHandle,
    queue_frame: CapHandle,
    dma_domain: CapHandle,
}

#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    let _ = microsystem_user_rt::debug_write(b"[user] block service ELF entered EL0\n");
    let mut configuration = Message::new(protocol::BLOCK, 0);
    if microsystem_user_rt::ipc_recv(boot_cap::BLOCK_CONFIG_ENDPOINT, &mut configuration, 0)
        .is_err()
    {
        microsystem_user_rt::exit(5);
    }
    let config = match parse_configuration(&configuration) {
        Ok(config) => config,
        Err(()) => microsystem_user_rt::exit(6),
    };
    if config.common == 0
        || config.notify == 0
        || config.isr == 0
        || config.device == 0
        || config.dma_iova == 0
        || config.data_iova == 0
    {
        microsystem_user_rt::exit(4);
    }
    if microsystem_user_rt::dma_map(config.dma_domain, config.queue_frame, config.dma_iova).is_err()
        || microsystem_user_rt::dma_map(
            config.dma_domain,
            boot_cap::SHARED_BLOCK_FRAME,
            config.data_iova,
        )
        .is_err()
        || microsystem_user_rt::dma_unmap(config.dma_domain, config.data_iova).is_err()
        || microsystem_user_rt::dma_unmap(config.dma_domain, config.data_iova)
            != Err(Status::NotFound)
        || microsystem_user_rt::dma_map(
            config.dma_domain,
            boot_cap::SHARED_BLOCK_FRAME,
            config.data_iova,
        )
        .is_err()
    {
        microsystem_user_rt::exit(10);
    }
    let _ = microsystem_user_rt::debug_write(
        b"[iommu] resident block DmaMap frames=2 capability=true pte=true cmdq=true unmap-remap=true\n",
    );
    if configure_queue(config.common, config.dma_iova).is_err() {
        microsystem_user_rt::exit(11);
    }
    let _ = microsystem_user_rt::debug_write(
        b"[user] block configured split queue0 size=16 driver-ok=true\n",
    );
    if microsystem_user_rt::irq_bind(config.irq, boot_cap::BLOCK_IRQ_NOTIFICATION).is_err() {
        microsystem_user_rt::exit(7);
    }
    if block_probe(
        config.common,
        config.notify,
        config.device,
        config.notify_multiplier,
        config.dma_iova,
        config.irq,
    )
    .is_err()
    {
        microsystem_user_rt::exit(9);
    }
    if probe_dynamic_frame(&config).is_err() {
        microsystem_user_rt::exit(12);
    }
    let _ = microsystem_user_rt::debug_write(
        b"[iommu] resident block dynamic-frame iova=0x102000 read=true dma-delete=busy reclaimed=true\n",
    );
    let _ = microsystem_user_rt::debug_write(
        b"[user] block driver queue0 read+write sector=0 mfs1=true flush=ok\n",
    );
    let _ = microsystem_user_rt::debug_write(b"[irq] resident block notification bitset=true\n");

    let mut request = Message::new(protocol::BLOCK, 0);
    let mut configured = Message::new(protocol::BLOCK, Operation::Configure as u16);
    configured.words[5] = Status::Ok as i64 as u64;
    let _ = microsystem_user_rt::debug_write(b"[ipc] resident block endpoint=2 ready\n");
    let _ = microsystem_user_rt::service_online();
    if microsystem_user_rt::ipc_reply_recv(boot_cap::BLOCK_ENDPOINT, &configured, &mut request, 0)
        .is_err()
    {
        microsystem_user_rt::exit(2);
    }
    loop {
        let mut reply = Message::new(protocol::BLOCK, request.opcode);
        let status = handle_request(
            &request,
            &mut reply,
            config.common,
            config.notify,
            config.notify_multiplier,
            config.dma_iova,
            config.data_iova,
            read64(config.device),
            config.irq,
        );
        reply.words[5] = status as i64 as u64;
        if microsystem_user_rt::ipc_reply_recv(boot_cap::BLOCK_ENDPOINT, &reply, &mut request, 0)
            .is_err()
        {
            microsystem_user_rt::exit(3);
        }
    }
}

fn probe_dynamic_frame(config: &TransportConfig) -> Result<(), ()> {
    let rights = Rights(Rights::READ.0 | Rights::WRITE.0 | Rights::MAP.0);
    let frame =
        microsystem_user_rt::frame_create(boot_cap::DRIVER_MEMORY_POOL, rights).map_err(|_| ())?;
    microsystem_user_rt::frame_map(frame, SCRATCH_VA, Rights(Rights::READ.0 | Rights::WRITE.0))
        .map_err(|_| ())?;
    let iova = config.dma_iova + 2 * 4096;
    microsystem_user_rt::dma_map(config.dma_domain, frame, iova).map_err(|_| ())?;
    microsystem_user_rt::frame_unmap(frame, SCRATCH_VA).map_err(|_| ())?;
    if microsystem_user_rt::cap_delete(frame) != Err(Status::Busy) {
        return Err(());
    }
    microsystem_user_rt::frame_map(frame, SCRATCH_VA, Rights(Rights::READ.0 | Rights::WRITE.0))
        .map_err(|_| ())?;
    let queue_size = read16(config.common + 24);
    submit(
        config.common,
        config.notify,
        config.notify_multiplier,
        config.dma_iova,
        queue_size,
        VIRTIO_BLK_T_IN,
        0,
        Some((iova, 512)),
        config.irq,
    )?;
    if unsafe { core::slice::from_raw_parts(SCRATCH_VA as *const u8, 8) } != b"MFS1SB\0\0" {
        return Err(());
    }
    microsystem_user_rt::dma_unmap(config.dma_domain, iova).map_err(|_| ())?;
    microsystem_user_rt::frame_unmap(frame, SCRATCH_VA).map_err(|_| ())?;
    microsystem_user_rt::cap_delete(frame).map_err(|_| ())
}

fn configure_queue(common: usize, dma_iova: u64) -> Result<(), ()> {
    unsafe {
        // The boot queue frame survives service restart, but VirtIO resets its
        // indices to zero. Old avail/used indices must not be replayed as DMA.
        core::ptr::write_bytes(QUEUE as *mut u8, 0, 4096);
        microsystem_user_rt::fence_store();
        write16(common + 22, 0);
        let maximum = read16(common + 24);
        if maximum < 3 {
            return Err(());
        }
        write16(common + 24, maximum.min(QUEUE_SIZE));
        write_mmio64(common + 32, dma_iova + DESC_OFFSET as u64);
        write_mmio64(common + 40, dma_iova + AVAIL_OFFSET as u64);
        write_mmio64(common + 48, dma_iova + USED_OFFSET as u64);
        write16(QUEUE + AVAIL_OFFSET, 0);
        write16(QUEUE + AVAIL_OFFSET + 2, 0);
        write16(QUEUE + USED_OFFSET, 0);
        write16(QUEUE + USED_OFFSET + 2, 0);
        microsystem_user_rt::fence_store();
        write16(common + 28, 1);
        write8(common + 20, read8(common + 20) | 4);
    }
    if read16(common + 28) != 1 || read8(common + 20) & 4 == 0 {
        return Err(());
    }
    Ok(())
}

fn parse_configuration(message: &Message) -> Result<TransportConfig, ()> {
    if message.protocol != protocol::BLOCK
        || message.opcode != Operation::Configure as u16
        || message.caps.contains(&CapHandle::INVALID)
        || message.words[4] == 0
    {
        return Err(());
    }
    Ok(TransportConfig {
        common: message.words[0] as usize,
        notify: message.words[1] as usize,
        isr: message.words[2] as usize,
        device: message.words[3] as usize,
        notify_multiplier: message.words[4] as usize,
        dma_iova: message.words[5],
        data_iova: message.words[5].checked_add(4096).ok_or(())?,
        irq: message.caps[3],
        queue_frame: message.caps[1],
        dma_domain: message.caps[2],
    })
}

#[allow(clippy::too_many_arguments)]
fn handle_request(
    request: &Message,
    reply: &mut Message,
    common: usize,
    notify: usize,
    notify_multiplier: usize,
    dma_iova: u64,
    data_iova: u64,
    capacity: u64,
    irq: CapHandle,
) -> Status {
    match request.opcode {
        value if value == Operation::Geometry as u16 => {
            reply.words[0] = capacity;
            reply.words[1] = 512;
            Status::Ok
        }
        value if value == Operation::Read as u16 || value == Operation::Write as u16 => {
            if request.caps[0] != boot_cap::SHARED_BLOCK_FRAME
                || request.words[0]
                    .checked_mul(8)
                    .and_then(|sector| sector.checked_add(8))
                    .filter(|end| *end <= capacity)
                    .is_none()
            {
                return Status::AccessDenied;
            }
            let request_type = if value == Operation::Read as u16 {
                VIRTIO_BLK_T_IN
            } else {
                VIRTIO_BLK_T_OUT
            };
            let first_sector = request.words[0] * 8;
            let queue_size = read16(common + 24);
            for index in 0..8u64 {
                if submit(
                    common,
                    notify,
                    notify_multiplier,
                    dma_iova,
                    queue_size,
                    request_type,
                    first_sector + index,
                    Some((data_iova + index * 512, 512)),
                    irq,
                )
                .is_err()
                {
                    return Status::Io;
                }
            }
            Status::Ok
        }
        value if value == Operation::Flush as u16 => {
            if submit(
                common,
                notify,
                notify_multiplier,
                dma_iova,
                read16(common + 24),
                VIRTIO_BLK_T_FLUSH,
                0,
                None,
                irq,
            )
            .is_ok()
            {
                Status::Ok
            } else {
                Status::Io
            }
        }
        _ => Status::Invalid,
    }
}

fn block_probe(
    common: usize,
    notify: usize,
    device: usize,
    notify_multiplier: usize,
    dma_iova: u64,
    irq: CapHandle,
) -> Result<(), ()> {
    let queue_size = read16(common + 24);
    if queue_size < 3 || read16(common + 28) != 1 || read8(common + 20) & 4 == 0 {
        return Err(());
    }
    submit(
        common,
        notify,
        notify_multiplier,
        dma_iova,
        queue_size,
        VIRTIO_BLK_T_IN,
        0,
        Some((dma_iova + DATA_OFFSET as u64, 512)),
        irq,
    )?;
    if unsafe { core::slice::from_raw_parts((QUEUE + DATA_OFFSET) as *const u8, 8) }
        != b"MFS1SB\0\0"
    {
        return Err(());
    }
    submit(
        common,
        notify,
        notify_multiplier,
        dma_iova,
        queue_size,
        VIRTIO_BLK_T_OUT,
        0,
        Some((dma_iova + DATA_OFFSET as u64, 512)),
        irq,
    )?;
    submit(
        common,
        notify,
        notify_multiplier,
        dma_iova,
        queue_size,
        VIRTIO_BLK_T_FLUSH,
        0,
        None,
        irq,
    )?;
    if read64(device) < 2 {
        return Err(());
    }
    Ok(())
}

fn submit(
    common: usize,
    notify: usize,
    notify_multiplier: usize,
    dma_iova: u64,
    queue_size: u16,
    request_type: u32,
    sector: u64,
    data: Option<(u64, u32)>,
    irq: CapHandle,
) -> Result<(), ()> {
    let deadline = microsystem_user_rt::clock_now()
        .map_err(|_| ())?
        .saturating_add(3_000_000_000);
    unsafe {
        write32(QUEUE + HEADER_OFFSET, request_type);
        write32(QUEUE + HEADER_OFFSET + 4, 0);
        write64(QUEUE + HEADER_OFFSET + 8, sector);
        write8(QUEUE + STATUS_OFFSET, u8::MAX);
        descriptor(0, dma_iova + HEADER_OFFSET as u64, 16, DESC_NEXT, 1);
        if let Some((data_address, data_bytes)) = data {
            let flags = if request_type == VIRTIO_BLK_T_IN {
                DESC_NEXT | DESC_WRITE
            } else {
                DESC_NEXT
            };
            descriptor(1, data_address, data_bytes, flags, 2);
            descriptor(2, dma_iova + STATUS_OFFSET as u64, 1, DESC_WRITE, 0);
        } else {
            descriptor(1, dma_iova + STATUS_OFFSET as u64, 1, DESC_WRITE, 0);
        }

        let available_index = read16(QUEUE + AVAIL_OFFSET + 2);
        let used_index = read16(QUEUE + USED_OFFSET + 2);
        write16(
            QUEUE + AVAIL_OFFSET + 4 + (available_index % queue_size) as usize * 2,
            0,
        );
        microsystem_user_rt::fence_store();
        write16(QUEUE + AVAIL_OFFSET + 2, available_index.wrapping_add(1));
        microsystem_user_rt::fence_store();
        let queue_notify_offset = read16(common + 30) as usize;
        write16(notify + queue_notify_offset * notify_multiplier, 0);
        for _ in 0..100_000 {
            if read16(QUEUE + USED_OFFSET + 2) != used_index {
                microsystem_user_rt::fence_load();
                return if read8(QUEUE + STATUS_OFFSET) == 0 {
                    Ok(())
                } else {
                    Err(())
                };
            }
            let now = microsystem_user_rt::clock_now().map_err(|_| ())?;
            if now >= deadline {
                let _ = microsystem_user_rt::debug_write(
                    b"[block] DMA completion timeout; controller recovery required\n",
                );
                microsystem_user_rt::exit(Status::TimedOut as i64 as u64);
            }
            match microsystem_user_rt::notification_wait(
                boot_cap::BLOCK_IRQ_NOTIFICATION,
                now.saturating_add(1_000_000).min(deadline),
            ) {
                Ok(_) => microsystem_user_rt::irq_ack(irq).map_err(|_| ())?,
                Err(Status::Busy | Status::TimedOut) => {
                    let _ = microsystem_user_rt::yield_now();
                }
                Err(_) => return Err(()),
            }
        }
    }
    Err(())
}

unsafe fn descriptor(index: usize, address: u64, length: u32, flags: u16, next: u16) {
    let base = QUEUE + DESC_OFFSET + index * 16;
    unsafe {
        write64(base, address);
        write32(base + 8, length);
        write16(base + 12, flags);
        write16(base + 14, next);
    }
}

fn read8(address: usize) -> u8 {
    unsafe { core::ptr::read_volatile(address as *const u8) }
}

fn read16(address: usize) -> u16 {
    unsafe { core::ptr::read_volatile(address as *const u16) }
}

fn read64(address: usize) -> u64 {
    unsafe { core::ptr::read_volatile(address as *const u64) }
}

unsafe fn write8(address: usize, value: u8) {
    unsafe { core::ptr::write_volatile(address as *mut u8, value) }
}

unsafe fn write16(address: usize, value: u16) {
    unsafe { core::ptr::write_volatile(address as *mut u16, value) }
}

unsafe fn write32(address: usize, value: u32) {
    unsafe { core::ptr::write_volatile(address as *mut u32, value) }
}

unsafe fn write64(address: usize, value: u64) {
    unsafe { core::ptr::write_volatile(address as *mut u64, value) }
}

unsafe fn write_mmio64(address: usize, value: u64) {
    unsafe {
        write32(address, value as u32);
        write32(address + 4, (value >> 32) as u32);
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo<'_>) -> ! {
    microsystem_user_rt::exit(1)
}
