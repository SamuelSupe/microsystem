#![no_std]

use microsystem_abi::{Status, gui};

pub mod composition;

pub const MAX_WINDOWS: usize = 3 + gui::MAX_DYNAMIC_CLIENTS * gui::MAX_CLIENT_WINDOWS;
pub const TITLE_BYTES: usize = 64;
pub const TITLE_BAR_HEIGHT: i32 = 28;
pub const TASKBAR_HEIGHT: i32 = 36;
pub const MIN_WIDTH: i32 = 180;
pub const MIN_HEIGHT: i32 = 120;
pub const DESKTOP_ICON_SIZE: i32 = 64;
pub const DESKTOP_ICON_TOP: i32 = 650;

#[derive(Clone, Copy)]
pub struct DesktopIcon {
    pub application: gui::Application,
    pub rect: gui::Rect,
    pub label: &'static [u8],
}

pub const DESKTOP_ICONS: [DesktopIcon; 5] = [
    desktop_icon(gui::Application::Terminal, 16, b"Terminal"),
    desktop_icon(gui::Application::Files, 92, b"Files"),
    desktop_icon(gui::Application::Monitor, 168, b"Monitor"),
    desktop_icon(gui::Application::Reader, 864, b"Reader"),
    desktop_icon(gui::Application::Editor, 940, b"Editor"),
];

const fn desktop_icon(application: gui::Application, x: i32, label: &'static [u8]) -> DesktopIcon {
    DesktopIcon {
        application,
        rect: gui::Rect {
            x,
            y: DESKTOP_ICON_TOP,
            width: DESKTOP_ICON_SIZE as u32,
            height: DESKTOP_ICON_SIZE as u32,
        },
        label,
    }
}

pub fn desktop_icon_at(x: i32, y: i32) -> Option<gui::Application> {
    DESKTOP_ICONS.iter().find_map(|icon| {
        let inside = x >= icon.rect.x
            && y >= icon.rect.y
            && x < icon.rect.x + icon.rect.width as i32
            && y < icon.rect.y + icon.rect.height as i32;
        inside.then_some(icon.application)
    })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WindowState {
    Normal,
    Minimized,
    Maximized,
    Closed,
}

#[derive(Clone, Copy)]
pub struct Window {
    pub id: u32,
    pub client: u8,
    pub rect: gui::Rect,
    pub restore: gui::Rect,
    pub state: WindowState,
    pub title: [u8; TITLE_BYTES],
    pub title_len: u8,
    pub sequence: u64,
}

impl Window {
    pub const EMPTY: Self = Self {
        id: 0,
        client: 0,
        rect: gui::Rect {
            x: 0,
            y: 0,
            width: 0,
            height: 0,
        },
        restore: gui::Rect {
            x: 0,
            y: 0,
            width: 0,
            height: 0,
        },
        state: WindowState::Closed,
        title: [0; TITLE_BYTES],
        title_len: 0,
        sequence: 0,
    };
}

pub struct Desktop {
    windows: [Window; MAX_WINDOWS],
    z_order: [u8; MAX_WINDOWS],
    count: usize,
    next_id: u32,
    focused: Option<u32>,
}

impl Desktop {
    pub const fn new() -> Self {
        Self {
            windows: [Window::EMPTY; MAX_WINDOWS],
            z_order: [0; MAX_WINDOWS],
            count: 0,
            next_id: 1,
            focused: None,
        }
    }

    pub fn create_window(
        &mut self,
        client: u8,
        rect: gui::Rect,
        title: &[u8],
    ) -> Result<u32, Status> {
        if let Some(existing) = self
            .windows
            .iter()
            .find(|window| {
                window.id != 0 && window.client == client && window.state == WindowState::Closed
            })
            .map(|window| window.id)
        {
            self.remove_window(existing)?;
        }
        if self.count == MAX_WINDOWS || !valid_rect(rect) || core::str::from_utf8(title).is_err() {
            return Err(Status::Invalid);
        }
        let slot = self
            .windows
            .iter()
            .position(|window| window.id == 0)
            .ok_or(Status::NoMemory)?;
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1).max(1);
        let mut window = Window {
            id,
            client,
            rect: clamp_rect(rect),
            restore: clamp_rect(rect),
            state: WindowState::Normal,
            title: [0; TITLE_BYTES],
            title_len: utf8_prefix_len(title, TITLE_BYTES) as u8,
            sequence: 0,
        };
        let title_len = window.title_len as usize;
        window.title[..title_len].copy_from_slice(&title[..title_len]);
        self.windows[slot] = window;
        self.z_order[self.count] = slot as u8;
        self.count += 1;
        self.focused = Some(id);
        Ok(id)
    }

    pub fn window(&self, id: u32) -> Option<&Window> {
        self.windows.iter().find(|window| window.id == id)
    }

    pub fn window_for_client(&self, client: u8) -> Option<&Window> {
        self.windows
            .iter()
            .find(|window| window.id != 0 && window.client == client)
    }

    pub fn set_title(&mut self, id: u32, title: &[u8]) -> Result<(), Status> {
        core::str::from_utf8(title).map_err(|_| Status::Invalid)?;
        let window = self.window_mut(id)?;
        window.title.fill(0);
        window.title_len = utf8_prefix_len(title, TITLE_BYTES) as u8;
        let length = window.title_len as usize;
        window.title[..length].copy_from_slice(&title[..length]);
        Ok(())
    }

    pub fn focused(&self) -> Option<u32> {
        self.focused
    }

    pub fn focus(&mut self, id: u32) -> Result<(), Status> {
        let slot = self
            .windows
            .iter()
            .position(|window| window.id == id && window.state != WindowState::Closed)
            .ok_or(Status::NotFound)?;
        if let Some(position) = self.z_order[..self.count]
            .iter()
            .position(|entry| *entry as usize == slot)
        {
            self.z_order[position..self.count].rotate_left(1);
            self.z_order[self.count - 1] = slot as u8;
        }
        self.focused = Some(id);
        Ok(())
    }

    pub fn move_window(&mut self, id: u32, x: i32, y: i32) -> Result<(), Status> {
        let window = self.window_mut(id)?;
        if window.state != WindowState::Normal {
            return Err(Status::Busy);
        }
        window.rect.x = x;
        window.rect.y = y;
        window.rect = clamp_rect(window.rect);
        window.restore = window.rect;
        Ok(())
    }

    pub fn resize_window(&mut self, id: u32, width: u32, height: u32) -> Result<(), Status> {
        let window = self.window_mut(id)?;
        if window.state != WindowState::Normal {
            return Err(Status::Busy);
        }
        window.rect.width = width.max(MIN_WIDTH as u32);
        window.rect.height = height.max(MIN_HEIGHT as u32);
        window.rect = clamp_rect(window.rect);
        window.restore = window.rect;
        Ok(())
    }

    pub fn action(&mut self, id: u32, action: gui::WindowAction) -> Result<(), Status> {
        let window = self.window_mut(id)?;
        match action {
            gui::WindowAction::Minimize => window.state = WindowState::Minimized,
            gui::WindowAction::Maximize => {
                if window.state == WindowState::Normal {
                    window.restore = window.rect;
                }
                window.rect = gui::Rect {
                    x: 0,
                    y: 0,
                    width: gui::WIDTH,
                    height: gui::HEIGHT - TASKBAR_HEIGHT as u32,
                };
                window.state = WindowState::Maximized;
            }
            gui::WindowAction::Restore => {
                window.rect = window.restore;
                window.state = WindowState::Normal;
            }
            gui::WindowAction::Close => window.state = WindowState::Closed,
        }
        if matches!(window.state, WindowState::Minimized | WindowState::Closed)
            && self.focused == Some(id)
        {
            self.focused = None;
        }
        Ok(())
    }

    pub fn remove_window(&mut self, id: u32) -> Result<(), Status> {
        let slot = self
            .windows
            .iter()
            .position(|window| window.id == id)
            .ok_or(Status::NotFound)?;
        let position = self.z_order[..self.count]
            .iter()
            .position(|entry| *entry as usize == slot)
            .ok_or(Status::NotFound)?;
        self.z_order[position..self.count].rotate_left(1);
        self.count -= 1;
        self.z_order[self.count] = 0;
        self.windows[slot] = Window::EMPTY;
        if self.focused == Some(id) {
            self.focused = self.z_order[..self.count].iter().rev().find_map(|entry| {
                let window = &self.windows[*entry as usize];
                matches!(window.state, WindowState::Normal | WindowState::Maximized)
                    .then_some(window.id)
            });
        }
        Ok(())
    }

    pub fn alt_tab(&mut self) -> Option<u32> {
        let visible: [Option<u32>; MAX_WINDOWS] = core::array::from_fn(|index| {
            if index >= self.count {
                return None;
            }
            let window = &self.windows[self.z_order[index] as usize];
            matches!(window.state, WindowState::Normal | WindowState::Maximized)
                .then_some(window.id)
        });
        let current = self.focused;
        let mut candidate = None;
        for id in visible.into_iter().flatten() {
            if Some(id) == current {
                candidate = None;
            } else if candidate.is_none() {
                candidate = Some(id);
            }
        }
        if candidate.is_none() {
            candidate = visible.into_iter().flatten().next();
        }
        if let Some(id) = candidate {
            let _ = self.focus(id);
        }
        candidate
    }

    pub fn z_order(&self) -> impl Iterator<Item = &Window> {
        self.z_order[..self.count]
            .iter()
            .map(|slot| &self.windows[*slot as usize])
    }

    pub fn hit_test(&self, x: i32, y: i32) -> Option<u32> {
        self.z_order[..self.count].iter().rev().find_map(|slot| {
            let window = &self.windows[*slot as usize];
            let visible = matches!(window.state, WindowState::Normal | WindowState::Maximized);
            let inside = x >= window.rect.x
                && y >= window.rect.y
                && x < window.rect.x.saturating_add(window.rect.width as i32)
                && y < window.rect.y.saturating_add(window.rect.height as i32);
            (visible && inside).then_some(window.id)
        })
    }

    fn window_mut(&mut self, id: u32) -> Result<&mut Window, Status> {
        self.windows
            .iter_mut()
            .find(|window| window.id == id)
            .ok_or(Status::NotFound)
    }
}

impl Default for Desktop {
    fn default() -> Self {
        Self::new()
    }
}

pub fn validate_command_stream(bytes: &[u8]) -> Result<usize, Status> {
    if bytes.len() > gui::COMMAND_BYTES {
        return Err(Status::Invalid);
    }
    let mut offset = 0usize;
    let mut commands = 0usize;
    let mut text_bytes = 0usize;
    while offset < bytes.len() {
        if bytes.len() - offset < core::mem::size_of::<gui::DrawCommand>() {
            return Err(Status::Invalid);
        }
        let command = unsafe {
            core::ptr::read_unaligned(bytes.as_ptr().add(offset).cast::<gui::DrawCommand>())
        };
        let command_bytes = command.header.bytes as usize;
        if command_bytes < core::mem::size_of::<gui::DrawCommand>()
            || offset
                .checked_add(command_bytes)
                .is_none_or(|end| end > bytes.len())
        {
            return Err(Status::Invalid);
        }
        if command.header.flags != 0
            || !matches!(command.header.kind, 1..=6)
            || !valid_rect(command.rect)
        {
            return Err(Status::Invalid);
        }
        if command.header.kind == gui::CommandKind::Text as u16 {
            let payload =
                &bytes[offset + core::mem::size_of::<gui::DrawCommand>()..offset + command_bytes];
            core::str::from_utf8(payload).map_err(|_| Status::Invalid)?;
            text_bytes = text_bytes
                .checked_add(payload.len())
                .ok_or(Status::Invalid)?;
            if text_bytes > gui::MAX_TEXT_BYTES {
                return Err(Status::Invalid);
            }
        }
        commands += 1;
        if commands > gui::MAX_COMMANDS {
            return Err(Status::Invalid);
        }
        offset += command_bytes;
    }
    Ok(commands)
}

pub fn replace_display_list(current: &mut [u8], candidate: &[u8]) -> Result<usize, Status> {
    if candidate.len() > current.len() {
        return Err(Status::Invalid);
    }
    let commands = validate_command_stream(candidate)?;
    current[..candidate.len()].copy_from_slice(candidate);
    current[candidate.len()..].fill(0);
    Ok(commands)
}

fn valid_rect(rect: gui::Rect) -> bool {
    rect.width > 0 && rect.height > 0 && rect.width <= gui::WIDTH && rect.height <= gui::HEIGHT
}

fn utf8_prefix_len(bytes: &[u8], limit: usize) -> usize {
    let mut length = bytes.len().min(limit);
    while core::str::from_utf8(&bytes[..length]).is_err() {
        length -= 1;
    }
    length
}

fn clamp_rect(mut rect: gui::Rect) -> gui::Rect {
    rect.width = rect.width.clamp(MIN_WIDTH as u32, gui::WIDTH);
    rect.height = rect
        .height
        .clamp(MIN_HEIGHT as u32, gui::HEIGHT - TASKBAR_HEIGHT as u32);
    rect.x = rect.x.clamp(0, gui::WIDTH as i32 - rect.width as i32);
    rect.y = rect
        .y
        .clamp(0, gui::HEIGHT as i32 - TASKBAR_HEIGHT - rect.height as i32);
    rect
}
