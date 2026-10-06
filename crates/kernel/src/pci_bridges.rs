use crate::pci::{self, Device, VirtioError};
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

pub const MAX_BUSES: usize = 8;
static START: [AtomicU64; MAX_BUSES] = [const { AtomicU64::new(0) }; MAX_BUSES];
static END: [AtomicU64; MAX_BUSES] = [const { AtomicU64::new(0) }; MAX_BUSES];
static NEXT: [AtomicU64; MAX_BUSES] = [const { AtomicU64::new(0) }; MAX_BUSES];
static PORT: [AtomicU32; MAX_BUSES] = [const { AtomicU32::new(0) }; MAX_BUSES];
static EJECT: [AtomicBool; MAX_BUSES] = [const { AtomicBool::new(false) }; MAX_BUSES];

pub fn active(bus: u8) -> bool {
    bus == 0
        || END
            .get(bus as usize)
            .is_some_and(|end| end.load(Ordering::Acquire) != 0)
}
pub fn reset(base: u64, end: u64, next: u64) {
    for bus in 1..MAX_BUSES {
        START[bus].store(0, Ordering::Release);
        END[bus].store(0, Ordering::Release);
        NEXT[bus].store(0, Ordering::Release);
        PORT[bus].store(0, Ordering::Release);
        EJECT[bus].store(false, Ordering::Release);
    }
    START[0].store(base, Ordering::Release);
    END[0].store(end, Ordering::Release);
    NEXT[0].store(next, Ordering::Release);
}
pub fn allocation(bus: u8) -> Result<(u64, u64), VirtioError> {
    let index = bus as usize;
    if index >= MAX_BUSES || !active(bus) {
        return Err(VirtioError::MissingWindow);
    }
    Ok((
        NEXT[index].load(Ordering::Acquire),
        END[index].load(Ordering::Acquire),
    ))
}
pub fn advance(bus: u8, next: u64) {
    NEXT[bus as usize].store(next, Ordering::Release);
}

pub fn prepare(ecam: usize) -> Result<(), VirtioError> {
    for parent in 0..MAX_BUSES {
        if !active(parent as u8) {
            continue;
        }
        let config = ecam + (parent << 20);
        for slot in 0..32 {
            if pci::read32(config, slot, 0, 8) >> 16 != 0x0604 {
                continue;
            }
            let command = pci::read16(config, slot, 0, 4);
            let numbers = pci::read32(config, slot, 0, 0x18);
            let assigned = ((numbers >> 8) & 255) as usize;
            let child = if assigned != 0 && assigned < MAX_BUSES && !active(assigned as u8) {
                assigned
            } else {
                (1..MAX_BUSES)
                    .find(|bus| !active(*bus as u8))
                    .ok_or(VirtioError::MissingWindow)?
            };
            let memory = pci::read32(config, slot, 0, 0x20);
            let existing_start = u64::from(memory & 0xfff0) << 16;
            let existing_end = (u64::from((memory >> 16) & 0xfff0) << 16) + 0x10_0000;
            let (cursor, parent_end) = allocation(parent as u8)?;
            let parent_start = START[parent].load(Ordering::Acquire);
            let (start, end) = if assigned != 0
                && existing_start >= parent_start
                && existing_start < existing_end
                && existing_end <= parent_end
            {
                (existing_start, existing_end)
            } else {
                let size = if parent == 0 {
                    32 * 1024 * 1024
                } else {
                    8 * 1024 * 1024
                };
                let start = cursor
                    .checked_add(size - 1)
                    .map(|cursor| cursor & !(size - 1))
                    .ok_or(VirtioError::MissingWindow)?;
                let end = start
                    .checked_add(size)
                    .filter(|end| *end <= parent_end)
                    .ok_or(VirtioError::MissingWindow)?;
                (start, end)
            };
            pci::write16(config, slot, 0, 4, command & !7);
            pci::write32(
                config,
                slot,
                0,
                0x18,
                numbers & 0xff00_0000 | parent as u32 | (child as u32) << 8 | (child as u32) << 16,
            );
            let window =
                ((start >> 16) as u32 & 0xfff0) | (((end - 1) >> 16) as u32 & 0xfff0) << 16;
            pci::write32(config, slot, 0, 0x20, window);
            pci::write32(config, slot, 0, 0x24, window);
            pci::write32(config, slot, 0, 0x28, 0);
            pci::write32(config, slot, 0, 0x2c, 0);
            pci::write16(config, slot, 0, 4, command | 6);
            START[child].store(start, Ordering::Release);
            END[child].store(end, Ordering::Release);
            NEXT[child].store(start, Ordering::Release);
            PORT[child].store(
                1 | (parent as u32) << 8 | (slot as u32) << 16,
                Ordering::Release,
            );
            // Every upstream bridge must forward the newly assigned bus number.
            let mut ancestor = parent;
            for _ in 0..MAX_BUSES {
                if ancestor == 0 {
                    break;
                }
                let port = PORT[ancestor].load(Ordering::Acquire);
                let upstream = ((port >> 8) & 255) as usize;
                let slot = ((port >> 16) & 255) as u8;
                let config = ecam + (upstream << 20);
                let numbers = pci::read32(config, slot, 0, 0x18);
                if ((numbers >> 16) & 255) < child as u32 {
                    pci::write32(
                        config,
                        slot,
                        0,
                        0x18,
                        numbers & !0x00ff_0000 | (child as u32) << 16,
                    );
                }
                ancestor = upstream;
            }
            advance(parent as u8, cursor.max(end));
            for device_slot in 0..32 {
                if pci::read32(ecam + (child << 20), device_slot, 0, 0) as u16 == 0xffff {
                    continue;
                }
                let device = Device {
                    bus: child as u8,
                    slot: device_slot,
                    function: 0,
                };
                for (base, bytes) in pci::bar_ranges(ecam, device)? {
                    if base >= start && base.checked_add(bytes).is_some_and(|limit| limit <= end) {
                        advance(
                            child as u8,
                            NEXT[child].load(Ordering::Acquire).max(base + bytes),
                        );
                    }
                }
            }
            crate::kprintln!(
                "[pci] bridge {:02x}:{:02x}.0 secondary={} window={:#x}+{:#x}",
                parent,
                slot,
                child,
                start,
                end - start
            );
        }
    }
    Ok(())
}

fn slot_registers(ecam: usize, bus: usize) -> Option<(usize, u8, usize)> {
    let port = PORT[bus].load(Ordering::Acquire);
    if port == 0 {
        return None;
    }
    let parent = ((port >> 8) & 255) as usize;
    let slot = ((port >> 16) & 255) as u8;
    let config = ecam + (parent << 20);
    let mut capability = pci::read8(config, slot, 0, 0x34) as usize;
    for _ in 0..48 {
        if !(0x40..=0xfc).contains(&capability) || capability % 4 != 0 {
            break;
        }
        if pci::read8(config, slot, 0, capability) == 0x10
            && pci::read16(config, slot, 0, capability + 2) & (1 << 8) != 0
        {
            return Some((config, slot, capability));
        }
        capability = pci::read8(config, slot, 0, capability + 1) as usize;
    }
    None
}

pub fn poll_hotplug(ecam: usize) {
    for bus in 1..MAX_BUSES {
        let Some((config, slot, capability)) = slot_registers(ecam, bus) else {
            continue;
        };
        let status = pci::read16(config, slot, 0, capability + 0x1a);
        if status & (1 << 3) != 0 {
            crate::kprintln!(
                "[pci] presence changed bus={} slot-status={:#x} control={:#x}",
                bus,
                status,
                pci::read16(config, slot, 0, capability + 0x18)
            );
        }
        let control = pci::read16(config, slot, 0, capability + 0x18);
        let present = status & (1 << 6) != 0;
        let powered_off = control & (1 << 10) != 0;
        // QEMU reports an attention-button event for insertion into a powered
        // off slot too. Only an already-powered slot requests an eject.
        if status & 1 != 0 && present && !powered_off {
            EJECT[bus].store(true, Ordering::Release);
        }
        if present && powered_off {
            EJECT[bus].store(false, Ordering::Release);
            pci::write16(
                config,
                slot,
                0,
                capability + 0x18,
                control & !((1 << 10) | (3 << 8)) | (1 << 8),
            );
        }
        pci::write16(config, slot, 0, capability + 0x1a, status & 0x1f);
    }
}
pub fn eject_requested(bus: u8) -> bool {
    EJECT
        .get(bus as usize)
        .is_some_and(|eject| eject.load(Ordering::Acquire))
}
pub fn finish_eject(ecam: usize, bus: u8) {
    if let Some((config, slot, capability)) = slot_registers(ecam, bus as usize) {
        let control = pci::read16(config, slot, 0, capability + 0x18);
        pci::write16(
            config,
            slot,
            0,
            capability + 0x18,
            control | (1 << 10) | (3 << 8),
        );
        pci::write16(config, slot, 0, capability + 0x1a, 1);
        EJECT[bus as usize].store(false, Ordering::Release);
    }
}
