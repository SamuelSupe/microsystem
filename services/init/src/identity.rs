use alloc::{
    format,
    string::{String, ToString},
    vec::Vec,
};
use core::fmt::Write;
use microsystem_abi::{
    CapHandle, Message, Rights, Status, boot_cap, filesystem::Operation as Fs, identity::Operation,
    protocol, service,
};
use microsystem_identity::{ADMIN, Actor, GUEST_UID, OPERATOR, READER, Snapshot};
use sha2::{Digest, Sha256};

const ACCOUNTS: &str = "/.system/accounts";
const AUDIT: &str = "/.system/audit.log";
const COMMAND_VA: u64 = 0x0058_0000;
const ACCOUNT_BYTES: usize = 16 + core::mem::size_of::<[microsystem_identity::User; 8]>();
const DEFAULT_KEY: [u8; 32] = *include_bytes!("../../../build/ssh-authorized-key.bin");

pub struct Registry {
    ready: bool,
    next_poll: u64,
    ssh_start: u64,
}
impl Registry {
    pub fn new() -> Self {
        Self {
            ready: false,
            next_poll: 0,
            ssh_start: 0,
        }
    }
    pub fn poll(&mut self) {
        let now = microsystem_user_rt::clock_now().unwrap_or(0);
        if now < self.next_poll {
            return;
        }
        self.next_poll = now.saturating_add(1_000_000_000);
        if !self.ready {
            let mut info = service::InfoV1::EMPTY;
            if microsystem_user_rt::service_status(4, &mut info).is_err()
                || info.state != service::ONLINE
            {
                return;
            }
            match load() {
                Ok(snapshot) => {
                    publish(&snapshot);
                    self.ready = true;
                    let _ = microsystem_user_rt::debug_write(
                        b"[identity] accounts loaded roles=enforced local-console=root\n",
                    );
                }
                Err(status) => {
                    let _ = microsystem_user_rt::debug_write_u64(
                        b"[identity] account store unavailable status=",
                        (status as i64).unsigned_abs(),
                        b" fail-closed=true\n",
                    );
                    self.next_poll = now.saturating_add(10_000_000_000);
                }
            }
        }
        let mut info = service::InfoV1::EMPTY;
        if self.ready
            && microsystem_user_rt::service_status(11, &mut info).is_ok()
            && self.ssh_start != info.starts
        {
            if let Ok(mut snapshot) = snapshot() {
                snapshot.revoke_peer(11);
                publish(&snapshot);
            }
            self.ssh_start = info.starts;
        }
    }
    pub fn request(&mut self, request: &Message, reply: &mut Message) -> Status {
        let peer = match microsystem_user_rt::ipc_peer() {
            Ok(peer) => peer as u32,
            Err(status) => return status,
        };
        if request.protocol != protocol::IDENTITY || !self.ready {
            return Status::Busy;
        }
        match request.opcode {
            value if value == Operation::AuthorizeSsh as u16 => self
                .authorize(peer, request, reply)
                .map(|()| Status::Ok)
                .unwrap_or_else(|s| s),
            value if value == Operation::DropCredential as u16 => {
                if peer != 11 {
                    return Status::AccessDenied;
                }
                let Ok(mut state) = snapshot() else {
                    return Status::Busy;
                };
                for c in &mut state.credentials {
                    if c.cookie == request.words[0] && c.peer_pid == peer {
                        *c = microsystem_identity::Credential::EMPTY;
                    }
                }
                publish(&state);
                Status::Ok
            }
            value if value == Operation::SessionAccepted as u16 => {
                if peer != 11 || request.words[0] == 0 {
                    return Status::AccessDenied;
                }
                let result = snapshot().and_then(|state| {
                    let actor =
                        state.actor(peer, request.words[0], microsystem_user_rt::clock_now()?)?;
                    audit(
                        actor.uid,
                        "ssh-login",
                        state.uid(actor.uid).ok_or(Status::AccessDenied)?.name(),
                    )
                });
                result.map(|()| Status::Ok).unwrap_or_else(|s| s)
            }
            value if value == Operation::Command as u16 => self.command(peer, request, reply),
            _ => Status::Invalid,
        }
    }
    fn authorize(&self, peer: u32, request: &Message, reply: &mut Message) -> Result<(), Status> {
        if peer != 11 {
            return Err(Status::AccessDenied);
        }
        let mut state = snapshot()?;
        let mut name = [0; 32];
        let mut key = [0; 32];
        for i in 0..4 {
            name[i * 8..i * 8 + 8].copy_from_slice(&request.words[i].to_le_bytes());
        }
        let name = core::str::from_utf8(&name[..name.iter().position(|b| *b == 0).unwrap_or(32)])
            .map_err(|_| Status::Invalid)?;
        if !microsystem_identity::valid_name(name)
            || request.caps[0] == CapHandle::INVALID
            || request.caps[1..].iter().any(|c| *c != CapHandle::INVALID)
        {
            return Err(Status::Invalid);
        }
        let frame = request.caps[0];
        let mapped = microsystem_user_rt::frame_map(frame, COMMAND_VA, Rights::READ);
        if mapped.is_ok() {
            unsafe {
                core::ptr::copy_nonoverlapping(COMMAND_VA as *const u8, key.as_mut_ptr(), 32);
            }
            let _ = microsystem_user_rt::frame_unmap(frame, COMMAND_VA);
        }
        let _ = microsystem_user_rt::cap_delete(frame);
        mapped?;
        let user = state
            .find(name)
            .filter(|u| u.enabled() && u.keys[..u.key_count as usize].contains(&key))
            .copied();
        let Some(user) = user else {
            audit(0, "ssh-rejected", name)?;
            return Err(Status::AccessDenied);
        };
        let now = microsystem_user_rt::clock_now()?;
        // SSH public-key probing and signature authentication may ask twice.
        // The service handles one transport at a time, so only its newest cookie
        // should survive; otherwise probe credentials exhaust the bounded table.
        state.revoke_peer(peer);
        let mut bytes = [0; 8];
        microsystem_user_rt::random_fill(boot_cap::RANDOM_SOURCE, &mut bytes)?;
        let cookie = u64::from_le_bytes(bytes).max(1);
        state.issue(
            user.uid,
            peer,
            cookie,
            now.saturating_add(3_600_000_000_000),
            now,
        )?;
        publish(&state);
        reply.words[0] = cookie;
        reply.words[1] = user.uid as u64;
        Ok(())
    }
    fn command(&mut self, peer: u32, request: &Message, reply: &mut Message) -> Status {
        let cap = request.caps[0];
        let mut mapped = false;
        let result = (|| {
            if !matches!(peer, 5 | 8 | 11) || request.words[0] > 512 || cap == CapHandle::INVALID {
                return Err(Status::Invalid);
            }
            microsystem_user_rt::frame_map(
                cap,
                COMMAND_VA,
                Rights(Rights::READ.0 | Rights::WRITE.0),
            )?;
            mapped = true;
            let bytes = unsafe {
                core::slice::from_raw_parts(COMMAND_VA as *const u8, request.words[0] as usize)
            };
            let command = core::str::from_utf8(bytes)
                .map_err(|_| Status::Invalid)?
                .to_string();
            let mut state = snapshot()?;
            if peer == 11 && request.words[3] == 0 {
                return Err(Status::AccessDenied);
            }
            let actor = state.actor(peer, request.words[3], microsystem_user_rt::clock_now()?)?;
            execute(&mut state, actor, peer, &command)
        })();
        let status = result.as_ref().err().copied().unwrap_or(Status::Ok);
        if mapped {
            let output =
                result.unwrap_or_else(|status| format!("user: failed status={status:?}\n"));
            let length = output.len().min(4096);
            unsafe {
                core::ptr::copy_nonoverlapping(output.as_ptr(), COMMAND_VA as *mut u8, length);
            }
            reply.words[0] = length as u64;
            let _ = microsystem_user_rt::frame_unmap(cap, COMMAND_VA);
        }
        if cap != CapHandle::INVALID {
            let _ = microsystem_user_rt::cap_delete(cap);
        }
        status
    }
}

pub fn snapshot() -> Result<Snapshot, Status> {
    unsafe { microsystem_identity::read_shared(microsystem_abi::identity::SNAPSHOT_VA) }
}
fn publish(state: &Snapshot) {
    unsafe {
        microsystem_identity::publish(microsystem_abi::identity::SNAPSHOT_VA, state);
    }
}
pub fn actor(cookie: u64) -> Result<Actor, Status> {
    let peer = microsystem_user_rt::ipc_peer()? as u32;
    if peer == 5 && cookie == 0 {
        return Ok(Actor::SYSTEM);
    }
    if peer == 11 && cookie == 0 {
        return Err(Status::AccessDenied);
    }
    snapshot()?.actor(peer, cookie, microsystem_user_rt::clock_now()?)
}
pub fn process_owner(pid: u64, actor: Actor) {
    if let Ok(mut state) = snapshot() {
        if let Some(uid) = pid
            .checked_sub(14)
            .and_then(|i| state.process_uids.get_mut(i as usize))
        {
            *uid = actor.uid;
            publish(&state);
        }
    }
}
pub fn desktop_actor() -> Result<Actor, Status> {
    let s = snapshot()?;
    s.uid(s.desktop_uid)
        .filter(|u| u.enabled())
        .map(|u| u.actor())
        .ok_or(Status::AccessDenied)
}

fn execute(state: &mut Snapshot, actor: Actor, peer: u32, command: &str) -> Result<String, Status> {
    let words: Vec<_> = command.split_whitespace().collect();
    match words.as_slice() {
        [] | ["list"] => {
            let mut output = String::new();
            for user in state.users.iter().filter(|u| u.name[0] != 0) {
                let _ = writeln!(
                    output,
                    "{} uid={} gid={} role={} enabled={} keys={} home={}",
                    user.name(),
                    user.uid,
                    user.gid,
                    role_name(user.role),
                    user.enabled(),
                    user.key_count,
                    user.home()
                );
            }
            return Ok(output);
        }
        ["whoami"] => {
            let u = state.uid(actor.uid).ok_or(Status::AccessDenied)?;
            return Ok(format!(
                "{} uid={} role={}\n",
                u.name(),
                u.uid,
                role_name(u.role)
            ));
        }
        ["keys", name] => {
            let user = state.find(name).ok_or(Status::NotFound)?;
            if !actor.administrator() && actor.uid != user.uid {
                return Err(Status::AccessDenied);
            }
            let mut output = String::new();
            for (index, key) in user.keys[..user.key_count as usize].iter().enumerate() {
                let _ = write!(output, "{index} ");
                for byte in key {
                    let _ = write!(output, "{byte:02x}");
                }
                output.push('\n');
            }
            return Ok(output);
        }
        ["audit"] if actor.administrator() => {
            let data = super::native::read(AUDIT, 65536)?;
            return Ok(
                String::from_utf8_lossy(&data[data.len().saturating_sub(4000)..]).into_owned(),
            );
        }
        _ => {}
    }
    if words.as_slice() == ["logout"] {
        if peer == 11 {
            return Err(Status::AccessDenied);
        }
        switch_desktop(state, GUEST_UID);
        audit(actor.uid, "logout", "desktop")?;
        return Ok("desktop logged out\n".to_string());
    }
    if !actor.administrator() {
        return Err(Status::AccessDenied);
    }
    let mut revoked = None;
    match words.as_slice() {
        ["add", name, role] => {
            let uid = state.add_user(name, parse_role(role)?)?;
            create_home(state.uid(uid).unwrap())?;
        }
        ["role", name, role] => {
            let uid = state.find(name).ok_or(Status::NotFound)?.uid;
            state.set_role(uid, parse_role(role)?)?;
            revoked = Some(uid);
        }
        ["enable", name] | ["disable", name] => {
            let uid = state.find(name).ok_or(Status::NotFound)?.uid;
            state.set_enabled(uid, words[0] == "enable")?;
            revoked = Some(uid);
        }
        ["key-add", name, value] => {
            let uid = state.find(name).ok_or(Status::NotFound)?.uid;
            state.add_key(uid, parse_key(value)?)?;
        }
        ["key-remove", name, index] => {
            let uid = state.find(name).ok_or(Status::NotFound)?.uid;
            state.remove_key(uid, index.parse().map_err(|_| Status::Invalid)?)?;
            revoked = Some(uid);
        }
        ["switch", name] => {
            let uid = state
                .find(name)
                .filter(|u| u.enabled())
                .ok_or(Status::NotFound)?
                .uid;
            switch_desktop(state, uid);
        }
        ["logout"] => {
            switch_desktop(state, GUEST_UID);
        }
        ["host-key", "rotate"] => {
            let mut secret = [0; 32];
            microsystem_user_rt::random_fill(boot_cap::RANDOM_SOURCE, &mut secret)?;
            super::native::atomic_write_mode("/.system/ssh/host.key", &secret, 0o600)?;
            super::native::fs(Fs::Chmod, "/.system/ssh/host.key", &[], 0o600)?;
            audit(actor.uid, "host-key-rotated", "sshd")?;
            state.revoke_peer(11);
            publish(state);
            let _ = microsystem_user_rt::service_stop(11, -15);
            return Ok("SSH host key rotated; reconnect with the new fingerprint\n".to_string());
        }
        _ => return Err(Status::Invalid),
    }
    save(state)?;
    // A committed account change is published even if the audit disk is full.
    // The console reports that failure; authorization must never retain an old key.
    let audited = audit(
        actor.uid,
        words[0],
        words.get(1).copied().unwrap_or("guest"),
    );
    publish(state);
    if let Some(uid) = revoked {
        terminate_user(state, uid);
    }
    audited?;
    Ok(format!("user: {} saved caller={peer}\n", words[0]))
}
fn switch_desktop(state: &mut Snapshot, uid: u32) {
    for slot in 0..16 {
        if super::SCRIPT_GUI_EVENTS[slot].load(core::sync::atomic::Ordering::Acquire) != 0 {
            let _ = microsystem_user_rt::thread_kill(14 + slot as u64, -15);
        }
    }
    state.desktop_uid = uid;
    for index in [6, 7, 8, 9] {
        state.core_uids[index] = uid;
    }
    publish(state);
    for pid in [7, 8, 9, 10] {
        let _ = microsystem_user_rt::service_stop(pid, -15);
    }
}
fn terminate_user(state: &mut Snapshot, uid: u32) {
    for (slot, owner) in state.process_uids.iter().enumerate() {
        if *owner == uid {
            let _ = microsystem_user_rt::thread_kill(14 + slot as u64, -15);
        }
    }
    if state.desktop_uid == uid {
        switch_desktop(state, GUEST_UID);
    }
}
fn role_name(role: u32) -> &'static str {
    match role {
        ADMIN => "admin",
        OPERATOR => "operator",
        _ => "reader",
    }
}
fn parse_role(role: &str) -> Result<u32, Status> {
    match role {
        "admin" => Ok(ADMIN),
        "operator" => Ok(OPERATOR),
        "reader" => Ok(READER),
        _ => Err(Status::Invalid),
    }
}
fn parse_key(text: &str) -> Result<[u8; 32], Status> {
    if text.len() != 64 || !text.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(Status::Invalid);
    }
    let mut key = [0; 32];
    for (i, byte) in key.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&text[i * 2..i * 2 + 2], 16).map_err(|_| Status::Invalid)?;
    }
    Ok(key)
}
fn create_home(user: &microsystem_identity::User) -> Result<(), Status> {
    super::native::fs(Fs::Mkdir, "/home", &[], 0)?;
    if user.home() != "/" {
        super::native::fs(Fs::Mkdir, user.home(), &[], 0)?;
        super::native::fs(
            Fs::Chown,
            user.home(),
            &[],
            u64::from(user.uid) | u64::from(user.gid) << 32,
        )?;
        super::native::fs(Fs::Chmod, user.home(), &[], 0o700)?;
    }
    Ok(())
}
fn save(state: &Snapshot) -> Result<(), Status> {
    let mut data = Vec::with_capacity(ACCOUNT_BYTES + 32);
    data.extend_from_slice(b"MICROID1");
    data.extend_from_slice(&1u32.to_le_bytes());
    data.extend_from_slice(&state.next_uid.to_le_bytes());
    data.extend_from_slice(unsafe {
        core::slice::from_raw_parts(
            state.users.as_ptr().cast::<u8>(),
            core::mem::size_of_val(&state.users),
        )
    });
    let digest = Sha256::digest(&data);
    data.extend_from_slice(&digest);
    super::native::atomic_write_mode(ACCOUNTS, &data, 0o600)?;
    super::native::fs(Fs::Chmod, ACCOUNTS, &[], 0o600)?;
    super::native::fs(Fs::Fsync, ACCOUNTS, &[], 0)?;
    Ok(())
}
fn load() -> Result<Snapshot, Status> {
    let mut state = Snapshot::defaults(DEFAULT_KEY);
    match super::native::read(ACCOUNTS, ACCOUNT_BYTES + 32) {
        Ok(data) => {
            if data.len() != ACCOUNT_BYTES + 32
                || &data[..8] != b"MICROID1"
                || data[8..12] != 1u32.to_le_bytes()
                || Sha256::digest(&data[..ACCOUNT_BYTES]).as_slice() != &data[ACCOUNT_BYTES..]
            {
                return Err(Status::Corrupt);
            }
            state.next_uid = u32::from_le_bytes(data[12..16].try_into().unwrap());
            unsafe {
                core::ptr::copy_nonoverlapping(
                    data[16..ACCOUNT_BYTES].as_ptr(),
                    state.users.as_mut_ptr().cast::<u8>(),
                    ACCOUNT_BYTES - 16,
                );
            }
            state.validate()?;
        }
        Err(Status::NotFound) => {
            for user in state.users.iter().filter(|u| u.name[0] != 0) {
                create_home(user)?;
            }
            super::native::fs(Fs::Mkdir, "/data", &[], 0)?;
            super::native::fs(Fs::Chmod, "/data", &[], 0o777)?;
            save(&state)?;
            audit(0, "accounts-initialized", "root,micro,guest")?;
        }
        Err(status) => return Err(status),
    }
    super::native::fs(Fs::Mkdir, "/.system/ssh", &[], 0)?;
    match super::native::fs(Fs::Stat, "/.system/ssh/host.key", &[], 0) {
        Err(Status::NotFound) => {
            let mut secret = [0; 32];
            microsystem_user_rt::random_fill(boot_cap::RANDOM_SOURCE, &mut secret)?;
            super::native::atomic_write_mode("/.system/ssh/host.key", &secret, 0o600)?;
        }
        Ok(_) => {}
        Err(status) => return Err(status),
    }
    super::native::fs(Fs::Chmod, "/.system/ssh/host.key", &[], 0o600)?;
    super::native::fs(Fs::Fsync, "/.system/ssh/host.key", &[], 0)?;
    Ok(state)
}
pub(super) fn audit(uid: u32, action: &str, subject: &str) -> Result<(), Status> {
    if let Ok(stat) = super::native::fs(Fs::Stat, AUDIT, &[], 0) {
        if stat.words[1] > 60_000 {
            super::native::fs(Fs::Replace, AUDIT, b"/.system/audit.previous", 0)?;
        }
    }
    let line = format!(
        "{} uid={uid} action={action} subject={subject}\n",
        microsystem_user_rt::clock_realtime().unwrap_or(0)
    );
    super::native::fs(Fs::Append, AUDIT, line.as_bytes(), 0)?;
    super::native::fs(Fs::Chmod, AUDIT, &[], 0o600)?;
    super::native::fs(Fs::Fsync, AUDIT, &[], 0)?;
    Ok(())
}
