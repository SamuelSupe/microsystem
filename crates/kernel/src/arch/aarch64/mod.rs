use core::arch::{asm, global_asm};

pub(crate) mod interrupt;
pub(crate) mod preempt;

pub use interrupt::{
    bind_device_irq, clock_nanos, device_interrupts, init_interrupts, interrupt_diagnostics,
    send_reschedule, timer_ticks,
};

global_asm!(include_str!("boot.S"));
global_asm!(include_str!("exception.S"));

unsafe extern "C" {
    pub static vectors: u8;
    static secondary_entry_phys_value: u64;
    static boot_user_l3_high: u8;
    static boot_ttbr0_l1_high: u8;
    static boot_task1_ttbr0_l1_high: u8;
    static boot_rr_data_high: u8;
    static boot_task1_rr_data_high: u8;
    static user_entry_high: u8;
    static user_rr_entry_high: u8;
    static user_rr_exit_high: u8;
    static user_ipc_client_high: u8;
    static user_ipc_server_high: u8;
    fn enter_user_demo();
    fn enter_user_fault_demo();
    fn enter_user_smp_demo();
    fn enter_user_rr(
        entry: u64,
        argument0: u64,
        argument1: u64,
        argument2: u64,
        argument3: u64,
        argument4: u64,
        argument5: u64,
        argument6: u64,
    );
    fn enter_user_service(
        entry: u64,
        argument0: u64,
        argument1: u64,
        argument2: u64,
        argument3: u64,
        argument4: u64,
        argument5: u64,
        argument6: u64,
    );
}

pub fn cpu_id() -> usize {
    let value: u64;
    unsafe {
        asm!("mrs {0}, mpidr_el1", out(reg) value, options(nomem, nostack, preserves_flags));
    }
    (value & 0xff) as usize
}

pub fn install_vectors() {
    unsafe {
        asm!("msr vbar_el1, {0}", "isb", in(reg) &vectors as *const u8 as u64, options(nostack, preserves_flags));
    }
}

pub fn psci_cpu_on(cpu: u64) -> i64 {
    let mut function = 0xc400_0003u64;
    let entry = unsafe { secondary_entry_phys_value };
    unsafe {
        asm!(
            "smc #0",
            inout("x0") function,
            in("x1") cpu,
            in("x2") entry,
            in("x3") 0u64,
            options(nostack)
        );
    }
    function as i64
}

pub fn wait_for_event() {
    unsafe {
        asm!("wfe", options(nomem, nostack));
    }
}
pub fn send_event() {
    unsafe {
        asm!("sev", options(nomem, nostack));
    }
}

pub fn invalidate_user_page(asid: u16, virtual_address: u64) {
    let operand = (asid as u64) << 48 | ((virtual_address >> 12) & 0x0000_ffff_ffff);
    unsafe {
        asm!(
            "dsb ishst",
            "tlbi vale1is, {operand}",
            "dsb ish",
            "isb",
            operand = in(reg) operand,
            options(nostack, preserves_flags)
        );
    }
}

pub fn invalidate_user_asid(asid: u16) {
    let operand = (asid as u64) << 48;
    unsafe {
        asm!(
            "dsb ishst",
            "tlbi aside1is, {operand}",
            "dsb ish",
            "isb",
            operand = in(reg) operand,
            options(nostack, preserves_flags)
        );
    }
}
pub fn counter() -> u64 {
    let value: u64;
    unsafe {
        asm!("mrs {0}, cntpct_el0", out(reg) value, options(nomem, nostack, preserves_flags));
    }
    value
}
pub fn run_user_demo() {
    unsafe {
        enter_user_demo();
    }
}

pub fn run_user_fault_demo() {
    unsafe {
        enter_user_fault_demo();
    }
}
pub fn run_user_smp_demo() {
    unsafe {
        enter_user_smp_demo();
    }
}
pub fn run_rr_demo() -> preempt::Report {
    let user_base = &raw const user_entry_high as usize;
    let entry = 0x0040_0000 + (&raw const user_rr_entry_high as usize - user_base);
    let exit = 0x0040_0000 + (&raw const user_rr_exit_high as usize - user_base);
    preempt::run(entry as u64, exit as u64)
}

pub fn run_ipc_demo() -> preempt::IpcReport {
    let user_base = &raw const user_entry_high as usize;
    let client = 0x0040_0000 + (&raw const user_ipc_client_high as usize - user_base);
    let server = 0x0040_0000 + (&raw const user_ipc_server_high as usize - user_base);
    preempt::run_ipc(client as u64, server as u64)
}

pub(crate) fn enter_rr(entry: u64, counter: u64) {
    unsafe { enter_user_rr(entry, counter, 0, 0, 0, 0, 0, 0) }
}

pub(crate) fn enter_service(entry: u64, arguments: [u64; 7]) {
    unsafe {
        enter_user_service(
            entry,
            arguments[0],
            arguments[1],
            arguments[2],
            arguments[3],
            arguments[4],
            arguments[5],
            arguments[6],
        )
    }
}

pub(crate) fn rr_data_high(task: usize) -> usize {
    if task == 0 {
        &raw const boot_rr_data_high as usize
    } else {
        &raw const boot_task1_rr_data_high as usize
    }
}

pub(crate) fn rr_ttbr0(task: usize, asid: u16) -> u64 {
    let high = if task == 0 {
        &raw const boot_ttbr0_l1_high as usize
    } else {
        &raw const boot_task1_ttbr0_l1_high as usize
    };
    ((asid as u64) << 48) | (high - 0xffffff8000000000) as u64
}

pub(crate) fn activate_ttbr0(value: u64) {
    unsafe {
        asm!(
            "msr ttbr0_el1, {0}",
            "isb",
            in(reg) value,
            options(nostack, preserves_flags)
        )
    }
}

pub fn current_ttbr0() -> u64 {
    let value: u64;
    unsafe { asm!("mrs {0}, ttbr0_el1", out(reg) value, options(nomem, nostack)) };
    value
}

pub fn validate_user_read(pointer: u64, bytes: u64) -> bool {
    let Some(end) = pointer.checked_add(bytes) else {
        return false;
    };
    if bytes > 4096 || pointer < 0x0040_0000 || end > 0x0060_0000 {
        return false;
    }
    let table = &raw const boot_user_l3_high as *const u64;
    let first = (pointer >> 12) & 0x1ff;
    let last = (end.saturating_sub(1) >> 12) & 0x1ff;
    for index in first..=last {
        let descriptor = unsafe { core::ptr::read_volatile(table.add(index as usize)) };
        if descriptor & 0b11 != 0b11 || descriptor & (1 << 6) == 0 {
            return false;
        }
    }
    true
}

pub fn shutdown(success: bool) -> ! {
    let block = [if success { 0x20026u64 } else { 0x20023u64 }, 0];
    unsafe {
        asm!(
            "hlt #0xf000",
            in("x0") 0x20u64,
            in("x1") block.as_ptr(),
            options(noreturn)
        );
    }
}
