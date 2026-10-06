#![no_std]
#![no_main]

extern crate alloc;

use alloc::string::ToString;
use alloc::vec::Vec;
use core::panic::PanicInfo;
use microsystem_abi::{
    DbResponseHeaderV1, Message, Status, boot_cap, database, filesystem, protocol,
};
use microsystem_sql::{ColumnInfo, Database, Error as SqlError, Execution, Value};

const SHARED_DATA: usize = 0x005e_0000;
const FILESYSTEM_DATA: usize = 0x0062_0000;
const SHARED_BYTES: usize = database::SHARED_FRAME_BYTES;
const RESPONSE_HEADER_BYTES: usize = core::mem::size_of::<DbResponseHeaderV1>();
const DATABASE_PATH: &str = "/.system/db/main.db";
const DATABASE_TEMP_PATH: &str = "/.system/db/main.db.tmp";

#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    let _ = microsystem_user_rt::debug_write(b"[user] db service ELF entered EL0\n");
    let (mut database_state, startup_error) = match ensure_storage().and_then(|_| load_database()) {
        Ok(database) => {
            let _ = microsystem_user_rt::debug_write(
                b"[user] msql snapshot loaded path=/.system/db/main.db\n",
            );
            (database, None)
        }
        Err(status) => {
            if status == Status::Corrupt {
                let _ = microsystem_user_rt::debug_write(
                    b"[db] snapshot load failed status=Corrupt; empty database not installed\n",
                );
            } else {
                let _ = microsystem_user_rt::debug_write(b"[db] startup failed status=-");
                let _ = microsystem_user_rt::debug_write_u64(
                    b"",
                    (status as i64).unsigned_abs(),
                    b"\n",
                );
            }
            let _ = microsystem_user_rt::debug_write(
                b"[db] service remains online; database execution disabled\n",
            );
            (Database::new(), Some(status))
        }
    };
    let _ = microsystem_user_rt::service_online();
    let mut request = Message::new(protocol::DATABASE, 0);
    let _ = microsystem_user_rt::debug_write(b"[ipc] resident db endpoint=26 ready\n");
    if microsystem_user_rt::ipc_recv(boot_cap::DATABASE_ENDPOINT, &mut request, 0).is_err() {
        microsystem_user_rt::exit(4);
    }
    loop {
        let mut reply = Message::new(protocol::DATABASE, request.opcode);
        let status = handle_request(&mut database_state, startup_error, &request, &mut reply);
        reply.words[5] = status as i64 as u64;
        if microsystem_user_rt::ipc_reply_recv(boot_cap::DATABASE_ENDPOINT, &reply, &mut request, 0)
            .is_err()
        {
            microsystem_user_rt::exit(5);
        }
    }
}

fn handle_request(
    database_state: &mut Database,
    startup_error: Option<Status>,
    request: &Message,
    reply: &mut Message,
) -> Status {
    if request.protocol != protocol::DATABASE
        || request.caps[0] != boot_cap::SHARED_FILESYSTEM_FRAME
    {
        return Status::AccessDenied;
    }
    match request.opcode {
        value if value == database::Operation::Ping as u16 => write_command_response(reply, 0),
        value if value == database::Operation::Execute as u16 => {
            let actor = unsafe {
                microsystem_identity::read_shared(microsystem_abi::identity::SNAPSHOT_VA)
            }
            .and_then(|state| {
                state.actor(
                    microsystem_user_rt::ipc_peer()? as u32,
                    request.words[3],
                    microsystem_user_rt::clock_now()?,
                )
            });
            if !actor.is_ok_and(|actor| actor.administrator()) {
                return write_status_error(reply, Status::AccessDenied);
            }
            if let Some(status) = startup_error {
                return write_status_error(reply, status);
            }
            let length = match usize::try_from(request.words[0]) {
                Ok(length) if length <= SHARED_BYTES => length,
                _ => return Status::Invalid,
            };
            let sql = unsafe { core::slice::from_raw_parts(SHARED_DATA as *const u8, length) };
            let sql = match core::str::from_utf8(sql) {
                Ok(sql) => sql,
                Err(_) => return write_error_response(reply, SqlError::InvalidLiteral),
            };
            match database_state.execute_query(sql) {
                Ok(execution) => return write_execution_response(reply, &execution),
                Err(SqlError::Unsupported) => {}
                Err(error) => return write_error_response(reply, error),
            }
            let mut candidate = database_state.clone();
            let execution = match candidate.execute(sql) {
                Ok(execution) => execution,
                Err(error) => return write_error_response(reply, error),
            };
            match &execution {
                Execution::Rows { .. } => write_execution_response(reply, &execution),
                Execution::Command { .. } => {
                    let mut snapshot = Vec::new();
                    if let Err(error) = candidate.encode_snapshot(&mut snapshot) {
                        return write_error_response(reply, error);
                    }
                    if let Err(status) = persist_snapshot(&snapshot) {
                        return write_status_error(reply, status);
                    }
                    *database_state = candidate;
                    write_execution_response(reply, &execution)
                }
            }
        }
        _ => Status::Invalid,
    }
}

fn ensure_storage() -> Result<(), Status> {
    let reply = fs_request(filesystem::Operation::Mkdir, "/.system", &[], 0)?;
    status_from_reply(&reply)?;
    let reply = fs_request(filesystem::Operation::Mkdir, "/.system/db", &[], 0)?;
    status_from_reply(&reply)
}

fn load_database() -> Result<Database, Status> {
    let stat = match fs_request(filesystem::Operation::Stat, DATABASE_PATH, &[], 0) {
        Ok(reply) => {
            status_from_reply(&reply)?;
            reply
        }
        Err(Status::NotFound) => return Ok(Database::new()),
        Err(status) => return Err(status),
    };
    if stat.words[0] != 1 {
        return Err(Status::Corrupt);
    }
    let length = usize::try_from(stat.words[1]).map_err(|_| Status::Corrupt)?;
    if length > microsystem_sql::MAX_SNAPSHOT_BYTES {
        return Err(Status::Corrupt);
    }
    let mut snapshot = Vec::with_capacity(length);
    let mut offset = 0usize;
    while offset < length {
        let reply = fs_request(
            filesystem::Operation::ReadRange,
            DATABASE_PATH,
            &[],
            offset as u64,
        )?;
        status_from_reply(&reply)?;
        let chunk_length = usize::try_from(reply.words[0]).map_err(|_| Status::Corrupt)?;
        if chunk_length == 0 || chunk_length > SHARED_BYTES || chunk_length > length - offset {
            return Err(Status::Corrupt);
        }
        let chunk =
            unsafe { core::slice::from_raw_parts(FILESYSTEM_DATA as *const u8, chunk_length) };
        snapshot.extend_from_slice(chunk);
        offset += chunk_length;
    }
    Database::decode_snapshot(&snapshot).map_err(|_| Status::Corrupt)
}

fn persist_snapshot(snapshot: &[u8]) -> Result<(), Status> {
    fs_request(filesystem::Operation::Write, DATABASE_TEMP_PATH, &[], 0)?;
    let chunk_bytes = SHARED_BYTES.saturating_sub(DATABASE_TEMP_PATH.len());
    if chunk_bytes == 0 {
        return Err(Status::Invalid);
    }
    let mut offset = 0usize;
    while offset < snapshot.len() {
        let end = (offset + chunk_bytes).min(snapshot.len());
        fs_request(
            filesystem::Operation::WriteRange,
            DATABASE_TEMP_PATH,
            &snapshot[offset..end],
            offset as u64,
        )?;
        offset = end;
    }
    fsync_path(DATABASE_TEMP_PATH)?;
    let reply = fs_request(
        filesystem::Operation::Replace,
        DATABASE_TEMP_PATH,
        DATABASE_PATH.as_bytes(),
        0,
    )?;
    status_from_reply(&reply)?;
    // MFS1's Replace operation atomically installs the destination and fsyncs
    // it before returning success, so this response is the final commit point.
    Ok(())
}

fn fsync_path(path: &str) -> Result<(), Status> {
    let descriptor = fs_request(filesystem::Operation::Open, path, &[], 0)?.words[0];
    if descriptor == 0 {
        return Err(Status::Io);
    }
    let synced = fs_request(filesystem::Operation::Fsync, "", &[], descriptor).map(|_| ());
    let closed = fs_request(filesystem::Operation::Close, "", &[], descriptor).map(|_| ());
    match synced {
        Ok(()) => closed,
        Err(status) => {
            let _ = closed;
            Err(status)
        }
    }
}

fn write_execution_response(reply: &mut Message, execution: &Execution) -> Status {
    match execution {
        Execution::Command { affected_rows } => write_command_response(reply, *affected_rows),
        Execution::Rows { columns, rows } => write_rows_response(reply, columns, rows),
    }
}

fn write_command_response(reply: &mut Message, affected_rows: u32) -> Status {
    if !write_header(database::ResponseKind::Command, 0, 0, affected_rows, 0) {
        return Status::NoSpace;
    }
    reply.words[0] = RESPONSE_HEADER_BYTES as u64;
    reply.words[1] = affected_rows as u64;
    Status::Ok
}

fn write_rows_response(reply: &mut Message, columns: &[ColumnInfo], rows: &[Vec<Value>]) -> Status {
    let mut payload = Vec::new();
    for column in columns {
        if column.name.len() > u8::MAX as usize {
            return Status::Invalid;
        }
        payload.push(column.name.len() as u8);
        payload.extend_from_slice(column.name.as_bytes());
        payload.push(column.ty as u8);
    }
    for row in rows {
        if row.len() != columns.len() {
            return Status::Corrupt;
        }
        for value in row {
            if encode_response_value(&mut payload, value).is_err() {
                return Status::NoSpace;
            }
        }
    }
    if columns.len() > u16::MAX as usize || rows.len() > u16::MAX as usize {
        return Status::NoSpace;
    }
    if RESPONSE_HEADER_BYTES.saturating_add(payload.len()) > SHARED_BYTES {
        return Status::NoSpace;
    }
    if !write_header(
        database::ResponseKind::Rows,
        columns.len() as u16,
        rows.len() as u16,
        0,
        payload.len() as u32,
    ) {
        return Status::NoSpace;
    }
    unsafe {
        core::ptr::copy_nonoverlapping(
            payload.as_ptr(),
            (SHARED_DATA + RESPONSE_HEADER_BYTES) as *mut u8,
            payload.len(),
        );
        microsystem_user_rt::fence();
    }
    reply.words[0] = (RESPONSE_HEADER_BYTES + payload.len()) as u64;
    reply.words[1] = rows.len() as u64;
    Status::Ok
}

fn encode_response_value(output: &mut Vec<u8>, value: &Value) -> Result<(), ()> {
    match value {
        Value::Null => output.push(database::ValueTag::Null as u8),
        Value::Integer(value) => {
            output.push(database::ValueTag::Integer as u8);
            output.extend_from_slice(&value.to_le_bytes());
        }
        Value::Text(value) => {
            if value.len() > u16::MAX as usize {
                return Err(());
            }
            output.push(database::ValueTag::Text as u8);
            output.extend_from_slice(&(value.len() as u16).to_le_bytes());
            output.extend_from_slice(value.as_bytes());
        }
        Value::Bool(value) => {
            output.push(database::ValueTag::Bool as u8);
            output.push(u8::from(*value));
        }
    }
    if output.len() > SHARED_BYTES - RESPONSE_HEADER_BYTES {
        Err(())
    } else {
        Ok(())
    }
}

fn write_error_response(reply: &mut Message, error: SqlError) -> Status {
    let status = map_sql_error(error);
    let message = error.to_string();
    let bytes = message.as_bytes();
    if RESPONSE_HEADER_BYTES + bytes.len() > SHARED_BYTES {
        return Status::Invalid;
    }
    if !write_header(database::ResponseKind::Error, 0, 0, 0, bytes.len() as u32) {
        return Status::NoSpace;
    }
    unsafe {
        core::ptr::copy_nonoverlapping(
            bytes.as_ptr(),
            (SHARED_DATA + RESPONSE_HEADER_BYTES) as *mut u8,
            bytes.len(),
        );
        microsystem_user_rt::fence();
    }
    reply.words[0] = (RESPONSE_HEADER_BYTES + bytes.len()) as u64;
    status
}

fn write_status_error(reply: &mut Message, status: Status) -> Status {
    let message = match status {
        Status::NoSpace => "database storage is full",
        Status::Corrupt => "database snapshot is corrupt",
        Status::NotFound => "database storage path is missing",
        _ => "database storage error",
    };
    let bytes = message.as_bytes();
    if !write_header(database::ResponseKind::Error, 0, 0, 0, bytes.len() as u32) {
        return Status::NoSpace;
    }
    unsafe {
        core::ptr::copy_nonoverlapping(
            bytes.as_ptr(),
            (SHARED_DATA + RESPONSE_HEADER_BYTES) as *mut u8,
            bytes.len(),
        );
        microsystem_user_rt::fence();
    }
    reply.words[0] = (RESPONSE_HEADER_BYTES + bytes.len()) as u64;
    status
}

fn write_header(
    kind: database::ResponseKind,
    columns: u16,
    rows: u16,
    affected_rows: u32,
    payload_bytes: u32,
) -> bool {
    let header = DbResponseHeaderV1 {
        magic: database::RESPONSE_MAGIC,
        version: database::RESPONSE_VERSION,
        kind: kind as u16,
        columns,
        rows,
        reserved: 0,
        affected_rows,
        payload_bytes,
    };
    unsafe {
        core::ptr::copy_nonoverlapping(
            (&header as *const DbResponseHeaderV1).cast::<u8>(),
            SHARED_DATA as *mut u8,
            RESPONSE_HEADER_BYTES,
        );
        microsystem_user_rt::fence();
    }
    true
}

fn fs_request(
    operation: filesystem::Operation,
    path: &str,
    data: &[u8],
    offset: u64,
) -> Result<Message, Status> {
    if path.len() > 255 || path.len().saturating_add(data.len()) > SHARED_BYTES {
        return Err(Status::Invalid);
    }
    unsafe {
        core::ptr::copy_nonoverlapping(path.as_ptr(), FILESYSTEM_DATA as *mut u8, path.len());
        core::ptr::copy_nonoverlapping(
            data.as_ptr(),
            (FILESYSTEM_DATA + path.len()) as *mut u8,
            data.len(),
        );
        microsystem_user_rt::fence();
    }
    let mut request = Message::new(protocol::FILESYSTEM, operation as u16);
    request.words[0] = path.len() as u64;
    request.words[1] = data.len() as u64;
    request.words[2] = offset;
    request.caps[0] = boot_cap::DATABASE_FILESYSTEM_FRAME;
    let mut reply = Message::new(protocol::FILESYSTEM, 0);
    microsystem_user_rt::ipc_call(boot_cap::FILESYSTEM_ENDPOINT, &request, &mut reply, 0)?;
    if reply.protocol != protocol::FILESYSTEM {
        return Err(Status::Io);
    }
    let status = status_from_raw(reply.words[5] as i64);
    if status != Status::Ok {
        return Err(status);
    }
    microsystem_user_rt::fence();
    Ok(reply)
}

fn status_from_reply(reply: &Message) -> Result<(), Status> {
    let status = status_from_raw(reply.words[5] as i64);
    if status == Status::Ok {
        Ok(())
    } else {
        Err(status)
    }
}

fn status_from_raw(status: i64) -> Status {
    match status {
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
        -11 => Status::NoSpace,
        -12 => Status::Corrupt,
        _ => Status::Io,
    }
}

fn map_sql_error(error: SqlError) -> Status {
    match error {
        SqlError::Corrupt | SqlError::Version => Status::Corrupt,
        SqlError::NoSpace | SqlError::TooLarge => Status::NoSpace,
        SqlError::TableNotFound | SqlError::ColumnNotFound => Status::NotFound,
        _ => Status::Invalid,
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo<'_>) -> ! {
    microsystem_user_rt::exit(1)
}
