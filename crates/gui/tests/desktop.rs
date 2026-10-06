use microsystem_abi::{Status, gui};
use microsystem_gui::{
    DESKTOP_ICON_SIZE, DESKTOP_ICON_TOP, DESKTOP_ICONS, Desktop, MIN_HEIGHT, MIN_WIDTH,
    TASKBAR_HEIGHT, WindowState, desktop_icon_at,
};

fn rect(x: i32, y: i32, width: u32, height: u32) -> gui::Rect {
    gui::Rect {
        x,
        y,
        width,
        height,
    }
}

fn ids(desktop: &Desktop) -> Vec<u32> {
    desktop.z_order().map(|window| window.id).collect()
}

#[test]
fn create_clamps_geometry_and_bounds_title_storage() {
    let mut desktop = Desktop::new();
    let title = [b'X'; 80];
    let id = desktop
        .create_window(1, rect(-50, -20, 1, 1), &title)
        .expect("valid positive geometry should be clamped");
    let window = desktop.window(id).expect("window exists");

    assert_eq!(window.rect, rect(0, 0, MIN_WIDTH as u32, MIN_HEIGHT as u32));
    assert_eq!(window.restore, window.rect);
    assert_eq!(window.title_len as usize, microsystem_gui::TITLE_BYTES);
    assert_eq!(&window.title[..window.title_len as usize], &title[..64]);
    assert_eq!(desktop.focused(), Some(id));
}

#[test]
fn title_truncation_never_splits_a_utf8_codepoint() {
    let mut desktop = Desktop::new();
    let title = format!("{}界", "a".repeat(microsystem_gui::TITLE_BYTES - 1));
    let id = desktop
        .create_window(1, rect(20, 20, 320, 220), title.as_bytes())
        .unwrap();
    let window = desktop.window(id).unwrap();
    assert!(core::str::from_utf8(&window.title[..window.title_len as usize]).is_ok());

    desktop.set_title(id, title.as_bytes()).unwrap();
    let window = desktop.window(id).unwrap();
    assert!(core::str::from_utf8(&window.title[..window.title_len as usize]).is_ok());
}

#[test]
fn rejects_invalid_titles_and_geometry_without_allocating_slots() {
    let mut desktop = Desktop::new();
    assert_eq!(
        desktop.create_window(1, rect(0, 0, 320, 200), &[0xff]),
        Err(Status::Invalid)
    );
    assert_eq!(
        desktop.create_window(1, rect(0, 0, 0, 200), b"zero-width"),
        Err(Status::Invalid)
    );
    assert_eq!(desktop.z_order().count(), 0);
}

#[test]
fn desktop_capacity_covers_builtin_and_multiple_application_windows() {
    let mut desktop = Desktop::new();
    for client in 1..=3 {
        desktop
            .create_window(client, rect(20, 20, 320, 220), b"window")
            .unwrap();
    }
    for client in 4..4 + gui::MAX_DYNAMIC_CLIENTS as u8 {
        for _ in 0..gui::MAX_CLIENT_WINDOWS {
            desktop.create_window(client, rect(20, 20, 320, 220), b"window").unwrap();
        }
    }
    assert_eq!(
        desktop.create_window(12, rect(20, 20, 320, 220), b"overflow"),
        Err(Status::Invalid)
    );
    let removed = desktop.window_for_client(4).unwrap().id;
    desktop.remove_window(removed).unwrap();
    let replacement = desktop.create_window(4, rect(20, 20, 320, 220), b"replacement").unwrap();
    assert_ne!(replacement, removed);
    assert_eq!(desktop.z_order().filter(|window| window.client == 4).count(), gui::MAX_CLIENT_WINDOWS);
}

#[test]
fn composition_commits_selected_utf8_and_discards_on_focus_change() {
    use microsystem_gui::composition::{Composition, Input};
    let mut input = Composition::new("ni\t你\nni\t尼\nnihao\t你好\n");
    input.focus(1);
    assert!(matches!(input.key(57, true, true, false, false, None), Input::Consumed));
    for letter in b"ni" { input.key(30, true, false, false, false, Some(*letter)); }
    assert_eq!(input.candidates()[..2], ["你", "尼"]);
    let Input::Commit { text, length } = input.key(3, true, false, false, false, None) else { panic!("candidate missing"); };
    assert_eq!(core::str::from_utf8(&text[..length]).unwrap(), "尼");
    for letter in b"nihao" { input.key(30, true, false, false, false, Some(*letter)); }
    assert!(input.focus(2));
    assert!(input.text().is_empty());
    assert!(matches!(input.key(28, true, false, false, false, None), Input::Pass));
    for letter in b"nihao" { input.key(30, true, false, false, false, Some(*letter)); }
    input.key(14, true, false, false, false, None);
    assert_eq!(input.text(), "niha");
    input.key(1, true, false, false, false, None);
    assert!(input.text().is_empty());
}

#[test]
fn composition_bounds_preedit_and_ignores_invalid_dictionary_entries() {
    use microsystem_gui::composition::{Composition, Input};
    let mut input = Composition::new("a\t123456789\na\t\na\t啊\nmalformed\n");
    input.key(57, true, true, false, false, None);
    input.key(30, true, false, false, false, Some(b'a'));
    assert_eq!(input.candidates()[0], "啊");
    assert!(matches!(input.key(46, true, true, false, false, Some(b'c')), Input::Pass));
    for _ in 0..40 { input.key(30, true, false, false, false, Some(b'a')); }
    assert_eq!(input.text().len(), 32);
    let Input::Commit { text, length } = input.key(28, true, false, false, false, None) else { panic!("raw preedit missing"); };
    assert_eq!(&text[..length], &[b'a'; 32]);
}

#[test]
fn focus_reorders_windows_and_alt_tab_cycles_visible_windows() {
    let mut desktop = Desktop::new();
    let first = desktop
        .create_window(1, rect(20, 20, 320, 220), b"first")
        .unwrap();
    let second = desktop
        .create_window(2, rect(40, 40, 320, 220), b"second")
        .unwrap();
    let third = desktop
        .create_window(3, rect(60, 60, 320, 220), b"third")
        .unwrap();
    assert_eq!(ids(&desktop), vec![first, second, third]);

    desktop.focus(first).unwrap();
    assert_eq!(ids(&desktop), vec![second, third, first]);
    assert_eq!(desktop.focused(), Some(first));
    assert_eq!(desktop.alt_tab(), Some(second));
    assert_eq!(desktop.focused(), Some(second));
    assert_eq!(desktop.alt_tab(), Some(third));
    assert_eq!(desktop.focused(), Some(third));
}

#[test]
fn hit_test_uses_topmost_visible_window_and_skips_minimized_or_closed_windows() {
    let mut desktop = Desktop::new();
    let bottom = desktop
        .create_window(1, rect(20, 20, 320, 220), b"bottom")
        .unwrap();
    let top = desktop
        .create_window(2, rect(40, 40, 320, 220), b"top")
        .unwrap();

    assert_eq!(desktop.hit_test(80, 80), Some(top));
    desktop.action(top, gui::WindowAction::Minimize).unwrap();
    assert_eq!(desktop.hit_test(80, 80), Some(bottom));
    desktop.action(bottom, gui::WindowAction::Close).unwrap();
    assert_eq!(desktop.hit_test(80, 80), None);
}

#[test]
fn move_resize_and_state_actions_preserve_restore_geometry() {
    let mut desktop = Desktop::new();
    let id = desktop
        .create_window(1, rect(100, 100, 320, 220), b"window")
        .unwrap();

    desktop.move_window(id, -30, -40).unwrap();
    assert_eq!(desktop.window(id).unwrap().rect.x, 0);
    assert_eq!(desktop.window(id).unwrap().rect.y, 0);
    desktop.resize_window(id, 1, 1).unwrap();
    assert_eq!(
        desktop.window(id).unwrap().rect,
        rect(0, 0, MIN_WIDTH as u32, MIN_HEIGHT as u32)
    );

    desktop.action(id, gui::WindowAction::Maximize).unwrap();
    let maximized = desktop.window(id).unwrap();
    assert_eq!(maximized.state, WindowState::Maximized);
    assert_eq!(
        maximized.rect,
        rect(0, 0, gui::WIDTH, gui::HEIGHT - TASKBAR_HEIGHT as u32)
    );
    assert_eq!(
        desktop.move_window(id, 10, 10),
        Err(Status::Busy),
        "maximized windows cannot be moved"
    );
    assert_eq!(desktop.resize_window(id, 640, 480), Err(Status::Busy));

    desktop.action(id, gui::WindowAction::Restore).unwrap();
    assert_eq!(desktop.window(id).unwrap().state, WindowState::Normal);
    assert_eq!(
        desktop.window(id).unwrap().rect,
        rect(0, 0, MIN_WIDTH as u32, MIN_HEIGHT as u32)
    );
    desktop.action(id, gui::WindowAction::Minimize).unwrap();
    assert_eq!(desktop.window(id).unwrap().state, WindowState::Minimized);
    assert_eq!(desktop.focused(), None);
    assert_eq!(desktop.move_window(id, 10, 10), Err(Status::Busy));
    desktop.action(id, gui::WindowAction::Restore).unwrap();
    assert_eq!(desktop.window(id).unwrap().state, WindowState::Normal);
    desktop.action(id, gui::WindowAction::Close).unwrap();
    assert_eq!(desktop.window(id).unwrap().state, WindowState::Closed);
    assert_eq!(desktop.focus(id), Err(Status::NotFound));
}

#[test]
fn closed_window_slot_can_be_reused_for_reopen() {
    let mut desktop = Desktop::new();
    let first = desktop
        .create_window(1, rect(20, 20, 320, 220), b"first")
        .unwrap();
    desktop.action(first, gui::WindowAction::Close).unwrap();

    let reopened = desktop
        .create_window(1, rect(40, 40, 320, 220), b"reopened")
        .expect("closing a window must release its desktop slot");
    assert_ne!(reopened, first);
    assert_eq!(desktop.window(reopened).unwrap().state, WindowState::Normal);
    assert_eq!(desktop.focused(), Some(reopened));
}

#[test]
fn desktop_icons_hit_only_inside_their_fixed_bounds() {
    for icon in DESKTOP_ICONS {
        let right = icon.rect.x + DESKTOP_ICON_SIZE - 1;
        let bottom = icon.rect.y + DESKTOP_ICON_SIZE - 1;
        assert_eq!(
            desktop_icon_at(icon.rect.x, icon.rect.y),
            Some(icon.application)
        );
        assert_eq!(desktop_icon_at(right, bottom), Some(icon.application));
        assert_eq!(desktop_icon_at(icon.rect.x - 1, icon.rect.y), None);
        assert_eq!(desktop_icon_at(right + 1, icon.rect.y), None);
    }

    assert_eq!(desktop_icon_at(0, DESKTOP_ICON_TOP - 1), None);
    assert_eq!(
        desktop_icon_at(16 + DESKTOP_ICON_SIZE, DESKTOP_ICON_TOP),
        None,
        "the four-pixel gap between icons must not launch a neighboring app"
    );
    assert_eq!(
        desktop_icon_at(16, DESKTOP_ICON_TOP + DESKTOP_ICON_SIZE),
        None
    );
}
