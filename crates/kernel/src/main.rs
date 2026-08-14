#![no_std]
#![no_main]

mod arch;
mod bootfs;
mod device_control;
mod dtb;
mod gpu;
mod input;
mod kernel_heap;
mod net;
mod pci;
mod physical_memory;
mod random;
mod rtc;
mod service_runtime;
mod smmu;
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

#[unsafe(no_mangle)]
extern "C" fn kernel_main(dtb_physical: usize) -> ! {
    arch::aarch64::install_vectors();
    kprintln!("MicroSystem 0.1.0 AArch64 capability microkernel");
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

    PLATFORM_GICD.store(platform.gicd_base, Ordering::Release);
    PLATFORM_GICR.store(platform.gicr_base, Ordering::Release);
    let timer_frequency =
        match arch::aarch64::init_interrupts(platform.gicd_base, platform.gicr_base, true) {
            Ok(frequency) => {
                kprintln!("[irq] GICv3 timer cpu0 frequency={}Hz slice=5ms", frequency);
                frequency
            }
            Err(()) => {
                kprintln!("[irq] GICv3 initialization failed");
                0
            }
        };
    let wait_start = arch::aarch64::counter();
    while arch::aarch64::counter().wrapping_sub(wait_start) < timer_frequency / 50 {
        core::hint::spin_loop();
    }
    let (timer_ctl, gicd_ctl, pending, hppir) =
        arch::aarch64::interrupt_diagnostics(platform.gicd_base, platform.gicr_base);
    kprintln!(
        "[irq] self-test ticks={} cntv_ctl={:#x} gicd_ctl={:#x} pending={:#x} hppir={}",
        arch::aarch64::timer_ticks(0),
        timer_ctl,
        gicd_ctl,
        pending,
        hppir
    );

    match physical_memory::initialize(platform.ram_base as u64, platform.ram_bytes as u64) {
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
            arch::aarch64::shutdown(false);
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
    arch::aarch64::run_user_demo();
    kprintln!("[service] init task exited cleanly");
    kprintln!("[isolation] launching EL0 invalid-pointer/privilege probe");
    arch::aarch64::run_user_fault_demo();
    kprintln!("[isolation] faulted task terminated; kernel survived");

    let result = arch::aarch64::psci_cpu_on(1);
    kprintln!("[smp] PSCI CPU_ON result={}", result);
    for _ in 0..5_000_000 {
        if CPU1_ONLINE.load(Ordering::Acquire) {
            break;
        }
        core::hint::spin_loop();
    }
    kprintln!("[smp] cpu1-online={}", CPU1_ONLINE.load(Ordering::Acquire));

    let ticks_before = [arch::aarch64::timer_ticks(0), arch::aarch64::timer_ticks(1)];
    kprintln!("[sched] starting two EL0 cpu-bound tasks without yield");
    SMP_DEMO_START.store(true, Ordering::Release);
    arch::aarch64::send_event();
    arch::aarch64::run_user_smp_demo();
    while !SMP_DEMO_DONE.load(Ordering::Acquire) {
        arch::aarch64::wait_for_event();
    }
    let ticks_after = [arch::aarch64::timer_ticks(0), arch::aarch64::timer_ticks(1)];
    kprintln!(
        "[sched] cpu-bound complete counters=[{},{}] timer-preemptions=[{},{}]",
        USER_EXIT_VALUES[0].load(Ordering::Acquire),
        USER_EXIT_VALUES[1].load(Ordering::Acquire),
        ticks_after[0].saturating_sub(ticks_before[0]),
        ticks_after[1].saturating_sub(ticks_before[1])
    );

    let rr = arch::aarch64::run_rr_demo();
    kprintln!(
        "[sched] round-robin context-switches={} progress=[{},{}]",
        rr.switches,
        rr.progress[0],
        rr.progress[1]
    );
    kprintln!(
        "[sched] fp-simd context-isolation={} tasks=2 context-switches={} checks=[{},{}] mismatches=[{},{}] q-regs=q0-q31 fpcr-fpsr=true signatures=[0x11,0x22]",
        rr.fp_simd_mismatches == [0, 0],
        rr.switches,
        rr.fp_simd_checks[0],
        rr.fp_simd_checks[1],
        rr.fp_simd_mismatches[0],
        rr.fp_simd_mismatches[1]
    );
    kprintln!("[mmu] round-robin address-spaces=2 asids=[1,2]");

    let ipc = arch::aarch64::run_ipc_demo();
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
        arch::aarch64::send_event();
        service_runtime::join_primary()
    } else {
        kprintln!("[service] resident EL0 loader failed");
        arch::aarch64::shutdown(false)
    }
}

#[unsafe(no_mangle)]
extern "C" fn secondary_main() -> ! {
    arch::aarch64::install_vectors();
    let gicd = PLATFORM_GICD.load(Ordering::Acquire);
    let gicr = PLATFORM_GICR.load(Ordering::Acquire);
    let timer_ready = arch::aarch64::init_interrupts(gicd, gicr, false).is_ok();
    CPU1_ONLINE.store(true, Ordering::Release);
    kprintln!("[boot] cpu1 online el1 high-half timer={}", timer_ready);
    arch::aarch64::send_event();
    while !SMP_DEMO_START.load(Ordering::Acquire) {
        arch::aarch64::wait_for_event();
    }
    arch::aarch64::run_user_smp_demo();
    SMP_DEMO_DONE.store(true, Ordering::Release);
    arch::aarch64::send_event();
    while !SERVICE_START.load(Ordering::Acquire) {
        arch::aarch64::wait_for_event();
    }
    service_runtime::run()
}

#[unsafe(no_mangle)]
extern "C" fn rust_irq(frame: &mut arch::aarch64::preempt::ExceptionFrame) {
    arch::aarch64::interrupt::handle_irq(frame);
}

#[unsafe(no_mangle)]
extern "C" fn rust_exception(esr: u64, elr: u64, far: u64) -> ! {
    kprintln!(
        "[panic] cpu={} ESR={:#x} ELR={:#x} FAR={:#x}",
        arch::aarch64::cpu_id(),
        esr,
        elr,
        far
    );
    arch::aarch64::shutdown(false)
}

#[unsafe(no_mangle)]
extern "C" fn rust_lower_sync(
    frame: &mut arch::aarch64::preempt::ExceptionFrame,
    esr: u64,
    elr: u64,
    far: u64,
) -> u64 {
    const EC_SVC64: u64 = 0x15;
    if esr >> 26 != EC_SVC64 {
        if let Some((action, pid)) =
            service_runtime::handle_application_exit(frame, microsystem_abi::Status::Fault as i64)
        {
            kprintln!(
                "[proc] application fault ESR={:#x} FAR={:#x}; pid={} terminated",
                esr,
                far,
                pid
            );
            return action;
        }
        kprintln!(
            "[fault] user exception ESR={:#x} ELR={:#x} FAR={:#x} x0={:#x} x1={:#x} x2={:#x} x8={:#x} ttbr0={:#x}; task terminated",
            esr,
            elr,
            far,
            frame.registers[0],
            frame.registers[1],
            frame.registers[2],
            frame.registers[8],
            arch::aarch64::current_ttbr0()
        );
        LAST_USER_STATUS.store(microsystem_abi::Status::Fault as i64, Ordering::Release);
        return 1;
    }
    let syscall = frame.registers[8];
    if let Some(action) = arch::aarch64::preempt::handle_ipc(frame, syscall) {
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
            if let Some((action, _)) = service_runtime::handle_application_exit(frame, arg0 as i64)
            {
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
                arch::aarch64::shutdown(success);
            }
            arch::aarch64::preempt::finish_ipc();
            LAST_USER_STATUS.store(arg0 as i64, Ordering::Release);
            let cpu = arch::aarch64::cpu_id();
            if cpu < USER_EXIT_VALUES.len() {
                USER_EXIT_VALUES[cpu].store(arg0, Ordering::Release);
            }
            1
        }
        18 => {
            frame.registers[0] = match arg0 {
                1 => arch::aarch64::timer_ticks(0),
                2 => arch::aarch64::timer_ticks(1),
                3 => arch::aarch64::cpu_id() as u64,
                _ => arch::aarch64::clock_nanos(),
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
        31 => {
            frame.registers[0] = service_runtime::clock_realtime()
                .map_or_else(|status| status as i64 as u64, |seconds| seconds);
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
            if arg1 == 0 && frame.registers[2] == 1 {
                service_runtime::mark_current_online();
                frame.registers[0] = 0;
                return 0;
            }
            if !service_runtime::validate_user_read(arg0, arg1)
                && !arch::aarch64::validate_user_read(arg0, arg1)
            {
                frame.registers[0] = (-8i64) as u64;
                return 0;
            }
            service_runtime::mark_current_ready();
            let bytes = unsafe { core::slice::from_raw_parts(arg0 as *const u8, arg1 as usize) };
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
    kprintln!("[panic] cpu={} {}", arch::aarch64::cpu_id(), info);
    arch::aarch64::shutdown(false)
}
