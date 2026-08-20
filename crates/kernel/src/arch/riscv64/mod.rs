use core::arch::{asm, global_asm};
use core::sync::atomic::{AtomicUsize, Ordering};

pub(crate) mod interrupt;
pub(crate) mod preempt;
pub mod sbi;

pub use interrupt::{
    bind_device_irq, clock_nanos, device_interrupts, init_interrupts, interrupt_diagnostics,
    send_reschedule, timer_ticks,
};
pub use preempt::{ExceptionFrame, FpContext};
pub use sbi::{
    SbiRet, hart_get_status, hart_start, hart_stop, remote_sfence_vma_asid, send_ipi, send_ipi_all,
    send_ipi_hart, set_timer, system_reset,
};

global_asm!(include_str!("boot.S"));
global_asm!(include_str!("exception.S"));

pub const PAGE_SIZE: usize = 4096;
pub const USER_ENTRY_VA: u64 = 0x0040_0000;
pub const USER_STACK_TOP: u64 = 0x0080_0000;
pub const MAX_HARTS: usize = 8;

// The base is the canonical Sv39 direct-map address for physical RAM.
pub const KERNEL_OFFSET: u64 = 0xffff_ffc0_0000_0000;

const SATP_MODE_SV39: u64 = 8 << 60;
const SATP_ASID_MASK: u64 = 0xffff << 44;
const SATP_PPN_MASK: u64 = (1 << 44) - 1;
const SV39_PHYSICAL_LIMIT: u64 = 1 << 56;
const SSTATUS_SPIE: u64 = 1 << 5;
const SSTATUS_SPP: u64 = 1 << 8;
const SCAUSE_INTERRUPT: u64 = 1 << 63;
const ECALL_FROM_U: u64 = 8;
const ECALL_FROM_S: u64 = 9;
const ENOSYS: u64 = (-38i64) as u64;

unsafe extern "C" {
    pub static trap_vector: u8;
    static boot_root_high: u8;
    static boot_user_l1_high: u8;
    static boot_user_l2_high: u8;
    static boot_rr_data_high: u8;
    static boot_task1_root_high: u8;
    static boot_task1_user_l1_high: u8;
    static boot_task1_user_l2_high: u8;
    static boot_task1_rr_data_high: u8;
    fn riscv_user_resume();
    fn riscv_trap_scratch_physical() -> u64;
    fn riscv_secondary_entry_physical() -> u64;
    fn riscv_user_entry_physical() -> u64;
    fn riscv_user_rr_entry_physical() -> u64;
    fn riscv_user_rr_exit_physical() -> u64;
    fn riscv_user_ipc_client_physical() -> u64;
    fn riscv_user_ipc_server_physical() -> u64;
    fn enter_user_with_stack(entry: u64, stack: u64, arguments: *const u64);
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

/// A syscall hook receives a complete trap frame. It owns advancing `sepc`
/// with [`preempt::ecall_return`] (or deliberately changing `sepc` when it
/// schedules another context).
pub type SyscallHandler = unsafe extern "C" fn(&mut ExceptionFrame);

static SYSCALL_HANDLER: AtomicUsize = AtomicUsize::new(0);
static BOOT_HART: AtomicUsize = AtomicUsize::new(usize::MAX);

pub fn install_syscall_handler(handler: Option<SyscallHandler>) {
    SYSCALL_HANDLER.store(
        handler.map_or(0, |handler| handler as usize),
        Ordering::Release,
    );
}

/// The weak assembly entry point delegates here. The common kernel owns the
/// syscall and user-fault policy; this backend only supplies the RV trap ABI.
#[unsafe(no_mangle)]
extern "C" fn riscv_default_trap(frame: *mut ExceptionFrame) {
    let frame = unsafe { &mut *frame };
    if frame.is_interrupt() {
        let interrupted_sepc = frame.sepc;
        interrupt::handle_irq(frame);
        validate_trap_return(frame, "interrupt", interrupted_sepc);
        return;
    }
    let cause = frame.scause;
    let sepc = frame.sepc;
    let stval = frame.stval;
    if !frame.from_user() {
        crate::kprintln!(
            "[trap] kernel sstatus={:#x} ra={:#x} sp={:#x} gp={:#x} tp={:#x} t6={:#x} satp={:#x}",
            frame.sstatus,
            frame.registers[preempt::REG_RA],
            frame.registers[preempt::REG_SP],
            frame.registers[preempt::REG_GP],
            frame.registers[preempt::REG_TP],
            frame.registers[30],
            current_satp()
        );
        // Only U-mode faults are eligible for application termination. A
        // synchronous trap taken while the kernel is running must follow the
        // same fatal path as the AArch64 current-EL exception vector.
        crate::rust_exception(cause, sepc, stval);
    }
    if is_syscall(cause) {
        frame.sepc = frame.sepc.wrapping_add(4);
    }
    let action = crate::rust_lower_sync(frame, cause, sepc, stval);
    if action == 1 {
        // The built-in boot probes enter U-mode from a normal Rust call. The
        // AArch64 path returns to that call after the probe exits; preserve the
        // same contract here by asking sret to resume in S-mode. Bit 0 of the
        // aligned scratch pointer is a private trap-return marker.
        frame.sepc = riscv_user_resume as *const () as usize as u64;
        frame.sstatus |= SSTATUS_SPP | (1 << 5);
        frame.sstatus &= !(1 << 1);
        frame.scratch |= 1;
    }
    validate_trap_return(frame, "exception", sepc);
}

fn validate_trap_return(frame: &ExceptionFrame, source: &str, interrupted_sepc: u64) {
    if frame.sepc != 0 {
        return;
    }
    crate::kprintln!(
        "[panic] zero trap return source={} scause={:#x} interrupted-sepc={:#x}",
        source,
        frame.scause,
        interrupted_sepc
    );
    shutdown(false)
}

pub fn initialize_boot_cpu() {
    let hart = physical_hart_id();
    if hart >= 2 {
        crate::kprintln!("[boot] unsupported boot hart {}; expected 0 or 1", hart);
        shutdown(false);
    }
    BOOT_HART.store(hart, Ordering::Release);
}

pub fn physical_hart_id() -> usize {
    let value: usize;
    unsafe {
        asm!("mv {0}, tp", out(reg) value, options(nomem, nostack, preserves_flags));
    }
    value
}

pub fn cpu_id() -> usize {
    let hart = physical_hart_id();
    let boot = BOOT_HART.load(Ordering::Acquire);
    if boot == usize::MAX || hart == boot {
        0
    } else {
        1
    }
}

pub(crate) fn physical_hart(cpu: usize) -> Option<usize> {
    let boot = BOOT_HART.load(Ordering::Acquire);
    if boot >= 2 {
        return None;
    }
    match cpu {
        0 => Some(boot),
        1 => Some(boot ^ 1),
        _ => None,
    }
}

fn physical_hart_mask(cpu_mask: u32) -> u64 {
    let mut hart_mask = 0u64;
    for cpu in 0..2 {
        if cpu_mask & (1 << cpu) != 0
            && let Some(hart) = physical_hart(cpu)
        {
            hart_mask |= 1 << hart;
        }
    }
    hart_mask
}

pub fn install_vectors() {
    let hart = physical_hart_id().min(MAX_HARTS - 1);
    let scratch = phys_to_virt(unsafe { riscv_trap_scratch_physical() }) + hart * 64;
    let stack_top = unsafe { core::ptr::read_volatile(scratch as *const u64) };
    let stack_top = if stack_top < KERNEL_OFFSET {
        phys_to_virt(stack_top) as u64
    } else {
        stack_top
    };
    set_trap_stack(stack_top);
    unsafe {
        asm!(
            "csrw stvec, {0}",
            "fence.i",
            in(reg) &raw const trap_vector as *const u8 as u64,
            options(nostack, preserves_flags)
        );
    }
}

pub fn set_trap_stack(stack_top: u64) {
    let hart = physical_hart_id().min(MAX_HARTS - 1);
    let scratch = phys_to_virt(unsafe { riscv_trap_scratch_physical() }) + hart * 64;
    unsafe {
        core::ptr::write_volatile(scratch as *mut u64, stack_top);
        core::ptr::write_volatile((scratch + 8) as *mut u64, hart as u64);
        asm!("csrw sscratch, {0}", in(reg) scratch as u64, options(nostack));
    }
}

pub fn secondary_entry_address() -> u64 {
    unsafe { riscv_secondary_entry_physical() }
}

pub fn psci_cpu_on(cpu: u64) -> i64 {
    let Ok(cpu) = usize::try_from(cpu) else {
        return sbi::SBI_ERR_INVALID_PARAM as i64;
    };
    let Some(hart) = physical_hart(cpu) else {
        return sbi::SBI_ERR_INVALID_PARAM as i64;
    };
    hart_start(hart as u64, secondary_entry_address(), 0).error as i64
}

pub fn wait_for_event() {
    unsafe { asm!("wfi", options(nomem, nostack, preserves_flags)) }
}

pub fn send_event() {
    let _ = send_ipi_all();
}

pub fn counter() -> u64 {
    let value: u64;
    unsafe {
        asm!("rdtime {0}", out(reg) value, options(nomem, nostack, preserves_flags));
    }
    value
}

pub fn satp_value(root_physical: u64, asid: u16) -> Result<u64, SatpError> {
    if root_physical == 0 || root_physical & (PAGE_SIZE as u64 - 1) != 0 {
        return Err(SatpError::UnalignedRoot);
    }
    if root_physical >= SV39_PHYSICAL_LIMIT || (root_physical >> 12) & !SATP_PPN_MASK != 0 {
        return Err(SatpError::PhysicalAddressTooWide);
    }
    Ok(SATP_MODE_SV39 | ((asid as u64) << 44) | (root_physical >> 12))
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SatpError {
    UnalignedRoot,
    PhysicalAddressTooWide,
}

pub fn activate_satp(root_physical: u64, asid: u16) -> Result<u64, SatpError> {
    let value = satp_value(root_physical, asid)?;
    unsafe { write_satp(value) };
    Ok(value)
}

pub fn activate_ttbr0(value: u64) {
    unsafe { write_satp(value) }
}

pub fn deactivate_satp() {
    unsafe {
        asm!(
            "csrw satp, zero",
            "sfence.vma zero, zero",
            "fence.i",
            options(nostack, preserves_flags)
        );
    }
}

pub fn current_satp() -> u64 {
    let value: u64;
    unsafe { asm!("csrr {0}, satp", out(reg) value, options(nomem, nostack, preserves_flags)) };
    value
}

pub fn current_ttbr0() -> u64 {
    current_satp()
}

unsafe fn write_satp(value: u64) {
    unsafe {
        asm!(
            "csrw satp, {0}",
            "sfence.vma zero, zero",
            "fence.i",
            in(reg) value,
            options(nostack, preserves_flags)
        );
    }
}

pub fn invalidate_user_page(asid: u16, virtual_address: u64) {
    let virtual_address = virtual_address & !(PAGE_SIZE as u64 - 1);
    let asid = asid as u64;
    unsafe {
        asm!(
            "sfence.vma {0}, {1}",
            in(reg) virtual_address,
            in(reg) asid,
            options(nostack, preserves_flags)
        );
    }
}

pub fn invalidate_user_asid(asid: u16) {
    let asid = asid as u64;
    unsafe {
        asm!(
            "fence rw, rw",
            "sfence.vma zero, {0}",
            in(reg) asid,
            options(nostack, preserves_flags)
        );
    }
    let cpu = cpu_id();
    let active_mask = crate::service_runtime::active_cpu_mask();
    let remote_cpu_mask = if cpu < u32::BITS as usize {
        active_mask & !(1u32 << cpu)
    } else {
        active_mask
    };
    let remote_hart_mask = physical_hart_mask(remote_cpu_mask);
    if remote_hart_mask != 0
        && !sbi::remote_sfence_vma_asid(remote_hart_mask, 0, 0, 0, asid as u16).is_ok()
    {
        shutdown(false);
    }
}

pub fn invalidate_all() {
    unsafe { asm!("sfence.vma zero, zero", options(nostack, preserves_flags)) }
}

pub const fn phys_to_virt(physical: u64) -> usize {
    KERNEL_OFFSET.wrapping_add(physical) as usize
}

pub const fn virt_to_phys(virtual_address: u64) -> Option<u64> {
    virt_to_phys_u64(virtual_address)
}

pub const fn virtual_address(physical: u64) -> usize {
    phys_to_virt(physical)
}

pub const fn physical_address(virtual_address: u64) -> Option<u64> {
    virt_to_phys(virtual_address)
}

pub const fn virt_to_phys_u64(virtual_address: u64) -> Option<u64> {
    if virtual_address >= KERNEL_OFFSET {
        let physical = virtual_address - KERNEL_OFFSET;
        if physical < (1 << 38) {
            return Some(physical);
        }
    }
    None
}

pub const fn is_canonical_sv39(address: u64) -> bool {
    let upper = address >> 39;
    ((address >> 38) & 1 == 0 && upper == 0) || ((address >> 38) & 1 == 1 && upper == 0x1fff)
}

pub fn user_entry_address() -> u64 {
    unsafe { riscv_user_entry_physical() }
}

fn user_virtual_address(symbol: u64) -> u64 {
    USER_ENTRY_VA
        .wrapping_add(symbol)
        .wrapping_sub(user_entry_address())
}

pub fn dma_barrier() {
    unsafe { asm!("fence iorw, iorw", options(nostack, preserves_flags)) }
}

pub fn dma_write_barrier() {
    unsafe { asm!("fence w, o", options(nostack, preserves_flags)) }
}

pub fn dma_read_barrier() {
    unsafe { asm!("fence i, r", options(nostack, preserves_flags)) }
}

pub fn is_syscall(cause: u64) -> bool {
    matches!(cause & !SCAUSE_INTERRUPT, 8 | 9)
}

pub fn flush_icache() {
    unsafe { asm!("fence.i", options(nostack, preserves_flags)) }
}

pub fn page_table_branch(physical: u64) -> u64 {
    ((physical >> 12) << 10) | PTE_V
}

pub fn page_table_entry(physical: u64, flags: u64) -> u64 {
    ((physical >> 12) << 10) | flags
}

pub fn page_table_physical(descriptor: u64) -> u64 {
    ((descriptor >> 10) & SATP_PPN_MASK) << 12
}

pub fn page_table_present(descriptor: u64) -> bool {
    descriptor & PTE_V != 0
}

pub fn address_space_root(physical: u64, asid: u16) -> u64 {
    SATP_MODE_SV39 | ((asid as u64) << 44) | (physical >> 12)
}

const PTE_V: u64 = 1 << 0;
const PTE_R: u64 = 1 << 1;
const PTE_W: u64 = 1 << 2;
const PTE_X: u64 = 1 << 3;
const PTE_U: u64 = 1 << 4;
const PTE_A: u64 = 1 << 6;
const PTE_D: u64 = 1 << 7;

pub fn normal_page_flags() -> u64 {
    PTE_V | PTE_R | PTE_W | PTE_U | PTE_A | PTE_D
}

pub fn user_page_flags(writable: bool, executable: bool) -> u64 {
    PTE_V
        | PTE_R
        | PTE_U
        | PTE_A
        | PTE_D
        | if writable { PTE_W } else { 0 }
        | if executable { PTE_X } else { 0 }
}

pub fn device_page_flags() -> u64 {
    PTE_V | PTE_R | PTE_W | PTE_U | PTE_A | PTE_D
}

pub fn user_page_accessible(descriptor: u64, write: bool) -> bool {
    descriptor & (PTE_V | PTE_U | PTE_R | PTE_A) == (PTE_V | PTE_U | PTE_R | PTE_A)
        && (!write || descriptor & PTE_W != 0)
}

pub const fn kernel_high_half_entry() -> u64 {
    ((0x8000_0000u64 >> 12) << 10) | 0xcf
}

pub const fn kernel_mmio_entry() -> u64 {
    0xcf
}

pub const fn kernel_pci_mmio_entry() -> u64 {
    ((0x4000_0000u64 >> 12) << 10) | 0xcf
}

pub fn validate_user_read(pointer: u64, bytes: u64) -> bool {
    let Some(end) = pointer.checked_add(bytes) else {
        return false;
    };
    bytes <= 4096 && pointer >= USER_ENTRY_VA && end <= 0x0060_0000
}

pub fn run_user_demo() {
    unsafe { enter_user_demo() }
}

pub fn run_user_fault_demo() {
    unsafe { enter_user_fault_demo() }
}

pub fn run_user_smp_demo() {
    unsafe { enter_user_smp_demo() }
}

pub fn run_rr_demo() -> preempt::Report {
    let entry = user_virtual_address(unsafe { riscv_user_rr_entry_physical() });
    let exit = user_virtual_address(unsafe { riscv_user_rr_exit_physical() });
    preempt::run(entry, exit)
}

pub fn run_ipc_demo() -> preempt::IpcReport {
    let client = user_virtual_address(unsafe { riscv_user_ipc_client_physical() });
    let server = user_virtual_address(unsafe { riscv_user_ipc_server_physical() });
    preempt::run_ipc(client, server)
}

pub(crate) fn enter_rr(entry: u64, counter: u64) {
    unsafe { enter_user_rr(entry, counter, 0, 0, 0, 0, 0, 0) }
}

pub(crate) fn prepare_rr_address_spaces() {
    unsafe {
        let root0 = (&raw const boot_root_high).cast::<u64>();
        let user_l1_0 = (&raw const boot_user_l1_high).cast::<u64>();
        let user_l2_0 = (&raw const boot_user_l2_high).cast::<u64>();
        let root1 = (&raw const boot_task1_root_high).cast_mut().cast::<u64>();
        let user_l1_1 = (&raw const boot_task1_user_l1_high)
            .cast_mut()
            .cast::<u64>();
        let user_l2_1 = (&raw const boot_task1_user_l2_high)
            .cast_mut()
            .cast::<u64>();

        core::ptr::copy_nonoverlapping(root0, root1, 512);
        core::ptr::copy_nonoverlapping(user_l1_0, user_l1_1, 512);
        core::ptr::copy_nonoverlapping(user_l2_0, user_l2_1, 512);

        root1.add(0).write_volatile(page_table_entry(
            rr_table_physical(&raw const boot_task1_user_l1_high),
            PTE_V,
        ));
        user_l1_1.add(2).write_volatile(page_table_entry(
            rr_table_physical(&raw const boot_task1_user_l2_high),
            PTE_V,
        ));
        user_l2_1.add(16).write_volatile(page_table_entry(
            rr_table_physical(&raw const boot_task1_rr_data_high),
            normal_page_flags(),
        ));
        asm!("fence rw, rw", options(nostack, preserves_flags));
    }
}

pub(crate) fn rr_data_high(task: usize) -> usize {
    if task == 0 {
        &raw const boot_rr_data_high as usize
    } else {
        &raw const boot_task1_rr_data_high as usize
    }
}

pub(crate) fn rr_satp(task: usize, asid: u16) -> u64 {
    let root = if task == 0 {
        &raw const boot_root_high
    } else {
        &raw const boot_task1_root_high
    };
    address_space_root(rr_table_physical(root), asid)
}

fn rr_table_physical(table: *const u8) -> u64 {
    table as u64 - KERNEL_OFFSET
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

pub unsafe fn enter_user(entry: u64, stack: u64, arguments: [u64; 7]) {
    unsafe { enter_user_with_stack(entry, stack, arguments.as_ptr()) }
}

pub fn shutdown(success: bool) -> ! {
    let reason = if success {
        sbi::RESET_REASON_NONE
    } else {
        sbi::RESET_REASON_SYSTEM_FAILURE
    };
    if !system_reset(sbi::RESET_TYPE_SHUTDOWN, reason).is_ok() {
        let value = if success { 0x5555u32 } else { 0x3333u32 };
        unsafe {
            core::ptr::write_volatile(phys_to_virt(0x0010_0000) as *mut u32, value);
        }
    }
    loop {
        wait_for_event();
    }
}

pub fn reboot() -> ! {
    if !sbi::system_reset(sbi::RESET_TYPE_COLD_REBOOT, sbi::RESET_REASON_NONE).is_ok() {
        unsafe {
            core::ptr::write_volatile(phys_to_virt(0x0010_0000) as *mut u32, 0x7777);
        }
    }
    loop {
        wait_for_event();
    }
}
