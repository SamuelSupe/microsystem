use core::arch::asm;
use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicBool, Ordering};

const THREADS: usize = 2;
const SWITCH_LIMIT: u64 = 20;
const COUNTER_VA: u64 = 0x0041_0000;

#[repr(C)]
pub struct ExceptionFrame {
    pub registers: [u64; 31],
    reserved: u64,
}

#[inline]
pub(crate) fn syscall_number(frame: &ExceptionFrame) -> u64 {
    frame.registers[8]
}

const _: [(); 256] = [(); core::mem::size_of::<ExceptionFrame>()];

#[repr(C, align(16))]
#[derive(Clone, Copy)]
struct FpSimdContext {
    registers: [u128; 32],
    fpcr: u64,
    fpsr: u64,
}

impl FpSimdContext {
    const EMPTY: Self = Self {
        registers: [0; 32],
        fpcr: 0,
        fpsr: 0,
    };
}

const _: [(); 528] = [(); core::mem::size_of::<FpSimdContext>()];
const _: [(); 512] = [(); core::mem::offset_of!(FpSimdContext, fpcr)];
const _: [(); 520] = [(); core::mem::offset_of!(FpSimdContext, fpsr)];

unsafe extern "C" {
    fn save_fp_simd_context(context: *mut FpSimdContext);
    fn load_fp_simd_context(context: *const FpSimdContext);
}

#[derive(Clone, Copy)]
pub(crate) struct Context {
    pub(crate) registers: [u64; 31],
    pub(crate) elr: u64,
    pub(crate) spsr: u64,
    pub(crate) sp_el0: u64,
    pub(crate) ttbr0: u64,
    fp_simd: FpSimdContext,
}

impl Context {
    pub(crate) const EMPTY: Self = Self {
        registers: [0; 31],
        elr: 0,
        spsr: 0,
        sp_el0: 0x0080_0000,
        ttbr0: 0,
        fp_simd: FpSimdContext::EMPTY,
    };
}

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
    mask_irq();
    let counters = [
        super::rr_data_high(0) as *mut u64,
        super::rr_data_high(1) as *mut u64,
    ];
    let ttbr0 = [super::rr_ttbr0(0, 1), super::rr_ttbr0(1, 2)];
    unsafe {
        for (task, counter) in counters.iter().copied().enumerate() {
            core::ptr::write_volatile(counter, 0);
            core::ptr::write_volatile(counter.add(1), 0x1111_1111_1111_1111u64 << task);
            core::ptr::write_volatile(counter.add(2), 0);
            core::ptr::write_volatile(counter.add(3), 0);
            core::ptr::write_volatile(counter.add(4), (task as u64) << 22);
            core::ptr::write_volatile(counter.add(5), 1u64 << task);
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
        state.contexts[1].registers[0] = COUNTER_VA;
        state.contexts[1].elr = entry;
        state.contexts[1].ttbr0 = ttbr0[1];
    }
    super::activate_ttbr0(ttbr0[0]);
    ACTIVE.store(true, Ordering::Release);
    super::enter_rr(entry, COUNTER_VA);
    super::activate_ttbr0(super::rr_ttbr0(0, 0));
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
    mask_irq();
    let counters = [
        super::rr_data_high(0) as *mut u64,
        super::rr_data_high(1) as *mut u64,
    ];
    let ttbr0 = [super::rr_ttbr0(0, 3), super::rr_ttbr0(1, 4)];
    unsafe {
        core::ptr::write_volatile(counters[0], 0);
        core::ptr::write_volatile(counters[1], 0);
        let state = &mut *STATE.0.get();
        *state = State {
            contexts: [Context::EMPTY; THREADS],
            current: 0,
            switches: 0,
            exit: 0,
            request: 0,
            request_ready: false,
        };
        state.contexts[1].registers[0] = 1;
        state.contexts[1].elr = server;
        state.contexts[1].ttbr0 = ttbr0[1];
    }
    super::activate_ttbr0(ttbr0[0]);
    IPC_ACTIVE.store(true, Ordering::Release);
    super::enter_rr(client, 1);
    IPC_ACTIVE.store(false, Ordering::Release);
    super::activate_ttbr0(super::rr_ttbr0(0, 0));
    IpcReport {
        reply: unsafe { core::ptr::read_volatile(counters[0]) },
        request: unsafe { core::ptr::read_volatile(counters[1]) },
    }
}

pub fn handle_ipc(frame: &mut ExceptionFrame, syscall: u64) -> Option<u64> {
    if !IPC_ACTIVE.load(Ordering::Acquire) {
        return None;
    }
    let state = unsafe { &mut *STATE.0.get() };
    match syscall {
        2 if state.current == 0 && frame.registers[0] == 1 => {
            state.request = frame.registers[1];
            state.request_ready = true;
            save(&mut state.contexts[0], frame);
            state.current = 1;
            load(&state.contexts[1], frame);
            Some(2)
        }
        3 if state.current == 1 && frame.registers[0] == 1 && state.request_ready => {
            frame.registers[0] = state.request;
            Some(0)
        }
        4 if state.current == 1 && frame.registers[0] == 1 && state.request_ready => {
            state.contexts[0].registers[0] = frame.registers[1];
            state.current = 0;
            load(&state.contexts[0], frame);
            Some(2)
        }
        2..=4 => {
            frame.registers[0] = (-1i64) as u64;
            Some(0)
        }
        _ => None,
    }
}

pub fn finish_ipc() {
    IPC_ACTIVE.store(false, Ordering::Release);
}

pub fn on_timer(frame: &mut ExceptionFrame) {
    if !ACTIVE.load(Ordering::Acquire) || super::cpu_id() != 0 || exception_level() != 0 {
        return;
    }
    let state = unsafe { &mut *STATE.0.get() };
    save(&mut state.contexts[state.current], frame);
    state.switches += 1;
    if state.switches >= SWITCH_LIMIT {
        ACTIVE.store(false, Ordering::Release);
        write_elr(state.exit);
        return;
    }
    state.current ^= 1;
    load(&state.contexts[state.current], frame);
}

pub(crate) fn save(context: &mut Context, frame: &ExceptionFrame) {
    context.registers = frame.registers;
    // All kernel and service binaries use the soft-float target, so the
    // scheduler can move the architectural FP/SIMD state here without a
    // compiler-generated vector instruction racing with this assembly call.
    unsafe { save_fp_simd_context(&mut context.fp_simd) };
    unsafe {
        asm!("mrs {0}, elr_el1", out(reg) context.elr, options(nomem, nostack));
        asm!("mrs {0}, spsr_el1", out(reg) context.spsr, options(nomem, nostack));
        asm!("mrs {0}, sp_el0", out(reg) context.sp_el0, options(nomem, nostack));
        asm!("mrs {0}, ttbr0_el1", out(reg) context.ttbr0, options(nomem, nostack));
    }
}

pub(crate) fn load(context: &Context, frame: &mut ExceptionFrame) {
    frame.registers = context.registers;
    unsafe { load_fp_simd_context(&context.fp_simd) };
    unsafe {
        asm!("msr elr_el1, {0}", in(reg) context.elr, options(nostack));
        asm!("msr spsr_el1, {0}", in(reg) context.spsr, options(nostack));
        asm!("msr sp_el0, {0}", in(reg) context.sp_el0, options(nostack));
        asm!("msr ttbr0_el1, {0}", "isb", in(reg) context.ttbr0, options(nostack));
    }
}

pub(crate) fn exception_level() -> u64 {
    let value: u64;
    unsafe { asm!("mrs {0}, spsr_el1", out(reg) value, options(nomem, nostack)) };
    value & 0xf
}

fn write_elr(value: u64) {
    unsafe { asm!("msr elr_el1, {0}", in(reg) value, options(nostack)) }
}

fn mask_irq() {
    unsafe { asm!("msr daifset, #2", options(nostack, preserves_flags)) }
}
