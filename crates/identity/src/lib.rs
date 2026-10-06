#![no_std]

use core::sync::atomic::{AtomicU64, Ordering};
use microsystem_abi::Status;

pub const MAX_USERS: usize = 8;
pub const MAX_KEYS: usize = 4;
pub const MAX_CREDENTIALS: usize = 32;
pub const GUEST_UID: u32 = 65534;
pub const SNAPSHOT_MAGIC: u64 = u64::from_le_bytes(*b"MICROID1");
pub const ENABLED: u32 = 1;
pub const ADMIN: u32 = 0;
pub const OPERATOR: u32 = 1;
pub const READER: u32 = 2;

#[repr(C)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct User {
    pub uid: u32,
    pub gid: u32,
    pub epoch: u32,
    pub flags: u32,
    pub role: u32,
    pub key_count: u32,
    pub name: [u8; 32],
    pub home: [u8; 64],
    pub keys: [[u8; 32]; MAX_KEYS],
}

impl User {
    pub const EMPTY: Self = Self {
        uid: 0,
        gid: 0,
        epoch: 0,
        flags: 0,
        role: READER,
        key_count: 0,
        name: [0; 32],
        home: [0; 64],
        keys: [[0; 32]; MAX_KEYS],
    };
    pub fn name(&self) -> &str {
        text(&self.name)
    }
    pub fn home(&self) -> &str {
        text(&self.home)
    }
    pub fn enabled(&self) -> bool {
        self.flags & ENABLED != 0
    }
    pub fn actor(&self) -> Actor {
        Actor {
            uid: self.uid,
            gid: self.gid,
            role: self.role,
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Credential {
    pub cookie: u64,
    pub expires_ns: u64,
    pub uid: u32,
    pub user_epoch: u32,
    pub peer_pid: u32,
    pub reserved: u32,
}
impl Credential {
    pub const EMPTY: Self = Self {
        cookie: 0,
        expires_ns: 0,
        uid: 0,
        user_epoch: 0,
        peer_pid: 0,
        reserved: 0,
    };
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Actor {
    pub uid: u32,
    pub gid: u32,
    pub role: u32,
}
impl Actor {
    pub const SYSTEM: Self = Self {
        uid: 0,
        gid: 0,
        role: ADMIN,
    };
    pub fn administrator(self) -> bool {
        self.role == ADMIN
    }
    pub fn access(self, owner: u32, group: u32, mode: u32, requested: u32) -> bool {
        if self.role == READER && requested & 2 != 0 {
            return false;
        }
        if self.administrator() {
            return true;
        }
        let shift = if self.uid == owner {
            6
        } else if self.gid == group {
            3
        } else {
            0
        };
        ((mode >> shift) & requested) == requested
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Snapshot {
    pub magic: u64,
    pub version: u32,
    pub next_uid: u32,
    pub desktop_uid: u32,
    pub initialized: u32,
    pub core_uids: [u32; 16],
    pub process_uids: [u32; 16],
    pub users: [User; MAX_USERS],
    pub credentials: [Credential; MAX_CREDENTIALS],
}

const SNAPSHOT_WORDS: usize = core::mem::size_of::<Snapshot>() / 8;
const _: () = assert!(core::mem::size_of::<Snapshot>() == 3160);
const _: () = assert!(core::mem::size_of::<User>() == 248);

/// Publishes to the init-owned, page-aligned shared frame. Only init may call
/// this; every other service maps the frame readonly. All shared words are
/// atomic so readers never race ordinary Rust loads/stores.
pub unsafe fn publish(address: usize, snapshot: &Snapshot) {
    let frame = address as *const AtomicU64;
    let sequence = unsafe { &*frame };
    let previous = sequence.load(Ordering::Relaxed) & !1;
    sequence.store(previous.wrapping_add(1), Ordering::SeqCst);
    let source = (snapshot as *const Snapshot).cast::<u64>();
    for index in 0..SNAPSHOT_WORDS {
        let word = unsafe { core::ptr::read(source.add(index)) };
        unsafe { &*frame.add(index + 1) }.store(word, Ordering::Relaxed);
    }
    sequence.store(previous.wrapping_add(2), Ordering::Release);
}

/// Copies one consistent snapshot; Busy allows a caller to retry an init
/// publication rather than accepting a partly changed credential table.
pub unsafe fn read_shared(address: usize) -> Result<Snapshot, Status> {
    let frame = address as *const AtomicU64;
    for _ in 0..4 {
        let before = unsafe { &*frame }.load(Ordering::Acquire);
        if before == 0 || before & 1 != 0 {
            continue;
        }
        let mut snapshot = Snapshot::EMPTY;
        let destination = (&mut snapshot as *mut Snapshot).cast::<u64>();
        for index in 0..SNAPSHOT_WORDS {
            unsafe {
                core::ptr::write(
                    destination.add(index),
                    (&*frame.add(index + 1)).load(Ordering::Relaxed),
                );
            }
        }
        core::sync::atomic::fence(Ordering::Acquire);
        if before == unsafe { &*frame }.load(Ordering::Acquire) {
            if snapshot.magic != SNAPSHOT_MAGIC
                || snapshot.version != 1
                || snapshot.initialized != 1
            {
                return Err(Status::Corrupt);
            }
            return Ok(snapshot);
        }
    }
    Err(Status::Busy)
}

impl Snapshot {
    pub const EMPTY: Self = Self {
        magic: SNAPSHOT_MAGIC,
        version: 1,
        next_uid: 1001,
        desktop_uid: 0,
        initialized: 0,
        core_uids: [0; 16],
        process_uids: [GUEST_UID; 16],
        users: [User::EMPTY; MAX_USERS],
        credentials: [Credential::EMPTY; MAX_CREDENTIALS],
    };

    pub fn defaults(micro_key: [u8; 32]) -> Self {
        let mut snapshot = Self::EMPTY;
        snapshot.users[0] = user(0, "root", "/", ADMIN).unwrap();
        snapshot.users[1] = user(1000, "micro", "/home/micro", OPERATOR).unwrap();
        snapshot.users[1].keys[0] = micro_key;
        snapshot.users[1].key_count = 1;
        snapshot.users[2] = user(GUEST_UID, "guest", "/home/guest", READER).unwrap();
        snapshot.initialized = 1;
        snapshot
    }

    pub fn find(&self, name: &str) -> Option<&User> {
        self.users
            .iter()
            .find(|user| user.name() == name && user.name[0] != 0)
    }
    pub fn uid(&self, uid: u32) -> Option<&User> {
        self.users
            .iter()
            .find(|user| user.uid == uid && user.name[0] != 0)
    }
    pub fn add_user(&mut self, name: &str, role: u32) -> Result<u32, Status> {
        if !valid_name(name) || role > READER {
            return Err(Status::Invalid);
        }
        if self.find(name).is_some() {
            return Err(Status::Busy);
        }
        let slot = self
            .users
            .iter()
            .position(|user| user.name[0] == 0)
            .ok_or(Status::NoSpace)?;
        let uid = self.next_uid;
        if uid >= GUEST_UID {
            return Err(Status::NoSpace);
        }
        let mut home = [0; 64];
        let prefix = b"/home/";
        home[..prefix.len()].copy_from_slice(prefix);
        home[prefix.len()..prefix.len() + name.len()].copy_from_slice(name.as_bytes());
        self.users[slot] = user(uid, name, text(&home), role)?;
        self.next_uid += 1;
        Ok(uid)
    }

    pub fn set_enabled(&mut self, uid: u32, enabled: bool) -> Result<(), Status> {
        if uid == 0 || uid == GUEST_UID {
            return Err(Status::AccessDenied);
        }
        let user = self
            .users
            .iter_mut()
            .find(|user| user.uid == uid && user.name[0] != 0)
            .ok_or(Status::NotFound)?;
        let epoch = user.epoch.checked_add(1).ok_or(Status::NoSpace)?;
        user.flags = if enabled { ENABLED } else { 0 };
        user.epoch = epoch;
        self.revoke(uid);
        Ok(())
    }

    pub fn set_role(&mut self, uid: u32, role: u32) -> Result<(), Status> {
        if role > READER {
            return Err(Status::Invalid);
        }
        if uid == 0 || uid == GUEST_UID {
            return Err(Status::AccessDenied);
        }
        let user = self
            .users
            .iter_mut()
            .find(|user| user.uid == uid && user.name[0] != 0)
            .ok_or(Status::NotFound)?;
        let epoch = user.epoch.checked_add(1).ok_or(Status::NoSpace)?;
        user.role = role;
        user.epoch = epoch;
        self.revoke(uid);
        Ok(())
    }

    pub fn add_key(&mut self, uid: u32, key: [u8; 32]) -> Result<(), Status> {
        if key == [0; 32] {
            return Err(Status::Invalid);
        }
        let user = self
            .users
            .iter_mut()
            .find(|user| user.uid == uid && user.name[0] != 0)
            .ok_or(Status::NotFound)?;
        if user.keys[..user.key_count as usize].contains(&key) {
            return Err(Status::Busy);
        }
        if user.key_count as usize == MAX_KEYS {
            return Err(Status::NoSpace);
        }
        user.keys[user.key_count as usize] = key;
        user.key_count += 1;
        Ok(())
    }

    pub fn remove_key(&mut self, uid: u32, index: usize) -> Result<(), Status> {
        let user = self
            .users
            .iter_mut()
            .find(|user| user.uid == uid && user.name[0] != 0)
            .ok_or(Status::NotFound)?;
        if index >= user.key_count as usize {
            return Err(Status::NotFound);
        }
        let epoch = user.epoch.checked_add(1).ok_or(Status::NoSpace)?;
        user.keys
            .copy_within(index + 1..user.key_count as usize, index);
        user.key_count -= 1;
        user.keys[user.key_count as usize] = [0; 32];
        user.epoch = epoch;
        self.revoke(uid);
        Ok(())
    }

    pub fn issue(
        &mut self,
        uid: u32,
        peer_pid: u32,
        cookie: u64,
        expires_ns: u64,
        now: u64,
    ) -> Result<(), Status> {
        let user = self
            .uid(uid)
            .filter(|user| user.enabled())
            .ok_or(Status::AccessDenied)?;
        let epoch = user.epoch;
        if cookie == 0
            || peer_pid == 0
            || expires_ns <= now
            || self
                .credentials
                .iter()
                .any(|credential| credential.cookie == cookie)
        {
            return Err(Status::Invalid);
        }
        let slot = self
            .credentials
            .iter()
            .position(|credential| credential.cookie == 0 || credential.expires_ns <= now)
            .ok_or(Status::NoSpace)?;
        self.credentials[slot] = Credential {
            cookie,
            expires_ns,
            uid,
            user_epoch: epoch,
            peer_pid,
            reserved: 0,
        };
        Ok(())
    }

    pub fn actor(&self, peer_pid: u32, cookie: u64, now: u64) -> Result<Actor, Status> {
        if self.initialized != 1 {
            return Err(Status::Busy);
        }
        if cookie != 0 {
            let credential = self
                .credentials
                .iter()
                .find(|credential| {
                    credential.cookie == cookie
                        && credential.peer_pid == peer_pid
                        && credential.expires_ns > now
                })
                .ok_or(Status::AccessDenied)?;
            return self
                .uid(credential.uid)
                .filter(|user| user.enabled() && user.epoch == credential.user_epoch)
                .map(User::actor)
                .ok_or(Status::AccessDenied);
        }
        let uid = if (1..=13).contains(&peer_pid) {
            self.core_uids[peer_pid as usize - 1]
        } else if (14..30).contains(&peer_pid) {
            self.process_uids[peer_pid as usize - 14]
        } else {
            return Err(Status::AccessDenied);
        };
        self.uid(uid)
            .filter(|user| user.enabled())
            .map(User::actor)
            .ok_or(Status::AccessDenied)
    }

    pub fn revoke(&mut self, uid: u32) {
        for credential in &mut self.credentials {
            if credential.uid == uid {
                *credential = Credential::EMPTY;
            }
        }
    }
    pub fn revoke_peer(&mut self, peer_pid: u32) {
        for credential in &mut self.credentials {
            if credential.peer_pid == peer_pid {
                *credential = Credential::EMPTY;
            }
        }
    }

    pub fn validate(&self) -> Result<(), Status> {
        if self.magic != SNAPSHOT_MAGIC
            || self.version != 1
            || self.initialized != 1
            || self.next_uid < 1001
            || self.next_uid > GUEST_UID
        {
            return Err(Status::Corrupt);
        }
        for (index, user) in self
            .users
            .iter()
            .enumerate()
            .filter(|(_, user)| user.name[0] != 0)
        {
            if !valid_name(user.name())
                || user.name().len() == 32
                || user.role > READER
                || user.flags & !ENABLED != 0
                || user.epoch == 0
                || user.key_count as usize > MAX_KEYS
                || !user.home().starts_with('/')
                || user.home().len() == 64
                || user.home().split('/').any(|part| part == "..")
            {
                return Err(Status::Corrupt);
            }
            if self.users[..index].iter().any(|other| {
                other.name[0] != 0 && (other.uid == user.uid || other.name() == user.name())
            }) {
                return Err(Status::Corrupt);
            }
            for (index, key) in user.keys[..user.key_count as usize].iter().enumerate() {
                if *key == [0; 32] || user.keys[..index].contains(key) {
                    return Err(Status::Corrupt);
                }
            }
            if user.uid != 0 && user.uid != GUEST_UID && user.uid >= self.next_uid {
                return Err(Status::Corrupt);
            }
        }
        if !self
            .uid(0)
            .is_some_and(|user| user.enabled() && user.role == ADMIN && user.name() == "root")
            || !self
                .uid(GUEST_UID)
                .is_some_and(|user| user.enabled() && user.role == READER && user.name() == "guest")
        {
            return Err(Status::Corrupt);
        }
        Ok(())
    }
}

pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 31
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_-".contains(&byte))
}
fn text(bytes: &[u8]) -> &str {
    core::str::from_utf8(
        &bytes[..bytes
            .iter()
            .position(|byte| *byte == 0)
            .unwrap_or(bytes.len())],
    )
    .unwrap_or("")
}
fn user(uid: u32, name: &str, home: &str, role: u32) -> Result<User, Status> {
    if !valid_name(name) || home.len() > 63 || !home.starts_with('/') || role > READER {
        return Err(Status::Invalid);
    }
    let mut user = User {
        uid,
        gid: uid,
        epoch: 1,
        flags: ENABLED,
        role,
        ..User::EMPTY
    };
    user.name[..name.len()].copy_from_slice(name.as_bytes());
    user.home[..home.len()].copy_from_slice(home.as_bytes());
    Ok(user)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn credentials_are_bound_to_peer_expiry_and_user_epoch() {
        let mut snapshot = Snapshot::defaults([1; 32]);
        snapshot.issue(1000, 11, 123, 100, 1).unwrap();
        assert_eq!(snapshot.actor(11, 123, 99).unwrap().uid, 1000);
        assert_eq!(snapshot.actor(5, 123, 99), Err(Status::AccessDenied));
        assert_eq!(snapshot.actor(11, 123, 100), Err(Status::AccessDenied));
        snapshot.remove_key(1000, 0).unwrap();
        assert_eq!(snapshot.actor(11, 123, 99), Err(Status::AccessDenied));
        snapshot.issue(1000, 11, 124, 200, 100).unwrap();
        snapshot.set_enabled(1000, false).unwrap();
        assert_eq!(snapshot.actor(11, 124, 101), Err(Status::AccessDenied));
    }
    #[test]
    fn role_permissions_follow_owner_group_and_directory_search_bits() {
        let operator = Actor {
            uid: 1000,
            gid: 1000,
            role: OPERATOR,
        };
        assert!(operator.access(1000, 1000, 0o600, 6));
        assert!(!operator.access(0, 0, 0o600, 4));
        assert!(operator.access(0, 1000, 0o640, 4));
        assert!(!operator.access(0, 1000, 0o640, 1));
        let reader = Actor {
            role: READER,
            ..operator
        };
        assert!(!reader.access(1000, 1000, 0o777, 2));
        assert!(Actor::SYSTEM.access(1000, 1000, 0, 7));
    }
    #[test]
    fn accounts_have_unique_ids_and_readonly_guest_is_not_administrator() {
        let mut snapshot = Snapshot::defaults([1; 32]);
        let uid = snapshot.add_user("alice", READER).unwrap();
        assert_eq!(snapshot.uid(uid).unwrap().home(), "/home/alice");
        assert_eq!(snapshot.add_user("alice", ADMIN), Err(Status::Busy));
        assert_eq!(snapshot.add_user("../escape", ADMIN), Err(Status::Invalid));
        snapshot.core_uids[7] = uid;
        assert!(!snapshot.actor(8, 0, 0).unwrap().administrator());
        assert_eq!(snapshot.set_enabled(0, false), Err(Status::AccessDenied));
        assert!(core::mem::size_of::<Snapshot>() <= 4096 - 8);
        snapshot.validate().unwrap();
        snapshot.users[1].key_count = MAX_KEYS as u32 + 1;
        assert_eq!(snapshot.validate(), Err(Status::Corrupt));
    }
    #[test]
    fn publication_retries_in_progress_changes_and_reads_the_complete_frame() {
        let frame: [AtomicU64; 512] = core::array::from_fn(|_| AtomicU64::new(0));
        let address = frame.as_ptr() as usize;
        assert!(matches!(unsafe { read_shared(address) }, Err(Status::Busy)));
        let snapshot = Snapshot::defaults([1; 32]);
        unsafe {
            publish(address, &snapshot);
        }
        let decoded = unsafe { read_shared(address) }.unwrap();
        assert_eq!(decoded.users, snapshot.users);
        frame[0].store(3, Ordering::Release);
        assert!(matches!(unsafe { read_shared(address) }, Err(Status::Busy)));
    }
}
