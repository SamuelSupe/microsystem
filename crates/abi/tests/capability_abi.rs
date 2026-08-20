use microsystem_abi::{
    ABI_VERSION, CapHandle, FilesystemStatsV1, MESSAGE_CAPS, MESSAGE_WORDS, Message, Rights,
    Status, Syscall, SystemControlOperation, ThreadLaunchV1, ThreadLaunchV2, boot_cap, filesystem,
    gui, network, process, script, time,
};

#[test]
fn cap_handle_round_trips_slot_and_generation() {
    let handle = CapHandle::from_parts(u16::MAX, u16::MAX);
    assert_eq!(handle.slot(), u16::MAX as usize);
    assert_eq!(handle.generation(), u16::MAX);
    assert_ne!(handle, CapHandle::INVALID);
}

#[test]
fn generation_is_part_of_identity() {
    let old = CapHandle::from_parts(7, 11);
    let recycled = CapHandle::from_parts(7, 12);
    assert_ne!(old, recycled);
    assert_eq!(old.slot(), recycled.slot());
    assert_ne!(old.generation(), recycled.generation());
}

#[test]
fn rights_intersection_cannot_grant_new_bits() {
    let source = Rights(Rights::READ.0 | Rights::WRITE.0 | Rights::GRANT.0);
    let requested = Rights(Rights::READ.0 | Rights::EXECUTE.0);
    let derived = source.intersect(requested);
    assert_eq!(derived, Rights::READ);
    assert!(derived.contains(Rights::READ));
    assert!(!derived.contains(Rights::WRITE));
    assert!(!derived.contains(Rights::EXECUTE));
    assert!(!derived.contains(Rights::GRANT));
}

#[test]
fn empty_rights_and_invalid_handle_are_unprivileged() {
    assert_eq!(Rights::NONE.intersect(Rights::ALL), Rights::NONE);
    assert!(!Rights::NONE.contains(Rights::READ));
    assert_eq!(CapHandle::INVALID.slot(), 0);
    assert_eq!(CapHandle::INVALID.generation(), 0);
}

#[test]
fn message_has_fixed_abi_defaults() {
    let message = Message::new(17, 3);
    assert_eq!(message.protocol, 17);
    assert_eq!(message.version, ABI_VERSION);
    assert_eq!(message.opcode, 3);
    assert_eq!(message.words, [0; MESSAGE_WORDS]);
    assert_eq!(message.caps, [CapHandle::INVALID; MESSAGE_CAPS]);
}

#[test]
fn syscall_and_status_numbers_are_stable() {
    assert_eq!(Syscall::Yield as u16, 0);
    assert_eq!(Syscall::DebugWrite as u16, 19);
    assert_eq!(Syscall::ThreadStatus as u16, 20);
    assert_eq!(Syscall::ThreadKill as u16, 21);
    assert_eq!(Syscall::SystemControl as u16, 22);
    assert_eq!(Syscall::NotificationSignal as u16, 23);
    assert_eq!(
        boot_cap::DEVICE_SYSTEM_CONTROL,
        CapHandle::from_parts(19, 1)
    );
    assert_eq!(
        boot_cap::SHARED_FILESYSTEM_FRAME,
        CapHandle::from_parts(25, 1)
    );
    assert_eq!(process::Operation::Wait as u16, 2);
    assert_eq!(process::Operation::List as u16, 3);
    assert_eq!(process::Operation::Kill as u16, 4);
    assert_eq!(process::Operation::MemoryPool as u16, 5);
    assert_eq!(filesystem::Operation::ReadRange as u16, 13);
    assert_eq!(filesystem::Operation::Stats as u16, 14);
    assert_eq!(filesystem::Operation::WriteRange as u16, 15);
    assert_eq!(core::mem::size_of::<FilesystemStatsV1>(), 80);
    assert_eq!(network::Operation::Stats as u16, 21);
    assert_eq!(SystemControlOperation::ActivatePci as u64, 1);
    assert_eq!(SystemControlOperation::Poweroff as u64, 2);
    assert_eq!(SystemControlOperation::Reboot as u64, 3);
    assert_eq!(boot_cap::SHELL_SYSTEM_CONTROL, CapHandle::from_parts(85, 1));
    assert_eq!(time::Operation::Sleep as u16, 1);
    assert_eq!(time::Operation::Uptime as u16, 2);
    assert_eq!(boot_cap::TIME_ENDPOINT, CapHandle::from_parts(39, 1));
    assert_eq!(Status::Ok as i32, 0);
    assert_eq!(Status::Corrupt as i32, -12);
}

#[test]
fn mica_gui_launch_and_ring_contract_is_stable() {
    assert_eq!(microsystem_abi::THREAD_LAUNCH_CAPS, 6);
    assert_eq!(microsystem_abi::THREAD_LAUNCH_V2_CAPS, 9);
    assert_eq!(microsystem_abi::THREAD_LAUNCH_FLAG_GUI, 1);
    assert_eq!(std::mem::size_of::<ThreadLaunchV1>(), 96);
    assert_eq!(std::mem::size_of::<ThreadLaunchV2>(), 120);
    assert_eq!(boot_cap::SCRIPT_GUI_COMMANDS.slot(), 72);
    assert_eq!(boot_cap::SCRIPT_GUI_EVENTS.slot(), 73);
    assert_eq!(boot_cap::SCRIPT_GUI_ENDPOINT.slot(), 74);
    assert_eq!(script::EVENT_GUI, 1 << 3);
    assert_eq!(script::FLAG_GUI_SESSION, 1 << 3);
    assert_eq!(gui::COMMAND_BYTES, 64 * 1024);
    assert_eq!(gui::EVENT_BYTES, 4 * 1024);
    assert_eq!(gui::COMMAND_HEADER_BYTES, 512);
    assert_eq!(gui::COMMAND_PAYLOAD_BYTES, gui::COMMAND_BYTES - 512);
    assert_eq!(gui::EVENT_RING_HEADER_BYTES, 64);
    assert_eq!(
        gui::EVENT_CAPACITY,
        (gui::EVENT_BYTES - gui::EVENT_RING_HEADER_BYTES) / std::mem::size_of::<gui::Event>()
    );
    assert_eq!(gui::MAX_DAMAGE_RECTS, 16);
    assert_eq!(gui::MAX_COMMANDS, 4096);
    assert_eq!(gui::MAX_TEXT_BYTES, 48 * 1024);
}

#[test]
fn gui_present_header_preserves_partial_damage_contract() {
    let rect = gui::Rect {
        x: 4,
        y: 8,
        width: 16,
        height: 12,
    };
    let mut header = gui::PresentHeaderV1::default();
    header.magic = gui::PRESENT_MAGIC;
    header.version = gui::VERSION;
    header.sequence = 1;
    header.damage_count = 1;
    header.damage[0] = rect;

    assert_eq!(core::mem::size_of::<gui::Rect>(), 16);
    assert_eq!(header.damage.len(), gui::MAX_DAMAGE_RECTS);
    assert_eq!(header.damage_count as usize, 1);
    assert_eq!(header.damage[0], rect);
    assert!(rect.x >= 0 && rect.y >= 0 && rect.width > 0 && rect.height > 0);
    assert!((header.damage_count as usize) <= gui::MAX_DAMAGE_RECTS);
}

#[test]
fn desktop_application_launch_contract_is_stable() {
    assert_eq!(gui::Operation::LaunchApplication as u16, 10);
    assert_eq!(gui::Application::Terminal as u16, 1);
    assert_eq!(gui::Application::Files as u16, 2);
    assert_eq!(gui::Application::Monitor as u16, 3);
    assert_eq!(gui::Application::Reader as u16, 4);
    assert_eq!(gui::Application::Editor as u16, 5);
    assert_eq!(
        gui::Application::from_u64(5),
        Some(gui::Application::Editor)
    );
    assert_eq!(gui::Application::from_u64(6), None);
    assert_eq!(boot_cap::GUI_LAUNCH_ENDPOINT.slot(), 84);
}
