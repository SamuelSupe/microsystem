#![no_std]
#![no_main]

use core::panic::PanicInfo;
use core::sync::atomic::{AtomicU64, Ordering};
use microsystem_abi::{
    CapHandle, Message, Rights, Status, boot_cap, filesystem, gui, protocol, script,
};
use microsystem_gui::{Desktop, WindowState, replace_display_list, validate_command_stream};

const FRAMEBUFFER_VA: u64 = 0x0080_0000;
const FRAMEBUFFER_PAGES: u32 = gui::WIDTH * gui::HEIGHT * 4 / 4096;
const DISPLAY_LIST_VA: u64 = 0x00b0_0000;
const DISPLAY_LIST_ARENA_BYTES: usize = gui::MAX_DYNAMIC_CLIENTS * gui::COMMAND_BYTES;
const DISPLAY_LIST_PAGES: u32 = ((DISPLAY_LIST_ARENA_BYTES + gui::COMMAND_BYTES) / 4096) as u32;
const DISPLAY_LIST_STAGING_VA: u64 = DISPLAY_LIST_VA + DISPLAY_LIST_ARENA_BYTES as u64;
const DYNAMIC_SHARED_VA: u64 = 0x00c0_0000;
const DYNAMIC_SHARED_STRIDE: u64 = 0x0002_0000;
const FILESYSTEM_SHARED_VA: u64 = 0x005e_0000;
const TERMINAL_SHARED_VA: u64 = 0x0061_0000;
const UNIFONT_PATH: &[u8] = b"/.system/fonts/unifont-17.0.05.ufb";
const GLYPH_CACHE_SIZE: usize = 128;
const GLYPH_CACHE_WAYS: usize = 2;
const GLYPH_CACHE_SETS: usize = GLYPH_CACHE_SIZE / GLYPH_CACHE_WAYS;
const BACKGROUND: u32 = 0x00c4_d8e8;
const BACKGROUND_BANDS: [u32; 6] = [
    0x00b7_cfe2,
    0x00c3_d9ea,
    0x00d0_e3f0,
    0x00de_ebf4,
    0x00eb_f2f8,
    0x00f5_f8fb,
];
const TASKBAR: u32 = 0x00e9_eff5;
const TASKBAR_BUTTON: u32 = 0x00f8_fafd;
const BORDER: u32 = 0x00a9_b8c8;
const BORDER_ACTIVE: u32 = 0x006b_a7ea;
const TITLE_ACTIVE: u32 = 0x00f7_f9fc;
const TITLE_IDLE: u32 = 0x00e3_e9f0;
const CLIENT: u32 = 0x00ff_ffff;
const PANEL: u32 = 0x00f1_f5f9;
const SHADOW: u32 = 0x006b_7d91;
const TEXT: u32 = 0x001b_2733;
const MUTED_TEXT: u32 = 0x006b_7785;
const ACCENT: u32 = 0x0000_7aff;
const ACCENT_TINT: u32 = 0x00d9_eaff;
const CHROME_DIVIDER: u32 = 0x00d1_d9e2;
const CONTROL_INACTIVE: u32 = 0x00b8_c1ca;
const CONTROL_CLOSE: u32 = 0x00ff_5f57;
const CONTROL_MINIMIZE: u32 = 0x00febc2e;
const CONTROL_MAXIMIZE: u32 = 0x0028_c840;
const INLINE_TEXT_BYTES: usize = 40;
const TERMINAL_SHARED_BYTES: usize = 4096;
const TERMINAL_SHARED_MAGIC: u64 = 0x5445_524d_5348_4152;
const CLEAR_REPLY: u16 = 1;
const TERMINAL_BUFFER_BYTES: usize = 4096;
const TERMINAL_LINE_BYTES: usize = 512;
const IPC_POLL_BUDGET_NS: u64 = 100_000;
const INPUT_BATCH_LIMIT: usize = 16;
const DIRTY_REGION_CAPACITY: usize = gui::MAX_DAMAGE_RECTS * 2;
const PENDING_EVENT_CAPACITY: usize = 16;
const EMPTY_RECT: gui::Rect = gui::Rect {
    x: 0,
    y: 0,
    width: 0,
    height: 0,
};

#[derive(Clone, Copy)]
struct DynamicClient {
    active: bool,
    application: u16,
    token: u64,
    pid: u64,
    window: u32,
    commands: CapHandle,
    events: CapHandle,
    notification: CapHandle,
    pending: Option<Message>,
    pending_events: [Option<gui::Event>; PENDING_EVENT_CAPACITY],
    pending_event_count: usize,
    display_bytes: usize,
    sequence: u64,
    damage: [gui::Rect; gui::MAX_DAMAGE_RECTS],
    damage_count: usize,
    full_redraw: bool,
}

impl DynamicClient {
    const EMPTY: Self = Self {
        active: false,
        application: 0,
        token: 0,
        pid: 0,
        window: 0,
        commands: CapHandle::INVALID,
        events: CapHandle::INVALID,
        notification: CapHandle::INVALID,
        pending: None,
        pending_events: [None; PENDING_EVENT_CAPACITY],
        pending_event_count: 0,
        display_bytes: 0,
        sequence: 0,
        damage: [EMPTY_RECT; gui::MAX_DAMAGE_RECTS],
        damage_count: 0,
        full_redraw: false,
    };
}

struct DirtyRegions {
    regions: [gui::Rect; DIRTY_REGION_CAPACITY],
    count: usize,
    full: bool,
}

impl DirtyRegions {
    const fn new() -> Self {
        Self {
            regions: [EMPTY_RECT; DIRTY_REGION_CAPACITY],
            count: 0,
            full: false,
        }
    }

    fn clear(&mut self) {
        self.count = 0;
        self.full = false;
    }

    fn mark_full(&mut self) {
        self.count = 0;
        self.full = true;
    }

    fn push(&mut self, rect: gui::Rect) {
        if self.full || rect.width == 0 || rect.height == 0 {
            return;
        }
        let mut merged = rect;
        let mut index = 0;
        while index < self.count {
            if intersect_rect(merged, self.regions[index]).is_some() {
                merged = union_rect(merged, self.regions[index]);
                self.count -= 1;
                self.regions[index] = self.regions[self.count];
            } else {
                index += 1;
            }
        }
        if self.count == self.regions.len() {
            self.mark_full();
            return;
        }
        self.regions[self.count] = merged;
        self.count += 1;
    }

    fn as_slice(&self) -> &[gui::Rect] {
        if self.full {
            core::slice::from_ref(&FULL_SCREEN)
        } else {
            &self.regions[..self.count]
        }
    }

    fn is_empty(&self) -> bool {
        !self.full && self.count == 0
    }
}

const FULL_SCREEN: gui::Rect = gui::Rect {
    x: 0,
    y: 0,
    width: gui::WIDTH,
    height: gui::HEIGHT,
};

static mut RENDER_DAMAGE: [gui::Rect; DIRTY_REGION_CAPACITY] = [EMPTY_RECT; DIRTY_REGION_CAPACITY];
static mut RENDER_DAMAGE_COUNT: usize = 0;
static DESKTOP_LAUNCH_DEADLINES: [AtomicU64; 2] = [const { AtomicU64::new(0) }; 2];

fn set_render_damage(regions: &[gui::Rect]) {
    let count = regions.len().min(DIRTY_REGION_CAPACITY);
    unsafe {
        let target = core::ptr::addr_of_mut!(RENDER_DAMAGE).cast::<gui::Rect>();
        core::ptr::copy_nonoverlapping(regions.as_ptr(), target, count);
        core::ptr::write(core::ptr::addr_of_mut!(RENDER_DAMAGE_COUNT), count);
    }
}

fn render_damage_count() -> usize {
    unsafe { core::ptr::read(core::ptr::addr_of!(RENDER_DAMAGE_COUNT)) }
}

fn render_damage(index: usize) -> gui::Rect {
    unsafe {
        core::ptr::read(
            core::ptr::addr_of!(RENDER_DAMAGE)
                .cast::<gui::Rect>()
                .add(index),
        )
    }
}

fn pixel_is_dirty(x: i32, y: i32) -> bool {
    (0..render_damage_count()).any(|index| {
        let damage = render_damage(index);
        x >= damage.x
            && y >= damage.y
            && x < damage.x.saturating_add(damage.width as i32)
            && y < damage.y.saturating_add(damage.height as i32)
    })
}

fn rect_is_dirty(rect: gui::Rect) -> bool {
    (0..render_damage_count()).any(|index| intersect_rect(rect, render_damage(index)).is_some())
}

#[derive(Clone, Copy)]
struct CachedGlyph {
    codepoint: u32,
    width: u8,
    bitmap: [u8; 32],
    valid: bool,
}

impl CachedGlyph {
    const EMPTY: Self = Self {
        codepoint: 0,
        width: 0,
        bitmap: [0; 32],
        valid: false,
    };
}

struct FontCache {
    available: bool,
    next_way: [u8; GLYPH_CACHE_SETS],
    entries: [CachedGlyph; GLYPH_CACHE_SIZE],
}

impl FontCache {
    fn load() -> Self {
        let mut cache = Self {
            available: false,
            next_way: [0; GLYPH_CACHE_SETS],
            entries: [CachedGlyph::EMPTY; GLYPH_CACHE_SIZE],
        };
        let mut header = [0u8; 16];
        cache.available = read_font_range(0, &mut header).is_ok()
            && &header[..4] == b"UFB1"
            && u32::from_le_bytes(header[12..16].try_into().unwrap_or([0; 4])) == 36;
        let marker = if cache.available {
            b"[gui] unifont runtime loaded=true cache=128 fallback=ascii\n".as_slice()
        } else {
            b"[gui] unifont runtime loaded=false cache=128 fallback=ascii\n".as_slice()
        };
        let _ = microsystem_user_rt::debug_write(marker);
        cache
    }

    fn glyph(&mut self, codepoint: u32) -> Option<CachedGlyph> {
        if !self.available || codepoint > u16::MAX as u32 {
            return None;
        }
        let set = codepoint as usize & (GLYPH_CACHE_SETS - 1);
        let first = set * GLYPH_CACHE_WAYS;
        if let Some(glyph) = self.entries[first..first + GLYPH_CACHE_WAYS]
            .iter()
            .find(|glyph| glyph.valid && glyph.codepoint == codepoint)
        {
            return Some(*glyph);
        }
        let mut encoded_offset = [0u8; 4];
        read_font_range(16 + codepoint as u64 * 4, &mut encoded_offset).ok()?;
        let offset = u32::from_le_bytes(encoded_offset) as u64;
        if offset == 0 {
            return None;
        }
        let mut record = [0u8; 36];
        read_font_range(offset, &mut record).ok()?;
        if !matches!((record[0], record[1]), (8 | 16, 16)) {
            return None;
        }
        let mut bitmap = [0u8; 32];
        bitmap.copy_from_slice(&record[4..]);
        let glyph = CachedGlyph {
            codepoint,
            width: record[0],
            bitmap,
            valid: true,
        };
        let way = self.next_way[set] as usize;
        self.entries[first + way] = glyph;
        self.next_way[set] = ((way + 1) % GLYPH_CACHE_WAYS) as u8;
        Some(glyph)
    }
}

fn read_font_range(offset: u64, output: &mut [u8]) -> Result<(), Status> {
    unsafe {
        core::ptr::copy_nonoverlapping(
            UNIFONT_PATH.as_ptr(),
            FILESYSTEM_SHARED_VA as *mut u8,
            UNIFONT_PATH.len(),
        );
        microsystem_user_rt::fence();
    }
    let mut request = Message::new(
        protocol::FILESYSTEM,
        filesystem::Operation::ReadRange as u16,
    );
    request.words[0] = UNIFONT_PATH.len() as u64;
    request.words[2] = offset;
    request.caps[0] = boot_cap::WINDOWD_FILESYSTEM_FRAME;
    let mut reply = Message::new(protocol::FILESYSTEM, 0);
    microsystem_user_rt::ipc_call(boot_cap::GUI_FILESYSTEM_ENDPOINT, &request, &mut reply, 0)?;
    if reply.words[5] as i64 != Status::Ok as i64 || (reply.words[0] as usize) < output.len() {
        return Err(Status::Io);
    }
    unsafe {
        microsystem_user_rt::fence();
        core::ptr::copy_nonoverlapping(
            FILESYSTEM_SHARED_VA as *const u8,
            output.as_mut_ptr(),
            output.len(),
        );
    }
    Ok(())
}

#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    let _ = microsystem_user_rt::debug_write(b"[user] windowd service ELF entered EL0\n");
    let rights = Rights(Rights::READ.0 | Rights::WRITE.0 | Rights::MAP.0 | Rights::MANAGE.0);
    let region = microsystem_user_rt::frame_region_create(
        boot_cap::GUI_MEMORY_POOL,
        FRAMEBUFFER_PAGES,
        rights,
    )
    .unwrap_or_else(|_| microsystem_user_rt::exit(2));
    microsystem_user_rt::frame_map(
        region,
        FRAMEBUFFER_VA,
        Rights(Rights::READ.0 | Rights::WRITE.0),
    )
    .unwrap_or_else(|_| microsystem_user_rt::exit(3));

    let display_region = microsystem_user_rt::frame_region_create(
        boot_cap::GUI_MEMORY_POOL,
        DISPLAY_LIST_PAGES,
        rights,
    )
    .unwrap_or_else(|_| microsystem_user_rt::exit(8));
    microsystem_user_rt::frame_map(
        display_region,
        DISPLAY_LIST_VA,
        Rights(Rights::READ.0 | Rights::WRITE.0),
    )
    .unwrap_or_else(|_| microsystem_user_rt::exit(9));

    let framebuffer = unsafe {
        core::slice::from_raw_parts_mut(
            FRAMEBUFFER_VA as *mut u32,
            (gui::WIDTH * gui::HEIGHT) as usize,
        )
    };
    let display_lists = unsafe {
        core::slice::from_raw_parts_mut(DISPLAY_LIST_VA as *mut u8, DISPLAY_LIST_ARENA_BYTES)
    };
    let display_staging = unsafe {
        core::slice::from_raw_parts_mut(DISPLAY_LIST_STAGING_VA as *mut u8, gui::COMMAND_BYTES)
    };
    let mut desktop = Desktop::new();
    let mut dynamic_clients = [DynamicClient::EMPTY; gui::MAX_DYNAMIC_CLIENTS];
    let mut config_pending = None;
    let mut terminal = TerminalView::new();
    let mut font = FontCache::load();
    clear(framebuffer, BACKGROUND);
    fill_rect(
        framebuffer,
        gui::Rect {
            x: 0,
            y: gui::HEIGHT as i32 - microsystem_gui::TASKBAR_HEIGHT,
            width: gui::WIDTH,
            height: microsystem_gui::TASKBAR_HEIGHT as u32,
        },
        TASKBAR,
    );

    let clients = [
        (boot_cap::GUI_TERMINAL_ENDPOINT, 1u8, b"Terminal".as_slice()),
        (boot_cap::GUI_FILES_ENDPOINT, 2u8, b"Files".as_slice()),
        (boot_cap::GUI_MONITOR_ENDPOINT, 3u8, b"Monitor".as_slice()),
    ];
    for (endpoint, client, title) in clients {
        let mut request = Message::new(protocol::GUI, 0);
        if microsystem_user_rt::ipc_recv(endpoint, &mut request, 0).is_err()
            || request.protocol != protocol::GUI
            || request.opcode != gui::Operation::CreateWindow as u16
        {
            microsystem_user_rt::exit(4);
        }
        let rect = gui::Rect {
            x: request.words[0] as i32,
            y: request.words[1] as i32,
            width: request.words[2] as u32,
            height: request.words[3] as u32,
        };
        let id = desktop
            .create_window(client, rect, title)
            .unwrap_or_else(|_| microsystem_user_rt::exit(5));
        let mut reply = Message::new(protocol::GUI, gui::Operation::CreateWindow as u16);
        reply.words[0] = id as u64;
        reply.words[1] = gui::WIDTH as u64;
        reply.words[2] = gui::HEIGHT as u64;
        let mut next = Message::new(0, 0);
        let deadline = microsystem_user_rt::clock_now()
            .unwrap_or(1)
            .saturating_add(10_000_000);
        match microsystem_user_rt::ipc_reply_recv(endpoint, &reply, &mut next, deadline) {
            Err(Status::TimedOut | Status::Busy) | Ok(()) => {}
            Err(_) => microsystem_user_rt::exit(6),
        }
    }
    render(
        &desktop,
        &terminal,
        &dynamic_clients,
        display_lists,
        &mut font,
        framebuffer,
        core::slice::from_ref(&FULL_SCREEN),
    );
    if microsystem_user_rt::gui_present(region).is_err() {
        microsystem_user_rt::exit(7);
    }
    let _ = microsystem_user_rt::debug_write(
        b"[gui] windowd ready resolution=1024x768 format=XRGB8888 clients=3 renderer=commands\n",
    );
    let _ = microsystem_user_rt::debug_write(
        b"[gui] renderer damage-merge=true local-window-damage=true command-cull=true glyph-cache=2way input-batch=16\n",
    );
    let _ = microsystem_user_rt::debug_write(
        b"[gui] desktop windows=3 move=true resize=true minimize=true maximize=true alt-tab=true\n",
    );
    let _ = microsystem_user_rt::debug_write(
        b"[gui] desktop launcher icons=5 fixed-registry=true click-to-open=true\n",
    );
    let _ = microsystem_user_rt::service_online();
    let mut pointer = (gui::WIDTH as i32 / 2, gui::HEIGHT as i32 / 2);
    let mut dragging: Option<(u32, i32, i32)> = None;
    let mut resizing: Option<(u32, i32, i32, u32, u32)> = None;
    let mut alt = false;
    let mut control = false;
    let mut shift = false;
    let mut unicode = UnicodeInput::new();
    let mut input_reported = false;
    let mut redraw_pending = false;
    let mut dirty = DirtyRegions::new();
    let mut next_frame = microsystem_user_rt::clock_now().unwrap_or(0);
    loop {
        let mut dynamic_changed = false;
        match service_dynamic_config(&mut config_pending, &mut dynamic_clients, &mut desktop) {
            Ok(true) => {
                dirty.mark_full();
                redraw_pending = true;
            }
            Ok(false) => {}
            Err(_) => microsystem_user_rt::exit(10),
        }
        for index in 0..dynamic_clients.len() {
            let changed = service_dynamic_client(
                index,
                &mut dynamic_clients,
                &mut desktop,
                display_lists,
                display_staging,
            )
            .unwrap_or_else(|_| microsystem_user_rt::exit(11));
            dynamic_changed |= changed;
            if changed {
                if dynamic_clients[index].full_redraw {
                    dirty.mark_full();
                } else {
                    for damage in dynamic_clients[index]
                        .damage
                        .iter()
                        .take(dynamic_clients[index].damage_count)
                    {
                        dirty.push(*damage);
                    }
                }
                dynamic_clients[index].damage_count = 0;
                dynamic_clients[index].full_redraw = false;
            }
        }
        if dynamic_changed && !dirty.is_empty() {
            redraw_pending = true;
        }
        for _ in 0..INPUT_BATCH_LIMIT {
            let input_blocked = dynamic_clients
                .iter()
                .any(|client| client.pending_event_count != 0);
            if input_blocked {
                break;
            }
            let mut event = gui::InputEvent::default();
            if microsystem_user_rt::gui_input(&mut event).is_err() {
                break;
            }
            let mut changed = false;
            let mut terminal_changed = false;
            let desktop_before = capture_desktop_visual(&desktop);
            let previous_focus = desktop.focused();
            let previous_pointer = pointer;
            match (event.event_type, event.code) {
                (1, 56 | 100) => alt = event.value != 0,
                (1, 29 | 97) => control = event.value != 0,
                (1, 42 | 54) => shift = event.value != 0,
                (1, 15) if alt && event.value == 1 => {
                    changed = desktop.alt_tab().is_some();
                }
                (1, 62) if alt && event.value == 1 => {
                    if let Some(id) = desktop.focused() {
                        changed = request_close(id, &desktop, &mut dynamic_clients);
                        if !changed {
                            changed = desktop.action(id, gui::WindowAction::Close).is_ok();
                        }
                    }
                }
                (1, 67) if alt && event.value == 1 => {
                    if let Some(id) = desktop.focused() {
                        changed = desktop.action(id, gui::WindowAction::Minimize).is_ok();
                        if changed {
                            push_configure(id, &desktop, &mut dynamic_clients);
                        }
                    }
                }
                (1, 68) if alt && event.value == 1 => {
                    if let Some(id) = desktop.focused() {
                        let action = if desktop
                            .window(id)
                            .is_some_and(|window| window.state == WindowState::Maximized)
                        {
                            gui::WindowAction::Restore
                        } else {
                            gui::WindowAction::Maximize
                        };
                        changed = desktop.action(id, action).is_ok();
                        if changed {
                            push_configure(id, &desktop, &mut dynamic_clients);
                            push_expose(id, &desktop, &mut dynamic_clients);
                        }
                    }
                }
                (1, 0x110) if event.value == 1 => {
                    if pointer.1 >= gui::HEIGHT as i32 - microsystem_gui::TASKBAR_HEIGHT {
                        if let Some(id) = taskbar_window_at(&desktop, pointer.0) {
                            let _ = desktop.action(id, gui::WindowAction::Restore);
                            let _ = desktop.focus(id);
                            push_configure(id, &desktop, &mut dynamic_clients);
                            push_expose(id, &desktop, &mut dynamic_clients);
                            changed = true;
                        }
                    } else if let Some(id) = desktop.hit_test(pointer.0, pointer.1) {
                        let _ = desktop.focus(id);
                        if let Some(window) = desktop.window(id).copied() {
                            if pointer.1 < window.rect.y + microsystem_gui::TITLE_BAR_HEIGHT {
                                if let Some(mut action) = window_control_at(window.rect, pointer.0)
                                {
                                    if action == gui::WindowAction::Maximize
                                        && window.state == WindowState::Maximized
                                    {
                                        action = gui::WindowAction::Restore;
                                    }
                                    if action == gui::WindowAction::Close {
                                        if !request_close(id, &desktop, &mut dynamic_clients) {
                                            let _ = desktop.action(id, action);
                                        }
                                    } else {
                                        let _ = desktop.action(id, action);
                                        push_configure(id, &desktop, &mut dynamic_clients);
                                        if action != gui::WindowAction::Minimize {
                                            push_expose(id, &desktop, &mut dynamic_clients);
                                        }
                                    }
                                } else {
                                    dragging = Some((
                                        id,
                                        pointer.0 - window.rect.x,
                                        pointer.1 - window.rect.y,
                                    ));
                                }
                            } else if pointer.0 >= window.rect.x + window.rect.width as i32 - 14
                                && pointer.1 >= window.rect.y + window.rect.height as i32 - 14
                            {
                                resizing = Some((
                                    id,
                                    pointer.0,
                                    pointer.1,
                                    window.rect.width,
                                    window.rect.height,
                                ));
                            }
                        }
                        changed = true;
                    } else if let Some(application) =
                        microsystem_gui::desktop_icon_at(pointer.0, pointer.1)
                    {
                        changed =
                            activate_application(application, &mut desktop, &mut dynamic_clients);
                    }
                }
                (1, 0x110) if event.value == 0 => {
                    dragging = None;
                    resizing = None;
                }
                (1, 14) if event.value == 1 && terminal_focused(&desktop) => {
                    changed = terminal.backspace();
                    terminal_changed = changed;
                }
                (1, 28) if event.value == 1 && terminal_focused(&desktop) => {
                    terminal.submit();
                    changed = true;
                    terminal_changed = true;
                }
                (1, code) if event.value == 1 && !alt && terminal_focused(&desktop) => {
                    if let Some(byte) = keycode_to_ascii(code, shift) {
                        changed = terminal.push(byte);
                        terminal_changed = changed;
                    }
                }
                (3, 0) => {
                    pointer.0 = (event.value.clamp(0, 32767) as i64 * (gui::WIDTH - 1) as i64
                        / 32767) as i32;
                    changed = true;
                    if let Some((id, dx, dy)) = dragging {
                        changed |= desktop
                            .move_window(id, pointer.0 - dx, pointer.1 - dy)
                            .is_ok();
                    }
                    if let Some((id, start_x, _, width, height)) = resizing {
                        let next = (width as i32 + pointer.0 - start_x).max(1) as u32;
                        changed |= desktop.resize_window(id, next, height).is_ok();
                    }
                }
                (3, 1) => {
                    pointer.1 = (event.value.clamp(0, 32767) as i64 * (gui::HEIGHT - 1) as i64
                        / 32767) as i32;
                    changed = true;
                    if let Some((id, dx, dy)) = dragging {
                        changed |= desktop
                            .move_window(id, pointer.0 - dx, pointer.1 - dy)
                            .is_ok();
                    }
                    if let Some((id, _, start_y, width, height)) = resizing {
                        let next = (height as i32 + pointer.1 - start_y).max(1) as u32;
                        changed |= desktop.resize_window(id, width, next).is_ok();
                    }
                }
                _ => {}
            }
            route_dynamic_input(
                event,
                pointer,
                control,
                shift,
                alt,
                &mut unicode,
                previous_focus,
                &desktop,
                &mut dynamic_clients,
            );
            if changed {
                if let Some(id) = dragging
                    .map(|value| value.0)
                    .or_else(|| resizing.map(|value| value.0))
                {
                    push_configure(id, &desktop, &mut dynamic_clients);
                }
            }
            if changed {
                let desktop_after = capture_desktop_visual(&desktop);
                if event.event_type == 3 && dragging.is_none() && resizing.is_none() {
                    dirty.push(pointer_damage(previous_pointer));
                    dirty.push(pointer_damage(pointer));
                } else {
                    let visual_changed =
                        mark_desktop_visual_changes(&desktop_before, &desktop_after, &mut dirty);
                    if terminal_changed
                        && let Some(window) = desktop.focused().and_then(|id| desktop.window(id))
                    {
                        dirty.push(window_paint_bounds(window.rect));
                    } else if !visual_changed && event.event_type == 3 {
                        dirty.push(pointer_damage(previous_pointer));
                        dirty.push(pointer_damage(pointer));
                    }
                }
                redraw_pending |= !dirty.is_empty();
                if !input_reported {
                    let _ = microsystem_user_rt::debug_write(
                        b"[gui] keyboard+tablet input routed focus=true drag=true alt-tab=true\n",
                    );
                    input_reported = true;
                }
            }
        }
        let now = microsystem_user_rt::clock_now().unwrap_or(next_frame);
        if redraw_pending && !dirty.is_empty() && now >= next_frame {
            if let Some(present) = damage_bounds(dirty.as_slice()) {
                render(
                    &desktop,
                    &terminal,
                    &dynamic_clients,
                    display_lists,
                    &mut font,
                    framebuffer,
                    core::slice::from_ref(&present),
                );
                draw_pointer(framebuffer, pointer.0, pointer.1);
                let _ = microsystem_user_rt::gui_present_rect(region, present);
            }
            redraw_pending = false;
            dirty.clear();
            next_frame = now.saturating_add(16_666_667);
        }
        let _ = microsystem_user_rt::yield_now();
    }
}

fn damage_bounds(damage: &[gui::Rect]) -> Option<gui::Rect> {
    let mut bounds: Option<gui::Rect> = None;
    for rect in damage {
        let Some(rect) = intersect_rect(*rect, FULL_SCREEN) else {
            continue;
        };
        bounds = Some(match bounds {
            None => rect,
            Some(bounds) => {
                let left = bounds.x.min(rect.x);
                let top = bounds.y.min(rect.y);
                let right = bounds
                    .x
                    .saturating_add(bounds.width as i32)
                    .max(rect.x.saturating_add(rect.width as i32));
                let bottom = bounds
                    .y
                    .saturating_add(bounds.height as i32)
                    .max(rect.y.saturating_add(rect.height as i32));
                gui::Rect {
                    x: left,
                    y: top,
                    width: (right - left) as u32,
                    height: (bottom - top) as u32,
                }
            }
        });
    }
    bounds
}

fn pointer_damage(pointer: (i32, i32)) -> gui::Rect {
    gui::Rect {
        x: pointer.0,
        y: pointer.1,
        width: 10,
        height: 16,
    }
}

#[derive(Clone, Copy)]
struct WindowVisual {
    id: u32,
    rect: gui::Rect,
    state: WindowState,
    z: u8,
}

impl WindowVisual {
    const EMPTY: Self = Self {
        id: 0,
        rect: EMPTY_RECT,
        state: WindowState::Closed,
        z: 0,
    };
}

struct DesktopVisual {
    windows: [WindowVisual; microsystem_gui::MAX_WINDOWS],
    count: usize,
    focused: Option<u32>,
}

fn capture_desktop_visual(desktop: &Desktop) -> DesktopVisual {
    let mut visual = DesktopVisual {
        windows: [WindowVisual::EMPTY; microsystem_gui::MAX_WINDOWS],
        count: 0,
        focused: desktop.focused(),
    };
    for (z, window) in desktop.z_order().enumerate() {
        visual.windows[visual.count] = WindowVisual {
            id: window.id,
            rect: window.rect,
            state: window.state,
            z: z as u8,
        };
        visual.count += 1;
    }
    visual
}

fn window_paint_bounds(rect: gui::Rect) -> gui::Rect {
    gui::Rect {
        width: rect.width.saturating_add(5),
        height: rect.height.saturating_add(6),
        ..rect
    }
}

fn mark_desktop_visual_changes(
    before: &DesktopVisual,
    after: &DesktopVisual,
    dirty: &mut DirtyRegions,
) -> bool {
    let mut changed = false;
    for window in before.windows[..before.count].iter() {
        let next = after.windows[..after.count]
            .iter()
            .find(|candidate| candidate.id == window.id);
        if next.is_none_or(|next| {
            next.rect != window.rect || next.state != window.state || next.z != window.z
        }) || before.focused != after.focused && before.focused == Some(window.id)
        {
            dirty.push(window_paint_bounds(window.rect));
            changed = true;
        }
    }
    for window in after.windows[..after.count].iter() {
        let previous = before.windows[..before.count]
            .iter()
            .find(|candidate| candidate.id == window.id);
        if previous.is_none_or(|previous| {
            previous.rect != window.rect || previous.state != window.state || previous.z != window.z
        }) || before.focused != after.focused && after.focused == Some(window.id)
        {
            dirty.push(window_paint_bounds(window.rect));
            changed = true;
        }
    }
    if before.focused != after.focused || before.count != after.count {
        dirty.push(gui::Rect {
            x: 0,
            y: gui::HEIGHT as i32 - microsystem_gui::TASKBAR_HEIGHT,
            width: gui::WIDTH,
            height: microsystem_gui::TASKBAR_HEIGHT as u32,
        });
        changed = true;
    }
    changed
}

fn service_dynamic_config(
    pending: &mut Option<Message>,
    clients: &mut [DynamicClient; gui::MAX_DYNAMIC_CLIENTS],
    desktop: &mut Desktop,
) -> Result<bool, Status> {
    let Some(request) = poll_endpoint(boot_cap::GUI_CONFIG_ENDPOINT, pending)? else {
        return Ok(false);
    };
    let mut reply = Message::new(protocol::GUI, request.opcode);
    let mut changed = false;
    let status = match request.opcode {
        value if value == gui::Operation::RegisterClient as u16 => {
            let _ = microsystem_user_rt::debug_write_u64(
                b"[gui] mica registration received endpoint=",
                request.words[0],
                b"\n",
            );
            let index = request.words[0] as usize;
            if request.protocol != protocol::GUI
                || index >= clients.len()
                || clients[index].active
                || request.words[1] == 0
                || request.words[2] == 0
                || (request.words[3] != 0 && gui::Application::from_u64(request.words[3]).is_none())
                || request.caps[..3].contains(&CapHandle::INVALID)
            {
                delete_message_caps(&request);
                Status::Invalid
            } else {
                let command_va = dynamic_command_va(index);
                let event_va = dynamic_event_va(index);
                let commands = request.caps[0];
                let events = request.caps[1];
                let notification = request.caps[2];
                match microsystem_user_rt::frame_map(commands, command_va, Rights::READ) {
                    Err(status) => {
                        let _ = microsystem_user_rt::debug_write_u64(
                            b"[gui] mica command map failed status=",
                            (status as i64).unsigned_abs(),
                            b"\n",
                        );
                        delete_message_caps(&request);
                        status
                    }
                    Ok(()) => match microsystem_user_rt::frame_map(
                        events,
                        event_va,
                        Rights(Rights::READ.0 | Rights::WRITE.0),
                    ) {
                        Err(status) => {
                            let _ = microsystem_user_rt::debug_write_u64(
                                b"[gui] mica event map failed status=",
                                (status as i64).unsigned_abs(),
                                b"\n",
                            );
                            let _ = microsystem_user_rt::frame_unmap(commands, command_va);
                            delete_message_caps(&request);
                            status
                        }
                        Ok(()) => {
                            clear_desktop_launch_pending(request.words[3]);
                            clients[index] = DynamicClient {
                                active: true,
                                application: request.words[3] as u16,
                                token: request.words[2],
                                pid: request.words[1],
                                commands,
                                events,
                                notification,
                                ..DynamicClient::EMPTY
                            };
                            let _ = microsystem_user_rt::debug_write(
                                b"[gui] mica client registered command=65536 event=4096 endpoint-isolated=true\n",
                            );
                            Status::Ok
                        }
                    },
                }
            }
        }
        value if value == gui::Operation::UnregisterClient as u16 => {
            let index = request.words[0] as usize;
            if index >= clients.len()
                || !clients[index].active
                || clients[index].token != request.words[2]
            {
                Status::NotFound
            } else {
                release_dynamic_client(index, clients, desktop);
                changed = true;
                Status::Ok
            }
        }
        _ => {
            delete_message_caps(&request);
            Status::Invalid
        }
    };
    reply.words[5] = status as i64 as u64;
    reply_endpoint(boot_cap::GUI_CONFIG_ENDPOINT, &reply, pending)?;
    Ok(changed)
}

fn service_dynamic_client(
    index: usize,
    clients: &mut [DynamicClient; gui::MAX_DYNAMIC_CLIENTS],
    desktop: &mut Desktop,
    display_lists: &mut [u8],
    display_staging: &mut [u8],
) -> Result<bool, Status> {
    if !clients[index].active {
        return Ok(false);
    }
    if clients[index].pid == 0 || clients[index].token == 0 {
        return Err(Status::Fault);
    }
    retry_pending_event(index, clients)?;
    let endpoint = boot_cap::gui_dynamic_endpoint(index);
    let Some(request) = poll_endpoint(endpoint, &mut clients[index].pending)? else {
        return Ok(false);
    };
    let mut reply = Message::new(protocol::GUI, request.opcode);
    let mut changed = false;
    let status = if request.protocol != protocol::GUI {
        Status::Invalid
    } else {
        match request.opcode {
            value if value == gui::Operation::CreateWindow as u16 => {
                if clients[index].window != 0 {
                    Status::Busy
                } else {
                    let header = dynamic_present_header(index);
                    let title_bytes = header.title_bytes as usize;
                    if header.magic != gui::PRESENT_MAGIC
                        || header.version != gui::VERSION
                        || title_bytes > header.title.len()
                        || core::str::from_utf8(&header.title[..title_bytes]).is_err()
                    {
                        Status::Invalid
                    } else {
                        let rect = gui::Rect {
                            x: request.words[0] as i32,
                            y: request.words[1] as i32,
                            width: request.words[2] as u32,
                            height: request.words[3] as u32,
                        };
                        match desktop.create_window(
                            4 + index as u8,
                            rect,
                            &header.title[..title_bytes],
                        ) {
                            Ok(window) => {
                                clients[index].window = window;
                                clients[index].full_redraw = true;
                                reply.words[0] = window as u64;
                                reply.words[1] = gui::WIDTH as u64;
                                reply.words[2] = gui::HEIGHT as u64;
                                push_configure(window, desktop, clients);
                                let _ = push_event(
                                    index,
                                    clients,
                                    event_word(gui::EventKind::Focus, window, 1),
                                );
                                push_expose(window, desktop, clients);
                                changed = true;
                                Status::Ok
                            }
                            Err(status) => status,
                        }
                    }
                }
            }
            value if value == gui::Operation::Present as u16 => {
                match snapshot_display_list(index, clients, desktop, display_lists, display_staging)
                {
                    Ok(()) => {
                        changed = true;
                        Status::Ok
                    }
                    Err(status) => status,
                }
            }
            value if value == gui::Operation::SetTitle as u16 => {
                let header = dynamic_present_header(index);
                let title_bytes = header.title_bytes as usize;
                if title_bytes > header.title.len() {
                    Status::Invalid
                } else {
                    match desktop.set_title(clients[index].window, &header.title[..title_bytes]) {
                        Ok(()) => {
                            clients[index].full_redraw = true;
                            changed = true;
                            Status::Ok
                        }
                        Err(status) => status,
                    }
                }
            }
            value if value == gui::Operation::QueryGeometry as u16 => {
                match desktop.window(clients[index].window) {
                    Some(window) => {
                        reply.words[0] = window.rect.x as u64;
                        reply.words[1] = window.rect.y as u64;
                        reply.words[2] = window.rect.width as u64;
                        reply.words[3] = window.rect.height as u64;
                        Status::Ok
                    }
                    None => Status::NotFound,
                }
            }
            value if value == gui::Operation::WindowAction as u16 => {
                let window = clients[index].window;
                let action = match request.words[0] as u16 {
                    1 => Some(gui::WindowAction::Minimize),
                    2 => Some(gui::WindowAction::Maximize),
                    3 => Some(gui::WindowAction::Restore),
                    4 => Some(gui::WindowAction::Close),
                    _ => None,
                };
                match action {
                    Some(action) => match desktop.action(window, action) {
                        Ok(()) => {
                            clients[index].full_redraw = true;
                            if action != gui::WindowAction::Close {
                                push_configure(window, desktop, clients);
                                if action != gui::WindowAction::Minimize {
                                    push_expose(window, desktop, clients);
                                }
                            }
                            changed = true;
                            Status::Ok
                        }
                        Err(status) => status,
                    },
                    None => Status::Invalid,
                }
            }
            value if value == gui::Operation::EventConsumed as u16 => {
                acknowledge_event(index, request.words[0] as u32)
            }
            _ => Status::Invalid,
        }
    };
    reply.words[5] = status as i64 as u64;
    reply_endpoint(endpoint, &reply, &mut clients[index].pending)?;
    Ok(changed)
}

fn poll_endpoint(
    endpoint: CapHandle,
    pending: &mut Option<Message>,
) -> Result<Option<Message>, Status> {
    if pending.is_some() {
        return Ok(pending.take());
    }
    let mut request = Message::new(0, 0);
    let deadline = microsystem_user_rt::clock_now()?.saturating_add(IPC_POLL_BUDGET_NS);
    match microsystem_user_rt::ipc_recv(endpoint, &mut request, deadline) {
        Ok(()) => Ok(Some(request)),
        Err(Status::TimedOut | Status::Busy) => Ok(None),
        Err(status) => Err(status),
    }
}

fn reply_endpoint(
    endpoint: CapHandle,
    reply: &Message,
    pending: &mut Option<Message>,
) -> Result<(), Status> {
    let mut next = Message::new(0, 0);
    let deadline = microsystem_user_rt::clock_now()?.saturating_add(IPC_POLL_BUDGET_NS);
    match microsystem_user_rt::ipc_reply_recv(endpoint, reply, &mut next, deadline) {
        Ok(()) => {
            *pending = Some(next);
            Ok(())
        }
        Err(Status::TimedOut | Status::Busy) => Ok(()),
        Err(status) => Err(status),
    }
}

fn delete_message_caps(message: &Message) {
    for capability in message.caps.iter().copied() {
        if capability != CapHandle::INVALID {
            let _ = microsystem_user_rt::cap_delete(capability);
        }
    }
}

fn release_dynamic_client(
    index: usize,
    clients: &mut [DynamicClient; gui::MAX_DYNAMIC_CLIENTS],
    desktop: &mut Desktop,
) {
    let client = clients[index];
    if client.window != 0 {
        let _ = desktop.remove_window(client.window);
    }
    let _ = microsystem_user_rt::frame_unmap(client.commands, dynamic_command_va(index));
    let _ = microsystem_user_rt::frame_unmap(client.events, dynamic_event_va(index));
    let _ = microsystem_user_rt::cap_delete(client.notification);
    let _ = microsystem_user_rt::cap_delete(client.events);
    let _ = microsystem_user_rt::cap_delete(client.commands);
    clients[index] = DynamicClient::EMPTY;
    let _ = microsystem_user_rt::debug_write_u64(
        b"[gui] mica client unregistered endpoint=",
        index as u64,
        b" resources-reclaimed=true\n",
    );
}

fn snapshot_display_list(
    index: usize,
    clients: &mut [DynamicClient; gui::MAX_DYNAMIC_CLIENTS],
    desktop: &Desktop,
    display_lists: &mut [u8],
    display_staging: &mut [u8],
) -> Result<(), Status> {
    let first = dynamic_present_header(index);
    if first.magic != gui::PRESENT_MAGIC
        || first.version != gui::VERSION
        || first.flags != 0
        || first.command_bytes as usize > gui::COMMAND_PAYLOAD_BYTES
        || first.damage_count as usize > gui::MAX_DAMAGE_RECTS
        || first.sequence <= clients[index].sequence
    {
        return Err(Status::Invalid);
    }
    let bytes = first.command_bytes as usize;
    let source = unsafe {
        core::slice::from_raw_parts(
            (dynamic_command_va(index) as usize + gui::COMMAND_HEADER_BYTES) as *const u8,
            bytes,
        )
    };
    let snapshot = &mut display_staging[..bytes];
    snapshot.copy_from_slice(source);
    microsystem_user_rt::fence();
    let second = dynamic_present_header(index);
    if first != second || display_hash(snapshot) != first.payload_hash {
        return Err(Status::Busy);
    }
    validate_command_stream(snapshot)?;
    let window = desktop
        .window(clients[index].window)
        .ok_or(Status::NotFound)?;
    let client_bounds = gui::Rect {
        x: 0,
        y: 0,
        width: window.rect.width.saturating_sub(4),
        height: window
            .rect
            .height
            .saturating_sub(microsystem_gui::TITLE_BAR_HEIGHT as u32 + 2),
    };
    for damage in first.damage.iter().take(first.damage_count as usize) {
        if damage.width == 0
            || damage.height == 0
            || damage.x < 0
            || damage.y < 0
            || damage
                .x
                .checked_add(damage.width as i32)
                .is_none_or(|right| right > client_bounds.width as i32)
            || damage
                .y
                .checked_add(damage.height as i32)
                .is_none_or(|bottom| bottom > client_bounds.height as i32)
        {
            return Err(Status::Invalid);
        }
    }
    validate_client_bounds(snapshot, window.rect)?;
    let start = index * gui::COMMAND_BYTES;
    replace_display_list(
        &mut display_lists[start..start + gui::COMMAND_BYTES],
        snapshot,
    )?;
    clients[index].display_bytes = bytes;
    clients[index].sequence = first.sequence;
    let damage_count = first.damage_count as usize;
    clients[index].damage_count = damage_count;
    let content_x = window.rect.x + 2;
    let content_y = window.rect.y + microsystem_gui::TITLE_BAR_HEIGHT;
    for (target, damage) in clients[index]
        .damage
        .iter_mut()
        .zip(first.damage.iter())
        .take(damage_count)
    {
        *target = gui::Rect {
            x: content_x.saturating_add(damage.x),
            y: content_y.saturating_add(damage.y),
            width: damage.width,
            height: damage.height,
        };
    }
    Ok(())
}

fn validate_client_bounds(bytes: &[u8], window: gui::Rect) -> Result<(), Status> {
    let client_width = window.width.saturating_sub(4);
    let client_height = window
        .height
        .saturating_sub(microsystem_gui::TITLE_BAR_HEIGHT as u32 + 2);
    let mut offset = 0usize;
    while offset < bytes.len() {
        let command = unsafe {
            core::ptr::read_unaligned(bytes.as_ptr().add(offset).cast::<gui::DrawCommand>())
        };
        let right = command.rect.x.checked_add(command.rect.width as i32);
        let bottom = command.rect.y.checked_add(command.rect.height as i32);
        if command.rect.x < 0
            || command.rect.y < 0
            || right.is_none_or(|value| value > client_width as i32)
            || bottom.is_none_or(|value| value > client_height as i32)
        {
            return Err(Status::Invalid);
        }
        offset += command.header.bytes as usize;
    }
    Ok(())
}

fn display_hash(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325u64, |hash, byte| {
        (hash ^ *byte as u64).wrapping_mul(0x100_0000_01b3)
    })
}

fn dynamic_present_header(index: usize) -> gui::PresentHeaderV1 {
    unsafe { core::ptr::read_volatile(dynamic_command_va(index) as *const gui::PresentHeaderV1) }
}

fn dynamic_command_va(index: usize) -> u64 {
    DYNAMIC_SHARED_VA + index as u64 * DYNAMIC_SHARED_STRIDE
}

fn dynamic_event_va(index: usize) -> u64 {
    dynamic_command_va(index) + gui::COMMAND_BYTES as u64
}

fn acknowledge_event(index: usize, tail: u32) -> Status {
    let header = unsafe { &mut *(dynamic_event_va(index) as *mut gui::EventRingHeaderV1) };
    if header.magic != gui::EVENT_MAGIC
        || tail > header.head
        || header.head.saturating_sub(tail) > header.capacity as u32
    {
        return Status::Invalid;
    }
    header.tail = tail;
    Status::Ok
}

fn retry_pending_event(
    index: usize,
    clients: &mut [DynamicClient; gui::MAX_DYNAMIC_CLIENTS],
) -> Result<(), Status> {
    let Some(event) = clients[index].pending_events[0] else {
        return Ok(());
    };
    if write_event(index, clients, event).is_ok() {
        clients[index].pending_events.rotate_left(1);
        clients[index].pending_events[PENDING_EVENT_CAPACITY - 1] = None;
        clients[index].pending_event_count -= 1;
    }
    Ok(())
}

fn push_event(
    index: usize,
    clients: &mut [DynamicClient; gui::MAX_DYNAMIC_CLIENTS],
    event: gui::Event,
) -> Result<(), Status> {
    match write_event(index, clients, event) {
        Err(Status::Busy) => {
            let client = &mut clients[index];
            if client.pending_event_count == client.pending_events.len() {
                return Err(Status::Busy);
            }
            client.pending_events[client.pending_event_count] = Some(event);
            client.pending_event_count += 1;
            Err(Status::Busy)
        }
        result => result,
    }
}

fn write_event(
    index: usize,
    clients: &mut [DynamicClient; gui::MAX_DYNAMIC_CLIENTS],
    event: gui::Event,
) -> Result<(), Status> {
    if !clients[index].active {
        return Err(Status::NotFound);
    }
    let header = unsafe { &mut *(dynamic_event_va(index) as *mut gui::EventRingHeaderV1) };
    if header.magic != gui::EVENT_MAGIC
        || header.version != gui::VERSION
        || header.capacity as usize != gui::EVENT_CAPACITY
        || header.head.saturating_sub(header.tail) > header.capacity as u32
    {
        return Err(Status::Invalid);
    }
    if header.head.saturating_sub(header.tail) == header.capacity as u32 {
        if event.kind == gui::EventKind::PointerMove as u16 {
            let last = header.head.saturating_sub(1) % header.capacity as u32;
            let pointer = (dynamic_event_va(index) as usize
                + gui::EVENT_RING_HEADER_BYTES
                + last as usize * core::mem::size_of::<gui::Event>())
                as *mut gui::Event;
            let current = unsafe { core::ptr::read_volatile(pointer) };
            if current.kind == gui::EventKind::PointerMove as u16 {
                unsafe { core::ptr::write_volatile(pointer, event) };
                header.dropped_pointer_moves = header.dropped_pointer_moves.saturating_add(1);
                return Ok(());
            }
            header.dropped_pointer_moves = header.dropped_pointer_moves.saturating_add(1);
            return Ok(());
        }
        return Err(Status::Busy);
    }
    let slot = header.head % header.capacity as u32;
    let pointer = (dynamic_event_va(index) as usize
        + gui::EVENT_RING_HEADER_BYTES
        + slot as usize * core::mem::size_of::<gui::Event>()) as *mut gui::Event;
    unsafe {
        core::ptr::write_volatile(pointer, event);
        microsystem_user_rt::fence();
    }
    header.head = header.head.wrapping_add(1);
    microsystem_user_rt::notification_signal(clients[index].notification, script::EVENT_GUI)
}

fn dynamic_index_for_window(desktop: &Desktop, window: Option<u32>) -> Option<usize> {
    let client = desktop.window(window?)?.client;
    (client >= 4)
        .then_some(client as usize - 4)
        .filter(|index| *index < gui::MAX_DYNAMIC_CLIENTS)
}

fn request_close(
    window: u32,
    desktop: &Desktop,
    clients: &mut [DynamicClient; gui::MAX_DYNAMIC_CLIENTS],
) -> bool {
    let Some(index) = dynamic_index_for_window(desktop, Some(window)) else {
        return false;
    };
    let header = unsafe { &mut *(dynamic_event_va(index) as *mut gui::EventRingHeaderV1) };
    header.flags |= gui::EVENT_RING_FLAG_CLOSE_REQUESTED;
    let _ = push_event(
        index,
        clients,
        gui::Event {
            kind: gui::EventKind::CloseRequested as u16,
            window,
            ..gui::Event::default()
        },
    );
    true
}

fn activate_application(
    application: gui::Application,
    desktop: &mut Desktop,
    clients: &mut [DynamicClient; gui::MAX_DYNAMIC_CLIENTS],
) -> bool {
    let client = match application {
        gui::Application::Terminal => Some(1),
        gui::Application::Files => Some(2),
        gui::Application::Monitor => Some(3),
        gui::Application::Reader | gui::Application::Editor => None,
    };
    let dynamic_active = clients
        .iter()
        .any(|client| client.active && client.application == application as u16);
    let window = client
        .and_then(|client| desktop.window_for_client(client).map(|window| window.id))
        .or_else(|| {
            clients
                .iter()
                .find(|client| client.active && client.application == application as u16)
                .and_then(|client| (client.window != 0).then_some(client.window))
        });
    if let Some(window) = window {
        let _ = desktop.action(window, gui::WindowAction::Restore);
        let _ = desktop.focus(window);
        push_configure(window, desktop, clients);
        push_expose(window, desktop, clients);
        return true;
    }
    if !matches!(
        application,
        gui::Application::Reader | gui::Application::Editor
    ) {
        return false;
    }
    if dynamic_active {
        return true;
    }
    let Some(pending_index) = desktop_launch_index(application) else {
        return false;
    };
    let now = microsystem_user_rt::clock_now().unwrap_or(0);
    if DESKTOP_LAUNCH_DEADLINES[pending_index].load(Ordering::Acquire) > now {
        return true;
    }
    let mut request = Message::new(protocol::GUI, gui::Operation::LaunchApplication as u16);
    request.words[0] = application as u64;
    let mut reply = Message::new(protocol::GUI, 0);
    if microsystem_user_rt::ipc_call(boot_cap::GUI_LAUNCH_ENDPOINT, &request, &mut reply, 0)
        .is_err()
        || reply.words[5] as i64 != Status::Ok as i64
    {
        return false;
    }
    DESKTOP_LAUNCH_DEADLINES[pending_index]
        .store(now.saturating_add(5_000_000_000), Ordering::Release);
    let _ = microsystem_user_rt::debug_write_u64(
        b"[gui] desktop icon clicked application=",
        application as u64,
        b" launch-requested=true\n",
    );
    true
}

fn desktop_launch_index(application: gui::Application) -> Option<usize> {
    match application {
        gui::Application::Reader => Some(0),
        gui::Application::Editor => Some(1),
        _ => None,
    }
}

fn clear_desktop_launch_pending(application: u64) {
    let Some(application) = gui::Application::from_u64(application) else {
        return;
    };
    if let Some(index) = desktop_launch_index(application) {
        DESKTOP_LAUNCH_DEADLINES[index].store(0, Ordering::Release);
    }
}

fn taskbar_window_at(desktop: &Desktop, x: i32) -> Option<u32> {
    let count = desktop.z_order().count().max(1);
    let (start, width) = taskbar_metrics(count);
    desktop
        .z_order()
        .enumerate()
        .find(|(index, _)| {
            let left = start + *index as i32 * (width as i32 + 6);
            x >= left && x < left + width as i32
        })
        .map(|(_, window)| window.id)
}

fn taskbar_metrics(count: usize) -> (i32, u32) {
    let count = count.max(1) as i32;
    let gap = 6;
    let width = ((gui::WIDTH as i32 - 40 - gap * (count - 1)) / count).clamp(58, 118);
    let total = width * count + gap * (count - 1);
    ((gui::WIDTH as i32 - total) / 2, width as u32)
}

fn window_control_at(rect: gui::Rect, x: i32) -> Option<gui::WindowAction> {
    let offset = x - (rect.x + 10);
    if !(0..48).contains(&offset) {
        return None;
    }
    match offset / 16 {
        0 => Some(gui::WindowAction::Close),
        1 => Some(gui::WindowAction::Minimize),
        2 => Some(gui::WindowAction::Maximize),
        _ => None,
    }
}

struct UnicodeInput {
    active: bool,
    value: u32,
    digits: u8,
}

impl UnicodeInput {
    const fn new() -> Self {
        Self {
            active: false,
            value: 0,
            digits: 0,
        }
    }

    fn reset(&mut self) {
        self.active = false;
        self.value = 0;
        self.digits = 0;
    }
}

#[allow(clippy::too_many_arguments)]
fn route_dynamic_input(
    input: gui::InputEvent,
    pointer: (i32, i32),
    control: bool,
    shift: bool,
    alt: bool,
    unicode: &mut UnicodeInput,
    previous_focus: Option<u32>,
    desktop: &Desktop,
    clients: &mut [DynamicClient; gui::MAX_DYNAMIC_CLIENTS],
) {
    let current_focus = desktop.focused();
    if previous_focus != current_focus {
        if let Some(index) = dynamic_index_for_window(desktop, previous_focus) {
            let _ = push_event(
                index,
                clients,
                event_word(gui::EventKind::Focus, previous_focus.unwrap_or(0), 0),
            );
        }
        if let Some(index) = dynamic_index_for_window(desktop, current_focus) {
            let _ = push_event(
                index,
                clients,
                event_word(gui::EventKind::Focus, current_focus.unwrap_or(0), 1),
            );
        }
    }
    let Some(index) = dynamic_index_for_window(desktop, current_focus) else {
        unicode.reset();
        return;
    };
    let window = current_focus.unwrap_or(0);
    match input.event_type {
        1 => {
            if input.code >= 0x100 {
                // Pointer buttons share the Linux input event type with keys.
            } else {
                if control && shift && input.code == 22 && input.value == 1 {
                    unicode.active = true;
                    unicode.value = 0;
                    unicode.digits = 0;
                    let _ =
                        microsystem_user_rt::debug_write(b"[gui] unicode text input active=true\n");
                    return;
                }
                if unicode.active {
                    if matches!(input.code, 29 | 42 | 54 | 97) || input.value != 1 {
                        return;
                    }
                    if input.code == 1 {
                        unicode.reset();
                        return;
                    }
                    if input.code == 14 {
                        unicode.value >>= 4;
                        unicode.digits = unicode.digits.saturating_sub(1);
                        return;
                    }
                    if matches!(input.code, 28 | 57) && unicode.digits != 0 {
                        if char::from_u32(unicode.value).is_some() {
                            let delivered =
                                push_text_event(index, clients, window, unicode.value).is_ok();
                            let _ = microsystem_user_rt::debug_write_u64(
                                b"[gui] unicode text input codepoint=",
                                unicode.value as u64,
                                if delivered {
                                    b" delivered=true\n"
                                } else {
                                    b" delivered=false\n"
                                },
                            );
                        }
                        unicode.reset();
                        return;
                    }
                    if let Some(nibble) = keycode_to_hex(input.code) {
                        if unicode.digits < 6 {
                            unicode.value = (unicode.value << 4) | nibble;
                            unicode.digits += 1;
                            let _ = microsystem_user_rt::debug_write_u64(
                                b"[gui] unicode text input accumulated=",
                                unicode.value as u64,
                                b"\n",
                            );
                        }
                    }
                    return;
                }
                let mut event = gui::Event {
                    kind: gui::EventKind::Key as u16,
                    window,
                    ..gui::Event::default()
                };
                event.words[0] = input.code as u32;
                event.words[1] = input.value.max(0) as u32;
                event.words[2] = (control as u32) | ((shift as u32) << 1) | ((alt as u32) << 2);
                let _ = push_event(index, clients, event);
                if input.value != 1 || alt {
                    return;
                }
                if let Some(byte) = keycode_to_ascii(input.code, shift) {
                    let _ = push_text_event(index, clients, window, byte as u32);
                }
            }
        }
        3 => {
            let mut event = gui::Event {
                kind: gui::EventKind::PointerMove as u16,
                window,
                ..gui::Event::default()
            };
            if let Some(target) = desktop.window(window) {
                event.words[0] = pointer.0.saturating_sub(target.rect.x).max(0) as u32;
                event.words[1] = pointer
                    .1
                    .saturating_sub(target.rect.y + microsystem_gui::TITLE_BAR_HEIGHT)
                    .max(0) as u32;
            }
            let _ = push_event(index, clients, event);
        }
        2 => {
            let mut event = gui::Event {
                kind: gui::EventKind::PointerWheel as u16,
                window,
                ..gui::Event::default()
            };
            event.words[0] = input.value as u32;
            if let Some(target) = desktop.window(window) {
                event.words[1] = pointer.0.saturating_sub(target.rect.x).max(0) as u32;
                event.words[2] = pointer
                    .1
                    .saturating_sub(target.rect.y + microsystem_gui::TITLE_BAR_HEIGHT)
                    .max(0) as u32;
            }
            let _ = push_event(index, clients, event);
        }
        _ => {}
    }
    if input.event_type == 1 && input.code == 0x110 {
        let mut event = gui::Event {
            kind: gui::EventKind::PointerButton as u16,
            window,
            ..gui::Event::default()
        };
        event.words[0] = input.code as u32;
        event.words[1] = input.value.max(0) as u32;
        if let Some(target) = desktop.window(window) {
            event.words[2] = pointer.0.saturating_sub(target.rect.x).max(0) as u32;
            event.words[3] = pointer
                .1
                .saturating_sub(target.rect.y + microsystem_gui::TITLE_BAR_HEIGHT)
                .max(0) as u32;
        }
        let _ = push_event(index, clients, event);
    }
}

fn push_configure(
    window: u32,
    desktop: &Desktop,
    clients: &mut [DynamicClient; gui::MAX_DYNAMIC_CLIENTS],
) {
    let Some(index) = dynamic_index_for_window(desktop, Some(window)) else {
        return;
    };
    let Some(value) = desktop.window(window) else {
        return;
    };
    let mut event = gui::Event {
        kind: gui::EventKind::Configure as u16,
        window,
        ..gui::Event::default()
    };
    event.words[0] = value.rect.x.max(0) as u32;
    event.words[1] = value.rect.y.max(0) as u32;
    event.words[2] = value.rect.width;
    event.words[3] = value.rect.height;
    let _ = push_event(index, clients, event);
}

fn push_expose(
    window: u32,
    desktop: &Desktop,
    clients: &mut [DynamicClient; gui::MAX_DYNAMIC_CLIENTS],
) {
    let Some(index) = dynamic_index_for_window(desktop, Some(window)) else {
        return;
    };
    let _ = push_event(
        index,
        clients,
        event_word(gui::EventKind::Expose, window, 1),
    );
}

fn event_word(kind: gui::EventKind, window: u32, value: u32) -> gui::Event {
    let mut event = gui::Event {
        kind: kind as u16,
        window,
        ..gui::Event::default()
    };
    event.words[0] = value;
    event
}

fn push_text_event(
    index: usize,
    clients: &mut [DynamicClient; gui::MAX_DYNAMIC_CLIENTS],
    window: u32,
    codepoint: u32,
) -> Result<(), Status> {
    push_event(
        index,
        clients,
        event_word(gui::EventKind::TextInput, window, codepoint),
    )
}

fn keycode_to_hex(code: u16) -> Option<u32> {
    match code {
        2..=11 => Some(((code - 1) % 10) as u32),
        30 => Some(10),
        48 => Some(11),
        46 => Some(12),
        32 => Some(13),
        18 => Some(14),
        33 => Some(15),
        _ => None,
    }
}

fn render(
    desktop: &Desktop,
    terminal: &TerminalView,
    dynamic_clients: &[DynamicClient; gui::MAX_DYNAMIC_CLIENTS],
    display_lists: &[u8],
    font: &mut FontCache,
    framebuffer: &mut [u32],
    damage: &[gui::Rect],
) {
    set_render_damage(damage);
    draw_desktop_background(framebuffer);
    draw_desktop_icons(framebuffer);
    for window in desktop.z_order() {
        if !matches!(window.state, WindowState::Normal | WindowState::Maximized) {
            continue;
        }
        let paint_bounds = gui::Rect {
            x: window.rect.x,
            y: window.rect.y,
            width: window.rect.width.saturating_add(5),
            height: window.rect.height.saturating_add(6),
        };
        if !rect_is_dirty(paint_bounds) {
            continue;
        }
        let active = desktop.focused() == Some(window.id);
        fill_rect(
            framebuffer,
            gui::Rect {
                x: window.rect.x + 5,
                y: window.rect.y + 6,
                width: window.rect.width,
                height: window.rect.height,
            },
            SHADOW,
        );
        fill_rect(framebuffer, window.rect, BORDER);
        stroke_rect(
            framebuffer,
            window.rect,
            if active { BORDER_ACTIVE } else { BORDER },
            1,
        );
        fill_rect(
            framebuffer,
            gui::Rect {
                x: window.rect.x + 1,
                y: window.rect.y + 1,
                width: window.rect.width.saturating_sub(2),
                height: (microsystem_gui::TITLE_BAR_HEIGHT - 1) as u32,
            },
            if active { TITLE_ACTIVE } else { TITLE_IDLE },
        );
        fill_rect(
            framebuffer,
            gui::Rect {
                x: window.rect.x + 2,
                y: window.rect.y + microsystem_gui::TITLE_BAR_HEIGHT,
                width: window.rect.width.saturating_sub(4),
                height: window
                    .rect
                    .height
                    .saturating_sub(microsystem_gui::TITLE_BAR_HEIGHT as u32 + 2),
            },
            CLIENT,
        );
        fill_round_rect(framebuffer, client_rect(window.rect), CLIENT, 7);
        fill_rect(
            framebuffer,
            gui::Rect {
                x: window.rect.x + 2,
                y: window.rect.y + microsystem_gui::TITLE_BAR_HEIGHT,
                width: window.rect.width.saturating_sub(4),
                height: 1,
            },
            CHROME_DIVIDER,
        );
        let title_bytes = (window.rect.width.saturating_sub(132) / 6) as usize;
        draw_text(
            framebuffer,
            window.rect.x + 64,
            window.rect.y + 10,
            &window.title[..(window.title_len as usize).min(title_bytes)],
            if active { TEXT } else { MUTED_TEXT },
        );
        draw_window_controls(framebuffer, window.rect, active);
        if window.client == 1 {
            draw_terminal(framebuffer, window.rect, terminal);
        } else if window.client == 2 {
            draw_files(framebuffer, window.rect);
        } else if window.client == 3 {
            draw_monitor(framebuffer, window.rect);
        } else if window.client >= 4 {
            let index = window.client as usize - 4;
            if let Some(client) = dynamic_clients.get(index).filter(|client| client.active) {
                let start = index * gui::COMMAND_BYTES;
                let end = start
                    .saturating_add(client.display_bytes)
                    .min(display_lists.len());
                render_display_list(framebuffer, window.rect, &display_lists[start..end], font);
            }
        }
        draw_resize_grip(framebuffer, window.rect, active);
    }
    let taskbar_bounds = gui::Rect {
        x: 0,
        y: gui::HEIGHT as i32 - microsystem_gui::TASKBAR_HEIGHT,
        width: gui::WIDTH,
        height: microsystem_gui::TASKBAR_HEIGHT as u32,
    };
    if !rect_is_dirty(taskbar_bounds) {
        return;
    }
    let visible_windows = desktop
        .z_order()
        .filter(|window| window.id != 0)
        .count()
        .max(1);
    draw_taskbar(framebuffer, visible_windows);
    let (task_start, task_width) = taskbar_metrics(visible_windows);
    let mut task_index = 0i32;
    for window in desktop.z_order() {
        if window.id != 0 {
            let x = task_start + task_index * (task_width as i32 + 6);
            fill_round_rect(
                framebuffer,
                gui::Rect {
                    x,
                    y: gui::HEIGHT as i32 - microsystem_gui::TASKBAR_HEIGHT + 6,
                    width: task_width,
                    height: 24,
                },
                if desktop.focused() == Some(window.id) {
                    ACCENT_TINT
                } else {
                    TASKBAR_BUTTON
                },
                7,
            );
            fill_rect(
                framebuffer,
                gui::Rect {
                    x,
                    y: gui::HEIGHT as i32 - 5,
                    width: task_width,
                    height: 2,
                },
                if desktop.focused() == Some(window.id) {
                    ACCENT
                } else {
                    CHROME_DIVIDER
                },
            );
            let task_title_bytes = (task_width.saturating_sub(16) / 6) as usize;
            draw_text(
                framebuffer,
                x + 8,
                gui::HEIGHT as i32 - microsystem_gui::TASKBAR_HEIGHT + 14,
                &window.title[..(window.title_len as usize).min(task_title_bytes)],
                if desktop.focused() == Some(window.id) {
                    TEXT
                } else {
                    MUTED_TEXT
                },
            );
            task_index += 1;
        }
    }
}

fn draw_desktop_background(framebuffer: &mut [u32]) {
    clear(framebuffer, BACKGROUND);
    let band_height =
        (gui::HEIGHT - microsystem_gui::TASKBAR_HEIGHT as u32) / BACKGROUND_BANDS.len() as u32;
    for (index, color) in BACKGROUND_BANDS.into_iter().enumerate() {
        fill_rect(
            framebuffer,
            gui::Rect {
                x: 0,
                y: index as i32 * band_height as i32,
                width: gui::WIDTH,
                height: band_height + u32::from(index + 1 == BACKGROUND_BANDS.len()),
            },
            color,
        );
    }
    fill_rect(
        framebuffer,
        gui::Rect {
            x: 0,
            y: 120,
            width: gui::WIDTH,
            height: 2,
        },
        0x00d8_e8f2,
    );
    fill_rect(
        framebuffer,
        gui::Rect {
            x: 0,
            y: 122,
            width: gui::WIDTH,
            height: 1,
        },
        0x00b5_cde0,
    );
}

fn draw_taskbar(framebuffer: &mut [u32], count: usize) {
    let top = gui::HEIGHT as i32 - microsystem_gui::TASKBAR_HEIGHT;
    fill_rect(
        framebuffer,
        gui::Rect {
            x: 0,
            y: top,
            width: gui::WIDTH,
            height: microsystem_gui::TASKBAR_HEIGHT as u32,
        },
        TASKBAR,
    );
    fill_rect(
        framebuffer,
        gui::Rect {
            x: 0,
            y: top,
            width: gui::WIDTH,
            height: 1,
        },
        0x00c6_d2de,
    );
    let (start, width) = taskbar_metrics(count);
    let dock_width = width as i32 * count as i32 + 6 * (count as i32 - 1) + 16;
    fill_round_rect(
        framebuffer,
        gui::Rect {
            x: start - 8,
            y: top + 3,
            width: dock_width.max(0) as u32,
            height: 30,
        },
        0x00f4_f7fa,
        10,
    );
}

fn draw_window_controls(framebuffer: &mut [u32], rect: gui::Rect, active: bool) {
    for (offset, color) in [
        (0, CONTROL_CLOSE),
        (16, CONTROL_MINIMIZE),
        (32, CONTROL_MAXIMIZE),
    ] {
        let button = gui::Rect {
            x: rect.x + 10 + offset,
            y: rect.y + 8,
            width: 12,
            height: 12,
        };
        fill_round_rect(
            framebuffer,
            button,
            if active { color } else { CONTROL_INACTIVE },
            6,
        );
    }
}

fn draw_resize_grip(framebuffer: &mut [u32], rect: gui::Rect, active: bool) {
    let color = if active { BORDER_ACTIVE } else { BORDER };
    let right = rect.x + rect.width as i32;
    let bottom = rect.y + rect.height as i32;
    for offset in [5, 9, 13] {
        fill_rect(
            framebuffer,
            gui::Rect {
                x: right - offset,
                y: bottom - 4,
                width: 2,
                height: 2,
            },
            color,
        );
        fill_rect(
            framebuffer,
            gui::Rect {
                x: right - 4,
                y: bottom - offset,
                width: 2,
                height: 2,
            },
            color,
        );
    }
}

fn draw_files(framebuffer: &mut [u32], window: gui::Rect) {
    let content = client_rect(window);
    if content.width < 240 || content.height < 180 {
        draw_text(framebuffer, content.x + 12, content.y + 16, b"FILES", TEXT);
        draw_text(
            framebuffer,
            content.x + 12,
            content.y + 36,
            b"RESIZE FOR DETAILS",
            MUTED_TEXT,
        );
        return;
    }
    let sidebar = gui::Rect {
        x: content.x + 10,
        y: content.y + 12,
        width: 92,
        height: content.height.saturating_sub(24),
    };
    fill_rect(framebuffer, sidebar, PANEL);
    draw_text(
        framebuffer,
        sidebar.x + 10,
        sidebar.y + 12,
        b"PLACES",
        MUTED_TEXT,
    );
    for (index, label) in [b"HOME".as_slice(), b"DOCUMENTS", b"SYSTEM"]
        .into_iter()
        .enumerate()
    {
        let y = sidebar.y + 38 + index as i32 * 30;
        if index == 0 {
            fill_rect(
                framebuffer,
                gui::Rect {
                    x: sidebar.x + 6,
                    y: y - 7,
                    width: 80,
                    height: 22,
                },
                ACCENT_TINT,
            );
        }
        draw_text(
            framebuffer,
            sidebar.x + 12,
            y,
            label,
            if index == 0 { TEXT } else { MUTED_TEXT },
        );
    }
    let main_x = sidebar.x + sidebar.width as i32 + 14;
    draw_text(framebuffer, main_x, content.y + 16, b"HOME", TEXT);
    fill_rect(
        framebuffer,
        gui::Rect {
            x: main_x,
            y: content.y + 34,
            width: content.width.saturating_sub(126),
            height: 1,
        },
        BORDER,
    );
    for (index, (name, color)) in [
        (b"DOCUMENTS".as_slice(), 0x0052_a8ff),
        (b"SCRIPTS".as_slice(), 0x00af_52de),
        (b"SYSTEM".as_slice(), 0x0034_c759),
    ]
    .into_iter()
    .enumerate()
    {
        let y = content.y + 56 + index as i32 * 42;
        fill_rect(
            framebuffer,
            gui::Rect {
                x: main_x,
                y,
                width: 28,
                height: 22,
            },
            color,
        );
        fill_rect(
            framebuffer,
            gui::Rect {
                x: main_x + 4,
                y: y - 4,
                width: 12,
                height: 5,
            },
            color,
        );
        draw_text(framebuffer, main_x + 40, y + 8, name, TEXT);
    }
}

fn draw_monitor(framebuffer: &mut [u32], window: gui::Rect) {
    let content = client_rect(window);
    draw_text(
        framebuffer,
        content.x + 12,
        content.y + 16,
        b"SYSTEM OVERVIEW",
        TEXT,
    );
    for (index, (label, value, color)) in [
        (b"CPU".as_slice(), 58u32, 0x0000_7aff),
        (b"MEMORY".as_slice(), 72u32, 0x00af_52de),
        (b"STORAGE".as_slice(), 41u32, 0x0034_c759),
    ]
    .into_iter()
    .enumerate()
    {
        let y = content.y + 48 + index as i32 * 66;
        if y + 50 > content.y + content.height as i32 - 10 {
            break;
        }
        fill_rect(
            framebuffer,
            gui::Rect {
                x: content.x + 12,
                y,
                width: content.width.saturating_sub(24),
                height: 50,
            },
            PANEL,
        );
        draw_text(framebuffer, content.x + 22, y + 10, label, MUTED_TEXT);
        let track_width = content.width.saturating_sub(44);
        fill_rect(
            framebuffer,
            gui::Rect {
                x: content.x + 22,
                y: y + 30,
                width: track_width,
                height: 7,
            },
            0x00d9_e1e9,
        );
        fill_rect(
            framebuffer,
            gui::Rect {
                x: content.x + 22,
                y: y + 30,
                width: track_width.saturating_mul(value) / 100,
                height: 7,
            },
            color,
        );
    }
}

fn client_rect(window: gui::Rect) -> gui::Rect {
    gui::Rect {
        x: window.x + 2,
        y: window.y + microsystem_gui::TITLE_BAR_HEIGHT,
        width: window.width.saturating_sub(4),
        height: window
            .height
            .saturating_sub(microsystem_gui::TITLE_BAR_HEIGHT as u32 + 2),
    }
}

fn application_color(application: gui::Application) -> u32 {
    match application {
        gui::Application::Terminal => 0x0034_3a40,
        gui::Application::Files => 0x0052_a8ff,
        gui::Application::Monitor => 0x0034_c759,
        gui::Application::Reader => 0x00af_52de,
        gui::Application::Editor => 0x00ff_375f,
    }
}

fn draw_application_symbol(framebuffer: &mut [u32], application: gui::Application, x: i32, y: i32) {
    match application {
        gui::Application::Terminal => {
            fill_rect(
                framebuffer,
                gui::Rect {
                    x: x + 6,
                    y: y + 7,
                    width: 32,
                    height: 24,
                },
                0x0029_2f36,
            );
            draw_text(framebuffer, x + 11, y + 16, b">_", 0x00f6_f8fb);
        }
        gui::Application::Files => {
            fill_rect(
                framebuffer,
                gui::Rect {
                    x: x + 7,
                    y: y + 12,
                    width: 30,
                    height: 19,
                },
                0x0052_a8ff,
            );
            fill_rect(
                framebuffer,
                gui::Rect {
                    x: x + 10,
                    y: y + 8,
                    width: 13,
                    height: 6,
                },
                0x0052_a8ff,
            );
        }
        gui::Application::Monitor => {
            for (offset, height) in [(8, 10), (17, 18), (26, 25)] {
                fill_rect(
                    framebuffer,
                    gui::Rect {
                        x: x + offset,
                        y: y + 32 - height,
                        width: 6,
                        height: height as u32,
                    },
                    0x0034_c759,
                );
            }
        }
        gui::Application::Reader => {
            fill_rect(
                framebuffer,
                gui::Rect {
                    x: x + 7,
                    y: y + 7,
                    width: 14,
                    height: 25,
                },
                0x00f4_effc,
            );
            fill_rect(
                framebuffer,
                gui::Rect {
                    x: x + 23,
                    y: y + 7,
                    width: 14,
                    height: 25,
                },
                0x00f4_effc,
            );
            fill_rect(
                framebuffer,
                gui::Rect {
                    x: x + 21,
                    y: y + 8,
                    width: 2,
                    height: 24,
                },
                0x005c_438f,
            );
        }
        gui::Application::Editor => {
            fill_rect(
                framebuffer,
                gui::Rect {
                    x: x + 10,
                    y: y + 5,
                    width: 24,
                    height: 29,
                },
                0x00f4_effc,
            );
            for row in 0..3 {
                fill_rect(
                    framebuffer,
                    gui::Rect {
                        x: x + 15,
                        y: y + 13 + row * 6,
                        width: 14,
                        height: 2,
                    },
                    0x00b8_5264,
                );
            }
        }
    }
}

fn draw_desktop_icons(framebuffer: &mut [u32]) {
    for icon in microsystem_gui::DESKTOP_ICONS {
        if !rect_is_dirty(icon.rect) {
            continue;
        }
        fill_round_rect(
            framebuffer,
            gui::Rect {
                x: icon.rect.x + 2,
                y: icon.rect.y + 3,
                width: icon.rect.width.saturating_sub(2),
                height: icon.rect.height.saturating_sub(2),
            },
            SHADOW,
            9,
        );
        fill_round_rect(framebuffer, icon.rect, 0x00f6_f9fc, 9);
        fill_round_rect(
            framebuffer,
            gui::Rect {
                x: icon.rect.x + 10,
                y: icon.rect.y + 6,
                width: 44,
                height: 38,
            },
            application_color(icon.application),
            8,
        );
        draw_application_symbol(
            framebuffer,
            icon.application,
            icon.rect.x + 10,
            icon.rect.y + 6,
        );
        let label_width = icon.label.len().saturating_mul(6) as i32;
        draw_text(
            framebuffer,
            icon.rect.x + (icon.rect.width as i32 - label_width).max(0) / 2,
            icon.rect.y + 52,
            icon.label,
            TEXT,
        );
    }
}

fn render_display_list(
    framebuffer: &mut [u32],
    window: gui::Rect,
    bytes: &[u8],
    font: &mut FontCache,
) {
    let content = gui::Rect {
        x: window.x + 2,
        y: window.y + microsystem_gui::TITLE_BAR_HEIGHT,
        width: window.width.saturating_sub(4),
        height: window
            .height
            .saturating_sub(microsystem_gui::TITLE_BAR_HEIGHT as u32 + 2),
    };
    let mut clip = content;
    let mut offset = 0usize;
    while offset < bytes.len() {
        let command = unsafe {
            core::ptr::read_unaligned(bytes.as_ptr().add(offset).cast::<gui::DrawCommand>())
        };
        let payload_start = offset + core::mem::size_of::<gui::DrawCommand>();
        let next = offset + command.header.bytes as usize;
        if next > bytes.len() || payload_start > next {
            return;
        }
        let relative = gui::Rect {
            x: content.x + command.rect.x,
            y: content.y + command.rect.y,
            width: command.rect.width,
            height: command.rect.height,
        };
        if command.header.kind != gui::CommandKind::SetClip as u16
            && intersect_rect(relative, clip).is_none_or(|rect| !rect_is_dirty(rect))
        {
            offset = next;
            continue;
        }
        match command.header.kind {
            value
                if value == gui::CommandKind::Clear as u16
                    || value == gui::CommandKind::FillRect as u16 =>
            {
                if let Some(rect) = intersect_rect(relative, clip) {
                    fill_rect(framebuffer, rect, command.color);
                }
            }
            value if value == gui::CommandKind::StrokeRect as u16 => {
                if let Some(rect) = intersect_rect(relative, clip) {
                    stroke_rect(
                        framebuffer,
                        rect,
                        command.color,
                        command.argument.clamp(1, 8),
                    );
                }
            }
            value if value == gui::CommandKind::Text as u16 => {
                let payload = &bytes[payload_start..next];
                if let Ok(text) = core::str::from_utf8(payload) {
                    draw_utf8_text(
                        framebuffer,
                        relative.x,
                        relative.y,
                        text,
                        command.color,
                        clip,
                        font,
                    );
                }
            }
            value if value == gui::CommandKind::Icon as u16 => {
                if let Some(rect) = intersect_rect(relative, clip) {
                    stroke_rect(framebuffer, rect, command.color, 2);
                    let inset = gui::Rect {
                        x: rect.x + 4,
                        y: rect.y + 4,
                        width: rect.width.saturating_sub(8),
                        height: rect.height.saturating_sub(8),
                    };
                    fill_rect(framebuffer, inset, command.color);
                }
            }
            value if value == gui::CommandKind::SetClip as u16 => {
                clip = intersect_rect(relative, content).unwrap_or(gui::Rect::default());
            }
            _ => return,
        }
        offset = next;
    }
}

fn intersect_rect(left: gui::Rect, right: gui::Rect) -> Option<gui::Rect> {
    let x1 = left.x.max(right.x);
    let y1 = left.y.max(right.y);
    let x2 = left
        .x
        .saturating_add(left.width as i32)
        .min(right.x.saturating_add(right.width as i32));
    let y2 = left
        .y
        .saturating_add(left.height as i32)
        .min(right.y.saturating_add(right.height as i32));
    (x2 > x1 && y2 > y1).then_some(gui::Rect {
        x: x1,
        y: y1,
        width: (x2 - x1) as u32,
        height: (y2 - y1) as u32,
    })
}

fn union_rect(left: gui::Rect, right: gui::Rect) -> gui::Rect {
    let x = left.x.min(right.x);
    let y = left.y.min(right.y);
    let right_edge = left
        .x
        .saturating_add(left.width as i32)
        .max(right.x.saturating_add(right.width as i32));
    let bottom = left
        .y
        .saturating_add(left.height as i32)
        .max(right.y.saturating_add(right.height as i32));
    gui::Rect {
        x,
        y,
        width: right_edge.saturating_sub(x) as u32,
        height: bottom.saturating_sub(y) as u32,
    }
}

fn stroke_rect(framebuffer: &mut [u32], rect: gui::Rect, color: u32, thickness: u32) {
    let thickness = thickness.min(rect.width).min(rect.height);
    fill_rect(
        framebuffer,
        gui::Rect {
            height: thickness,
            ..rect
        },
        color,
    );
    fill_rect(
        framebuffer,
        gui::Rect {
            y: rect.y + rect.height as i32 - thickness as i32,
            height: thickness,
            ..rect
        },
        color,
    );
    fill_rect(
        framebuffer,
        gui::Rect {
            width: thickness,
            ..rect
        },
        color,
    );
    fill_rect(
        framebuffer,
        gui::Rect {
            x: rect.x + rect.width as i32 - thickness as i32,
            width: thickness,
            ..rect
        },
        color,
    );
}

fn draw_utf8_text(
    framebuffer: &mut [u32],
    mut x: i32,
    y: i32,
    text: &str,
    color: u32,
    clip: gui::Rect,
    font: &mut FontCache,
) {
    let line = gui::Rect {
        x: clip.x,
        y,
        width: clip.width,
        height: 16,
    };
    if !rect_is_dirty(line) {
        return;
    }
    for character in text.chars() {
        let glyph = font.glyph(character as u32);
        let width = glyph.map_or(8, |glyph| glyph.width) as i32;
        if x + width > clip.x + clip.width as i32 {
            break;
        }
        if rect_is_dirty(gui::Rect {
            x,
            y,
            width: width as u32,
            height: 16,
        }) {
            if let Some(glyph) = glyph {
                draw_unifont_glyph(framebuffer, x, y, glyph, color, clip);
            } else if character.is_ascii() {
                draw_glyph_scaled(framebuffer, x, y, character as u8, color);
            } else {
                stroke_rect(
                    framebuffer,
                    gui::Rect {
                        x,
                        y,
                        width: 10,
                        height: 14,
                    },
                    color,
                    1,
                );
            }
        }
        x += width + 2;
    }
}

fn draw_unifont_glyph(
    framebuffer: &mut [u32],
    x: i32,
    y: i32,
    glyph: CachedGlyph,
    color: u32,
    clip: gui::Rect,
) {
    let bytes_per_row = (glyph.width / 8) as usize;
    for row in 0..16usize {
        for column in 0..glyph.width as usize {
            let byte = glyph.bitmap[row * bytes_per_row + column / 8];
            if byte & (0x80 >> (column % 8)) == 0 {
                continue;
            }
            let px = x + column as i32;
            let py = y + row as i32;
            if px >= clip.x
                && py >= clip.y
                && px < clip.x + clip.width as i32
                && py < clip.y + clip.height as i32
                && px >= 0
                && py >= 0
                && px < gui::WIDTH as i32
                && py < gui::HEIGHT as i32
                && pixel_is_dirty(px, py)
            {
                framebuffer[py as usize * gui::WIDTH as usize + px as usize] = color;
            }
        }
    }
}

fn terminal_focused(desktop: &Desktop) -> bool {
    desktop
        .focused()
        .and_then(|id| desktop.window(id))
        .is_some_and(|window| window.client == 1 && window.state != WindowState::Closed)
}

fn keycode_to_ascii(code: u16, shift: bool) -> Option<u8> {
    let byte = match code {
        2..=11 => {
            const NORMAL: &[u8; 10] = b"1234567890";
            const SHIFTED: &[u8; 10] = b"!@#$%^&*()";
            if shift {
                SHIFTED[(code - 2) as usize]
            } else {
                NORMAL[(code - 2) as usize]
            }
        }
        12 => {
            if shift {
                b'_'
            } else {
                b'-'
            }
        }
        13 => {
            if shift {
                b'+'
            } else {
                b'='
            }
        }
        16..=25 => b"qwertyuiop"[(code - 16) as usize],
        30..=38 => b"asdfghjkl"[(code - 30) as usize],
        44..=50 => b"zxcvbnm"[(code - 44) as usize],
        39 => {
            if shift {
                b':'
            } else {
                b';'
            }
        }
        40 => {
            if shift {
                b'\"'
            } else {
                b'\''
            }
        }
        43 => {
            if shift {
                b'|'
            } else {
                b'\\'
            }
        }
        51 => {
            if shift {
                b'<'
            } else {
                b','
            }
        }
        52 => {
            if shift {
                b'>'
            } else {
                b'.'
            }
        }
        53 => {
            if shift {
                b'?'
            } else {
                b'/'
            }
        }
        57 => b' ',
        _ => return None,
    };
    Some(if shift && byte.is_ascii_lowercase() {
        byte.to_ascii_uppercase()
    } else {
        byte
    })
}

struct TerminalView {
    bytes: [u8; TERMINAL_BUFFER_BYTES],
    length: usize,
    line: [u8; TERMINAL_LINE_BYTES],
    line_length: usize,
}

impl TerminalView {
    fn new() -> Self {
        let mut terminal = Self {
            bytes: [0; TERMINAL_BUFFER_BYTES],
            length: 0,
            line: [0; TERMINAL_LINE_BYTES],
            line_length: 0,
        };
        terminal.append(b"MicroSystem GUI Terminal\nType 'help' for commands.\n\nmicro> ");
        terminal
    }

    fn reset(&mut self) {
        self.length = 0;
        self.line_length = 0;
        self.append(b"MicroSystem GUI Terminal\n\nmicro> ");
    }

    fn push(&mut self, byte: u8) -> bool {
        if self.line_length == self.line.len() {
            return false;
        }
        self.line[self.line_length] = byte;
        self.line_length += 1;
        self.append(&[byte]);
        true
    }

    fn backspace(&mut self) -> bool {
        if self.line_length == 0 {
            return false;
        }
        self.line_length -= 1;
        self.length = self.length.saturating_sub(1);
        true
    }

    fn submit(&mut self) {
        let mut command = [0u8; TERMINAL_LINE_BYTES];
        let command_length = self.line_length;
        command[..command_length].copy_from_slice(&self.line[..command_length]);
        self.append(b"\n");
        let mut request = Message::new(protocol::GUI, gui::Operation::TerminalCommand as u16);
        pack_terminal_text(&mut request, &command[..command_length]);
        let mut reply = Message::new(protocol::GUI, 0);
        if microsystem_user_rt::ipc_call(boot_cap::GUI_TERMINAL_EVENTS, &request, &mut reply, 0)
            .is_ok()
            && reply.protocol == protocol::GUI
            && reply.opcode == gui::Operation::TerminalCommand as u16
        {
            if reply.flags & CLEAR_REPLY != 0 {
                self.reset();
                return;
            }
            if let Some(text) = unpack_terminal_text(&reply) {
                self.append(text);
            }
        } else {
            self.append(b"terminal service unavailable\n");
        }
        self.line_length = 0;
        self.append(b"micro> ");
    }

    fn append(&mut self, text: &[u8]) {
        for byte in text.iter().copied() {
            if self.length == self.bytes.len() {
                let keep = self.bytes.len() / 2;
                self.bytes.copy_within(self.length - keep..self.length, 0);
                self.length = keep;
            }
            self.bytes[self.length] = byte;
            self.length += 1;
        }
    }
}

fn pack_terminal_text(message: &mut Message, text: &[u8]) {
    let length = text.len().min(TERMINAL_SHARED_BYTES);
    unsafe {
        core::ptr::copy_nonoverlapping(text.as_ptr(), TERMINAL_SHARED_VA as *mut u8, length);
        microsystem_user_rt::fence();
    }
    message.words[0] = length as u64;
    message.words[4] = TERMINAL_SHARED_MAGIC;
}

fn unpack_terminal_text(message: &Message) -> Option<&[u8]> {
    let length = usize::try_from(message.words[0]).ok()?;
    if message.words[4] == TERMINAL_SHARED_MAGIC {
        if length > TERMINAL_SHARED_BYTES {
            return None;
        }
        microsystem_user_rt::fence();
        return Some(unsafe {
            core::slice::from_raw_parts(TERMINAL_SHARED_VA as *const u8, length)
        });
    }
    if length > INLINE_TEXT_BYTES {
        return None;
    }
    let bytes = unsafe {
        core::slice::from_raw_parts(message.words[1..].as_ptr().cast::<u8>(), INLINE_TEXT_BYTES)
    };
    Some(&bytes[..length])
}

fn draw_terminal(framebuffer: &mut [u32], rect: gui::Rect, terminal: &TerminalView) {
    let panel = gui::Rect {
        x: rect.x + 10,
        y: rect.y + microsystem_gui::TITLE_BAR_HEIGHT + 10,
        width: rect.width.saturating_sub(20),
        height: rect
            .height
            .saturating_sub(microsystem_gui::TITLE_BAR_HEIGHT as u32 + 20),
    };
    fill_rect(framebuffer, panel, 0x0008_101d);
    stroke_rect(framebuffer, panel, 0x0025_3852, 1);
    fill_rect(
        framebuffer,
        gui::Rect {
            x: panel.x,
            y: panel.y,
            width: 3,
            height: panel.height,
        },
        ACCENT,
    );
    let left = panel.x + 14;
    let top = panel.y + 14;
    let right = panel.x + panel.width as i32 - 10;
    let bottom = panel.y + panel.height as i32 - 10;
    let columns = ((right - left) / 12).max(1) as usize;
    let visible_lines = ((bottom - top) / 18).max(1) as usize;
    let mut total_lines = 1usize;
    let mut column = 0usize;
    for byte in terminal.bytes[..terminal.length].iter().copied() {
        if byte == b'\n' {
            total_lines += 1;
            column = 0;
        } else {
            if column == columns {
                total_lines += 1;
                column = 0;
            }
            column += 1;
        }
    }
    let first_line = total_lines.saturating_sub(visible_lines);
    let mut line = 0usize;
    let mut column = 0usize;
    for byte in terminal.bytes[..terminal.length].iter().copied() {
        if byte == b'\n' {
            line += 1;
            column = 0;
            continue;
        }
        if column == columns {
            line += 1;
            column = 0;
        }
        if line >= first_line {
            let x = left + column as i32 * 12;
            let y = top + (line - first_line) as i32 * 18;
            if y + 14 <= bottom {
                draw_glyph_scaled(framebuffer, x, y, byte, 0x00d9_e6f7);
            }
        }
        column += 1;
    }
}

fn draw_glyph_scaled(framebuffer: &mut [u32], x: i32, y: i32, byte: u8, color: u32) {
    if !rect_is_dirty(gui::Rect {
        x,
        y,
        width: 10,
        height: 14,
    }) {
        return;
    }
    for (row, bits) in glyph(byte).into_iter().enumerate() {
        for column in 0..5 {
            if bits & (1 << (4 - column)) == 0 {
                continue;
            }
            fill_rect(
                framebuffer,
                gui::Rect {
                    x: x + column * 2,
                    y: y + row as i32 * 2,
                    width: 2,
                    height: 2,
                },
                color,
            );
        }
    }
}

fn draw_pointer(framebuffer: &mut [u32], x: i32, y: i32) {
    for row in 0..16 {
        for column in 0..=row / 2 + 1 {
            let px = x + column;
            let py = y + row;
            if px >= 0
                && py >= 0
                && px < gui::WIDTH as i32
                && py < gui::HEIGHT as i32
                && pixel_is_dirty(px, py)
            {
                framebuffer[py as usize * gui::WIDTH as usize + px as usize] = SHADOW;
            }
        }
    }
    for row in 1..14 {
        for column in 1..=row / 2 {
            let px = x + column;
            let py = y + row;
            if px >= 0
                && py >= 0
                && px < gui::WIDTH as i32
                && py < gui::HEIGHT as i32
                && pixel_is_dirty(px, py)
            {
                framebuffer[py as usize * gui::WIDTH as usize + px as usize] = TEXT;
            }
        }
    }
}

fn draw_text(framebuffer: &mut [u32], x: i32, y: i32, text: &[u8], color: u32) {
    for (index, byte) in text.iter().copied().enumerate() {
        let glyph_x = x + index as i32 * 6;
        if !rect_is_dirty(gui::Rect {
            x: glyph_x,
            y,
            width: 5,
            height: 7,
        }) {
            continue;
        }
        let glyph = glyph(byte);
        for (row, bits) in glyph.into_iter().enumerate() {
            for column in 0..5 {
                if bits & (1 << (4 - column)) != 0 {
                    let px = glyph_x + column;
                    let py = y + row as i32;
                    if px >= 0
                        && py >= 0
                        && px < gui::WIDTH as i32
                        && py < gui::HEIGHT as i32
                        && pixel_is_dirty(px, py)
                    {
                        framebuffer[py as usize * gui::WIDTH as usize + px as usize] = color;
                    }
                }
            }
        }
    }
}

fn glyph(byte: u8) -> [u8; 7] {
    match byte.to_ascii_uppercase() {
        b'A' => [14, 17, 17, 31, 17, 17, 17],
        b'B' => [30, 17, 17, 30, 17, 17, 30],
        b'C' => [14, 17, 16, 16, 16, 17, 14],
        b'D' => [30, 17, 17, 17, 17, 17, 30],
        b'E' => [31, 16, 16, 30, 16, 16, 31],
        b'F' => [31, 16, 16, 30, 16, 16, 16],
        b'G' => [14, 17, 16, 23, 17, 17, 14],
        b'H' => [17, 17, 17, 31, 17, 17, 17],
        b'I' => [31, 4, 4, 4, 4, 4, 31],
        b'J' => [1, 1, 1, 1, 17, 17, 14],
        b'K' => [17, 18, 20, 24, 20, 18, 17],
        b'L' => [16, 16, 16, 16, 16, 16, 31],
        b'M' => [17, 27, 21, 21, 17, 17, 17],
        b'N' => [17, 25, 21, 19, 17, 17, 17],
        b'O' => [14, 17, 17, 17, 17, 17, 14],
        b'P' => [30, 17, 17, 30, 16, 16, 16],
        b'Q' => [14, 17, 17, 17, 21, 18, 13],
        b'R' => [30, 17, 17, 30, 20, 18, 17],
        b'S' => [15, 16, 16, 14, 1, 1, 30],
        b'T' => [31, 4, 4, 4, 4, 4, 4],
        b'U' => [17, 17, 17, 17, 17, 17, 14],
        b'V' => [17, 17, 17, 17, 17, 10, 4],
        b'W' => [17, 17, 17, 21, 21, 21, 10],
        b'X' => [17, 17, 10, 4, 10, 17, 17],
        b'Y' => [17, 17, 10, 4, 4, 4, 4],
        b'Z' => [31, 1, 2, 4, 8, 16, 31],
        b'0' => [14, 17, 19, 21, 25, 17, 14],
        b'1' => [4, 12, 4, 4, 4, 4, 14],
        b'2' => [14, 17, 1, 2, 4, 8, 31],
        b'3' => [30, 1, 1, 14, 1, 1, 30],
        b'4' => [2, 6, 10, 18, 31, 2, 2],
        b'5' => [31, 16, 16, 30, 1, 1, 30],
        b'6' => [14, 16, 16, 30, 17, 17, 14],
        b'7' => [31, 1, 2, 4, 8, 8, 8],
        b'8' => [14, 17, 17, 14, 17, 17, 14],
        b'9' => [14, 17, 17, 15, 1, 1, 14],
        b'>' => [16, 8, 4, 8, 16, 0, 0],
        b'<' => [1, 2, 4, 2, 1, 0, 0],
        b':' => [0, 4, 0, 0, 4, 0, 0],
        b';' => [0, 4, 0, 0, 4, 4, 8],
        b'.' => [0, 0, 0, 0, 0, 0, 4],
        b',' => [0, 0, 0, 0, 0, 4, 8],
        b'-' => [0, 0, 0, 31, 0, 0, 0],
        b'_' => [0, 0, 0, 0, 0, 0, 31],
        b'/' => [1, 2, 4, 8, 16, 0, 0],
        b'?' => [14, 17, 1, 2, 4, 0, 4],
        b'\'' => [4, 4, 8, 0, 0, 0, 0],
        b' ' => [0; 7],
        _ => [0; 7],
    }
}

fn clear(framebuffer: &mut [u32], color: u32) {
    fill_rect(framebuffer, FULL_SCREEN, color);
}

fn fill_rect(framebuffer: &mut [u32], rect: gui::Rect, color: u32) {
    for index in 0..render_damage_count() {
        let Some(rect) = intersect_rect(rect, render_damage(index)) else {
            continue;
        };
        let left = rect.x.max(0) as usize;
        let top = rect.y.max(0) as usize;
        let right = (rect.x.saturating_add(rect.width as i32)).clamp(0, gui::WIDTH as i32) as usize;
        let bottom =
            (rect.y.saturating_add(rect.height as i32)).clamp(0, gui::HEIGHT as i32) as usize;
        for y in top..bottom {
            framebuffer[y * gui::WIDTH as usize + left..y * gui::WIDTH as usize + right]
                .fill(color);
        }
    }
}

fn fill_round_rect(framebuffer: &mut [u32], rect: gui::Rect, color: u32, radius: u32) {
    let _ = radius;
    fill_rect(framebuffer, rect, color);
}

#[panic_handler]
fn panic(_info: &PanicInfo<'_>) -> ! {
    microsystem_user_rt::exit(1)
}
