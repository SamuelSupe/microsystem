use microsystem_abi::{Status, gui};
use microsystem_gui::{replace_display_list, validate_command_stream};

fn command(kind: u16, rect: gui::Rect, payload: &[u8]) -> Vec<u8> {
    let draw = gui::DrawCommand {
        header: gui::CommandHeader {
            kind,
            flags: 0,
            bytes: (core::mem::size_of::<gui::DrawCommand>() + payload.len()) as u32,
        },
        rect,
        color: 0x0012_3456,
        argument: 0,
    };
    let raw = unsafe {
        core::slice::from_raw_parts(
            (&draw as *const gui::DrawCommand).cast::<u8>(),
            core::mem::size_of::<gui::DrawCommand>(),
        )
    };
    let mut bytes = raw.to_vec();
    bytes.extend_from_slice(payload);
    bytes
}

fn valid_rect() -> gui::Rect {
    gui::Rect {
        x: 8,
        y: 12,
        width: 320,
        height: 200,
    }
}

#[test]
fn accepts_utf8_text_and_multiple_commands() {
    let mut stream = command(
        gui::CommandKind::Text as u16,
        valid_rect(),
        "héllo 世界".as_bytes(),
    );
    stream.extend_from_slice(&command(
        gui::CommandKind::FillRect as u16,
        valid_rect(),
        &[],
    ));
    assert_eq!(validate_command_stream(&stream), Ok(2));
}

#[test]
fn accepts_every_defined_draw_command_kind() {
    let kinds = [
        gui::CommandKind::Clear,
        gui::CommandKind::FillRect,
        gui::CommandKind::StrokeRect,
        gui::CommandKind::Text,
        gui::CommandKind::Icon,
        gui::CommandKind::SetClip,
    ];
    for kind in kinds {
        let payload: &[u8] = if kind == gui::CommandKind::Text {
            b"widget"
        } else {
            &[]
        };
        assert_eq!(
            validate_command_stream(&command(kind as u16, valid_rect(), payload)),
            Ok(1),
            "defined command kind must remain accepted: {kind:?}"
        );
    }
}

#[test]
fn rejects_invalid_utf8_unknown_kinds_and_bad_rectangles() {
    assert_eq!(
        validate_command_stream(&command(
            gui::CommandKind::Text as u16,
            valid_rect(),
            &[0xff, 0xfe],
        )),
        Err(Status::Invalid)
    );
    assert_eq!(
        validate_command_stream(&command(99, valid_rect(), &[])),
        Err(Status::Invalid)
    );
    assert_eq!(
        validate_command_stream(&command(
            gui::CommandKind::FillRect as u16,
            gui::Rect {
                width: 0,
                ..valid_rect()
            },
            &[],
        )),
        Err(Status::Invalid)
    );
}

#[test]
fn rejects_truncated_or_oversized_streams_as_a_single_transaction() {
    let valid = command(gui::CommandKind::FillRect as u16, valid_rect(), &[]);
    let mut truncated = valid.clone();
    truncated.push(0);
    assert_eq!(validate_command_stream(&truncated), Err(Status::Invalid));

    let mut oversized_text = command(
        gui::CommandKind::Text as u16,
        valid_rect(),
        &vec![b'a'; gui::MAX_TEXT_BYTES + 1],
    );
    assert!(oversized_text.len() > gui::MAX_TEXT_BYTES);
    assert_eq!(
        validate_command_stream(&oversized_text),
        Err(Status::Invalid)
    );
    oversized_text.resize(gui::COMMAND_BYTES + 1, 0);
    assert_eq!(
        validate_command_stream(&oversized_text),
        Err(Status::Invalid)
    );
}

#[test]
fn rejects_short_header_and_out_of_bounds_command_length() {
    assert_eq!(
        validate_command_stream(&[0; core::mem::size_of::<gui::DrawCommand>() - 1]),
        Err(Status::Invalid)
    );
    let mut bytes = command(gui::CommandKind::Clear as u16, valid_rect(), &[]);
    let bytes_field = core::mem::size_of::<gui::CommandHeader>() - core::mem::size_of::<u32>();
    bytes[bytes_field..bytes_field + 4].copy_from_slice(&u32::MAX.to_le_bytes());
    assert_eq!(validate_command_stream(&bytes), Err(Status::Invalid));
}

#[test]
fn display_list_replacement_is_atomic_and_clears_stale_tail() {
    let valid = command(gui::CommandKind::FillRect as u16, valid_rect(), &[]);
    let invalid = command(99, valid_rect(), &[]);
    let mut current = vec![0xa5; valid.len() + 7];
    let before = current.clone();

    assert_eq!(
        replace_display_list(&mut current, &invalid),
        Err(Status::Invalid)
    );
    assert_eq!(
        current, before,
        "invalid Present must leave the old list intact"
    );

    assert_eq!(replace_display_list(&mut current, &valid), Ok(1));
    assert_eq!(&current[..valid.len()], valid.as_slice());
    assert!(current[valid.len()..].iter().all(|byte| *byte == 0));
}
