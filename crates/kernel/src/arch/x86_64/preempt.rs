use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicBool, Ordering};

pub const REG_ARG0: usize = 0;
pub const REG_ARG1: usize = 1;
pub const REG_ARG2: usize = 2;
pub const REG_ARG3: usize = 3;
pub const REG_ARG4: usize = 4;
pub const REG_ARG5: usize = 5;
pub const REG_SYSCALL: usize = 8;

const THREADS: usize = 2;
const SWITCH_LIMIT: u64 = 20;
const COUNTER_VA: u64 = 0x0041_0000;

#[repr(C)]
pub struct ExceptionFrame {
    pub registers: [u64; 15],
    pub vector: u64,
    pub error: u64,
    pub rip: u64,
    pub cs: u64,
    pub rflags: u64,
    pub rsp: u64,
    pub ss: u64,
}

impl ExceptionFrame {
    pub const fn from_user(&self) -> bool {
        self.cs & 3 == 3
    }
}

const _: [(); 176] = [(); core::mem::size_of::<ExceptionFrame>()];
const _: [(); 120] = [(); core::mem::offset_of!(ExceptionFrame, vector)];

#[repr(C, align(16))]
#[derive(Clone, Copy)]
struct FpContext([u8; 512]);

impl FpContext {
    // Intel FXSAVE layout: FCW at 0, MXCSR at 24, and XMM0 at 160.
    const EMPTY: Self = {
        let mut bytes = [0; 512];
        bytes[0] = 0x7f;
        bytes[1] = 0x03;
        bytes[24] = 0x80;
        bytes[25] = 0x1f;
        Self(bytes)
    };

    fn with_xmm0(signature: u64) -> Self {
        let mut context = Self::EMPTY;
        context.0[160..168].copy_from_slice(&signature.to_ne_bytes());
        context
    }
}

#[derive(Clone, Copy)]
pub(crate) struct Context {
    pub(crate) registers: [u64; 15],
    pub(crate) elr: u64,
    pub(crate) spsr: u64,
    pub(crate) sp_el0: u64,
    pub(crate) ttbr0: u64,
    fp: FpContext,
}

impl Context {
    pub(crate) const EMPTY: Self = Self {
        registers: [0; 15],
        elr: 0,
        spsr: 0x202,
        sp_el0: super::USER_STACK_TOP,
        ttbr0: 0,
        fp: FpContext::EMPTY,
    };
}

pub(crate) fn save(context: &mut Context, frame: &ExceptionFrame) {
    context.registers = frame.registers;
    context.elr = frame.rip;
    context.spsr = frame.rflags;
    context.sp_el0 = frame.rsp;
    context.ttbr0 = super::current_ttbr0();
    unsafe {
        core::arch::asm!(
            "fxsave64 [{}]",
            in(reg) context.fp.0.as_mut_ptr(),
            options(nostack)
        )
    };
}

pub(crate) fn load(context: &Context, frame: &mut ExceptionFrame) {
    frame.registers = context.registers;
    frame.rip = context.elr;
    frame.cs = super::USER_CODE_SELECTOR;
    frame.rflags = context.spsr | 0x200;
    frame.rsp = context.sp_el0;
    frame.ss = super::USER_DATA_SELECTOR;
    super::activate_ttbr0(context.ttbr0);
    unsafe {
        core::arch::asm!(
            "fxrstor64 [{}]",
            in(reg) context.fp.0.as_ptr(),
            options(nostack)
        )
    };
}

pub(crate) fn interrupted_user(frame: &ExceptionFrame) -> bool {
    frame.from_user()
}

pub(crate) fn syscall_number(frame: &ExceptionFrame) -> u64 {
    frame.registers[REG_SYSCALL]
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
    super::prepare_rr_address_spaces();
    let original = super::current_ttbr0();
    let counters = [
        super::rr_data_high(0) as *mut u64,
        super::rr_data_high(1) as *mut u64,
    ];
    let roots = [super::rr_root(0), super::rr_root(1)];
    unsafe {
        for (counter, signature) in counters.into_iter().zip([0x11, 0x22]) {
            core::ptr::write_volatile(counter, 0);
            core::ptr::write_volatile(counter.add(1), 0);
            core::ptr::write_volatile(counter.add(2), 0);
            core::ptr::write_volatile(counter.add(3), signature);
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
        state.contexts[1].registers[REG_ARG0] = COUNTER_VA;
        state.contexts[1].elr = entry;
        state.contexts[1].ttbr0 = roots[1];
        state.contexts[1].fp = FpContext::with_xmm0(0x22);
    }
    super::activate_ttbr0(roots[0]);
    ACTIVE.store(true, Ordering::Release);
    super::enter_rr(entry, COUNTER_VA);
    ACTIVE.store(false, Ordering::Release);
    super::activate_ttbr0(original);
    let state = unsafe { &*STATE.0.get() };
    let fp_simd_checks = unsafe {
        [
            core::ptr::read_volatile(counters[0].add(2)),
            core::ptr::read_volatile(counters[1].add(2)),
        ]
    };
    let fp_simd_mismatches = unsafe {
        [
            core::ptr::read_volatile(counters[0].add(1)),
            core::ptr::read_volatile(counters[1].add(1)),
        ]
    };
    Report {
        switches: state.switches,
        progress: unsafe {
            [
                core::ptr::read_volatile(counters[0]),
                core::ptr::read_volatile(counters[1]),
            ]
        },
        fp_simd_checks,
        fp_simd_mismatches,
        fp_simd_verified: fp_simd_checks.iter().all(|checks| *checks > 0)
            && fp_simd_mismatches == [0, 0],
    }
}

pub fn run_ipc(client: u64, server: u64) -> IpcReport {
    super::prepare_rr_address_spaces();
    let original = super::current_ttbr0();
    let roots = [super::rr_root(0), super::rr_root(1)];
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
        state.contexts[1].registers[REG_ARG0] = 1;
        state.contexts[1].elr = server;
        state.contexts[1].ttbr0 = roots[1];
    }
    super::activate_ttbr0(roots[0]);
    IPC_ACTIVE.store(true, Ordering::Release);
    super::enter_rr(client, 1);
    IPC_ACTIVE.store(false, Ordering::Release);
    super::activate_ttbr0(original);
    let state = unsafe { &*STATE.0.get() };
    IpcReport {
        request: state.request,
        reply: state.contexts[0].registers[REG_ARG0],
    }
}

pub fn handle_ipc(frame: &mut ExceptionFrame, syscall: u64) -> Option<u64> {
    if !IPC_ACTIVE.load(Ordering::Acquire) {
        return None;
    }
    let state = unsafe { &mut *STATE.0.get() };
    match syscall {
        2 if state.current == 0 && frame.registers[REG_ARG0] == 1 => {
            state.request = frame.registers[REG_ARG1];
            state.request_ready = true;
            save(&mut state.contexts[0], frame);
            state.current = 1;
            load(&state.contexts[1], frame);
            Some(2)
        }
        3 if state.current == 1 && state.request_ready => {
            frame.registers[REG_ARG0] = state.request;
            Some(0)
        }
        4 if state.current == 1 && state.request_ready => {
            state.contexts[0].registers[REG_ARG0] = frame.registers[REG_ARG1];
            state.current = 0;
            load(&state.contexts[0], frame);
            Some(2)
        }
        2..=4 => {
            frame.registers[REG_ARG0] = u64::MAX;
            Some(0)
        }
        _ => None,
    }
}

pub fn finish_ipc() {
    IPC_ACTIVE.store(false, Ordering::Release);
}

pub(crate) fn on_timer(frame: &mut ExceptionFrame) {
    if !ACTIVE.load(Ordering::Acquire) || !frame.from_user() || super::cpu_id() != 0 {
        return;
    }
    let state = unsafe { &mut *STATE.0.get() };
    save(&mut state.contexts[state.current], frame);
    state.switches += 1;
    if state.switches >= SWITCH_LIMIT {
        ACTIVE.store(false, Ordering::Release);
        frame.rip = state.exit;
        return;
    }
    state.current ^= 1;
    load(&state.contexts[state.current], frame);
}
