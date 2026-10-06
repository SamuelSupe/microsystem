use alloc::{
    format,
    string::{String, ToString},
    vec::Vec,
};
use core::fmt::Write;
use microsystem_abi::{
    CapHandle, Message, Rights, Status, application, boot_cap, filesystem::Operation as Fs,
    protocol, virtual_memory,
};
use microsystem_shell::{AppCommand, app_identifier, parse_app};
use sha2::{Digest, Sha256};

const COMMAND_VA: u64 = 0x0058_0000;
const FS_DATA: usize = 0x0063_0000;
const APPS: &str = "/.system/apps";

pub fn request(request: &Message, reply: &mut Message) -> Status {
    let cap = request.caps[0];
    let mut mapped = false;
    let result = (|| {
        if request.protocol != protocol::APPLICATION
            || request.opcode != application::Operation::Command as u16
            || request.words[0] > 512
            || cap == CapHandle::INVALID
            || request.caps[1..]
                .iter()
                .any(|cap| *cap != CapHandle::INVALID)
        {
            return Err(Status::Invalid);
        }
        microsystem_user_rt::frame_map(cap, COMMAND_VA, Rights(Rights::READ.0 | Rights::WRITE.0))?;
        mapped = true;
        let bytes = unsafe {
            core::slice::from_raw_parts(COMMAND_VA as *const u8, request.words[0] as usize)
        };
        let command = core::str::from_utf8(bytes)
            .map_err(|_| Status::Invalid)?
            .to_string();
        let actor = super::identity::actor(request.words[3])?;
        let parsed = parse_app(&command).map_err(|_| Status::Invalid)?;
        match parsed {
            AppCommand::List | AppCommand::Info(_) => {}
            AppCommand::Run(_) if actor.role != microsystem_identity::READER => {}
            _ if actor.administrator() => {}
            _ => return Err(Status::AccessDenied),
        }
        let output = execute(parsed, actor)?;
        Ok(output)
    })();
    let status = result.as_ref().err().copied().unwrap_or(Status::Ok);
    if mapped {
        let output = result.unwrap_or_else(|status| format!("app: failed status={status:?}\n"));
        let bytes = output.as_bytes();
        let length = bytes.len().min(4096);
        unsafe {
            core::ptr::copy_nonoverlapping(bytes.as_ptr(), COMMAND_VA as *mut u8, length);
        }
        reply.words[0] = length as u64;
        let _ = microsystem_user_rt::frame_unmap(cap, COMMAND_VA);
    }
    // Preserve init's own grants if an administrative request aliases one.
    for cap in request.caps.iter().copied() {
        if cap != CapHandle::INVALID
            && ![
                boot_cap::SYSTEM_INFO,
                boot_cap::RANDOM_SOURCE,
                boot_cap::ROOT_FILESYSTEM_FRAME,
            ]
            .contains(&cap)
        {
            let _ = microsystem_user_rt::cap_delete(cap);
        }
    }
    status
}

fn execute(command: AppCommand<'_>, actor: microsystem_identity::Actor) -> Result<String, Status> {
    match command {
        AppCommand::List => match fs(Fs::List, APPS, &[], 0) {
            Ok(reply) => shared_text(reply.words[0] as usize),
            Err(Status::NotFound) => Ok("no installed applications\n".to_string()),
            Err(status) => Err(status),
        },
        AppCommand::Info(name) => {
            let (current, previous) = active(name)?;
            let manifest = metadata(name, &current)?;
            Ok(format!(
                "{name} active={current} previous={previous}\n{manifest}"
            ))
        }
        AppCommand::Install {
            name,
            version,
            source,
            pages,
            permissions,
        } => {
            let stat = fs(Fs::Stat, source, &[], 0)?;
            if stat.words[0] != 1
                || stat.words[1] == 0
                || stat.words[1] > application::MAX_IMAGE_BYTES as u64
            {
                return Err(Status::Invalid);
            }
            let directory = format!("{APPS}/{name}");
            let metadata_path = format!("{directory}/{version}.meta");
            match fs(Fs::Stat, &metadata_path, &[], 0) {
                Ok(_) => return Err(Status::Busy),
                Err(Status::NotFound) => {}
                Err(status) => return Err(status),
            }
            let previous = match active(name) {
                Ok((current, _)) => current,
                Err(Status::NotFound) => "-".to_string(),
                Err(status) => return Err(status),
            };
            fs(Fs::Mkdir, APPS, &[], 0)?;
            fs(Fs::Mkdir, &directory, &[], 0)?;
            let temporary = format!("{directory}/install.pending");
            match fs(Fs::Unlink, &temporary, &[], 0) {
                Ok(_) | Err(Status::NotFound) => {}
                Err(status) => return Err(status),
            }
            fs(Fs::Copy, source, temporary.as_bytes(), 0)?;
            let image = image(&temporary)?;
            let hash = digest(&image);
            sync_file(&temporary)?;
            let executable = format!("{directory}/{version}.elf");
            fs(Fs::Replace, &temporary, executable.as_bytes(), 0)?;
            let manifest = format!("MICROAPP1\n{version}\n{hash}\n{pages}\n{permissions}\n");
            atomic_write(&metadata_path, manifest.as_bytes())?;
            publish(name, version, &previous)?;
            Ok(format!(
                "app: installed {name} version={version} pages={pages} permissions={permissions}\n"
            ))
        }
        AppCommand::Rollback(name) => {
            let (current, previous) = active(name)?;
            if previous == "-" {
                return Err(Status::NotFound);
            }
            let _ = verified_image(name, &previous)?;
            publish(name, &previous, &current)?;
            Ok(format!("app: rolled back {name} version={previous}\n"))
        }
        AppCommand::Run(name) => {
            let (version, _) = active(name)?;
            let (bytes, pages, permissions) = verified_image(name, &version)?;
            launch(name, &bytes, pages, permissions, actor)
        }
        AppCommand::Exec(path) => {
            launch(path, &image(path)?, virtual_memory::PAGE_BUDGET, 0, actor)
        }
    }
}

fn launch(
    name: &str,
    bytes: &[u8],
    pages: u32,
    permissions: u64,
    actor: microsystem_identity::Actor,
) -> Result<String, Status> {
    let program = super::program_id(name);
    let pid = microsystem_user_rt::thread_start_native(bytes, program, pages, permissions)?;
    super::remember_program(pid, program);
    super::identity::process_owner(pid, actor);
    let _ = microsystem_user_rt::debug_write_u64(
        b"[proc] filesystem ELF loaded pid=",
        pid,
        b" immutable-copy=true\n",
    );
    Ok(format!(
        "app: {name} pid={pid} pages={pages} permissions={permissions}\n"
    ))
}

fn image(path: &str) -> Result<Vec<u8>, Status> {
    let bytes = read(path, application::MAX_IMAGE_BYTES)?;
    let elf = microsystem_kernel::elf::ElfImage::parse(&bytes)?;
    #[cfg(target_arch = "x86_64")]
    let window = 0x100000;
    #[cfg(not(target_arch = "x86_64"))]
    let window = 0xe0000;
    for segment in elf.segments() {
        let segment = segment?;
        if segment
            .virtual_address
            .checked_add(segment.memory_size)
            .is_none_or(|end| end > microsystem_kernel::elf::USER_MIN + window)
        {
            return Err(Status::Invalid);
        }
    }
    Ok(bytes)
}

fn digest(bytes: &[u8]) -> String {
    let mut result = String::with_capacity(64);
    for byte in Sha256::digest(bytes) {
        let _ = write!(result, "{byte:02x}");
    }
    result
}

fn active(name: &str) -> Result<(String, String), Status> {
    let bytes = read(&format!("{APPS}/{name}/active"), 256)?;
    let text = core::str::from_utf8(&bytes).map_err(|_| Status::Corrupt)?;
    let mut lines = text.lines();
    if lines.next() != Some("MICROACTIVE1") {
        return Err(Status::Corrupt);
    }
    let current = lines
        .next()
        .filter(|version| app_identifier(version))
        .ok_or(Status::Corrupt)?;
    let previous = lines
        .next()
        .filter(|version| *version == "-" || app_identifier(version))
        .ok_or(Status::Corrupt)?;
    if lines.next().is_some() {
        return Err(Status::Corrupt);
    }
    Ok((current.to_string(), previous.to_string()))
}

fn publish(name: &str, current: &str, previous: &str) -> Result<(), Status> {
    atomic_write(
        &format!("{APPS}/{name}/active"),
        format!("MICROACTIVE1\n{current}\n{previous}\n").as_bytes(),
    )
}

fn metadata(name: &str, version: &str) -> Result<String, Status> {
    let bytes = read(&format!("{APPS}/{name}/{version}.meta"), 256)?;
    String::from_utf8(bytes).map_err(|_| Status::Corrupt)
}

fn verified_image(name: &str, version: &str) -> Result<(Vec<u8>, u32, u64), Status> {
    let manifest = metadata(name, version)?;
    let mut lines = manifest.lines();
    if lines.next() != Some("MICROAPP1") || lines.next() != Some(version) {
        return Err(Status::Corrupt);
    }
    let hash = lines
        .next()
        .filter(|hash| hash.len() == 64)
        .ok_or(Status::Corrupt)?;
    let pages = lines
        .next()
        .and_then(|pages| pages.parse::<u32>().ok())
        .filter(|pages| *pages != 0 && *pages <= virtual_memory::PAGE_BUDGET)
        .ok_or(Status::Corrupt)?;
    let permissions = lines
        .next()
        .and_then(|permissions| permissions.parse::<u64>().ok())
        .filter(|permissions| *permissions & !(application::RANDOM | application::SYSTEM_INFO) == 0)
        .ok_or(Status::Corrupt)?;
    if lines.next().is_some() {
        return Err(Status::Corrupt);
    }
    let bytes = image(&format!("{APPS}/{name}/{version}.elf"))?;
    if digest(&bytes) != hash {
        return Err(Status::Corrupt);
    }
    Ok((bytes, pages, permissions))
}

pub(super) fn read(path: &str, maximum: usize) -> Result<Vec<u8>, Status> {
    let stat = fs(Fs::Stat, path, &[], 0)?;
    let length = usize::try_from(stat.words[1]).map_err(|_| Status::Invalid)?;
    if stat.words[0] != 1 || length == 0 || length > maximum {
        return Err(Status::Invalid);
    }
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(length)
        .map_err(|_| Status::NoMemory)?;
    while bytes.len() < length {
        let reply = fs(Fs::ReadRange, path, &[], bytes.len() as u64)?;
        let count = usize::try_from(reply.words[0]).map_err(|_| Status::Corrupt)?;
        if count == 0 || count > 4096 || count > length - bytes.len() {
            return Err(Status::Corrupt);
        }
        bytes
            .extend_from_slice(unsafe { core::slice::from_raw_parts(FS_DATA as *const u8, count) });
    }
    Ok(bytes)
}

pub(super) fn atomic_write(path: &str, data: &[u8]) -> Result<(), Status> {
    atomic_write_mode(path, data, 0o644)
}

pub(super) fn atomic_write_mode(path: &str, data: &[u8], mode: u32) -> Result<(), Status> {
    let temporary = format!("{path}.pending");
    match fs(Fs::Stat, &temporary, &[], 0) {
        Err(Status::NotFound) => {
            fs(Fs::Write, &temporary, &[], 0)?;
        }
        Ok(_) => {}
        Err(status) => return Err(status),
    }
    fs(Fs::Chmod, &temporary, &[], mode as u64)?;
    fs(Fs::Write, &temporary, data, 0)?;
    sync_file(&temporary)?;
    fs(Fs::Replace, &temporary, path.as_bytes(), 0)?;
    Ok(())
}

fn sync_file(path: &str) -> Result<(), Status> {
    let descriptor = fs(Fs::Open, path, &[], 0)?.words[0];
    let result = fs(Fs::Fsync, "", &[], descriptor).map(|_| ());
    let close = fs(Fs::Close, "", &[], descriptor).map(|_| ());
    result.and(close)
}

fn shared_text(length: usize) -> Result<String, Status> {
    if length > 4096 {
        return Err(Status::Corrupt);
    }
    core::str::from_utf8(unsafe { core::slice::from_raw_parts(FS_DATA as *const u8, length) })
        .map(str::to_string)
        .map_err(|_| Status::Corrupt)
}

pub(super) fn fs(operation: Fs, path: &str, data: &[u8], offset: u64) -> Result<Message, Status> {
    if path.len() > 255 || path.len() + data.len() > 4096 {
        return Err(Status::Invalid);
    }
    unsafe {
        core::ptr::copy_nonoverlapping(path.as_ptr(), FS_DATA as *mut u8, path.len());
        core::ptr::copy_nonoverlapping(
            data.as_ptr(),
            (FS_DATA as *mut u8).add(path.len()),
            data.len(),
        );
    }
    let mut request = Message::new(protocol::FILESYSTEM, operation as u16);
    request.words[..3].copy_from_slice(&[path.len() as u64, data.len() as u64, offset]);
    request.caps[0] = boot_cap::ROOT_FILESYSTEM_FRAME;
    let mut reply = Message::new(0, 0);
    let deadline = microsystem_user_rt::clock_now()?.saturating_add(30_000_000_000);
    microsystem_user_rt::ipc_call(
        boot_cap::FILESYSTEM_ENDPOINT,
        &request,
        &mut reply,
        deadline,
    )?;
    if reply.protocol != protocol::FILESYSTEM {
        return Err(Status::Corrupt);
    }
    super::status_word(reply.words[5])?;
    Ok(reply)
}
