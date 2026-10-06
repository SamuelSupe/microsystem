#![no_std]
#![no_main]

mod arch;
mod bootfs;
mod device_control;
mod dtb;
#[cfg(target_arch = "x86_64")]
mod x86_firmware;
mod gpu;
mod input;
mod kernel_heap;
mod net;
mod pci;
mod pci_bridges;
mod physical_memory;
mod random;
#[cfg(target_arch = "riscv64")]
mod riscv_iommu;
mod rtc;
mod service_runtime;
#[cfg(target_arch = "aarch64")]
mod smmu;
#[cfg(target_arch = "x86_64")]
mod vtd;
#[cfg(target_arch = "riscv64")]
pub(crate) use riscv_iommu as smmu;
#[cfg(target_arch = "x86_64")]
pub(crate) use vtd as smmu;
mod uart;

use core::panic::PanicInfo;
use core::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, AtomicUsize, Ordering};
use microsystem_abi::{ObjectType, Rights};
use microsystem_kernel::capability::CapabilityTable;
use microsystem_kernel::elf::ElfImage;

static CPU1_ONLINE: AtomicBool = AtomicBool::new(false);
static PLATFORM_GICD: AtomicUsize = AtomicUsize::new(0);
static PLATFORM_GICR: AtomicUsize = AtomicUsize::new(0);
static SMP_DEMO_START: AtomicBool = AtomicBool::new(false);
static SMP_DEMO_DONE: AtomicBool = AtomicBool::new(false);
static SERVICE_START: AtomicBool = AtomicBool::new(false);
static USER_EXIT_VALUES: [AtomicU64; 2] = [AtomicU64::new(0), AtomicU64::new(0)];
static LAST_USER_STATUS: AtomicI64 = AtomicI64::new(i64::MIN);
static BOOTFS: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../build/bootfs.cpio"
));

fn interrupt_bases(platform: &dtb::PlatformInfo) -> (usize, usize) {
    #[cfg(target_arch = "aarch64")]
    {
        (platform.gicd_base, platform.gicr_base)
    }
    #[cfg(target_arch = "riscv64")]
    {
        (platform.clint_base, platform.plic_base)
    }
    #[cfg(target_arch = "x86_64")]
    {
        (platform.gicd_base, platform.gicr_base)
    }
}

#[unsafe(no_mangle)]
extern "C" fn kernel_main(dtb_physical: usize) -> ! {
    arch::selected::initialize_boot_cpu();
    arch::selected::install_vectors();
    kprintln!("MicroSystem 0.1.0 capability microkernel");
    kprintln!("[boot] cpu0 online el1 high-half");
    kprintln!("[mmu] TTBR1 high-half and TTBR0 EL0 mappings active");

    let platform = dtb::discover(dtb_physical);
    uart::set_physical_base(platform.uart_base);
    rtc::initialize(platform.rtc_base);
    service_runtime::grant_console_uart(platform.uart_base as u64);
    kprintln!(
        "[boot] dtb valid={} bytes={} cpus={}",
        platform.valid,
        platform.bytes,
        platform.cpus
    );
    kprintln!(
        "[boot] ram={:#x}+{:#x} uart={:#x}",
        platform.ram_base,
        platform.ram_bytes,
        platform.uart_base
    );
    kprintln!(
        "[boot] gicd={:#x} gicr={:#x} pcie={:#x} smmu-v3={:#x}",
        platform.gicd_base,
        platform.gicr_base,
        platform.pcie_base,
        platform.smmu_base
    );
    #[cfg(target_arch = "riscv64")]
    kprintln!(
        "[boot] riscv-iommu={:#x}+{:#x}",
        platform.riscv_iommu_base,
        platform.riscv_iommu_bytes
    );
    kprintln!(
        "[boot] pcie-mmio={:#x}+{:#x} iommu-map={:#x}->{:#x}+{:#x} mask={:#x}",
        platform.pcie_mmio_base,
        platform.pcie_mmio_bytes,
        platform.iommu_rid_base,
        platform.iommu_sid_base,
        platform.iommu_map_length,
        platform.iommu_map_mask
    );
    kprintln!("[boot] pcie-present={}", platform.has_pcie);

    let (interrupt_base, interrupt_aux) = interrupt_bases(&platform);
    PLATFORM_GICD.store(interrupt_base, Ordering::Release);
    PLATFORM_GICR.store(interrupt_aux, Ordering::Release);
    let timer_frequency = match arch::selected::init_interrupts(interrupt_base, interrupt_aux, true)
    {
        Ok(frequency) => {
            #[cfg(target_arch = "aarch64")]
            kprintln!("[irq] GICv3 timer cpu0 frequency={}Hz slice=5ms", frequency);
            #[cfg(target_arch = "riscv64")]
            kprintln!(
                "[irq] PLIC/SBI timer cpu0 frequency={}Hz slice=5ms",
                frequency
            );
            #[cfg(target_arch = "x86_64")]
            kprintln!(
                "[irq] local-APIC timer cpu0 frequency={}Hz slice=5ms",
                frequency
            );
            frequency
        }
        Err(()) => {
            kprintln!("[irq] interrupt/timer initialization failed; stopping boot");
            arch::selected::shutdown(false)
        }
    };
    let wait_start = arch::selected::counter();
    while arch::selected::counter().wrapping_sub(wait_start) < timer_frequency / 50 {
        core::hint::spin_loop();
    }
    let (timer_ctl, gicd_ctl, pending, hppir) =
        arch::selected::interrupt_diagnostics(interrupt_base, interrupt_aux);
    kprintln!(
        "[irq] self-test ticks={} cntv_ctl={:#x} gicd_ctl={:#x} pending={:#x} hppir={}",
        arch::selected::timer_ticks(0),
        timer_ctl,
        gicd_ctl,
        pending,
        hppir
    );

    match physical_memory::initialize(&platform) {
        Ok(layout) => {
            kprintln!(
                "[mm] kernel heap ready base={:#x} bytes={:#x}",
                layout.kernel_heap_base,
                layout.kernel_heap_bytes
            );
            kprintln!(
                "[mm] frame allocator ready base={:#x} free={}",
                layout.frame_base,
                layout.free_frames
            )
        }
        Err(status) => {
            kprintln!(
                "[mm] frame allocator initialization failed status={}",
                status as i64
            );
            arch::selected::shutdown(false);
        }
    }

    let mut capabilities = CapabilityTable::new();
    let _root = capabilities
        .insert_root(1, ObjectType::SystemControl, Rights::ALL)
        .unwrap();
    kprintln!(
        "[kernel] capability table ready objects={}",
        capabilities.len()
    );
    let mut bootfs_entries = 0usize;
    let mut valid_elfs = 0usize;
    let mut bootfs_valid = true;
    for entry in bootfs::Archive::new(BOOTFS) {
        match entry {
            Ok(entry) => {
                bootfs_entries += 1;
                if entry.name.starts_with("bin/") && ElfImage::parse(entry.data).is_ok() {
                    valid_elfs += 1;
                }
            }
            Err(()) => bootfs_valid = false,
        }
    }
    kprintln!(
        "[bootfs] valid={} entries={} static-elfs={}",
        bootfs_valid,
        bootfs_entries,
        valid_elfs
    );
    device_control::configure(platform);

    kprintln!("[service] launching init task at EL0");
    arch::selected::run_user_demo();
    kprintln!("[service] init task exited cleanly");
    kprintln!("[isolation] launching EL0 invalid-pointer/privilege probe");
    arch::selected::run_user_fault_demo();
    kprintln!("[isolation] faulted task terminated; kernel survived");

    let result = arch::selected::psci_cpu_on(1);
    kprintln!("[smp] PSCI CPU_ON result={}", result);
    let smp_wait_start = arch::selected::counter();
    let smp_wait_budget = if timer_frequency == 0 {
        5_000_000
    } else {
        timer_frequency
    };
    if timer_frequency == 0 {
        for _ in 0..smp_wait_budget {
            if CPU1_ONLINE.load(Ordering::Acquire) {
                break;
            }
            core::hint::spin_loop();
        }
    } else {
        while !CPU1_ONLINE.load(Ordering::Acquire)
            && arch::selected::counter().wrapping_sub(smp_wait_start) < smp_wait_budget
        {
            core::hint::spin_loop();
        }
    }
    let cpu1_online = CPU1_ONLINE.load(Ordering::Acquire);
    kprintln!("[smp] cpu1-online={}", cpu1_online);
    if !cpu1_online {
        kprintln!("[smp] secondary hart did not come online; aborting SMP gate");
        arch::selected::shutdown(false);
    }

    let ticks_before = [
        arch::selected::timer_ticks(0),
        arch::selected::timer_ticks(1),
    ];
    kprintln!("[sched] starting two EL0 cpu-bound tasks without yield");
    SMP_DEMO_START.store(true, Ordering::Release);
    arch::selected::send_event();
    arch::selected::run_user_smp_demo();
    while !SMP_DEMO_DONE.load(Ordering::Acquire) {
        arch::selected::wait_for_event();
    }
    let ticks_after = [
        arch::selected::timer_ticks(0),
        arch::selected::timer_ticks(1),
    ];
    kprintln!(
        "[sched] cpu-bound complete counters=[{},{}] timer-preemptions=[{},{}]",
        USER_EXIT_VALUES[0].load(Ordering::Acquire),
        USER_EXIT_VALUES[1].load(Ordering::Acquire),
        ticks_after[0].saturating_sub(ticks_before[0]),
        ticks_after[1].saturating_sub(ticks_before[1])
    );

    let rr = arch::selected::run_rr_demo();
    kprintln!(
        "[sched] round-robin context-switches={} progress=[{},{}]",
        rr.switches,
        rr.progress[0],
        rr.progress[1]
    );
    kprintln!(
        "[sched] fp-simd context-isolation={} tasks=2 context-switches={} checks=[{},{}] mismatches=[{},{}] {} signatures=[0x11,0x22]",
        rr.fp_simd_verified
            && rr.fp_simd_checks.iter().all(|checks| *checks > 0)
            && rr.fp_simd_mismatches == [0, 0],
        rr.switches,
        rr.fp_simd_checks[0],
        rr.fp_simd_checks[1],
        rr.fp_simd_mismatches[0],
        rr.fp_simd_mismatches[1],
        if cfg!(target_arch = "aarch64") {
            "q-regs=q0-q31 fpcr-fpsr=true"
        } else if cfg!(target_arch = "x86_64") {
            "x87-sse=fxsave64 true"
        } else {
            "fd-regs=f0-f31 fcsr=true"
        }
    );
    kprintln!("[mmu] round-robin address-spaces=2 asids=[1,2]");

    let ipc = arch::selected::run_ipc_demo();
    kprintln!(
        "[ipc] call/recv/reply endpoint=1 request={:#x} reply={:#x} asids=[3,4]",
        ipc.request,
        ipc.reply
    );

    if service_runtime::prepare(BOOTFS).is_ok() {
        kprintln!(
            "[service] resident EL0 address-spaces={} asids=[{:#x}..{:#x}]",
            service_runtime::SERVICE_COUNT,
            0x20,
            0x20 + service_runtime::SERVICE_COUNT - 1
        );
        SERVICE_START.store(true, Ordering::Release);
        arch::selected::send_event();
        service_runtime::join_primary()
    } else {
        kprintln!("[service] resident EL0 loader failed");
        arch::selected::shutdown(false)
    }
}

#[unsafe(no_mangle)]
extern "C" fn secondary_main() -> ! {
    arch::selected::install_vectors();
    let gicd = PLATFORM_GICD.load(Ordering::Acquire);
    let gicr = PLATFORM_GICR.load(Ordering::Acquire);
    let timer_ready = arch::selected::init_interrupts(gicd, gicr, false).is_ok();
    if !timer_ready {
        kprintln!("[boot] cpu1 interrupt/timer initialization failed; stopping boot");
        arch::selected::shutdown(false);
    }
    CPU1_ONLINE.store(true, Ordering::Release);
    kprintln!("[boot] cpu1 online el1 high-half timer={}", timer_ready);
    arch::selected::send_event();
    while !SMP_DEMO_START.load(Ordering::Acquire) {
        arch::selected::wait_for_event();
    }
    arch::selected::run_user_smp_demo();
    SMP_DEMO_DONE.store(true, Ordering::Release);
    arch::selected::send_event();
    while !SERVICE_START.load(Ordering::Acquire) {
        arch::selected::wait_for_event();
    }
    service_runtime::run()
}

#[unsafe(no_mangle)]
extern "C" fn rust_irq(frame: &mut arch::selected::preempt::ExceptionFrame) {
    arch::selected::interrupt::handle_irq(frame);
}

#[unsafe(no_mangle)]
extern "C" fn rust_exception(esr: u64, elr: u64, far: u64) -> ! {
    let cpu = arch::selected::cpu_id();
    let (task, task_elr, task_ttbr0) = service_runtime::panic_context(cpu);
    kprintln!(
        "[panic] cpu={} task={} ESR={:#x} ELR={:#x} FAR={:#x} task-elr={:#x} task-ttbr0={:#x}",
        cpu,
        task,
        esr,
        elr,
        far,
        task_elr,
        task_ttbr0
    );
    arch::selected::shutdown(false)
}

#[unsafe(no_mangle)]
extern "C" fn rust_lower_sync(
    frame: &mut arch::selected::preempt::ExceptionFrame,
    esr: u64,
    elr: u64,
    far: u64,
) -> u64 {
    if !arch::is_syscall(esr) {
        let fault_status = match service_runtime::handle_page_fault(frame, esr, far) {
            Some(Ok(())) => return 0,
            Some(Err(status)) => status,
            None => microsystem_abi::Status::Fault,
        };
        if let Some((action, pid)) = service_runtime::handle_task_exit(frame, fault_status as i64) {
            kprintln!(
                "[proc] {} fault ESR={:#x} FAR={:#x}; pid={} terminated",
                if pid < microsystem_abi::process::FIRST_APPLICATION_PID {
                    "service"
                } else {
                    "application"
                },
                esr,
                far,
                pid
            );
            return action;
        }
        #[cfg(target_arch = "x86_64")]
        kprintln!(
            "[fault] user exception ESR={:#x} error={:#x} ELR={:#x} FAR={:#x} cs={:#x} rsp={:#x} frame={:#x} x0={:#x} x1={:#x} x2={:#x} x8={:#x} ttbr0={:#x}; task terminated",
            esr,
            frame.error,
            elr,
            far,
            frame.cs,
            frame.rsp,
            frame as *const arch::selected::preempt::ExceptionFrame as usize,
            frame.registers[0],
            frame.registers[1],
            frame.registers[2],
            frame.registers[8],
            arch::selected::current_ttbr0()
        );
        #[cfg(not(target_arch = "x86_64"))]
        kprintln!(
            "[fault] user exception ESR={:#x} ELR={:#x} FAR={:#x} x0={:#x} x1={:#x} x2={:#x} x8={:#x} ttbr0={:#x}; task terminated",
            esr,
            elr,
            far,
            frame.registers[0],
            frame.registers[1],
            frame.registers[2],
            frame.registers[8],
            arch::selected::current_ttbr0()
        );
        LAST_USER_STATUS.store(microsystem_abi::Status::Fault as i64, Ordering::Release);
        return 1;
    }
    let syscall = arch::selected::preempt::syscall_number(frame);
    if let Some(action) = service_runtime::handle_virtual_memory(frame, syscall) {
        return action;
    }
    if let Some(action) = arch::selected::preempt::handle_ipc(frame, syscall) {
        return action;
    }
    if let Some(action) = service_runtime::handle_ipc(frame, syscall) {
        return action;
    }
    if let Some(action) = service_runtime::handle_notification(frame, syscall) {
        return action;
    }
    if let Some(action) = service_runtime::handle_irq_control(frame, syscall) {
        return action;
    }
    if let Some(action) = service_runtime::handle_dma_control(frame, syscall) {
        return action;
    }
    if let Some(action) = service_runtime::handle_system_control(frame, syscall) {
        return action;
    }
    if let Some(action) = service_runtime::handle_system_stats(frame, syscall) {
        return action;
    }
    if let Some(action) = service_runtime::handle_gui_present(frame, syscall) {
        return action;
    }
    if let Some(action) = service_runtime::handle_gui_input(frame, syscall) {
        return action;
    }
    if let Some(action) = service_runtime::handle_network(frame, syscall) {
        return action;
    }
    if syscall == 45 {
        frame.registers[0] = service_runtime::ipc_peer().map(|pid| pid as i64).unwrap_or_else(|status| status as i64) as u64;
        return 0;
    }
    if let Some(action) = service_runtime::handle_frame_mapping(frame, syscall) {
        return action;
    }
    if let Some(action) = service_runtime::handle_capability(frame, syscall) {
        return action;
    }
    if syscall == 0 {
        if let Some(action) = service_runtime::handle_yield(frame) {
            return action;
        }
    }
    let arg0 = frame.registers[0];
    let arg1 = frame.registers[1];
    match syscall {
        0 => {
            frame.registers[0] = 0;
            0
        }
        1 => {
            if let Some((action, _)) = service_runtime::handle_task_exit(frame, arg0 as i64) {
                return action;
            }
            if let Some((service, success)) = service_runtime::exit_disposition(arg0) {
                if success {
                    kprintln!("[system] shutdown");
                } else {
                    kprintln!(
                        "[service] critical service {} exited status={}",
                        service,
                        arg0
                    );
                }
                arch::selected::shutdown(success);
            }
            arch::selected::preempt::finish_ipc();
            LAST_USER_STATUS.store(arg0 as i64, Ordering::Release);
            let cpu = arch::selected::cpu_id();
            if cpu < USER_EXIT_VALUES.len() {
                USER_EXIT_VALUES[cpu].store(arg0, Ordering::Release);
            }
            1
        }
        18 => {
            frame.registers[0] = match arg0 {
                1 => arch::selected::timer_ticks(0),
                2 => arch::selected::timer_ticks(1),
                3 => arch::selected::cpu_id() as u64,
                _ => arch::selected::clock_nanos(),
            };
            0
        }
        13 => {
            frame.registers[0] = match service_runtime::start_thread(arg0) {
                Ok(pid) => pid,
                Err(status) => status as i64 as u64,
            };
            0
        }
        30 => {
            frame.registers[0] = match service_runtime::start_thread_ex(arg0, arg1) {
                Ok(pid) => pid,
                Err(status) => status as i64 as u64,
            };
            0
        }
        42 => {
            frame.registers[0] = service_runtime::start_native(
                arg0,
                arg1,
                frame.registers[2],
                frame.registers[3],
                frame.registers[4],
            )
            .unwrap_or_else(|status| status as i64 as u64);
            0
        }
        31 => {
            frame.registers[0] = service_runtime::clock_realtime()
                .map_or_else(|status| status as i64 as u64, |seconds| seconds);
            0
        }
        35 => {
            frame.registers[0] = service_runtime::write_service_status(arg0, arg1)
                .map_or_else(|status| status as i64 as u64, |()| 0);
            0
        }
        36 => {
            frame.registers[0] = service_runtime::stop_service(arg0, arg1 as i64)
                .map_or_else(|status| status as i64 as u64, |()| 0);
            0
        }
        20 => {
            frame.registers[0] =
                match service_runtime::write_application_status(arg0, arg1, frame.registers[2]) {
                    Ok(()) => 0,
                    Err(status) => status as i64 as u64,
                };
            0
        }
        21 => {
            frame.registers[0] = match service_runtime::kill_application(arg0, arg1 as i64) {
                Ok(()) => 0,
                Err(status) => status as i64 as u64,
            };
            0
        }
        19 => {
            let console_bytes = frame.registers[2] == 3;
            if console_bytes && !service_runtime::current_task_owns_console() {
                frame.registers[0] = microsystem_abi::Status::AccessDenied as i64 as u64;
                return 0;
            }
            if arg1 == 0 && frame.registers[2] == 1 {
                service_runtime::mark_current_online();
                frame.registers[0] = 0;
                return 0;
            }
            if arg1 == 0 && frame.registers[2] == 2 {
                #[cfg(target_arch = "x86_64")]
                {
                    frame.registers[0] = if service_runtime::current_task_owns_console() {
                        uart::get().map_or(u64::MAX, u64::from)
                    } else {
                        microsystem_abi::Status::AccessDenied as i64 as u64
                    };
                }
                #[cfg(not(target_arch = "x86_64"))]
                {
                    frame.registers[0] = u64::MAX;
                }
                return 0;
            }
            let Ok(length) = usize::try_from(arg1) else {
                frame.registers[0] = (-8i64) as u64;
                return 0;
            };
            let mut copied = [0u8; 4096];
            let Some(copied) = copied.get_mut(..length) else {
                frame.registers[0] = (-8i64) as u64;
                return 0;
            };
            let bytes = if service_runtime::active_on_current_cpu() {
                if service_runtime::copy_user_read(arg0, copied).is_err() {
                    frame.registers[0] = (-8i64) as u64;
                    return 0;
                }
                &*copied
            } else {
                if !arch::selected::validate_user_read(arg0, arg1) {
                    frame.registers[0] = (-8i64) as u64;
                    return 0;
                }
                unsafe { core::slice::from_raw_parts(arg0 as *const u8, length) }
            };
            service_runtime::mark_current_ready();
            if console_bytes {
                uart::write_bytes(bytes);
                frame.registers[0] = 0;
                return 0;
            }
            if let Ok(text) = core::str::from_utf8(bytes) {
                kprint!("{}", text);
                frame.registers[0] = 0;
            } else {
                frame.registers[0] = (-1i64) as u64;
            }
            0
        }
        _ => {
            frame.registers[0] = (-9i64) as u64;
            0
        }
    }
}

#[panic_handler]
fn panic(info: &PanicInfo<'_>) -> ! {
    kprintln!("[panic] cpu={} {}", arch::selected::cpu_id(), info);
    arch::selected::shutdown(false)
}
