use core::sync::atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering};

use super::preempt::ExceptionFrame;

const TIMER_VECTOR: u8 = 0x20;
const RESCHEDULE_VECTOR: u8 = 0xf1;
const DEVICE_VECTOR: u8 = 0x31;
const SPURIOUS_VECTOR: u8 = 0xff;

static TIMER_TICKS: [AtomicU64; 2] = [AtomicU64::new(0), AtomicU64::new(0)];
static DEVICE_INTERRUPTS: AtomicU64 = AtomicU64::new(0);
static DEVICE_ISR: AtomicUsize = AtomicUsize::new(0);
static TSC_HZ: AtomicU64 = AtomicU64::new(1_000_000_000);
static DEVICE_IRQ: AtomicU32 = AtomicU32::new(0);

pub fn init_interrupts(_base: usize, _aux: usize, _boot_cpu: bool) -> Result<u64, ()> {
    if _base == 0 { return Err(()); }
    let frequency = tsc_frequency();
    TSC_HZ.store(frequency, Ordering::Release);
    unsafe {
        mask_legacy_pic();
        write_apic(0xf0, 0x100 | 0xff);
        write_apic(0x3e0, 0b1011);
        write_apic(0x320, TIMER_VECTOR as u32 | (1 << 17));
        write_apic(0x380, (frequency / 16 / 200).max(1) as u32);
        core::arch::asm!("sti", options(nomem, nostack, preserves_flags));
    }
    Ok(frequency)
}

unsafe fn mask_legacy_pic() {
    unsafe {
        core::arch::asm!(
            "out dx, al",
            in("dx") 0x21u16,
            in("al") 0xffu8,
            options(nomem, nostack, preserves_flags)
        );
        core::arch::asm!(
            "out dx, al",
            in("dx") 0xa1u16,
            in("al") 0xffu8,
            options(nomem, nostack, preserves_flags)
        );
    }
}

pub fn handle_irq(frame: &mut ExceptionFrame) {
    match frame.vector as u8 {
        TIMER_VECTOR => {
            let cpu = super::cpu_id().min(1);
            TIMER_TICKS[cpu].fetch_add(1, Ordering::Relaxed);
            super::preempt::on_timer(frame);
            crate::service_runtime::on_timer(frame);
        }
        RESCHEDULE_VECTOR => crate::service_runtime::on_reschedule(frame),
        DEVICE_VECTOR => {
            DEVICE_INTERRUPTS.fetch_add(1, Ordering::Relaxed);
            let isr = DEVICE_ISR.load(Ordering::Acquire);
            if isr != 0 {
                let status = unsafe { core::ptr::read_volatile(isr as *const u8) };
                if status & 1 != 0 {
                    crate::service_runtime::notify_device_irq();
                }
            }
        }
        SPURIOUS_VECTOR => return,
        _ => {}
    }
    unsafe { write_apic(0xb0, 0) };
}

pub fn bind_device_irq(base: usize, irq: u32, isr: usize) -> Result<(), ()> {
    if base == 0 || irq >= 24 || isr == 0 {
        return Err(());
    }
    let ioapic = crate::arch::phys_to_virt(base as u64);
    unsafe {
        ioapic_write(ioapic, 0x10 + irq * 2 + 1, 0);
        ioapic_write(
            ioapic,
            0x10 + irq * 2,
            DEVICE_VECTOR as u32 | (1 << 13) | (1 << 15),
        );
    }
    DEVICE_IRQ.store(irq, Ordering::Release);
    DEVICE_ISR.store(isr, Ordering::Release);
    Ok(())
}

pub fn send_reschedule(cpu: usize) {
    if cpu >= 2 || cpu == super::cpu_id() {
        return;
    }
    let Some(target) = super::cpu_apic_id(cpu) else { return; };
    unsafe {
        write_apic(0x310, target << 24);
        write_apic(0x300, RESCHEDULE_VECTOR as u32 | (1 << 14));
    }
}

pub fn timer_ticks(cpu: usize) -> u64 {
    TIMER_TICKS
        .get(cpu)
        .map_or(0, |ticks| ticks.load(Ordering::Relaxed))
}

pub fn device_interrupts() -> u64 {
    DEVICE_INTERRUPTS.load(Ordering::Relaxed)
}

pub fn clock_nanos() -> u64 {
    let hz = TSC_HZ.load(Ordering::Acquire).max(1);
    (super::counter() as u128 * 1_000_000_000u128 / hz as u128) as u64
}

pub fn interrupt_diagnostics(_base: usize, _aux: usize) -> (u32, u32, u32, u32) {
    let timer = unsafe { read_apic(0x320) };
    let spurious = unsafe { read_apic(0xf0) };
    let irr = unsafe { read_apic(0x200) };
    (timer, spurious, irr, DEVICE_IRQ.load(Ordering::Acquire))
}

fn tsc_frequency() -> u64 {
    let leaf = core::arch::x86_64::__cpuid(0);
    if leaf.eax >= 0x15 {
        let timing = core::arch::x86_64::__cpuid(0x15);
        if timing.eax != 0 && timing.ebx != 0 && timing.ecx != 0 {
            return timing.ecx as u64 * timing.ebx as u64 / timing.eax as u64;
        }
    }
    1_000_000_000
}

unsafe fn read_apic(offset: usize) -> u32 {
    unsafe {
        core::ptr::read_volatile(
            (crate::arch::phys_to_virt(super::local_apic_base()) + offset) as *const u32,
        )
    }
}

unsafe fn write_apic(offset: usize, value: u32) {
    unsafe {
        core::ptr::write_volatile(
            (crate::arch::phys_to_virt(super::local_apic_base()) + offset) as *mut u32,
            value,
        )
    }
}

unsafe fn ioapic_write(base: usize, register: u32, value: u32) {
    unsafe {
        core::ptr::write_volatile(base as *mut u32, register);
        core::ptr::write_volatile((base + 0x10) as *mut u32, value);
    }
}
