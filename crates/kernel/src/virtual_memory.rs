use super::arch::preempt::ExceptionFrame;
use super::{
    ASID_BASE, STATE, TASK_COUNT, arch, current_task, lock_state, physical_memory, task_memory,
    task_memory_mut,
};
use core::cell::UnsafeCell;
use microsystem_abi::{Status, virtual_memory as vm};
use microsystem_kernel::vm::AddressSpace;

struct Spaces(UnsafeCell<[AddressSpace; TASK_COUNT]>);
unsafe impl Sync for Spaces {}
static SPACES: Spaces = Spaces(UnsafeCell::new([const { AddressSpace::new() }; TASK_COUNT]));

// Every caller holds the scheduler lock, including user-buffer materialization.
fn space(task: usize) -> &'static mut AddressSpace {
    unsafe { &mut (&mut *SPACES.0.get())[task] }
}

pub(super) fn handle(frame: &mut ExceptionFrame, syscall: u64) -> Option<u64> {
    if !((37..=41).contains(&syscall) || syscall == 43) || !super::active_on_current_cpu() {
        return None;
    }
    let _guard = lock_state();
    let task = current_task(unsafe { &*STATE.0.get() });
    let start = frame.registers[0];
    let result = match syscall {
        37 => space(task).reserve(start, frame.registers[1], frame.registers[2]),
        38 => space(task).remove(start).map(|region| {
            release_range(task, region.start, region.bytes);
            0
        }),
        39 => space(task)
            .protect(start, frame.registers[1])
            .map(|region| {
                for page in (region.start..region.start + region.bytes).step_by(4096) {
                    if let Some(entry) = descriptor_mut(task, page, false).ok().flatten() {
                        if arch::page_table_present(*entry) {
                            *entry = arch::page_table_entry(
                                arch::page_table_physical(*entry),
                                arch::user_page_flags(region.rights & vm::WRITE != 0, false),
                            );
                        }
                    }
                }
                arch::invalidate_user_asid(ASID_BASE + task as u16);
                0
            }),
        40 => {
            let mut stats = space(task).stats();
            let heap_pages = (super::HEAP_BYTES / 4096
                + if matches!(task, super::MFS_TASK | super::DB_TASK | super::NETD_TASK) {
                    super::LARGE_HEAP_BYTES / 4096
                } else {
                    0
                }) as u32;
            stats.reserved_pages += heap_pages;
            stats.page_budget += heap_pages;
            super::copy_to_user(
                task,
                start,
                (&stats as *const vm::StatsV1).cast(),
                core::mem::size_of::<vm::StatsV1>(),
            )
            .map(|()| 0)
            .ok_or(Status::Fault)
        }
        41 => {
            let bytes = frame.registers[1];
            space(task).resize(start, bytes).map(|old| {
                if bytes < old.bytes {
                    release_range(task, start + bytes, old.bytes - bytes);
                }
                0
            })
        }
        43 => commit_range(task, start, frame.registers[1]).map(|()| 0),
        _ => unreachable!(),
    };
    frame.registers[0] = match result {
        Ok(value) => value,
        Err(status) => status as i64 as u64,
    };
    Some(0)
}

pub(super) fn fault(
    frame: &ExceptionFrame,
    cause: u64,
    address: u64,
) -> Option<Result<(), Status>> {
    if !super::active_on_current_cpu() {
        return None;
    }
    #[cfg(target_arch = "aarch64")]
    let access = (cause >> 26 == 0x24).then_some(cause & (1 << 6) != 0);
    #[cfg(target_arch = "riscv64")]
    let access = match cause {
        13 => Some(false),
        15 => Some(true),
        _ => None,
    };
    #[cfg(target_arch = "x86_64")]
    let access = (cause == 14 && frame.error & (1 << 4) == 0).then_some(frame.error & 2 != 0);
    #[cfg(not(target_arch = "x86_64"))]
    let _ = frame;
    let write = access?;
    let _guard = lock_state();
    let task = current_task(unsafe { &*STATE.0.get() });
    let space = unsafe { &mut (&mut *SPACES.0.get())[task] };
    if !is_heap(task, address) {
        space.find(address)?;
    }
    space.faults = space.faults.saturating_add(1);
    if descriptor(task, address).is_some_and(arch::page_table_present) {
        return Some(Err(Status::Fault));
    }
    Some(commit(task, address, write))
}

pub(super) fn commit(task: usize, address: u64, write: bool) -> Result<(), Status> {
    let space = unsafe { &mut (&mut *SPACES.0.get())[task] };
    let rights = if is_heap(task, address) {
        vm::READ | vm::WRITE
    } else {
        space.find(address).ok_or(Status::Fault)?.rights
    };
    if write && rights & vm::WRITE == 0 {
        return Err(Status::AccessDenied);
    }
    if descriptor(task, address).is_some_and(arch::page_table_present) {
        return Ok(());
    }
    if task >= super::APPLICATION_START && physical_memory::free_frames() <= 64 {
        space.allocation_failures += 1;
        return Err(Status::NoMemory);
    }
    let page = match physical_memory::allocate_zeroed() {
        Ok(page) => page,
        Err(status) => {
            space.allocation_failures += 1;
            return Err(status);
        }
    };
    let entry = match descriptor_mut(task, address, true) {
        Ok(Some(entry)) => entry,
        result => {
            let _ = physical_memory::release(page);
            space.allocation_failures += 1;
            return Err(result.err().unwrap_or(Status::Fault));
        }
    };
    *entry = arch::page_table_entry(page, arch::user_page_flags(rights & vm::WRITE != 0, false));
    space.resident_pages += 1;
    arch::invalidate_user_asid(ASID_BASE + task as u16);
    Ok(())
}

pub(super) fn descriptor(task: usize, address: u64) -> Option<u64> {
    if is_heap(task, address) {
        return super::user_descriptor(task_memory(task)?, address);
    }
    if !(vm::START..vm::END).contains(&address) {
        return None;
    }
    let memory = task_memory(task)?;
    let branch = memory.level2[(address >> 21) as usize];
    if !arch::page_table_present(branch) {
        return Some(0);
    }
    let entries = arch::phys_to_virt(arch::page_table_physical(branch)) as *const u64;
    Some(unsafe { *entries.add(((address >> 12) & 511) as usize) })
}

fn descriptor_mut(
    task: usize,
    address: u64,
    create: bool,
) -> Result<Option<&'static mut u64>, Status> {
    if is_heap(task, address) {
        return Ok(super::user_descriptor_mut(
            task_memory_mut(task).ok_or(Status::Fault)?,
            address,
        ));
    }
    let memory = task_memory_mut(task).ok_or(Status::Fault)?;
    let branch = &mut memory.level2[(address >> 21) as usize];
    if !arch::page_table_present(*branch) {
        if !create {
            return Ok(None);
        }
        *branch = arch::page_table_branch(physical_memory::allocate_zeroed()?);
    }
    let entries = arch::phys_to_virt(arch::page_table_physical(*branch)) as *mut u64;
    Ok(Some(unsafe {
        &mut *entries.add(((address >> 12) & 511) as usize)
    }))
}

fn release_range(task: usize, start: u64, bytes: u64) {
    let space = unsafe { &mut (&mut *SPACES.0.get())[task] };
    for address in (start..start + bytes).step_by(4096) {
        if let Some(entry) = descriptor_mut(task, address, false).ok().flatten() {
            if arch::page_table_present(*entry) {
                let page = arch::page_table_physical(*entry);
                *entry = 0;
                arch::invalidate_user_asid(ASID_BASE + task as u16);
                let _ = physical_memory::release(page);
                space.resident_pages -= 1;
            }
        }
    }
    if let Some(memory) = task_memory_mut(task) {
        for branch in &mut memory.level2[(vm::START >> 21) as usize..(vm::END >> 21) as usize] {
            if !arch::page_table_present(*branch) {
                continue;
            }
            let physical = arch::page_table_physical(*branch);
            let entries = unsafe {
                core::slice::from_raw_parts(arch::phys_to_virt(physical) as *const u64, 512)
            };
            if entries.iter().all(|entry| *entry == 0) {
                *branch = 0;
                arch::invalidate_user_asid(ASID_BASE + task as u16);
                let _ = physical_memory::release(physical);
            }
        }
    }
}

pub(super) fn reclaim(task: usize) -> u32 {
    let resident = unsafe { (&*SPACES.0.get())[task].resident_pages };
    if resident != 0 {
        release_range(task, super::HEAP_START, super::HEAP_BYTES as u64);
        if matches!(task, super::MFS_TASK | super::DB_TASK | super::NETD_TASK) {
            release_range(
                task,
                super::LARGE_HEAP_START,
                super::LARGE_HEAP_BYTES as u64,
            );
        }
        let regions = unsafe { (&*SPACES.0.get())[task].regions() };
        for region in regions.iter().filter(|region| region.bytes != 0) {
            release_range(task, region.start, region.bytes);
        }
    }
    unsafe { (&mut *SPACES.0.get())[task] = AddressSpace::new() };
    resident
}

pub(super) fn resident_pages(task: usize) -> u32 {
    unsafe { (&*SPACES.0.get())[task].resident_pages }
}

pub(super) fn set_budget(task: usize, pages: u32) -> Result<(), Status> {
    space(task).set_budget(pages)
}

fn commit_range(task: usize, start: u64, bytes: u64) -> Result<(), Status> {
    let end = start.checked_add(bytes).ok_or(Status::Invalid)?;
    if bytes == 0 || bytes > 32 * 1024 * 1024 {
        return Err(Status::Invalid);
    }
    let first = start & !4095;
    let last = end.checked_add(4095).ok_or(Status::Invalid)? & !4095;
    if is_heap(task, start) {
        let limit = if start < super::HEAP_START + super::HEAP_BYTES as u64 {
            super::HEAP_START + super::HEAP_BYTES as u64
        } else {
            super::LARGE_HEAP_END
        };
        if end > limit {
            return Err(Status::Invalid);
        }
    } else {
        let region = space(task).find(start).ok_or(Status::Fault)?;
        if !region.contains(end - 1) || region.rights & vm::WRITE == 0 {
            return Err(Status::AccessDenied);
        }
    }
    let needed = (first..last)
        .step_by(4096)
        .filter(|address| !descriptor(task, *address).is_some_and(arch::page_table_present))
        .count();
    let mut tables = 0;
    if (vm::START..vm::END).contains(&start) {
        let memory = task_memory(task).ok_or(Status::Fault)?;
        tables = ((first >> 21)..=((last - 1) >> 21))
            .filter(|index| !arch::page_table_present(memory.level2[*index as usize]))
            .count();
    }
    let reserve = if task >= super::APPLICATION_START {
        64
    } else {
        0
    };
    if physical_memory::free_frames() < needed + tables + reserve {
        space(task).allocation_failures += 1;
        return Err(Status::NoMemory);
    }
    for address in (first..last).step_by(4096) {
        commit(task, address, true)?;
    }
    Ok(())
}

pub(super) fn is_heap(task: usize, address: u64) -> bool {
    (super::HEAP_START..super::HEAP_START + super::HEAP_BYTES as u64).contains(&address)
        || (matches!(task, super::MFS_TASK | super::DB_TASK | super::NETD_TASK)
            && (super::LARGE_HEAP_START..super::LARGE_HEAP_END).contains(&address))
}
