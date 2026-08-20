use core::arch::asm;
use core::sync::atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering};

const TIMER_SLICE_HZ: u64 = 200;
const DEFAULT_TIMEBASE_FREQUENCY: u64 = 10_000_000;

const SIE_SSIE: u64 = 1 << 1;
const SIE_STIE: u64 = 1 << 5;
const SIE_SEIE: u64 = 1 << 9;
const SSTATUS_SIE: u64 = 1 << 1;

const SCAUSE_SOFTWARE_INTERRUPT: u64 = 1;
const SCAUSE_TIMER_INTERRUPT: u64 = 5;
const SCAUSE_EXTERNAL_INTERRUPT: u64 = 9;
const PLIC_PENDING_OFFSET: usize = 0x1000;
const PLIC_ENABLE_OFFSET: usize = 0x2000;
const PLIC_ENABLE_STRIDE: usize = 0x80;
const PLIC_CONTEXT_OFFSET: usize = 0x20_0000;
const PLIC_CONTEXT_STRIDE: usize = 0x1000;
const PLIC_PRIORITY_OFFSET: usize = 0;
const PLIC_THRESHOLD_OFFSET: usize = 0;
const PLIC_CLAIM_OFFSET: usize = 4;
const PLIC_MAX_IRQ: u32 = 1023;

static TIMER_FREQUENCY: AtomicU64 = AtomicU64::new(0);
static TIMER_INTERVAL: AtomicU64 = AtomicU64::new(0);
static TICKS: [AtomicU64; super::MAX_HARTS] = [const { AtomicU64::new(0) }; super::MAX_HARTS];
static PLIC_BASE: AtomicUsize = AtomicUsize::new(0);
static DEVICE_IRQ: AtomicU32 = AtomicU32::new(u32::MAX);
static DEVICE_ISR: AtomicUsize = AtomicUsize::new(0);
static DEVICE_INTERRUPTS: AtomicU64 = AtomicU64::new(0);

pub fn init_interrupts(
    _clint_physical: usize,
    plic_physical: usize,
    _primary: bool,
) -> Result<u64, ()> {
    if plic_physical == 0 {
        return Err(());
    }

    let plic = super::phys_to_virt(plic_physical as u64);
    PLIC_BASE.store(plic, Ordering::Release);
    let hart = super::physical_hart_id().min(super::MAX_HARTS - 1);
    let context = plic_context(plic, hart);
    write32(context + PLIC_THRESHOLD_OFFSET, 0);

    let frequency = DEFAULT_TIMEBASE_FREQUENCY;
    let interval = (frequency / TIMER_SLICE_HZ).max(1);
    if super::sbi::set_timer(super::counter().wrapping_add(interval)).error != 0 {
        return Err(());
    }

    TIMER_FREQUENCY.store(frequency, Ordering::Release);
    TIMER_INTERVAL.store(interval, Ordering::Release);
    unsafe {
        let enables = SIE_SSIE | SIE_STIE | SIE_SEIE;
        asm!("csrs sie, {0}", in(reg) enables, options(nostack, preserves_flags));
        asm!(
            "csrs sstatus, {0}",
            in(reg) SSTATUS_SIE,
            options(nostack, preserves_flags)
        );
    }
    Ok(frequency)
}

pub fn handle_irq(frame: &mut super::preempt::ExceptionFrame) {
    if !frame.is_interrupt() {
        return;
    }
    match frame.exception_code() {
        SCAUSE_TIMER_INTERRUPT => {
            let cpu = super::cpu_id();
            if cpu < TICKS.len() {
                TICKS[cpu].fetch_add(1, Ordering::Relaxed);
            }
            let interval = TIMER_INTERVAL.load(Ordering::Acquire);
            if interval != 0 {
                let _ = super::sbi::set_timer(super::counter().wrapping_add(interval));
            }
            super::preempt::on_timer(frame);
            crate::service_runtime::on_timer(frame);
        }
        SCAUSE_SOFTWARE_INTERRUPT => {
            clear_software_interrupt();
            crate::service_runtime::on_reschedule(frame);
        }
        SCAUSE_EXTERNAL_INTERRUPT => handle_external_interrupt(),
        _ => {}
    }
}

pub fn send_reschedule(cpu: usize) {
    if let Some(hart) = super::physical_hart(cpu) {
        let _ = super::sbi::send_ipi_hart(hart);
    }
}

pub fn bind_device_irq(plic_physical: usize, irq: u32, isr: usize) -> Result<(), ()> {
    let base = if plic_physical != 0 {
        let plic = super::phys_to_virt(plic_physical as u64);
        PLIC_BASE.store(plic, Ordering::Release);
        plic
    } else {
        PLIC_BASE.load(Ordering::Acquire)
    };
    if base == 0 || !(1..=PLIC_MAX_IRQ).contains(&irq) {
        return Err(());
    }

    let hart = super::physical_hart(0).ok_or(())?;
    let context_id = plic_context_id(hart);
    let priority = base + PLIC_PRIORITY_OFFSET + irq as usize * 4;
    write32(priority, 1);

    let enable =
        base + PLIC_ENABLE_OFFSET + context_id * PLIC_ENABLE_STRIDE + (irq as usize / 32) * 4;
    let bit = 1u32 << (irq % 32);
    write32(enable, read32(enable) | bit);
    write32(
        base + PLIC_CONTEXT_OFFSET + context_id * PLIC_CONTEXT_STRIDE,
        0,
    );

    DEVICE_ISR.store(isr, Ordering::Release);
    DEVICE_IRQ.store(irq, Ordering::Release);
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
    ((super::counter() as u128 * 1_000_000_000u128) / frequency as u128) as u64
}

pub fn interrupt_diagnostics(_clint_physical: usize, plic_physical: usize) -> (u64, u32, u32, u64) {
    let status = read_status();
    let base = if plic_physical != 0 {
        super::phys_to_virt(plic_physical as u64)
    } else {
        PLIC_BASE.load(Ordering::Acquire)
    };
    let hart = super::physical_hart_id().min(super::MAX_HARTS - 1);
    let context_id = plic_context_id(hart);
    let threshold = if base == 0 {
        0
    } else {
        read32(base + PLIC_CONTEXT_OFFSET + context_id * PLIC_CONTEXT_STRIDE)
    };
    let pending = if base == 0 {
        0
    } else {
        read32(base + PLIC_PENDING_OFFSET)
    };
    (status, threshold, pending, read_scause())
}

fn handle_external_interrupt() {
    let base = PLIC_BASE.load(Ordering::Acquire);
    if base == 0 {
        return;
    }
    let context_id = plic_context_id(super::physical_hart_id().min(super::MAX_HARTS - 1));
    let claim = base + PLIC_CONTEXT_OFFSET + context_id * PLIC_CONTEXT_STRIDE + PLIC_CLAIM_OFFSET;
    let irq = read32(claim);
    if irq == 0 {
        return;
    }

    if irq == DEVICE_IRQ.load(Ordering::Acquire) {
        let isr = DEVICE_ISR.load(Ordering::Acquire);
        if isr != 0 && unsafe { core::ptr::read_volatile(isr as *const u8) != 0 } {
            DEVICE_INTERRUPTS.fetch_add(1, Ordering::Relaxed);
            crate::service_runtime::notify_device_irq();
        }
    }
    write32(claim, irq);
}

fn clear_software_interrupt() {
    unsafe {
        asm!(
            "csrc sip, {0}",
            in(reg) SIE_SSIE,
            options(nostack, preserves_flags)
        );
    }
}

fn plic_context_id(hart: usize) -> usize {
    // QEMU virt exposes the S-mode context after each hart's M-mode context.
    hart * 2 + 1
}

fn plic_context(base: usize, hart: usize) -> usize {
    base + PLIC_CONTEXT_OFFSET + plic_context_id(hart) * PLIC_CONTEXT_STRIDE
}

fn read_status() -> u64 {
    let value: u64;
    unsafe { asm!("csrr {0}, sstatus", out(reg) value, options(nomem, nostack)) };
    value
}

fn read_scause() -> u64 {
    let value: u64;
    unsafe { asm!("csrr {0}, scause", out(reg) value, options(nomem, nostack)) };
    value
}

fn read32(address: usize) -> u32 {
    unsafe { core::ptr::read_volatile(address as *const u32) }
}

fn write32(address: usize, value: u32) {
    unsafe { core::ptr::write_volatile(address as *mut u32, value) }
}
