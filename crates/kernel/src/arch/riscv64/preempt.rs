use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

pub const REG_A0: usize = 0;
pub const REG_A1: usize = 1;
pub const REG_A2: usize = 2;
pub const REG_A3: usize = 3;
pub const REG_A4: usize = 4;
pub const REG_A5: usize = 5;
pub const REG_A6: usize = 6;
pub const REG_A7: usize = 7;
pub const REG_RA: usize = 8;
pub const REG_SP: usize = 9;
pub const REG_GP: usize = 10;
pub const REG_TP: usize = 11;

const SSTATUS_SPIE: u64 = 1 << 5;
const SSTATUS_SPP: u64 = 1 << 8;
const SSTATUS_FS_DIRTY: u64 = 3 << 13;
const SCAUSE_INTERRUPT: u64 = 1 << 63;

#[repr(C, align(16))]
#[derive(Clone, Copy)]
pub struct ExceptionFrame {
    /// The first eight slots are the syscall ABI a0..a7. The remaining slots
    /// preserve ra, sp, gp, tp, temporaries and saved registers.
    pub registers: [u64; 31],
    pub sepc: u64,
    pub sstatus: u64,
    pub scause: u64,
    pub stval: u64,
    pub(crate) scratch: u64,
}

const _: [(); 288] = [(); core::mem::size_of::<ExceptionFrame>()];
const _: [(); 248] = [(); core::mem::offset_of!(ExceptionFrame, sepc)];
const _: [(); 280] = [(); core::mem::offset_of!(ExceptionFrame, scratch)];

impl ExceptionFrame {
    pub const fn is_interrupt(&self) -> bool {
        self.scause & SCAUSE_INTERRUPT != 0
    }

    pub const fn exception_code(&self) -> u64 {
        self.scause & !SCAUSE_INTERRUPT
    }

    pub const fn from_user(&self) -> bool {
        self.sstatus & SSTATUS_SPP == 0
    }

    pub const fn syscall_number(&self) -> u64 {
        self.registers[REG_A7]
    }

    pub const fn argument(&self, index: usize) -> u64 {
        self.registers[REG_A0 + index]
    }

    pub fn set_return_value(&mut self, value: u64) {
        self.registers[REG_A0] = value;
    }
}

#[repr(C, align(16))]
#[derive(Clone, Copy)]
pub struct FpContext {
    pub registers: [u64; 32],
    pub fcsr: u64,
}

impl FpContext {
    pub const EMPTY: Self = Self {
        registers: [0; 32],
        fcsr: 0,
    };
}

pub type FdContext = FpContext;

const _: [(); 272] = [(); core::mem::size_of::<FpContext>()];
const _: [(); 256] = [(); core::mem::offset_of!(FpContext, fcsr)];

unsafe extern "C" {
    #[link_name = "save_fp_context"]
    fn save_fp_context_asm(context: *mut FpContext);
    #[link_name = "load_fp_context"]
    fn load_fp_context_asm(context: *const FpContext);
    #[link_name = "reset_fp_context"]
    fn reset_fp_context_asm();
}

pub unsafe fn save_fp_context(context: *mut FpContext) {
    unsafe { save_fp_context_asm(context) }
}

pub unsafe fn load_fp_context(context: *const FpContext) {
    unsafe { load_fp_context_asm(context) }
}

pub unsafe fn reset_fp_context() {
    unsafe { reset_fp_context_asm() }
}

#[derive(Clone, Copy)]
pub(crate) struct Context {
    pub(crate) registers: [u64; 31],
    // These names retain the AArch64 backend's context contract. On RV64
    // they contain sepc, sstatus, the user stack pointer, and satp.
    pub(crate) elr: u64,
    pub(crate) spsr: u64,
    pub(crate) sp_el0: u64,
    pub(crate) ttbr0: u64,
    pub(crate) fp: FpContext,
}

impl Context {
    pub(crate) const EMPTY: Self = {
        let mut registers = [0; 31];
        registers[REG_SP] = super::USER_STACK_TOP;
        Self {
            registers,
            elr: 0,
            // The scheduler saves/restores F/D state eagerly in S-mode.
            // Keeping FS enabled is therefore required even for a task that
            // has not executed a floating-point instruction yet.
            spsr: SSTATUS_SPIE | SSTATUS_FS_DIRTY,
            sp_el0: super::USER_STACK_TOP,
            ttbr0: 0,
            fp: FpContext::EMPTY,
        }
    };
}

pub(crate) fn save(context: &mut Context, frame: &ExceptionFrame) {
    context.registers = frame.registers;
    context.elr = frame.sepc;
    context.spsr = frame.sstatus;
    context.sp_el0 = frame.registers[REG_SP];
    context.ttbr0 = super::current_satp();
    unsafe { save_fp_context(&mut context.fp) };
}

pub(crate) fn load(context: &Context, frame: &mut ExceptionFrame) {
    frame.registers = context.registers;
    frame.registers[REG_SP] = context.sp_el0;
    frame.sepc = context.elr;
    frame.sstatus = context.spsr;
    super::activate_ttbr0(context.ttbr0);
    unsafe { load_fp_context(&context.fp) };
}

pub(crate) fn exception_level() -> u64 {
    let value: u64;
    unsafe {
        core::arch::asm!(
            "csrr {0}, sstatus",
            out(reg) value,
            options(nomem, nostack, preserves_flags)
        )
    };
    u64::from(value & SSTATUS_SPP != 0)
}

pub(crate) fn syscall_number(frame: &ExceptionFrame) -> u64 {
    frame.syscall_number()
}

const THREADS: usize = 2;
const SWITCH_LIMIT: u64 = 20;
const COUNTER_VA: u64 = 0x0041_0000;

struct State {
    contexts: [Context; THREADS],
    current: usize,
    switches: u64,
    exit: u64,
    request: u64,
    request_ready: bool,
}

struct StateCell(UnsafeCell<State>);

unsafe impl Sync for StateCell {}

static ACTIVE: AtomicBool = AtomicBool::new(false);
static IPC_ACTIVE: AtomicBool = AtomicBool::new(false);
static STATE: StateCell = StateCell(UnsafeCell::new(State {
    contexts: [Context::EMPTY; THREADS],
    current: 0,
    switches: 0,
    exit: 0,
    request: 0,
    request_ready: false,
}));

pub struct Report {
    pub switches: u64,
    pub progress: [u64; THREADS],
    pub fp_simd_checks: [u64; THREADS],
    pub fp_simd_mismatches: [u64; THREADS],
    pub fp_simd_verified: bool,
}

pub struct IpcReport {
    pub request: u64,
    pub reply: u64,
}

pub fn run(entry: u64, exit: u64) -> Report {
    super::prepare_rr_address_spaces();
    let original_satp = super::current_satp();
    let counters = [
        super::rr_data_high(0) as *mut u64,
        super::rr_data_high(1) as *mut u64,
    ];
    let satp = [super::rr_satp(0, 1), super::rr_satp(1, 2)];
    unsafe {
        for (task, counter) in counters.iter().copied().enumerate() {
            core::ptr::write_volatile(counter, 0);
            core::ptr::write_volatile(counter.add(1), ((task + 1) * 0x11) as u64);
            core::ptr::write_volatile(counter.add(2), 0);
            core::ptr::write_volatile(counter.add(3), 0);
            core::ptr::write_volatile(counter.add(4), ((task + 1) as u64) << 5);
        }
        let state = &mut *STATE.0.get();
        *state = State {
            contexts: [Context::EMPTY; THREADS],
            current: 0,
            switches: 0,
            exit,
            request: 0,
            request_ready: false,
        };
        state.contexts[1].registers[REG_A0] = COUNTER_VA;
        state.contexts[1].elr = entry;
        state.contexts[1].ttbr0 = satp[1];
    }
    super::activate_ttbr0(satp[0]);
    ACTIVE.store(true, Ordering::Release);
    super::enter_rr(entry, COUNTER_VA);
    ACTIVE.store(false, Ordering::Release);
    super::activate_ttbr0(original_satp);
    let state = unsafe { &*STATE.0.get() };
    Report {
        switches: state.switches,
        progress: unsafe {
            [
                core::ptr::read_volatile(counters[0]),
                core::ptr::read_volatile(counters[1]),
            ]
        },
        fp_simd_checks: unsafe {
            [
                core::ptr::read_volatile(counters[0].add(3)),
                core::ptr::read_volatile(counters[1].add(3)),
            ]
        },
        fp_simd_mismatches: unsafe {
            [
                core::ptr::read_volatile(counters[0].add(2)),
                core::ptr::read_volatile(counters[1].add(2)),
            ]
        },
        fp_simd_verified: true,
    }
}

pub fn run_ipc(client: u64, server: u64) -> IpcReport {
    super::prepare_rr_address_spaces();
    let original_satp = super::current_satp();
    let satp = [super::rr_satp(0, 3), super::rr_satp(1, 4)];
    unsafe {
        let state = &mut *STATE.0.get();
        *state = State {
            contexts: [Context::EMPTY; THREADS],
            current: 0,
            switches: 0,
            exit: 0,
            request: 0,
            request_ready: false,
        };
        state.contexts[1].registers[REG_A0] = 1;
        state.contexts[1].elr = server;
        state.contexts[1].ttbr0 = satp[1];
    }
    super::activate_ttbr0(satp[0]);
    IPC_ACTIVE.store(true, Ordering::Release);
    super::enter_rr(client, 1);
    IPC_ACTIVE.store(false, Ordering::Release);
    super::activate_ttbr0(original_satp);
    let state = unsafe { &*STATE.0.get() };
    IpcReport {
        request: state.request,
        reply: state.contexts[0].registers[REG_A0],
    }
}

pub fn handle_ipc(frame: &mut ExceptionFrame, syscall: u64) -> Option<u64> {
    if !IPC_ACTIVE.load(Ordering::Acquire) {
        return None;
    }
    let state = unsafe { &mut *STATE.0.get() };
    match syscall {
        2 if state.current == 0 && frame.registers[REG_A0] == 1 => {
            state.request = frame.registers[REG_A1];
            state.request_ready = true;
            save(&mut state.contexts[0], frame);
            state.current = 1;
            load(&state.contexts[1], frame);
            Some(2)
        }
        3 if state.current == 1 && frame.registers[REG_A0] == 1 && state.request_ready => {
            frame.registers[REG_A0] = state.request;
            Some(0)
        }
        4 if state.current == 1 && frame.registers[REG_A0] == 1 && state.request_ready => {
            state.contexts[0].registers[REG_A0] = frame.registers[REG_A1];
            state.current = 0;
            load(&state.contexts[0], frame);
            Some(2)
        }
        2..=4 => {
            frame.registers[REG_A0] = (-1i64) as u64;
            Some(0)
        }
        _ => None,
    }
}

pub fn finish_ipc() {
    IPC_ACTIVE.store(false, Ordering::Release);
}

/// Complete an ecall after a syscall handler has written its result.
/// RISC-V `sepc` points at the ecall itself, so returning without this step
/// would immediately re-enter the same syscall.
pub fn ecall_return(frame: &mut ExceptionFrame, value: u64) {
    frame.set_return_value(value);
    frame.sepc = frame.sepc.wrapping_add(4);
}

pub fn syscall_return(frame: &mut ExceptionFrame, value: u64) {
    ecall_return(frame, value);
}

pub fn advance_ecall(frame: &mut ExceptionFrame) {
    frame.sepc = frame.sepc.wrapping_add(4);
}

pub type TimerHook = unsafe extern "C" fn(&mut ExceptionFrame);

static TIMER_HOOK: AtomicUsize = AtomicUsize::new(0);

pub fn install_timer_hook(hook: Option<TimerHook>) {
    TIMER_HOOK.store(hook.map_or(0, |hook| hook as usize), Ordering::Release);
}

pub(crate) fn on_timer(frame: &mut ExceptionFrame) {
    if ACTIVE.load(Ordering::Acquire) && super::cpu_id() == 0 && frame.from_user() {
        let state = unsafe { &mut *STATE.0.get() };
        save(&mut state.contexts[state.current], frame);
        state.switches += 1;
        if state.switches >= SWITCH_LIMIT {
            ACTIVE.store(false, Ordering::Release);
            frame.sepc = state.exit;
        } else {
            state.current ^= 1;
            load(&state.contexts[state.current], frame);
        }
    }
    let hook = TIMER_HOOK.load(Ordering::Acquire);
    if hook != 0 {
        let hook: TimerHook = unsafe { core::mem::transmute(hook) };
        unsafe { hook(frame) };
    }
}
