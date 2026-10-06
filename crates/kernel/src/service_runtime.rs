use core::cell::UnsafeCell;
use core::mem;
use core::ptr;
use core::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, AtomicU64, AtomicUsize, Ordering};

use crate::arch::selected as arch;
use crate::arch::selected::preempt::{self, Context, ExceptionFrame};
use crate::bootfs::Archive;
use crate::device_control;
use crate::kernel_heap;
use crate::pci::UserTransportGrant;
use crate::physical_memory;
use crate::smmu;
use microsystem_abi::{
    ABI_VERSION, CapHandle, MESSAGE_CAP_MOVE_MASK, MESSAGE_CAPS, Message, ObjectType, Rights,
    Status, SystemControlOperation, ThreadLaunchV1, ThreadLaunchV2, message_cap_move,
};
use microsystem_kernel::capability::{Capability, CapabilityTable, MAX_CAPABILITIES};
use microsystem_kernel::elf::{ElfImage, USER_MIN};
use microsystem_kernel::ipc::ReplySlot;

#[path = "virtual_memory.rs"]
mod virtual_memory;

pub fn handle_virtual_memory(frame: &mut ExceptionFrame, syscall: u64) -> Option<u64> {
    virtual_memory::handle(frame, syscall)
}

pub fn handle_page_fault(
    frame: &ExceptionFrame,
    cause: u64,
    address: u64,
) -> Option<Result<(), Status>> {
    virtual_memory::fault(frame, cause, address)
}

pub const SERVICE_COUNT: usize = microsystem_abi::service::NAMES.len();
pub const SERVICE_NAMES: [&str; SERVICE_COUNT] = microsystem_abi::service::NAMES;
const APPLICATION_START: usize = SERVICE_COUNT;
const APPLICATION_COUNT: usize = microsystem_abi::process::MAX_APPLICATIONS;
const PCI_ACTIVATION_IDLE: u8 = 0;
const PCI_ACTIVATION_STARTING: u8 = 1;
const PCI_ACTIVATION_READY: u8 = 2;
const PCI_ACTIVATION_FAILED: u8 = 3;
const CPU_COUNT: usize = 2;
const IDLE_START: usize = APPLICATION_START + APPLICATION_COUNT;
const TASK_COUNT: usize = IDLE_START + CPU_COUNT;

#[cfg(target_arch = "x86_64")]
const IMAGE_BYTES: usize = 0x100000;
#[cfg(not(target_arch = "x86_64"))]
const IMAGE_BYTES: usize = 0xe0000;
const HEAP_BYTES: usize = 0x100000;
const HEAP_START: u64 = 0x0068_0000;
const LARGE_HEAP_BYTES: usize = 0x2000000;
const LARGE_HEAP_START: u64 = 0x0080_0000;
const LARGE_HEAP_END: u64 = LARGE_HEAP_START + LARGE_HEAP_BYTES as u64;
const LARGE_HEAP_TABLES: usize = LARGE_HEAP_BYTES / 0x20_0000;
const STACK_BYTES: usize = 16 * 4096;
const STACK_TOP: u64 = 0x0080_0000;
const ASID_BASE: u16 = 0x20;
const ENDPOINT_COUNT: usize = 29;
const ENDPOINT_OWNERS: [usize; ENDPOINT_COUNT] = [
    1, 2, 3, 0, 5, 2, 6, 6, 6, 7, 8, 9, 11, 11, 0, 6, 6, 6, 6, 6, 6, 6, 6, 6, 0, 12, 0, 0, 0,
];
const BLOCK_TASK: usize = 2;
const CONSOLE_TASK: usize = 1;
const SHELL_TASK: usize = 4;
const DEVMGR_TASK: usize = 5;
const WINDOWD_TASK: usize = 6;
const TERMINAL_TASK: usize = 7;
const FILES_TASK: usize = 8;
const MONITOR_TASK: usize = 9;
const SSHD_TASK: usize = 10;
const NETD_TASK: usize = 11;
const DB_TASK: usize = 12;
const BLOCK_COMMON_VA: u64 = 0x005a_0000;
const BLOCK_DEVICE_VA: u64 = 0x005a_1000;
const BLOCK_NOTIFY_VA: u64 = 0x005a_2000;
const BLOCK_ISR_VA: u64 = 0x005a_3000;
const BLOCK_QUEUE_VA: u64 = 0x005b_0000;
const SHARED_DATA_VA: u64 = 0x005c_0000;
const FILESYSTEM_DATA_VA: u64 = 0x005e_0000;
const SSH_FILESYSTEM_DATA_VA: u64 = 0x005f_0000;
const WINDOWD_FILESYSTEM_DATA_VA: u64 = 0x0060_0000;
const TERMINAL_FILESYSTEM_DATA_VA: u64 = 0x0061_0000;
const DATABASE_FILESYSTEM_DATA_VA: u64 = 0x0062_0000;
const ROOT_FILESYSTEM_DATA_VA: u64 = 0x0063_0000;
const NETWORK_FILESYSTEM_DATA_VA: u64 = 0x0064_0000;
const CONSOLE_UART_VA: u64 = 0x005d_0000;
const DEVMGR_PCI_CONFIG_VA: u64 = 0x005e_0000;
const DEVMGR_ECAM_SCAN_VA: u64 = 0x0042_0000;
const DEVMGR_ECAM_SLOTS: usize = 32;
const MFS_TASK: usize = 3;
const SHARED_FRAME_OBJECT: u32 = 1;
const FILESYSTEM_FRAME_OBJECT: u32 = 13;
const SSH_FILESYSTEM_FRAME_OBJECT: u32 = 14;
const WINDOWD_FILESYSTEM_FRAME_OBJECT: u32 = 15;
const TERMINAL_FILESYSTEM_FRAME_OBJECT: u32 = 16;
const DATABASE_FILESYSTEM_FRAME_OBJECT: u32 = 17;
const ROOT_FILESYSTEM_FRAME_OBJECT: u32 = 18;
const NETWORK_FILESYSTEM_FRAME_OBJECT: u32 = 19;
const IDENTITY_SNAPSHOT_OBJECT: u32 = 20;
const DEVICE_NOTIFICATION_OBJECT: u32 = 2;
const SHARED_FRAME_NODE: u32 = 1;
const FILESYSTEM_FRAME_NODE: u32 = 8;
const SSH_FILESYSTEM_FRAME_NODE: u32 = 33;
const WINDOWD_FILESYSTEM_FRAME_NODE: u32 = 43;
const TERMINAL_FILESYSTEM_FRAME_NODE: u32 = 45;
const DATABASE_FILESYSTEM_FRAME_NODE: u32 = 47;
const DEVICE_NOTIFICATION_NODE: u32 = 2;
const DEVICE_IRQ_OBJECT: u32 = 37;
const DEVICE_IRQ_NODE: u32 = 3;
const DEVICE_MMIO_OBJECT: u32 = 10;
const DEVICE_MMIO_NODE: u32 = 4;
const DEVICE_QUEUE_OBJECT: u32 = 11;
const DEVICE_QUEUE_NODE: u32 = 5;
const DMA_DOMAIN_OBJECT: u32 = 12;
const DMA_DOMAIN_NODE: u32 = 6;
const ROOT_MEMORY_POOL_OBJECT: u32 = 50;
const ROOT_MEMORY_POOL_NODE: u32 = 7;
const DRIVER_MEMORY_POOL_OBJECT: u32 = 51;
const DRIVER_MEMORY_POOL_NODE: u32 = 9;
const MEMORY_POOL_QUOTA: usize = 4;
const MAX_MEMORY_POOLS: usize = 16;
const MAX_MEMORY_POOL_QUOTA: usize = 2048;
const MAX_DYNAMIC_NOTIFICATIONS: usize = 16;
const RUNTIME_FRAME_OBJECT_BASE: u32 = 200;
const RUNTIME_FRAME_COUNT: usize = 4096;
const FRAME_REGION_COUNT: usize = 32;
const FRAME_REGION_MAX_PAGES: usize = 2048;
const GUI_MEMORY_POOL_OBJECT: u32 = 52;
const GUI_MEMORY_POOL_NODE: u32 = 18;
const GUI_MEMORY_POOL_QUOTA: usize = 1536;
const SCRIPT_MEMORY_POOL_OBJECT: u32 = 53;
const SCRIPT_MEMORY_POOL_NODE: u32 = 28;
const SCRIPT_MEMORY_POOL_QUOTA: usize = 264;
const NETWORK_MEMORY_POOL_OBJECT: u32 = 54;
const NETWORK_MEMORY_POOL_NODE: u32 = 29;
const NETWORK_MEMORY_POOL_QUOTA: usize = 128;
const DYNAMIC_MAP_START: u64 = 0x0058_0000;
const DYNAMIC_MAP_END: u64 = 0x005b_0000;
const CONSOLE_ENDPOINT_NODE: u32 = 10;
const BLOCK_ENDPOINT_NODE: u32 = 11;
const FILESYSTEM_ENDPOINT_NODE: u32 = 12;
const PROCESS_ENDPOINT_NODE: u32 = 13;
const DEVMGR_ENDPOINT_NODE: u32 = 14;
const BLOCK_CONFIG_ENDPOINT_NODE: u32 = 15;
const TIME_ENDPOINT_NODE: u32 = 17;
const GUI_TERMINAL_ENDPOINT_NODE: u32 = 19;
const GUI_FILES_ENDPOINT_NODE: u32 = 20;
const GUI_MONITOR_ENDPOINT_NODE: u32 = 21;
const GUI_TERMINAL_EVENTS_NODE: u32 = 23;
const GUI_FILES_EVENTS_NODE: u32 = 24;
const GUI_MONITOR_EVENTS_NODE: u32 = 25;
const NETWORK_DEVICE_NODE: u32 = 26;
const RANDOM_SOURCE_NODE: u32 = 27;
const GUI_CONFIG_ENDPOINT_NODE: u32 = 34;
const GUI_DYNAMIC_ENDPOINT_NODE_BASE: u32 = 35;
const GUI_LAUNCH_ENDPOINT_NODE: u32 = 44;
const DATABASE_ENDPOINT_NODE: u32 = 46;
const TERMINAL_CANCEL_OBJECT: u32 = 95;
const TERMINAL_CANCEL_NODE: u32 = 48;
const SERVICE_ENDPOINT_NODE: u32 = 49;
const ROOT_FILESYSTEM_FRAME_NODE: u32 = 50;
const APPLICATION_ENDPOINT_NODE: u32 = 51;
const NETWORK_FILESYSTEM_FRAME_NODE: u32 = 52;
const IDENTITY_SNAPSHOT_NODE: u32 = 53;
const IDENTITY_ENDPOINT_NODE: u32 = 54;

struct NativeScratch(UnsafeCell<[u8; microsystem_abi::application::MAX_IMAGE_BYTES]>);
unsafe impl Sync for NativeScratch {}
static NATIVE_SCRATCH: NativeScratch = NativeScratch(UnsafeCell::new(
    [0; microsystem_abi::application::MAX_IMAGE_BYTES],
));

struct NativeLaunch {
    image: &'static [u8],
    pages: u32,
    permissions: u64,
}

#[repr(C, align(4096))]
struct TaskMemory {
    #[cfg(target_arch = "x86_64")]
    level0: [u64; 512],
    level1: [u64; 512],
    level2: [u64; 512],
    level3: [u64; 512],
    level3_high: [u64; 512],
    large_heap_level3: [[u64; 512]; LARGE_HEAP_TABLES],
    image: [u8; IMAGE_BYTES],
    stack: [u8; STACK_BYTES],
}

#[derive(Clone, Copy)]
struct Start {
    entry: u64,
    stack: u64,
    ttbr0: u64,
}

impl Start {
    const EMPTY: Self = Self {
        entry: 0,
        stack: STACK_TOP,
        ttbr0: 0,
    };
}

struct MemoryCell(UnsafeCell<[*mut TaskMemory; TASK_COUNT]>);
struct StartCell(UnsafeCell<[Start; TASK_COUNT]>);
struct StateCell(UnsafeCell<State>);
struct GrantCell(UnsafeCell<Option<UserTransportGrant>>);
struct CapCell(UnsafeCell<[CapabilityTable; TASK_COUNT]>);
struct BootCapsCell(UnsafeCell<[CapabilityTable; SERVICE_COUNT]>);
struct StateGuard;

#[derive(Clone, Copy)]
struct FrameRegion {
    object: u32,
    pool: u32,
    pages: u16,
    frames: [u32; FRAME_REGION_MAX_PAGES],
}

impl FrameRegion {
    const EMPTY: Self = Self {
        object: 0,
        pool: 0,
        pages: 0,
        frames: [0; FRAME_REGION_MAX_PAGES],
    };
}

struct RegionCell(UnsafeCell<[FrameRegion; FRAME_REGION_COUNT]>);

unsafe impl Sync for RegionCell {}

unsafe impl Sync for MemoryCell {}
unsafe impl Sync for StartCell {}
unsafe impl Sync for StateCell {}
unsafe impl Sync for GrantCell {}
unsafe impl Sync for CapCell {}
unsafe impl Sync for BootCapsCell {}

impl Drop for StateGuard {
    fn drop(&mut self) {
        STATE_LOCK_OWNER.fetch_add(1, Ordering::Release);
    }
}

struct State {
    contexts: [Context; TASK_COUNT],
    threads: [ThreadState; TASK_COUNT],
    endpoints: [Endpoint; ENDPOINT_COUNT],
    reply_to: [Option<usize>; TASK_COUNT],
    requests: [Message; TASK_COUNT],
    async_replies: [ReplySlot; TASK_COUNT],
    services: [microsystem_abi::service::InfoV1; SERVICE_COUNT],
    current: [usize; CPU_COUNT],
    last_accounted_ticks: [u64; CPU_COUNT],
    applications: [ApplicationSlot; APPLICATION_COUNT],
}

#[derive(Clone, Copy)]
struct ApplicationSlot {
    program: u64,
    profile: u16,
    exit_status: i64,
    cpu_mask: u32,
    cpu_ticks: [u64; CPU_COUNT],
    reclaimed_frames: u32,
    reclaimed_pools: u32,
    reclaimed_mappings: u32,
}

impl ApplicationSlot {
    const EMPTY: Self = Self {
        program: 0,
        profile: microsystem_abi::THREAD_PROFILE_APPLICATION,
        exit_status: 0,
        cpu_mask: 0,
        cpu_ticks: [0; CPU_COUNT],
        reclaimed_frames: 0,
        reclaimed_pools: 0,
        reclaimed_mappings: 0,
    };
}

#[derive(Clone, Copy, Default)]
struct ResourceCleanup {
    frames: u32,
    pools: u32,
    mappings: u32,
}

#[derive(Clone, Copy)]
enum ThreadState {
    Runnable,
    Running {
        cpu: usize,
    },
    Exited,
    Sending {
        endpoint: usize,
        reply_buffer: u64,
        deadline: u64,
    },
    Receiving {
        endpoint: usize,
        buffer: u64,
        deadline: u64,
    },
    WaitingReply {
        server: usize,
        reply_buffer: u64,
        deadline: u64,
    },
    WaitingNotification {
        object: u32,
        deadline: u64,
    },
    Terminating {
        cpu: usize,
        status: i64,
    },
}

#[derive(Clone, Copy)]
struct Endpoint {
    pending: Option<usize>,
}

#[derive(Clone, Copy)]
struct CapTransfer {
    message: Message,
    sender: usize,
    receiver: usize,
    sources: [CapHandle; MESSAGE_CAPS],
    destinations: [CapHandle; MESSAGE_CAPS],
    inserted: [bool; MESSAGE_CAPS],
    moved: [bool; MESSAGE_CAPS],
}

impl Endpoint {
    const EMPTY: Self = Self { pending: None };
}

static MEMORY: MemoryCell = MemoryCell(UnsafeCell::new([ptr::null_mut(); TASK_COUNT]));
static STARTS: StartCell = StartCell(UnsafeCell::new([Start::EMPTY; TASK_COUNT]));
static STATE: StateCell = StateCell(UnsafeCell::new(State {
    contexts: [Context::EMPTY; TASK_COUNT],
    threads: [ThreadState::Runnable; TASK_COUNT],
    endpoints: [Endpoint::EMPTY; ENDPOINT_COUNT],
    reply_to: [None; TASK_COUNT],
    requests: [Message::new(0, 0); TASK_COUNT],
    async_replies: [ReplySlot::Idle; TASK_COUNT],
    services: [microsystem_abi::service::InfoV1::EMPTY; SERVICE_COUNT],
    current: [0; CPU_COUNT],
    last_accounted_ticks: [0; CPU_COUNT],
    applications: [ApplicationSlot::EMPTY; APPLICATION_COUNT],
}));
static PREPARED: AtomicBool = AtomicBool::new(false);
static BOOT_CAPS: BootCapsCell = BootCapsCell(UnsafeCell::new(
    [const { CapabilityTable::new() }; SERVICE_COUNT],
));
static ACTIVE_CPUS: AtomicU32 = AtomicU32::new(0);
static READY: AtomicU32 = AtomicU32::new(0);
static ONLINE: AtomicU32 = AtomicU32::new(0);
static SERVICE_STATUS_REPORTED: AtomicBool = AtomicBool::new(false);
static SWITCHES: AtomicU64 = AtomicU64::new(0);
static CPU_SWITCHES: [AtomicU64; CPU_COUNT] = [AtomicU64::new(0), AtomicU64::new(0)];
static CPU_NON_IDLE_RUNS: [AtomicU64; CPU_COUNT] = [AtomicU64::new(0), AtomicU64::new(0)];
static SMP_REPORTED: AtomicBool = AtomicBool::new(false);
static STATE_LOCK_NEXT: AtomicU32 = AtomicU32::new(0);
static STATE_LOCK_OWNER: AtomicU32 = AtomicU32::new(0);
static BLOCK_GRANT: GrantCell = GrantCell(UnsafeCell::new(None));
static PCI_ACTIVATION_STATE: AtomicU8 = AtomicU8::new(PCI_ACTIVATION_IDLE);
static CONSOLE_UART: AtomicU64 = AtomicU64::new(0);
static CAPS: CapCell = CapCell(UnsafeCell::new(
    [const { CapabilityTable::new() }; TASK_COUNT],
));
static NEXT_OBJECT: AtomicU32 = AtomicU32::new(100);
static NEXT_DERIVATION: AtomicU32 = AtomicU32::new(1000);
static DEVICE_NOTIFICATION: AtomicU64 = AtomicU64::new(0);
static DEVICE_IRQ_BOUND: AtomicBool = AtomicBool::new(false);
static DEVICE_IRQ_ACKS: AtomicU64 = AtomicU64::new(0);
static NOTIFICATION_BLOCKS: AtomicU64 = AtomicU64::new(0);
static RESCHEDULE_WAKEUPS: AtomicU64 = AtomicU64::new(0);
static IPC_CALLS: AtomicU64 = AtomicU64::new(0);
static GUI_PRESENT_REPORTED: AtomicBool = AtomicBool::new(false);
static GUI_PARTIAL_PRESENT_REPORTED: AtomicBool = AtomicBool::new(false);
const SERIAL_SERVICE_MASK: u32 = ((1u32 << 6) - 1) | (1u32 << SSHD_TASK) | (1u32 << NETD_TASK);
static EXPECTED_SERVICE_MASK: AtomicU32 = AtomicU32::new(SERIAL_SERVICE_MASK);
static DMA_AUTHORIZED: AtomicU32 = AtomicU32::new(0);
static DMA_FRAME_OBJECTS: [AtomicU32; 128] = [const { AtomicU32::new(0) }; 128];
static RUNTIME_FRAME_PHYSICAL: [AtomicU64; RUNTIME_FRAME_COUNT] =
    [const { AtomicU64::new(0) }; RUNTIME_FRAME_COUNT];
static RUNTIME_FRAME_POOL: [AtomicU32; RUNTIME_FRAME_COUNT] =
    [const { AtomicU32::new(0) }; RUNTIME_FRAME_COUNT];
static FRAME_REGIONS: RegionCell =
    RegionCell(UnsafeCell::new([FrameRegion::EMPTY; FRAME_REGION_COUNT]));
static MEMORY_POOL_OBJECTS: [AtomicU32; MAX_MEMORY_POOLS] =
    [const { AtomicU32::new(0) }; MAX_MEMORY_POOLS];
static MEMORY_POOL_QUOTAS: [AtomicU32; MAX_MEMORY_POOLS] =
    [const { AtomicU32::new(0) }; MAX_MEMORY_POOLS];
static NOTIFICATION_OBJECTS: [AtomicU32; MAX_DYNAMIC_NOTIFICATIONS] =
    [const { AtomicU32::new(0) }; MAX_DYNAMIC_NOTIFICATIONS];
static NOTIFICATION_BITS: [AtomicU64; MAX_DYNAMIC_NOTIFICATIONS] =
    [const { AtomicU64::new(0) }; MAX_DYNAMIC_NOTIFICATIONS];
static DTB_FRAME_REPORTED: AtomicBool = AtomicBool::new(false);
static MEMORY_POOLS_SEEN: AtomicU32 = AtomicU32::new(0);
static MEMORY_POOLS_REPORTED: AtomicBool = AtomicBool::new(false);
static FILESYSTEM_FRAME_PHYSICAL: AtomicU64 = AtomicU64::new(0);
static SSH_FILESYSTEM_FRAME_PHYSICAL: AtomicU64 = AtomicU64::new(0);
static WINDOWD_FILESYSTEM_FRAME_PHYSICAL: AtomicU64 = AtomicU64::new(0);
static TERMINAL_FILESYSTEM_FRAME_PHYSICAL: AtomicU64 = AtomicU64::new(0);
static DATABASE_FILESYSTEM_FRAME_PHYSICAL: AtomicU64 = AtomicU64::new(0);
static ROOT_FILESYSTEM_FRAME_PHYSICAL: AtomicU64 = AtomicU64::new(0);
static NETWORK_FILESYSTEM_FRAME_PHYSICAL: AtomicU64 = AtomicU64::new(0);
static IDENTITY_SNAPSHOT_PHYSICAL: AtomicU64 = AtomicU64::new(0);
static KERNEL_HEAP_REPORTED: AtomicBool = AtomicBool::new(false);
static BOOTFS_ADDRESS: AtomicUsize = AtomicUsize::new(0);
static BOOTFS_LENGTH: AtomicUsize = AtomicUsize::new(0);

fn lock_state() -> StateGuard {
    let ticket = STATE_LOCK_NEXT.fetch_add(1, Ordering::Relaxed);
    while STATE_LOCK_OWNER.load(Ordering::Acquire) != ticket {
        core::hint::spin_loop();
    }
    StateGuard
}

fn current_cpu() -> usize {
    arch::cpu_id().min(CPU_COUNT - 1)
}

fn current_task(state: &State) -> usize {
    state.current[current_cpu()]
}

fn task_memory(task: usize) -> Option<&'static TaskMemory> {
    let pointer = *unsafe { &*MEMORY.0.get() }.get(task)?;
    unsafe { pointer.as_ref() }
}

fn task_memory_mut(task: usize) -> Option<&'static mut TaskMemory> {
    let pointer = *unsafe { &*MEMORY.0.get() }.get(task)?;
    unsafe { pointer.as_mut() }
}

fn idle_task(cpu: usize) -> usize {
    IDLE_START + cpu
}

fn is_idle_task(task: usize) -> bool {
    (IDLE_START..IDLE_START + CPU_COUNT).contains(&task)
}

fn set_running(state: &mut State, cpu: usize, task: usize) {
    let now = arch::timer_ticks(cpu);
    let previous = state.current[cpu];
    let elapsed = now.saturating_sub(state.last_accounted_ticks[cpu]);
    if let Some(index) = previous.checked_sub(APPLICATION_START)
        && index < APPLICATION_COUNT
    {
        state.applications[index].cpu_ticks[cpu] =
            state.applications[index].cpu_ticks[cpu].saturating_add(elapsed);
    }
    state.last_accounted_ticks[cpu] = now;
    state.current[cpu] = task;
    state.threads[task] = ThreadState::Running { cpu };
    if let Some(index) = task.checked_sub(APPLICATION_START)
        && index < APPLICATION_COUNT
    {
        state.applications[index].cpu_mask |= 1 << cpu;
    }
    if !is_idle_task(task) {
        CPU_NON_IDLE_RUNS[cpu].fetch_add(1, Ordering::Relaxed);
    }
}

pub fn grant_console_uart(physical: u64) {
    CONSOLE_UART.store(physical, Ordering::Release);
}

pub fn prepare(bootfs: &'static [u8]) -> Result<(), ()> {
    PREPARED.store(false, Ordering::Release);
    ACTIVE_CPUS.store(0, Ordering::Release);
    READY.store(0, Ordering::Release);
    ONLINE.store(0, Ordering::Release);
    SERVICE_STATUS_REPORTED.store(false, Ordering::Release);
    SWITCHES.store(0, Ordering::Release);
    BOOTFS_ADDRESS.store(bootfs.as_ptr() as usize, Ordering::Release);
    BOOTFS_LENGTH.store(bootfs.len(), Ordering::Release);
    unsafe { *BLOCK_GRANT.0.get() = None };
    PCI_ACTIVATION_STATE.store(PCI_ACTIVATION_IDLE, Ordering::Release);

    let init = image_by_name(bootfs, SERVICE_NAMES[0]).ok_or(())?;
    load_task(0, init)?;
    let idle = Archive::new(bootfs)
        .find_map(|entry| match entry {
            Ok(entry) if entry.name == "bin/idle" => Some(entry.data),
            _ => None,
        })
        .ok_or(())?;
    for cpu in 0..CPU_COUNT {
        load_task(idle_task(cpu), idle)?;
    }

    arch::flush_icache();
    PREPARED.store(true, Ordering::Release);
    Ok(())
}

pub fn run() -> ! {
    while !PREPARED.load(Ordering::Acquire) {
        arch::wait_for_event();
    }
    let starts = unsafe { &*STARTS.0.get() };
    let _guard = lock_state();
    let state = unsafe { &mut *STATE.0.get() };
    // This frame remains below every exception until the CPU shuts down. A
    // whole-State temporary would consume most of the 64 KiB kernel stack.
    state.threads.fill(ThreadState::Exited);
    state.endpoints.fill(Endpoint::EMPTY);
    state.reply_to.fill(None);
    state.requests.fill(Message::new(0, 0));
    state.async_replies.fill(ReplySlot::Idle);
    state.services.fill(microsystem_abi::service::InfoV1::EMPTY);
    state.current.fill(0);
    state.last_accounted_ticks = [arch::timer_ticks(0), arch::timer_ticks(1)];
    state.applications.fill(ApplicationSlot::EMPTY);
    for (context, start) in state.contexts.iter_mut().zip(starts) {
        *context = Context::EMPTY;
        context.elr = start.entry;
        context.sp_el0 = start.stack;
        context.ttbr0 = start.ttbr0;
    }
    state.threads[0] = ThreadState::Runnable;
    for cpu in 0..CPU_COUNT {
        state.threads[idle_task(cpu)] = ThreadState::Runnable;
    }
    set_running(state, 1, 0);
    for table in unsafe { &mut *CAPS.0.get() } {
        *table = CapabilityTable::new();
    }
    NEXT_OBJECT.store(100, Ordering::Release);
    NEXT_DERIVATION.store(1000, Ordering::Release);
    DEVICE_NOTIFICATION.store(0, Ordering::Release);
    DEVICE_IRQ_BOUND.store(false, Ordering::Release);
    DEVICE_IRQ_ACKS.store(0, Ordering::Release);
    NOTIFICATION_BLOCKS.store(0, Ordering::Release);
    RESCHEDULE_WAKEUPS.store(0, Ordering::Release);
    IPC_CALLS.store(0, Ordering::Release);
    GUI_PRESENT_REPORTED.store(false, Ordering::Release);
    GUI_PARTIAL_PRESENT_REPORTED.store(false, Ordering::Release);
    DMA_AUTHORIZED.store(0, Ordering::Release);
    for object in &DMA_FRAME_OBJECTS {
        object.store(0, Ordering::Release);
    }
    reset_runtime_frames();
    unsafe { *FRAME_REGIONS.0.get() = [FrameRegion::EMPTY; FRAME_REGION_COUNT] };
    if reset_memory_pools().is_err() {
        arch::shutdown(true);
    }
    reset_notifications();
    reset_filesystem_frame();
    DTB_FRAME_REPORTED.store(false, Ordering::Release);
    MEMORY_POOLS_SEEN.store(0, Ordering::Release);
    MEMORY_POOLS_REPORTED.store(false, Ordering::Release);
    for switches in &CPU_SWITCHES {
        switches.store(0, Ordering::Release);
    }
    for runs in &CPU_NON_IDLE_RUNS {
        runs.store(0, Ordering::Release);
    }
    SMP_REPORTED.store(false, Ordering::Release);
    if install_boot_capabilities().is_err() {
        arch::shutdown(true);
    }
    for task in 0..SERVICE_COUNT {
        unsafe { (&mut *BOOT_CAPS.0.get())[task] = (&*CAPS.0.get())[task].clone() };
        state.services[task].pid = (task + 1) as u32;
        state.services[task].program = program_id(SERVICE_NAMES[task]);
    }
    state.services[0].starts = 1;
    state.services[0].state = microsystem_abi::service::STARTING;
    state.services[0].started_ns = arch::clock_nanos();
    arch::activate_ttbr0(starts[0].ttbr0);
    ACTIVE_CPUS.fetch_or(1 << 1, Ordering::Release);
    let registers = state.contexts[0].registers;
    drop(_guard);
    arch::enter_service(
        starts[0].entry,
        [
            registers[0],
            registers[1],
            registers[2],
            registers[3],
            registers[4],
            registers[5],
            registers[6],
        ],
    );
    ACTIVE_CPUS.fetch_and(!(1 << 1), Ordering::Release);
    arch::shutdown(false)
}

pub fn join_primary() -> ! {
    while ACTIVE_CPUS.load(Ordering::Acquire) & (1 << 1) == 0 {
        arch::wait_for_event();
    }
    let starts = unsafe { &*STARTS.0.get() };
    let guard = lock_state();
    let state = unsafe { &mut *STATE.0.get() };
    let task = next_runnable(state, IDLE_START, 0).unwrap_or_else(|| idle_task(0));
    set_running(state, 0, task);
    let context = state.contexts[task];
    ACTIVE_CPUS.fetch_or(1, Ordering::Release);
    drop(guard);
    arch::activate_ttbr0(starts[task].ttbr0);
    arch::enter_service(context.elr, context.registers[..7].try_into().unwrap());
    ACTIVE_CPUS.fetch_and(!1, Ordering::Release);
    arch::shutdown(false)
}

pub fn handle_yield(frame: &mut ExceptionFrame) -> Option<u64> {
    if !active_on_current_cpu() {
        return None;
    }
    let _guard = lock_state();
    wake_notifications();
    switch(frame);
    Some(2)
}

pub fn handle_ipc(frame: &mut ExceptionFrame, syscall: u64) -> Option<u64> {
    if !active_on_current_cpu() || !matches!(syscall, 2..=4 | 32 | 33) {
        return None;
    }
    if matches!(syscall, 2 | 32) {
        IPC_CALLS.fetch_add(1, Ordering::Relaxed);
    }
    let _guard = lock_state();
    Some(match syscall {
        2 => call(frame),
        3 => receive(frame),
        4 => reply_receive(frame),
        32 => try_call(frame),
        33 => poll_reply(frame),
        _ => unreachable!(),
    })
}

pub fn handle_capability(frame: &mut ExceptionFrame, syscall: u64) -> Option<u64> {
    if !active_on_current_cpu() || !(6..=10).contains(&syscall) {
        return None;
    }
    let _guard = lock_state();
    let task = current_task(unsafe { &*STATE.0.get() });
    let result = match syscall {
        6 => unsafe { &mut (&mut *CAPS.0.get())[task] }
            .copy_cap_with_node(
                CapHandle(frame.registers[0] as u32),
                Rights(frame.registers[1] as u32),
                allocate_derivation(),
            )
            .map(|handle| handle.0 as u64),
        7 => unsafe { &mut (&mut *CAPS.0.get())[task] }
            .move_cap(CapHandle(frame.registers[0] as u32))
            .map(|handle| handle.0 as u64),
        8 => delete_capability(task, CapHandle(frame.registers[0] as u32)).map(|()| 0),
        9 => revoke_global(task, CapHandle(frame.registers[0] as u32)).map(|count| count as u64),
        10 if task == 0
            || matches!(
                frame.registers[0],
                value if value == ObjectType::Frame as u64
                    || value == ObjectType::FrameRegion as u64
            ) =>
        {
            create_object(
                task,
                frame.registers[0],
                Rights(frame.registers[1] as u32),
                frame.registers[2],
                frame.registers[3],
            )
            .map(|handle| handle.0 as u64)
        }
        10 => Err(Status::AccessDenied),
        _ => unreachable!(),
    };
    frame.registers[0] = match result {
        Ok(value) => value,
        Err(status) => status as i64 as u64,
    };
    Some(0)
}

pub fn handle_frame_mapping(frame: &mut ExceptionFrame, syscall: u64) -> Option<u64> {
    if !active_on_current_cpu() || !(11..=12).contains(&syscall) {
        return None;
    }
    let _guard = lock_state();
    let task = current_task(unsafe { &*STATE.0.get() });
    let handle = CapHandle(frame.registers[0] as u32);
    let capability = unsafe { &(&*CAPS.0.get())[task] }.capability(handle, Rights::MAP);
    let Ok(capability) = capability else {
        frame.registers[0] = Status::BadCapability as i64 as u64;
        return Some(0);
    };
    let (pages, region) = match capability.object_type {
        ObjectType::Frame => (1usize, None),
        ObjectType::FrameRegion => {
            let Some(region) = frame_region(capability.object) else {
                frame.registers[0] = Status::BadCapability as i64 as u64;
                return Some(0);
            };
            (region.pages as usize, Some(region))
        }
        _ => {
            frame.registers[0] = Status::BadCapability as i64 as u64;
            return Some(0);
        }
    };
    if capability.object_type == ObjectType::Frame
        && runtime_frame_physical(capability.object).is_none()
    {
        frame.registers[0] = Status::BadCapability as i64 as u64;
        return Some(0);
    }
    let address = frame.registers[1];
    let bytes = (pages as u64).saturating_mul(4096);
    let end = address.checked_add(bytes);
    let valid_single = pages == 1
        && ((DYNAMIC_MAP_START..DYNAMIC_MAP_END).contains(&address)
            || (task == WINDOWD_TASK
                && address >= LARGE_HEAP_START
                && end.is_some_and(|end| end <= LARGE_HEAP_END)));
    let mica_region = matches!(task, 0 | MFS_TASK | SHELL_TASK | SSHD_TASK | NETD_TASK)
        || task
            .checked_sub(APPLICATION_START)
            .and_then(|index| unsafe { (&*STATE.0.get()).applications.get(index) })
            .is_some_and(|application| {
                application.profile == microsystem_abi::THREAD_PROFILE_MICA
                    && address >= DYNAMIC_MAP_START
                    && end.is_some_and(|end| end <= DYNAMIC_MAP_END)
            });
    let valid_region = pages > 1
        && ((task == WINDOWD_TASK
            && address >= LARGE_HEAP_START
            && end.is_some_and(|end| end <= LARGE_HEAP_END))
            || mica_region);
    if address % 4096 != 0 || !(valid_single || valid_region) {
        frame.registers[0] = Status::Invalid as i64 as u64;
        return Some(0);
    }
    let Some(memory) = task_memory_mut(task) else {
        frame.registers[0] = Status::Fault as i64 as u64;
        return Some(0);
    };
    let result = (|| -> Result<(), Status> {
        if syscall == 11 {
            let requested = Rights(frame.registers[2] as u32);
            let allowed = Rights(Rights::READ.0 | Rights::WRITE.0 | Rights::EXECUTE.0);
            if requested.0 == 0
                || requested.0 & !allowed.0 != 0
                || !capability.rights.contains(requested)
            {
                Err(Status::AccessDenied)
            } else if requested.contains(Rights::WRITE) && requested.contains(Rights::EXECUTE) {
                Err(Status::AccessDenied)
            } else if (0..pages).any(|page| {
                user_descriptor(memory, address + page as u64 * 4096)
                    .is_some_and(|descriptor| arch::page_table_present(descriptor))
            }) {
                Err(Status::Busy)
            } else {
                for page in 0..pages {
                    let physical = region
                        .as_ref()
                        .and_then(|region| runtime_frame_physical(region.frames[page]))
                        .or_else(|| runtime_frame_physical(capability.object))
                        .ok_or(Status::BadCapability)?;
                    *user_descriptor_mut(memory, address + page as u64 * 4096)
                        .ok_or(Status::Invalid)? = arch::page_table_entry(
                        physical,
                        user_page_flags(
                            requested.contains(Rights::WRITE),
                            requested.contains(Rights::EXECUTE),
                        ),
                    );
                }
                Ok(())
            }
        } else {
            for page in 0..pages {
                let expected = region
                    .as_ref()
                    .and_then(|region| runtime_frame_physical(region.frames[page]))
                    .or_else(|| runtime_frame_physical(capability.object))
                    .ok_or(Status::BadCapability)?;
                let descriptor =
                    user_descriptor(memory, address + page as u64 * 4096).ok_or(Status::Invalid)?;
                if !arch::page_table_present(descriptor) {
                    return Err(Status::NotFound);
                }
                if arch::page_table_physical(descriptor) != expected {
                    return Err(Status::AccessDenied);
                }
            }
            for page in 0..pages {
                *user_descriptor_mut(memory, address + page as u64 * 4096)
                    .ok_or(Status::Invalid)? = 0;
            }
            Ok(())
        }
    })();
    if result.is_ok() {
        arch::invalidate_user_asid(ASID_BASE + task as u16);
    }
    frame.registers[0] = match result {
        Ok(()) => Status::Ok as i64 as u64,
        Err(status) => status as i64 as u64,
    };
    Some(0)
}

pub fn handle_notification(frame: &mut ExceptionFrame, syscall: u64) -> Option<u64> {
    if !active_on_current_cpu() || !matches!(syscall, 5 | 23 | 34) {
        return None;
    }
    let _guard = lock_state();
    let task = current_task(unsafe { &*STATE.0.get() });
    let required = if matches!(syscall, 5 | 34) {
        Rights::READ
    } else {
        Rights::WRITE
    };
    let notification = unsafe { &(&*CAPS.0.get())[task] }
        .capability(CapHandle(frame.registers[0] as u32), required);
    let Ok(notification) = notification else {
        frame.registers[0] = Status::AccessDenied as i64 as u64;
        return Some(0);
    };
    if notification.object_type != ObjectType::Notification {
        frame.registers[0] = Status::AccessDenied as i64 as u64;
        return Some(0);
    }
    if syscall == 23 {
        let bits = frame.registers[1];
        if notification.object == DEVICE_NOTIFICATION_OBJECT {
            frame.registers[0] = Status::AccessDenied as i64 as u64;
        } else if bits == 0 {
            frame.registers[0] = Status::Invalid as i64 as u64;
        } else if signal_notification(notification.object, bits).is_none() {
            frame.registers[0] = Status::BadCapability as i64 as u64;
        } else {
            frame.registers[0] = Status::Ok as i64 as u64;
        }
        return Some(0);
    }
    if notification.object == DEVICE_NOTIFICATION_OBJECT && task != BLOCK_TASK {
        frame.registers[0] = Status::AccessDenied as i64 as u64;
        return Some(0);
    }
    if syscall == 5 && deadline_elapsed(frame.registers[1]) {
        frame.registers[0] = Status::TimedOut as i64 as u64;
        return Some(0);
    }
    let Some(bits) = take_notification_bits(notification.object) else {
        frame.registers[0] = Status::BadCapability as i64 as u64;
        return Some(0);
    };
    if bits != 0 {
        frame.registers[0] = bits;
        return Some(0);
    }

    if syscall == 34 {
        frame.registers[0] = Status::Busy as i64 as u64;
        return Some(0);
    }

    let cpu = current_cpu();
    let state = unsafe { &mut *STATE.0.get() };
    let Some(next) = next_other_runnable(state, task, cpu) else {
        frame.registers[0] = Status::Busy as i64 as u64;
        return Some(0);
    };
    preempt::save(&mut state.contexts[task], frame);
    state.threads[task] = ThreadState::WaitingNotification {
        object: notification.object,
        deadline: frame.registers[1],
    };
    set_running(state, cpu, next);
    NOTIFICATION_BLOCKS.fetch_add(1, Ordering::Relaxed);
    preempt::load(&state.contexts[next], frame);
    record_switch(cpu);
    Some(2)
}

pub fn handle_irq_control(frame: &mut ExceptionFrame, syscall: u64) -> Option<u64> {
    if !active_on_current_cpu() || !(14..=15).contains(&syscall) {
        return None;
    }
    let _guard = lock_state();
    let task = current_task(unsafe { &*STATE.0.get() });
    if task != BLOCK_TASK {
        frame.registers[0] = Status::AccessDenied as i64 as u64;
        return Some(0);
    }
    let table = unsafe { &(&*CAPS.0.get())[task] };
    let irq = table.capability(CapHandle(frame.registers[0] as u32), Rights::ACK);
    if !matches!(irq, Ok(capability) if capability.object_type == ObjectType::Irq && capability.object == DEVICE_IRQ_OBJECT)
    {
        frame.registers[0] = Status::BadCapability as i64 as u64;
        return Some(0);
    }
    if syscall == 14 {
        let notification = table.capability(CapHandle(frame.registers[1] as u32), Rights::READ);
        if !matches!(notification, Ok(capability) if capability.object_type == ObjectType::Notification && capability.object == DEVICE_NOTIFICATION_OBJECT)
        {
            frame.registers[0] = Status::BadCapability as i64 as u64;
            return Some(0);
        }
        DEVICE_IRQ_BOUND.store(true, Ordering::Release);
    } else {
        if !DEVICE_IRQ_BOUND.load(Ordering::Acquire) {
            frame.registers[0] = Status::Invalid as i64 as u64;
            return Some(0);
        }
        let acknowledgements = DEVICE_IRQ_ACKS.fetch_add(1, Ordering::Relaxed) + 1;
        if acknowledgements == 1 {
            crate::kprintln!(
                "[irq] virtio-blk INTx completions={}",
                arch::device_interrupts()
            );
        }
    }
    frame.registers[0] = Status::Ok as i64 as u64;
    Some(0)
}

pub fn handle_dma_control(frame: &mut ExceptionFrame, syscall: u64) -> Option<u64> {
    if !active_on_current_cpu() || !(16..=17).contains(&syscall) {
        return None;
    }
    let _guard = lock_state();
    let task = current_task(unsafe { &*STATE.0.get() });
    if task != BLOCK_TASK {
        frame.registers[0] = Status::AccessDenied as i64 as u64;
        return Some(0);
    }
    let table = unsafe { &(&*CAPS.0.get())[task] };
    let domain = table.capability(CapHandle(frame.registers[0] as u32), Rights::MAP);
    if !matches!(domain, Ok(capability) if capability.object_type == ObjectType::DmaDomain && capability.object == DMA_DOMAIN_OBJECT)
    {
        frame.registers[0] = Status::BadCapability as i64 as u64;
        return Some(0);
    }
    let Some(grant) = (unsafe { *BLOCK_GRANT.0.get() }) else {
        frame.registers[0] = Status::Invalid as i64 as u64;
        return Some(0);
    };
    if syscall == 16 {
        let iova = frame.registers[2];
        let Some(index) = iova
            .checked_sub(grant.dma_iova)
            .filter(|offset| offset.is_multiple_of(4096))
            .map(|offset| offset / 4096)
            .filter(|index| *index < 32)
        else {
            frame.registers[0] = Status::AccessDenied as i64 as u64;
            return Some(0);
        };
        let authorization_bit = 1u32 << index;
        if DMA_AUTHORIZED.load(Ordering::Acquire) & authorization_bit != 0 {
            frame.registers[0] = Status::Busy as i64 as u64;
            return Some(0);
        }
        let dma_rights = Rights(Rights::MAP.0 | Rights::WRITE.0);
        let frame_capability = table.capability(CapHandle(frame.registers[1] as u32), dma_rights);
        let Ok(frame_capability) = frame_capability else {
            frame.registers[0] = Status::AccessDenied as i64 as u64;
            return Some(0);
        };
        let physical = if index == 0 {
            if frame_capability.object != DEVICE_QUEUE_OBJECT {
                frame.registers[0] = Status::AccessDenied as i64 as u64;
                return Some(0);
            }
            grant.queue_physical
        } else if index == 1 {
            if frame_capability.object != SHARED_FRAME_OBJECT {
                frame.registers[0] = Status::AccessDenied as i64 as u64;
                return Some(0);
            }
            grant.data_physical
        } else if frame_capability.object_type == ObjectType::Frame {
            let Some(physical) = runtime_frame_physical(frame_capability.object) else {
                frame.registers[0] = Status::AccessDenied as i64 as u64;
                return Some(0);
            };
            physical
        } else {
            frame.registers[0] = Status::AccessDenied as i64 as u64;
            return Some(0);
        };
        if frame_capability.object_type != ObjectType::Frame {
            frame.registers[0] = Status::AccessDenied as i64 as u64;
            return Some(0);
        }
        if smmu::map_runtime_page(iova, physical).is_err() {
            frame.registers[0] = Status::Io as i64 as u64;
            return Some(0);
        }
        DMA_FRAME_OBJECTS[index as usize].store(frame_capability.object, Ordering::Release);
        DMA_AUTHORIZED.fetch_or(authorization_bit, Ordering::Release);
    } else {
        let iova = frame.registers[1];
        let Some(index) = iova
            .checked_sub(grant.dma_iova)
            .filter(|offset| offset.is_multiple_of(4096))
            .map(|offset| offset / 4096)
            .filter(|index| *index < 32)
        else {
            frame.registers[0] = Status::AccessDenied as i64 as u64;
            return Some(0);
        };
        let authorization_bit = 1u32 << index;
        if DMA_AUTHORIZED.load(Ordering::Acquire) & authorization_bit == 0 {
            frame.registers[0] = Status::NotFound as i64 as u64;
            return Some(0);
        }
        if smmu::unmap_runtime_page(iova).is_err() {
            frame.registers[0] = Status::Io as i64 as u64;
            return Some(0);
        }
        DMA_FRAME_OBJECTS[index as usize].store(0, Ordering::Release);
        DMA_AUTHORIZED.fetch_and(!authorization_bit, Ordering::Release);
    }
    frame.registers[0] = Status::Ok as i64 as u64;
    Some(0)
}

pub fn handle_system_control(frame: &mut ExceptionFrame, syscall: u64) -> Option<u64> {
    if !active_on_current_cpu() || syscall != 22 {
        return None;
    }
    let operation = match frame.registers[1] {
        value if value == SystemControlOperation::ActivatePci as u64 => {
            SystemControlOperation::ActivatePci
        }
        value if value == SystemControlOperation::Poweroff as u64 => {
            SystemControlOperation::Poweroff
        }
        value if value == SystemControlOperation::Reboot as u64 => SystemControlOperation::Reboot,
        _ => {
            frame.registers[0] = Status::AccessDenied as i64 as u64;
            return Some(0);
        }
    };
    let guard = lock_state();
    let task = current_task(unsafe { &*STATE.0.get() });
    let capability = unsafe { &(&*CAPS.0.get())[task] }
        .capability(CapHandle(frame.registers[0] as u32), Rights::MANAGE);
    let task_authorized = matches!(
        (task, operation),
        (DEVMGR_TASK, SystemControlOperation::ActivatePci)
            | (SHELL_TASK, SystemControlOperation::Poweroff)
            | (SHELL_TASK, SystemControlOperation::Reboot)
            | (TERMINAL_TASK, SystemControlOperation::Poweroff)
            | (TERMINAL_TASK, SystemControlOperation::Reboot)
    );
    let authorized = task_authorized
        && matches!(capability, Ok(capability) if capability.object_type == ObjectType::SystemControl);
    drop(guard);
    if !authorized {
        frame.registers[0] = Status::AccessDenied as i64 as u64;
        return Some(0);
    }
    let result = match operation {
        SystemControlOperation::ActivatePci => {
            let slot = u8::try_from(frame.registers[2]).ok();
            let function = u8::try_from(frame.registers[3]).ok();
            match (slot, function) {
                (Some(slot), Some(function)) if slot < 32 && function < 8 => {
                    if PCI_ACTIVATION_STATE
                        .compare_exchange(
                            PCI_ACTIVATION_IDLE,
                            PCI_ACTIVATION_STARTING,
                            Ordering::AcqRel,
                            Ordering::Acquire,
                        )
                        .is_err()
                    {
                        Err(Status::Busy)
                    } else {
                        let result =
                            device_control::activate(slot, function).and_then(install_transport);
                        PCI_ACTIVATION_STATE.store(
                            if result.is_ok() {
                                PCI_ACTIVATION_READY
                            } else {
                                PCI_ACTIVATION_FAILED
                            },
                            Ordering::Release,
                        );
                        result
                    }
                }
                _ => Err(Status::Invalid),
            }
        }
        SystemControlOperation::Poweroff => {
            crate::kprintln!("[system] shutdown");
            return arch::shutdown(true);
        }
        SystemControlOperation::Reboot => {
            crate::kprintln!("[system] reboot");
            return arch::reboot();
        }
    };
    frame.registers[0] = match result {
        Ok(()) => Status::Ok as i64 as u64,
        Err(status) => status as i64 as u64,
    };
    Some(0)
}

pub fn handle_system_stats(frame: &mut ExceptionFrame, syscall: u64) -> Option<u64> {
    if !active_on_current_cpu() || syscall != 24 {
        return None;
    }
    let _guard = lock_state();
    let task = current_task(unsafe { &*STATE.0.get() });
    let capability = unsafe { &(&*CAPS.0.get())[task] }
        .capability(CapHandle(frame.registers[0] as u32), Rights::READ);
    if !matches!(capability, Ok(capability) if capability.object_type == ObjectType::SystemInfo) {
        frame.registers[0] = Status::AccessDenied as i64 as u64;
        return Some(0);
    }
    if frame.registers[2] as usize != core::mem::size_of::<microsystem_abi::SystemStats>() {
        frame.registers[0] = Status::Invalid as i64 as u64;
        return Some(0);
    }
    let state = unsafe { &*STATE.0.get() };
    let mut runnable = 0u32;
    let mut blocked = 0u32;
    for thread in &state.threads {
        match thread {
            ThreadState::Runnable | ThreadState::Running { .. } => runnable += 1,
            ThreadState::Sending { .. }
            | ThreadState::Receiving { .. }
            | ThreadState::WaitingReply { .. }
            | ThreadState::WaitingNotification { .. }
            | ThreadState::Terminating { .. } => blocked += 1,
            ThreadState::Exited => {}
        }
    }
    let stats = microsystem_abi::SystemStats {
        version: 1,
        cpu_count: CPU_COUNT as u16,
        reserved: 0,
        cpu_ticks: [arch::timer_ticks(0), arch::timer_ticks(1)],
        free_frames: physical_memory::free_frames() as u64,
        kernel_heap_used: kernel_heap::allocated_bytes() as u64,
        kernel_heap_total: physical_memory::kernel_heap_bytes() as u64,
        runnable_threads: runnable,
        blocked_threads: blocked,
        irq_count: arch::device_interrupts(),
        ipc_calls: IPC_CALLS.load(Ordering::Relaxed),
    };
    frame.registers[0] = if copy_to_user(
        task,
        frame.registers[1],
        (&stats as *const microsystem_abi::SystemStats).cast::<u8>(),
        core::mem::size_of::<microsystem_abi::SystemStats>(),
    )
    .is_some()
    {
        Status::Ok as i64 as u64
    } else {
        Status::Fault as i64 as u64
    };
    Some(0)
}

pub fn handle_gui_present(frame: &mut ExceptionFrame, syscall: u64) -> Option<u64> {
    if !active_on_current_cpu() || syscall != 25 {
        return None;
    }
    let _guard = lock_state();
    let task = current_task(unsafe { &*STATE.0.get() });
    if task != WINDOWD_TASK {
        frame.registers[0] = Status::AccessDenied as i64 as u64;
        return Some(0);
    }
    let capability = unsafe { &(&*CAPS.0.get())[task] }
        .capability(CapHandle(frame.registers[0] as u32), Rights::READ);
    let Ok(capability) = capability else {
        frame.registers[0] = Status::BadCapability as i64 as u64;
        return Some(0);
    };
    let Some(region) = (capability.object_type == ObjectType::FrameRegion)
        .then(|| frame_region(capability.object))
        .flatten()
    else {
        frame.registers[0] = Status::BadCapability as i64 as u64;
        return Some(0);
    };
    const FRAMEBUFFER_PAGES: usize = 1024 * 768 * 4 / 4096;
    if region.pages as usize != FRAMEBUFFER_PAGES {
        frame.registers[0] = Status::Invalid as i64 as u64;
        return Some(0);
    }
    let requested = (
        frame.registers[1],
        frame.registers[2],
        frame.registers[3],
        frame.registers[4],
    );
    let (x, y, width, height) = if requested == (0, 0, 0, 0) {
        (0usize, 0usize, 1024usize, 768usize)
    } else {
        let Ok(x) = usize::try_from(requested.0) else {
            frame.registers[0] = Status::Invalid as i64 as u64;
            return Some(0);
        };
        let Ok(y) = usize::try_from(requested.1) else {
            frame.registers[0] = Status::Invalid as i64 as u64;
            return Some(0);
        };
        let Ok(width) = usize::try_from(requested.2) else {
            frame.registers[0] = Status::Invalid as i64 as u64;
            return Some(0);
        };
        let Ok(height) = usize::try_from(requested.3) else {
            frame.registers[0] = Status::Invalid as i64 as u64;
            return Some(0);
        };
        if width == 0
            || height == 0
            || x.checked_add(width).is_none_or(|right| right > 1024)
            || y.checked_add(height).is_none_or(|bottom| bottom > 768)
        {
            frame.registers[0] = Status::Invalid as i64 as u64;
            return Some(0);
        }
        (x, y, width, height)
    };
    let Some(destination) = crate::gpu::framebuffer_mut() else {
        frame.registers[0] = Status::NotFound as i64 as u64;
        return Some(0);
    };
    for row in y..y + height {
        let mut offset = (row * 1024 + x) * 4;
        let mut remaining = width * 4;
        while remaining != 0 {
            let page = offset / 4096;
            let within_page = offset % 4096;
            let bytes = remaining.min(4096 - within_page);
            let Some(physical) = runtime_frame_physical(region.frames[page]) else {
                frame.registers[0] = Status::Fault as i64 as u64;
                return Some(0);
            };
            let source = (crate::arch::phys_to_virt(physical) + within_page) as *const u8;
            unsafe {
                core::ptr::copy_nonoverlapping(source, destination.as_mut_ptr().add(offset), bytes);
            }
            offset += bytes;
            remaining -= bytes;
        }
    }
    frame.registers[0] =
        if crate::gpu::flush_rect(x as u32, y as u32, width as u32, height as u32).is_ok() {
            if !GUI_PRESENT_REPORTED.swap(true, Ordering::AcqRel) {
                crate::kprintln!(
                    "[gui] EL0 windowd presented scanout bytes={} dma-isolated=true",
                    destination.len()
                );
            }
            if (width != 1024 || height != 768)
                && !GUI_PARTIAL_PRESENT_REPORTED.swap(true, Ordering::AcqRel)
            {
                crate::kprintln!(
                    "[gui] EL0 windowd partial-present=true rect={}x{}+{},{}",
                    width,
                    height,
                    x,
                    y
                );
            }
            Status::Ok as i64 as u64
        } else {
            Status::Io as i64 as u64
        };
    Some(0)
}

pub fn handle_gui_input(frame: &mut ExceptionFrame, syscall: u64) -> Option<u64> {
    if !active_on_current_cpu() || syscall != 26 {
        return None;
    }
    let _guard = lock_state();
    let task = current_task(unsafe { &*STATE.0.get() });
    if task != WINDOWD_TASK {
        frame.registers[0] = Status::AccessDenied as i64 as u64;
        return Some(0);
    }
    if frame.registers[1] as usize != core::mem::size_of::<microsystem_abi::gui::InputEvent>() {
        frame.registers[0] = Status::Invalid as i64 as u64;
        return Some(0);
    }
    let Some(event) = crate::input::poll() else {
        frame.registers[0] = Status::Busy as i64 as u64;
        return Some(0);
    };
    frame.registers[0] = if copy_to_user(
        task,
        frame.registers[0],
        (&event as *const microsystem_abi::gui::InputEvent).cast::<u8>(),
        core::mem::size_of::<microsystem_abi::gui::InputEvent>(),
    )
    .is_some()
    {
        Status::Ok as i64 as u64
    } else {
        Status::Fault as i64 as u64
    };
    Some(0)
}

pub fn handle_network(frame: &mut ExceptionFrame, syscall: u64) -> Option<u64> {
    if !active_on_current_cpu() || (!(27..=29).contains(&syscall) && syscall != 44) {
        return None;
    }
    let _guard = lock_state();
    let task = current_task(unsafe { &*STATE.0.get() });
    if syscall != 29 && task != NETD_TASK {
        frame.registers[0] = Status::AccessDenied as i64 as u64;
        return Some(0);
    }
    let handle = CapHandle(frame.registers[0] as u32);
    let (object_type, rights) = if syscall == 27 || syscall == 44 {
        (ObjectType::NetworkDevice, Rights::READ)
    } else if syscall == 28 {
        (ObjectType::NetworkDevice, Rights::WRITE)
    } else {
        (ObjectType::RandomSource, Rights::READ)
    };
    let capability = unsafe { &(&*CAPS.0.get())[task] }.capability(handle, rights);
    if !matches!(capability, Ok(capability) if capability.object_type == object_type) {
        frame.registers[0] = Status::AccessDenied as i64 as u64;
        return Some(0);
    }
    if syscall == 44 {
        let pointer = frame.registers[1];
        let index = frame.registers[2] as usize;
        frame.registers[0] = match crate::net::hardware_info(index) {
            Some(info)
                if copy_to_user(
                    task,
                    pointer,
                    (&info as *const microsystem_abi::network::HardwareInfoV1).cast::<u8>(),
                    core::mem::size_of_val(&info),
                )
                .is_some() =>
            {
                Status::Ok as i64 as u64
            }
            Some(_) => Status::Fault as i64 as u64,
            None => Status::Invalid as i64 as u64,
        };
        return Some(0);
    }
    let pointer = frame.registers[1];
    let bytes = frame.registers[2] as usize;
    let maximum = if syscall == 29 { 256 } else { 1536 };
    if bytes == 0 || bytes > maximum {
        frame.registers[0] = Status::Invalid as i64 as u64;
        return Some(0);
    }
    let mut buffer = [0u8; 1536];
    frame.registers[0] = match syscall {
        27 => match crate::net::receive(frame.registers[3] as usize, &mut buffer[..bytes]) {
            Ok(0) => 0,
            Ok(received)
                if validate_user_range(task, pointer, received as u64, true)
                    && copy_to_user(task, pointer, buffer.as_ptr(), received).is_some() =>
            {
                received as u64
            }
            Ok(_) => Status::Fault as i64 as u64,
            Err(()) => Status::Io as i64 as u64,
        },
        28 if validate_user_range(task, pointer, bytes as u64, false)
            && copy_from_user(task, pointer, buffer.as_mut_ptr(), bytes).is_some() =>
        {
            match crate::net::send(frame.registers[3] as usize, &buffer[..bytes]) {
                Ok(true) => Status::Ok as i64 as u64,
                Ok(false) => Status::Busy as i64 as u64,
                Err(()) => Status::Io as i64 as u64,
            }
        }
        29 if validate_user_range(task, pointer, bytes as u64, true) => {
            match crate::random::fill(&mut buffer[..bytes]) {
                Ok(()) if copy_to_user(task, pointer, buffer.as_ptr(), bytes).is_some() => {
                    Status::Ok as i64 as u64
                }
                Ok(()) => Status::Fault as i64 as u64,
                Err(()) => Status::Io as i64 as u64,
            }
        }
        _ => Status::Fault as i64 as u64,
    };
    Some(0)
}

fn install_transport(grant: UserTransportGrant) -> Result<(), Status> {
    let _guard = lock_state();
    if unsafe { (*BLOCK_GRANT.0.get()).is_some() } {
        return Err(Status::Busy);
    }
    for task in [BLOCK_TASK, DEVMGR_TASK, MFS_TASK] {
        if task_memory(task).is_none() {
            return Err(Status::Invalid);
        }
    }
    for task in [BLOCK_TASK, DEVMGR_TASK, MFS_TASK] {
        let memory = task_memory_mut(task).expect("prevalidated transport task memory");
        map_transport_memory(memory, task, grant);
    }
    crate::arch::dma_write_barrier();
    for task in [BLOCK_TASK, MFS_TASK, DEVMGR_TASK] {
        arch::invalidate_user_asid(ASID_BASE + task as u16);
    }
    unsafe { *BLOCK_GRANT.0.get() = Some(grant) };
    Ok(())
}

pub fn notify_device_irq() {
    if !DEVICE_IRQ_BOUND.load(Ordering::Acquire) {
        return;
    }
    DEVICE_NOTIFICATION.fetch_or(1, Ordering::Release);
    arch::send_reschedule(0);
    arch::send_reschedule(1);
    arch::send_event();
}

pub fn on_timer(frame: &mut ExceptionFrame) {
    if !active_on_current_cpu() || !preempt::interrupted_user(frame) {
        return;
    }
    let _guard = lock_state();
    wake_notifications();
    expire_deadlines();
    switch(frame);
}

pub fn on_reschedule(frame: &mut ExceptionFrame) {
    if !active_on_current_cpu() || !preempt::interrupted_user(frame) {
        return;
    }
    let _guard = lock_state();
    wake_notifications();
    RESCHEDULE_WAKEUPS.fetch_add(1, Ordering::Relaxed);
    switch(frame);
}

pub fn mark_current_ready() {
    if !active_on_current_cpu() {
        return;
    }
    let _guard = lock_state();
    let current = current_task(unsafe { &*STATE.0.get() });
    if current < SERVICE_COUNT {
        READY.fetch_or(1 << current, Ordering::Release);
    }
}

pub fn enable_gui_services() {
    EXPECTED_SERVICE_MASK.store((1u32 << SERVICE_COUNT) - 1, Ordering::Release);
}

pub fn ready_count() -> u32 {
    let expected_mask = EXPECTED_SERVICE_MASK.load(Ordering::Acquire);
    (READY.load(Ordering::Acquire) & expected_mask).count_ones()
}

pub fn mark_current_online() {
    if !active_on_current_cpu() {
        return;
    }
    let _guard = lock_state();
    let current = current_task(unsafe { &*STATE.0.get() });
    if current >= SERVICE_COUNT {
        return;
    }
    let state = unsafe { &mut *STATE.0.get() };
    state.services[current].state = microsystem_abi::service::ONLINE;
    state.services[current].online_ns = arch::clock_nanos();
    unsafe {
        (&mut *BOOT_CAPS.0.get())[current] =
            (&*CAPS.0.get())[current].snapshot_matching(|capability| capability.object < 100);
    }
    let online = ONLINE.fetch_or(1 << current, Ordering::AcqRel) | (1 << current);
    let expected_mask = EXPECTED_SERVICE_MASK.load(Ordering::Acquire);
    let expected = expected_mask.count_ones();
    if online & expected_mask == expected_mask
        && SERVICE_STATUS_REPORTED
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    {
        crate::kprintln!(
            "[service] resident EL0 ready={}/{} online={}/{} switches={}",
            ready_count(),
            expected,
            (online & expected_mask).count_ones(),
            expected,
            switches()
        );
        let (waits, acknowledgements, wakeups) = notification_diagnostics();
        crate::kprintln!(
            "[irq] resident notification blocking-waits={} irq-acks={} sgi-wakeups={}",
            waits,
            acknowledgements,
            wakeups
        );
        crate::kprintln!("[service] root task ready");
        crate::kprintln!("[service] block transport ready (VirtIO-PCI)");
        crate::kprintln!("[service] mfs1 ready (resident EL0)");
    }
    arch::send_event();
}

pub fn exit_disposition(code: u64) -> Option<(&'static str, bool)> {
    if !active_on_current_cpu() {
        return None;
    }
    let _guard = lock_state();
    let current = current_task(unsafe { &*STATE.0.get() });
    if current < SERVICE_COUNT {
        Some((SERVICE_NAMES[current], current == SHELL_TASK && code == 0))
    } else {
        None
    }
}

pub fn start_thread(program: u64) -> Result<u64, Status> {
    start_thread_inner(program, None)
}

pub fn clock_realtime() -> Result<u64, Status> {
    crate::rtc::realtime().ok_or(Status::NotSupported)
}

pub fn ipc_peer() -> Result<u64, Status> {
    if !active_on_current_cpu() { return Err(Status::Busy); }
    let _guard = lock_state();
    let state = unsafe { &*STATE.0.get() };
    state.reply_to[current_task(state)].map(|caller| caller as u64 + 1).ok_or(Status::Busy)
}

pub fn start_thread_ex(pointer: u64, bytes: u64) -> Result<u64, Status> {
    if !active_on_current_cpu() {
        return Err(Status::AccessDenied);
    }
    let _guard = lock_state();
    let task = current_task(unsafe { &*STATE.0.get() });
    if task != 0 {
        return Err(Status::AccessDenied);
    }
    if !matches!(bytes as usize, size if size == mem::size_of::<ThreadLaunchV1>() || size == mem::size_of::<ThreadLaunchV2>())
        || !validate_user_range(task, pointer, bytes, false)
    {
        return Err(Status::Invalid);
    }
    let launch = if bytes as usize == mem::size_of::<ThreadLaunchV1>() {
        let mut legacy = ThreadLaunchV1::default();
        copy_from_user(
            task,
            pointer,
            (&mut legacy as *mut ThreadLaunchV1).cast::<u8>(),
            mem::size_of::<ThreadLaunchV1>(),
        )
        .ok_or(Status::Fault)?;
        let mut launch = ThreadLaunchV2 {
            version: legacy.version,
            profile: legacy.profile,
            flags: legacy.flags,
            program: legacy.program,
            arguments: legacy.arguments,
            ..ThreadLaunchV2::default()
        };
        launch.caps[..microsystem_abi::THREAD_LAUNCH_CAPS].copy_from_slice(&legacy.caps);
        launch.rights[..microsystem_abi::THREAD_LAUNCH_CAPS].copy_from_slice(&legacy.rights);
        launch
    } else {
        let mut launch = ThreadLaunchV2::default();
        copy_from_user(
            task,
            pointer,
            (&mut launch as *mut ThreadLaunchV2).cast::<u8>(),
            mem::size_of::<ThreadLaunchV2>(),
        )
        .ok_or(Status::Fault)?;
        launch
    };
    drop(_guard);
    start_thread_inner(launch.program, Some(&launch))
}

fn start_thread_inner(program: u64, launch: Option<&ThreadLaunchV2>) -> Result<u64, Status> {
    if !active_on_current_cpu() {
        return Err(Status::AccessDenied);
    }
    let guard = lock_state();
    start_thread_locked(guard, program, launch, None)
}

pub fn start_native(
    pointer: u64,
    bytes: u64,
    program: u64,
    pages: u64,
    permissions: u64,
) -> Result<u64, Status> {
    if !active_on_current_cpu() {
        return Err(Status::AccessDenied);
    }
    let guard = lock_state();
    if current_task(unsafe { &*STATE.0.get() }) != 0 {
        return Err(Status::AccessDenied);
    }
    if bytes == 0
        || bytes > microsystem_abi::application::MAX_IMAGE_BYTES as u64
        || pages == 0
        || pages > microsystem_abi::virtual_memory::PAGE_BUDGET as u64
        || permissions
            & !(microsystem_abi::application::RANDOM | microsystem_abi::application::SYSTEM_INFO)
            != 0
    {
        return Err(Status::Invalid);
    }
    let image = unsafe { &mut *NATIVE_SCRATCH.0.get() };
    for offset in (0..bytes as usize).step_by(4096) {
        let count = (bytes as usize - offset).min(4096);
        let address = pointer.checked_add(offset as u64).ok_or(Status::Fault)?;
        copy_from_user(0, address, image[offset..].as_mut_ptr(), count).ok_or(Status::Fault)?;
    }
    start_thread_locked(
        guard,
        program,
        None,
        Some(NativeLaunch {
            image: &image[..bytes as usize],
            pages: pages as u32,
            permissions,
        }),
    )
}

fn start_thread_locked(
    guard: StateGuard,
    program: u64,
    launch: Option<&ThreadLaunchV2>,
    native: Option<NativeLaunch>,
) -> Result<u64, Status> {
    if current_task(unsafe { &*STATE.0.get() }) != 0 {
        return Err(Status::AccessDenied);
    }
    if program == 0 {
        return Err(Status::Invalid);
    }
    let profile = if native.is_some() {
        microsystem_abi::THREAD_PROFILE_NATIVE
    } else {
        launch.map_or(microsystem_abi::THREAD_PROFILE_APPLICATION, |launch| {
            launch.profile
        })
    };
    if let Some(launch) = launch {
        if !matches!(launch.version, 1 | 2)
            || launch.flags & !microsystem_abi::THREAD_LAUNCH_FLAG_GUI != 0
            || launch.version == 1 && launch.flags != 0
            || launch.profile != microsystem_abi::THREAD_PROFILE_MICA
            || program != program_id("mica")
        {
            return Err(Status::Invalid);
        }
    }
    let state = unsafe { &mut *STATE.0.get() };
    let service_task = native
        .is_none()
        .then(|| {
            SERVICE_NAMES
                .iter()
                .enumerate()
                .skip(1)
                .find_map(|(task, name)| (program_id(name) == program).then_some(task))
        })
        .flatten();
    let (task, image, application) = if let Some(task) = service_task {
        if !matches!(state.threads[task], ThreadState::Exited) {
            return Err(Status::Busy);
        }
        if state.services[task].state == microsystem_abi::service::QUIESCE_FAILED {
            return Err(Status::Io);
        }
        (
            task,
            image_by_name(bootfs(), SERVICE_NAMES[task]).ok_or(Status::NotFound)?,
            false,
        )
    } else {
        let task = (APPLICATION_START..APPLICATION_START + APPLICATION_COUNT)
            .find(|task| matches!(state.threads[*task], ThreadState::Exited))
            .ok_or(Status::Busy)?;
        (
            task,
            if let Some(native) = &native {
                native.image
            } else {
                application_image_by_id(program, launch.is_some())?
            },
            true,
        )
    };
    if launch.is_some() && !application {
        return Err(Status::Invalid);
    }
    let launch_caps = if let Some(launch) = launch {
        Some(validate_launch_capabilities(launch)?)
    } else {
        None
    };
    load_task(task, image).map_err(|_| Status::Invalid)?;
    if !application {
        unsafe { (&mut *CAPS.0.get())[task] = (&*BOOT_CAPS.0.get())[task].clone() };
        state.services[task].state = microsystem_abi::service::STARTING;
        state.services[task].starts += 1;
        state.services[task].started_ns = arch::clock_nanos();
        state.services[task].online_ns = 0;
    }
    arch::flush_icache();
    arch::invalidate_user_asid(ASID_BASE + task as u16);
    let start = unsafe { (&*STARTS.0.get())[task] };
    state.contexts[task] = Context::EMPTY;
    state.contexts[task].elr = start.entry;
    state.contexts[task].sp_el0 = start.stack;
    state.contexts[task].ttbr0 = start.ttbr0;
    if application {
        state.contexts[task].registers[0] = (task + 1) as u64;
    } else if task == CONSOLE_TASK {
        let uart = CONSOLE_UART.load(Ordering::Acquire);
        state.contexts[task].registers[0] = CONSOLE_UART_VA + (uart & 0xfff);
    } else if task == DEVMGR_TASK {
        let (base, bytes) = device_control::pcie_window().ok_or(Status::NotFound)?;
        state.contexts[task].registers[0] = base;
        state.contexts[task].registers[1] = bytes;
        state.contexts[task].registers[2] = DEVMGR_ECAM_SCAN_VA;
        state.contexts[task].registers[3] = u64::from(unsafe { (*BLOCK_GRANT.0.get()).is_some() });
        state.contexts[task].registers[4] =
            u64::from(EXPECTED_SERVICE_MASK.load(Ordering::Acquire) & (1 << WINDOWD_TASK) != 0);
    }
    if application {
        let index = task - APPLICATION_START;
        let tables = unsafe { &mut *CAPS.0.get() };
        tables[task] = CapabilityTable::new();
        let mut install = if let (Some(launch), Some(capabilities)) = (launch, launch_caps) {
            install_launch_capabilities(
                &mut tables[task],
                launch,
                capabilities,
                &mut state.contexts[task],
            )
        } else if native.is_some() {
            Ok(())
        } else {
            insert_endpoint_cap(
                tables,
                task,
                microsystem_abi::boot_cap::PROCESS_ENDPOINT,
                4,
                Rights::WRITE,
                PROCESS_ENDPOINT_NODE,
            )
        };
        if let Some(native) = &native {
            install = install.and_then(|()| virtual_memory::set_budget(task, native.pages));
            for (flag, handle, object, object_type, node) in [
                (
                    microsystem_abi::application::RANDOM,
                    microsystem_abi::boot_cap::RANDOM_SOURCE,
                    62,
                    ObjectType::RandomSource,
                    RANDOM_SOURCE_NODE,
                ),
                (
                    microsystem_abi::application::SYSTEM_INFO,
                    microsystem_abi::boot_cap::SYSTEM_INFO,
                    60,
                    ObjectType::SystemInfo,
                    22,
                ),
            ] {
                if native.permissions & flag != 0 {
                    install = install.and_then(|()| {
                        tables[task]
                            .insert_root_at(handle, object, object_type, Rights::READ, node)
                            .map(|_| ())
                    });
                }
            }
        }
        #[cfg(target_arch = "x86_64")]
        if install.is_ok() {
            install = prepare_x86_application_entry(task, &mut state.contexts[task]);
        }
        if let Err(status) = install {
            tables[task] = CapabilityTable::new();
            unsafe { (&mut *STARTS.0.get())[task] = Start::EMPTY };
            state.contexts[task] = Context::EMPTY;
            state.threads[task] = ThreadState::Exited;
            return Err(status);
        }
        state.applications[index] = ApplicationSlot {
            program,
            profile,
            exit_status: 0,
            cpu_mask: 0,
            cpu_ticks: [0; CPU_COUNT],
            reclaimed_frames: 0,
            reclaimed_pools: 0,
            reclaimed_mappings: 0,
        };
    }
    state.threads[task] = ThreadState::Runnable;
    let pid = (task + 1) as u64;
    drop(guard);
    arch::send_reschedule(0);
    arch::send_reschedule(1);
    arch::send_event();
    Ok(pid)
}

#[cfg(target_arch = "x86_64")]
fn prepare_x86_application_entry(task: usize, context: &mut Context) -> Result<(), Status> {
    // ExceptionFrame keeps syscall arguments contiguous through r10, while a
    // fresh SysV function expects its fourth argument in rcx and the rest on
    // a callee-aligned stack.
    let arguments = [
        context.registers[0],
        context.registers[1],
        context.registers[2],
        context.registers[3],
        context.registers[4],
        context.registers[5],
        context.registers[6],
        context.registers[7],
        context.registers[8],
        context.registers[9],
        context.registers[10],
    ];
    let memory = task_memory_mut(task).ok_or(Status::Fault)?;
    let stack_words = [
        0,
        arguments[6],
        arguments[7],
        arguments[8],
        arguments[9],
        arguments[10],
        0,
    ];
    let stack_start = STACK_BYTES - stack_words.len() * mem::size_of::<u64>();
    for (index, word) in stack_words.into_iter().enumerate() {
        let offset = stack_start + index * mem::size_of::<u64>();
        memory.stack[offset..offset + mem::size_of::<u64>()].copy_from_slice(&word.to_ne_bytes());
    }
    context.registers[3] = 0;
    context.registers[6] = 0;
    context.registers[7] = arguments[3];
    context.registers[8..11].fill(0);
    context.sp_el0 = STACK_TOP - (stack_words.len() * mem::size_of::<u64>()) as u64;
    Ok(())
}

fn validate_launch_capabilities(
    launch: &ThreadLaunchV2,
) -> Result<[Option<Capability>; microsystem_abi::THREAD_LAUNCH_V2_CAPS], Status> {
    let expected_types = [
        ObjectType::FrameRegion,
        ObjectType::Notification,
        ObjectType::Endpoint,
        ObjectType::Endpoint,
        ObjectType::RandomSource,
        ObjectType::SystemInfo,
        ObjectType::FrameRegion,
        ObjectType::Frame,
        ObjectType::Endpoint,
    ];
    let allowed_rights = [
        Rights(Rights::READ.0 | Rights::WRITE.0 | Rights::MAP.0 | Rights::GRANT.0),
        Rights(Rights::READ.0 | Rights::WRITE.0),
        Rights::WRITE,
        Rights::WRITE,
        Rights::READ,
        Rights::READ,
        Rights(Rights::READ.0 | Rights::WRITE.0 | Rights::MAP.0),
        Rights(Rights::READ.0 | Rights::WRITE.0 | Rights::MAP.0),
        Rights::WRITE,
    ];
    let tables = unsafe { &*CAPS.0.get() };
    let mut output = [None; microsystem_abi::THREAD_LAUNCH_V2_CAPS];
    let gui = launch.flags & microsystem_abi::THREAD_LAUNCH_FLAG_GUI != 0;
    for index in 0..microsystem_abi::THREAD_LAUNCH_V2_CAPS {
        let handle = launch.caps[index];
        let rights = launch.rights[index];
        if handle == CapHandle::INVALID {
            if index < 4 || gui && index >= microsystem_abi::THREAD_LAUNCH_CAPS || rights.0 != 0 {
                return Err(Status::Invalid);
            }
            continue;
        }
        if !gui && index >= microsystem_abi::THREAD_LAUNCH_CAPS {
            return Err(Status::Invalid);
        }
        if rights.0 == 0 || rights.0 & !allowed_rights[index].0 != 0 {
            return Err(Status::AccessDenied);
        }
        let capability = tables[0].capability(handle, Rights::GRANT)?;
        if capability.object_type != expected_types[index] || !capability.rights.contains(rights) {
            return Err(Status::AccessDenied);
        }
        output[index] = Some(capability);
    }
    Ok(output)
}

fn install_launch_capabilities(
    table: &mut CapabilityTable,
    launch: &ThreadLaunchV2,
    capabilities: [Option<Capability>; microsystem_abi::THREAD_LAUNCH_V2_CAPS],
    context: &mut Context,
) -> Result<(), Status> {
    let destinations = [
        microsystem_abi::boot_cap::SCRIPT_SESSION_REGION,
        microsystem_abi::boot_cap::SCRIPT_IO_NOTIFICATION,
        microsystem_abi::boot_cap::SCRIPT_FILESYSTEM_ENDPOINT,
        microsystem_abi::boot_cap::SCRIPT_NETWORK_ENDPOINT,
        microsystem_abi::boot_cap::SCRIPT_RANDOM_SOURCE,
        microsystem_abi::boot_cap::SCRIPT_SYSTEM_DATA,
        microsystem_abi::boot_cap::SCRIPT_GUI_COMMANDS,
        microsystem_abi::boot_cap::SCRIPT_GUI_EVENTS,
        microsystem_abi::boot_cap::SCRIPT_GUI_ENDPOINT,
    ];
    for index in 0..microsystem_abi::THREAD_LAUNCH_V2_CAPS {
        let Some(capability) = capabilities[index] else {
            if index < microsystem_abi::THREAD_LAUNCH_CAPS {
                context.registers[index + 1] = CapHandle::INVALID.0 as u64;
            }
            continue;
        };
        table.import_copy_at(
            destinations[index],
            capability,
            launch.rights[index],
            allocate_derivation(),
        )?;
        if index < microsystem_abi::THREAD_LAUNCH_CAPS {
            context.registers[index + 1] = destinations[index].0 as u64;
        }
    }
    table.insert_root_at(
        microsystem_abi::boot_cap::SCRIPT_BROKER_ENDPOINT,
        15,
        ObjectType::Endpoint,
        Rights::WRITE,
        32,
    )?;
    context.registers[7..11].copy_from_slice(&launch.arguments);
    Ok(())
}

fn image_by_name(bootfs: &'static [u8], name: &str) -> Option<&'static [u8]> {
    Archive::new(bootfs).find_map(|entry| match entry {
        Ok(entry) if entry.name.strip_prefix("bin/") == Some(name) => Some(entry.data),
        _ => None,
    })
}

fn application_image_by_id(program: u64, allow_mica: bool) -> Result<&'static [u8], Status> {
    Archive::new(bootfs())
        .find_map(|entry| {
            let entry = entry.ok()?;
            let name = entry.name.strip_prefix("bin/")?;
            if SERVICE_NAMES.contains(&name)
                || name == "idle"
                || name == "mica" && !allow_mica
                || program_id(name) != program
            {
                return None;
            }
            Some(entry.data)
        })
        .ok_or(Status::NotFound)
}

fn bootfs() -> &'static [u8] {
    let address = BOOTFS_ADDRESS.load(Ordering::Acquire);
    let length = BOOTFS_LENGTH.load(Ordering::Acquire);
    if address == 0 || length == 0 {
        return &[];
    }
    unsafe { core::slice::from_raw_parts(address as *const u8, length) }
}

fn program_id(name: &str) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in name.as_bytes() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x100_0000_01b3);
    }
    hash
}

fn finish_application(state: &mut State, task: usize, status: i64) {
    if let ReplySlot::Pending(server) = state.async_replies[task] {
        if state.reply_to[server as usize] == Some(task) {
            state.reply_to[server as usize] = None;
        }
    }
    state.async_replies[task] = ReplySlot::Idle;
    match state.threads[task] {
        ThreadState::Sending { endpoint, .. } => {
            if let Some(index) = endpoint.checked_sub(1)
                && state.endpoints[index].pending == Some(task)
            {
                state.endpoints[index].pending = None;
            }
        }
        ThreadState::WaitingReply { server, .. } => {
            if state.reply_to[server] == Some(task) {
                state.reply_to[server] = None;
            }
        }
        _ => {}
    }
    state.threads[task] = ThreadState::Exited;
    state.reply_to[task] = None;
    state.requests[task] = Message::new(0, 0);
    let cleanup = reclaim_task_capabilities(task);
    let application = &mut state.applications[task - APPLICATION_START];
    application.exit_status = status;
    application.reclaimed_frames = cleanup.frames;
    application.reclaimed_pools = cleanup.pools;
    application.reclaimed_mappings = cleanup.mappings;
}

fn finish_service(state: &mut State, task: usize, status: i64) {
    // Wake every client before replacing a service's address space. No caller
    // may keep a reply pointer or a queued capability transfer into that image.
    for client in 0..TASK_COUNT {
        if let ReplySlot::Pending(server) = state.async_replies[client] {
            if server as usize == task {
                state.async_replies[client] = ReplySlot::Failed(Status::Io);
            }
        }
        let waiting = match state.threads[client] {
            ThreadState::WaitingReply { server, .. } => server == task,
            ThreadState::Sending { endpoint, .. } => ENDPOINT_OWNERS[endpoint - 1] == task,
            _ => false,
        };
        if waiting {
            state.threads[client] = ThreadState::Runnable;
            state.contexts[client].registers[0] = Status::Io as i64 as u64;
        }
        if state.reply_to[client] == Some(task) {
            state.reply_to[client] = None;
        }
    }
    for (index, endpoint) in state.endpoints.iter_mut().enumerate() {
        if ENDPOINT_OWNERS[index] == task || endpoint.pending == Some(task) {
            endpoint.pending = None;
        }
    }
    state.reply_to[task] = None;
    state.async_replies[task] = ReplySlot::Idle;
    state.requests[task] = Message::new(0, 0);
    state.threads[task] = ThreadState::Exited;
    state.services[task].state = microsystem_abi::service::STOPPED;
    state.services[task].exit_status = status;
    state.services[task].online_ns = 0;
    ONLINE.fetch_and(!(1 << task), Ordering::Release);
    READY.fetch_and(!(1 << task), Ordering::Release);
    let quiesced = task != BLOCK_TASK || quiesce_block().is_ok();
    if quiesced {
        reclaim_task_capabilities(task);
    } else {
        // A controller that has not acknowledged reset may still DMA. Keep
        // its memory pinned and refuse a restart until explicit recovery.
        state.services[task].state = microsystem_abi::service::QUIESCE_FAILED;
    }
    crate::kprintln!(
        "[service] stopped name={} status={} starts={} quiesced={}",
        SERVICE_NAMES[task],
        status,
        state.services[task].starts,
        quiesced
    );
    arch::send_reschedule(0);
    arch::send_reschedule(1);
}

fn quiesce_block() -> Result<(), Status> {
    let Some(grant) = (unsafe { *BLOCK_GRANT.0.get() }) else {
        return Ok(());
    };
    let status = (arch::phys_to_virt(grant.common_physical) + 20) as *mut u8;
    unsafe { ptr::write_volatile(status, 0) };
    let mut acknowledged = false;
    for _ in 0..1_000_000 {
        if unsafe { ptr::read_volatile(status) } == 0 {
            acknowledged = true;
            break;
        }
        core::hint::spin_loop();
    }
    if !acknowledged {
        return Err(Status::TimedOut);
    }
    DEVICE_IRQ_BOUND.store(false, Ordering::Release);
    DEVICE_NOTIFICATION.store(0, Ordering::Release);
    for page in 0..32 {
        if DMA_AUTHORIZED.load(Ordering::Acquire) & (1 << page) != 0 {
            smmu::unmap_runtime_page(grant.dma_iova + page * 4096).map_err(|_| Status::Io)?;
            DMA_FRAME_OBJECTS[page as usize].store(0, Ordering::Release);
            DMA_AUTHORIZED.fetch_and(!(1 << page), Ordering::Release);
        }
    }
    Ok(())
}

pub fn write_service_status(pid: u64, pointer: u64) -> Result<(), Status> {
    if !active_on_current_cpu() {
        return Err(Status::AccessDenied);
    }
    let _guard = lock_state();
    let state = unsafe { &*STATE.0.get() };
    if current_task(state) != 0 {
        return Err(Status::AccessDenied);
    }
    let task = pid
        .checked_sub(1)
        .filter(|task| *task < SERVICE_COUNT as u64)
        .ok_or(Status::Invalid)? as usize;
    let info = state.services[task];
    copy_to_user(
        0,
        pointer,
        (&info as *const microsystem_abi::service::InfoV1).cast(),
        mem::size_of::<microsystem_abi::service::InfoV1>(),
    )
    .ok_or(Status::Fault)
}

pub fn stop_service(pid: u64, status: i64) -> Result<(), Status> {
    if !active_on_current_cpu() {
        return Err(Status::AccessDenied);
    }
    let _guard = lock_state();
    let state = unsafe { &mut *STATE.0.get() };
    if current_task(state) != 0 {
        return Err(Status::AccessDenied);
    }
    let task = pid
        .checked_sub(1)
        .filter(|task| *task != 0 && *task < SERVICE_COUNT as u64)
        .ok_or(Status::Invalid)? as usize;
    match state.threads[task] {
        ThreadState::Exited => return Err(Status::NotFound),
        ThreadState::Terminating { .. } => return Err(Status::Busy),
        ThreadState::Running { cpu } => {
            state.services[task].state = microsystem_abi::service::STOPPING;
            state.threads[task] = ThreadState::Terminating { cpu, status };
            arch::send_reschedule(cpu);
        }
        _ => finish_service(state, task, status),
    }
    Ok(())
}

pub fn handle_task_exit(frame: &mut ExceptionFrame, status: i64) -> Option<(u64, u64)> {
    if !active_on_current_cpu() {
        return None;
    }
    let _guard = lock_state();
    let cpu = current_cpu();
    let state = unsafe { &mut *STATE.0.get() };
    let task = state.current[cpu];
    if task == 0 || task >= APPLICATION_START + APPLICATION_COUNT {
        return None;
    }
    if task < SERVICE_COUNT {
        finish_service(state, task, status);
    } else {
        finish_application(state, task, status);
    }
    let next = next_other_runnable(state, task, cpu)?;
    set_running(state, cpu, next);
    preempt::load(&state.contexts[next], frame);
    record_switch(cpu);
    Some((2, (task + 1) as u64))
}

pub fn write_application_status(pid: u64, pointer: u64, mode: u64) -> Result<(), Status> {
    if !active_on_current_cpu() {
        return Err(Status::AccessDenied);
    }
    let _guard = lock_state();
    if current_task(unsafe { &*STATE.0.get() }) != 0 {
        return Err(Status::AccessDenied);
    }
    let (task, index) = application_slot(pid)?;
    let state = unsafe { &*STATE.0.get() };
    if state.applications[index].program == 0 {
        return Err(Status::NotFound);
    }
    let running = !matches!(state.threads[task], ThreadState::Exited);
    let (frames, pools, mappings) = if running {
        task_live_resources(task)
    } else {
        (
            state.applications[index].reclaimed_frames,
            state.applications[index].reclaimed_pools,
            state.applications[index].reclaimed_mappings,
        )
    };
    if mode == 5 {
        let output = microsystem_abi::ProcessInfoV2 {
            version: 2,
            running: running as u16,
            pid: pid as u32,
            exit_status: state.applications[index].exit_status,
            program: state.applications[index].program,
            cpu_ticks: state.applications[index].cpu_ticks,
            cpu_mask: state.applications[index].cpu_mask,
            owned_pages: if running { task_owned_pages(task) } else { 0 },
        };
        let bytes = mem::size_of::<microsystem_abi::ProcessInfoV2>();
        if !validate_user_range(0, pointer, bytes as u64, true)
            || copy_to_user(
                0,
                pointer,
                (&output as *const microsystem_abi::ProcessInfoV2).cast::<u8>(),
                bytes,
            )
            .is_none()
        {
            return Err(Status::Fault);
        }
        return Ok(());
    }
    let output = [
        running as i64,
        state.applications[index].exit_status,
        state.applications[index].cpu_mask as i64,
        frames as i64,
        pools as i64,
        mappings as i64,
    ];
    let output_words = match mode {
        0 => 2,
        3 => 3,
        4 => output.len(),
        _ => return Err(Status::Invalid),
    };
    let output_bytes = output_words * core::mem::size_of::<i64>();
    if !validate_user_range(0, pointer, output_bytes as u64, true)
        || copy_to_user(0, pointer, output.as_ptr().cast::<u8>(), output_bytes).is_none()
    {
        return Err(Status::Fault);
    }
    Ok(())
}

fn task_owned_pages(task: usize) -> u32 {
    let tables = unsafe { &*CAPS.0.get() };
    let mut objects = [0u32; MAX_CAPABILITIES];
    let mut count = 0usize;
    let mut pages = 0u32;
    for capability in tables[task].capabilities() {
        if !matches!(
            capability.object_type,
            ObjectType::Frame | ObjectType::FrameRegion
        ) || objects[..count].contains(&capability.object)
        {
            continue;
        }
        objects[count] = capability.object;
        count += 1;
        pages = pages.saturating_add(
            frame_region(capability.object)
                .map(|region| region.pages as u32)
                .unwrap_or(1),
        );
    }
    pages.saturating_add(virtual_memory::resident_pages(task))
}

pub fn kill_application(pid: u64, status: i64) -> Result<(), Status> {
    if !active_on_current_cpu() {
        return Err(Status::AccessDenied);
    }
    let _guard = lock_state();
    if current_task(unsafe { &*STATE.0.get() }) != 0 {
        return Err(Status::AccessDenied);
    }
    let (task, index) = application_slot(pid)?;
    let state = unsafe { &mut *STATE.0.get() };
    if state.applications[index].program == 0 {
        return Err(Status::NotFound);
    }
    if matches!(state.threads[task], ThreadState::Exited) {
        return Err(Status::Invalid);
    }
    if matches!(state.threads[task], ThreadState::Terminating { .. }) {
        return Err(Status::Busy);
    }
    let running_cpu = match state.threads[task] {
        ThreadState::Running { cpu } => Some(cpu),
        _ => None,
    };
    if let Some(cpu) = running_cpu {
        state.threads[task] = ThreadState::Terminating { cpu, status };
        state.applications[index].exit_status = status;
        arch::send_reschedule(cpu);
    } else {
        finish_application(state, task, status);
    }
    Ok(())
}

pub fn switches() -> u64 {
    SWITCHES.load(Ordering::Acquire)
}

pub fn notification_diagnostics() -> (u64, u64, u64) {
    (
        NOTIFICATION_BLOCKS.load(Ordering::Acquire),
        DEVICE_IRQ_ACKS.load(Ordering::Acquire),
        RESCHEDULE_WAKEUPS.load(Ordering::Acquire),
    )
}

pub fn copy_user_read(pointer: u64, output: &mut [u8]) -> Result<(), Status> {
    if !active_on_current_cpu() {
        return Err(Status::AccessDenied);
    }
    let _guard = lock_state();
    let current = current_task(unsafe { &*STATE.0.get() });
    if !validate_user_range(current, pointer, output.len() as u64, false) {
        return Err(Status::Fault);
    }
    copy_from_user(current, pointer, output.as_mut_ptr(), output.len()).ok_or(Status::Fault)
}

pub fn active_on_current_cpu() -> bool {
    let cpu = arch::cpu_id();
    cpu < CPU_COUNT && ACTIVE_CPUS.load(Ordering::Acquire) & (1 << cpu) != 0
}

pub fn current_task_owns_console() -> bool {
    if !active_on_current_cpu() {
        return false;
    }
    let _guard = lock_state();
    current_task(unsafe { &*STATE.0.get() }) == CONSOLE_TASK
}

pub fn active_cpu_mask() -> u32 {
    ACTIVE_CPUS.load(Ordering::Acquire)
}

pub fn panic_context(cpu: usize) -> (usize, u64, u64) {
    let state = unsafe { &*STATE.0.get() };
    let cpu = cpu.min(CPU_COUNT - 1);
    let task = state.current[cpu].min(TASK_COUNT - 1);
    let context = state.contexts[task];
    (task, context.elr, context.ttbr0)
}

fn switch(frame: &mut ExceptionFrame) {
    let cpu = current_cpu();
    let state = unsafe { &mut *STATE.0.get() };
    let current = state.current[cpu];
    preempt::save(&mut state.contexts[current], frame);
    if matches!(state.threads[current], ThreadState::Running { cpu: owner } if owner == cpu) {
        state.threads[current] = ThreadState::Runnable;
    } else if let ThreadState::Terminating { cpu: owner, status } = state.threads[current] {
        if owner == cpu {
            if current < SERVICE_COUNT {
                finish_service(state, current, status);
            } else {
                finish_application(state, current, status);
            }
        }
    }
    let Some(next) = next_runnable(state, current, cpu) else {
        return;
    };
    set_running(state, cpu, next);
    preempt::load(&state.contexts[next], frame);
    record_switch(cpu);
}

fn call(frame: &mut ExceptionFrame) -> u64 {
    let request_pointer = frame.registers[1];
    let reply_pointer = frame.registers[2];
    let deadline = frame.registers[3];
    let cpu = current_cpu();
    let state = unsafe { &mut *STATE.0.get() };
    let caller = state.current[cpu];
    let endpoint =
        match resolve_endpoint(caller, CapHandle(frame.registers[0] as u32), Rights::WRITE) {
            Ok(endpoint) => endpoint,
            Err(status) => return fail(frame, status),
        };
    let Some(endpoint_index) = endpoint
        .checked_sub(1)
        .filter(|index| *index < ENDPOINT_COUNT)
    else {
        return fail(frame, Status::BadCapability);
    };
    if deadline_elapsed(deadline) {
        return fail(frame, Status::TimedOut);
    }
    let Some(request) = read_message(caller, request_pointer) else {
        return fail(frame, Status::Fault);
    };
    if request.version != ABI_VERSION
        || !validate_user_range(
            caller,
            reply_pointer,
            core::mem::size_of::<Message>() as u64,
            true,
        )
    {
        return fail(frame, Status::Invalid);
    }
    let server = ENDPOINT_OWNERS[endpoint_index];
    if matches!(
        state.threads[server],
        ThreadState::Exited | ThreadState::Terminating { .. }
    ) {
        return fail(frame, Status::Io);
    }
    let waiting_buffer = match state.threads[server] {
        ThreadState::Receiving {
            endpoint: waiting_endpoint,
            buffer,
            ..
        } if waiting_endpoint == endpoint => Some(buffer),
        _ => None,
    };
    if waiting_buffer.is_none() && state.endpoints[endpoint_index].pending.is_some() {
        return fail(frame, Status::Busy);
    }
    let Some(next) = waiting_buffer
        .map(|_| server)
        .or_else(|| next_other_runnable(state, caller, cpu))
    else {
        return fail(frame, Status::Busy);
    };

    if let Some(buffer) = waiting_buffer {
        let transfer = match stage_cap_transfer(caller, server, request) {
            Ok(transfer) => transfer,
            Err(status) => return fail(frame, status),
        };
        if !write_message(server, buffer, &transfer.message) {
            transfer.rollback();
            return fail(frame, Status::Fault);
        }
        transfer.commit();
    }

    state.requests[caller] = request;
    preempt::save(&mut state.contexts[caller], frame);
    if waiting_buffer.is_some() {
        state.threads[caller] = ThreadState::WaitingReply {
            server,
            reply_buffer: reply_pointer,
            deadline,
        };
        state.threads[server] = ThreadState::Runnable;
        state.reply_to[server] = Some(caller);
        state.contexts[server].registers[0] = Status::Ok as i64 as u64;
    } else {
        state.threads[caller] = ThreadState::Sending {
            endpoint,
            reply_buffer: reply_pointer,
            deadline,
        };
        state.endpoints[endpoint_index].pending = Some(caller);
    }
    set_running(state, cpu, next);
    preempt::load(&state.contexts[next], frame);
    record_switch(cpu);
    2
}

fn try_call(frame: &mut ExceptionFrame) -> u64 {
    let state = unsafe { &mut *STATE.0.get() };
    let caller = current_task(state);
    if !matches!(state.async_replies[caller], ReplySlot::Idle) {
        return fail(frame, Status::Busy);
    }
    let endpoint =
        match resolve_endpoint(caller, CapHandle(frame.registers[0] as u32), Rights::WRITE) {
            Ok(endpoint) => endpoint,
            Err(status) => return fail(frame, status),
        };
    let Some(request) = read_message(caller, frame.registers[1]) else {
        return fail(frame, Status::Fault);
    };
    if request.version != ABI_VERSION
        || request.caps != [CapHandle::INVALID; MESSAGE_CAPS]
        || request.flags & MESSAGE_CAP_MOVE_MASK != 0
    {
        return fail(frame, Status::Invalid);
    }
    let server = ENDPOINT_OWNERS[endpoint - 1];
    if matches!(state.threads[server], ThreadState::Exited) {
        return fail(frame, Status::NotFound);
    }
    let ThreadState::Receiving {
        endpoint: waiting,
        buffer,
        ..
    } = state.threads[server]
    else {
        return fail(frame, Status::Busy);
    };
    if waiting != endpoint || state.endpoints[endpoint - 1].pending.is_some() {
        return fail(frame, Status::Busy);
    }
    if !write_message(server, buffer, &request) {
        return fail(frame, Status::Fault);
    }
    let _ = state.async_replies[caller].begin(server as u32);
    state.reply_to[server] = Some(caller);
    state.threads[server] = ThreadState::Runnable;
    state.contexts[server].registers[0] = Status::Ok as i64 as u64;
    arch::send_reschedule(0);
    arch::send_reschedule(1);
    frame.registers[0] = Status::Ok as i64 as u64;
    0
}

fn poll_reply(frame: &mut ExceptionFrame) -> u64 {
    let state = unsafe { &mut *STATE.0.get() };
    let caller = current_task(state);
    match state.async_replies[caller].result() {
        Ok(message) => {
            if !write_message(caller, frame.registers[0], &message) {
                return fail(frame, Status::Fault);
            }
            state.async_replies[caller] = ReplySlot::Idle;
            frame.registers[0] = Status::Ok as i64 as u64;
            0
        }
        Err(status) => {
            if matches!(state.async_replies[caller], ReplySlot::Failed(_)) {
                state.async_replies[caller] = ReplySlot::Idle;
            }
            fail(frame, status)
        }
    }
}

fn receive(frame: &mut ExceptionFrame) -> u64 {
    let state = unsafe { &*STATE.0.get() };
    let endpoint = match resolve_endpoint(
        current_task(state),
        CapHandle(frame.registers[0] as u32),
        Rights::READ,
    ) {
        Ok(endpoint) => endpoint,
        Err(status) => return fail(frame, status),
    };
    let buffer = frame.registers[1];
    let deadline = frame.registers[2];
    receive_after_reply(frame, endpoint, buffer, deadline, None)
}

fn reply_receive(frame: &mut ExceptionFrame) -> u64 {
    let state = unsafe { &mut *STATE.0.get() };
    let endpoint = match resolve_endpoint(
        current_task(state),
        CapHandle(frame.registers[0] as u32),
        Rights::READ,
    ) {
        Ok(endpoint) => endpoint,
        Err(status) => return fail(frame, status),
    };
    let reply_pointer = frame.registers[1];
    let buffer = frame.registers[2];
    let deadline = frame.registers[3];
    let server = current_task(state);
    let Some(reply) = read_message(server, reply_pointer) else {
        return fail(frame, Status::Fault);
    };
    if reply.version != ABI_VERSION {
        return fail(frame, Status::Invalid);
    }
    let Some(caller) = state.reply_to[server] else {
        // A cancelled caller never receives the result's transferred ownership.
        for index in 0..MESSAGE_CAPS {
            if reply.flags & message_cap_move(index) != 0 && reply.caps[index] != CapHandle::INVALID
            {
                let _ = delete_capability(server, reply.caps[index]);
            }
        }
        return receive_after_reply(frame, endpoint, buffer, deadline, None);
    };
    if state.async_replies[caller].waiting_for(server as u32) {
        if reply.caps != [CapHandle::INVALID; MESSAGE_CAPS]
            || reply.flags & MESSAGE_CAP_MOVE_MASK != 0
        {
            state.async_replies[caller] = ReplySlot::Failed(Status::NotSupported);
        } else {
            let _ = state.async_replies[caller].complete(server as u32, reply);
        }
        state.reply_to[server] = None;
        return receive_after_reply(frame, endpoint, buffer, deadline, None);
    }
    receive_after_reply(frame, endpoint, buffer, deadline, Some((caller, reply)))
}

fn receive_after_reply(
    frame: &mut ExceptionFrame,
    endpoint: usize,
    buffer: u64,
    deadline: u64,
    reply: Option<(usize, Message)>,
) -> u64 {
    let cpu = current_cpu();
    let state = unsafe { &mut *STATE.0.get() };
    let server = state.current[cpu];
    let Some(endpoint_index) = endpoint
        .checked_sub(1)
        .filter(|index| *index < ENDPOINT_COUNT)
    else {
        return fail(frame, Status::BadCapability);
    };
    if ENDPOINT_OWNERS[endpoint_index] != server {
        return fail(frame, Status::AccessDenied);
    }
    let mut reply_transfer = None;
    if let Some((caller, message)) = reply {
        let ThreadState::WaitingReply {
            server: expected_server,
            reply_buffer,
            ..
        } = state.threads[caller]
        else {
            return fail(frame, Status::Invalid);
        };
        if expected_server != server {
            return fail(frame, Status::Invalid);
        }
        reply_transfer = match stage_cap_transfer(server, caller, message) {
            Ok(transfer) => Some((reply_buffer, transfer)),
            Err(status) => return fail(frame, status),
        };
    }

    if deadline_elapsed(deadline) {
        if let Some((reply_buffer, transfer)) = reply_transfer {
            if !write_message(transfer.receiver, reply_buffer, &transfer.message) {
                transfer.rollback();
                return fail(frame, Status::Fault);
            }
            transfer.commit();
        }
        if let Some((caller, _)) = reply {
            state.threads[caller] = ThreadState::Runnable;
            state.contexts[caller].registers[0] = Status::Ok as i64 as u64;
            state.reply_to[server] = None;
        }
        return fail(frame, Status::TimedOut);
    }
    if !validate_user_range(server, buffer, core::mem::size_of::<Message>() as u64, true) {
        if let Some((_, transfer)) = reply_transfer {
            transfer.rollback();
        }
        return fail(frame, Status::Fault);
    }

    let pending = state.endpoints[endpoint_index].pending;
    if let Some(caller) = pending {
        let request = state.requests[caller];
        let ThreadState::Sending {
            reply_buffer,
            deadline,
            ..
        } = state.threads[caller]
        else {
            if let Some((_, transfer)) = reply_transfer {
                transfer.rollback();
            }
            return fail(frame, Status::Invalid);
        };
        let request_transfer = match stage_cap_transfer(caller, server, request) {
            Ok(transfer) => transfer,
            Err(status) => {
                if let Some((_, transfer)) = reply_transfer {
                    transfer.rollback();
                }
                return fail(frame, status);
            }
        };
        if let Some((reply_buffer, transfer)) = reply_transfer {
            if !write_message(transfer.receiver, reply_buffer, &transfer.message) {
                transfer.rollback();
                request_transfer.rollback();
                return fail(frame, Status::Fault);
            }
        }
        if !write_message(server, buffer, &request_transfer.message) {
            if let Some((_, transfer)) = reply_transfer {
                transfer.rollback();
            }
            request_transfer.rollback();
            return fail(frame, Status::Fault);
        }
        if let Some((_, transfer)) = reply_transfer {
            transfer.commit();
        }
        request_transfer.commit();
        if let Some((previous, _)) = reply {
            state.threads[previous] = ThreadState::Runnable;
            state.contexts[previous].registers[0] = Status::Ok as i64 as u64;
        }
        state.endpoints[endpoint_index].pending = None;
        state.threads[caller] = ThreadState::WaitingReply {
            server,
            reply_buffer,
            deadline,
        };
        state.reply_to[server] = Some(caller);
        frame.registers[0] = Status::Ok as i64 as u64;
        return 0;
    }

    let Some(next) = reply
        .map(|(caller, _)| caller)
        .or_else(|| next_other_runnable(state, server, cpu))
    else {
        if let Some((_, transfer)) = reply_transfer {
            transfer.rollback();
        }
        return fail(frame, Status::Busy);
    };
    if let Some((reply_buffer, transfer)) = reply_transfer {
        if !write_message(transfer.receiver, reply_buffer, &transfer.message) {
            transfer.rollback();
            return fail(frame, Status::Fault);
        }
        transfer.commit();
    }
    preempt::save(&mut state.contexts[server], frame);
    if let Some((caller, _)) = reply {
        state.threads[caller] = ThreadState::Runnable;
        state.contexts[caller].registers[0] = Status::Ok as i64 as u64;
        state.reply_to[server] = None;
    }
    state.threads[server] = ThreadState::Receiving {
        endpoint,
        buffer,
        deadline,
    };
    set_running(state, cpu, next);
    preempt::load(&state.contexts[next], frame);
    record_switch(cpu);
    2
}

fn resolve_endpoint(task: usize, handle: CapHandle, rights: Rights) -> Result<usize, Status> {
    let capability = unsafe { &(&*CAPS.0.get())[task] }.capability(handle, rights)?;
    if capability.object_type != ObjectType::Endpoint
        || !(1..=ENDPOINT_COUNT as u32).contains(&capability.object)
    {
        return Err(Status::BadCapability);
    }
    Ok(capability.object as usize)
}

fn install_boot_capabilities() -> Result<(), Status> {
    let filesystem_physical = physical_memory::allocate_zeroed()?;
    FILESYSTEM_FRAME_PHYSICAL.store(filesystem_physical, Ordering::Release);
    let ssh_filesystem_physical = physical_memory::allocate_zeroed()?;
    SSH_FILESYSTEM_FRAME_PHYSICAL.store(ssh_filesystem_physical, Ordering::Release);
    let windowd_filesystem_physical = physical_memory::allocate_zeroed()?;
    WINDOWD_FILESYSTEM_FRAME_PHYSICAL.store(windowd_filesystem_physical, Ordering::Release);
    let terminal_filesystem_physical = physical_memory::allocate_zeroed()?;
    TERMINAL_FILESYSTEM_FRAME_PHYSICAL.store(terminal_filesystem_physical, Ordering::Release);
    let database_filesystem_physical = physical_memory::allocate_zeroed()?;
    DATABASE_FILESYSTEM_FRAME_PHYSICAL.store(database_filesystem_physical, Ordering::Release);
    let root_filesystem_physical = physical_memory::allocate_zeroed()?;
    ROOT_FILESYSTEM_FRAME_PHYSICAL.store(root_filesystem_physical, Ordering::Release);
    let network_filesystem_physical = physical_memory::allocate_zeroed()?;
    NETWORK_FILESYSTEM_FRAME_PHYSICAL.store(network_filesystem_physical, Ordering::Release);
    let identity_physical = physical_memory::allocate_zeroed()?;
    IDENTITY_SNAPSHOT_PHYSICAL.store(identity_physical, Ordering::Release);
    map_page(task_memory_mut(0).ok_or(Status::Fault)?, microsystem_abi::identity::SNAPSHOT_VA as u64, identity_physical, normal_page_flags());
    map_page(
        task_memory_mut(0).ok_or(Status::Fault)?,
        ROOT_FILESYSTEM_DATA_VA,
        root_filesystem_physical,
        normal_page_flags(),
    );
    let tables = unsafe { &mut *CAPS.0.get() };
    let shared_rights = Rights(Rights::READ.0 | Rights::WRITE.0 | Rights::MAP.0 | Rights::GRANT.0);
    NOTIFICATION_OBJECTS[0].store(TERMINAL_CANCEL_OBJECT, Ordering::Release);
    for (task, rights) in [(WINDOWD_TASK, Rights::WRITE), (TERMINAL_TASK, Rights::READ)] {
        tables[task].insert_root_at(
            microsystem_abi::boot_cap::GUI_TERMINAL_NOTIFICATION,
            TERMINAL_CANCEL_OBJECT,
            ObjectType::Notification,
            rights,
            TERMINAL_CANCEL_NODE,
        )?;
    }
    for task in [BLOCK_TASK, MFS_TASK] {
        tables[task].insert_root_at(
            microsystem_abi::boot_cap::SHARED_BLOCK_FRAME,
            SHARED_FRAME_OBJECT,
            ObjectType::Frame,
            shared_rights,
            SHARED_FRAME_NODE,
        )?;
    }
    for task in [MFS_TASK, SHELL_TASK, DB_TASK] {
        tables[task].insert_root_at(
            microsystem_abi::boot_cap::SHARED_FILESYSTEM_FRAME,
            FILESYSTEM_FRAME_OBJECT,
            ObjectType::Frame,
            shared_rights,
            FILESYSTEM_FRAME_NODE,
        )?;
    }
    for task in [MFS_TASK, SSHD_TASK] {
        tables[task].insert_root_at(
            microsystem_abi::boot_cap::SSH_FILESYSTEM_FRAME,
            SSH_FILESYSTEM_FRAME_OBJECT,
            ObjectType::Frame,
            shared_rights,
            SSH_FILESYSTEM_FRAME_NODE,
        )?;
    }
    for task in [MFS_TASK, WINDOWD_TASK] {
        tables[task].insert_root_at(
            microsystem_abi::boot_cap::WINDOWD_FILESYSTEM_FRAME,
            WINDOWD_FILESYSTEM_FRAME_OBJECT,
            ObjectType::Frame,
            shared_rights,
            WINDOWD_FILESYSTEM_FRAME_NODE,
        )?;
    }
    for task in [MFS_TASK, WINDOWD_TASK, TERMINAL_TASK] {
        tables[task].insert_root_at(
            microsystem_abi::boot_cap::GUI_TERMINAL_COMMANDS,
            TERMINAL_FILESYSTEM_FRAME_OBJECT,
            ObjectType::Frame,
            shared_rights,
            TERMINAL_FILESYSTEM_FRAME_NODE,
        )?;
    }
    for task in [MFS_TASK, DB_TASK] {
        tables[task].insert_root_at(
            microsystem_abi::boot_cap::DATABASE_FILESYSTEM_FRAME,
            DATABASE_FILESYSTEM_FRAME_OBJECT,
            ObjectType::Frame,
            shared_rights,
            DATABASE_FILESYSTEM_FRAME_NODE,
        )?;
    }
    for task in [0, MFS_TASK] {
        tables[task].insert_root_at(
            microsystem_abi::boot_cap::ROOT_FILESYSTEM_FRAME,
            ROOT_FILESYSTEM_FRAME_OBJECT,
            ObjectType::Frame,
            shared_rights,
            ROOT_FILESYSTEM_FRAME_NODE,
        )?;
    }
    for task in [MFS_TASK, NETD_TASK] {
        tables[task].insert_root_at(microsystem_abi::boot_cap::NETWORK_FILESYSTEM_FRAME,
            NETWORK_FILESYSTEM_FRAME_OBJECT, ObjectType::Frame, shared_rights, NETWORK_FILESYSTEM_FRAME_NODE)?;
    }
    for task in [0, MFS_TASK, SHELL_TASK, WINDOWD_TASK, TERMINAL_TASK, FILES_TASK, MONITOR_TASK, SSHD_TASK, NETD_TASK, DB_TASK] {
        let rights = if task == 0 { Rights(Rights::READ.0 | Rights::WRITE.0 | Rights::MAP.0) } else { Rights(Rights::READ.0 | Rights::MAP.0) };
        tables[task].insert_root_at(microsystem_abi::boot_cap::IDENTITY_SNAPSHOT, IDENTITY_SNAPSHOT_OBJECT, ObjectType::Frame, rights, IDENTITY_SNAPSHOT_NODE)?;
    }
    tables[BLOCK_TASK].insert_root_at(
        microsystem_abi::boot_cap::BLOCK_IRQ_NOTIFICATION,
        DEVICE_NOTIFICATION_OBJECT,
        ObjectType::Notification,
        Rights(Rights::READ.0 | Rights::ACK.0),
        DEVICE_NOTIFICATION_NODE,
    )?;
    tables[WINDOWD_TASK].insert_root_at(
        microsystem_abi::boot_cap::GUI_MEMORY_POOL,
        GUI_MEMORY_POOL_OBJECT,
        ObjectType::MemoryPool,
        Rights::MANAGE,
        GUI_MEMORY_POOL_NODE,
    )?;
    tables[0].insert_root_at(
        microsystem_abi::boot_cap::SCRIPT_MEMORY_POOL,
        SCRIPT_MEMORY_POOL_OBJECT,
        ObjectType::MemoryPool,
        Rights(Rights::MANAGE.0 | Rights::GRANT.0),
        SCRIPT_MEMORY_POOL_NODE,
    )?;
    tables[NETD_TASK].insert_root_at(
        microsystem_abi::boot_cap::NETWORK_MEMORY_POOL,
        NETWORK_MEMORY_POOL_OBJECT,
        ObjectType::MemoryPool,
        Rights::MANAGE,
        NETWORK_MEMORY_POOL_NODE,
    )?;
    for task in [SHELL_TASK, TERMINAL_TASK, MONITOR_TASK] {
        tables[task].insert_root_at(
            microsystem_abi::boot_cap::SYSTEM_INFO,
            60,
            ObjectType::SystemInfo,
            Rights::READ,
            22,
        )?;
    }
    tables[0].insert_root_at(
        microsystem_abi::boot_cap::SYSTEM_INFO,
        60,
        ObjectType::SystemInfo,
        Rights(Rights::READ.0 | Rights::GRANT.0),
        22,
    )?;
    tables[0].insert_root_at(
        microsystem_abi::boot_cap::RANDOM_SOURCE,
        62,
        ObjectType::RandomSource,
        Rights(Rights::READ.0 | Rights::GRANT.0),
        RANDOM_SOURCE_NODE,
    )?;
    tables[NETD_TASK].insert_root_at(
        microsystem_abi::boot_cap::NETWORK_DEVICE,
        61,
        ObjectType::NetworkDevice,
        Rights(Rights::READ.0 | Rights::WRITE.0),
        NETWORK_DEVICE_NODE,
    )?;
    tables[NETD_TASK].insert_root_at(
        microsystem_abi::boot_cap::RANDOM_SOURCE,
        62,
        ObjectType::RandomSource,
        Rights::READ,
        RANDOM_SOURCE_NODE,
    )?;
    tables[SSHD_TASK].insert_root_at(
        microsystem_abi::boot_cap::RANDOM_SOURCE,
        62,
        ObjectType::RandomSource,
        Rights::READ,
        RANDOM_SOURCE_NODE,
    )?;
    tables[SSHD_TASK].insert_root_at(
        microsystem_abi::boot_cap::SYSTEM_INFO,
        60,
        ObjectType::SystemInfo,
        Rights::READ,
        22,
    )?;
    tables[0].insert_root_at(
        microsystem_abi::boot_cap::DEVMGR_VIRTIO_IRQ,
        DEVICE_IRQ_OBJECT,
        ObjectType::Irq,
        Rights(Rights::ACK.0 | Rights::MANAGE.0 | Rights::GRANT.0),
        DEVICE_IRQ_NODE,
    )?;
    tables[DEVMGR_TASK].insert_root_at(
        microsystem_abi::boot_cap::DEVICE_SYSTEM_CONTROL,
        1,
        ObjectType::SystemControl,
        Rights::MANAGE,
        16,
    )?;
    for task in [SHELL_TASK, TERMINAL_TASK] {
        tables[task].insert_root_at(
            microsystem_abi::boot_cap::SHELL_SYSTEM_CONTROL,
            1,
            ObjectType::SystemControl,
            Rights::MANAGE,
            16,
        )?;
    }
    tables[0].insert_root_at(
        microsystem_abi::boot_cap::DEVMGR_VIRTIO_MMIO,
        DEVICE_MMIO_OBJECT,
        ObjectType::MmioRegion,
        Rights(Rights::READ.0 | Rights::WRITE.0 | Rights::MAP.0 | Rights::GRANT.0),
        DEVICE_MMIO_NODE,
    )?;
    tables[0].insert_root_at(
        microsystem_abi::boot_cap::DEVMGR_QUEUE_FRAME,
        DEVICE_QUEUE_OBJECT,
        ObjectType::Frame,
        Rights(Rights::READ.0 | Rights::WRITE.0 | Rights::MAP.0 | Rights::GRANT.0),
        DEVICE_QUEUE_NODE,
    )?;
    tables[0].insert_root_at(
        microsystem_abi::boot_cap::DEVMGR_DMA_DOMAIN,
        DMA_DOMAIN_OBJECT,
        ObjectType::DmaDomain,
        Rights(Rights::READ.0 | Rights::WRITE.0 | Rights::MAP.0 | Rights::GRANT.0),
        DMA_DOMAIN_NODE,
    )?;
    tables[0].insert_root_at(
        microsystem_abi::boot_cap::ROOT_MEMORY_POOL,
        ROOT_MEMORY_POOL_OBJECT,
        ObjectType::MemoryPool,
        Rights(Rights::MANAGE.0 | Rights::GRANT.0),
        ROOT_MEMORY_POOL_NODE,
    )?;
    tables[BLOCK_TASK].insert_root_at(
        microsystem_abi::boot_cap::DRIVER_MEMORY_POOL,
        DRIVER_MEMORY_POOL_OBJECT,
        ObjectType::MemoryPool,
        Rights(Rights::MANAGE.0),
        DRIVER_MEMORY_POOL_NODE,
    )?;
    for (task, rights) in [
        (CONSOLE_TASK, Rights::READ),
        (0, Rights::WRITE),
        (SHELL_TASK, Rights::WRITE),
    ] {
        insert_endpoint_cap(
            tables,
            task,
            microsystem_abi::boot_cap::CONSOLE_ENDPOINT,
            1,
            rights,
            CONSOLE_ENDPOINT_NODE,
        )?;
    }
    insert_endpoint_cap(
        tables,
        WINDOWD_TASK,
        microsystem_abi::boot_cap::GUI_FILESYSTEM_ENDPOINT,
        3,
        Rights::WRITE,
        FILESYSTEM_ENDPOINT_NODE,
    )?;
    insert_endpoint_cap(
        tables,
        TERMINAL_TASK,
        microsystem_abi::boot_cap::TERMINAL_FILESYSTEM_ENDPOINT,
        3,
        Rights::WRITE,
        FILESYSTEM_ENDPOINT_NODE,
    )?;
    for (task, rights) in [(BLOCK_TASK, Rights::READ), (MFS_TASK, Rights::WRITE)] {
        insert_endpoint_cap(
            tables,
            task,
            microsystem_abi::boot_cap::BLOCK_ENDPOINT,
            2,
            rights,
            BLOCK_ENDPOINT_NODE,
        )?;
    }
    for (task, rights) in [
        (MFS_TASK, Rights::READ),
        (SHELL_TASK, Rights::WRITE),
        (SSHD_TASK, Rights::WRITE),
        (DB_TASK, Rights::WRITE),
        (NETD_TASK, Rights::WRITE),
        (0, Rights(Rights::WRITE.0 | Rights::GRANT.0)),
    ] {
        insert_endpoint_cap(
            tables,
            task,
            microsystem_abi::boot_cap::FILESYSTEM_ENDPOINT,
            3,
            rights,
            FILESYSTEM_ENDPOINT_NODE,
        )?;
    }
    for (task, rights) in [(DB_TASK, Rights::READ), (SHELL_TASK, Rights::WRITE)] {
        insert_endpoint_cap(
            tables,
            task,
            microsystem_abi::boot_cap::DATABASE_ENDPOINT,
            26,
            rights,
            DATABASE_ENDPOINT_NODE,
        )?;
    }
    for (task, rights) in [
        (0, Rights::READ),
        (SHELL_TASK, Rights::WRITE),
        (TERMINAL_TASK, Rights::WRITE),
        (MONITOR_TASK, Rights::WRITE),
        (SSHD_TASK, Rights::WRITE),
    ] {
        insert_endpoint_cap(
            tables,
            task,
            microsystem_abi::boot_cap::PROCESS_ENDPOINT,
            4,
            rights,
            PROCESS_ENDPOINT_NODE,
        )?;
    }
    for (task, rights) in [
        (0, Rights::READ),
        (SHELL_TASK, Rights::WRITE),
        (TERMINAL_TASK, Rights::WRITE),
        (MONITOR_TASK, Rights::WRITE),
    ] {
        insert_endpoint_cap(
            tables,
            task,
            microsystem_abi::boot_cap::TIME_ENDPOINT,
            4,
            rights,
            TIME_ENDPOINT_NODE,
        )?;
    }
    for (task, rights) in [(DEVMGR_TASK, Rights::READ), (0, Rights::WRITE)] {
        insert_endpoint_cap(
            tables,
            task,
            microsystem_abi::boot_cap::DEVMGR_ENDPOINT,
            5,
            rights,
            DEVMGR_ENDPOINT_NODE,
        )?;
    }
    for (task, rights) in [(BLOCK_TASK, Rights::READ), (DEVMGR_TASK, Rights::WRITE)] {
        insert_endpoint_cap(
            tables,
            task,
            microsystem_abi::boot_cap::BLOCK_CONFIG_ENDPOINT,
            6,
            rights,
            BLOCK_CONFIG_ENDPOINT_NODE,
        )?;
    }
    insert_endpoint_cap(
        tables,
        0,
        microsystem_abi::boot_cap::NETWORK_ENDPOINT,
        13,
        Rights(Rights::WRITE.0 | Rights::GRANT.0),
        30,
    )?;
    insert_endpoint_cap(
        tables,
        NETD_TASK,
        microsystem_abi::boot_cap::NETWORK_ENDPOINT,
        13,
        Rights::READ,
        30,
    )?;
    for task in [SHELL_TASK, TERMINAL_TASK] {
        insert_endpoint_cap(
            tables,
            task,
            microsystem_abi::boot_cap::NETWORK_ENDPOINT,
            13,
            Rights::WRITE,
            30,
        )?;
    }
    insert_endpoint_cap(
        tables,
        SSHD_TASK,
        microsystem_abi::boot_cap::SSH_NETWORK_ENDPOINT,
        14,
        Rights::WRITE,
        31,
    )?;
    insert_endpoint_cap(
        tables,
        0,
        microsystem_abi::boot_cap::SCRIPT_BROKER_ENDPOINT,
        15,
        Rights::READ,
        32,
    )?;
    insert_endpoint_cap(
        tables,
        NETD_TASK,
        microsystem_abi::boot_cap::SSH_NETWORK_ENDPOINT,
        14,
        Rights::READ,
        31,
    )?;
    for (handle, object, node, client) in [
        (
            microsystem_abi::boot_cap::GUI_TERMINAL_ENDPOINT,
            7,
            GUI_TERMINAL_ENDPOINT_NODE,
            TERMINAL_TASK,
        ),
        (
            microsystem_abi::boot_cap::GUI_FILES_ENDPOINT,
            8,
            GUI_FILES_ENDPOINT_NODE,
            FILES_TASK,
        ),
        (
            microsystem_abi::boot_cap::GUI_MONITOR_ENDPOINT,
            9,
            GUI_MONITOR_ENDPOINT_NODE,
            MONITOR_TASK,
        ),
    ] {
        insert_endpoint_cap(tables, WINDOWD_TASK, handle, object, Rights::READ, node)?;
        insert_endpoint_cap(tables, client, handle, object, Rights::WRITE, node)?;
    }
    for (handle, object, node, client) in [
        (
            microsystem_abi::boot_cap::GUI_TERMINAL_EVENTS,
            10,
            GUI_TERMINAL_EVENTS_NODE,
            TERMINAL_TASK,
        ),
        (
            microsystem_abi::boot_cap::GUI_FILES_EVENTS,
            11,
            GUI_FILES_EVENTS_NODE,
            FILES_TASK,
        ),
        (
            microsystem_abi::boot_cap::GUI_MONITOR_EVENTS,
            12,
            GUI_MONITOR_EVENTS_NODE,
            MONITOR_TASK,
        ),
    ] {
        insert_endpoint_cap(tables, WINDOWD_TASK, handle, object, Rights::WRITE, node)?;
        insert_endpoint_cap(tables, client, handle, object, Rights::READ, node)?;
    }
    insert_endpoint_cap(
        tables,
        0,
        microsystem_abi::boot_cap::GUI_CONFIG_ENDPOINT,
        16,
        Rights(Rights::WRITE.0 | Rights::GRANT.0 | Rights::MANAGE.0),
        GUI_CONFIG_ENDPOINT_NODE,
    )?;
    insert_endpoint_cap(
        tables,
        0,
        microsystem_abi::boot_cap::GUI_LAUNCH_ENDPOINT,
        25,
        Rights::READ,
        GUI_LAUNCH_ENDPOINT_NODE,
    )?;
    insert_endpoint_cap(
        tables,
        WINDOWD_TASK,
        microsystem_abi::boot_cap::GUI_LAUNCH_ENDPOINT,
        25,
        Rights::WRITE,
        GUI_LAUNCH_ENDPOINT_NODE,
    )?;
    insert_endpoint_cap(
        tables,
        WINDOWD_TASK,
        microsystem_abi::boot_cap::GUI_CONFIG_ENDPOINT,
        16,
        Rights::READ,
        GUI_CONFIG_ENDPOINT_NODE,
    )?;
    for index in 0..microsystem_abi::gui::MAX_DYNAMIC_CLIENTS {
        let handle = microsystem_abi::boot_cap::gui_dynamic_endpoint(index);
        let object = 17 + index as u32;
        let node = GUI_DYNAMIC_ENDPOINT_NODE_BASE + index as u32;
        insert_endpoint_cap(
            tables,
            0,
            handle,
            object,
            Rights(Rights::WRITE.0 | Rights::GRANT.0 | Rights::MANAGE.0),
            node,
        )?;
        insert_endpoint_cap(tables, WINDOWD_TASK, handle, object, Rights::READ, node)?;
    }
    crate::kprintln!("[ipc] filesystem payload frame isolated from block DMA=true");
    insert_endpoint_cap(
        tables,
        0,
        microsystem_abi::boot_cap::SERVICE_ENDPOINT,
        27,
        Rights::READ,
        SERVICE_ENDPOINT_NODE,
    )?;
    for task in [SHELL_TASK, TERMINAL_TASK] {
        insert_endpoint_cap(
            tables,
            task,
            microsystem_abi::boot_cap::SERVICE_ENDPOINT,
            27,
            Rights::WRITE,
            SERVICE_ENDPOINT_NODE,
        )?;
    }
    for (task, rights) in [
        (0, Rights::READ),
        (SHELL_TASK, Rights::WRITE),
        (TERMINAL_TASK, Rights::WRITE),
    ] {
        insert_endpoint_cap(
            tables,
            task,
            microsystem_abi::boot_cap::APPLICATION_ENDPOINT,
            28,
            rights,
            APPLICATION_ENDPOINT_NODE,
        )?;
    }
    for (task, rights) in [(0, Rights::READ), (SHELL_TASK, Rights::WRITE), (TERMINAL_TASK, Rights::WRITE), (SSHD_TASK, Rights::WRITE)] {
        insert_endpoint_cap(tables, task, microsystem_abi::boot_cap::IDENTITY_ENDPOINT, 29, rights, IDENTITY_ENDPOINT_NODE)?;
    }
    Ok(())
}

fn insert_endpoint_cap(
    tables: &mut [CapabilityTable; TASK_COUNT],
    task: usize,
    handle: CapHandle,
    object: u32,
    rights: Rights,
    node: u32,
) -> Result<(), Status> {
    tables[task].insert_root_at(handle, object, ObjectType::Endpoint, rights, node)
}

fn allocate_derivation() -> u32 {
    loop {
        let node = NEXT_DERIVATION.fetch_add(1, Ordering::Relaxed);
        if node != 0 {
            return node;
        }
    }
}

fn stage_cap_transfer(
    sender: usize,
    receiver: usize,
    message: Message,
) -> Result<CapTransfer, Status> {
    if sender >= TASK_COUNT || receiver >= TASK_COUNT || sender == receiver {
        return Err(Status::Invalid);
    }
    let tables = unsafe { &mut *CAPS.0.get() };
    let mut capabilities: [Option<Capability>; MESSAGE_CAPS] = [None; MESSAGE_CAPS];
    let mut moved = [false; MESSAGE_CAPS];
    let mut destinations = [CapHandle::INVALID; MESSAGE_CAPS];
    let mut inserted = [false; MESSAGE_CAPS];
    let mut required_slots = 0usize;

    for index in 0..MESSAGE_CAPS {
        let source = message.caps[index];
        moved[index] = message.flags & message_cap_move(index) != 0;
        if source == CapHandle::INVALID {
            if moved[index] {
                return Err(Status::Invalid);
            }
            continue;
        }
        if message.caps[..index].contains(&source) {
            return Err(Status::Invalid);
        }
        let capability = tables[sender].capability(source, Rights::GRANT)?;
        capabilities[index] = Some(capability);
        if !moved[index]
            && ((capability.node == SHARED_FRAME_NODE && capability.object == SHARED_FRAME_OBJECT)
                || (capability.node == FILESYSTEM_FRAME_NODE
                    && capability.object == FILESYSTEM_FRAME_OBJECT)
                || (capability.node == SSH_FILESYSTEM_FRAME_NODE
                    && capability.object == SSH_FILESYSTEM_FRAME_OBJECT)
                || (capability.node == WINDOWD_FILESYSTEM_FRAME_NODE
                    && capability.object == WINDOWD_FILESYSTEM_FRAME_OBJECT)
                || (capability.node == TERMINAL_FILESYSTEM_FRAME_NODE
                    && capability.object == TERMINAL_FILESYSTEM_FRAME_OBJECT)
                || (capability.node == DATABASE_FILESYSTEM_FRAME_NODE
                    && capability.object == DATABASE_FILESYSTEM_FRAME_OBJECT)
                || (capability.node == ROOT_FILESYSTEM_FRAME_NODE
                    && capability.object == ROOT_FILESYSTEM_FRAME_OBJECT)
                || (capability.node == NETWORK_FILESYSTEM_FRAME_NODE
                    && capability.object == NETWORK_FILESYSTEM_FRAME_OBJECT))
        {
            if let Some(handle) = tables[receiver].handle_for_object_with_rights(
                capability.object,
                capability.object_type,
                capability.rights,
            ) {
                destinations[index] = handle;
                continue;
            }
        }
        required_slots += 1;
    }
    for (index, capability) in capabilities.iter().enumerate() {
        let Some(capability) = capability else {
            continue;
        };
        if !moved[index] {
            continue;
        }
        let mapped_by_sender = match capability.object_type {
            ObjectType::Frame => task_runtime_frame_is_mapped(sender, capability.object),
            ObjectType::FrameRegion => task_frame_region_is_mapped(sender, capability.object),
            _ => false,
        };
        if !mapped_by_sender {
            continue;
        }
        let retains_mapping_authority = tables[sender].capabilities().any(|other| {
            other.object == capability.object
                && other.object_type == capability.object_type
                && other.rights.contains(Rights::MAP)
                && !capabilities
                    .iter()
                    .enumerate()
                    .any(|(moved_index, moved_cap)| {
                        moved[moved_index]
                            && moved_cap.is_some_and(|moved_cap| moved_cap.node == other.node)
                    })
        });
        if !retains_mapping_authority {
            return Err(Status::Busy);
        }
    }
    if tables[receiver].free_slots() < required_slots {
        return Err(Status::NoMemory);
    }

    for index in 0..MESSAGE_CAPS {
        let Some(capability) = capabilities[index] else {
            continue;
        };
        if destinations[index] != CapHandle::INVALID {
            continue;
        }
        let destination = if moved[index] {
            tables[receiver].import_move(capability)
        } else {
            tables[receiver].import_copy(capability, allocate_derivation())
        };
        match destination {
            Ok(handle) => {
                destinations[index] = handle;
                inserted[index] = true;
            }
            Err(status) => {
                for rollback in 0..index {
                    if inserted[rollback] {
                        let _ = tables[receiver].delete(destinations[rollback]);
                    }
                }
                return Err(status);
            }
        }
    }

    let mut delivered = message;
    delivered.flags &= !MESSAGE_CAP_MOVE_MASK;
    delivered.caps = destinations;
    Ok(CapTransfer {
        message: delivered,
        sender,
        receiver,
        sources: message.caps,
        destinations,
        inserted,
        moved,
    })
}

impl CapTransfer {
    fn commit(self) {
        let tables = unsafe { &mut *CAPS.0.get() };
        for index in 0..MESSAGE_CAPS {
            if self.moved[index] && self.sources[index] != CapHandle::INVALID {
                let _ = tables[self.sender].delete(self.sources[index]);
            }
        }
    }

    fn rollback(self) {
        let tables = unsafe { &mut *CAPS.0.get() };
        for index in 0..MESSAGE_CAPS {
            if self.inserted[index] {
                let _ = tables[self.receiver].delete(self.destinations[index]);
            }
        }
    }
}

fn revoke_global(task: usize, handle: CapHandle) -> Result<usize, Status> {
    let tables = unsafe { &mut *CAPS.0.get() };
    let root = tables[task].capability(handle, Rights::MANAGE)?;
    let mut nodes = [0u32; TASK_COUNT * MAX_CAPABILITIES];
    nodes[0] = root.node;
    let mut count = 1usize;
    loop {
        let mut changed = false;
        for table in tables.iter() {
            changed |= table.collect_child_nodes(&mut nodes, &mut count);
        }
        if !changed {
            break;
        }
    }
    let descendants = &nodes[1..count];
    let mut deleted = 0usize;
    for (task, table) in tables.iter_mut().enumerate() {
        let mut frames = [0u32; MAX_CAPABILITIES];
        let mut frame_count = 0usize;
        for capability in table.capabilities() {
            if matches!(
                capability.object_type,
                ObjectType::Frame | ObjectType::FrameRegion
            ) && descendants.contains(&capability.node)
                && !frames[..frame_count].contains(&capability.object)
            {
                frames[frame_count] = capability.object;
                frame_count += 1;
            }
        }
        deleted += table.delete_nodes(descendants);
        for object in frames[..frame_count].iter().copied() {
            let object_type = if frame_region(object).is_some() {
                ObjectType::FrameRegion
            } else {
                ObjectType::Frame
            };
            let retains_mapping_authority = table.capabilities().any(|capability| {
                capability.object == object
                    && capability.object_type == object_type
                    && capability.rights.contains(Rights::MAP)
            });
            if !retains_mapping_authority {
                if let Some(region) = frame_region(object) {
                    for frame in &region.frames[..region.pages as usize] {
                        clear_task_runtime_frame_mappings(task, *frame);
                    }
                } else {
                    clear_task_runtime_frame_mappings(task, object);
                }
            }
        }
    }
    Ok(deleted)
}

fn reclaim_task_capabilities(task: usize) -> ResourceCleanup {
    let mut frame_objects = [0u32; MAX_CAPABILITIES];
    let mut frame_count = 0usize;
    let mut pool_objects = [0u32; MAX_CAPABILITIES];
    let mut pool_count = 0usize;
    let mut notification_objects = [0u32; MAX_CAPABILITIES];
    let mut notification_count = 0usize;
    let tables = unsafe { &mut *CAPS.0.get() };
    for capability in tables[task].capabilities() {
        let (objects, count) = match capability.object_type {
            ObjectType::Frame | ObjectType::FrameRegion => (&mut frame_objects, &mut frame_count),
            ObjectType::MemoryPool => (&mut pool_objects, &mut pool_count),
            ObjectType::Notification => (&mut notification_objects, &mut notification_count),
            _ => continue,
        };
        if !objects[..*count].contains(&capability.object) {
            objects[*count] = capability.object;
            *count += 1;
        }
    }

    let mut cleanup = ResourceCleanup {
        mappings: clear_task_runtime_mappings(task),
        ..ResourceCleanup::default()
    };
    let anonymous = virtual_memory::reclaim(task);
    cleanup.frames = anonymous;
    cleanup.mappings = cleanup.mappings.saturating_add(anonymous);
    tables[task] = CapabilityTable::new();

    for object in frame_objects[..frame_count].iter().copied() {
        let region = frame_region(object);
        let object_type = if region.is_some() {
            ObjectType::FrameRegion
        } else {
            ObjectType::Frame
        };
        let references = tables
            .iter()
            .map(|table| table.object_references(object, object_type))
            .sum::<usize>();
        if references == 0 {
            let pool = region
                .map(|region| region.pool)
                .or_else(|| runtime_frame_pool_owner(object));
            if let Some(region) = region {
                let pages = region.pages;
                for frame in &region.frames[..region.pages as usize] {
                    cleanup.mappings = cleanup
                        .mappings
                        .saturating_add(clear_runtime_frame_mappings(*frame));
                }
                release_frame_region(object);
                cleanup.frames = cleanup.frames.saturating_add(pages as u32);
            } else {
                cleanup.mappings = cleanup
                    .mappings
                    .saturating_add(clear_runtime_frame_mappings(object));
                release_runtime_frame(object);
                cleanup.frames = cleanup.frames.saturating_add(1);
            }
            if pool.is_some_and(|pool| unregister_unused_memory_pool(tables, pool)) {
                cleanup.pools = cleanup.pools.saturating_add(1);
            }
        }
    }
    for object in pool_objects[..pool_count].iter().copied() {
        if unregister_unused_memory_pool(tables, object) {
            cleanup.pools = cleanup.pools.saturating_add(1);
        }
    }
    for object in notification_objects[..notification_count].iter().copied() {
        if tables
            .iter()
            .all(|table| table.object_references(object, ObjectType::Notification) == 0)
        {
            unregister_notification(object);
        }
    }
    cleanup
}

fn task_live_resources(task: usize) -> (u32, u32, u32) {
    let tables = unsafe { &*CAPS.0.get() };
    let mut frame_objects = [0u32; MAX_CAPABILITIES];
    let mut frame_count = 0usize;
    let mut pool_objects = [0u32; MAX_CAPABILITIES];
    let mut pool_count = 0usize;
    for capability in tables[task].capabilities() {
        let (objects, count) = match capability.object_type {
            ObjectType::Frame | ObjectType::FrameRegion => (&mut frame_objects, &mut frame_count),
            ObjectType::MemoryPool => (&mut pool_objects, &mut pool_count),
            _ => continue,
        };
        if !objects[..*count].contains(&capability.object) {
            objects[*count] = capability.object;
            *count += 1;
        }
    }
    (
        frame_count as u32,
        pool_count as u32,
        task_runtime_mapping_count(task),
    )
}

fn delete_capability(task: usize, handle: CapHandle) -> Result<(), Status> {
    let tables = unsafe { &mut *CAPS.0.get() };
    let capability = tables[task].capability(handle, Rights(0))?;
    let references = tables
        .iter()
        .map(|table| table.object_references(capability.object, capability.object_type))
        .sum::<usize>();
    let last_reference = references == 1;
    if capability.object_type == ObjectType::Frame
        && last_reference
        && (runtime_frame_is_mapped(capability.object)
            || runtime_frame_is_dma_mapped(capability.object))
    {
        return Err(Status::Busy);
    }
    if capability.object_type == ObjectType::MemoryPool
        && last_reference
        && allocated_pool_frames(capability.object) != 0
    {
        return Err(Status::Busy);
    }
    if capability.object_type == ObjectType::FrameRegion
        && last_reference
        && frame_region_is_mapped(capability.object)
    {
        return Err(Status::Busy);
    }
    let frame_pool = (capability.object_type == ObjectType::Frame && last_reference)
        .then(|| runtime_frame_pool_owner(capability.object))
        .flatten();
    tables[task].delete(handle)?;
    if last_reference {
        match capability.object_type {
            ObjectType::Frame => {
                release_runtime_frame(capability.object);
                if let Some(pool) = frame_pool {
                    // A task teardown may remove the last pool cap while a shared Frame
                    // keeps the pool alive. The last Frame release closes that registry slot.
                    unregister_unused_memory_pool(tables, pool);
                }
            }
            ObjectType::FrameRegion => release_frame_region(capability.object),
            ObjectType::MemoryPool => unregister_memory_pool(capability.object),
            ObjectType::Notification => unregister_notification(capability.object),
            _ => {}
        }
    }
    Ok(())
}

fn object_type(value: u64) -> Result<ObjectType, Status> {
    match value {
        1 => Ok(ObjectType::Task),
        2 => Ok(ObjectType::Thread),
        3 => Ok(ObjectType::Frame),
        4 => Ok(ObjectType::Endpoint),
        5 => Ok(ObjectType::Notification),
        6 => Ok(ObjectType::Irq),
        7 => Ok(ObjectType::MmioRegion),
        8 => Ok(ObjectType::MemoryPool),
        9 => Ok(ObjectType::DmaDomain),
        10 => Ok(ObjectType::SystemControl),
        11 => Ok(ObjectType::FrameRegion),
        12 => Ok(ObjectType::SystemInfo),
        _ => Err(Status::Invalid),
    }
}

fn application_slot(pid: u64) -> Result<(usize, usize), Status> {
    let task = pid.checked_sub(1).ok_or(Status::NotFound)? as usize;
    let index = task
        .checked_sub(APPLICATION_START)
        .filter(|index| *index < APPLICATION_COUNT)
        .ok_or(Status::NotFound)?;
    Ok((task, index))
}

fn create_object(
    task: usize,
    raw_type: u64,
    rights: Rights,
    argument: u64,
    extra: u64,
) -> Result<CapHandle, Status> {
    let object_type = object_type(raw_type)?;
    let tables = unsafe { &mut *CAPS.0.get() };
    if object_type == ObjectType::Frame {
        let authority = u32::try_from(argument)
            .map(CapHandle)
            .map_err(|_| Status::BadCapability)?;
        let pool = tables[task].capability(authority, Rights::MANAGE)?;
        if pool.object_type != ObjectType::MemoryPool || memory_pool_quota(pool.object).is_none() {
            return Err(Status::BadCapability);
        }
        let allowed = Rights(
            Rights::READ.0
                | Rights::WRITE.0
                | Rights::EXECUTE.0
                | Rights::MAP.0
                | Rights::GRANT.0
                | Rights::MANAGE.0,
        );
        if !rights.contains(Rights::MAP) || rights.0 & !allowed.0 != 0 {
            return Err(Status::Invalid);
        }
        let object = allocate_runtime_frame(pool.object)?;
        return match tables[task].insert_root_with_node(
            object,
            ObjectType::Frame,
            rights,
            allocate_derivation(),
        ) {
            Ok(handle) => Ok(handle),
            Err(status) => {
                release_runtime_frame(object);
                Err(status)
            }
        };
    }
    if object_type == ObjectType::FrameRegion {
        let authority = u32::try_from(argument)
            .map(CapHandle)
            .map_err(|_| Status::BadCapability)?;
        let pool = tables[task].capability(authority, Rights::MANAGE)?;
        let pages = usize::try_from(extra).map_err(|_| Status::Invalid)?;
        if pool.object_type != ObjectType::MemoryPool
            || memory_pool_quota(pool.object).is_none()
            || pages == 0
            || pages > FRAME_REGION_MAX_PAGES
        {
            return Err(Status::Invalid);
        }
        let allowed = Rights(
            Rights::READ.0 | Rights::WRITE.0 | Rights::MAP.0 | Rights::GRANT.0 | Rights::MANAGE.0,
        );
        if !rights.contains(Rights::MAP) || rights.0 & !allowed.0 != 0 {
            return Err(Status::Invalid);
        }
        let object = NEXT_OBJECT.fetch_add(1, Ordering::Relaxed);
        allocate_frame_region(object, pool.object, pages)?;
        return match tables[task].insert_root_with_node(
            object,
            ObjectType::FrameRegion,
            rights,
            allocate_derivation(),
        ) {
            Ok(handle) => Ok(handle),
            Err(status) => {
                release_frame_region(object);
                Err(status)
            }
        };
    }
    if object_type == ObjectType::MemoryPool {
        let allowed = Rights(Rights::MANAGE.0 | Rights::GRANT.0);
        let quota = usize::try_from(argument).map_err(|_| Status::Invalid)?;
        if !rights.contains(Rights::MANAGE) || rights.0 & !allowed.0 != 0 {
            return Err(Status::Invalid);
        }
        let object = NEXT_OBJECT.fetch_add(1, Ordering::Relaxed);
        register_memory_pool(object, quota)?;
        return match tables[task].insert_root_with_node(
            object,
            object_type,
            rights,
            allocate_derivation(),
        ) {
            Ok(handle) => Ok(handle),
            Err(status) => {
                unregister_memory_pool(object);
                Err(status)
            }
        };
    }
    if object_type == ObjectType::Notification {
        let allowed = Rights(Rights::READ.0 | Rights::WRITE.0 | Rights::GRANT.0 | Rights::MANAGE.0);
        if argument != 0
            || rights.0 & !allowed.0 != 0
            || !rights.contains(Rights::READ) && !rights.contains(Rights::WRITE)
        {
            return Err(Status::Invalid);
        }
        let object = NEXT_OBJECT.fetch_add(1, Ordering::Relaxed);
        register_notification(object)?;
        return match tables[task].insert_root_with_node(
            object,
            object_type,
            rights,
            allocate_derivation(),
        ) {
            Ok(handle) => Ok(handle),
            Err(status) => {
                unregister_notification(object);
                Err(status)
            }
        };
    }
    let object = NEXT_OBJECT.fetch_add(1, Ordering::Relaxed);
    tables[task].insert_root_with_node(object, object_type, rights, allocate_derivation())
}

fn fixed_memory_pool_bit(object: u32) -> Option<u32> {
    match object {
        ROOT_MEMORY_POOL_OBJECT => Some(1),
        DRIVER_MEMORY_POOL_OBJECT => Some(2),
        GUI_MEMORY_POOL_OBJECT => Some(4),
        SCRIPT_MEMORY_POOL_OBJECT => Some(8),
        NETWORK_MEMORY_POOL_OBJECT => Some(16),
        _ => None,
    }
}

fn reset_memory_pools() -> Result<(), Status> {
    for (object, quota) in MEMORY_POOL_OBJECTS.iter().zip(&MEMORY_POOL_QUOTAS) {
        object.store(0, Ordering::Release);
        quota.store(0, Ordering::Release);
    }
    register_memory_pool(ROOT_MEMORY_POOL_OBJECT, MEMORY_POOL_QUOTA)?;
    register_memory_pool(DRIVER_MEMORY_POOL_OBJECT, MEMORY_POOL_QUOTA)?;
    register_memory_pool(GUI_MEMORY_POOL_OBJECT, GUI_MEMORY_POOL_QUOTA)?;
    register_memory_pool(SCRIPT_MEMORY_POOL_OBJECT, SCRIPT_MEMORY_POOL_QUOTA)?;
    register_memory_pool(NETWORK_MEMORY_POOL_OBJECT, NETWORK_MEMORY_POOL_QUOTA)
}

fn register_memory_pool(object: u32, quota: usize) -> Result<(), Status> {
    if object == 0 || quota == 0 || quota > MAX_MEMORY_POOL_QUOTA {
        return Err(Status::Invalid);
    }
    if memory_pool_quota(object).is_some() {
        return Err(Status::Busy);
    }
    let reserved = MEMORY_POOL_QUOTAS
        .iter()
        .map(|quota| quota.load(Ordering::Acquire) as usize)
        .sum::<usize>();
    if reserved
        .checked_add(quota)
        .is_none_or(|sum| sum > RUNTIME_FRAME_COUNT)
    {
        return Err(Status::NoMemory);
    }
    let Some(slot) = MEMORY_POOL_OBJECTS
        .iter()
        .position(|registered| registered.load(Ordering::Acquire) == 0)
    else {
        return Err(Status::NoMemory);
    };
    MEMORY_POOL_QUOTAS[slot].store(quota as u32, Ordering::Release);
    MEMORY_POOL_OBJECTS[slot].store(object, Ordering::Release);
    Ok(())
}

fn unregister_memory_pool(object: u32) {
    if fixed_memory_pool_bit(object).is_some() {
        return;
    }
    if let Some(slot) = MEMORY_POOL_OBJECTS
        .iter()
        .position(|registered| registered.load(Ordering::Acquire) == object)
    {
        MEMORY_POOL_OBJECTS[slot].store(0, Ordering::Release);
        MEMORY_POOL_QUOTAS[slot].store(0, Ordering::Release);
    }
}

fn memory_pool_quota(object: u32) -> Option<usize> {
    MEMORY_POOL_OBJECTS
        .iter()
        .position(|registered| registered.load(Ordering::Acquire) == object)
        .map(|slot| MEMORY_POOL_QUOTAS[slot].load(Ordering::Acquire) as usize)
        .filter(|quota| *quota != 0)
}

fn allocated_pool_frames(object: u32) -> usize {
    RUNTIME_FRAME_POOL
        .iter()
        .filter(|owner| owner.load(Ordering::Acquire) == object)
        .count()
}

fn unregister_unused_memory_pool(tables: &[CapabilityTable; TASK_COUNT], object: u32) -> bool {
    if fixed_memory_pool_bit(object).is_some()
        || memory_pool_quota(object).is_none()
        || allocated_pool_frames(object) != 0
        || tables
            .iter()
            .any(|table| table.object_references(object, ObjectType::MemoryPool) != 0)
    {
        return false;
    }
    unregister_memory_pool(object);
    true
}

fn reset_notifications() {
    for (object, bits) in NOTIFICATION_OBJECTS.iter().zip(&NOTIFICATION_BITS) {
        object.store(0, Ordering::Release);
        bits.store(0, Ordering::Release);
    }
}

fn register_notification(object: u32) -> Result<(), Status> {
    if object == 0 || object == DEVICE_NOTIFICATION_OBJECT {
        return Err(Status::Invalid);
    }
    if notification_slot(object).is_some() {
        return Err(Status::Busy);
    }
    let Some(slot) = NOTIFICATION_OBJECTS
        .iter()
        .position(|registered| registered.load(Ordering::Acquire) == 0)
    else {
        return Err(Status::NoMemory);
    };
    NOTIFICATION_BITS[slot].store(0, Ordering::Release);
    NOTIFICATION_OBJECTS[slot].store(object, Ordering::Release);
    Ok(())
}

fn unregister_notification(object: u32) {
    if let Some(slot) = notification_slot(object) {
        if object != TERMINAL_CANCEL_OBJECT {
            NOTIFICATION_OBJECTS[slot].store(0, Ordering::Release);
        }
        NOTIFICATION_BITS[slot].store(0, Ordering::Release);
    }
}

fn notification_slot(object: u32) -> Option<usize> {
    NOTIFICATION_OBJECTS
        .iter()
        .position(|registered| registered.load(Ordering::Acquire) == object)
}

fn signal_notification(object: u32, bits: u64) -> Option<bool> {
    let slot = notification_slot(object)?;
    NOTIFICATION_BITS[slot].fetch_or(bits, Ordering::AcqRel);
    let woke = wake_notification_object(object);
    if woke {
        arch::send_reschedule(0);
        arch::send_reschedule(1);
        arch::send_event();
    }
    Some(woke)
}

fn take_notification_bits(object: u32) -> Option<u64> {
    if object == DEVICE_NOTIFICATION_OBJECT {
        return Some(DEVICE_NOTIFICATION.swap(0, Ordering::AcqRel));
    }
    notification_slot(object).map(|slot| NOTIFICATION_BITS[slot].swap(0, Ordering::AcqRel))
}

fn allocate_runtime_frame(pool_object: u32) -> Result<u32, Status> {
    if (current_task(unsafe { &*STATE.0.get() }) >= APPLICATION_START
        || pool_object == SCRIPT_MEMORY_POOL_OBJECT)
        && physical_memory::free_frames() <= 64
    {
        return Err(Status::NoMemory);
    }
    let quota = memory_pool_quota(pool_object).ok_or(Status::BadCapability)?;
    let allocated = allocated_pool_frames(pool_object);
    if allocated >= quota {
        return Err(Status::NoMemory);
    }
    for index in 0..RUNTIME_FRAME_COUNT {
        if RUNTIME_FRAME_PHYSICAL[index]
            .compare_exchange(0, u64::MAX, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            continue;
        }
        match physical_memory::allocate_zeroed() {
            Ok(physical) => {
                RUNTIME_FRAME_POOL[index].store(pool_object, Ordering::Release);
                RUNTIME_FRAME_PHYSICAL[index].store(physical, Ordering::Release);
                if !DTB_FRAME_REPORTED.swap(true, Ordering::AcqRel) {
                    crate::kprintln!("[mm] MemoryPool DTB frame allocator active quota=4");
                }
                if let Some(pool_bit) = fixed_memory_pool_bit(pool_object) {
                    let seen = MEMORY_POOLS_SEEN.fetch_or(pool_bit, Ordering::AcqRel) | pool_bit;
                    if seen == 3 && !MEMORY_POOLS_REPORTED.swap(true, Ordering::AcqRel) {
                        crate::kprintln!(
                            "[mm] MemoryPool independent pools=2 quotas=[4,4] root=true driver=true"
                        );
                    }
                }
                return Ok(RUNTIME_FRAME_OBJECT_BASE + index as u32);
            }
            Err(status) => {
                RUNTIME_FRAME_POOL[index].store(0, Ordering::Release);
                RUNTIME_FRAME_PHYSICAL[index].store(0, Ordering::Release);
                return Err(status);
            }
        }
    }
    Err(Status::NoMemory)
}

fn release_runtime_frame(object: u32) {
    let Some(index) = object
        .checked_sub(RUNTIME_FRAME_OBJECT_BASE)
        .map(|index| index as usize)
        .filter(|index| *index < RUNTIME_FRAME_COUNT)
    else {
        return;
    };
    let physical = RUNTIME_FRAME_PHYSICAL[index].swap(0, Ordering::AcqRel);
    RUNTIME_FRAME_POOL[index].store(0, Ordering::Release);
    if physical != 0 && physical != u64::MAX {
        let _ = physical_memory::release(physical);
    }
}

fn allocate_frame_region(object: u32, pool: u32, pages: usize) -> Result<(), Status> {
    let regions = unsafe { &mut *FRAME_REGIONS.0.get() };
    let Some(slot) = regions.iter_mut().find(|region| region.object == 0) else {
        return Err(Status::NoMemory);
    };
    let mut allocated = 0usize;
    while allocated < pages {
        match allocate_runtime_frame(pool) {
            Ok(frame) => {
                slot.frames[allocated] = frame;
                allocated += 1;
            }
            Err(status) => {
                for frame in &mut slot.frames[..allocated] {
                    release_runtime_frame(*frame);
                    *frame = 0;
                }
                return Err(status);
            }
        }
    }
    slot.object = object;
    slot.pool = pool;
    slot.pages = pages as u16;
    Ok(())
}

fn frame_region(object: u32) -> Option<&'static FrameRegion> {
    unsafe { &*FRAME_REGIONS.0.get() }
        .iter()
        .find(|region| region.object == object)
}

fn release_frame_region(object: u32) {
    let regions = unsafe { &mut *FRAME_REGIONS.0.get() };
    let Some(region) = regions.iter_mut().find(|region| region.object == object) else {
        return;
    };
    for frame in &region.frames[..region.pages as usize] {
        release_runtime_frame(*frame);
    }
    *region = FrameRegion::EMPTY;
}

fn frame_region_is_mapped(object: u32) -> bool {
    let Some(region) = frame_region(object) else {
        return false;
    };
    region.frames[..region.pages as usize]
        .iter()
        .any(|frame| runtime_frame_is_mapped(*frame) || runtime_frame_is_dma_mapped(*frame))
}

fn runtime_frame_pool_owner(object: u32) -> Option<u32> {
    let index = object.checked_sub(RUNTIME_FRAME_OBJECT_BASE)? as usize;
    if index >= RUNTIME_FRAME_COUNT {
        return None;
    }
    match RUNTIME_FRAME_POOL[index].load(Ordering::Acquire) {
        0 => None,
        pool => Some(pool),
    }
}

fn runtime_frame_physical(object: u32) -> Option<u64> {
    let shared = match object {
        FILESYSTEM_FRAME_OBJECT => Some(&FILESYSTEM_FRAME_PHYSICAL),
        SSH_FILESYSTEM_FRAME_OBJECT => Some(&SSH_FILESYSTEM_FRAME_PHYSICAL),
        WINDOWD_FILESYSTEM_FRAME_OBJECT => Some(&WINDOWD_FILESYSTEM_FRAME_PHYSICAL),
        TERMINAL_FILESYSTEM_FRAME_OBJECT => Some(&TERMINAL_FILESYSTEM_FRAME_PHYSICAL),
        DATABASE_FILESYSTEM_FRAME_OBJECT => Some(&DATABASE_FILESYSTEM_FRAME_PHYSICAL),
        ROOT_FILESYSTEM_FRAME_OBJECT => Some(&ROOT_FILESYSTEM_FRAME_PHYSICAL),
        NETWORK_FILESYSTEM_FRAME_OBJECT => Some(&NETWORK_FILESYSTEM_FRAME_PHYSICAL),
        IDENTITY_SNAPSHOT_OBJECT => Some(&IDENTITY_SNAPSHOT_PHYSICAL),
        _ => None,
    };
    if let Some(shared) = shared {
        return Some(shared.load(Ordering::Acquire)).filter(|physical| *physical != 0);
    }
    let index = object.checked_sub(RUNTIME_FRAME_OBJECT_BASE)? as usize;
    if index >= RUNTIME_FRAME_COUNT {
        return None;
    }
    match RUNTIME_FRAME_PHYSICAL[index].load(Ordering::Acquire) {
        0 | u64::MAX => None,
        physical => Some(physical),
    }
}

fn reset_runtime_frames() {
    for (physical, pool) in RUNTIME_FRAME_PHYSICAL.iter().zip(&RUNTIME_FRAME_POOL) {
        let address = physical.swap(0, Ordering::AcqRel);
        pool.store(0, Ordering::Release);
        if address != 0 && address != u64::MAX {
            let _ = physical_memory::release(address);
        }
    }
}

fn reset_filesystem_frame() {
    let address = NETWORK_FILESYSTEM_FRAME_PHYSICAL.swap(0, Ordering::AcqRel);
    if address != 0 { let _ = physical_memory::release(address); }
    let address = IDENTITY_SNAPSHOT_PHYSICAL.swap(0, Ordering::AcqRel);
    if address != 0 { let _ = physical_memory::release(address); }
    let address = ROOT_FILESYSTEM_FRAME_PHYSICAL.swap(0, Ordering::AcqRel);
    if address != 0 {
        let _ = physical_memory::release(address);
    }
    let address = FILESYSTEM_FRAME_PHYSICAL.swap(0, Ordering::AcqRel);
    if address != 0 {
        let _ = physical_memory::release(address);
    }
    let address = SSH_FILESYSTEM_FRAME_PHYSICAL.swap(0, Ordering::AcqRel);
    if address != 0 {
        let _ = physical_memory::release(address);
    }
    let address = WINDOWD_FILESYSTEM_FRAME_PHYSICAL.swap(0, Ordering::AcqRel);
    if address != 0 {
        let _ = physical_memory::release(address);
    }
    let address = TERMINAL_FILESYSTEM_FRAME_PHYSICAL.swap(0, Ordering::AcqRel);
    if address != 0 {
        let _ = physical_memory::release(address);
    }
    let address = DATABASE_FILESYSTEM_FRAME_PHYSICAL.swap(0, Ordering::AcqRel);
    if address != 0 {
        let _ = physical_memory::release(address);
    }
}

fn runtime_frame_is_mapped(object: u32) -> bool {
    let Some(frame_physical) = runtime_frame_physical(object) else {
        return false;
    };
    unsafe { &*MEMORY.0.get() }.iter().any(|pointer| {
        unsafe { pointer.as_ref() }.is_some_and(|memory| {
            memory
                .level3
                .iter()
                .chain(memory.level3_high.iter())
                .chain(memory.large_heap_level3.iter().flatten())
                .any(|descriptor| {
                    arch::page_table_present(*descriptor)
                        && arch::page_table_physical(*descriptor) == frame_physical
                })
        })
    })
}

fn task_runtime_frame_is_mapped(task: usize, object: u32) -> bool {
    let Some(frame_physical) = runtime_frame_physical(object) else {
        return false;
    };
    let Some(memory) = task_memory(task) else {
        return false;
    };
    memory
        .level3
        .iter()
        .chain(memory.level3_high.iter())
        .chain(memory.large_heap_level3.iter().flatten())
        .any(|descriptor| {
            arch::page_table_present(*descriptor)
                && arch::page_table_physical(*descriptor) == frame_physical
        })
}

fn task_frame_region_is_mapped(task: usize, object: u32) -> bool {
    let Some(region) = frame_region(object) else {
        return false;
    };
    region.frames[..region.pages as usize]
        .iter()
        .any(|frame| task_runtime_frame_is_mapped(task, *frame))
}

fn runtime_frame_is_dma_mapped(object: u32) -> bool {
    DMA_FRAME_OBJECTS
        .iter()
        .any(|mapped| mapped.load(Ordering::Acquire) == object)
}

fn clear_task_runtime_mappings(task: usize) -> u32 {
    let Some(memory) = task_memory_mut(task) else {
        return 0;
    };
    let first = ((DYNAMIC_MAP_START >> 12) & 0x1ff) as usize;
    let last = (((DYNAMIC_MAP_END - 1) >> 12) & 0x1ff) as usize;
    let mut cleared = 0u32;
    for descriptor in &mut memory.level3[first..=last] {
        if arch::page_table_present(*descriptor) {
            *descriptor = 0;
            cleared = cleared.saturating_add(1);
        }
    }
    if cleared != 0 {
        arch::invalidate_user_asid(ASID_BASE + task as u16);
    }
    cleared
}

fn task_runtime_mapping_count(task: usize) -> u32 {
    let Some(memory) = task_memory(task) else {
        return 0;
    };
    let first = ((DYNAMIC_MAP_START >> 12) & 0x1ff) as usize;
    let last = (((DYNAMIC_MAP_END - 1) >> 12) & 0x1ff) as usize;
    memory.level3[first..=last]
        .iter()
        .filter(|descriptor| arch::page_table_present(**descriptor))
        .count() as u32
}

fn clear_runtime_frame_mappings(object: u32) -> u32 {
    (0..TASK_COUNT)
        .map(|task| clear_task_runtime_frame_mappings(task, object))
        .sum()
}

fn clear_task_runtime_frame_mappings(task: usize, object: u32) -> u32 {
    let Some(physical) = runtime_frame_physical(object) else {
        return 0;
    };
    let mut cleared = 0u32;
    let Some(memory) = task_memory_mut(task) else {
        return 0;
    };
    for descriptor in memory
        .level3
        .iter_mut()
        .chain(memory.level3_high.iter_mut())
        .chain(memory.large_heap_level3.iter_mut().flatten())
    {
        if arch::page_table_present(*descriptor)
            && arch::page_table_physical(*descriptor) == physical
        {
            *descriptor = 0;
            cleared = cleared.saturating_add(1);
        }
    }
    if cleared != 0 {
        // Revoke commits cap deletion and mapping removal under the scheduler lock.
        arch::invalidate_user_asid(ASID_BASE + task as u16);
    }
    cleared
}

fn next_runnable(state: &State, after: usize, cpu: usize) -> Option<usize> {
    (1..=TASK_COUNT)
        .map(|offset| (after + offset) % TASK_COUNT)
        .find(|task| !is_idle_task(*task) && matches!(state.threads[*task], ThreadState::Runnable))
        .or_else(|| {
            let idle = idle_task(cpu);
            matches!(state.threads[idle], ThreadState::Runnable).then_some(idle)
        })
}

fn next_other_runnable(state: &State, after: usize, cpu: usize) -> Option<usize> {
    (1..TASK_COUNT)
        .map(|offset| (after + offset) % TASK_COUNT)
        .find(|task| !is_idle_task(*task) && matches!(state.threads[*task], ThreadState::Runnable))
        .or_else(|| {
            let idle = idle_task(cpu);
            (after != idle && matches!(state.threads[idle], ThreadState::Runnable)).then_some(idle)
        })
}

fn record_switch(cpu: usize) {
    SWITCHES.fetch_add(1, Ordering::Relaxed);
    CPU_SWITCHES[cpu].fetch_add(1, Ordering::Relaxed);
    let counts = [
        CPU_SWITCHES[0].load(Ordering::Relaxed),
        CPU_SWITCHES[1].load(Ordering::Relaxed),
    ];
    let non_idle = [
        CPU_NON_IDLE_RUNS[0].load(Ordering::Relaxed),
        CPU_NON_IDLE_RUNS[1].load(Ordering::Relaxed),
    ];
    if counts[0] >= 20
        && counts[1] >= 20
        && non_idle[0] != 0
        && non_idle[1] != 0
        && SMP_REPORTED
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    {
        crate::kprintln!(
            "[sched] resident shared-ready-queue cpus=2 ticket-lock=true idle-tasks=2 switches=[{},{}] non-idle=[{},{}]",
            counts[0],
            counts[1],
            non_idle[0],
            non_idle[1]
        );
    }
}

fn expire_deadlines() {
    let now = arch::clock_nanos();
    let state = unsafe { &mut *STATE.0.get() };
    for task in 0..TASK_COUNT {
        let deadline = match state.threads[task] {
            ThreadState::Runnable
            | ThreadState::Running { .. }
            | ThreadState::Exited
            | ThreadState::Terminating { .. } => 0,
            ThreadState::Sending { deadline, .. }
            | ThreadState::Receiving { deadline, .. }
            | ThreadState::WaitingReply { deadline, .. }
            | ThreadState::WaitingNotification { deadline, .. } => deadline,
        };
        if deadline == 0 || deadline > now {
            continue;
        }
        match state.threads[task] {
            ThreadState::Sending { endpoint, .. } => {
                if let Some(index) = endpoint.checked_sub(1) {
                    if state.endpoints[index].pending == Some(task) {
                        state.endpoints[index].pending = None;
                    }
                }
            }
            ThreadState::WaitingReply { server, .. } => {
                if state.reply_to[server] == Some(task) {
                    state.reply_to[server] = None;
                }
            }
            ThreadState::WaitingNotification { .. } => {}
            _ => {}
        }
        state.threads[task] = ThreadState::Runnable;
        state.contexts[task].registers[0] = Status::TimedOut as i64 as u64;
    }
}

fn wake_notifications() {
    let state = unsafe { &mut *STATE.0.get() };
    for task in 0..state.threads.len() {
        let ThreadState::WaitingNotification { object, .. } = state.threads[task] else {
            continue;
        };
        let Some(bits) = take_notification_bits(object) else {
            continue;
        };
        if bits != 0 {
            state.threads[task] = ThreadState::Runnable;
            state.contexts[task].registers[0] = bits;
        }
    }
}

fn wake_notification_object(object: u32) -> bool {
    let state = unsafe { &mut *STATE.0.get() };
    let Some(task) = state.threads.iter().position(
        |thread| matches!(thread, ThreadState::WaitingNotification { object: waiting, .. } if *waiting == object),
    ) else {
        return false;
    };
    let Some(bits) = take_notification_bits(object) else {
        return false;
    };
    if bits == 0 {
        return false;
    }
    state.threads[task] = ThreadState::Runnable;
    state.contexts[task].registers[0] = bits;
    true
}

fn deadline_elapsed(deadline: u64) -> bool {
    deadline != 0 && deadline <= arch::clock_nanos()
}

fn fail(frame: &mut ExceptionFrame, status: Status) -> u64 {
    frame.registers[0] = status as i64 as u64;
    0
}

fn read_message(task: usize, pointer: u64) -> Option<Message> {
    let mut message = Message::new(0, 0);
    copy_from_user(
        task,
        pointer,
        (&mut message as *mut Message).cast::<u8>(),
        core::mem::size_of::<Message>(),
    )?;
    Some(message)
}

fn write_message(task: usize, pointer: u64, message: &Message) -> bool {
    copy_to_user(
        task,
        pointer,
        (message as *const Message).cast::<u8>(),
        core::mem::size_of::<Message>(),
    )
    .is_some()
}

fn validate_user_range(task: usize, pointer: u64, bytes: u64, write: bool) -> bool {
    if task >= TASK_COUNT || bytes > 4096 {
        return false;
    }
    let Some(end) = pointer.checked_add(bytes) else {
        return false;
    };
    let low_range = pointer >= USER_MIN && end <= STACK_TOP;
    let large_heap_range =
        uses_large_window(task) && pointer >= LARGE_HEAP_START && end <= LARGE_HEAP_END;
    let anonymous_range = pointer >= microsystem_abi::virtual_memory::START
        && end <= microsystem_abi::virtual_memory::END;
    if !low_range && !large_heap_range && !anonymous_range {
        return false;
    }
    if bytes == 0 {
        return true;
    }
    let mut page = pointer & !0xfff;
    let last = (end - 1) & !0xfff;
    loop {
        if (anonymous_range || virtual_memory::is_heap(task, page))
            && virtual_memory::commit(task, page, write).is_err()
        {
            return false;
        }
        let Some(descriptor) = task_memory(task).and_then(|memory| user_descriptor(memory, page))
        else {
            return false;
        };
        if !crate::arch::user_page_accessible(descriptor, write) {
            return false;
        }
        if page == last {
            break;
        }
        page += 4096;
    }
    true
}

fn copy_from_user(task: usize, pointer: u64, destination: *mut u8, bytes: usize) -> Option<()> {
    if bytes == 0 {
        return Some(());
    }
    if !validate_user_range(task, pointer, u64::try_from(bytes).ok()?, false) {
        return None;
    }
    let memory = task_memory(task)?;
    let mut offset = 0;
    while offset < bytes {
        let address = pointer.checked_add(offset as u64)?;
        let descriptor = user_descriptor(memory, address)?;
        let page_offset = address as usize & 0xfff;
        let chunk = (bytes - offset).min(4096 - page_offset);
        let source = crate::arch::phys_to_virt(arch::page_table_physical(descriptor)) + page_offset;
        for index in 0..chunk {
            unsafe {
                destination
                    .add(offset + index)
                    .write(core::ptr::read_volatile((source + index) as *const u8));
            }
        }
        offset += chunk;
    }
    Some(())
}

fn copy_to_user(task: usize, pointer: u64, source: *const u8, bytes: usize) -> Option<()> {
    if bytes == 0 {
        return Some(());
    }
    if !validate_user_range(task, pointer, u64::try_from(bytes).ok()?, true) {
        return None;
    }
    let memory = task_memory(task)?;
    let mut offset = 0;
    while offset < bytes {
        let address = pointer.checked_add(offset as u64)?;
        let descriptor = user_descriptor(memory, address)?;
        let page_offset = address as usize & 0xfff;
        let chunk = (bytes - offset).min(4096 - page_offset);
        let destination =
            crate::arch::phys_to_virt(arch::page_table_physical(descriptor)) + page_offset;
        for index in 0..chunk {
            unsafe {
                core::ptr::write_volatile(
                    (destination + index) as *mut u8,
                    source.add(offset + index).read(),
                );
            }
        }
        offset += chunk;
    }
    Some(())
}

fn user_descriptor(memory: &TaskMemory, virtual_address: u64) -> Option<u64> {
    if (USER_MIN..0x0060_0000).contains(&virtual_address) {
        let index = ((virtual_address >> 12) & 0x1ff) as usize;
        return Some(memory.level3[index]);
    }
    if (0x0060_0000..STACK_TOP).contains(&virtual_address) {
        let index = ((virtual_address >> 12) & 0x1ff) as usize;
        return Some(memory.level3_high[index]);
    }
    if (LARGE_HEAP_START..LARGE_HEAP_END).contains(&virtual_address) {
        let offset = (virtual_address - LARGE_HEAP_START) as usize;
        let table = offset / 0x20_0000;
        let index = (offset >> 12) & 0x1ff;
        return Some(memory.large_heap_level3[table][index]);
    }
    if (microsystem_abi::virtual_memory::START..microsystem_abi::virtual_memory::END)
        .contains(&virtual_address)
    {
        let branch = memory.level2[(virtual_address >> 21) as usize];
        if !arch::page_table_present(branch) {
            return Some(0);
        }
        let entries = arch::phys_to_virt(arch::page_table_physical(branch)) as *const u64;
        return Some(unsafe { *entries.add(((virtual_address >> 12) & 511) as usize) });
    }
    None
}

fn user_descriptor_mut(memory: &mut TaskMemory, virtual_address: u64) -> Option<&mut u64> {
    if (USER_MIN..0x0060_0000).contains(&virtual_address) {
        let index = ((virtual_address >> 12) & 0x1ff) as usize;
        return Some(&mut memory.level3[index]);
    }
    if (0x0060_0000..STACK_TOP).contains(&virtual_address) {
        let index = ((virtual_address >> 12) & 0x1ff) as usize;
        return Some(&mut memory.level3_high[index]);
    }
    if (LARGE_HEAP_START..LARGE_HEAP_END).contains(&virtual_address) {
        let offset = (virtual_address - LARGE_HEAP_START) as usize;
        let table = offset / 0x20_0000;
        let index = (offset >> 12) & 0x1ff;
        return Some(&mut memory.large_heap_level3[table][index]);
    }
    None
}

fn uses_large_window(task: usize) -> bool {
    matches!(task, MFS_TASK | WINDOWD_TASK | DB_TASK | NETD_TASK)
}

fn load_task(task: usize, bytes: &[u8]) -> Result<(), ()> {
    let image = ElfImage::parse(bytes).map_err(|_| ())?;
    let memories = unsafe { &mut *MEMORY.0.get() };
    let slot = memories.get_mut(task).ok_or(())?;
    if slot.is_null() {
        *slot = kernel_heap::allocate_zeroed(core::alloc::Layout::new::<TaskMemory>())
            .map_err(|_| ())?
            .cast::<TaskMemory>();
        if !KERNEL_HEAP_REPORTED.swap(true, Ordering::AcqRel) {
            crate::kprintln!(
                "[mm] TaskMemory/page tables allocated from kernel heap slot-bytes={:#x} allocated={:#x}",
                mem::size_of::<TaskMemory>(),
                kernel_heap::allocated_bytes()
            );
        }
    }
    let memory = unsafe { slot.as_mut() }.ok_or(())?;
    #[cfg(target_arch = "x86_64")]
    memory.level0.fill(0);
    memory.level1.fill(0);
    memory.level2.fill(0);
    memory.level3.fill(0);
    memory.level3_high.fill(0);
    for table in &mut memory.large_heap_level3 {
        table.fill(0);
    }
    memory.image.fill(0);
    memory.stack.fill(0);

    #[cfg(target_arch = "x86_64")]
    let level0_physical = physical(memory.level0.as_ptr() as usize);
    let level1_physical = physical(memory.level1.as_ptr() as usize);
    let level2_physical = physical(memory.level2.as_ptr() as usize);
    let level3_physical = physical(memory.level3.as_ptr() as usize);
    let level3_high_physical = physical(memory.level3_high.as_ptr() as usize);
    let image_physical = physical(memory.image.as_ptr() as usize);
    let stack_physical = physical(memory.stack.as_ptr() as usize);
    #[cfg(target_arch = "x86_64")]
    {
        memory.level0[0] = crate::arch::page_table_branch(level1_physical);
        memory.level0[256] = crate::arch::kernel_high_half_entry();
        memory.level1[0] = crate::arch::page_table_branch(level2_physical);
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        memory.level1[0] = crate::arch::page_table_branch(level2_physical);
        memory.level1[256] = crate::arch::kernel_mmio_entry();
        memory.level1[257] = crate::arch::kernel_pci_mmio_entry();
        memory.level1[258] = crate::arch::kernel_high_half_entry();
    }
    memory.level2[2] = crate::arch::page_table_branch(level3_physical);
    memory.level2[3] = crate::arch::page_table_branch(level3_high_physical);
    if uses_large_window(task) {
        for (table, entries) in memory.large_heap_level3.iter().enumerate() {
            let level2 = (LARGE_HEAP_START as usize >> 21) + table;
            memory.level2[level2] =
                crate::arch::page_table_branch(physical(entries.as_ptr() as usize));
        }
    }

    for segment in image.segments() {
        let segment = segment.map_err(|_| ())?;
        let start = segment.virtual_address.checked_sub(USER_MIN).ok_or(())? as usize;
        let end = start.checked_add(segment.memory_size as usize).ok_or(())?;
        if end > IMAGE_BYTES {
            return Err(());
        }
        let file_start = segment.file_offset as usize;
        let file_end = file_start
            .checked_add(segment.file_size as usize)
            .ok_or(())?;
        let destination_end = start.checked_add(segment.file_size as usize).ok_or(())?;
        memory.image[start..destination_end].copy_from_slice(&bytes[file_start..file_end]);
        if segment.memory_size == 0 {
            continue;
        }
        let first_page = start & !0xfff;
        let last_page = (end - 1) & !0xfff;
        for offset in (first_page..=last_page).step_by(4096) {
            let index = (((USER_MIN as usize + offset) >> 12) & 0x1ff) as usize;
            memory.level3[index] = crate::arch::page_table_entry(
                image_physical + offset as u64,
                crate::arch::user_page_flags(segment.writable, segment.executable),
            );
        }
    }
    for offset in (0..STACK_BYTES).step_by(4096) {
        map_page(
            memory,
            STACK_TOP - STACK_BYTES as u64 + offset as u64,
            stack_physical + offset as u64,
            normal_page_flags(),
        );
    }
    if task == CONSOLE_TASK {
        let uart = CONSOLE_UART.load(Ordering::Acquire);
        if uart == 0 {
            return Err(());
        }
        map_page(memory, CONSOLE_UART_VA, uart & !0xfff, device_page_flags());
    } else if task == DEVMGR_TASK {
        let ecam = device_control::ecam_physical().ok_or(())?;
        for slot in 0..DEVMGR_ECAM_SLOTS {
            map_page(
                memory,
                DEVMGR_ECAM_SCAN_VA + slot as u64 * 4096,
                ecam + (slot as u64) * 0x8000,
                device_page_flags(),
            );
        }
    }
    if matches!(task, MFS_TASK | SHELL_TASK | DB_TASK) {
        let filesystem_physical = FILESYSTEM_FRAME_PHYSICAL.load(Ordering::Acquire);
        if filesystem_physical == 0 {
            return Err(());
        }
        map_page(
            memory,
            FILESYSTEM_DATA_VA,
            filesystem_physical,
            normal_page_flags(),
        );
    }
    if matches!(task, MFS_TASK | SSHD_TASK) {
        let filesystem_physical = SSH_FILESYSTEM_FRAME_PHYSICAL.load(Ordering::Acquire);
        if filesystem_physical == 0 {
            return Err(());
        }
        map_page(
            memory,
            if task == MFS_TASK {
                SSH_FILESYSTEM_DATA_VA
            } else {
                FILESYSTEM_DATA_VA
            },
            filesystem_physical,
            normal_page_flags(),
        );
    }
    if matches!(task, MFS_TASK | WINDOWD_TASK) {
        let filesystem_physical = WINDOWD_FILESYSTEM_FRAME_PHYSICAL.load(Ordering::Acquire);
        if filesystem_physical == 0 {
            return Err(());
        }
        map_page(
            memory,
            if task == MFS_TASK {
                WINDOWD_FILESYSTEM_DATA_VA
            } else {
                FILESYSTEM_DATA_VA
            },
            filesystem_physical,
            normal_page_flags(),
        );
    }
    if matches!(task, MFS_TASK | WINDOWD_TASK | TERMINAL_TASK) {
        let filesystem_physical = TERMINAL_FILESYSTEM_FRAME_PHYSICAL.load(Ordering::Acquire);
        if filesystem_physical == 0 {
            return Err(());
        }
        map_page(
            memory,
            TERMINAL_FILESYSTEM_DATA_VA,
            filesystem_physical,
            normal_page_flags(),
        );
    }
    if matches!(task, MFS_TASK | DB_TASK) {
        let filesystem_physical = DATABASE_FILESYSTEM_FRAME_PHYSICAL.load(Ordering::Acquire);
        if filesystem_physical == 0 {
            return Err(());
        }
        map_page(
            memory,
            DATABASE_FILESYSTEM_DATA_VA,
            filesystem_physical,
            normal_page_flags(),
        );
    }

    if matches!(task, 0 | MFS_TASK) {
        let physical = ROOT_FILESYSTEM_FRAME_PHYSICAL.load(Ordering::Acquire);
        if physical != 0 {
            map_page(
                memory,
                ROOT_FILESYSTEM_DATA_VA,
                physical,
                normal_page_flags(),
            );
        } else if task != 0 {
            return Err(());
        }
    }
    if matches!(task, MFS_TASK | NETD_TASK) {
        let physical = NETWORK_FILESYSTEM_FRAME_PHYSICAL.load(Ordering::Acquire);
        if physical == 0 { return Err(()); }
        map_page(memory, if task == MFS_TASK { NETWORK_FILESYSTEM_DATA_VA } else { FILESYSTEM_DATA_VA }, physical, normal_page_flags());
    }
    if matches!(task, MFS_TASK | SHELL_TASK | WINDOWD_TASK | TERMINAL_TASK | FILES_TASK | MONITOR_TASK | SSHD_TASK | NETD_TASK | DB_TASK) {
        let physical = IDENTITY_SNAPSHOT_PHYSICAL.load(Ordering::Acquire);
        if physical == 0 { return Err(()); }
        map_page(memory, microsystem_abi::identity::SNAPSHOT_VA as u64, physical, user_page_flags(false, false));
    }

    if let Some(grant) = unsafe { *BLOCK_GRANT.0.get() } {
        map_transport_memory(memory, task, grant);
    }
    let starts = unsafe { &mut *STARTS.0.get() };
    #[cfg(target_arch = "x86_64")]
    let stack = {
        // Resident services also enter as SysV callees even when they have no
        // stack arguments.
        memory.stack[STACK_BYTES - mem::size_of::<u64>()..].fill(0);
        STACK_TOP - mem::size_of::<u64>() as u64
    };
    #[cfg(not(target_arch = "x86_64"))]
    let stack = STACK_TOP;
    starts[task] = Start {
        entry: image.entry(),
        stack,
        ttbr0: crate::arch::address_space_root(
            {
                #[cfg(target_arch = "x86_64")]
                {
                    level0_physical
                }
                #[cfg(not(target_arch = "x86_64"))]
                {
                    level1_physical
                }
            },
            ASID_BASE + task as u16,
        ),
    };
    Ok(())
}

fn map_transport_memory(memory: &mut TaskMemory, task: usize, grant: UserTransportGrant) {
    if matches!(task, BLOCK_TASK | DEVMGR_TASK) {
        for (address, physical) in [
            (BLOCK_COMMON_VA, grant.common_physical),
            (BLOCK_NOTIFY_VA, grant.notify_physical),
            (BLOCK_ISR_VA, grant.isr_physical),
            (BLOCK_DEVICE_VA, grant.device_physical),
        ] {
            map_page(memory, address, physical, device_page_flags());
        }
    }
    if task == DEVMGR_TASK {
        map_page(
            memory,
            DEVMGR_PCI_CONFIG_VA,
            grant.pci_function_physical,
            device_page_flags(),
        );
    }
    if task == BLOCK_TASK {
        map_page(
            memory,
            BLOCK_QUEUE_VA,
            grant.queue_physical,
            normal_page_flags(),
        );
    }
    if matches!(task, BLOCK_TASK | MFS_TASK) {
        map_page(
            memory,
            SHARED_DATA_VA,
            grant.data_physical,
            normal_page_flags(),
        );
    }
}

fn map_page(memory: &mut TaskMemory, virtual_address: u64, physical: u64, flags: u64) {
    let index = ((virtual_address >> 12) & 0x1ff) as usize;
    if virtual_address < 0x0060_0000 {
        memory.level3[index] = crate::arch::page_table_entry(physical, flags);
    } else {
        memory.level3_high[index] = crate::arch::page_table_entry(physical, flags);
    }
}

fn normal_page_flags() -> u64 {
    crate::arch::normal_page_flags()
}

fn user_page_flags(writable: bool, executable: bool) -> u64 {
    crate::arch::user_page_flags(writable, executable)
}

fn device_page_flags() -> u64 {
    crate::arch::device_page_flags()
}

fn physical(high: usize) -> u64 {
    crate::arch::virt_to_phys(high as u64).unwrap_or(0)
}
