#![no_std]

use microsystem_abi::{
    CapHandle, Message, ObjectType, Rights, Status, Syscall, SystemControlOperation, SystemStats,
};

#[cfg(any(
    target_arch = "aarch64",
    target_arch = "riscv64",
    target_arch = "x86_64"
))]
mod allocator {
    use core::alloc::{GlobalAlloc, Layout};
    use core::ptr;
    use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    #[cfg(not(feature = "large-heap"))]
    const START: usize = 0x0068_0000;
    #[cfg(not(feature = "large-heap"))]
    const END: usize = 0x0078_0000;
    #[cfg(feature = "large-heap")]
    const START: usize = 0x0080_0000;
    #[cfg(feature = "large-heap")]
    const END: usize = 0x0280_0000;
    static HEAD: AtomicUsize = AtomicUsize::new(0);
    static INITIALIZED: AtomicBool = AtomicBool::new(false);
    static LOCKED: AtomicBool = AtomicBool::new(false);

    #[derive(Clone, Copy)]
    #[repr(C)]
    struct FreeBlock {
        bytes: usize,
        next: usize,
    }

    #[derive(Clone, Copy)]
    #[repr(C)]
    struct AllocationHeader {
        start: usize,
        bytes: usize,
    }

    struct Guard;

    impl Drop for Guard {
        fn drop(&mut self) {
            LOCKED.store(false, Ordering::Release);
        }
    }

    pub struct FreeList;

    unsafe impl GlobalAlloc for FreeList {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            let _guard = lock();
            if !unsafe { initialize() } {
                return ptr::null_mut();
            }
            let mut previous = 0;
            let mut current = HEAD.load(Ordering::Relaxed);
            while current != 0 {
                let block = unsafe { ptr::read(current as *const FreeBlock) };
                let Some(user) = align_up(
                    current + core::mem::size_of::<AllocationHeader>(),
                    layout.align(),
                ) else {
                    return ptr::null_mut();
                };
                let Some(data_end) = user.checked_add(layout.size().max(1)) else {
                    return ptr::null_mut();
                };
                let Some(block_end) = current.checked_add(block.bytes) else {
                    return ptr::null_mut();
                };
                if data_end <= block_end {
                    let suffix = align_up(data_end, core::mem::align_of::<FreeBlock>())
                        .filter(|suffix| *suffix <= block_end)
                        .unwrap_or(block_end);
                    let keep_suffix = block_end - suffix >= core::mem::size_of::<FreeBlock>();
                    let committed_end = if keep_suffix {
                        suffix + core::mem::size_of::<FreeBlock>()
                    } else {
                        block_end
                    };
                    if super::virtual_commit(current as u64, (committed_end - current) as u64)
                        .is_err()
                    {
                        return ptr::null_mut();
                    }
                    let next = if keep_suffix { suffix } else { block.next };
                    if keep_suffix {
                        unsafe {
                            ptr::write(
                                suffix as *mut FreeBlock,
                                FreeBlock {
                                    bytes: block_end - suffix,
                                    next: block.next,
                                },
                            )
                        };
                    }
                    if previous == 0 {
                        HEAD.store(next, Ordering::Relaxed);
                    } else {
                        unsafe { (*(previous as *mut FreeBlock)).next = next };
                    }
                    let allocation_end = if keep_suffix { suffix } else { block_end };
                    unsafe {
                        ptr::write(
                            (user - core::mem::size_of::<AllocationHeader>())
                                as *mut AllocationHeader,
                            AllocationHeader {
                                start: current,
                                bytes: allocation_end - current,
                            },
                        )
                    };
                    return user as *mut u8;
                }
                previous = current;
                current = block.next;
            }
            let Some(bytes) = layout
                .size()
                .max(1)
                .checked_add(layout.align())
                .and_then(|bytes| bytes.checked_add(core::mem::size_of::<AllocationHeader>()))
                .and_then(|bytes| align_up(bytes, 4096))
            else {
                return ptr::null_mut();
            };
            let Ok(base) = super::virtual_map(
                0,
                bytes as u64,
                microsystem_abi::virtual_memory::READ | microsystem_abi::virtual_memory::WRITE,
            ) else {
                return ptr::null_mut();
            };
            if super::virtual_commit(base, bytes as u64).is_err() {
                let _ = super::virtual_unmap(base);
                return ptr::null_mut();
            }
            let user = align_up(
                base as usize + core::mem::size_of::<AllocationHeader>(),
                layout.align(),
            )
            .unwrap();
            unsafe {
                ptr::write(
                    (user - core::mem::size_of::<AllocationHeader>()) as *mut AllocationHeader,
                    AllocationHeader {
                        start: base as usize,
                        bytes,
                    },
                );
            }
            user as *mut u8
        }

        unsafe fn dealloc(&self, pointer: *mut u8, _layout: Layout) {
            if pointer.is_null() {
                return;
            }
            let _guard = lock();
            let header = unsafe {
                ptr::read(
                    pointer
                        .sub(core::mem::size_of::<AllocationHeader>())
                        .cast::<AllocationHeader>(),
                )
            };
            if (microsystem_abi::virtual_memory::START as usize
                ..microsystem_abi::virtual_memory::END as usize)
                .contains(&header.start)
            {
                let _ = super::virtual_unmap(header.start as u64);
                return;
            }
            if header.start < START
                || header.bytes < core::mem::size_of::<FreeBlock>()
                || header
                    .start
                    .checked_add(header.bytes)
                    .is_none_or(|end| end > END)
            {
                return;
            }
            unsafe { insert_free(header.start, header.bytes) };
        }
    }

    fn lock() -> Guard {
        while LOCKED
            .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            core::hint::spin_loop();
        }
        Guard
    }

    unsafe fn initialize() -> bool {
        if INITIALIZED.load(Ordering::Acquire) {
            return true;
        }
        if super::virtual_commit(START as u64, 4096).is_err() {
            return false;
        }
        unsafe {
            ptr::write(
                START as *mut FreeBlock,
                FreeBlock {
                    bytes: END - START,
                    next: 0,
                },
            )
        };
        HEAD.store(START, Ordering::Release);
        INITIALIZED.store(true, Ordering::Release);
        true
    }

    unsafe fn insert_free(start: usize, bytes: usize) {
        let mut previous = 0;
        let mut current = HEAD.load(Ordering::Relaxed);
        while current != 0 && current < start {
            previous = current;
            current = unsafe { (*(current as *const FreeBlock)).next };
        }
        unsafe {
            ptr::write(
                start as *mut FreeBlock,
                FreeBlock {
                    bytes,
                    next: current,
                },
            )
        };
        if previous == 0 {
            HEAD.store(start, Ordering::Relaxed);
        } else {
            unsafe { (*(previous as *mut FreeBlock)).next = start };
        }

        if current != 0 && start + bytes == current {
            let next = unsafe { ptr::read(current as *const FreeBlock) };
            unsafe {
                (*(start as *mut FreeBlock)).bytes += next.bytes;
                (*(start as *mut FreeBlock)).next = next.next;
            }
        }
        if previous != 0 {
            let previous_end = previous + unsafe { (*(previous as *const FreeBlock)).bytes };
            if previous_end == start {
                let released = unsafe { ptr::read(start as *const FreeBlock) };
                unsafe {
                    (*(previous as *mut FreeBlock)).bytes += released.bytes;
                    (*(previous as *mut FreeBlock)).next = released.next;
                }
            }
        }
    }

    fn align_up(value: usize, alignment: usize) -> Option<usize> {
        value
            .checked_add(alignment - 1)
            .map(|aligned| aligned & !(alignment - 1))
    }
}

#[cfg(any(
    target_arch = "aarch64",
    target_arch = "riscv64",
    target_arch = "x86_64"
))]
#[global_allocator]
static USER_ALLOCATOR: allocator::FreeList = allocator::FreeList;

#[cfg(target_arch = "aarch64")]
pub unsafe fn syscall(number: Syscall, args: [u64; 6]) -> i64 {
    let mut x0 = args[0];
    unsafe {
        core::arch::asm!(
            "svc #0",
            in("x8") number as u64,
            inout("x0") x0,
            in("x1") args[1], in("x2") args[2], in("x3") args[3],
            in("x4") args[4], in("x5") args[5],
            options(nostack)
        );
    }
    x0 as i64
}

#[cfg(target_arch = "riscv64")]
pub unsafe fn syscall(number: Syscall, args: [u64; 6]) -> i64 {
    let mut a0 = args[0];
    unsafe {
        core::arch::asm!(
            "ecall",
            in("a7") number as u64,
            inout("a0") a0,
            in("a1") args[1], in("a2") args[2], in("a3") args[3],
            in("a4") args[4], in("a5") args[5],
            options(nostack)
        );
    }
    a0 as i64
}

#[cfg(target_arch = "x86_64")]
pub unsafe fn syscall(number: Syscall, args: [u64; 6]) -> i64 {
    let mut result = args[0];
    unsafe {
        core::arch::asm!(
            "int 0x80",
            inlateout("rax") number as u64 => _,
            inout("rdi") result, in("rsi") args[1], in("rdx") args[2],
            in("r10") args[3], in("r8") args[4], in("r9") args[5],
            options(nostack)
        );
    }
    result as i64
}

#[cfg(not(any(
    target_arch = "aarch64",
    target_arch = "riscv64",
    target_arch = "x86_64"
)))]
pub unsafe fn syscall(_number: Syscall, _args: [u64; 6]) -> i64 {
    Status::NotSupported as i64
}

#[inline]
pub fn fence() {
    #[cfg(target_arch = "aarch64")]
    unsafe {
        core::arch::asm!("dmb ish", options(nostack, preserves_flags));
    }
    #[cfg(target_arch = "riscv64")]
    unsafe {
        core::arch::asm!("fence rw, rw", options(nostack));
    }
    #[cfg(target_arch = "x86_64")]
    unsafe {
        core::arch::asm!("mfence", options(nostack, preserves_flags));
    }
    #[cfg(not(any(
        target_arch = "aarch64",
        target_arch = "riscv64",
        target_arch = "x86_64"
    )))]
    core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
}

#[inline]
pub fn fence_store() {
    #[cfg(target_arch = "aarch64")]
    unsafe {
        core::arch::asm!("dmb oshst", options(nostack, preserves_flags));
    }
    #[cfg(target_arch = "riscv64")]
    unsafe {
        core::arch::asm!("fence w, w", options(nostack));
    }
    #[cfg(target_arch = "x86_64")]
    unsafe {
        core::arch::asm!("sfence", options(nostack, preserves_flags));
    }
    #[cfg(not(any(
        target_arch = "aarch64",
        target_arch = "riscv64",
        target_arch = "x86_64"
    )))]
    core::sync::atomic::fence(core::sync::atomic::Ordering::Release);
}

#[inline]
pub fn fence_load() {
    #[cfg(target_arch = "aarch64")]
    unsafe {
        core::arch::asm!("dmb oshld", options(nostack, preserves_flags));
    }
    #[cfg(target_arch = "riscv64")]
    unsafe {
        core::arch::asm!("fence r, r", options(nostack));
    }
    #[cfg(target_arch = "x86_64")]
    unsafe {
        core::arch::asm!("lfence", options(nostack, preserves_flags));
    }
    #[cfg(not(any(
        target_arch = "aarch64",
        target_arch = "riscv64",
        target_arch = "x86_64"
    )))]
    core::sync::atomic::fence(core::sync::atomic::Ordering::Acquire);
}

#[inline]
pub fn wait_for_event() {
    #[cfg(target_arch = "aarch64")]
    unsafe {
        core::arch::asm!("wfe", options(nomem, nostack, preserves_flags));
    }
    #[cfg(target_arch = "riscv64")]
    {
        let _ = yield_now();
    }
    #[cfg(target_arch = "x86_64")]
    unsafe {
        core::arch::asm!("pause", options(nomem, nostack, preserves_flags));
    }
    #[cfg(not(any(
        target_arch = "aarch64",
        target_arch = "riscv64",
        target_arch = "x86_64"
    )))]
    core::hint::spin_loop();
}

pub unsafe fn fault_probe(address: u64) {
    #[cfg(target_arch = "aarch64")]
    unsafe {
        core::arch::asm!(
            "ldr xzr, [{address}]",
            address = in(reg) address,
            options(nostack)
        );
    }
    #[cfg(target_arch = "riscv64")]
    unsafe {
        core::arch::asm!(
            "ld x0, 0({address})",
            address = in(reg) address,
            options(nostack)
        );
    }
    #[cfg(target_arch = "x86_64")]
    unsafe {
        core::arch::asm!(
            "mov rax, qword ptr [{address}]",
            address = in(reg) address,
            out("rax") _,
            options(nostack, readonly)
        );
    }
    #[cfg(not(any(
        target_arch = "aarch64",
        target_arch = "riscv64",
        target_arch = "x86_64"
    )))]
    unsafe {
        core::ptr::read_volatile(address as *const u8);
    }
}

pub unsafe fn privileged_probe() {
    #[cfg(target_arch = "aarch64")]
    unsafe {
        core::arch::asm!("msr ttbr0_el1, xzr", options(nostack));
    }
    #[cfg(target_arch = "riscv64")]
    unsafe {
        core::arch::asm!("csrw satp, x0", options(nostack));
    }
    #[cfg(target_arch = "x86_64")]
    unsafe {
        core::arch::asm!("mov cr3, rax", in("rax") 0u64, options(nostack));
    }
    #[cfg(not(any(
        target_arch = "aarch64",
        target_arch = "riscv64",
        target_arch = "x86_64"
    )))]
    core::hint::spin_loop();
}

pub fn yield_now() -> Result<(), Status> {
    let result = unsafe { syscall(Syscall::Yield, [0; 6]) };
    if result == 0 {
        Ok(())
    } else {
        Err(Status::NotSupported)
    }
}

pub fn debug_write(bytes: &[u8]) -> Result<(), Status> {
    let result = unsafe {
        syscall(
            Syscall::DebugWrite,
            [bytes.as_ptr() as u64, bytes.len() as u64, 0, 0, 0, 0],
        )
    };
    if result == 0 {
        Ok(())
    } else {
        Err(Status::AccessDenied)
    }
}

pub fn debug_read() -> Option<u8> {
    let result = unsafe { syscall(Syscall::DebugWrite, [0, 0, 2, 0, 0, 0]) };
    u8::try_from(result).ok()
}

pub fn debug_write_u64(prefix: &[u8], value: u64, suffix: &[u8]) -> Result<(), Status> {
    let mut digits = [0u8; 20];
    let mut cursor = digits.len();
    let mut remaining = value;
    loop {
        cursor -= 1;
        digits[cursor] = b'0' + (remaining % 10) as u8;
        remaining /= 10;
        if remaining == 0 {
            break;
        }
    }
    let digit_count = digits.len() - cursor;
    let total = prefix.len() + digit_count + suffix.len();
    let mut output = [0u8; 128];
    if total > output.len() {
        return Err(Status::Invalid);
    }
    output[..prefix.len()].copy_from_slice(prefix);
    output[prefix.len()..prefix.len() + digit_count].copy_from_slice(&digits[cursor..]);
    output[prefix.len() + digit_count..total].copy_from_slice(suffix);
    debug_write(&output[..total])
}

pub fn service_online() -> Result<(), Status> {
    status(unsafe { syscall(Syscall::DebugWrite, [0, 0, 1, 0, 0, 0]) })
}

pub fn exit(code: u64) -> ! {
    let _ = unsafe { syscall(Syscall::Exit, [code, 0, 0, 0, 0, 0]) };
    loop {
        core::hint::spin_loop();
    }
}

pub fn clock_now() -> Result<u64, Status> {
    let value = unsafe { syscall(Syscall::ClockNow, [0; 6]) };
    if value >= 0 {
        Ok(value as u64)
    } else {
        Err(Status::NotSupported)
    }
}

pub fn timer_ticks(cpu: usize) -> Result<u64, Status> {
    if cpu > 1 {
        return Err(Status::Invalid);
    }
    let value = unsafe { syscall(Syscall::ClockNow, [cpu as u64 + 1, 0, 0, 0, 0, 0]) };
    if value >= 0 {
        Ok(value as u64)
    } else {
        Err(Status::NotSupported)
    }
}

pub fn current_cpu() -> Result<usize, Status> {
    let value = unsafe { syscall(Syscall::ClockNow, [3, 0, 0, 0, 0, 0]) };
    if (0..=1).contains(&value) {
        Ok(value as usize)
    } else {
        Err(Status::NotSupported)
    }
}

pub fn thread_start(program: u64) -> Result<u64, Status> {
    let value = unsafe { syscall(Syscall::ThreadStart, [program, 0, 0, 0, 0, 0]) };
    if value > 0 {
        Ok(value as u64)
    } else {
        Err(status_error(value))
    }
}

pub fn thread_start_ex(launch: &microsystem_abi::ThreadLaunchV1) -> Result<u64, Status> {
    let value = unsafe {
        syscall(
            Syscall::ThreadStartEx,
            [
                launch as *const microsystem_abi::ThreadLaunchV1 as u64,
                core::mem::size_of::<microsystem_abi::ThreadLaunchV1>() as u64,
                0,
                0,
                0,
                0,
            ],
        )
    };
    if value > 0 {
        Ok(value as u64)
    } else {
        Err(status_error(value))
    }
}

pub fn thread_start_ex_v2(launch: &microsystem_abi::ThreadLaunchV2) -> Result<u64, Status> {
    let value = unsafe {
        syscall(
            Syscall::ThreadStartEx,
            [
                launch as *const microsystem_abi::ThreadLaunchV2 as u64,
                core::mem::size_of::<microsystem_abi::ThreadLaunchV2>() as u64,
                0,
                0,
                0,
                0,
            ],
        )
    };
    if value > 0 {
        Ok(value as u64)
    } else {
        Err(status_error(value))
    }
}

pub fn clock_realtime() -> Result<u64, Status> {
    let value = unsafe { syscall(Syscall::ClockRealtime, [0; 6]) };
    if value >= 0 {
        Ok(value as u64)
    } else {
        Err(status_error(value))
    }
}

pub fn thread_status(pid: u64) -> Result<(bool, i64), Status> {
    let mut output = [0i64; 2];
    status(unsafe {
        syscall(
            Syscall::ThreadStatus,
            [pid, output.as_mut_ptr() as u64, 0, 0, 0, 0],
        )
    })?;
    Ok((output[0] != 0, output[1]))
}

pub fn thread_status_with_cpu_mask(pid: u64) -> Result<(bool, i64, u32), Status> {
    let mut output = [0i64; 3];
    status(unsafe {
        syscall(
            Syscall::ThreadStatus,
            [pid, output.as_mut_ptr() as u64, 3, 0, 0, 0],
        )
    })?;
    Ok((output[0] != 0, output[1], output[2] as u32))
}

pub fn thread_status_v2(
    pid: u64,
    output: &mut microsystem_abi::ProcessInfoV2,
) -> Result<(), Status> {
    status(unsafe {
        syscall(
            Syscall::ThreadStatus,
            [
                pid,
                output as *mut microsystem_abi::ProcessInfoV2 as u64,
                5,
                0,
                0,
                0,
            ],
        )
    })
}

pub fn thread_status_with_resources(pid: u64) -> Result<(bool, i64, u32, u32, u32, u32), Status> {
    let mut output = [0i64; 6];
    status(unsafe {
        syscall(
            Syscall::ThreadStatus,
            [pid, output.as_mut_ptr() as u64, 4, 0, 0, 0],
        )
    })?;
    Ok((
        output[0] != 0,
        output[1],
        output[2] as u32,
        output[3] as u32,
        output[4] as u32,
        output[5] as u32,
    ))
}

pub fn thread_kill(pid: u64, exit_status: i64) -> Result<(), Status> {
    status(unsafe { syscall(Syscall::ThreadKill, [pid, exit_status as u64, 0, 0, 0, 0]) })
}

pub fn object_create(object_type: ObjectType, rights: Rights) -> Result<CapHandle, Status> {
    let value = unsafe {
        syscall(
            Syscall::ObjectCreate,
            [object_type as u64, rights.0 as u64, 0, 0, 0, 0],
        )
    };
    cap_result(value)
}

pub fn memory_pool_create(quota: u32, rights: Rights) -> Result<CapHandle, Status> {
    let value = unsafe {
        syscall(
            Syscall::ObjectCreate,
            [
                ObjectType::MemoryPool as u64,
                rights.0 as u64,
                quota as u64,
                0,
                0,
                0,
            ],
        )
    };
    cap_result(value)
}

pub fn frame_create(pool: CapHandle, rights: Rights) -> Result<CapHandle, Status> {
    let value = unsafe {
        syscall(
            Syscall::ObjectCreate,
            [
                ObjectType::Frame as u64,
                rights.0 as u64,
                pool.0 as u64,
                0,
                0,
                0,
            ],
        )
    };
    cap_result(value)
}

pub fn frame_region_create(
    pool: CapHandle,
    pages: u32,
    rights: Rights,
) -> Result<CapHandle, Status> {
    if pages == 0 {
        return Err(Status::Invalid);
    }
    let value = unsafe {
        syscall(
            Syscall::ObjectCreate,
            [
                ObjectType::FrameRegion as u64,
                rights.0 as u64,
                pool.0 as u64,
                pages as u64,
                0,
                0,
            ],
        )
    };
    cap_result(value)
}

pub fn system_stats(info: CapHandle, output: &mut SystemStats) -> Result<(), Status> {
    status(unsafe {
        syscall(
            Syscall::SystemStats,
            [
                info.0 as u64,
                output as *mut SystemStats as u64,
                core::mem::size_of::<SystemStats>() as u64,
                0,
                0,
                0,
            ],
        )
    })
}

pub fn gui_present(region: CapHandle) -> Result<(), Status> {
    status(unsafe { syscall(Syscall::GuiPresent, [region.0 as u64, 0, 0, 0, 0, 0]) })
}

pub fn gui_present_rect(region: CapHandle, rect: microsystem_abi::gui::Rect) -> Result<(), Status> {
    if rect.x < 0 || rect.y < 0 || rect.width == 0 || rect.height == 0 {
        return Err(Status::Invalid);
    }
    status(unsafe {
        syscall(
            Syscall::GuiPresent,
            [
                region.0 as u64,
                rect.x as u64,
                rect.y as u64,
                rect.width as u64,
                rect.height as u64,
                0,
            ],
        )
    })
}

pub fn gui_input(event: &mut microsystem_abi::gui::InputEvent) -> Result<(), Status> {
    status(unsafe {
        syscall(
            Syscall::GuiInput,
            [
                event as *mut microsystem_abi::gui::InputEvent as u64,
                core::mem::size_of::<microsystem_abi::gui::InputEvent>() as u64,
                0,
                0,
                0,
                0,
            ],
        )
    })
}

pub fn net_receive(device: CapHandle, frame: &mut [u8]) -> Result<usize, Status> {
    net_receive_on(device, 0, frame)
}

pub fn net_receive_on(
    device: CapHandle,
    interface: u32,
    frame: &mut [u8],
) -> Result<usize, Status> {
    if frame.is_empty() || frame.len() > 1536 {
        return Err(Status::Invalid);
    }
    let value = unsafe {
        syscall(
            Syscall::NetReceive,
            [
                device.0 as u64,
                frame.as_mut_ptr() as u64,
                frame.len() as u64,
                interface as u64,
                0,
                0,
            ],
        )
    };
    if value >= 0 {
        Ok(value as usize)
    } else {
        Err(status_error(value))
    }
}

pub fn net_send(device: CapHandle, frame: &[u8]) -> Result<(), Status> {
    net_send_on(device, 0, frame)
}

pub fn net_send_on(device: CapHandle, interface: u32, frame: &[u8]) -> Result<(), Status> {
    if frame.is_empty() || frame.len() > 1536 {
        return Err(Status::Invalid);
    }
    status(unsafe {
        syscall(
            Syscall::NetSend,
            [
                device.0 as u64,
                frame.as_ptr() as u64,
                frame.len() as u64,
                interface as u64,
                0,
                0,
            ],
        )
    })
}

pub fn network_interface_info(
    device: CapHandle,
    interface: u32,
) -> Result<microsystem_abi::network::HardwareInfoV1, Status> {
    let mut info = microsystem_abi::network::HardwareInfoV1::default();
    status(unsafe {
        syscall(
            Syscall::NetworkInterfaceInfo,
            [
                device.0 as u64,
                (&mut info as *mut microsystem_abi::network::HardwareInfoV1) as u64,
                interface as u64,
                0,
                0,
                0,
            ],
        )
    })?;
    Ok(info)
}

pub fn ipc_peer() -> Result<u64, Status> {
    let value = unsafe { syscall(Syscall::IpcPeer, [0; 6]) };
    if value >= 0 { Ok(value as u64) } else { Err(status_error(value)) }
}

pub fn random_fill(source: CapHandle, bytes: &mut [u8]) -> Result<(), Status> {
    if bytes.is_empty() || bytes.len() > 256 {
        return Err(Status::Invalid);
    }
    status(unsafe {
        syscall(
            Syscall::RandomFill,
            [
                source.0 as u64,
                bytes.as_mut_ptr() as u64,
                bytes.len() as u64,
                0,
                0,
                0,
            ],
        )
    })
}

pub fn frame_map(frame: CapHandle, address: u64, rights: Rights) -> Result<(), Status> {
    status(unsafe {
        syscall(
            Syscall::FrameMap,
            [frame.0 as u64, address, rights.0 as u64, 0, 0, 0],
        )
    })
}

/// Reserves page-aligned anonymous memory. Zero selects a free address. Pages
/// are zero-filled on first access; executable mappings are unsupported.
pub fn virtual_map(address: u64, bytes: u64, rights: u64) -> Result<u64, Status> {
    let result = unsafe { syscall(Syscall::VirtualMap, [address, bytes, rights, 0, 0, 0]) };
    if result < 0 {
        Err(status_error(result))
    } else {
        Ok(result as u64)
    }
}

/// Removes the complete reservation at address and releases its resident pages.
pub fn virtual_unmap(address: u64) -> Result<(), Status> {
    status(unsafe { syscall(Syscall::VirtualUnmap, [address, 0, 0, 0, 0, 0]) })
}

pub fn virtual_protect(address: u64, rights: u64) -> Result<(), Status> {
    status(unsafe { syscall(Syscall::VirtualProtect, [address, rights, 0, 0, 0, 0]) })
}

/// Resizes a complete reservation in place. A conflicting neighbour leaves
/// the old region unchanged. Shrinking releases the discarded pages.
pub fn virtual_resize(address: u64, bytes: u64) -> Result<(), Status> {
    status(unsafe { syscall(Syscall::VirtualResize, [address, bytes, 0, 0, 0, 0]) })
}

pub fn virtual_stats(output: &mut microsystem_abi::virtual_memory::StatsV1) -> Result<(), Status> {
    status(unsafe {
        syscall(
            Syscall::VirtualStats,
            [output as *mut _ as u64, 0, 0, 0, 0, 0],
        )
    })
}

/// Init-only static ELF launch. The kernel copies and validates the image,
/// grants only the process endpoint and the explicitly requested information
/// capabilities, and enforces the additional anonymous-page budget.
pub fn thread_start_native(
    image: &[u8],
    program: u64,
    pages: u32,
    permissions: u64,
) -> Result<u64, Status> {
    let result = unsafe {
        syscall(
            Syscall::ThreadStartNative,
            [
                image.as_ptr() as u64,
                image.len() as u64,
                program,
                pages as u64,
                permissions,
                0,
            ],
        )
    };
    if result < 0 {
        Err(status_error(result))
    } else {
        Ok(result as u64)
    }
}

/// Ensures a writable heap/anonymous byte range is resident. NoMemory is
/// returned before changing mappings when the physical allocation cannot fit.
pub fn virtual_commit(address: u64, bytes: u64) -> Result<(), Status> {
    status(unsafe { syscall(Syscall::VirtualCommit, [address, bytes, 0, 0, 0, 0]) })
}

/// Console-service-only byte output sharing the kernel diagnostic UART lock.
pub fn console_write_bytes(bytes: &[u8]) -> Result<(), Status> {
    status(unsafe {
        syscall(
            Syscall::DebugWrite,
            [bytes.as_ptr() as u64, bytes.len() as u64, 3, 0, 0, 0],
        )
    })
}

pub fn frame_unmap(frame: CapHandle, address: u64) -> Result<(), Status> {
    status(unsafe { syscall(Syscall::FrameUnmap, [frame.0 as u64, address, 0, 0, 0, 0]) })
}

pub fn cap_copy(source: CapHandle, rights: Rights) -> Result<CapHandle, Status> {
    let value = unsafe {
        syscall(
            Syscall::CapCopy,
            [source.0 as u64, rights.0 as u64, 0, 0, 0, 0],
        )
    };
    cap_result(value)
}

pub fn cap_move(source: CapHandle) -> Result<CapHandle, Status> {
    let value = unsafe { syscall(Syscall::CapMove, [source.0 as u64, 0, 0, 0, 0, 0]) };
    cap_result(value)
}

pub fn cap_revoke(handle: CapHandle) -> Result<usize, Status> {
    let value = unsafe { syscall(Syscall::CapRevoke, [handle.0 as u64, 0, 0, 0, 0, 0]) };
    if value >= 0 {
        Ok(value as usize)
    } else {
        Err(status_error(value))
    }
}

pub fn cap_delete(handle: CapHandle) -> Result<(), Status> {
    status(unsafe { syscall(Syscall::CapDelete, [handle.0 as u64, 0, 0, 0, 0, 0]) })
}

pub fn notification_wait(notification: CapHandle, deadline: u64) -> Result<u64, Status> {
    loop {
        let value = unsafe {
            syscall(
                Syscall::NotificationWait,
                [notification.0 as u64, deadline, 0, 0, 0, 0],
            )
        };
        if value > 0 {
            return Ok(value as u64);
        }
        let error = status_error(value);
        if error != Status::Busy {
            return Err(error);
        }
        let _ = yield_now();
    }
}

pub fn notification_signal(notification: CapHandle, bits: u64) -> Result<(), Status> {
    status(unsafe {
        syscall(
            Syscall::NotificationSignal,
            [notification.0 as u64, bits, 0, 0, 0, 0],
        )
    })
}

/// Consumes available notification bits without sleeping. Busy means no event.
pub fn notification_poll(notification: CapHandle) -> Result<u64, Status> {
    let value = unsafe {
        syscall(
            Syscall::NotificationPoll,
            [notification.0 as u64, 0, 0, 0, 0, 0],
        )
    };
    if value < 0 {
        Err(status_error(value))
    } else {
        Ok(value as u64)
    }
}

pub fn irq_bind(irq: CapHandle, notification: CapHandle) -> Result<(), Status> {
    status(unsafe {
        syscall(
            Syscall::IrqBind,
            [irq.0 as u64, notification.0 as u64, 0, 0, 0, 0],
        )
    })
}

pub fn irq_ack(irq: CapHandle) -> Result<(), Status> {
    status(unsafe { syscall(Syscall::IrqAck, [irq.0 as u64, 0, 0, 0, 0, 0]) })
}

pub fn dma_map(domain: CapHandle, frame: CapHandle, iova: u64) -> Result<(), Status> {
    status(unsafe {
        syscall(
            Syscall::DmaMap,
            [domain.0 as u64, frame.0 as u64, iova, 0, 0, 0],
        )
    })
}

pub fn dma_unmap(domain: CapHandle, iova: u64) -> Result<(), Status> {
    status(unsafe { syscall(Syscall::DmaUnmap, [domain.0 as u64, iova, 0, 0, 0, 0]) })
}

pub fn system_activate_pci(system: CapHandle, slot: u8, function: u8) -> Result<(), Status> {
    status(unsafe {
        syscall(
            Syscall::SystemControl,
            [
                system.0 as u64,
                SystemControlOperation::ActivatePci as u64,
                slot as u64,
                function as u64,
                0,
                0,
            ],
        )
    })
}

pub fn system_power(system: CapHandle, reboot: bool) -> Result<(), Status> {
    let operation = if reboot {
        SystemControlOperation::Reboot
    } else {
        SystemControlOperation::Poweroff
    };
    status(unsafe {
        syscall(
            Syscall::SystemControl,
            [system.0 as u64, operation as u64, 0, 0, 0, 0],
        )
    })
}

fn cap_result(value: i64) -> Result<CapHandle, Status> {
    if value > 0 {
        Ok(CapHandle(value as u32))
    } else {
        Err(status_error(value))
    }
}

pub fn ipc_call(
    endpoint: CapHandle,
    request: &Message,
    reply: &mut Message,
    deadline: u64,
) -> Result<(), Status> {
    loop {
        let result = status(unsafe {
            syscall(
                Syscall::IpcCall,
                [
                    endpoint.0 as u64,
                    request as *const Message as u64,
                    reply as *mut Message as u64,
                    deadline,
                    0,
                    0,
                ],
            )
        });
        match result {
            Err(Status::Busy) => {
                let _ = yield_now();
            }
            result => return result,
        }
    }
}

/// Attempts a call without blocking or yielding. The receiver must be waiting.
/// Only messages without capability transfers are accepted. Poll the single
/// reply slot before submitting another call.
pub fn ipc_try_call(endpoint: CapHandle, request: &Message) -> Result<(), Status> {
    status(unsafe {
        syscall(
            Syscall::IpcTryCall,
            [
                endpoint.0 as u64,
                request as *const Message as u64,
                0,
                0,
                0,
                0,
            ],
        )
    })
}

pub fn service_status(pid: u64, info: &mut microsystem_abi::service::InfoV1) -> Result<(), Status> {
    status(unsafe {
        syscall(
            Syscall::ServiceStatus,
            [pid, info as *mut _ as u64, 0, 0, 0, 0],
        )
    })
}

pub fn service_stop(pid: u64, exit_status: i64) -> Result<(), Status> {
    status(unsafe { syscall(Syscall::ServiceStop, [pid, exit_status as u64, 0, 0, 0, 0]) })
}

/// Returns Busy while a call is pending. Successful retrieval acknowledges the
/// reply; an invalid destination leaves it available for a subsequent poll.
pub fn ipc_poll_reply(reply: &mut Message) -> Result<(), Status> {
    status(unsafe {
        syscall(
            Syscall::IpcPollReply,
            [reply as *mut Message as u64, 0, 0, 0, 0, 0],
        )
    })
}

pub fn ipc_recv(endpoint: CapHandle, message: &mut Message, deadline: u64) -> Result<(), Status> {
    loop {
        let result = status(unsafe {
            syscall(
                Syscall::IpcRecv,
                [
                    endpoint.0 as u64,
                    message as *mut Message as u64,
                    deadline,
                    0,
                    0,
                    0,
                ],
            )
        });
        match result {
            Err(Status::Busy) => {
                let _ = yield_now();
            }
            result => return result,
        }
    }
}

pub fn ipc_reply_recv(
    endpoint: CapHandle,
    reply: &Message,
    next: &mut Message,
    deadline: u64,
) -> Result<(), Status> {
    loop {
        let result = status(unsafe {
            syscall(
                Syscall::IpcReplyRecv,
                [
                    endpoint.0 as u64,
                    reply as *const Message as u64,
                    next as *mut Message as u64,
                    deadline,
                    0,
                    0,
                ],
            )
        });
        match result {
            Err(Status::Busy) => {
                let _ = yield_now();
            }
            result => return result,
        }
    }
}

fn status(value: i64) -> Result<(), Status> {
    if value == 0 {
        Ok(())
    } else {
        Err(status_error(value))
    }
}

fn status_error(value: i64) -> Status {
    match value {
        -1 => Status::Invalid,
        -2 => Status::BadCapability,
        -3 => Status::AccessDenied,
        -4 => Status::NotFound,
        -5 => Status::NoMemory,
        -6 => Status::Busy,
        -7 => Status::TimedOut,
        -8 => Status::Fault,
        -10 => Status::Io,
        -11 => Status::NoSpace,
        -12 => Status::Corrupt,
        _ => Status::NotSupported,
    }
}
