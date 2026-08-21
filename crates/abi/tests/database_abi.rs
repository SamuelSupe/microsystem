use microsystem_abi::{DbResponseHeaderV1, database, protocol};

#[test]
fn database_protocol_numbers_and_constants_are_stable() {
    assert_eq!(protocol::DATABASE, 10);
    assert_eq!(database::RESPONSE_MAGIC.to_le_bytes(), *b"SQL1");
    assert_eq!(database::RESPONSE_VERSION, 1);
    assert_eq!(database::SHARED_FRAME_BYTES, 4096);

    assert_eq!(database::Operation::Ping as u16, 1);
    assert_eq!(database::Operation::Execute as u16, 2);
    assert_eq!(database::ResponseKind::Command as u16, 1);
    assert_eq!(database::ResponseKind::Rows as u16, 2);
    assert_eq!(database::ResponseKind::Error as u16, 3);
    assert_eq!(database::ValueTag::Null as u8, 0);
    assert_eq!(database::ValueTag::Integer as u8, 1);
    assert_eq!(database::ValueTag::Text as u8, 2);
    assert_eq!(database::ValueTag::Bool as u8, 3);
}

#[test]
fn database_response_header_v1_has_stable_c_layout() {
    assert_eq!(core::mem::size_of::<DbResponseHeaderV1>(), 24);
    assert_eq!(core::mem::align_of::<DbResponseHeaderV1>(), 4);
    assert_eq!(core::mem::offset_of!(DbResponseHeaderV1, magic), 0);
    assert_eq!(core::mem::offset_of!(DbResponseHeaderV1, version), 4);
    assert_eq!(core::mem::offset_of!(DbResponseHeaderV1, kind), 6);
    assert_eq!(core::mem::offset_of!(DbResponseHeaderV1, columns), 8);
    assert_eq!(core::mem::offset_of!(DbResponseHeaderV1, rows), 10);
    assert_eq!(core::mem::offset_of!(DbResponseHeaderV1, reserved), 12);
    assert_eq!(core::mem::offset_of!(DbResponseHeaderV1, affected_rows), 16);
    assert_eq!(core::mem::offset_of!(DbResponseHeaderV1, payload_bytes), 20);

    let header = DbResponseHeaderV1 {
        magic: database::RESPONSE_MAGIC,
        version: database::RESPONSE_VERSION,
        kind: database::ResponseKind::Rows as u16,
        columns: 2,
        rows: 3,
        reserved: 0,
        affected_rows: 0,
        payload_bytes: 128,
    };
    assert_eq!(header.magic.to_le_bytes(), *b"SQL1");
    assert_eq!(header.kind, database::ResponseKind::Rows as u16);
    assert_eq!(header.columns, 2);
    assert_eq!(header.rows, 3);
    assert_eq!(header.payload_bytes, 128);
}
