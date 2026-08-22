#![no_std]

pub const ABI_VERSION: u16 = 1;
pub const MESSAGE_WORDS: usize = 6;
pub const MESSAGE_CAPS: usize = 4;
pub const MESSAGE_CAP_MOVE_SHIFT: usize = 12;
pub const MESSAGE_CAP_MOVE_MASK: u16 = ((1 << MESSAGE_CAPS) - 1) << MESSAGE_CAP_MOVE_SHIFT;
pub const DEADLINE_INFINITE: u64 = 0;

pub const fn message_cap_move(index: usize) -> u16 {
    if index < MESSAGE_CAPS {
        1 << (MESSAGE_CAP_MOVE_SHIFT + index)
    } else {
        0
    }
}

#[repr(transparent)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct CapHandle(pub u32);

impl CapHandle {
    pub const INVALID: Self = Self(0);

    pub const fn from_parts(slot: u16, generation: u16) -> Self {
        Self(((generation as u32) << 16) | slot as u32)
    }

    pub const fn slot(self) -> usize {
        (self.0 & 0xffff) as usize
    }

    pub const fn generation(self) -> u16 {
        (self.0 >> 16) as u16
    }
}

#[repr(transparent)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Rights(pub u32);

impl Rights {
    pub const NONE: Self = Self(0);
    pub const READ: Self = Self(1 << 0);
    pub const WRITE: Self = Self(1 << 1);
    pub const EXECUTE: Self = Self(1 << 2);
    pub const GRANT: Self = Self(1 << 3);
    pub const MANAGE: Self = Self(1 << 4);
    pub const MAP: Self = Self(1 << 5);
    pub const ACK: Self = Self(1 << 6);
    pub const ALL: Self = Self(u32::MAX);

    pub const fn contains(self, required: Self) -> bool {
        self.0 & required.0 == required.0
    }

    pub const fn intersect(self, other: Self) -> Self {
        Self(self.0 & other.0)
    }
}

#[repr(u16)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ObjectType {
    Task = 1,
    Thread = 2,
    Frame = 3,
    Endpoint = 4,
    Notification = 5,
    Irq = 6,
    MmioRegion = 7,
    MemoryPool = 8,
    DmaDomain = 9,
    SystemControl = 10,
    FrameRegion = 11,
    SystemInfo = 12,
    NetworkDevice = 13,
    RandomSource = 14,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Message {
    pub protocol: u16,
    pub version: u16,
    pub opcode: u16,
    pub flags: u16,
    pub words: [u64; MESSAGE_WORDS],
    pub caps: [CapHandle; MESSAGE_CAPS],
}

impl Message {
    pub const fn new(protocol: u16, opcode: u16) -> Self {
        Self {
            protocol,
            version: ABI_VERSION,
            opcode,
            flags: 0,
            words: [0; MESSAGE_WORDS],
            caps: [CapHandle::INVALID; MESSAGE_CAPS],
        }
    }
}

#[repr(u16)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Syscall {
    Yield = 0,
    Exit = 1,
    IpcCall = 2,
    IpcRecv = 3,
    IpcReplyRecv = 4,
    NotificationWait = 5,
    CapCopy = 6,
    CapMove = 7,
    CapDelete = 8,
    CapRevoke = 9,
    ObjectCreate = 10,
    FrameMap = 11,
    FrameUnmap = 12,
    ThreadStart = 13,
    IrqBind = 14,
    IrqAck = 15,
    DmaMap = 16,
    DmaUnmap = 17,
    ClockNow = 18,
    DebugWrite = 19,
    ThreadStatus = 20,
    ThreadKill = 21,
    SystemControl = 22,
    NotificationSignal = 23,
    SystemStats = 24,
    GuiPresent = 25,
    GuiInput = 26,
    NetReceive = 27,
    NetSend = 28,
    RandomFill = 29,
    ThreadStartEx = 30,
    ClockRealtime = 31,
}

#[repr(u64)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SystemControlOperation {
    ActivatePci = 1,
    Poweroff = 2,
    Reboot = 3,
}

#[repr(i32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Status {
    Ok = 0,
    Invalid = -1,
    BadCapability = -2,
    AccessDenied = -3,
    NotFound = -4,
    NoMemory = -5,
    Busy = -6,
    TimedOut = -7,
    Fault = -8,
    NotSupported = -9,
    Io = -10,
    NoSpace = -11,
    Corrupt = -12,
}

pub mod protocol {
    pub const CONSOLE: u16 = 1;
    pub const BLOCK: u16 = 2;
    pub const FILESYSTEM: u16 = 3;
    pub const PROCESS: u16 = 4;
    pub const TIME: u16 = 5;
    pub const GUI: u16 = 6;
    pub const SSH: u16 = 7;
    pub const SCRIPT: u16 = 8;
    pub const NETWORK: u16 = 9;
    pub const DATABASE: u16 = 10;
}

pub mod process {
    pub const FIRST_APPLICATION_PID: u64 = 14;
    pub const MAX_APPLICATIONS: usize = 8;

    #[repr(u16)]
    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub enum Operation {
        Spawn = 1,
        Wait = 2,
        List = 3,
        Kill = 4,
        MemoryPool = 5,
        SpawnScript = 6,
    }
}

pub mod filesystem {
    pub const SCRIPT_REGISTER: u16 = 0x100;
    pub const SCRIPT_UNREGISTER: u16 = 0x101;
    pub const SCRIPT_REQUEST_MAGIC: u64 = u64::from_le_bytes(*b"MICAFS01");
    pub const SCRIPT_TRUSTED_READ_MAGIC: u64 = u64::from_le_bytes(*b"MICACA01");

    #[repr(u16)]
    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub enum Operation {
        Stat = 1,
        List = 2,
        Read = 3,
        Write = 4,
        Mkdir = 5,
        Sync = 6,
        Open = 7,
        Fsync = 8,
        Rename = 9,
        Unlink = 10,
        Close = 11,
        WriteAtomic = 12,
        ReadRange = 13,
        Stats = 14,
        WriteRange = 15,
        Replace = 16,
    }
}

pub mod database {
    pub const RESPONSE_MAGIC: u32 = u32::from_le_bytes(*b"SQL1");
    pub const RESPONSE_VERSION: u16 = 1;
    pub const SHARED_FRAME_BYTES: usize = 4096;

    #[repr(u16)]
    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub enum Operation {
        Ping = 1,
        Execute = 2,
    }

    #[repr(u16)]
    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub enum ResponseKind {
        Command = 1,
        Rows = 2,
        Error = 3,
    }

    #[repr(u8)]
    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub enum ValueTag {
        Null = 0,
        Integer = 1,
        Text = 2,
        Bool = 3,
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct DbResponseHeaderV1 {
    pub magic: u32,
    pub version: u16,
    pub kind: u16,
    pub columns: u16,
    pub rows: u16,
    pub reserved: u32,
    pub affected_rows: u32,
    pub payload_bytes: u32,
}

pub const THREAD_LAUNCH_CAPS: usize = 6;
pub const THREAD_LAUNCH_V2_CAPS: usize = 9;
pub const THREAD_PROFILE_APPLICATION: u16 = 0;
pub const THREAD_PROFILE_MICA: u16 = 1;
pub const THREAD_LAUNCH_FLAG_GUI: u32 = 1 << 0;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ThreadLaunchV1 {
    pub version: u16,
    pub profile: u16,
    pub flags: u32,
    pub program: u64,
    pub caps: [CapHandle; THREAD_LAUNCH_CAPS],
    pub rights: [Rights; THREAD_LAUNCH_CAPS],
    pub arguments: [u64; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ThreadLaunchV2 {
    pub version: u16,
    pub profile: u16,
    pub flags: u32,
    pub program: u64,
    pub caps: [CapHandle; THREAD_LAUNCH_V2_CAPS],
    pub rights: [Rights; THREAD_LAUNCH_V2_CAPS],
    pub arguments: [u64; 4],
}

pub mod network {
    pub const BROWSE_REQUEST_MAGIC: u64 = u64::from_le_bytes(*b"MICAWEB1");

    pub const VERSION: u16 = 1;
    pub const MAX_CONNECTIONS_PER_SESSION: usize = 8;
    pub const MAX_CONNECTIONS: usize = 64;
    pub const MAX_TRANSFER_BYTES: usize = 32 * 1024;

    #[repr(u16)]
    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub enum Operation {
        OpenSession = 1,
        CloseSession = 2,
        Resolve = 3,
        TcpConnect = 4,
        TcpRead = 5,
        TcpWrite = 6,
        TcpClose = 7,
        UdpOpen = 8,
        UdpSendTo = 9,
        UdpRecvFrom = 10,
        UdpClose = 11,
        TlsConnect = 12,
        Cancel = 13,
        SshListen = 14,
        SshAccept = 15,
        SshSharedFrame = 16,
        SshReceive = 17,
        SshSend = 18,
        SshClose = 19,
        SshStatus = 20,
        Stats = 21,
    }
}

pub mod script {
    pub const VERSION: u16 = 1;
    pub const SESSION_BYTES: usize = 64 * 1024;
    pub const SESSION_PAGES: u32 = 16;
    pub const SESSION_VA: u64 = 0x0058_0000;
    pub const GUI_COMMAND_VA: u64 = 0x0059_0000;
    pub const GUI_EVENT_VA: u64 = 0x005a_0000;
    pub const SOURCE_OFFSET: usize = 4096;
    // Eval sessions carry source inline. File sessions carry only the manifest
    // prefix here; the Mica task reads the exact launcher-selected path through
    // FileService so 64 KiB sources do not consume the I/O rings.
    pub const SOURCE_BYTES: usize = 16 * 1024;
    pub const STDIN_OFFSET: usize = SOURCE_OFFSET + SOURCE_BYTES;
    pub const STDIN_BYTES: usize = 8 * 1024;
    pub const STDOUT_OFFSET: usize = STDIN_OFFSET + STDIN_BYTES;
    pub const STDOUT_BYTES: usize = 32 * 1024;
    // File and network brokers may use the output ring as transient scratch. The
    // runtime preserves and restores committed output around each broker call.
    pub const BROKER_OFFSET: usize = STDOUT_OFFSET;
    pub const BROKER_BYTES: usize = STDOUT_BYTES;
    pub const POLICY_OFFSET: usize = 256;
    pub const POLICY_BYTES: usize = SOURCE_OFFSET - POLICY_OFFSET;
    pub const MODE_EVAL: u16 = 1;
    pub const MODE_FILE: u16 = 2;
    pub const MODE_REPL: u16 = 3;
    pub const FLAG_INTERRUPT: u32 = 1 << 0;
    pub const FLAG_OUTPUT_READY: u32 = 1 << 1;
    pub const FLAG_EXIT_READY: u32 = 1 << 2;
    pub const EVENT_INPUT: u64 = 1 << 0;
    pub const EVENT_OUTPUT: u64 = 1 << 1;
    pub const EVENT_EXIT: u64 = 1 << 2;
    pub const EVENT_GUI: u64 = 1 << 3;
    pub const FLAG_GUI_SESSION: u32 = 1 << 3;

    #[repr(u16)]
    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub enum Operation {
        Collect = 1,
        Interrupt = 2,
        ProcessList = 3,
        ProcessSpawn = 4,
        ProcessWait = 5,
        ProcessKill = 6,
    }

    #[repr(C)]
    #[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
    pub struct SessionHeaderV1 {
        pub magic: u32,
        pub version: u16,
        pub mode: u16,
        pub flags: u32,
        pub source_bytes: u32,
        pub stdin_head: u32,
        pub stdin_tail: u32,
        pub stdout_head: u32,
        pub stdout_tail: u32,
        pub exit_status: i32,
        pub timeout_ns: u64,
        pub instruction_limit: u64,
        pub permission_mask: u64,
        pub policy_bytes: u32,
        pub argv_bytes: u16,
        pub path_bytes: u16,
    }

    pub const SESSION_MAGIC: u32 = u32::from_le_bytes(*b"MICA");
}

pub mod gui {
    pub const VERSION: u16 = 1;
    pub const WIDTH: u32 = 1024;
    pub const HEIGHT: u32 = 768;
    pub const COMMAND_BYTES: usize = 64 * 1024;
    pub const EVENT_BYTES: usize = 4096;
    pub const MAX_COMMANDS: usize = 4096;
    pub const MAX_TEXT_BYTES: usize = 48 * 1024;
    pub const MAX_DAMAGE_RECTS: usize = 16;
    pub const COMMAND_HEADER_BYTES: usize = 512;
    pub const COMMAND_PAYLOAD_BYTES: usize = COMMAND_BYTES - COMMAND_HEADER_BYTES;
    pub const EVENT_RING_HEADER_BYTES: usize = 64;
    pub const EVENT_CAPACITY: usize =
        (EVENT_BYTES - EVENT_RING_HEADER_BYTES) / core::mem::size_of::<Event>();
    pub const PRESENT_MAGIC: u32 = u32::from_le_bytes(*b"GUIC");
    pub const EVENT_MAGIC: u32 = u32::from_le_bytes(*b"GUIE");
    pub const EVENT_RING_FLAG_CLOSE_REQUESTED: u32 = 1 << 0;
    pub const MAX_DYNAMIC_CLIENTS: usize = 8;

    #[repr(u16)]
    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub enum Operation {
        CreateWindow = 1,
        Present = 2,
        SetTitle = 3,
        QueryGeometry = 4,
        WindowAction = 5,
        TerminalCommand = 6,
        RegisterClient = 7,
        UnregisterClient = 8,
        EventConsumed = 9,
        LaunchApplication = 10,
    }

    #[repr(u16)]
    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub enum Application {
        Terminal = 1,
        Files = 2,
        Monitor = 3,
        Reader = 4,
        Editor = 5,
    }

    impl Application {
        pub const fn from_u64(value: u64) -> Option<Self> {
            match value {
                1 => Some(Self::Terminal),
                2 => Some(Self::Files),
                3 => Some(Self::Monitor),
                4 => Some(Self::Reader),
                5 => Some(Self::Editor),
                _ => None,
            }
        }
    }

    #[repr(u16)]
    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub enum WindowAction {
        Minimize = 1,
        Maximize = 2,
        Restore = 3,
        Close = 4,
    }

    #[repr(u16)]
    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub enum CommandKind {
        Clear = 1,
        FillRect = 2,
        StrokeRect = 3,
        Text = 4,
        Icon = 5,
        SetClip = 6,
    }

    #[repr(C)]
    #[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
    pub struct Rect {
        pub x: i32,
        pub y: i32,
        pub width: u32,
        pub height: u32,
    }

    #[repr(C)]
    #[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
    pub struct CommandHeader {
        pub kind: u16,
        pub flags: u16,
        pub bytes: u32,
    }

    #[repr(C)]
    #[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
    pub struct DrawCommand {
        pub header: CommandHeader,
        pub rect: Rect,
        pub color: u32,
        pub argument: u32,
    }

    #[repr(u16)]
    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub enum EventKind {
        PointerMove = 1,
        PointerButton = 2,
        PointerWheel = 3,
        Key = 4,
        TextInput = 5,
        Focus = 6,
        Configure = 7,
        Expose = 8,
        CloseRequested = 9,
    }

    #[repr(C)]
    #[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
    pub struct InputEvent {
        pub event_type: u16,
        pub code: u16,
        pub value: i32,
    }

    #[repr(C)]
    #[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
    pub struct Event {
        pub kind: u16,
        pub flags: u16,
        pub window: u32,
        pub words: [u32; 6],
    }

    #[repr(C)]
    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub struct PresentHeaderV1 {
        pub magic: u32,
        pub version: u16,
        pub reserved: u16,
        pub sequence: u64,
        pub command_bytes: u32,
        pub damage_count: u16,
        pub flags: u16,
        pub payload_hash: u64,
        pub title_bytes: u16,
        pub title: [u8; 64],
        pub damage: [Rect; MAX_DAMAGE_RECTS],
    }

    impl Default for PresentHeaderV1 {
        fn default() -> Self {
            Self {
                magic: 0,
                version: 0,
                reserved: 0,
                sequence: 0,
                command_bytes: 0,
                damage_count: 0,
                flags: 0,
                payload_hash: 0,
                title_bytes: 0,
                title: [0; 64],
                damage: [Rect::default(); MAX_DAMAGE_RECTS],
            }
        }
    }

    #[repr(C)]
    #[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
    pub struct EventRingHeaderV1 {
        pub magic: u32,
        pub version: u16,
        pub capacity: u16,
        pub head: u32,
        pub tail: u32,
        pub dropped_pointer_moves: u32,
        pub flags: u32,
        pub reserved: [u32; 10],
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct SystemStats {
    pub version: u16,
    pub cpu_count: u16,
    pub reserved: u32,
    pub cpu_ticks: [u64; 2],
    pub free_frames: u64,
    pub kernel_heap_used: u64,
    pub kernel_heap_total: u64,
    pub runnable_threads: u32,
    pub blocked_threads: u32,
    pub irq_count: u64,
    pub ipc_calls: u64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct FilesystemStatsV1 {
    pub version: u16,
    pub reserved: u16,
    pub block_size: u32,
    pub total_blocks: u64,
    pub used_blocks: u64,
    pub free_blocks: u64,
    pub generation: u64,
    pub transaction: u64,
    pub entries: u64,
    pub checkpoint_block: u64,
    pub segments_since_checkpoint: u64,
    pub active_arena: u8,
    pub segment_directory: u8,
    pub active_segments: u16,
    pub reserved_tail: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ProcessInfoV2 {
    pub version: u16,
    pub running: u16,
    pub pid: u32,
    pub exit_status: i64,
    pub program: u64,
    pub cpu_ticks: [u64; 2],
    pub cpu_mask: u32,
    pub owned_pages: u32,
}

pub mod time {
    #[repr(u16)]
    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub enum Operation {
        Sleep = 1,
        Uptime = 2,
        Realtime = 3,
    }
}

pub mod boot_cap {
    use super::CapHandle;

    pub const SHARED_BLOCK_FRAME: CapHandle = CapHandle::from_parts(16, 1);
    pub const BLOCK_IRQ_NOTIFICATION: CapHandle = CapHandle::from_parts(17, 1);
    pub const VIRTIO_BLOCK_IRQ: CapHandle = CapHandle::from_parts(18, 1);
    pub const DEVICE_SYSTEM_CONTROL: CapHandle = CapHandle::from_parts(19, 1);
    pub const DEVMGR_VIRTIO_MMIO: CapHandle = CapHandle::from_parts(20, 1);
    pub const DEVMGR_QUEUE_FRAME: CapHandle = CapHandle::from_parts(21, 1);
    pub const DEVMGR_DMA_DOMAIN: CapHandle = CapHandle::from_parts(22, 1);
    pub const DEVMGR_VIRTIO_IRQ: CapHandle = CapHandle::from_parts(23, 1);
    pub const ROOT_MEMORY_POOL: CapHandle = CapHandle::from_parts(24, 1);
    pub const SHARED_FILESYSTEM_FRAME: CapHandle = CapHandle::from_parts(25, 1);
    pub const CONSOLE_ENDPOINT: CapHandle = CapHandle::from_parts(32, 1);
    pub const BLOCK_ENDPOINT: CapHandle = CapHandle::from_parts(33, 1);
    pub const FILESYSTEM_ENDPOINT: CapHandle = CapHandle::from_parts(34, 1);
    pub const PROCESS_ENDPOINT: CapHandle = CapHandle::from_parts(35, 1);
    pub const DEVMGR_ENDPOINT: CapHandle = CapHandle::from_parts(36, 1);
    pub const BLOCK_CONFIG_ENDPOINT: CapHandle = CapHandle::from_parts(37, 1);
    pub const DRIVER_MEMORY_POOL: CapHandle = CapHandle::from_parts(38, 1);
    pub const TIME_ENDPOINT: CapHandle = CapHandle::from_parts(39, 1);
    pub const GUI_TERMINAL_ENDPOINT: CapHandle = CapHandle::from_parts(40, 1);
    pub const GUI_FILES_ENDPOINT: CapHandle = CapHandle::from_parts(41, 1);
    pub const GUI_MONITOR_ENDPOINT: CapHandle = CapHandle::from_parts(42, 1);
    pub const GUI_TERMINAL_COMMANDS: CapHandle = CapHandle::from_parts(43, 1);
    pub const GUI_FILES_COMMANDS: CapHandle = CapHandle::from_parts(44, 1);
    pub const GUI_MONITOR_COMMANDS: CapHandle = CapHandle::from_parts(45, 1);
    pub const GUI_TERMINAL_EVENTS: CapHandle = CapHandle::from_parts(46, 1);
    pub const GUI_FILES_EVENTS: CapHandle = CapHandle::from_parts(47, 1);
    pub const GUI_MONITOR_EVENTS: CapHandle = CapHandle::from_parts(48, 1);
    pub const GUI_TERMINAL_NOTIFICATION: CapHandle = CapHandle::from_parts(49, 1);
    pub const GUI_FILES_NOTIFICATION: CapHandle = CapHandle::from_parts(50, 1);
    pub const GUI_MONITOR_NOTIFICATION: CapHandle = CapHandle::from_parts(51, 1);
    pub const GUI_MEMORY_POOL: CapHandle = CapHandle::from_parts(52, 1);
    pub const SYSTEM_INFO: CapHandle = CapHandle::from_parts(53, 1);
    pub const GUI_CONFIG_ENDPOINT: CapHandle = CapHandle::from_parts(54, 1);
    pub const GUI_FILESYSTEM_ENDPOINT: CapHandle = CapHandle::from_parts(55, 1);
    pub const TERMINAL_FILESYSTEM_ENDPOINT: CapHandle = CapHandle::from_parts(56, 1);
    pub const FILES_FILESYSTEM_ENDPOINT: CapHandle = CapHandle::from_parts(57, 1);
    pub const NETWORK_DEVICE: CapHandle = CapHandle::from_parts(58, 1);
    pub const RANDOM_SOURCE: CapHandle = CapHandle::from_parts(59, 1);
    pub const NETWORK_ENDPOINT: CapHandle = CapHandle::from_parts(60, 1);
    pub const NETWORK_MEMORY_POOL: CapHandle = CapHandle::from_parts(61, 1);
    pub const SCRIPT_MEMORY_POOL: CapHandle = CapHandle::from_parts(62, 1);
    pub const SCRIPT_SESSION_REGION: CapHandle = CapHandle::from_parts(63, 1);
    pub const SCRIPT_IO_NOTIFICATION: CapHandle = CapHandle::from_parts(64, 1);
    pub const SCRIPT_FILESYSTEM_ENDPOINT: CapHandle = CapHandle::from_parts(65, 1);
    pub const SCRIPT_NETWORK_ENDPOINT: CapHandle = CapHandle::from_parts(66, 1);
    pub const SCRIPT_RANDOM_SOURCE: CapHandle = CapHandle::from_parts(67, 1);
    pub const SCRIPT_SYSTEM_DATA: CapHandle = CapHandle::from_parts(68, 1);
    pub const SSH_NETWORK_ENDPOINT: CapHandle = CapHandle::from_parts(69, 1);
    pub const SCRIPT_BROKER_ENDPOINT: CapHandle = CapHandle::from_parts(70, 1);
    pub const SSH_FILESYSTEM_FRAME: CapHandle = CapHandle::from_parts(71, 1);
    pub const SCRIPT_GUI_COMMANDS: CapHandle = CapHandle::from_parts(72, 1);
    pub const SCRIPT_GUI_EVENTS: CapHandle = CapHandle::from_parts(73, 1);
    pub const SCRIPT_GUI_ENDPOINT: CapHandle = CapHandle::from_parts(74, 1);
    pub const GUI_DYNAMIC_ENDPOINT_BASE: u16 = 75;
    pub const WINDOWD_FILESYSTEM_FRAME: CapHandle = CapHandle::from_parts(83, 1);
    pub const GUI_LAUNCH_ENDPOINT: CapHandle = CapHandle::from_parts(84, 1);
    pub const SHELL_SYSTEM_CONTROL: CapHandle = CapHandle::from_parts(85, 1);
    pub const DATABASE_ENDPOINT: CapHandle = CapHandle::from_parts(86, 1);
    pub const DATABASE_FILESYSTEM_FRAME: CapHandle = CapHandle::from_parts(87, 1);

    pub const fn gui_dynamic_endpoint(index: usize) -> CapHandle {
        CapHandle::from_parts(GUI_DYNAMIC_ENDPOINT_BASE + index as u16, 1)
    }
}
