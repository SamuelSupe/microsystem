use microsystem_abi::{Message, Status, protocol, service};
use microsystem_init::{dependencies_ready, depends_on, retry_delay};

const POLL_NS: u64 = 100_000_000;
const START_TIMEOUT_NS: u64 = 120_000_000_000;
const STABLE_NS: u64 = 60_000_000_000;

#[derive(Clone, Copy, Default)]
struct Recovery {
    held: bool,
    failed_start: u64,
    attempts: u8,
    next_ns: u64,
}

pub struct Supervisor {
    recovery: [Recovery; service::NAMES.len()],
    next_poll: u64,
    configured: (u64, u64),
}

impl Supervisor {
    pub fn new() -> Self {
        Self {
            recovery: [Recovery::default(); service::NAMES.len()],
            next_poll: 0,
            configured: (1, 1),
        }
    }

    pub fn request(&mut self, request: &Message, reply: &mut Message) -> Status {
        if request.protocol != protocol::SERVICE {
            return Status::Invalid;
        }
        let pid = request.words[0];
        let Some(task) = pid
            .checked_sub(1)
            .filter(|task| *task < service::NAMES.len() as u64)
        else {
            return Status::Invalid;
        };
        let task = task as usize;
        let mut info = service::InfoV1::EMPTY;
        if let Err(status) = microsystem_user_rt::service_status(pid, &mut info) {
            return status;
        }
        match request.opcode {
            value if value == service::Operation::List as u16 => {
                reply.words = [
                    info.pid as u64,
                    info.state as u64,
                    info.starts,
                    info.exit_status as u64,
                    u64::from(self.recovery[task].held),
                    Status::Ok as u64,
                ];
                Status::Ok
            }
            value
                if value == service::Operation::Restart as u16
                    || value == service::Operation::Stop as u16 =>
            {
                if !super::identity::actor(request.words[3])
                    .is_ok_and(|actor| actor.administrator())
                {
                    return Status::AccessDenied;
                }
                if task == 0 || info.starts == 0 {
                    return Status::AccessDenied;
                }
                self.recovery[task].held = value == service::Operation::Stop as u16;
                self.recovery[task].attempts = 0;
                self.recovery[task].next_ns = 0;
                match microsystem_user_rt::service_stop(pid, -15) {
                    Ok(()) | Err(Status::NotFound) => Status::Ok,
                    Err(status) => status,
                }
            }
            _ => Status::Invalid,
        }
    }

    pub fn poll(&mut self) {
        let now = microsystem_user_rt::clock_now().unwrap_or(0);
        if now < self.next_poll {
            return;
        }
        self.next_poll = now.saturating_add(POLL_NS);
        let mut infos = [service::InfoV1::EMPTY; service::NAMES.len()];
        for (task, info) in infos.iter_mut().enumerate() {
            if microsystem_user_rt::service_status((task + 1) as u64, info).is_err() {
                return;
            }
        }
        for task in 1..infos.len() {
            let info = infos[task];
            if info.state == service::ONLINE && now.saturating_sub(info.online_ns) >= STABLE_NS {
                self.recovery[task].attempts = 0;
            }
            if info.state == service::STARTING
                && now.saturating_sub(info.started_ns) >= START_TIMEOUT_NS
            {
                let _ = microsystem_user_rt::service_stop(info.pid as u64, Status::TimedOut as i64);
            }
            if info.starts != 0
                && matches!(info.state, service::STOPPED | service::QUIESCE_FAILED)
                && self.recovery[task].failed_start != info.starts
            {
                self.recovery[task].failed_start = info.starts;
                self.stop_dependants(task, &infos);
                if matches!(task, 3 | 6 | 10 | 11) {
                    super::terminate_script_sessions();
                }
            }
        }

        for task in [1, 5, 2, 3, 12, 11, 10, 6, 7, 8, 9, 4] {
            let info = infos[task];
            let recovery = &mut self.recovery[task];
            if info.starts == 0
                || info.state != service::STOPPED
                || recovery.held
                || now < recovery.next_ns
                || !dependencies_ready(task, &infos)
            {
                continue;
            }
            if recovery.attempts >= 8 {
                recovery.held = true;
                report(task, b" restart-budget-exhausted=true\n");
                continue;
            }
            recovery.attempts += 1;
            recovery.next_ns = now.saturating_add(retry_delay(recovery.attempts));
            match microsystem_user_rt::thread_start(super::program_id(service::NAMES[task])) {
                Ok(_) => report(task, b" restarting=true\n"),
                Err(_) => report(task, b" restart-failed=true\n"),
            }
        }

        let mut block = service::InfoV1::EMPTY;
        let mut devmgr = service::InfoV1::EMPTY;
        let _ = microsystem_user_rt::service_status(3, &mut block);
        let _ = microsystem_user_rt::service_status(6, &mut devmgr);
        let pair = (block.starts, devmgr.starts);
        if pair != self.configured
            && block.state == service::STARTING
            && matches!(devmgr.state, service::STARTING | service::ONLINE)
        {
            if super::delegate_block_transport().is_ok() {
                self.configured = pair;
            } else {
                let _ = microsystem_user_rt::service_stop(3, Status::Io as i64);
            }
        }
    }

    fn stop_dependants(&mut self, failed: usize, infos: &[service::InfoV1; service::NAMES.len()]) {
        for task in (1..infos.len()).rev() {
            if depends_on(task, failed) && infos[task].starts != 0 {
                let _ = microsystem_user_rt::service_stop((task + 1) as u64, Status::Io as i64);
            }
        }
    }
}

fn report(task: usize, suffix: &[u8]) {
    let mut line = [0; 128];
    let mut length = 0;
    for part in [
        b"[supervisor] service=".as_slice(),
        service::NAMES[task].as_bytes(),
        suffix,
    ] {
        if length + part.len() > line.len() {
            return;
        }
        line[length..length + part.len()].copy_from_slice(part);
        length += part.len();
    }
    let _ = microsystem_user_rt::debug_write(&line[..length]);
}
