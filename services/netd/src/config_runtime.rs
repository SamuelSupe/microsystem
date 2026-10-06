use microsystem_abi::{Message, Status, boot_cap, filesystem, protocol};
use microsystem_netd::config::Configuration;

const FILESYSTEM_VA: usize = 0x005e_0000;
const PATH: &str = "/.system/network.conf";

pub fn load() -> Result<Configuration, Status> {
    let reply = request(filesystem::Operation::ReadRange, PATH, &[], 0)?;
    let length = reply.words[0] as usize;
    if length > 4096 {
        return Err(Status::Corrupt);
    }
    let bytes = unsafe { core::slice::from_raw_parts(FILESYSTEM_VA as *const u8, length) };
    let text = core::str::from_utf8(bytes).map_err(|_| Status::Corrupt)?;
    Configuration::parse(text).map_err(|_| Status::Corrupt)
}

pub fn save(configuration: &Configuration) -> Result<(), Status> {
    let encoded = configuration.encode().map_err(|_| Status::NoSpace)?;
    request(filesystem::Operation::Write, PATH, encoded.as_bytes(), 0)?;
    request(filesystem::Operation::Fsync, PATH, &[], 0)?;
    Ok(())
}

fn request(
    operation: filesystem::Operation,
    path: &str,
    bytes: &[u8],
    word: u64,
) -> Result<Message, Status> {
    if path.len() + bytes.len() > 4096 {
        return Err(Status::Invalid);
    }
    unsafe {
        core::ptr::copy_nonoverlapping(path.as_ptr(), FILESYSTEM_VA as *mut u8, path.len());
        core::ptr::copy_nonoverlapping(
            bytes.as_ptr(),
            (FILESYSTEM_VA + path.len()) as *mut u8,
            bytes.len(),
        );
    }
    microsystem_user_rt::fence();
    let mut request = Message::new(protocol::FILESYSTEM, operation as u16);
    request.words[0] = path.len() as u64;
    request.words[1] = bytes.len() as u64;
    request.words[2] = word;
    request.caps[0] = boot_cap::NETWORK_FILESYSTEM_FRAME;
    let mut reply = Message::new(0, 0);
    let deadline = microsystem_user_rt::clock_now()?.saturating_add(5_000_000_000);
    microsystem_user_rt::ipc_call(
        boot_cap::FILESYSTEM_ENDPOINT,
        &request,
        &mut reply,
        deadline,
    )?;
    if reply.words[5] as i64 == 0 {
        return Ok(reply);
    }
    Err(match reply.words[5] as i64 {
        -4 => Status::NotFound,
        -11 => Status::NoSpace,
        -12 => Status::Corrupt,
        _ => Status::Io,
    })
}
