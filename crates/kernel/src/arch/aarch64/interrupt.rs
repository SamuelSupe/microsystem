use core::arch::asm;
use core::sync::atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering};

const KERNEL_OFFSET: usize = 0xffffff8000000000;
const GICR_STRIDE: usize = 0x20000;
const TIMER_IRQ: u32 = 27;
const RESCHEDULE_SGI: u32 = 1;
const TIMER_SLICE_HZ: u64 = 200;

static TIMER_INTERVAL: AtomicU64 = AtomicU64::new(0);
static TIMER_FREQUENCY: AtomicU64 = AtomicU64::new(0);
static TICKS: [AtomicU64; 2] = [AtomicU64::new(0), AtomicU64::new(0)];
static DEVICE_IRQ: AtomicU32 = AtomicU32::new(u32::MAX);
static DEVICE_ISR: AtomicUsize = AtomicUsize::new(0);
static DEVICE_INTERRUPTS: AtomicU64 = AtomicU64::new(0);

pub fn init_interrupts(
    gicd_physical: usize,
    gicr_physical: usize,
    primary: bool,
) -> Result<u64, ()> {
    if gicd_physical == 0 || gicr_physical == 0 {
        return Err(());
    }
    if primary {
        let distributor = KERNEL_OFFSET + gicd_physical;
        write32(distributor, (1 << 4) | (1 << 1) | 1);
        while read32(distributor) & (1 << 31) != 0 {
            core::hint::spin_loop();
        }
    }

    let redistributor = redistributor_for_current_cpu(gicr_physical).ok_or(())?;
    let mut waker = read32(redistributor + 0x14);
    waker &= !(1 << 1);
    write32(redistributor + 0x14, waker);
    while read32(redistributor + 0x14) & (1 << 2) != 0 {
        core::hint::spin_loop();
    }

    let sgi = redistributor + 0x10000;
    write32(sgi + 0x80, u32::MAX);
    write8(sgi + 0x400 + RESCHEDULE_SGI as usize, 0x80);
    write8(sgi + 0x400 + TIMER_IRQ as usize, 0x80);
    write32(sgi + 0x100, (1 << RESCHEDULE_SGI) | (1 << TIMER_IRQ));

    let frequency: u64;
    unsafe {
        asm!("mrs {0}, cntfrq_el0", out(reg) frequency, options(nomem, nostack, preserves_flags));
        asm!("msr icc_sre_el1, {0}", "isb", in(reg) 1u64, options(nostack, preserves_flags));
        asm!("msr icc_pmr_el1, {0}", in(reg) 0xffu64, options(nostack, preserves_flags));
        asm!("msr icc_igrpen1_el1, {0}", "isb", in(reg) 1u64, options(nostack, preserves_flags));
    }
    let interval = frequency / TIMER_SLICE_HZ;
    TIMER_FREQUENCY.store(frequency, Ordering::Release);
    TIMER_INTERVAL.store(interval, Ordering::Release);
    rearm_timer(interval);
    unsafe {
        asm!("msr daifclr, #2", options(nostack, preserves_flags));
    }
    Ok(frequency)
}

pub fn handle_irq(frame: &mut super::preempt::ExceptionFrame) {
    let interrupt: u64;
    unsafe {
        asm!("mrs {0}, icc_iar1_el1", out(reg) interrupt, options(nomem, nostack));
    }
    let id = (interrupt & 0x00ff_ffff) as u32;
    if id == TIMER_IRQ {
        let cpu = super::cpu_id();
        if cpu < TICKS.len() {
            TICKS[cpu].fetch_add(1, Ordering::Relaxed);
        }
        rearm_timer(TIMER_INTERVAL.load(Ordering::Relaxed));
        super::preempt::on_timer(frame);
        crate::service_runtime::on_timer(frame);
    } else if id == RESCHEDULE_SGI {
        crate::service_runtime::on_reschedule(frame);
    } else if id == DEVICE_IRQ.load(Ordering::Acquire) {
        let isr = DEVICE_ISR.load(Ordering::Acquire);
        if isr != 0 && unsafe { core::ptr::read_volatile(isr as *const u8) } != 0 {
            DEVICE_INTERRUPTS.fetch_add(1, Ordering::Relaxed);
            crate::service_runtime::notify_device_irq();
        }
    }
    if id < 1020 {
        unsafe {
            asm!("msr icc_eoir1_el1, {0}", "isb", in(reg) interrupt, options(nostack, preserves_flags));
        }
    }
}

pub fn send_reschedule(cpu: usize) {
    if cpu >= 16 {
        return;
    }
    let value = ((RESCHEDULE_SGI as u64) << 24) | (1u64 << cpu);
    unsafe {
        asm!("dsb ishst", "msr icc_sgi1r_el1, {0}", "isb", in(reg) value, options(nostack));
    }
}

pub fn bind_device_irq(gicd_physical: usize, irq: u32, isr: usize) -> Result<(), ()> {
    if gicd_physical == 0 || !(32..1020).contains(&irq) || isr == 0 {
        return Err(());
    }
    let distributor = KERNEL_OFFSET + gicd_physical;
    let bit = 1u32 << (irq % 32);
    let group = distributor + 0x80 + (irq as usize / 32) * 4;
    write32(group, read32(group) | bit);
    write8(distributor + 0x400 + irq as usize, 0x80);

    let config = distributor + 0xc00 + (irq as usize / 16) * 4;
    let shift = (irq % 16) * 2;
    write32(config, read32(config) & !(0b11 << shift));
    write64(distributor + 0x6000 + irq as usize * 8, 0);

    DEVICE_ISR.store(isr, Ordering::Release);
    DEVICE_IRQ.store(irq, Ordering::Release);
    write32(distributor + 0x100 + (irq as usize / 32) * 4, bit);
    Ok(())
}

pub fn device_interrupts() -> u64 {
    DEVICE_INTERRUPTS.load(Ordering::Relaxed)
}

pub fn timer_ticks(cpu: usize) -> u64 {
    TICKS
        .get(cpu)
        .map(|ticks| ticks.load(Ordering::Relaxed))
        .unwrap_or(0)
}

pub fn clock_nanos() -> u64 {
    let frequency = TIMER_FREQUENCY.load(Ordering::Acquire);
    if frequency == 0 {
        return 0;
    }
    let counter = super::counter();
    counter / frequency * 1_000_000_000 + counter % frequency * 1_000_000_000 / frequency
}

pub fn interrupt_diagnostics(gicd_physical: usize, gicr_physical: usize) -> (u64, u32, u32, u64) {
    let timer_control: u64;
    let highest_pending: u64;
    unsafe {
        asm!("mrs {0}, cntv_ctl_el0", out(reg) timer_control, options(nomem, nostack));
        asm!("mrs {0}, icc_hppir1_el1", out(reg) highest_pending, options(nomem, nostack));
    }
    let distributor_control = read32(KERNEL_OFFSET + gicd_physical);
    let pending = redistributor_for_current_cpu(gicr_physical)
        .map(|base| read32(base + 0x10000 + 0x200))
        .unwrap_or(0);
    (timer_control, distributor_control, pending, highest_pending)
}

fn redistributor_for_current_cpu(physical: usize) -> Option<usize> {
    let affinity = current_affinity();
    let mut base = KERNEL_OFFSET + physical;
    for _ in 0..64 {
        let typer = read64(base + 0x08);
        if typer >> 32 == affinity {
            return Some(base);
        }
        if typer & (1 << 4) != 0 {
            break;
        }
        base += GICR_STRIDE;
    }
    None
}

fn current_affinity() -> u64 {
    let mpidr: u64;
    unsafe {
        asm!("mrs {0}, mpidr_el1", out(reg) mpidr, options(nomem, nostack, preserves_flags));
    }
    ((mpidr >> 32) & 0xff) << 24
        | ((mpidr >> 16) & 0xff) << 16
        | ((mpidr >> 8) & 0xff) << 8
        | (mpidr & 0xff)
}

fn rearm_timer(interval: u64) {
    unsafe {
        asm!("msr cntv_tval_el0, {0}", in(reg) interval, options(nostack, preserves_flags));
        asm!("msr cntv_ctl_el0, {0}", "isb", in(reg) 1u64, options(nostack, preserves_flags));
    }
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

fn write32(address: usize, value: u32) {
    unsafe { core::ptr::write_volatile(address as *mut u32, value) }
}

fn write64(address: usize, value: u64) {
    unsafe { core::ptr::write_volatile(address as *mut u64, value) }
}
