use alloc::borrow::Cow;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use microsystem_abi::{CapHandle, Message, Rights, Status, boot_cap, gui, protocol, script};
use microsystem_mica::{ErrorValue, Value};

const MAX_WIDGETS: usize = 256;
const MAX_CHILDREN: usize = 32;
const DEFAULT_WIDTH: u32 = 420;
const DEFAULT_HEIGHT: u32 = 260;
const COLOR_BACKGROUND: u32 = 0x00f5_f8fb;
const COLOR_PANEL: u32 = 0x00eb_f0f5;
const COLOR_INPUT: u32 = 0x00ff_ffff;
const COLOR_BUTTON: u32 = 0x00e7_ecf2;
const COLOR_BORDER: u32 = 0x00c4_ced9;
const COLOR_ACCENT: u32 = 0x0000_7aff;
const COLOR_SELECTION: u32 = 0x00d9_eaff;
const COLOR_TEXT: u32 = 0x001b_2733;
const COLOR_MUTED: u32 = 0x006b_7785;

pub const PRELUDE: &str = r#"
function __gui_set_root(self, root)
    return _gui_set_root(self, root)
end
function __gui_set_title(self, title)
    return _gui_set_title(self, title)
end
function __gui_invalidate(self)
    return _gui_invalidate(self)
end
function __gui_present(self)
    return _gui_present(self)
end
function __gui_close(self)
    return _gui_close(self)
end
function __gui_set_text(self, text)
    return _gui_set_text(self, text)
end
function __gui_set_checked(self, checked)
    return _gui_set_checked(self, checked)
end
function __gui_set_items(self, items)
    return _gui_set_items(self, items)
end
function __gui_set_callback(self, name, callback)
    return _gui_set_callback(self, name, callback)
end
function __gui_wrap_widget(native)
    return {
        _widget = native._widget,
        set_text = __gui_set_text,
        set_checked = __gui_set_checked,
        set_items = __gui_set_items,
        set_callback = __gui_set_callback
    }
end
function __gui_window(spec)
    local native, err = _gui_window(spec)
    if not native then return nil, err end
    return {
        _window = native._window,
        set_root = __gui_set_root,
        set_title = __gui_set_title,
        invalidate = __gui_invalidate,
        present = __gui_present,
        run = __gui_run,
        close = __gui_close
    }, nil
end
function __gui_widget(kind, spec)
    return __gui_wrap_widget(_gui_widget(kind, spec))
end
function __gui_label(spec) return __gui_widget("label", spec) end
function __gui_button(spec) return __gui_widget("button", spec) end
function __gui_text_input(spec) return __gui_widget("text_input", spec) end
function __gui_checkbox(spec) return __gui_widget("checkbox", spec) end
function __gui_list(spec) return __gui_widget("list", spec) end
function __gui_scroll(spec) return __gui_widget("scroll", spec) end
function __gui_row(spec) return __gui_widget("row", spec) end
function __gui_column(spec) return __gui_widget("column", spec) end
function __gui_spacer(spec) return __gui_widget("spacer", spec) end
function __gui_canvas(spec) return __gui_widget("canvas", spec) end
function __gui_run(self)
    while true do
        local callback, event, err = _gui_next(self)
        if err then return nil, err end
        if callback then
            if event.kind == "change" or event.kind == "submit" then
                callback(event.text)
            elseif event.kind == "select" then
                callback(event.value)
            else
                callback(event)
            end
        end
        if not _gui_is_open(self) then return true, nil end
        if event and event.kind == "close" then
            _gui_close({ _window = event.window })
            if event.window == self._window then return true, nil end
        end
    end
end
local __mica_require = require
local __mica_gui = {
    clipboard = { read = _gui_clipboard_read, write = _gui_clipboard_write },
    window = __gui_window,
    label = __gui_label,
    button = __gui_button,
    text_input = __gui_text_input,
    checkbox = __gui_checkbox,
    list = __gui_list,
    scroll = __gui_scroll,
    row = __gui_row,
    column = __gui_column,
    spacer = __gui_spacer,
    canvas = __gui_canvas
}
function require(name)
    if name == "gui" then return __mica_gui end
    return __mica_require(name)
end
"#;

#[derive(Clone, Copy, Eq, PartialEq)]
enum WidgetKind {
    Label,
    Button,
    TextInput,
    Checkbox,
    List,
    Scroll,
    Row,
    Column,
    Spacer,
    Canvas,
}

struct Widget {
    id: u32,
    owner: u32,
    kind: WidgetKind,
    text: String,
    cursor: usize,
    selected_all: bool,
    placeholder: String,
    checked: bool,
    selected: usize,
    items: Vec<String>,
    children: Vec<u32>,
    width: Option<u32>,
    height: Option<u32>,
    grow: u32,
    padding: u32,
    gap: u32,
    scroll: i32,
    rect: gui::Rect,
    on_click: Option<Value>,
    on_change: Option<Value>,
    on_submit: Option<Value>,
    on_select: Option<Value>,
    canvas: Vec<CanvasCommand>,
}

struct CanvasCommand {
    kind: gui::CommandKind,
    rect: gui::Rect,
    color: u32,
    argument: u32,
    text: String,
}

impl Widget {
    fn from_spec(kind: WidgetKind, spec: &Value) -> Result<Self, ErrorValue> {
        let children = table_value(spec, "children")
            .and_then(|value| match value {
                Value::Table(values) => Some(values),
                _ => None,
            })
            .map(|values| {
                values
                    .iter()
                    .filter_map(|(_, value)| object_id(value, "_widget").ok())
                    .take(MAX_CHILDREN)
                    .collect()
            })
            .unwrap_or_default();
        let items = table_value(spec, "items")
            .and_then(|value| match value {
                Value::Table(values) => Some(values),
                _ => None,
            })
            .map(|values| {
                values
                    .iter()
                    .filter_map(|(_, value)| match value {
                        Value::String(value) => Some(value.clone()),
                        _ => None,
                    })
                    .take(MAX_CHILDREN)
                    .collect()
            })
            .unwrap_or_default();
        let text = table_string(spec, "text").unwrap_or_default();
        let cursor = text.len();
        Ok(Self {
            id: 0,
            owner: 0,
            kind,
            text,
            cursor,
            selected_all: false,
            placeholder: table_string(spec, "placeholder").unwrap_or_default(),
            checked: table_bool(spec, "checked"),
            selected: 0,
            items,
            children,
            width: table_u32(spec, "width"),
            height: table_u32(spec, "height"),
            grow: table_u32(spec, "grow").unwrap_or(0),
            padding: table_u32(spec, "padding").unwrap_or(0).min(64),
            gap: table_u32(spec, "gap").unwrap_or(0).min(64),
            scroll: 0,
            rect: gui::Rect::default(),
            on_click: table_callback(spec, "on_click"),
            on_change: table_callback(spec, "on_change"),
            on_submit: table_callback(spec, "on_submit"),
            on_select: table_callback(spec, "on_select"),
            canvas: parse_canvas_commands(spec)?,
        })
    }

    fn interactive(&self) -> bool {
        matches!(
            self.kind,
            WidgetKind::Button | WidgetKind::TextInput | WidgetKind::Checkbox | WidgetKind::List
        )
    }
}

#[derive(Clone, Copy)]
struct WindowContext {
    id: u32,
    active: bool,
    width: u32,
    height: u32,
    root: Option<u32>,
    focused: Option<u32>,
    sequence: u64,
    dirty: bool,
    reported: bool,
    unicode_codepoint: Option<u32>,
}

impl WindowContext {
    const EMPTY: Self = Self {
        id: 0,
        active: false,
        width: DEFAULT_WIDTH,
        height: DEFAULT_HEIGHT,
        root: None,
        focused: None,
        sequence: 0,
        dirty: false,
        reported: false,
        unicode_codepoint: None,
    };
}

pub struct GuiHost {
    endpoint: CapHandle,
    notification: CapHandle,
    command_base: *mut u8,
    event_base: *mut u8,
    current: WindowContext,
    windows: Vec<WindowContext>,
    next_widget: u32,
    widgets: Vec<Widget>,
}

impl GuiHost {
    pub fn new(notification: CapHandle) -> Result<Self, Status> {
        microsystem_user_rt::frame_map(
            boot_cap::SCRIPT_GUI_COMMANDS,
            script::GUI_COMMAND_VA,
            Rights(Rights::READ.0 | Rights::WRITE.0),
        )?;
        if let Err(status) = microsystem_user_rt::frame_map(
            boot_cap::SCRIPT_GUI_EVENTS,
            script::GUI_EVENT_VA,
            Rights(Rights::READ.0 | Rights::WRITE.0),
        ) {
            let _ = microsystem_user_rt::frame_unmap(
                boot_cap::SCRIPT_GUI_COMMANDS,
                script::GUI_COMMAND_VA,
            );
            return Err(status);
        }
        Ok(Self {
            endpoint: boot_cap::SCRIPT_GUI_ENDPOINT,
            notification,
            command_base: script::GUI_COMMAND_VA as *mut u8,
            event_base: script::GUI_EVENT_VA as *mut u8,
            current: WindowContext::EMPTY,
            windows: Vec::new(),
            next_widget: 1,
            widgets: Vec::new(),
        })
    }

    pub fn call(&mut self, name: &str, arguments: &[Value]) -> Result<Vec<Value>, ErrorValue> {
        match name {
            "_gui_window" => self.create_window(arguments),
            "_gui_widget" => self.create_widget(arguments),
            "_gui_set_root" => self.set_root(arguments),
            "_gui_set_title" => self.set_title(arguments),
            "_gui_invalidate" => {
                self.require_window(arguments.first())?;
                self.current.dirty = true;
                ok(Value::Bool(true))
            }
            "_gui_present" => self.present_now(arguments),
            "_gui_set_text" => self.set_text(arguments),
            "_gui_set_checked" => self.set_checked(arguments),
            "_gui_set_items" => self.set_items(arguments),
            "_gui_set_callback" => self.set_callback(arguments),
            "_gui_close" => self.close(arguments),
            "_gui_next" => self.next_event(),
            "_gui_is_open" => {
                let id = object_id(
                    arguments
                        .first()
                        .ok_or_else(|| ErrorValue::new("argument", "window is required"))?,
                    "_window",
                )?;
                Ok(alloc::vec![Value::Bool(
                    self.windows.iter().any(|window| window.id == id)
                )])
            }
            "_gui_clipboard_read" => self
                .clipboard_read()
                .and_then(|text| ok(Value::String(text))),
            "_gui_clipboard_write" => {
                self.clipboard_write(string_argument(arguments, 0)?)?;
                ok(Value::Bool(true))
            }
            _ => Err(ErrorValue::new("name", "unknown GUI operation").operation(name)),
        }
    }

    pub fn gc_roots(&self) -> Vec<Value> {
        self.widgets
            .iter()
            .flat_map(|widget| {
                [
                    widget.on_click.clone(),
                    widget.on_change.clone(),
                    widget.on_submit.clone(),
                    widget.on_select.clone(),
                ]
                .into_iter()
                .flatten()
            })
            .collect()
    }

    fn create_window(&mut self, arguments: &[Value]) -> Result<Vec<Value>, ErrorValue> {
        if self.windows.len() == gui::MAX_CLIENT_WINDOWS {
            return system_error(ErrorValue::new("busy", "GUI window limit exceeded"));
        }
        let spec = arguments
            .first()
            .ok_or_else(|| ErrorValue::new("argument", "window options are required"))?;
        let width = table_u32(spec, "width")
            .unwrap_or(DEFAULT_WIDTH)
            .clamp(180, gui::WIDTH);
        let height = table_u32(spec, "height")
            .unwrap_or(DEFAULT_HEIGHT)
            .clamp(120, gui::HEIGHT - 36);
        let title = table_string(spec, "title").unwrap_or_else(|| "Mica".to_string());
        self.write_header_title(&title)?;
        let mut request = Message::new(protocol::GUI, gui::Operation::CreateWindow as u16);
        request.words[0] =
            table_u32(spec, "x").unwrap_or(72 + self.windows.len() as u32 * 32) as u64;
        request.words[1] =
            table_u32(spec, "y").unwrap_or(76 + self.windows.len() as u32 * 32) as u64;
        request.words[2] = width as u64;
        request.words[3] = height as u64;
        let reply = self.call_endpoint("gui.create_window", &request)?;
        let id = reply.words[0] as u32;
        if id == 0 {
            return system_error(ErrorValue::new("gui", "window server returned no window"));
        }
        self.save_current();
        self.current = WindowContext {
            id,
            active: true,
            width,
            height,
            dirty: true,
            ..WindowContext::EMPTY
        };
        self.windows.push(self.current);
        ok(object("_window", self.current.id, &[]))
    }

    fn create_widget(&mut self, arguments: &[Value]) -> Result<Vec<Value>, ErrorValue> {
        if self.widgets.len() == MAX_WIDGETS {
            return Err(ErrorValue::new("limit", "GUI widget limit exceeded"));
        }
        let kind = match string_argument(arguments, 0)? {
            "label" => WidgetKind::Label,
            "button" => WidgetKind::Button,
            "text_input" => WidgetKind::TextInput,
            "checkbox" => WidgetKind::Checkbox,
            "list" => WidgetKind::List,
            "scroll" => WidgetKind::Scroll,
            "row" => WidgetKind::Row,
            "column" => WidgetKind::Column,
            "spacer" => WidgetKind::Spacer,
            "canvas" => WidgetKind::Canvas,
            _ => return Err(ErrorValue::new("argument", "unknown GUI widget")),
        };
        let empty = Value::Table(Vec::new());
        let spec = arguments.get(1).unwrap_or(&empty);
        let mut widget = Widget::from_spec(kind, spec)?;
        let id = self.next_widget;
        self.next_widget = self
            .next_widget
            .checked_add(1)
            .ok_or_else(|| ErrorValue::new("limit", "GUI widget handles exhausted"))?;
        widget.id = id;
        self.widgets.push(widget);
        Ok(alloc::vec![object("_widget", id, &[])])
    }

    fn set_root(&mut self, arguments: &[Value]) -> Result<Vec<Value>, ErrorValue> {
        self.require_window(arguments.first())?;
        let root = object_id(
            arguments
                .get(1)
                .ok_or_else(|| ErrorValue::new("argument", "root widget is required"))?,
            "_widget",
        )?;
        self.bind_tree(root)?;
        self.current.root = Some(root);
        self.current.dirty = true;
        ok(Value::Bool(true))
    }

    fn set_title(&mut self, arguments: &[Value]) -> Result<Vec<Value>, ErrorValue> {
        self.require_window(arguments.first())?;
        let title = string_argument(arguments, 1)?;
        self.write_header_title(title)?;
        let mut request = Message::new(protocol::GUI, gui::Operation::SetTitle as u16);
        request.words[0] = self.current.id as u64;
        self.call_endpoint("gui.set_title", &request)?;
        ok(Value::Bool(true))
    }

    fn set_text(&mut self, arguments: &[Value]) -> Result<Vec<Value>, ErrorValue> {
        let id = object_id(
            arguments
                .first()
                .ok_or_else(|| ErrorValue::new("argument", "widget is required"))?,
            "_widget",
        )?;
        let text = string_argument(arguments, 1)?.to_string();
        let widget = self.widget_mut(id)?;
        widget.text = text;
        widget.cursor = widget.text.len();
        widget.selected_all = false;
        self.current.dirty = true;
        ok(Value::Bool(true))
    }

    fn set_checked(&mut self, arguments: &[Value]) -> Result<Vec<Value>, ErrorValue> {
        let id = object_id(
            arguments
                .first()
                .ok_or_else(|| ErrorValue::new("argument", "widget is required"))?,
            "_widget",
        )?;
        let checked = matches!(arguments.get(1), Some(Value::Bool(true)));
        self.widget_mut(id)?.checked = checked;
        self.current.dirty = true;
        ok(Value::Bool(true))
    }

    fn set_items(&mut self, arguments: &[Value]) -> Result<Vec<Value>, ErrorValue> {
        let id = object_id(
            arguments
                .first()
                .ok_or_else(|| ErrorValue::new("argument", "widget is required"))?,
            "_widget",
        )?;
        let values = match arguments.get(1) {
            Some(Value::Table(values)) => values,
            _ => return Err(ErrorValue::new("type", "items must be a table")),
        };
        self.widget_mut(id)?.items = values
            .iter()
            .filter_map(|(_, value)| match value {
                Value::String(value) => Some(value.clone()),
                _ => None,
            })
            .take(MAX_CHILDREN)
            .collect();
        self.current.dirty = true;
        ok(Value::Bool(true))
    }

    fn set_callback(&mut self, arguments: &[Value]) -> Result<Vec<Value>, ErrorValue> {
        let id = object_id(
            arguments
                .first()
                .ok_or_else(|| ErrorValue::new("argument", "widget is required"))?,
            "_widget",
        )?;
        let name = string_argument(arguments, 1)?;
        let callback = match arguments.get(2) {
            Some(value @ (Value::Function(_) | Value::Native(_))) => value.clone(),
            Some(Value::Nil) => Value::Nil,
            _ => {
                return Err(ErrorValue::new(
                    "type",
                    "callback must be a function or nil",
                ));
            }
        };
        let slot = match name {
            "click" => &mut self.widget_mut(id)?.on_click,
            "change" => &mut self.widget_mut(id)?.on_change,
            "submit" => &mut self.widget_mut(id)?.on_submit,
            "select" => &mut self.widget_mut(id)?.on_select,
            _ => return Err(ErrorValue::new("argument", "unknown callback name")),
        };
        *slot = (!matches!(callback, Value::Nil)).then_some(callback);
        ok(Value::Bool(true))
    }

    fn close(&mut self, arguments: &[Value]) -> Result<Vec<Value>, ErrorValue> {
        self.require_window(arguments.first())?;
        if self.current.id != 0 {
            let mut request = Message::new(protocol::GUI, gui::Operation::WindowAction as u16);
            request.words[0] = gui::WindowAction::Close as u64;
            request.words[1] = self.current.id as u64;
            self.call_endpoint("gui.close", &request)?;
            let id = self.current.id;
            self.windows.retain(|window| window.id != id);
            self.widgets.retain(|widget| widget.owner != id);
            self.current = WindowContext::EMPTY;
        }
        ok(Value::Bool(true))
    }

    fn next_event(&mut self) -> Result<Vec<Value>, ErrorValue> {
        self.present_dirty_windows()?;
        loop {
            if let Some(event) = self.pop_event()? {
                return self.dispatch(event);
            }
            match microsystem_user_rt::notification_wait(self.notification, 0) {
                Ok(_) => {}
                Err(Status::Busy | Status::TimedOut) => {
                    let _ = microsystem_user_rt::yield_now();
                }
                Err(status) => return Err(status_error("gui.wait", status)),
            }
        }
    }

    fn present_now(&mut self, arguments: &[Value]) -> Result<Vec<Value>, ErrorValue> {
        self.require_window(arguments.first())?;
        if self.current.dirty {
            self.present()?;
        }
        ok(Value::Bool(true))
    }

    fn present(&mut self) -> Result<(), ErrorValue> {
        self.sync_geometry()?;
        let root = self
            .current
            .root
            .ok_or_else(|| ErrorValue::new("state", "window root is not set"))?;
        let content = gui::Rect {
            x: 0,
            y: 0,
            width: self.current.width.saturating_sub(4),
            height: self.current.height.saturating_sub(30),
        };
        self.layout(root, content)?;
        let mut bytes = Vec::new();
        push_command(
            &mut bytes,
            gui::CommandKind::Clear,
            content,
            COLOR_BACKGROUND,
            0,
            &[],
        )?;
        self.render_widget(root, &mut bytes)?;
        let header = unsafe { &mut *self.command_base.cast::<gui::PresentHeaderV1>() };
        self.current.sequence = self.current.sequence.wrapping_add(1).max(1);
        *header = gui::PresentHeaderV1 {
            magic: gui::PRESENT_MAGIC,
            version: gui::VERSION,
            sequence: self.current.sequence,
            command_bytes: bytes.len() as u32,
            damage_count: 1,
            payload_hash: hash(&bytes),
            damage: core::array::from_fn(|index| {
                if index == 0 {
                    content
                } else {
                    gui::Rect::default()
                }
            }),
            ..gui::PresentHeaderV1::default()
        };
        unsafe {
            core::ptr::copy_nonoverlapping(
                bytes.as_ptr(),
                self.command_base.add(gui::COMMAND_HEADER_BYTES),
                bytes.len(),
            );
            microsystem_user_rt::fence();
        }
        let mut request = Message::new(protocol::GUI, gui::Operation::Present as u16);
        request.words[0] = self.current.id as u64;
        self.call_endpoint("gui.present", &request)?;
        self.current.dirty = false;
        if let Some(codepoint) = self.current.unicode_codepoint.take() {
            let _ = microsystem_user_rt::debug_write_u64(
                b"[mica] gui unicode rendered codepoint=",
                codepoint as u64,
                b" true\n",
            );
        }
        if !self.current.reported {
            let _ = microsystem_user_rt::debug_write(
                b"[mica] gui presented widgets=true atomic=true isolated=true\n",
            );
            self.current.reported = true;
        }
        Ok(())
    }

    fn sync_geometry(&mut self) -> Result<(), ErrorValue> {
        let mut request = Message::new(protocol::GUI, gui::Operation::QueryGeometry as u16);
        request.words[0] = self.current.id as u64;
        let reply = self.call_endpoint("gui.query_geometry", &request)?;
        self.current.width = (reply.words[2] as u32).clamp(180, gui::WIDTH);
        self.current.height = (reply.words[3] as u32).clamp(120, gui::HEIGHT);
        Ok(())
    }

    fn pop_event(&mut self) -> Result<Option<gui::Event>, ErrorValue> {
        let header = unsafe { &mut *self.event_base.cast::<gui::EventRingHeaderV1>() };
        if header.magic != gui::EVENT_MAGIC
            || header.version != gui::VERSION
            || header.capacity as usize != gui::EVENT_CAPACITY
            || header.head.saturating_sub(header.tail) > header.capacity as u32
        {
            return Err(ErrorValue::new("gui", "invalid GUI event ring"));
        }
        if header.tail == header.head {
            return Ok(None);
        }
        microsystem_user_rt::fence();
        let slot = header.tail % header.capacity as u32;
        let pointer = unsafe {
            self.event_base
                .add(
                    gui::EVENT_RING_HEADER_BYTES
                        + slot as usize * core::mem::size_of::<gui::Event>(),
                )
                .cast::<gui::Event>()
        };
        let event = unsafe { core::ptr::read_volatile(pointer) };
        header.tail = header.tail.wrapping_add(1);
        let mut request = Message::new(protocol::GUI, gui::Operation::EventConsumed as u16);
        request.words[0] = header.tail as u64;
        self.call_endpoint("gui.event_consumed", &request)?;
        Ok(Some(event))
    }

    fn dispatch(&mut self, event: gui::Event) -> Result<Vec<Value>, ErrorValue> {
        if self.select_window(event.window).is_err() {
            return Ok(alloc::vec![Value::Nil, Value::Nil, Value::Nil]);
        }
        let mut callback = None;
        let mut kind = "event";
        let mut event_number = event.words[0] as i64;
        match event.kind {
            value if value == gui::EventKind::CloseRequested as u16 => {
                let _ = microsystem_user_rt::debug_write(
                    b"[mica] gui close requested delivered=true\n",
                );
                kind = "close";
            }
            value if value == gui::EventKind::Configure as u16 => {
                let next_width = event.words[2].clamp(180, gui::WIDTH);
                let next_height = event.words[3].clamp(120, gui::HEIGHT);
                if next_width != self.current.width || next_height != self.current.height {
                    self.current.width = next_width;
                    self.current.height = next_height;
                    self.current.dirty = true;
                }
                kind = "configure";
            }
            value if value == gui::EventKind::Focus as u16 => {
                let active = event.words[0] != 0;
                if self.current.active != active {
                    self.current.active = active;
                    self.current.dirty = true;
                }
                kind = "focus";
            }
            value if value == gui::EventKind::PointerButton as u16 && event.words[1] == 1 => {
                if let Some(id) = self.hit_test(event.words[2] as i32, event.words[3] as i32) {
                    let _ = microsystem_user_rt::debug_write_u64(
                        b"[mica] gui pointer widget=",
                        id as u64,
                        b" delivered=true\n",
                    );
                    self.current.focused = Some(id);
                    let widget = self.widget_mut(id)?;
                    kind = "click";
                    callback = widget.on_click.clone();
                    if widget.kind == WidgetKind::TextInput {
                        widget.cursor = widget.text.len();
                        widget.selected_all = false;
                        self.current.dirty = true;
                    } else if widget.kind == WidgetKind::Checkbox {
                        widget.checked = !widget.checked;
                        callback = widget.on_change.clone().or(callback);
                        self.current.dirty = true;
                    } else if widget.kind == WidgetKind::List {
                        let row = event.words[3]
                            .saturating_sub(widget.rect.y.max(0) as u32)
                            .saturating_sub(4)
                            / 24;
                        if (row as usize) < widget.items.len() {
                            widget.selected = row as usize;
                            callback = widget.on_select.clone().or(callback);
                            self.current.dirty = true;
                            kind = "select";
                            event_number = row as i64 + 1;
                        }
                    }
                }
            }
            value if value == gui::EventKind::PointerWheel as u16 => {
                if let Some(id) = self.scroll_at(event.words[1] as i32, event.words[2] as i32) {
                    let limit = self.scroll_limit(id)?;
                    let widget = self.widget_mut(id)?;
                    widget.scroll = widget
                        .scroll
                        .saturating_sub(event.words[0] as i32)
                        .clamp(0, limit);
                    self.current.dirty = true;
                    kind = "scroll";
                }
            }
            value if value == gui::EventKind::TextInput as u16 => {
                if let Some(id) = self.current.focused {
                    let widget = self.widget_mut(id)?;
                    if widget.kind == WidgetKind::TextInput
                        && let Some(character) = char::from_u32(event.words[0])
                    {
                        if widget.selected_all {
                            widget.text.clear();
                            widget.cursor = 0;
                            widget.selected_all = false;
                        }
                        let cursor = widget.cursor.min(widget.text.len());
                        widget.text.insert(cursor, character);
                        widget.cursor = cursor + character.len_utf8();
                        callback = widget.on_change.clone();
                        let unicode = (event.words[0] > 0x7f).then_some(event.words[0]);
                        let _ = widget;
                        if unicode.is_some() {
                            self.current.unicode_codepoint = unicode;
                        }
                        self.current.dirty = true;
                        kind = "change";
                    }
                }
            }
            value if value == gui::EventKind::Key as u16 && event.words[1] == 1 => {
                let key = event.words[0] as u16;
                let control = event.words[2] & 1 != 0;
                if key == 15 {
                    self.focus_next();
                    self.current.dirty = true;
                    kind = "focus";
                } else if key == 28 {
                    if let Some(id) = self.current.focused {
                        callback = self.widget(id)?.on_submit.clone();
                        kind = "submit";
                    }
                } else if let Some(id) = self.current.focused {
                    if control
                        && matches!(key, 45 | 46 | 47)
                        && self.widget(id)?.kind == WidgetKind::TextInput
                    {
                        if self.edit_clipboard(id, key)? {
                            callback = self.widget(id)?.on_change.clone();
                            self.current.dirty = true;
                            kind = "change";
                        } else {
                            kind = "clipboard";
                        }
                    } else {
                        let widget = self.widget_mut(id)?;
                        if widget.kind == WidgetKind::TextInput && control && key == 30 {
                            if !widget.text.is_empty() && !widget.selected_all {
                                widget.selected_all = true;
                                self.current.dirty = true;
                                kind = "selection";
                            }
                        } else if widget.kind == WidgetKind::TextInput && matches!(key, 14 | 111) {
                            let had_selection = widget.selected_all;
                            let text_changed = if had_selection {
                                widget.text.clear();
                                widget.cursor = 0;
                                widget.selected_all = false;
                                true
                            } else if key == 14 && widget.cursor > 0 {
                                let start = previous_char_boundary(&widget.text, widget.cursor);
                                widget.text.drain(start..widget.cursor);
                                widget.cursor = start;
                                true
                            } else if key == 111 && widget.cursor < widget.text.len() {
                                let end = next_char_boundary(&widget.text, widget.cursor);
                                widget.text.drain(widget.cursor..end);
                                true
                            } else {
                                false
                            };
                            if had_selection || text_changed {
                                if text_changed {
                                    callback = widget.on_change.clone();
                                }
                                self.current.dirty = true;
                                kind = if text_changed { "change" } else { "selection" };
                            }
                        } else if widget.kind == WidgetKind::TextInput
                            && matches!(key, 102 | 105 | 106 | 107)
                        {
                            let next = if widget.selected_all {
                                if matches!(key, 102 | 105) {
                                    0
                                } else {
                                    widget.text.len()
                                }
                            } else {
                                match key {
                                    102 => 0,
                                    107 => widget.text.len(),
                                    105 => previous_char_boundary(&widget.text, widget.cursor),
                                    106 => next_char_boundary(&widget.text, widget.cursor),
                                    _ => widget.cursor,
                                }
                            };
                            if next != widget.cursor || widget.selected_all {
                                widget.cursor = next;
                                widget.selected_all = false;
                                self.current.dirty = true;
                                kind = "cursor";
                            }
                        }
                    }
                }
            }
            _ => {}
        }
        let mut event_value = object(
            "window",
            event.window,
            &[("kind", Value::String(kind.to_string()))],
        );
        if let Value::Table(entries) = &mut event_value {
            entries.push((Value::String("value".into()), Value::Integer(event_number)));
            if let Some(id) = self.current.focused {
                entries.push((
                    Value::String("text".into()),
                    Value::String(self.widget(id)?.text.clone()),
                ));
            }
        }
        Ok(alloc::vec![
            callback.unwrap_or(Value::Nil),
            event_value,
            Value::Nil
        ])
    }

    fn layout(&mut self, id: u32, rect: gui::Rect) -> Result<(), ErrorValue> {
        let index = self.widget_index(id)?;
        self.widgets[index].rect = rect;
        let kind = self.widgets[index].kind;
        if !matches!(
            kind,
            WidgetKind::Row | WidgetKind::Column | WidgetKind::Scroll
        ) {
            return Ok(());
        }
        let children = self.widgets[index].children.clone();
        let padding = self.widgets[index].padding;
        let gap = self.widgets[index].gap;
        let inner = inset(rect, padding);
        let horizontal = kind == WidgetKind::Row;
        let main = if horizontal {
            inner.width
        } else {
            inner.height
        };
        let gaps = gap.saturating_mul(children.len().saturating_sub(1) as u32);
        let mut fixed = 0u32;
        let mut grow = 0u32;
        for child in &children {
            let widget = self.widget(*child)?;
            let extent = if horizontal {
                widget.width
            } else {
                widget.height
            };
            if widget.grow == 0 {
                fixed = fixed.saturating_add(extent.unwrap_or(32));
            } else {
                grow = grow.saturating_add(widget.grow);
            }
        }
        let remaining = main.saturating_sub(fixed.saturating_add(gaps));
        let mut cursor = if horizontal {
            inner.x
        } else {
            inner.y.saturating_sub(self.widgets[index].scroll.max(0))
        };
        for child in children {
            let widget = self.widget(child)?;
            let extent = if widget.grow == 0 {
                (if horizontal {
                    widget.width
                } else {
                    widget.height
                })
                .unwrap_or(32)
            } else {
                remaining.saturating_mul(widget.grow) / grow.max(1)
            };
            let child_rect = if horizontal {
                gui::Rect {
                    x: cursor,
                    y: inner.y,
                    width: extent,
                    height: self
                        .widget(child)?
                        .height
                        .unwrap_or(inner.height)
                        .min(inner.height),
                }
            } else {
                gui::Rect {
                    x: inner.x,
                    y: cursor,
                    width: self
                        .widget(child)?
                        .width
                        .unwrap_or(inner.width)
                        .min(inner.width),
                    height: extent,
                }
            };
            self.layout(child, child_rect)?;
            cursor = cursor.saturating_add(extent as i32 + gap as i32);
        }
        Ok(())
    }

    fn render_widget(&self, id: u32, output: &mut Vec<u8>) -> Result<(), ErrorValue> {
        let widget = self.widget(id)?;
        match widget.kind {
            WidgetKind::Label => push_text(output, widget.rect, &widget.text, COLOR_TEXT)?,
            WidgetKind::Button => {
                push_command(
                    output,
                    gui::CommandKind::FillRect,
                    widget.rect,
                    COLOR_BUTTON,
                    0,
                    &[],
                )?;
                push_command(
                    output,
                    gui::CommandKind::FillRect,
                    gui::Rect {
                        height: 2,
                        ..widget.rect
                    },
                    0x00f8_fafd,
                    0,
                    &[],
                )?;
                push_command(
                    output,
                    gui::CommandKind::StrokeRect,
                    widget.rect,
                    if self.current.active && self.current.focused == Some(id) {
                        COLOR_ACCENT
                    } else {
                        COLOR_BORDER
                    },
                    if self.current.active && self.current.focused == Some(id) {
                        2
                    } else {
                        1
                    },
                    &[],
                )?;
                push_text(output, inset(widget.rect, 8), &widget.text, COLOR_TEXT)?;
            }
            WidgetKind::TextInput => {
                push_command(
                    output,
                    gui::CommandKind::FillRect,
                    widget.rect,
                    COLOR_INPUT,
                    0,
                    &[],
                )?;
                push_command(
                    output,
                    gui::CommandKind::StrokeRect,
                    widget.rect,
                    if self.current.active && self.current.focused == Some(id) {
                        COLOR_ACCENT
                    } else {
                        COLOR_BORDER
                    },
                    if self.current.active && self.current.focused == Some(id) {
                        2
                    } else {
                        1
                    },
                    &[],
                )?;
                let focused = self.current.active && self.current.focused == Some(id);
                let text_rect = inset(widget.rect, 6);
                if focused && widget.selected_all {
                    push_command(
                        output,
                        gui::CommandKind::FillRect,
                        text_rect,
                        COLOR_SELECTION,
                        0,
                        &[],
                    )?;
                }
                let text = text_input_display(widget, focused);
                push_text(
                    output,
                    text_rect,
                    &text,
                    if widget.text.is_empty() {
                        COLOR_MUTED
                    } else {
                        COLOR_TEXT
                    },
                )?;
            }
            WidgetKind::Checkbox => {
                let box_rect = gui::Rect {
                    width: 20,
                    height: 20,
                    ..widget.rect
                };
                push_command(
                    output,
                    gui::CommandKind::FillRect,
                    box_rect,
                    COLOR_INPUT,
                    0,
                    &[],
                )?;
                push_command(
                    output,
                    gui::CommandKind::StrokeRect,
                    box_rect,
                    if self.current.active && self.current.focused == Some(id) {
                        COLOR_ACCENT
                    } else {
                        COLOR_BORDER
                    },
                    2,
                    &[],
                )?;
                if widget.checked {
                    push_command(
                        output,
                        gui::CommandKind::FillRect,
                        inset(box_rect, 4),
                        COLOR_ACCENT,
                        0,
                        &[],
                    )?;
                }
                let text_rect = gui::Rect {
                    x: widget.rect.x + 28,
                    ..widget.rect
                };
                push_text(output, text_rect, &widget.text, COLOR_TEXT)?;
            }
            WidgetKind::List => {
                push_command(output, gui::CommandKind::SetClip, widget.rect, 0, 0, &[])?;
                push_command(
                    output,
                    gui::CommandKind::FillRect,
                    widget.rect,
                    COLOR_INPUT,
                    0,
                    &[],
                )?;
                push_command(
                    output,
                    gui::CommandKind::StrokeRect,
                    widget.rect,
                    if self.current.active && self.current.focused == Some(id) {
                        COLOR_ACCENT
                    } else {
                        COLOR_BORDER
                    },
                    if self.current.active && self.current.focused == Some(id) {
                        2
                    } else {
                        1
                    },
                    &[],
                )?;
                for (index, item) in widget.items.iter().enumerate() {
                    let row = gui::Rect {
                        x: widget.rect.x + 4,
                        y: widget.rect.y + 4 + index as i32 * 24,
                        width: widget.rect.width.saturating_sub(8),
                        height: 22,
                    };
                    if index == widget.selected {
                        push_command(
                            output,
                            gui::CommandKind::FillRect,
                            row,
                            COLOR_SELECTION,
                            0,
                            &[],
                        )?;
                    }
                    push_text(output, row, item, COLOR_TEXT)?;
                }
            }
            WidgetKind::Canvas => {
                push_command(output, gui::CommandKind::SetClip, widget.rect, 0, 0, &[])?;
                push_command(
                    output,
                    gui::CommandKind::FillRect,
                    widget.rect,
                    COLOR_PANEL,
                    0,
                    &[],
                )?;
                push_command(
                    output,
                    gui::CommandKind::StrokeRect,
                    widget.rect,
                    COLOR_BORDER,
                    1,
                    &[],
                )?;
                for command in &widget.canvas {
                    let candidate = gui::Rect {
                        x: widget.rect.x.saturating_add(command.rect.x),
                        y: widget.rect.y.saturating_add(command.rect.y),
                        width: command.rect.width,
                        height: command.rect.height,
                    };
                    let Some(rect) = intersect(candidate, widget.rect) else {
                        continue;
                    };
                    push_command(
                        output,
                        command.kind,
                        rect,
                        command.color,
                        command.argument,
                        command.text.as_bytes(),
                    )?;
                }
            }
            WidgetKind::Row | WidgetKind::Column | WidgetKind::Scroll => {
                for child in &widget.children {
                    push_command(output, gui::CommandKind::SetClip, widget.rect, 0, 0, &[])?;
                    self.render_widget(*child, output)?;
                }
            }
            WidgetKind::Spacer => {}
        }
        Ok(())
    }

    fn hit_test(&self, x: i32, y: i32) -> Option<u32> {
        self.widgets.iter().rev().find_map(|widget| {
            (widget.owner == self.current.id && widget.interactive() && contains(widget.rect, x, y))
                .then_some(widget.id)
        })
    }

    fn scroll_at(&self, x: i32, y: i32) -> Option<u32> {
        self.widgets.iter().rev().find_map(|widget| {
            (widget.owner == self.current.id
                && widget.kind == WidgetKind::Scroll
                && contains(widget.rect, x, y))
            .then_some(widget.id)
        })
    }

    fn scroll_limit(&self, id: u32) -> Result<i32, ErrorValue> {
        let widget = self.widget(id)?;
        let content = widget.children.iter().try_fold(0u32, |height, child| {
            Ok::<_, ErrorValue>(height.saturating_add(self.widget(*child)?.height.unwrap_or(32)))
        })?;
        let gaps = widget
            .gap
            .saturating_mul(widget.children.len().saturating_sub(1) as u32);
        let content = content
            .saturating_add(gaps)
            .saturating_add(widget.padding.saturating_mul(2));
        Ok(content.saturating_sub(widget.rect.height) as i32)
    }

    fn focus_next(&mut self) {
        let interactive: Vec<u32> = self
            .widgets
            .iter()
            .filter_map(|widget| {
                (widget.owner == self.current.id && widget.interactive()).then_some(widget.id)
            })
            .collect();
        if interactive.is_empty() {
            self.current.focused = None;
            return;
        }
        let next = self
            .current
            .focused
            .and_then(|focused| interactive.iter().position(|id| *id == focused))
            .map(|index| (index + 1) % interactive.len())
            .unwrap_or(0);
        self.current.focused = Some(interactive[next]);
    }

    fn call_endpoint(&self, operation: &str, request: &Message) -> Result<Message, ErrorValue> {
        let mut reply = Message::new(0, 0);
        microsystem_user_rt::ipc_call(self.endpoint, request, &mut reply, 0)
            .map_err(|status| status_error(operation, status))?;
        let status = status_from_raw(reply.words[5] as i64);
        if status != Status::Ok {
            return Err(status_error(operation, status));
        }
        Ok(reply)
    }

    fn write_header_title(&mut self, title: &str) -> Result<(), ErrorValue> {
        let header = unsafe { &mut *self.command_base.cast::<gui::PresentHeaderV1>() };
        let mut length = title.len().min(header.title.len());
        while !title.is_char_boundary(length) {
            length -= 1;
        }
        header.magic = gui::PRESENT_MAGIC;
        header.version = gui::VERSION;
        header.title.fill(0);
        header.title[..length].copy_from_slice(&title.as_bytes()[..length]);
        header.title_bytes = length as u16;
        Ok(())
    }

    fn require_window(&mut self, value: Option<&Value>) -> Result<(), ErrorValue> {
        let id = object_id(
            value.ok_or_else(|| ErrorValue::new("argument", "window is required"))?,
            "_window",
        )?;
        self.select_window(id)
    }

    fn save_current(&mut self) {
        if let Some(saved) = self
            .windows
            .iter_mut()
            .find(|window| window.id == self.current.id)
        {
            *saved = self.current;
        }
    }

    fn select_window(&mut self, id: u32) -> Result<(), ErrorValue> {
        if id != 0 && id == self.current.id {
            return Ok(());
        }
        self.save_current();
        self.current = *self
            .windows
            .iter()
            .find(|window| id != 0 && window.id == id)
            .ok_or_else(|| ErrorValue::new("access", "window does not belong to this session"))?;
        Ok(())
    }

    fn present_dirty_windows(&mut self) -> Result<(), ErrorValue> {
        self.save_current();
        let ids: Vec<u32> = self
            .windows
            .iter()
            .filter(|window| window.dirty && window.root.is_some())
            .map(|window| window.id)
            .collect();
        for id in ids {
            self.select_window(id)?;
            self.present()?;
        }
        Ok(())
    }

    fn bind_tree(&mut self, root: u32) -> Result<(), ErrorValue> {
        let mut pending = alloc::vec![root];
        let mut visited = Vec::new();
        while let Some(id) = pending.pop() {
            if visited.contains(&id) {
                return Err(ErrorValue::new(
                    "argument",
                    "widget tree contains repeated children",
                ));
            }
            let widget = self.widget(id)?;
            if widget.owner != 0 && widget.owner != self.current.id {
                return Err(ErrorValue::new(
                    "access",
                    "widget belongs to another window",
                ));
            }
            pending.extend_from_slice(&widget.children);
            visited.push(id);
        }
        for id in visited {
            let index = self.widget_index(id)?;
            self.widgets[index].owner = self.current.id;
        }
        Ok(())
    }

    fn widget_index(&self, id: u32) -> Result<usize, ErrorValue> {
        self.widgets
            .iter()
            .position(|widget| widget.id == id)
            .ok_or_else(|| ErrorValue::new("argument", "unknown widget"))
    }

    fn clipboard_write(&self, text: &str) -> Result<(), ErrorValue> {
        if text.len() > gui::CLIPBOARD_BYTES {
            return Err(ErrorValue::new(
                "limit",
                "clipboard text exceeds 4096 bytes",
            ));
        }
        unsafe {
            core::ptr::copy_nonoverlapping(
                text.as_ptr(),
                self.command_base.add(gui::COMMAND_HEADER_BYTES),
                text.len(),
            );
        }
        microsystem_user_rt::fence();
        let mut request = Message::new(protocol::GUI, gui::Operation::ClipboardWrite as u16);
        request.words[0] = text.len() as u64;
        self.call_endpoint("gui.clipboard_write", &request)?;
        Ok(())
    }

    fn clipboard_read(&self) -> Result<String, ErrorValue> {
        let request = Message::new(protocol::GUI, gui::Operation::ClipboardRead as u16);
        let reply = self.call_endpoint("gui.clipboard_read", &request)?;
        let length = reply.words[0] as usize;
        if length > gui::CLIPBOARD_BYTES {
            return Err(ErrorValue::new("gui", "invalid clipboard length"));
        }
        microsystem_user_rt::fence();
        let bytes = unsafe {
            core::slice::from_raw_parts(self.command_base.add(gui::COMMAND_HEADER_BYTES), length)
        };
        core::str::from_utf8(bytes)
            .map(ToString::to_string)
            .map_err(|_| ErrorValue::new("gui", "invalid clipboard text"))
    }

    fn edit_clipboard(&mut self, id: u32, key: u16) -> Result<bool, ErrorValue> {
        if key != 47 {
            let widget = self.widget(id)?;
            if !widget.selected_all {
                return Ok(false);
            }
            self.clipboard_write(&widget.text)?;
            if key == 46 {
                return Ok(false);
            }
            let widget = self.widget_mut(id)?;
            widget.text.clear();
            widget.cursor = 0;
            widget.selected_all = false;
            return Ok(true);
        }
        let text = self.clipboard_read()?;
        if text.is_empty() {
            return Ok(false);
        }
        let widget = self.widget_mut(id)?;
        let retained = if widget.selected_all {
            0
        } else {
            widget.text.len()
        };
        if retained + text.len() > gui::CLIPBOARD_BYTES {
            return Err(ErrorValue::new("limit", "input text exceeds 4096 bytes"));
        }
        if widget.selected_all {
            widget.text.clear();
            widget.cursor = 0;
        }
        widget.text.insert_str(widget.cursor, &text);
        widget.cursor += text.len();
        widget.selected_all = false;
        Ok(true)
    }

    fn widget(&self, id: u32) -> Result<&Widget, ErrorValue> {
        self.widgets
            .get(self.widget_index(id)?)
            .ok_or_else(|| ErrorValue::new("argument", "unknown widget"))
    }

    fn widget_mut(&mut self, id: u32) -> Result<&mut Widget, ErrorValue> {
        let index = self.widget_index(id)?;
        let owner = self.widgets[index].owner;
        if owner != 0 {
            self.select_window(owner)?;
        }
        self.widgets
            .get_mut(index)
            .ok_or_else(|| ErrorValue::new("argument", "unknown widget"))
    }
}

fn object(key: &str, id: u32, fields: &[(&str, Value)]) -> Value {
    let mut values = alloc::vec![(Value::String(key.into()), Value::Integer(id as i64))];
    values.extend(
        fields
            .iter()
            .map(|(key, value)| (Value::String((*key).into()), value.clone())),
    );
    Value::Table(values)
}

fn object_id(value: &Value, key: &str) -> Result<u32, ErrorValue> {
    match value.table_get(&Value::String(key.into())) {
        Some(Value::Integer(value)) => {
            u32::try_from(*value).map_err(|_| ErrorValue::new("argument", "invalid GUI object"))
        }
        _ => Err(ErrorValue::new("type", "expected GUI object")),
    }
}

fn table_value<'a>(value: &'a Value, key: &str) -> Option<&'a Value> {
    value.table_get(&Value::String(key.into()))
}

fn table_string(value: &Value, key: &str) -> Option<String> {
    match table_value(value, key) {
        Some(Value::String(value)) => Some(value.clone()),
        _ => None,
    }
}

fn table_u32(value: &Value, key: &str) -> Option<u32> {
    match table_value(value, key) {
        Some(Value::Integer(value)) => u32::try_from(*value).ok(),
        _ => None,
    }
}

fn table_i32(value: &Value, key: &str) -> Option<i32> {
    match table_value(value, key) {
        Some(Value::Integer(value)) => i32::try_from(*value).ok(),
        _ => None,
    }
}

fn table_bool(value: &Value, key: &str) -> bool {
    matches!(table_value(value, key), Some(Value::Bool(true)))
}

fn table_callback(value: &Value, key: &str) -> Option<Value> {
    match table_value(value, key) {
        Some(value @ (Value::Function(_) | Value::Native(_))) => Some(value.clone()),
        _ => None,
    }
}

fn parse_canvas_commands(spec: &Value) -> Result<Vec<CanvasCommand>, ErrorValue> {
    let Some(Value::Table(commands)) = table_value(spec, "commands") else {
        return Ok(Vec::new());
    };
    let mut output = Vec::new();
    let mut text_bytes = 0usize;
    for (_, value) in commands.iter().take(gui::MAX_COMMANDS) {
        let kind = match table_string(value, "kind").as_deref() {
            Some("clear") => gui::CommandKind::Clear,
            Some("fill_rect") => gui::CommandKind::FillRect,
            Some("stroke_rect") => gui::CommandKind::StrokeRect,
            Some("text") => gui::CommandKind::Text,
            Some("icon") => gui::CommandKind::Icon,
            Some("set_clip") => gui::CommandKind::SetClip,
            Some(_) => return Err(ErrorValue::new("argument", "unknown canvas command")),
            None => {
                return Err(ErrorValue::new(
                    "argument",
                    "canvas command kind is required",
                ));
            }
        };
        let text = table_string(value, "text").unwrap_or_default();
        text_bytes = text_bytes.saturating_add(text.len());
        if text_bytes > gui::MAX_TEXT_BYTES {
            return Err(ErrorValue::new("limit", "canvas text exceeds 48 KiB"));
        }
        let rect = gui::Rect {
            x: table_i32(value, "x").unwrap_or(0),
            y: table_i32(value, "y").unwrap_or(0),
            width: table_u32(value, "width").unwrap_or(1),
            height: table_u32(value, "height").unwrap_or(1),
        };
        if rect.x < 0
            || rect.y < 0
            || rect.width == 0
            || rect.height == 0
            || rect.width > gui::WIDTH
            || rect.height > gui::HEIGHT
        {
            return Err(ErrorValue::new("argument", "invalid canvas rectangle"));
        }
        output.push(CanvasCommand {
            kind,
            rect,
            color: table_u32(value, "color").unwrap_or(COLOR_TEXT),
            argument: table_u32(value, "thickness").unwrap_or(1),
            text,
        });
    }
    Ok(output)
}

fn string_argument(arguments: &[Value], index: usize) -> Result<&str, ErrorValue> {
    match arguments.get(index) {
        Some(Value::String(value)) => Ok(value),
        _ => Err(ErrorValue::new("type", "expected string")),
    }
}

fn ok(value: Value) -> Result<Vec<Value>, ErrorValue> {
    Ok(alloc::vec![value, Value::Nil])
}

fn system_error(error: ErrorValue) -> Result<Vec<Value>, ErrorValue> {
    Ok(alloc::vec![Value::Nil, error_value(error)])
}

fn error_value(error: ErrorValue) -> Value {
    Value::Table(alloc::vec![
        (Value::String("kind".into()), Value::String(error.kind)),
        (
            Value::String("message".into()),
            Value::String(error.message)
        ),
        (
            Value::String("operation".into()),
            Value::String(error.operation)
        ),
        (Value::String("code".into()), Value::Integer(error.code)),
    ])
}

fn status_error(operation: &str, status: Status) -> ErrorValue {
    ErrorValue::new("system", "GUI service rejected operation")
        .operation(operation)
        .code(status as i64)
}

fn status_from_raw(value: i64) -> Status {
    match value {
        0 => Status::Ok,
        -1 => Status::Invalid,
        -2 => Status::BadCapability,
        -3 => Status::AccessDenied,
        -4 => Status::NotFound,
        -5 => Status::NoMemory,
        -6 => Status::Busy,
        -7 => Status::TimedOut,
        -8 => Status::Fault,
        -9 => Status::NotSupported,
        -10 => Status::Io,
        -11 => Status::NoSpace,
        _ => Status::Corrupt,
    }
}

fn inset(rect: gui::Rect, inset: u32) -> gui::Rect {
    let inset = inset.min(rect.width / 2).min(rect.height / 2);
    gui::Rect {
        x: rect.x + inset as i32,
        y: rect.y + inset as i32,
        width: rect.width.saturating_sub(inset * 2),
        height: rect.height.saturating_sub(inset * 2),
    }
}

fn contains(rect: gui::Rect, x: i32, y: i32) -> bool {
    x >= rect.x
        && y >= rect.y
        && x < rect.x.saturating_add(rect.width as i32)
        && y < rect.y.saturating_add(rect.height as i32)
}

fn intersect(left: gui::Rect, right: gui::Rect) -> Option<gui::Rect> {
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

fn previous_char_boundary(text: &str, cursor: usize) -> usize {
    text[..cursor.min(text.len())]
        .char_indices()
        .next_back()
        .map_or(0, |(index, _)| index)
}

fn next_char_boundary(text: &str, cursor: usize) -> usize {
    let cursor = cursor.min(text.len());
    text[cursor..]
        .char_indices()
        .nth(1)
        .map_or(text.len(), |(offset, _)| cursor + offset)
}

fn text_input_display(widget: &Widget, focused: bool) -> Cow<'_, str> {
    if !focused {
        return Cow::Borrowed(if widget.text.is_empty() {
            &widget.placeholder
        } else {
            &widget.text
        });
    }
    if widget.selected_all {
        return Cow::Borrowed(&widget.text);
    }

    let visible_chars = (widget.rect.width.saturating_sub(12) / 16).max(1) as usize;
    if widget.text.is_empty() {
        let mut display = String::from("|");
        display.extend(
            widget
                .placeholder
                .chars()
                .take(visible_chars.saturating_sub(1)),
        );
        return Cow::Owned(display);
    }

    let cursor = widget.cursor.min(widget.text.len());
    let cursor_chars = widget.text[..cursor].chars().count();
    let show_ellipsis = visible_chars > 1 && cursor_chars >= visible_chars;
    let before_capacity = visible_chars.saturating_sub(1 + if show_ellipsis { 1 } else { 0 });
    let start_chars = cursor_chars.saturating_sub(before_capacity);
    let start = widget
        .text
        .char_indices()
        .nth(start_chars)
        .map_or(0, |(index, _)| index);
    let mut display = String::new();
    if show_ellipsis && start > 0 {
        display.push('…');
    }
    display.push_str(&widget.text[start..cursor]);
    display.push('|');
    let remaining = visible_chars.saturating_sub(display.chars().count());
    display.extend(widget.text[cursor..].chars().take(remaining));
    Cow::Owned(display)
}

fn push_text(
    output: &mut Vec<u8>,
    rect: gui::Rect,
    text: &str,
    color: u32,
) -> Result<(), ErrorValue> {
    push_command(
        output,
        gui::CommandKind::Text,
        rect,
        color,
        0,
        text.as_bytes(),
    )
}

fn push_command(
    output: &mut Vec<u8>,
    kind: gui::CommandKind,
    rect: gui::Rect,
    color: u32,
    argument: u32,
    payload: &[u8],
) -> Result<(), ErrorValue> {
    let bytes = core::mem::size_of::<gui::DrawCommand>().saturating_add(payload.len());
    if output.len().saturating_add(bytes) > gui::COMMAND_PAYLOAD_BYTES {
        return Err(ErrorValue::new("limit", "GUI display list exceeds 64 KiB"));
    }
    let command = gui::DrawCommand {
        header: gui::CommandHeader {
            kind: kind as u16,
            flags: 0,
            bytes: bytes as u32,
        },
        rect,
        color,
        argument,
    };
    let raw = unsafe {
        core::slice::from_raw_parts(
            (&command as *const gui::DrawCommand).cast::<u8>(),
            core::mem::size_of::<gui::DrawCommand>(),
        )
    };
    output.extend_from_slice(raw);
    output.extend_from_slice(payload);
    Ok(())
}

fn hash(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325u64, |hash, byte| {
        (hash ^ *byte as u64).wrapping_mul(0x100_0000_01b3)
    })
}
