use core::arch::{asm, global_asm};
use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

pub(crate) mod interrupt;
pub(crate) mod preempt;

pub use interrupt::{
    bind_device_irq, clock_nanos, device_interrupts, init_interrupts, interrupt_diagnostics,
    send_reschedule, timer_ticks,
};

global_asm!(include_str!("boot.S"), options(att_syntax));
global_asm!(include_str!("exception.S"), options(att_syntax));

pub const KERNEL_OFFSET: u64 = 0xffff_8000_0000_0000;
pub const USER_ENTRY_VA: u64 = 0x0040_0000;
pub const USER_STACK_TOP: u64 = 0x0080_0000;
pub const USER_CODE_SELECTOR: u64 = 0x1b;
pub const USER_DATA_SELECTOR: u64 = 0x23;

const PAGE_PRESENT: u64 = 1;
const PAGE_WRITE: u64 = 1 << 1;
const PAGE_USER: u64 = 1 << 2;
const PAGE_WRITE_THROUGH: u64 = 1 << 3;
const PAGE_CACHE_DISABLE: u64 = 1 << 4;
const PAGE_NO_EXECUTE: u64 = 1 << 63;
static LOCAL_APIC: AtomicU64 = AtomicU64::new(0xfee0_0000);
static CPU_APIC_IDS: [AtomicU32; 2] = [AtomicU32::new(0), AtomicU32::new(1)];

pub fn local_apic_base() -> u64 { LOCAL_APIC.load(Ordering::Acquire) }
pub fn hardware_cpu_id(base: u64) -> u32 { unsafe { core::ptr::read_volatile((phys_to_virt(base) + 0x20) as *const u32) >> 24 } }
pub fn configure_topology(base: u64, bsp: u32, second: Option<u32>) {
    LOCAL_APIC.store(base, Ordering::Release);
    CPU_APIC_IDS[0].store(bsp, Ordering::Release);
    CPU_APIC_IDS[1].store(second.unwrap_or(u32::MAX), Ordering::Release);
}
pub fn cpu_apic_id(cpu: usize) -> Option<u32> { CPU_APIC_IDS.get(cpu).map(|id| id.load(Ordering::Acquire)).filter(|id| *id <= 255) }

unsafe extern "C" {
    static boot_pdpt_high: u8;
    static boot_user_pml4_high: u8;
    static boot_user_pdpt_high: u8;
    static boot_user_pd_high: u8;
    static boot_user_pt_high: u8;
    static boot_task1_pml4_high: u8;
    static boot_task1_pdpt_high: u8;
    static boot_task1_pd_high: u8;
    static boot_task1_pt_high: u8;
    static boot_rr_data_high: u8;
    static boot_task1_rr_data_high: u8;
    static boot_user_stack_high: u8;
    static boot_task1_stack_high: u8;
    static user_entry_high: u8;
    static user_rr_entry_high: u8;
    static user_rr_exit_high: u8;
    static user_ipc_client_high: u8;
    static user_ipc_server_high: u8;
    static ap_trampoline_high: u8;
    static ap_trampoline_end_high: u8;
    fn enter_user_demo();
    fn enter_user_fault_demo();
    fn enter_user_smp_demo();
    fn enter_user_rr(entry: u64, argument0: u64);
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
    fn enter_user_with_stack(entry: u64, stack: u64, arguments: *const u64);
    fn x86_int80();
    fn x86_timer();
    fn x86_reschedule();
    fn x86_device();
    fn x86_page_fault();
    fn x86_general_protection();
    fn x86_double_fault();
    fn x86_invalid_tss();
    fn x86_segment_not_present();
    fn x86_stack_segment_fault();
    fn x86_alignment_check();
    fn x86_control_protection();
    fn x86_vmm_communication();
    fn x86_security_exception();
    fn x86_invalid_opcode();
    fn x86_spurious();
    fn x86_default_exception();
    fn x86_reload_segments();
}

#[repr(C, packed)]
#[derive(Clone, Copy)]
struct IdtEntry {
    offset_low: u16,
    selector: u16,
    ist: u8,
    attributes: u8,
    offset_mid: u16,
    offset_high: u32,
    zero: u32,
}

impl IdtEntry {
    const MISSING: Self = Self {
        offset_low: 0,
        selector: 0,
        ist: 0,
        attributes: 0,
        offset_mid: 0,
        offset_high: 0,
        zero: 0,
    };

    fn interrupt(handler: unsafe extern "C" fn(), user: bool) -> Self {
        Self::interrupt_on_stack(handler, user, 0)
    }

    fn interrupt_on_stack(handler: unsafe extern "C" fn(), user: bool, ist: u8) -> Self {
        let address = handler as usize as u64;
        Self {
            offset_low: address as u16,
            selector: 0x08,
            ist: ist & 0x7,
            attributes: if user { 0xee } else { 0x8e },
            offset_mid: (address >> 16) as u16,
            offset_high: (address >> 32) as u32,
            zero: 0,
        }
    }
}

#[repr(C, packed)]
struct DescriptorPointer {
    limit: u16,
    base: u64,
}

#[repr(C, packed)]
#[derive(Clone, Copy)]
struct TaskStateSegment {
    reserved0: u32,
    rsp: [u64; 3],
    reserved1: u64,
    ist: [u64; 7],
    reserved2: u64,
    reserved3: u16,
    iomap_base: u16,
}

impl TaskStateSegment {
    const EMPTY: Self = Self {
        reserved0: 0,
        rsp: [0; 3],
        reserved1: 0,
        ist: [0; 7],
        reserved2: 0,
        reserved3: 0,
        iomap_base: core::mem::size_of::<Self>() as u16,
    };
}

struct IdtCell(UnsafeCell<[IdtEntry; 256]>);
unsafe impl Sync for IdtCell {}
struct TssCell(UnsafeCell<[TaskStateSegment; 2]>);
unsafe impl Sync for TssCell {}
struct GdtCell(UnsafeCell<[[u64; 7]; 2]>);
unsafe impl Sync for GdtCell {}

static IDT: IdtCell = IdtCell(UnsafeCell::new([IdtEntry::MISSING; 256]));
static TSS: TssCell = TssCell(UnsafeCell::new([TaskStateSegment::EMPTY; 2]));
const GDT_TEMPLATE: [u64; 7] = [
    0,
    0x00af_9a00_0000_ffff,
    0x00af_9200_0000_ffff,
    0x00af_fa00_0000_ffff,
    0x00af_f200_0000_ffff,
    0,
    0,
];
static GDT: GdtCell = GdtCell(UnsafeCell::new([GDT_TEMPLATE; 2]));

#[repr(C, align(16))]
struct KernelStacks([[u8; 64 * 1024]; 2]);
struct StackCell(UnsafeCell<KernelStacks>);
unsafe impl Sync for StackCell {}
static KERNEL_STACKS: StackCell = StackCell(UnsafeCell::new(KernelStacks([[0; 64 * 1024]; 2])));
#[repr(C, align(16))]
struct DoubleFaultStacks([[u8; 16 * 1024]; 2]);
struct DoubleFaultStackCell(UnsafeCell<DoubleFaultStacks>);
unsafe impl Sync for DoubleFaultStackCell {}
static DOUBLE_FAULT_STACKS: DoubleFaultStackCell =
    DoubleFaultStackCell(UnsafeCell::new(DoubleFaultStacks([[0; 16 * 1024]; 2])));
static USER_TABLES_READY: AtomicBool = AtomicBool::new(false);
static IDT_READY: AtomicBool = AtomicBool::new(false);

pub fn initialize_boot_cpu() {
    initialize_user_tables();
}

pub fn install_vectors() {
    let cpu = cpu_id().min(1);
    unsafe {
        let idt = &mut *IDT.0.get();
        if cpu == 0 {
            idt.fill(IdtEntry::interrupt(x86_default_exception, false));
            idt[6] = IdtEntry::interrupt(x86_invalid_opcode, false);
            idt[8] = IdtEntry::interrupt_on_stack(x86_double_fault, false, 1);
            idt[10] = IdtEntry::interrupt(x86_invalid_tss, false);
            idt[11] = IdtEntry::interrupt(x86_segment_not_present, false);
            idt[12] = IdtEntry::interrupt(x86_stack_segment_fault, false);
            idt[13] = IdtEntry::interrupt(x86_general_protection, false);
            idt[14] = IdtEntry::interrupt(x86_page_fault, false);
            idt[17] = IdtEntry::interrupt(x86_alignment_check, false);
            idt[21] = IdtEntry::interrupt(x86_control_protection, false);
            idt[29] = IdtEntry::interrupt(x86_vmm_communication, false);
            idt[30] = IdtEntry::interrupt(x86_security_exception, false);
            idt[0x20] = IdtEntry::interrupt(x86_timer, false);
            idt[0x31] = IdtEntry::interrupt(x86_device, false);
            idt[0x80] = IdtEntry::interrupt(x86_int80, true);
            idt[0xf1] = IdtEntry::interrupt(x86_reschedule, false);
            idt[0xff] = IdtEntry::interrupt(x86_spurious, false);
            IDT_READY.store(true, Ordering::Release);
        } else {
            while !IDT_READY.load(Ordering::Acquire) {
                core::hint::spin_loop();
            }
        }

        let stack = core::ptr::addr_of_mut!((*KERNEL_STACKS.0.get()).0[cpu]) as *mut u8;
        let double_fault_stack =
            core::ptr::addr_of_mut!((*DOUBLE_FAULT_STACKS.0.get()).0[cpu]) as *mut u8;
        let tss = &mut (*TSS.0.get())[cpu];
        tss.rsp[0] = stack.add(64 * 1024) as u64;
        tss.ist[0] = double_fault_stack.add(16 * 1024) as u64;

        let tss_address = tss as *mut TaskStateSegment as u64;
        let limit = (core::mem::size_of::<TaskStateSegment>() - 1) as u64;
        let gdt = &mut (*GDT.0.get())[cpu];
        gdt[5] = limit
            | ((tss_address & 0x00ff_ffff) << 16)
            | (0x89u64 << 40)
            | ((limit & 0x000f_0000) << 32)
            | ((tss_address & 0xff00_0000) << 32);
        gdt[6] = tss_address >> 32;
        let gdt_pointer = DescriptorPointer {
            limit: (core::mem::size_of_val(gdt) - 1) as u16,
            base: gdt.as_ptr() as u64,
        };
        let idt_pointer = DescriptorPointer {
            limit: (core::mem::size_of_val(idt) - 1) as u16,
            base: idt.as_ptr() as u64,
        };
        asm!("lgdt [{}]", in(reg) &gdt_pointer, options(readonly, nostack, preserves_flags));
        x86_reload_segments();
        asm!("ltr {0:x}", in(reg) 0x28u16, options(nostack, preserves_flags));
        asm!("lidt [{}]", in(reg) &idt_pointer, options(readonly, nostack, preserves_flags));
    }
}

#[unsafe(no_mangle)]
extern "C" fn x86_handle_trap(frame: &mut preempt::ExceptionFrame) -> u64 {
    match frame.vector as u8 {
        8 => {
            crate::uart::emergency_write(b"[panic] x86 double fault\n");
            shutdown(false)
        }
        0x20 | 0x31 | 0xf1 | 0xff => {
            crate::rust_irq(frame);
            0
        }
        0x80 => crate::rust_lower_sync(frame, 0x80, frame.rip, 0),
        vector if frame.from_user() => {
            let fault = if vector == 14 {
                read_cr2()
            } else {
                frame.error
            };
            crate::rust_lower_sync(frame, vector as u64, frame.rip, fault)
        }
        vector => {
            let kernel_stack = (frame as *const preempt::ExceptionFrame as *const u8)
                .wrapping_add(core::mem::offset_of!(preempt::ExceptionFrame, rsp))
                .cast::<u64>();
            let stack_words = unsafe {
                [
                    kernel_stack.read(),
                    kernel_stack.add(1).read(),
                    kernel_stack.add(2).read(),
                    kernel_stack.add(3).read(),
                ]
            };
            crate::kprintln!(
                "[x86] kernel trap vector={:#x} error={:#x} rip={:#x} cs={:#x} rflags={:#x} frame={:#x} stack=[{:#x},{:#x},{:#x},{:#x}]",
                vector,
                frame.error,
                frame.rip,
                frame.cs,
                frame.rflags,
                frame as *const preempt::ExceptionFrame as usize,
                stack_words[0],
                stack_words[1],
                stack_words[2],
                stack_words[3]
            );
            crate::rust_exception(
                vector as u64,
                frame.rip,
                if vector == 14 {
                    read_cr2()
                } else {
                    frame.error
                },
            )
        }
    }
}

pub fn cpu_id() -> usize {
    let id = hardware_cpu_id(local_apic_base());
    CPU_APIC_IDS.iter().position(|candidate| candidate.load(Ordering::Acquire) == id).unwrap_or(0)
}

pub fn psci_cpu_on(cpu: u64) -> i64 {
    if cpu != 1 {
        return -1;
    }
    let Some(target) = cpu_apic_id(cpu as usize) else { return -1; };
    let source = &raw const ap_trampoline_high;
    let bytes = &raw const ap_trampoline_end_high as usize - source as usize;
    if bytes == 0 || bytes > 4096 {
        return -1;
    }
    unsafe {
        core::ptr::copy_nonoverlapping(source, phys_to_virt(0x8000) as *mut u8, bytes);
        dma_write_barrier();
        if !send_startup_ipi(target as usize, 0x0000_c500)
            || !send_startup_ipi(target as usize, 0x0000_8500)
            || !send_startup_ipi(target as usize, 0x0000_4608)
        {
            return -1;
        }
        for _ in 0..100_000 {
            core::hint::spin_loop();
        }
        if !send_startup_ipi(1, 0x0000_4608) {
            return -1;
        }
    }
    0
}

pub fn wait_for_event() {
    core::hint::spin_loop();
}

pub fn send_event() {
    for cpu in 0..2 {
        send_reschedule(cpu);
    }
}

pub fn counter() -> u64 {
    unsafe { core::arch::x86_64::_rdtsc() }
}

pub fn invalidate_user_page(_asid: u16, virtual_address: u64) {
    unsafe { asm!("invlpg [{}]", in(reg) virtual_address, options(nostack, preserves_flags)) }
}

pub fn invalidate_user_asid(_asid: u16) {
    let root = current_ttbr0();
    activate_ttbr0(root);
    send_event();
}

pub fn activate_ttbr0(value: u64) {
    unsafe { asm!("mov cr3, {0}", in(reg) value, options(nostack, preserves_flags)) }
}

pub fn current_ttbr0() -> u64 {
    let value: u64;
    unsafe { asm!("mov {0}, cr3", out(reg) value, options(nomem, nostack, preserves_flags)) };
    value
}

pub const fn phys_to_virt(physical: u64) -> usize {
    KERNEL_OFFSET.wrapping_add(physical) as usize
}

pub const fn virt_to_phys(virtual_address: u64) -> Option<u64> {
    if virtual_address >= KERNEL_OFFSET {
        Some(virtual_address - KERNEL_OFFSET)
    } else {
        None
    }
}

pub fn dma_barrier() {
    unsafe { asm!("mfence", options(nostack, preserves_flags)) }
}

pub fn dma_write_barrier() {
    unsafe { asm!("sfence", options(nostack, preserves_flags)) }
}

pub fn dma_read_barrier() {
    unsafe { asm!("lfence", options(nostack, preserves_flags)) }
}

pub fn is_syscall(cause: u64) -> bool {
    cause == 0x80
}

pub fn flush_icache() {
    unsafe { asm!("mfence", options(nostack, preserves_flags)) }
}

pub const fn page_table_branch(physical: u64) -> u64 {
    (physical & !0xfff) | PAGE_PRESENT | PAGE_WRITE | PAGE_USER
}

pub const fn page_table_entry(physical: u64, flags: u64) -> u64 {
    (physical & 0x000f_ffff_ffff_f000) | flags
}

pub const fn page_table_physical(descriptor: u64) -> u64 {
    descriptor & 0x000f_ffff_ffff_f000
}

pub const fn page_table_present(descriptor: u64) -> bool {
    descriptor & PAGE_PRESENT != 0
}

pub const fn address_space_root(physical: u64, _asid: u16) -> u64 {
    physical & !0xfff
}

pub const fn normal_page_flags() -> u64 {
    PAGE_PRESENT | PAGE_WRITE | PAGE_USER | PAGE_NO_EXECUTE
}

pub const fn user_page_flags(writable: bool, executable: bool) -> u64 {
    PAGE_PRESENT
        | PAGE_USER
        | if writable { PAGE_WRITE } else { 0 }
        | if executable { 0 } else { PAGE_NO_EXECUTE }
}

pub const fn device_page_flags() -> u64 {
    PAGE_PRESENT
        | PAGE_WRITE
        | PAGE_USER
        | PAGE_WRITE_THROUGH
        | PAGE_CACHE_DISABLE
        | PAGE_NO_EXECUTE
}

pub const fn user_page_accessible(descriptor: u64, write: bool) -> bool {
    descriptor & (PAGE_PRESENT | PAGE_USER) == PAGE_PRESENT | PAGE_USER
        && (!write || descriptor & PAGE_WRITE != 0)
}

pub fn kernel_high_half_entry() -> u64 {
    page_table_branch(symbol_physical(&raw const boot_pdpt_high))
}

pub const fn kernel_mmio_entry() -> u64 {
    0
}

pub const fn kernel_pci_mmio_entry() -> u64 {
    0
}

pub fn validate_user_read(pointer: u64, bytes: u64) -> bool {
    let Some(end) = pointer.checked_add(bytes) else {
        return false;
    };
    if bytes > 4096 || pointer < USER_ENTRY_VA || end > 0x0060_0000 {
        return false;
    }
    walk_user_page(pointer).is_some() && (bytes == 0 || walk_user_page(end - 1).is_some())
}

pub fn run_user_demo() {
    let original = current_ttbr0();
    activate_ttbr0(rr_root(0));
    unsafe { enter_user_demo() };
    activate_ttbr0(original);
}

pub fn run_user_fault_demo() {
    let original = current_ttbr0();
    activate_ttbr0(rr_root(0));
    unsafe { enter_user_fault_demo() };
    activate_ttbr0(original);
}

pub fn run_user_smp_demo() {
    let original = current_ttbr0();
    activate_ttbr0(rr_root(cpu_id().min(1)));
    unsafe { enter_user_smp_demo() };
    activate_ttbr0(original);
}

pub fn run_rr_demo() -> preempt::Report {
    let base = &raw const user_entry_high as usize;
    preempt::run(
        (USER_ENTRY_VA as usize + (&raw const user_rr_entry_high as usize - base)) as u64,
        (USER_ENTRY_VA as usize + (&raw const user_rr_exit_high as usize - base)) as u64,
    )
}

pub fn run_ipc_demo() -> preempt::IpcReport {
    let base = &raw const user_entry_high as usize;
    preempt::run_ipc(
        (USER_ENTRY_VA as usize + (&raw const user_ipc_client_high as usize - base)) as u64,
        (USER_ENTRY_VA as usize + (&raw const user_ipc_server_high as usize - base)) as u64,
    )
}

pub(crate) fn enter_rr(entry: u64, counter: u64) {
    unsafe { enter_user_rr(entry, counter) }
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

pub(crate) fn rr_data_high(task: usize) -> usize {
    if task == 0 {
        &raw const boot_rr_data_high as usize
    } else {
        &raw const boot_task1_rr_data_high as usize
    }
}

pub(crate) fn rr_root(task: usize) -> u64 {
    let root = if task == 0 {
        &raw const boot_user_pml4_high
    } else {
        &raw const boot_task1_pml4_high
    };
    symbol_physical(root)
}

pub(crate) fn prepare_rr_address_spaces() {
    initialize_user_tables();
}

pub fn shutdown(success: bool) -> ! {
    let value = if success { 0x10u32 } else { 0x11u32 };
    unsafe { asm!("out dx, eax", in("dx") 0xf4u16, in("eax") value, options(nomem, nostack)) }
    loop {
        unsafe { asm!("cli", "hlt", options(nomem, nostack)) }
    }
}

pub fn reboot() -> ! {
    unsafe { asm!("out 0x64, al", in("al") 0xfeu8, options(nomem, nostack)) }
    loop {
        unsafe { asm!("cli", "hlt", options(nomem, nostack)) }
    }
}

fn initialize_user_tables() {
    if USER_TABLES_READY.swap(true, Ordering::AcqRel) {
        return;
    }
    unsafe {
        initialize_user_root(
            &raw const boot_user_pml4_high,
            &raw const boot_user_pdpt_high,
            &raw const boot_user_pd_high,
            &raw const boot_user_pt_high,
            &raw const boot_rr_data_high,
            &raw const boot_user_stack_high,
        );
        initialize_user_root(
            &raw const boot_task1_pml4_high,
            &raw const boot_task1_pdpt_high,
            &raw const boot_task1_pd_high,
            &raw const boot_task1_pt_high,
            &raw const boot_task1_rr_data_high,
            &raw const boot_task1_stack_high,
        );
    }
}

unsafe fn initialize_user_root(
    pml4: *const u8,
    pdpt: *const u8,
    pd: *const u8,
    pt: *const u8,
    rr_data: *const u8,
    stack: *const u8,
) {
    let pml4 = pml4.cast_mut().cast::<u64>();
    let pdpt = pdpt.cast_mut().cast::<u64>();
    let pd = pd.cast_mut().cast::<u64>();
    let pt = pt.cast_mut().cast::<u64>();
    unsafe {
        core::ptr::write_bytes(pml4, 0, 512);
        core::ptr::write_bytes(pdpt, 0, 512);
        core::ptr::write_bytes(pd, 0, 512);
        core::ptr::write_bytes(pt, 0, 512);
        pml4.add(0)
            .write_volatile(page_table_branch(symbol_physical(pdpt.cast())));
        pml4.add(256)
            .write_volatile(page_table_branch(symbol_physical(
                &raw const boot_pdpt_high,
            )));
        pdpt.add(0)
            .write_volatile(page_table_branch(symbol_physical(pd.cast())));
        pd.add(2)
            .write_volatile(page_table_branch(symbol_physical(pt.cast())));
        let code = symbol_physical(&raw const user_entry_high) & !0xfff;
        for index in 0..8 {
            pt.add(index).write_volatile(page_table_entry(
                code + index as u64 * 4096,
                user_page_flags(false, true),
            ));
        }
        pt.add(16).write_volatile(page_table_entry(
            symbol_physical(rr_data),
            normal_page_flags(),
        ));
        pt.add(511).write_volatile(page_table_entry(
            symbol_physical(stack),
            normal_page_flags(),
        ));
        dma_write_barrier();
    }
}

fn symbol_physical(symbol: *const u8) -> u64 {
    symbol as u64 - KERNEL_OFFSET
}

fn walk_user_page(virtual_address: u64) -> Option<u64> {
    let indexes = [
        (virtual_address >> 39) & 0x1ff,
        (virtual_address >> 30) & 0x1ff,
        (virtual_address >> 21) & 0x1ff,
        (virtual_address >> 12) & 0x1ff,
    ];
    let mut table = current_ttbr0() & !0xfff;
    for index in indexes {
        let descriptor = unsafe {
            core::ptr::read_volatile((phys_to_virt(table) as *const u64).add(index as usize))
        };
        if !page_table_present(descriptor) || descriptor & PAGE_USER == 0 {
            return None;
        }
        table = page_table_physical(descriptor);
    }
    Some(table)
}

fn read_cr2() -> u64 {
    let value: u64;
    unsafe { asm!("mov {0}, cr2", out(reg) value, options(nomem, nostack, preserves_flags)) };
    value
}

unsafe fn send_startup_ipi(cpu: usize, command: u32) -> bool {
    let apic = phys_to_virt(local_apic_base());
    unsafe {
        core::ptr::write_volatile((apic + 0x310) as *mut u32, (cpu as u32) << 24);
        core::ptr::write_volatile((apic + 0x300) as *mut u32, command);
        for _ in 0..1_000_000 {
            if core::ptr::read_volatile((apic + 0x300) as *const u32) & (1 << 12) == 0 {
                return true;
            }
            core::hint::spin_loop();
        }
    }
    false
}
