#![no_std]
#![no_main]

extern crate alloc;

use core::panic::PanicInfo;
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use microsystem_abi::{
    CapHandle, Message, ObjectType, Rights, Status, ThreadLaunchV2, boot_cap, filesystem, gui,
    message_cap_move, process, protocol, script,
};
use microsystem_block::Operation as BlockOperation;
mod identity;
mod native;
mod supervisor;

static PROCESS_PROGRAMS: [AtomicU64; process::MAX_APPLICATIONS] =
    [const { AtomicU64::new(0) }; process::MAX_APPLICATIONS];
static SCRIPT_NOTIFICATIONS: [AtomicU32; process::MAX_APPLICATIONS] =
    [const { AtomicU32::new(0) }; process::MAX_APPLICATIONS];
static SCRIPT_TOKENS: [AtomicU64; process::MAX_APPLICATIONS] =
    [const { AtomicU64::new(0) }; process::MAX_APPLICATIONS];
static SCRIPT_REGIONS: [AtomicU32; process::MAX_APPLICATIONS] =
    [const { AtomicU32::new(0) }; process::MAX_APPLICATIONS];
static SCRIPT_CHILD_OWNER: [AtomicU64; process::MAX_APPLICATIONS] =
    [const { AtomicU64::new(0) }; process::MAX_APPLICATIONS];
static SCRIPT_GUI_COMMANDS: [AtomicU32; process::MAX_APPLICATIONS] =
    [const { AtomicU32::new(0) }; process::MAX_APPLICATIONS];
static SCRIPT_GUI_EVENTS: [AtomicU32; process::MAX_APPLICATIONS] =
    [const { AtomicU32::new(0) }; process::MAX_APPLICATIONS];
static SCRIPT_GUI_ENDPOINT_SLOTS: [AtomicU32; process::MAX_APPLICATIONS] =
    [const { AtomicU32::new(0) }; process::MAX_APPLICATIONS];
static SCRIPT_GUI_CLOSE_DEADLINES: [AtomicU64; process::MAX_APPLICATIONS] =
    [const { AtomicU64::new(0) }; process::MAX_APPLICATIONS];
static GUI_ENDPOINT_OWNERS: [AtomicU64; gui::MAX_DYNAMIC_CLIENTS] =
    [const { AtomicU64::new(0) }; gui::MAX_DYNAMIC_CLIENTS];
static GUI_AVAILABLE: AtomicU32 = AtomicU32::new(0);
static DESKTOP_LAUNCH_PENDING: AtomicU32 = AtomicU32::new(0);
const APPLICATION_WAIT_NS: u64 = 2_000_000_000;
const DESKTOP_GUI_TIMEOUT_NS: u64 = 86_400_000_000_000;

#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    let _ = microsystem_user_rt::debug_write(b"[user] bootfs init ELF entered EL0\n");
    start_service("devmgr", 6);
    start_service("console", 2);
    verify_notification_deadline();
    let mut request = Message::new(protocol::CONSOLE, 0);
    request.words[0] = 0x10;
    let mut reply = Message::new(protocol::CONSOLE, 0);
    if microsystem_user_rt::ipc_call(boot_cap::CONSOLE_ENDPOINT, &request, &mut reply, 0).is_err()
        || reply.protocol != protocol::CONSOLE
        || reply.words[0] != 0x11
    {
        microsystem_user_rt::exit(2);
    }
    let _ = microsystem_user_rt::debug_write(b"[ipc] resident init->console reply=0x11\n");
    verify_notification_signal();
    verify_notification_blocking_wake();
    let expired_request = Message::new(protocol::CONSOLE, 0);
    let mut expired_reply = Message::new(protocol::CONSOLE, 0);
    if microsystem_user_rt::ipc_call(
        boot_cap::CONSOLE_ENDPOINT,
        &expired_request,
        &mut expired_reply,
        1,
    ) != Err(microsystem_abi::Status::TimedOut)
    {
        microsystem_user_rt::exit(8);
    }
    let _ = microsystem_user_rt::debug_write(b"[ipc] resident absolute-deadline timeout=true\n");
    let rights = Rights(Rights::READ.0 | Rights::GRANT.0 | Rights::MANAGE.0);
    let notification = microsystem_user_rt::object_create(ObjectType::Notification, rights)
        .unwrap_or_else(|_| microsystem_user_rt::exit(5));
    let child = microsystem_user_rt::cap_copy(notification, Rights::READ)
        .unwrap_or_else(|_| microsystem_user_rt::exit(6));
    if microsystem_user_rt::cap_revoke(notification).ok() != Some(1)
        || microsystem_user_rt::cap_delete(child).is_ok()
        || microsystem_user_rt::cap_delete(notification).is_err()
    {
        microsystem_user_rt::exit(7);
    }
    let _ = microsystem_user_rt::debug_write(
        b"[cap] resident generation rights revoke stale-handle=true\n",
    );
    verify_cross_task_capabilities();
    verify_remote_mapping_revoke();
    verify_endpoint_capability();
    verify_memory_pool_mapping();
    verify_process_kill();
    verify_application_capacity();
    verify_isolation_matrix();
    verify_privileged_fault();
    start_service("block", 3);
    start_service("mfs", 4);
    start_service("db", 13);
    start_service("shell", 5);
    let gui = delegate_block_transport().unwrap_or(false);
    start_service("netd", 12);
    start_service("sshd", 11);
    if gui {
        let _ = microsystem_user_rt::debug_write(
            b"[bootfs] root task started manifest services=devmgr,console,block,mfs,db,shell,netd,sshd,windowd,terminal,files,monitor\n",
        );
        start_service("windowd", 7);
        GUI_AVAILABLE.store(1, Ordering::Release);
        start_service("terminal", 8);
        start_service("files", 9);
        start_service("monitor", 10);
    } else {
        let _ = microsystem_user_rt::debug_write(
            b"[bootfs] root task started manifest services=devmgr,console,block,mfs,db,shell,netd,sshd\n",
        );
    }
    let _ = microsystem_user_rt::debug_write(b"[ipc] resident procman endpoint=4 ready\n");
    let _ = microsystem_user_rt::service_online();
    let mut pending_process = None;
    let mut pending_script = None;
    let mut pending_desktop_launch = None;
    let mut pending_service = None;
    let mut pending_native = None;
    let mut pending_identity = None;
    let mut identities = identity::Registry::new();
    let mut supervisor = supervisor::Supervisor::new();
    loop {
        identities.poll();
        let _ = service_endpoint(
            boot_cap::IDENTITY_ENDPOINT,
            &mut pending_identity,
            |request, reply| identities.request(request, reply),
        );
        for (name, status) in [
            (
                b"process".as_slice(),
                service_process_endpoint(&mut pending_process),
            ),
            (
                b"script".as_slice(),
                service_script_endpoint(&mut pending_script),
            ),
            (
                b"desktop-launch".as_slice(),
                service_desktop_launch_endpoint(&mut pending_desktop_launch),
            ),
        ] {
            if let Err(status) = status {
                let _ = microsystem_user_rt::debug_write(b"[gui] desktop launcher broker=");
                let _ = microsystem_user_rt::debug_write(name);
                let _ = microsystem_user_rt::debug_write_u64(
                    b" failed-status=",
                    (status as i64).unsigned_abs(),
                    b"\n",
                );
                microsystem_user_rt::exit(4);
            }
        }
        let _ = service_endpoint(
            boot_cap::SERVICE_ENDPOINT,
            &mut pending_service,
            |request, reply| supervisor.request(request, reply),
        );
        supervisor.poll();
        let _ = service_endpoint(
            boot_cap::APPLICATION_ENDPOINT,
            &mut pending_native,
            native::request,
        );
        launch_pending_desktop_application();
        reap_script_sessions();
        let _ = microsystem_user_rt::yield_now();
    }
}

fn service_process_endpoint(pending: &mut Option<Message>) -> Result<(), Status> {
    service_endpoint(
        boot_cap::PROCESS_ENDPOINT,
        pending,
        |request, reply| match request.protocol {
            protocol::PROCESS => handle_process(request, reply),
            protocol::TIME => handle_time(request, reply),
            _ => Status::Invalid,
        },
    )
}

fn service_script_endpoint(pending: &mut Option<Message>) -> Result<(), Status> {
    service_endpoint(boot_cap::SCRIPT_BROKER_ENDPOINT, pending, handle_script)
}

fn service_desktop_launch_endpoint(pending: &mut Option<Message>) -> Result<(), Status> {
    service_endpoint(boot_cap::GUI_LAUNCH_ENDPOINT, pending, |request, _reply| {
        if request.protocol != protocol::GUI
            || request.opcode != gui::Operation::LaunchApplication as u16
        {
            return Status::Invalid;
        }
        let Some(application) = gui::Application::from_u64(request.words[0]) else {
            return Status::Invalid;
        };
        if !matches!(
            application,
            gui::Application::Reader | gui::Application::Editor
        ) {
            return Status::AccessDenied;
        }
        match DESKTOP_LAUNCH_PENDING.compare_exchange(
            0,
            application as u32,
            Ordering::AcqRel,
            Ordering::Acquire,
        ) {
            Ok(_) => Status::Ok,
            Err(_) => Status::Busy,
        }
    })
}

fn launch_pending_desktop_application() {
    let pending = DESKTOP_LAUNCH_PENDING.load(Ordering::Acquire) as u64;
    let Some(application) = gui::Application::from_u64(pending) else {
        return;
    };
    // windowd supplies only this enum. Paths and launcher policy stay in init so a
    // compromised GUI service cannot turn the launcher into an arbitrary script broker.
    let (source, policy, path, argv) = match application {
        gui::Application::Reader => (
            b"--!mica 1\n--!allow gui.window\n--!allow net.browse\n--!allow fs.write:/data\n"
                .as_slice(),
            b"gui.window\nnet.browse\nfs.write:/data\n".as_slice(),
            b"/.system/examples/mica/browser.mica".as_slice(),
            b"".as_slice(),
        ),
        gui::Application::Editor => (
            b"--!mica 1\n--!allow gui.window\n--!allow fs.read:/data\n--!allow fs.write:/data\n"
                .as_slice(),
            b"gui.window\nfs.read:/data\nfs.write:/data\n".as_slice(),
            b"/.system/examples/mica/editor.mica".as_slice(),
            b"/data/note.txt\0".as_slice(),
        ),
        _ => return,
    };
    let mut prepared = Message::new(protocol::PROCESS, process::Operation::SpawnScript as u16);
    if prepare_script_session(&mut prepared) != Status::Ok {
        report_desktop_launch(application, Status::NoMemory);
        DESKTOP_LAUNCH_PENDING.store(0, Ordering::Release);
        return;
    }
    let region = prepared.caps[0];
    let result = match prepare_desktop_script_region(region, source, policy, path, argv) {
        Ok(()) => {
            let mut reply = Message::new(protocol::PROCESS, process::Operation::SpawnScript as u16);
            let status = match identity::desktop_actor() {
                Ok(actor) if actor.role != microsystem_identity::READER => {
                    launch_script_session(region, true, Some(application), &mut reply, actor)
                }
                _ => Status::AccessDenied,
            };
            if status == Status::Ok && reply.caps[1] != CapHandle::INVALID {
                let _ = microsystem_user_rt::cap_delete(reply.caps[1]);
            }
            (status == Status::Ok).then_some(()).ok_or(status)
        }
        Err(status) => {
            let _ = microsystem_user_rt::cap_delete(region);
            Err(status)
        }
    };
    report_desktop_launch(application, result.err().unwrap_or(Status::Ok));
    DESKTOP_LAUNCH_PENDING.store(0, Ordering::Release);
}

fn prepare_desktop_script_region(
    region: CapHandle,
    source: &[u8],
    policy: &[u8],
    path: &[u8],
    argv: &[u8],
) -> Result<(), Status> {
    if source.len() > script::SOURCE_BYTES
        || argv.len() > script::STDIN_BYTES
        || policy.len().saturating_add(path.len()) > script::POLICY_BYTES
        || core::str::from_utf8(source).is_err()
        || core::str::from_utf8(policy).is_err()
        || core::str::from_utf8(path).is_err()
    {
        return Err(Status::Invalid);
    }
    microsystem_user_rt::frame_map(
        region,
        script::SESSION_VA,
        Rights(Rights::READ.0 | Rights::WRITE.0),
    )?;
    unsafe { core::ptr::write_bytes(script::SESSION_VA as *mut u8, 0, script::SESSION_BYTES) };
    let header = unsafe { &mut *(script::SESSION_VA as *mut script::SessionHeaderV1) };
    *header = script::SessionHeaderV1 {
        magic: script::SESSION_MAGIC,
        version: script::VERSION,
        mode: script::MODE_FILE,
        source_bytes: source.len() as u32,
        stdin_head: argv.len() as u32,
        stdin_tail: argv.len() as u32,
        timeout_ns: DESKTOP_GUI_TIMEOUT_NS,
        instruction_limit: 10_000_000,
        flags: script::FLAG_GUI_SESSION,
        policy_bytes: policy.len() as u32,
        argv_bytes: argv.len() as u16,
        path_bytes: path.len() as u16,
        ..script::SessionHeaderV1::default()
    };
    unsafe {
        core::ptr::copy_nonoverlapping(
            source.as_ptr(),
            (script::SESSION_VA as *mut u8).add(script::SOURCE_OFFSET),
            source.len(),
        );
        core::ptr::copy_nonoverlapping(
            policy.as_ptr(),
            (script::SESSION_VA as *mut u8).add(script::POLICY_OFFSET),
            policy.len(),
        );
        core::ptr::copy_nonoverlapping(
            path.as_ptr(),
            (script::SESSION_VA as *mut u8).add(script::POLICY_OFFSET + policy.len()),
            path.len(),
        );
        core::ptr::copy_nonoverlapping(
            argv.as_ptr(),
            (script::SESSION_VA as *mut u8).add(script::STDIN_OFFSET),
            argv.len(),
        );
    }
    microsystem_user_rt::frame_unmap(region, script::SESSION_VA)
}

fn report_desktop_launch(application: gui::Application, status: Status) {
    let result: &[u8] = if status == Status::Ok {
        b" accepted=true\n"
    } else {
        b" accepted=false\n"
    };
    let _ = microsystem_user_rt::debug_write_u64(
        b"[gui] desktop launch application=",
        application as u64,
        result,
    );
}

fn service_endpoint(
    endpoint: CapHandle,
    pending: &mut Option<Message>,
    mut handler: impl FnMut(&Message, &mut Message) -> Status,
) -> Result<(), Status> {
    let mut request = if let Some(request) = pending.take() {
        request
    } else {
        let now = microsystem_user_rt::clock_now()?;
        let deadline = now.saturating_add(1_000_000);
        let mut request = Message::new(0, 0);
        match microsystem_user_rt::ipc_recv(endpoint, &mut request, deadline) {
            Err(Status::TimedOut) => return Ok(()),
            Err(status) => return Err(status),
            Ok(()) => request,
        }
    };
    for batch_index in 0..8 {
        let mut reply = Message::new(request.protocol, request.opcode);
        reply.words[5] = handler(&request, &mut reply) as i64 as u64;
        // At the batch boundary, reply with an already-expired receive
        // deadline. This closes the kernel reply-to state without accepting a
        // ninth request that would remain outstanding while init polls a
        // different endpoint.
        let deadline = if batch_index == 7 {
            1
        } else {
            microsystem_user_rt::clock_now()?.saturating_add(1_000_000)
        };
        match microsystem_user_rt::ipc_reply_recv(endpoint, &reply, &mut request, deadline) {
            Ok(()) => {}
            Err(Status::TimedOut) => return Ok(()),
            Err(status) => return Err(status),
        }
    }
    Ok(())
}

fn handle_time(request: &Message, reply: &mut Message) -> Status {
    const MAX_SLEEP_NS: u64 = 1_000_000_000;

    match request.opcode {
        value if value == microsystem_abi::time::Operation::Uptime as u16 => {
            match microsystem_user_rt::clock_now() {
                Ok(now) => {
                    reply.words[0] = now;
                    Status::Ok
                }
                Err(status) => status,
            }
        }
        value if value == microsystem_abi::time::Operation::Sleep as u16 => {
            let duration = request.words[0];
            if duration > MAX_SLEEP_NS {
                return Status::Invalid;
            }
            let start = match microsystem_user_rt::clock_now() {
                Ok(now) => now,
                Err(status) => return status,
            };
            let deadline = start.saturating_add(duration);
            loop {
                let now = match microsystem_user_rt::clock_now() {
                    Ok(now) => now,
                    Err(status) => return status,
                };
                if now >= deadline {
                    reply.words[0] = now;
                    return Status::Ok;
                }
                let _ = microsystem_user_rt::yield_now();
            }
        }
        _ => Status::Invalid,
    }
}

fn start_service(name: &str, expected_pid: u64) {
    match microsystem_user_rt::thread_start(program_id(name)) {
        Ok(pid) if pid == expected_pid => return,
        Ok(pid) => {
            let _ = microsystem_user_rt::debug_write(b"[service] start failed name=");
            let _ = microsystem_user_rt::debug_write(name.as_bytes());
            let _ = microsystem_user_rt::debug_write_u64(
                b" expected-pid=",
                expected_pid,
                b" actual-pid=",
            );
            let _ = microsystem_user_rt::debug_write_u64(b"", pid, b"\n");
        }
        Err(status) => {
            let _ = microsystem_user_rt::debug_write(b"[service] start failed name=");
            let _ = microsystem_user_rt::debug_write(name.as_bytes());
            let _ =
                microsystem_user_rt::debug_write_u64(b" expected-pid=", expected_pid, b" status=-");
            let _ =
                microsystem_user_rt::debug_write_u64(b"", (status as i64).unsigned_abs(), b"\n");
        }
    }
    microsystem_user_rt::exit(20);
}

fn verify_process_kill() {
    let spinner = program_id("spinner");
    let first = microsystem_user_rt::thread_start(spinner)
        .unwrap_or_else(|_| microsystem_user_rt::exit(21));
    remember_program(first, spinner);
    let second = microsystem_user_rt::thread_start(spinner)
        .unwrap_or_else(|_| microsystem_user_rt::exit(21));
    remember_program(second, spinner);
    let start = microsystem_user_rt::clock_now().unwrap_or(0);
    while microsystem_user_rt::clock_now().unwrap_or(start) < start.saturating_add(20_000_000) {
        let _ = microsystem_user_rt::yield_now();
    }
    let Ok((first_running, _, first_cpu_mask)) =
        microsystem_user_rt::thread_status_with_cpu_mask(first)
    else {
        microsystem_user_rt::exit(22);
    };
    let Ok((second_running, _, second_cpu_mask)) =
        microsystem_user_rt::thread_status_with_cpu_mask(second)
    else {
        microsystem_user_rt::exit(22);
    };
    if first == second
        || !first_running
        || !second_running
        || first_cpu_mask == 0
        || second_cpu_mask == 0
        || (first_cpu_mask | second_cpu_mask) != 0b11
        || microsystem_user_rt::thread_kill(first, -15).is_err()
        || !wait_for_status(first, -15)
        || !is_running(second)
    {
        microsystem_user_rt::exit(22);
    }
    if microsystem_user_rt::thread_kill(second, -15).is_err() || !wait_for_status(second, -15) {
        microsystem_user_rt::exit(22);
    }
    let counter = program_id("counter");
    let reused = microsystem_user_rt::thread_start(counter)
        .unwrap_or_else(|_| microsystem_user_rt::exit(22));
    remember_program(reused, counter);
    if reused != first || !wait_for_status(reused, 0) {
        microsystem_user_rt::exit(22);
    }
    let _ = microsystem_user_rt::debug_write(
        b"[proc] duplicate spinner independent=true survivor-running=true first-status=-15 second-status=-15 reclaimed=true slot-reuse=true\n",
    );
    let _ = microsystem_user_rt::debug_write_u64(
        b"[sched] application spinners dual-core=true tasks=2 cpus=2 cpu-masks-pair=",
        first_cpu_mask as u64 * 10 + second_cpu_mask as u64,
        b"\n",
    );
    let _ = microsystem_user_rt::debug_write(b"[proc] counter slot-reuse=true wait-status=0\n");
}

fn is_running(pid: u64) -> bool {
    microsystem_user_rt::thread_status(pid)
        .map(|(running, _)| running)
        .unwrap_or(false)
}

fn remember_program(pid: u64, program: u64) {
    if let Some(slot) =
        PROCESS_PROGRAMS.get(pid.saturating_sub(process::FIRST_APPLICATION_PID) as usize)
    {
        slot.store(program, Ordering::Release);
    }
}

fn wait_for_status(pid: u64, expected: i64) -> bool {
    let deadline = microsystem_user_rt::clock_now()
        .unwrap_or(0)
        .saturating_add(APPLICATION_WAIT_NS);
    loop {
        match microsystem_user_rt::thread_status(pid) {
            Ok((false, status)) => return status == expected,
            Ok((true, _)) if microsystem_user_rt::clock_now().unwrap_or(deadline) < deadline => {
                let _ = microsystem_user_rt::yield_now();
            }
            _ => return false,
        }
    }
}

fn verify_memory_pool_mapping() {
    const FIRST_VA: u64 = 0x0058_0000;
    const SECOND_VA: u64 = 0x0058_1000;
    const VALUE: u64 = 0x4d49_4352_4f46_524d;
    let rights = Rights(Rights::READ.0 | Rights::WRITE.0 | Rights::MAP.0 | Rights::GRANT.0);
    let mut frames = [CapHandle::INVALID; 4];
    for frame in &mut frames {
        *frame = microsystem_user_rt::frame_create(boot_cap::ROOT_MEMORY_POOL, rights)
            .unwrap_or_else(|_| microsystem_user_rt::exit(26));
    }
    if microsystem_user_rt::frame_create(boot_cap::ROOT_MEMORY_POOL, rights)
        != Err(Status::NoMemory)
        || microsystem_user_rt::frame_map(
            frames[0],
            FIRST_VA,
            Rights(Rights::READ.0 | Rights::WRITE.0),
        )
        .is_err()
    {
        microsystem_user_rt::exit(27);
    }
    unsafe {
        (FIRST_VA as *mut u64).write_volatile(VALUE);
    }
    if microsystem_user_rt::cap_delete(frames[0]) != Err(Status::Busy)
        || microsystem_user_rt::frame_unmap(frames[0], FIRST_VA).is_err()
        || microsystem_user_rt::frame_map(frames[0], SECOND_VA, Rights::READ).is_err()
        || unsafe { (SECOND_VA as *const u64).read_volatile() } != VALUE
        || microsystem_user_rt::frame_unmap(frames[0], SECOND_VA).is_err()
    {
        microsystem_user_rt::exit(28);
    }
    for frame in frames {
        if microsystem_user_rt::cap_delete(frame).is_err() {
            microsystem_user_rt::exit(29);
        }
    }
    let recycled = microsystem_user_rt::frame_create(boot_cap::ROOT_MEMORY_POOL, rights)
        .unwrap_or_else(|_| microsystem_user_rt::exit(29));
    if microsystem_user_rt::cap_delete(recycled).is_err() {
        microsystem_user_rt::exit(29);
    }
    let dynamic_pool = microsystem_user_rt::memory_pool_create(2, Rights::MANAGE)
        .unwrap_or_else(|_| microsystem_user_rt::exit(30));
    let dynamic_frames = [
        microsystem_user_rt::frame_create(dynamic_pool, rights)
            .unwrap_or_else(|_| microsystem_user_rt::exit(30)),
        microsystem_user_rt::frame_create(dynamic_pool, rights)
            .unwrap_or_else(|_| microsystem_user_rt::exit(30)),
    ];
    if microsystem_user_rt::frame_create(dynamic_pool, rights) != Err(Status::NoMemory)
        || microsystem_user_rt::cap_delete(dynamic_pool) != Err(Status::Busy)
    {
        microsystem_user_rt::exit(30);
    }
    for frame in dynamic_frames {
        if microsystem_user_rt::cap_delete(frame).is_err() {
            microsystem_user_rt::exit(30);
        }
    }
    if microsystem_user_rt::cap_delete(dynamic_pool).is_err() {
        microsystem_user_rt::exit(30);
    }
    let pool_rights = Rights(Rights::MANAGE.0 | Rights::GRANT.0);
    let replacement_pool = microsystem_user_rt::memory_pool_create(2, pool_rights)
        .unwrap_or_else(|_| microsystem_user_rt::exit(30));
    let mut request = Message::new(protocol::CONSOLE, 0);
    request.words[0] = 0x30;
    request.caps[0] = replacement_pool;
    request.flags |= message_cap_move(0);
    let mut reply = Message::new(protocol::CONSOLE, 0);
    if microsystem_user_rt::ipc_call(boot_cap::CONSOLE_ENDPOINT, &request, &mut reply, 0).is_err()
        || reply.words[0] != 0x31
        || reply.caps[0] == CapHandle::INVALID
        || reply.caps[1] == CapHandle::INVALID
        || microsystem_user_rt::frame_map(reply.caps[1], FIRST_VA, Rights::READ).is_err()
    {
        microsystem_user_rt::exit(30);
    }
    let mut mapped_move = Message::new(protocol::CONSOLE, 0);
    mapped_move.caps[0] = reply.caps[1];
    mapped_move.flags |= message_cap_move(0);
    let mut mapped_move_reply = Message::new(protocol::CONSOLE, 0);
    let mapped_move_deadline = microsystem_user_rt::clock_now()
        .unwrap_or_else(|_| microsystem_user_rt::exit(30))
        .saturating_add(50_000_000);
    if microsystem_user_rt::ipc_call(
        boot_cap::CONSOLE_ENDPOINT,
        &mapped_move,
        &mut mapped_move_reply,
        mapped_move_deadline,
    ) != Err(Status::TimedOut)
        || microsystem_user_rt::frame_unmap(reply.caps[1], FIRST_VA).is_err()
        || microsystem_user_rt::cap_delete(reply.caps[0]) != Err(Status::Busy)
        || microsystem_user_rt::cap_delete(reply.caps[1]).is_err()
        || microsystem_user_rt::cap_delete(reply.caps[0]).is_err()
    {
        microsystem_user_rt::exit(30);
    }
    let _ = microsystem_user_rt::debug_write(
        b"[mmu] resident memory-pool quota=4 frame-map/unmap remap=true mapped-delete=busy mapped-move=blocked reclaimed=true\n",
    );
    let _ = microsystem_user_rt::debug_write(
        b"[mm] MemoryPool dynamic quota=2 third=NoMemory live-delete=Busy reclaimed=true registry-reused=true cross-task-move=true\n",
    );
}

fn verify_privileged_fault() {
    let privprobe = program_id("privprobe");
    let pid = microsystem_user_rt::thread_start(privprobe)
        .unwrap_or_else(|_| microsystem_user_rt::exit(23));
    remember_program(pid, privprobe);
    let deadline = microsystem_user_rt::clock_now()
        .unwrap_or_else(|_| microsystem_user_rt::exit(24))
        .saturating_add(100_000_000);
    loop {
        match microsystem_user_rt::thread_status(pid) {
            Ok((false, status)) if status == Status::Fault as i64 => break,
            Ok((true, _)) => {
                if microsystem_user_rt::clock_now().unwrap_or(deadline) >= deadline {
                    microsystem_user_rt::exit(24);
                }
                let _ = microsystem_user_rt::yield_now();
            }
            _ => microsystem_user_rt::exit(25),
        }
    }
    let _ = microsystem_user_rt::debug_write_u64(
        b"[isolation] privileged instruction task pid=",
        pid,
        b" faulted status=-8 reclaimed=true\n",
    );
    let _ = microsystem_user_rt::debug_write_u64(
        b"[proc] dynamic application capacity=16 first-pid=",
        process::FIRST_APPLICATION_PID,
        b" independent-slots=true\n",
    );
}

fn verify_application_capacity() {
    let mut pids = [0u64; process::MAX_APPLICATIONS];
    for pid in &mut pids {
        *pid = microsystem_user_rt::thread_start(program_id("spinner"))
            .unwrap_or_else(|_| microsystem_user_rt::exit(38));
        remember_program(*pid, program_id("spinner"));
    }
    if microsystem_user_rt::thread_start(program_id("spinner")) != Err(Status::Busy) {
        microsystem_user_rt::exit(39);
    }
    for pid in pids {
        let _ = microsystem_user_rt::thread_kill(pid, -15);
        if !wait_for_status(pid, -15) {
            microsystem_user_rt::exit(40);
        }
    }
    let _ = microsystem_user_rt::debug_write(
        b"[proc] application capacity live=16 overflow=Busy all-reclaimed=true\n",
    );
}

fn verify_isolation_matrix() {
    let badptr = program_id("badptr");
    let badptr_pid =
        microsystem_user_rt::thread_start(badptr).unwrap_or_else(|_| microsystem_user_rt::exit(30));
    remember_program(badptr_pid, badptr);
    if !wait_for_status(badptr_pid, 0) {
        microsystem_user_rt::exit(31);
    }
    let _ = microsystem_user_rt::debug_write_u64(
        b"[proc] invalid-pointer probe pid=",
        badptr_pid,
        b" wait-status=0 task-survived=true\n",
    );

    let crossptr = program_id("crossptr");
    let crossptr_pid = microsystem_user_rt::thread_start(crossptr)
        .unwrap_or_else(|_| microsystem_user_rt::exit(34));
    remember_program(crossptr_pid, crossptr);
    if !wait_for_status(crossptr_pid, 0) {
        microsystem_user_rt::exit(35);
    }
    let _ = microsystem_user_rt::debug_write_u64(
        b"[proc] cross-page pointer probe pid=",
        crossptr_pid,
        b" wait-status=0 task-survived=true\n",
    );

    let pageprobe = program_id("pageprobe");
    let pageprobe_pid = microsystem_user_rt::thread_start(pageprobe)
        .unwrap_or_else(|_| microsystem_user_rt::exit(32));
    let readonly_pid = microsystem_user_rt::thread_start(pageprobe)
        .unwrap_or_else(|_| microsystem_user_rt::exit(32));
    remember_program(pageprobe_pid, pageprobe);
    remember_program(readonly_pid, pageprobe);
    if !wait_for_status(pageprobe_pid, Status::Fault as i64) {
        microsystem_user_rt::exit(33);
    }
    let (_, _, _, frames, _, mappings) =
        microsystem_user_rt::thread_status_with_resources(pageprobe_pid)
            .unwrap_or_else(|_| microsystem_user_rt::exit(36));
    if frames < 2 || mappings < 2 {
        microsystem_user_rt::exit(37);
    }
    let _ = microsystem_user_rt::debug_write(
        b"[mm] anonymous fault cleanup resident-pages-reclaimed=true\n",
    );
    if !wait_for_status(readonly_pid, Status::Fault as i64) {
        microsystem_user_rt::exit(41);
    }
    let _ = microsystem_user_rt::debug_write(
        b"[mm] anonymous read-only mapping rejected writer reclaimed=true\n",
    );
    let _ = microsystem_user_rt::debug_write_u64(
        b"[proc] page-fault probe pid=",
        pageprobe_pid,
        b" wait-status=-8 reclaimed=true\n",
    );
}

fn verify_endpoint_capability() {
    let request = Message::new(protocol::BLOCK, BlockOperation::Geometry as u16);
    let mut reply = Message::new(protocol::BLOCK, 0);
    if microsystem_user_rt::ipc_call(boot_cap::BLOCK_ENDPOINT, &request, &mut reply, 0)
        != Err(Status::BadCapability)
    {
        microsystem_user_rt::exit(20);
    }
    let _ =
        microsystem_user_rt::debug_write(b"[ipc] resident endpoint capabilities enforced=true\n");
}

fn verify_notification_deadline() {
    let notification = microsystem_user_rt::object_create(
        ObjectType::Notification,
        Rights(Rights::READ.0 | Rights::MANAGE.0),
    )
    .unwrap_or_else(|_| microsystem_user_rt::exit(17));
    let deadline = microsystem_user_rt::clock_now()
        .unwrap_or_else(|_| microsystem_user_rt::exit(18))
        .checked_add(50_000_000)
        .unwrap_or_else(|| microsystem_user_rt::exit(18));
    if microsystem_user_rt::notification_wait(notification, deadline) != Err(Status::TimedOut)
        || microsystem_user_rt::cap_delete(notification).is_err()
    {
        microsystem_user_rt::exit(19);
    }
    let _ = microsystem_user_rt::debug_write(
        b"[irq] resident generic notification deadline-block=true\n",
    );
}

fn verify_notification_signal() {
    let rights = Rights(Rights::READ.0 | Rights::WRITE.0 | Rights::GRANT.0 | Rights::MANAGE.0);
    let notification = microsystem_user_rt::object_create(ObjectType::Notification, rights)
        .unwrap_or_else(|_| microsystem_user_rt::exit(36));
    let signaler =
        microsystem_user_rt::cap_copy(notification, Rights(Rights::WRITE.0 | Rights::GRANT.0))
            .unwrap_or_else(|_| microsystem_user_rt::exit(36));
    let read_only = microsystem_user_rt::cap_copy(notification, Rights::READ)
        .unwrap_or_else(|_| microsystem_user_rt::exit(36));
    if microsystem_user_rt::notification_signal(read_only, 1) != Err(Status::AccessDenied) {
        microsystem_user_rt::exit(36);
    }

    let mut request = Message::new(protocol::CONSOLE, 0);
    request.words[0] = 0x40;
    request.caps[0] = signaler;
    request.flags |= message_cap_move(0);
    let mut reply = Message::new(protocol::CONSOLE, 0);
    if microsystem_user_rt::ipc_call(boot_cap::CONSOLE_ENDPOINT, &request, &mut reply, 0).is_err()
        || reply.protocol != protocol::CONSOLE
        || reply.words[0] != 0x41
        || reply.words[5] as i64 != Status::Ok as i64
        || reply.caps[0] == CapHandle::INVALID
    {
        microsystem_user_rt::exit(36);
    }
    let deadline = microsystem_user_rt::clock_now()
        .unwrap_or_else(|_| microsystem_user_rt::exit(36))
        .saturating_add(50_000_000);
    if microsystem_user_rt::notification_wait(notification, deadline).ok() != Some(0x5)
        || microsystem_user_rt::cap_delete(reply.caps[0]).is_err()
        || microsystem_user_rt::cap_delete(read_only).is_err()
        || microsystem_user_rt::cap_delete(notification).is_err()
    {
        microsystem_user_rt::exit(36);
    }
    let _ = microsystem_user_rt::debug_write(
        b"[irq] resident generic notification signal=true bitset=0x5 cross-task=true\n",
    );
}

fn verify_notification_blocking_wake() {
    let rights = Rights(Rights::READ.0 | Rights::WRITE.0 | Rights::GRANT.0 | Rights::MANAGE.0);
    let notification = microsystem_user_rt::object_create(ObjectType::Notification, rights)
        .unwrap_or_else(|_| microsystem_user_rt::exit(37));
    let signaler =
        microsystem_user_rt::cap_copy(notification, Rights(Rights::WRITE.0 | Rights::GRANT.0))
            .unwrap_or_else(|_| microsystem_user_rt::exit(37));
    let mut request = Message::new(protocol::CONSOLE, 0);
    request.words[0] = 0x42;
    request.caps[0] = signaler;
    request.flags |= message_cap_move(0);
    let mut reply = Message::new(protocol::CONSOLE, 0);
    if microsystem_user_rt::ipc_call(boot_cap::CONSOLE_ENDPOINT, &request, &mut reply, 0).is_err()
        || reply.words[0] != 0x43
        || reply.words[5] as i64 != Status::Ok as i64
        || reply.caps[0] != CapHandle::INVALID
    {
        microsystem_user_rt::exit(37);
    }
    let start = microsystem_user_rt::clock_now().unwrap_or_else(|_| microsystem_user_rt::exit(37));
    let deadline = start.saturating_add(500_000_000);
    if microsystem_user_rt::notification_wait(notification, deadline).ok() != Some(0x8)
        || microsystem_user_rt::clock_now()
            .unwrap_or_else(|_| microsystem_user_rt::exit(37))
            .saturating_sub(start)
            < 10_000_000
        || microsystem_user_rt::cap_delete(notification).is_err()
    {
        microsystem_user_rt::exit(37);
    }
    let _ = microsystem_user_rt::debug_write(
        b"[irq] resident generic notification wake-after-block=true cross-task=true\n",
    );
}

fn verify_remote_mapping_revoke() {
    let root = microsystem_user_rt::frame_create(
        boot_cap::ROOT_MEMORY_POOL,
        Rights(
            Rights::READ.0 | Rights::WRITE.0 | Rights::MAP.0 | Rights::GRANT.0 | Rights::MANAGE.0,
        ),
    )
    .unwrap_or_else(|_| microsystem_user_rt::exit(38));
    let child = microsystem_user_rt::cap_copy(
        root,
        Rights(Rights::READ.0 | Rights::WRITE.0 | Rights::MAP.0 | Rights::GRANT.0),
    )
    .unwrap_or_else(|_| microsystem_user_rt::exit(38));
    let mut map_request = Message::new(protocol::CONSOLE, 0);
    map_request.words[0] = 0x50;
    map_request.caps[0] = child;
    map_request.flags |= message_cap_move(0);
    let mut map_reply = Message::new(protocol::CONSOLE, 0);
    if microsystem_user_rt::ipc_call(boot_cap::CONSOLE_ENDPOINT, &map_request, &mut map_reply, 0)
        .is_err()
        || map_reply.words[0] != 0x51
        || map_reply.words[5] as i64 != Status::Ok as i64
        || microsystem_user_rt::cap_revoke(root).ok() != Some(1)
    {
        microsystem_user_rt::exit(38);
    }

    let mut verify_request = Message::new(protocol::CONSOLE, 0);
    verify_request.words[0] = 0x52;
    let mut verify_reply = Message::new(protocol::CONSOLE, 0);
    if microsystem_user_rt::ipc_call(
        boot_cap::CONSOLE_ENDPOINT,
        &verify_request,
        &mut verify_reply,
        0,
    )
    .is_err()
        || verify_reply.words[0] != 0x53
        || verify_reply.words[5] as i64 != Status::Ok as i64
        || microsystem_user_rt::cap_delete(root).is_err()
    {
        microsystem_user_rt::exit(38);
    }
    let _ = microsystem_user_rt::debug_write(
        b"[cap] resident revoke cleared remote frame mapping=true tlbi=true task-survived=true\n",
    );
}

fn delegate_block_transport() -> Result<bool, Status> {
    let mut request = Message::new(protocol::BLOCK, BlockOperation::Configure as u16);
    request.caps = [
        boot_cap::DEVMGR_VIRTIO_MMIO,
        boot_cap::DEVMGR_QUEUE_FRAME,
        boot_cap::DEVMGR_DMA_DOMAIN,
        boot_cap::DEVMGR_VIRTIO_IRQ,
    ];
    let mut reply = Message::new(protocol::BLOCK, 0);
    if microsystem_user_rt::ipc_call(
        boot_cap::DEVMGR_ENDPOINT,
        &request,
        &mut reply,
        microsystem_user_rt::clock_now()?.saturating_add(10_000_000_000),
    )
    .is_err()
        || reply.protocol != protocol::BLOCK
        || reply.words[5] as i64 != 0
    {
        return Err(Status::Io);
    }
    let _ = microsystem_user_rt::debug_write(
        b"[devmgr] resident root delegated mmio/frame/dma/irq to devmgr via IPC\n",
    );
    Ok(reply.words[0] != 0)
}

fn terminate_script_sessions() {
    for (index, region) in SCRIPT_REGIONS.iter().enumerate() {
        if region.load(Ordering::Acquire) != 0 {
            let _ = microsystem_user_rt::thread_kill(
                process::FIRST_APPLICATION_PID + index as u64,
                Status::Io as i64,
            );
        }
    }
}

fn handle_script(request: &Message, reply: &mut Message) -> Status {
    if request.protocol != protocol::SCRIPT {
        return Status::Invalid;
    }
    let token = request.words[0];
    let Some(owner_slot) = SCRIPT_TOKENS
        .iter()
        .position(|candidate| candidate.load(Ordering::Acquire) == token && token != 0)
    else {
        return Status::AccessDenied;
    };
    let owner_pid = process::FIRST_APPLICATION_PID + owner_slot as u64;
    let region = CapHandle(SCRIPT_REGIONS[owner_slot].load(Ordering::Acquire));
    if region == CapHandle::INVALID
        || microsystem_user_rt::frame_map(
            region,
            script::SESSION_VA,
            Rights(Rights::READ.0 | Rights::WRITE.0),
        )
        .is_err()
    {
        return Status::BadCapability;
    }
    let status = (|| {
        let (source, policy) = mapped_script_policy()?;
        match request.opcode {
            value if value == script::Operation::ProcessList as u16 => {
                if !script_unscoped_allowed(source, policy, "proc.list") {
                    return Err(Status::AccessDenied);
                }
                let first = request.words[1].max(process::FIRST_APPLICATION_PID);
                let last = process::FIRST_APPLICATION_PID + process::MAX_APPLICATIONS as u64 - 1;
                for pid in first..=last {
                    let mut info = microsystem_abi::ProcessInfoV2::default();
                    match microsystem_user_rt::thread_status_v2(pid, &mut info) {
                        Ok(()) => {
                            unsafe {
                                core::ptr::write(
                                    (script::SESSION_VA as *mut u8)
                                        .add(script::BROKER_OFFSET)
                                        .cast::<microsystem_abi::ProcessInfoV2>(),
                                    info,
                                );
                                microsystem_user_rt::fence();
                            }
                            reply.words[0] =
                                core::mem::size_of::<microsystem_abi::ProcessInfoV2>() as u64;
                            reply.words[1] = (pid < last).then_some(pid + 1).unwrap_or(0);
                            return Ok(());
                        }
                        Err(Status::NotFound) => {}
                        Err(status) => return Err(status),
                    }
                }
                reply.words[0] = 0;
                reply.words[1] = 0;
                Ok(())
            }
            value if value == script::Operation::ProcessSpawn as u16 => {
                let program = request.words[1];
                let Some(name) = application_program_name(program) else {
                    return Err(Status::NotFound);
                };
                if !script_program_allowed(source, policy, "proc.spawn", name) {
                    return Err(Status::AccessDenied);
                }
                let pid = microsystem_user_rt::thread_start(program)?;
                remember_program(pid, program);
                let owner = identity::snapshot()?.actor(
                    owner_pid as u32,
                    0,
                    microsystem_user_rt::clock_now()?,
                )?;
                identity::process_owner(pid, owner);
                let slot = (pid - process::FIRST_APPLICATION_PID) as usize;
                SCRIPT_CHILD_OWNER[slot].store(owner_pid, Ordering::Release);
                reply.words[0] = pid;
                Ok(())
            }
            value if value == script::Operation::ProcessWait as u16 => {
                let pid = request.words[1];
                let slot = application_slot(pid)?;
                if SCRIPT_CHILD_OWNER[slot].load(Ordering::Acquire) != owner_pid {
                    return Err(Status::AccessDenied);
                }
                match microsystem_user_rt::thread_status(pid) {
                    Ok((true, _)) => Err(Status::Busy),
                    Ok((false, exit)) => {
                        SCRIPT_CHILD_OWNER[slot].store(0, Ordering::Release);
                        reply.words[0] = exit as u64;
                        Ok(())
                    }
                    Err(status) => Err(status),
                }
            }
            value if value == script::Operation::ProcessKill as u16 => {
                let pid = request.words[1];
                let slot = application_slot(pid)?;
                if SCRIPT_CHILD_OWNER[slot].load(Ordering::Acquire) != owner_pid {
                    return Err(Status::AccessDenied);
                }
                let program = PROCESS_PROGRAMS[slot].load(Ordering::Acquire);
                let Some(name) = application_program_name(program) else {
                    return Err(Status::NotFound);
                };
                if !script_program_allowed(source, policy, "proc.kill", name) {
                    return Err(Status::AccessDenied);
                }
                microsystem_user_rt::thread_kill(pid, -15)
            }
            _ => Err(Status::Invalid),
        }
    })();
    let _ = microsystem_user_rt::frame_unmap(region, script::SESSION_VA);
    status.map(|()| Status::Ok).unwrap_or_else(|status| status)
}

fn mapped_script_policy() -> Result<(&'static str, &'static str), Status> {
    let header = unsafe { &*(script::SESSION_VA as *const script::SessionHeaderV1) };
    if header.magic != script::SESSION_MAGIC
        || header.version != script::VERSION
        || !matches!(
            header.mode,
            script::MODE_EVAL | script::MODE_FILE | script::MODE_REPL
        )
        || (header.mode != script::MODE_REPL && header.source_bytes == 0)
        || header.source_bytes as usize > script::SOURCE_BYTES
        || header.policy_bytes as usize > script::POLICY_BYTES
        || header.argv_bytes as usize > script::STDIN_BYTES
        || header.path_bytes as usize
            > script::POLICY_BYTES.saturating_sub(header.policy_bytes as usize)
        || (header.mode == script::MODE_FILE && header.path_bytes == 0)
    {
        return Err(Status::Invalid);
    }
    let source = unsafe {
        core::slice::from_raw_parts(
            (script::SESSION_VA as *const u8).add(script::SOURCE_OFFSET),
            header.source_bytes as usize,
        )
    };
    let policy = unsafe {
        core::slice::from_raw_parts(
            (script::SESSION_VA as *const u8).add(script::POLICY_OFFSET),
            header.policy_bytes as usize,
        )
    };
    let source = core::str::from_utf8(source).map_err(|_| Status::Invalid)?;
    Ok((
        if header.mode == script::MODE_FILE {
            source
        } else {
            ""
        },
        core::str::from_utf8(policy).map_err(|_| Status::Invalid)?,
    ))
}

fn script_unscoped_allowed(source: &str, policy: &str, rule: &str) -> bool {
    let manifest = allocless_manifest_rule(rule);
    (source.is_empty() || manifest_has_permission(source, manifest.as_str()))
        && has_permission(policy, rule)
}

fn script_program_allowed(source: &str, policy: &str, operation: &str, program: &str) -> bool {
    let mut manifest = TextRule::new();
    manifest.push("--!allow ");
    manifest.push(operation);
    manifest.push(":");
    manifest.push(program);
    let mut launcher = TextRule::new();
    launcher.push(operation);
    launcher.push(":");
    launcher.push(program);
    (source.is_empty() || manifest_has_permission(source, manifest.as_str()))
        && has_permission(policy, launcher.as_str())
}

fn allocless_manifest_rule(rule: &str) -> TextRule {
    let mut output = TextRule::new();
    output.push("--!allow ");
    output.push(rule);
    output
}

struct TextRule {
    bytes: [u8; 96],
    length: usize,
}

impl TextRule {
    const fn new() -> Self {
        Self {
            bytes: [0; 96],
            length: 0,
        }
    }
    fn push(&mut self, text: &str) {
        let available = self.bytes.len().saturating_sub(self.length);
        let count = text.len().min(available);
        self.bytes[self.length..self.length + count].copy_from_slice(&text.as_bytes()[..count]);
        self.length += count;
    }
    fn as_str(&self) -> &str {
        core::str::from_utf8(&self.bytes[..self.length]).unwrap_or("")
    }
}

fn application_slot(pid: u64) -> Result<usize, Status> {
    let slot = pid
        .checked_sub(process::FIRST_APPLICATION_PID)
        .ok_or(Status::NotFound)? as usize;
    (slot < process::MAX_APPLICATIONS)
        .then_some(slot)
        .ok_or(Status::NotFound)
}

fn application_program_name(program: u64) -> Option<&'static str> {
    [
        "counter",
        "spinner",
        "badptr",
        "crossptr",
        "pageprobe",
        "privprobe",
        "resourceprobe",
        "resourcefault",
        "resourcekill",
    ]
    .into_iter()
    .find(|name| program_id(name) == program)
}

fn handle_process(request: &Message, reply: &mut Message) -> Status {
    if request.protocol != protocol::PROCESS {
        return Status::Invalid;
    }
    let actor = match identity::actor(request.words[3]) {
        Ok(actor) => actor,
        Err(status) => {
            if request.opcode == process::Operation::SpawnScript as u16
                && request.caps[0] != CapHandle::INVALID
            {
                let _ = microsystem_user_rt::cap_delete(request.caps[0]);
            }
            return status;
        }
    };
    if actor.role == microsystem_identity::READER
        && matches!(request.opcode, value if value == process::Operation::Spawn as u16 || value == process::Operation::SpawnScript as u16 || value == process::Operation::MemoryPool as u16)
    {
        if request.caps[0] != CapHandle::INVALID {
            let _ = microsystem_user_rt::cap_delete(request.caps[0]);
        }
        return Status::AccessDenied;
    }
    if request.opcode == process::Operation::Kill as u16 && !actor.administrator() {
        let Ok(state) = identity::snapshot() else {
            return Status::Busy;
        };
        if request.words[0]
            .checked_sub(14)
            .and_then(|i| state.process_uids.get(i as usize))
            .copied()
            != Some(actor.uid)
        {
            return Status::AccessDenied;
        }
    }
    match request.opcode {
        value if value == process::Operation::Spawn as u16 && request.words[0] != 0 => {
            if application_program_name(request.words[0]).is_none() {
                return Status::AccessDenied;
            }
            match microsystem_user_rt::thread_start(request.words[0]) {
                Ok(pid) => {
                    identity::process_owner(pid, actor);
                    if let Some(program) = PROCESS_PROGRAMS
                        .get(pid.saturating_sub(process::FIRST_APPLICATION_PID) as usize)
                    {
                        program.store(request.words[0], Ordering::Release);
                    }
                    reply.words[0] = pid;
                    Status::Ok
                }
                Err(status) => status,
            }
        }
        value if value == process::Operation::Wait as u16 => {
            match microsystem_user_rt::thread_status_with_resources(request.words[0]) {
                Ok((true, _, _, frames, pools, mappings)) => {
                    reply.words[1] = frames as u64;
                    reply.words[2] = pools as u64;
                    reply.words[3] = mappings as u64;
                    Status::Busy
                }
                Ok((false, status, _, frames, pools, mappings)) => {
                    reply.words[0] = status as u64;
                    reply.words[1] = frames as u64;
                    reply.words[2] = pools as u64;
                    reply.words[3] = mappings as u64;
                    cleanup_script_session(request.words[0]);
                    Status::Ok
                }
                Err(status) => status,
            }
        }
        value if value == process::Operation::List as u16 => {
            let first = request.words[0].max(process::FIRST_APPLICATION_PID);
            let last = process::FIRST_APPLICATION_PID + process::MAX_APPLICATIONS as u64 - 1;
            for pid in first..=last {
                let mut info = microsystem_abi::ProcessInfoV2::default();
                match microsystem_user_rt::thread_status_v2(pid, &mut info) {
                    Ok(()) => {
                        reply.words[0] = info.pid as u64;
                        reply.words[1] = u64::from(info.running != 0);
                        reply.words[2] = info.exit_status as u64;
                        reply.words[3] = (pid < last).then_some(pid + 1).unwrap_or(0);
                        reply.words[4] = info.program;
                        return Status::Ok;
                    }
                    Err(Status::NotFound) => {}
                    Err(status) => return status,
                }
            }
            reply.words[0] = 0;
            Status::Ok
        }
        value if value == process::Operation::Kill as u16 => {
            match microsystem_user_rt::thread_kill(request.words[0], request.words[1] as i64) {
                Ok(()) => Status::Ok,
                Err(status) => status,
            }
        }
        value if value == process::Operation::MemoryPool as u16 => {
            let Ok(quota) = u32::try_from(request.words[0]) else {
                return Status::Invalid;
            };
            let rights = Rights(Rights::MANAGE.0 | Rights::GRANT.0);
            match microsystem_user_rt::memory_pool_create(quota, rights) {
                Ok(pool) => {
                    reply.caps[0] = pool;
                    reply.flags |= message_cap_move(0);
                    Status::Ok
                }
                Err(status) => status,
            }
        }
        value if value == process::Operation::SpawnScript as u16 => {
            spawn_script_as(request, reply, actor)
        }
        _ => Status::Invalid,
    }
}

fn spawn_script_as(
    request: &Message,
    reply: &mut Message,
    actor: microsystem_identity::Actor,
) -> Status {
    let source_bytes = request.words[0] as usize;
    let flags = request.words[1] as u32;
    if source_bytes > script::SOURCE_BYTES || flags & !script::FLAG_GUI_SESSION != 0 {
        return Status::Invalid;
    }
    if source_bytes == 0 {
        return prepare_script_session(reply);
    }
    let region = request.caps[0];
    if region == CapHandle::INVALID {
        return Status::BadCapability;
    }
    launch_script_session(
        region,
        flags & script::FLAG_GUI_SESSION != 0,
        None,
        reply,
        actor,
    )
}

fn prepare_script_session(reply: &mut Message) -> Status {
    let rights = Rights(
        Rights::READ.0 | Rights::WRITE.0 | Rights::MAP.0 | Rights::GRANT.0 | Rights::MANAGE.0,
    );
    let region = match microsystem_user_rt::frame_region_create(
        boot_cap::SCRIPT_MEMORY_POOL,
        script::SESSION_PAGES,
        rights,
    ) {
        Ok(region) => region,
        Err(status) => return status,
    };
    reply.caps[0] = region;
    reply.flags |= message_cap_move(0);
    Status::Ok
}

fn launch_script_session(
    region: CapHandle,
    gui_requested: bool,
    desktop_application: Option<gui::Application>,
    reply: &mut Message,
    actor: microsystem_identity::Actor,
) -> Status {
    let desktop_application = desktop_application.or_else(|| identify_desktop_application(region));
    let (_allow_random, allow_stats, allow_gui, token) = match inspect_script_permissions(region) {
        Ok(permissions) => permissions,
        Err(status) => {
            let _ = microsystem_user_rt::debug_write_u64(
                b"[mica] launch rejected stage=permissions status=",
                (status as i64).unsigned_abs(),
                b"\n",
            );
            let _ = microsystem_user_rt::cap_delete(region);
            return status;
        }
    };
    if gui_requested && (!allow_gui || GUI_AVAILABLE.load(Ordering::Acquire) == 0) {
        let _ = microsystem_user_rt::cap_delete(region);
        return if allow_gui {
            Status::NotSupported
        } else {
            Status::AccessDenied
        };
    }
    if let Err(status) = register_script_filesystem(region, token, actor.uid) {
        let _ = microsystem_user_rt::debug_write_u64(
            b"[mica] launch rejected stage=filesystem status=",
            (status as i64).unsigned_abs(),
            b"\n",
        );
        let _ = microsystem_user_rt::cap_delete(region);
        return status;
    }
    if let Err(status) = register_script_network(region, token) {
        let _ = microsystem_user_rt::debug_write_u64(
            b"[mica] launch rejected stage=network status=",
            (status as i64).unsigned_abs(),
            b"\n",
        );
        unregister_script_filesystem(token);
        let _ = microsystem_user_rt::cap_delete(region);
        return status;
    }
    let notification_rights =
        Rights(Rights::READ.0 | Rights::WRITE.0 | Rights::GRANT.0 | Rights::MANAGE.0);
    let notification =
        match microsystem_user_rt::object_create(ObjectType::Notification, notification_rights) {
            Ok(notification) => notification,
            Err(status) => {
                unregister_script_network(token);
                unregister_script_filesystem(token);
                let _ = microsystem_user_rt::cap_delete(region);
                return status;
            }
        };
    let wait_notification = match microsystem_user_rt::cap_copy(
        notification,
        Rights(Rights::READ.0 | Rights::WRITE.0 | Rights::GRANT.0),
    ) {
        Ok(notification) => notification,
        Err(status) => {
            unregister_script_network(token);
            unregister_script_filesystem(token);
            let _ = microsystem_user_rt::cap_delete(notification);
            let _ = microsystem_user_rt::cap_delete(region);
            return status;
        }
    };
    let mut gui_resources = None;
    if gui_requested {
        let rights = Rights(
            Rights::READ.0 | Rights::WRITE.0 | Rights::MAP.0 | Rights::GRANT.0 | Rights::MANAGE.0,
        );
        let commands = match microsystem_user_rt::frame_region_create(
            boot_cap::SCRIPT_MEMORY_POOL,
            16,
            rights,
        ) {
            Ok(commands) => commands,
            Err(status) => {
                unregister_script_network(token);
                unregister_script_filesystem(token);
                let _ = microsystem_user_rt::cap_delete(wait_notification);
                let _ = microsystem_user_rt::cap_delete(notification);
                let _ = microsystem_user_rt::cap_delete(region);
                return status;
            }
        };
        let events = match microsystem_user_rt::frame_create(boot_cap::SCRIPT_MEMORY_POOL, rights) {
            Ok(events) => events,
            Err(status) => {
                let _ = microsystem_user_rt::cap_delete(commands);
                unregister_script_network(token);
                unregister_script_filesystem(token);
                let _ = microsystem_user_rt::cap_delete(wait_notification);
                let _ = microsystem_user_rt::cap_delete(notification);
                let _ = microsystem_user_rt::cap_delete(region);
                return status;
            }
        };
        let endpoint_slot = match reserve_gui_endpoint(token) {
            Some(slot) => slot,
            None => {
                let _ = microsystem_user_rt::cap_delete(events);
                let _ = microsystem_user_rt::cap_delete(commands);
                unregister_script_network(token);
                unregister_script_filesystem(token);
                let _ = microsystem_user_rt::cap_delete(wait_notification);
                let _ = microsystem_user_rt::cap_delete(notification);
                let _ = microsystem_user_rt::cap_delete(region);
                return Status::Busy;
            }
        };
        if let Err(status) = initialize_gui_event_ring(events) {
            release_gui_endpoint(endpoint_slot, token);
            let _ = microsystem_user_rt::cap_delete(events);
            let _ = microsystem_user_rt::cap_delete(commands);
            unregister_script_network(token);
            unregister_script_filesystem(token);
            let _ = microsystem_user_rt::cap_delete(wait_notification);
            let _ = microsystem_user_rt::cap_delete(notification);
            let _ = microsystem_user_rt::cap_delete(region);
            return status;
        }
        gui_resources = Some((commands, events, endpoint_slot));
    }
    let mut launch = ThreadLaunchV2 {
        version: 2,
        profile: microsystem_abi::THREAD_PROFILE_MICA,
        flags: if gui_requested {
            microsystem_abi::THREAD_LAUNCH_FLAG_GUI
        } else {
            0
        },
        program: program_id("mica"),
        ..ThreadLaunchV2::default()
    };
    launch.caps[..6].copy_from_slice(&[
        region,
        notification,
        boot_cap::FILESYSTEM_ENDPOINT,
        boot_cap::NETWORK_ENDPOINT,
        boot_cap::RANDOM_SOURCE,
        if allow_stats {
            boot_cap::SYSTEM_INFO
        } else {
            CapHandle::INVALID
        },
    ]);
    launch.rights[..6].copy_from_slice(&[
        Rights(Rights::READ.0 | Rights::WRITE.0 | Rights::MAP.0 | Rights::GRANT.0),
        Rights(Rights::READ.0 | Rights::WRITE.0),
        Rights::WRITE,
        Rights::WRITE,
        Rights::READ,
        if allow_stats {
            Rights::READ
        } else {
            Rights::NONE
        },
    ]);
    if let Some((commands, events, endpoint_slot)) = gui_resources {
        launch.caps[6] = commands;
        launch.caps[7] = events;
        launch.caps[8] = boot_cap::gui_dynamic_endpoint(endpoint_slot);
        launch.rights[6] = Rights(Rights::READ.0 | Rights::WRITE.0 | Rights::MAP.0);
        launch.rights[7] = Rights(Rights::READ.0 | Rights::WRITE.0 | Rights::MAP.0);
        launch.rights[8] = Rights::WRITE;
    }
    let pid = match microsystem_user_rt::thread_start_ex_v2(&launch) {
        Ok(pid) => pid,
        Err(status) => {
            let _ = microsystem_user_rt::debug_write_u64(
                b"[mica] launch rejected stage=thread status=",
                (status as i64).unsigned_abs(),
                b"\n",
            );
            if let Some((commands, events, endpoint_slot)) = gui_resources {
                release_gui_endpoint(endpoint_slot, token);
                let _ = microsystem_user_rt::cap_delete(events);
                let _ = microsystem_user_rt::cap_delete(commands);
            }
            unregister_script_network(token);
            unregister_script_filesystem(token);
            let _ = microsystem_user_rt::cap_delete(wait_notification);
            let _ = microsystem_user_rt::cap_delete(notification);
            let _ = microsystem_user_rt::cap_delete(region);
            return status;
        }
    };
    let Some(slot) = pid
        .checked_sub(process::FIRST_APPLICATION_PID)
        .map(|value| value as usize)
    else {
        return Status::Fault;
    };
    if slot >= SCRIPT_NOTIFICATIONS.len() {
        let _ = microsystem_user_rt::thread_kill(pid, -15);
        return Status::Fault;
    }
    if let Some((commands, events, endpoint_slot)) = gui_resources {
        if let Err(status) = register_script_gui(
            endpoint_slot,
            pid,
            token,
            desktop_application,
            commands,
            events,
            notification,
        ) {
            let _ = microsystem_user_rt::thread_kill(pid, -15);
            release_gui_endpoint(endpoint_slot, token);
            let _ = microsystem_user_rt::cap_delete(events);
            let _ = microsystem_user_rt::cap_delete(commands);
            unregister_script_network(token);
            unregister_script_filesystem(token);
            let _ = microsystem_user_rt::cap_delete(wait_notification);
            let _ = microsystem_user_rt::cap_delete(notification);
            let _ = microsystem_user_rt::cap_delete(region);
            return status;
        }
        SCRIPT_GUI_COMMANDS[slot].store(commands.0, Ordering::Release);
        SCRIPT_GUI_EVENTS[slot].store(events.0, Ordering::Release);
        SCRIPT_GUI_ENDPOINT_SLOTS[slot].store(endpoint_slot as u32 + 1, Ordering::Release);
    }
    PROCESS_PROGRAMS[slot].store(program_id("mica"), Ordering::Release);
    identity::process_owner(pid, actor);
    SCRIPT_NOTIFICATIONS[slot].store(notification.0, Ordering::Release);
    SCRIPT_TOKENS[slot].store(token, Ordering::Release);
    SCRIPT_REGIONS[slot].store(region.0, Ordering::Release);
    reply.words[0] = pid;
    reply.caps[0] = region;
    reply.caps[1] = wait_notification;
    reply.flags |= message_cap_move(1);
    Status::Ok
}

fn identify_desktop_application(region: CapHandle) -> Option<gui::Application> {
    microsystem_user_rt::frame_map(region, script::SESSION_VA, Rights::READ).ok()?;
    let application = (|| {
        let header = unsafe { &*(script::SESSION_VA as *const script::SessionHeaderV1) };
        let offset = header.policy_bytes as usize;
        let length = header.path_bytes as usize;
        if offset.saturating_add(length) > script::POLICY_BYTES {
            return None;
        }
        let path = unsafe {
            core::slice::from_raw_parts(
                (script::SESSION_VA as *const u8).add(script::POLICY_OFFSET + offset),
                length,
            )
        };
        match path {
            b"/.system/examples/mica/browser.mica" => Some(gui::Application::Reader),
            b"/.system/examples/mica/editor.mica" => Some(gui::Application::Editor),
            _ => None,
        }
    })();
    let _ = microsystem_user_rt::frame_unmap(region, script::SESSION_VA);
    application
}

fn reserve_gui_endpoint(token: u64) -> Option<usize> {
    GUI_ENDPOINT_OWNERS
        .iter()
        .enumerate()
        .find_map(|(index, owner)| {
            owner
                .compare_exchange(0, token, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
                .then_some(index)
        })
}

fn release_gui_endpoint(index: usize, token: u64) {
    if let Some(owner) = GUI_ENDPOINT_OWNERS.get(index) {
        let _ = owner.compare_exchange(token, 0, Ordering::AcqRel, Ordering::Acquire);
    }
}

fn initialize_gui_event_ring(events: CapHandle) -> Result<(), Status> {
    microsystem_user_rt::frame_map(
        events,
        script::GUI_EVENT_VA,
        Rights(Rights::READ.0 | Rights::WRITE.0),
    )?;
    unsafe { core::ptr::write_bytes(script::GUI_EVENT_VA as *mut u8, 0, gui::EVENT_BYTES) };
    let header = unsafe { &mut *(script::GUI_EVENT_VA as *mut gui::EventRingHeaderV1) };
    *header = gui::EventRingHeaderV1 {
        magic: gui::EVENT_MAGIC,
        version: gui::VERSION,
        capacity: gui::EVENT_CAPACITY as u16,
        ..gui::EventRingHeaderV1::default()
    };
    microsystem_user_rt::frame_unmap(events, script::GUI_EVENT_VA)
}

fn register_script_gui(
    endpoint_slot: usize,
    pid: u64,
    token: u64,
    desktop_application: Option<gui::Application>,
    commands: CapHandle,
    events: CapHandle,
    notification: CapHandle,
) -> Result<(), Status> {
    let _ = microsystem_user_rt::debug_write_u64(
        b"[gui] mica registration requested endpoint=",
        endpoint_slot as u64,
        b"\n",
    );
    let mut request = Message::new(protocol::GUI, gui::Operation::RegisterClient as u16);
    request.words[0] = endpoint_slot as u64;
    request.words[1] = pid;
    request.words[2] = token;
    request.words[3] = desktop_application.map_or(0, |application| application as u64);
    request.caps[0] = commands;
    request.caps[1] = events;
    request.caps[2] = notification;
    let mut reply = Message::new(protocol::GUI, 0);
    if let Err(status) =
        microsystem_user_rt::ipc_call(boot_cap::GUI_CONFIG_ENDPOINT, &request, &mut reply, 0)
    {
        let _ = microsystem_user_rt::debug_write_u64(
            b"[gui] mica registration IPC failed status=",
            (status as i64).unsigned_abs(),
            b"\n",
        );
        return Err(status);
    }
    let result = status_word(reply.words[5]);
    if let Err(status) = result {
        let _ = microsystem_user_rt::debug_write_u64(
            b"[gui] mica registration rejected status=",
            (status as i64).unsigned_abs(),
            b"\n",
        );
    }
    result
}

fn unregister_script_gui(endpoint_slot: usize, token: u64) {
    let mut request = Message::new(protocol::GUI, gui::Operation::UnregisterClient as u16);
    request.words[0] = endpoint_slot as u64;
    request.words[2] = token;
    let mut reply = Message::new(protocol::GUI, 0);
    let deadline = microsystem_user_rt::clock_now()
        .unwrap_or(1)
        .saturating_add(100_000_000);
    let _ = microsystem_user_rt::ipc_call(
        boot_cap::GUI_CONFIG_ENDPOINT,
        &request,
        &mut reply,
        deadline,
    );
}

fn reap_script_sessions() {
    let now = microsystem_user_rt::clock_now().unwrap_or(0);
    for slot in 0..process::MAX_APPLICATIONS {
        if SCRIPT_TOKENS[slot].load(Ordering::Acquire) == 0 {
            continue;
        }
        let pid = process::FIRST_APPLICATION_PID + slot as u64;
        match microsystem_user_rt::thread_status(pid) {
            Ok((false, _)) | Err(Status::NotFound) => {
                cleanup_script_session(pid);
                continue;
            }
            Err(_) => continue,
            Ok((true, _)) => {}
        }
        let events = CapHandle(SCRIPT_GUI_EVENTS[slot].load(Ordering::Acquire));
        if events == CapHandle::INVALID {
            continue;
        }
        let close_requested =
            microsystem_user_rt::frame_map(events, script::GUI_EVENT_VA, Rights::READ).is_ok_and(
                |()| {
                    microsystem_user_rt::fence();
                    let header =
                        unsafe { &*(script::GUI_EVENT_VA as *const gui::EventRingHeaderV1) };
                    let requested = header.magic == gui::EVENT_MAGIC
                        && header.flags & gui::EVENT_RING_FLAG_CLOSE_REQUESTED != 0;
                    let _ = microsystem_user_rt::frame_unmap(events, script::GUI_EVENT_VA);
                    requested
                },
            );
        if !close_requested {
            SCRIPT_GUI_CLOSE_DEADLINES[slot].store(0, Ordering::Release);
            continue;
        }
        let deadline = SCRIPT_GUI_CLOSE_DEADLINES[slot].load(Ordering::Acquire);
        if deadline == 0 {
            SCRIPT_GUI_CLOSE_DEADLINES[slot]
                .store(now.saturating_add(2_000_000_000), Ordering::Release);
        } else if now >= deadline {
            let _ = microsystem_user_rt::thread_kill(pid, -15);
        }
    }
}

fn inspect_script_permissions(region: CapHandle) -> Result<(bool, bool, bool, u64), Status> {
    microsystem_user_rt::frame_map(
        region,
        script::SESSION_VA,
        Rights(Rights::READ.0 | Rights::WRITE.0),
    )?;
    let result = (|| {
        let header = unsafe { &mut *(script::SESSION_VA as *mut script::SessionHeaderV1) };
        if header.magic != script::SESSION_MAGIC
            || header.version != script::VERSION
            || !matches!(
                header.mode,
                script::MODE_EVAL | script::MODE_FILE | script::MODE_REPL
            )
            || (header.mode != script::MODE_REPL && header.source_bytes == 0)
            || header.source_bytes as usize > script::SOURCE_BYTES
            || header.policy_bytes as usize > script::POLICY_BYTES
            || header.argv_bytes as usize > script::STDIN_BYTES
            || header.path_bytes as usize
                > script::POLICY_BYTES.saturating_sub(header.policy_bytes as usize)
            || (header.mode == script::MODE_FILE && header.path_bytes == 0)
        {
            return Err(Status::Invalid);
        }
        let source = unsafe {
            core::slice::from_raw_parts(
                (script::SESSION_VA as *const u8).add(script::SOURCE_OFFSET),
                header.source_bytes as usize,
            )
        };
        let policy = unsafe {
            core::slice::from_raw_parts(
                (script::SESSION_VA as *const u8).add(script::POLICY_OFFSET),
                header.policy_bytes as usize,
            )
        };
        let source = core::str::from_utf8(source).map_err(|_| Status::Invalid)?;
        let policy = core::str::from_utf8(policy).map_err(|_| Status::Invalid)?;
        let mut token = [0u8; 8];
        microsystem_user_rt::random_fill(boot_cap::RANDOM_SOURCE, &mut token)?;
        let token = u64::from_le_bytes(token).max(1);
        header.permission_mask = token;
        let launcher_only = header.mode != script::MODE_FILE;
        Ok((
            (launcher_only || manifest_has_permission(source, "--!allow random"))
                && has_permission(policy, "random"),
            (launcher_only || manifest_has_permission(source, "--!allow sys.stats"))
                && has_permission(policy, "sys.stats"),
            !launcher_only
                && manifest_has_permission(source, "--!allow gui.window")
                && has_permission(policy, "gui.window"),
            token,
        ))
    })();
    microsystem_user_rt::frame_unmap(region, script::SESSION_VA)?;
    result
}

fn register_script_filesystem(region: CapHandle, token: u64, uid: u32) -> Result<(), Status> {
    let mut request = Message::new(protocol::FILESYSTEM, filesystem::SCRIPT_REGISTER);
    request.words[3] = token;
    request.words[0] = uid as u64;
    request.caps[0] = region;
    let mut reply = Message::new(protocol::FILESYSTEM, 0);
    microsystem_user_rt::ipc_call(boot_cap::FILESYSTEM_ENDPOINT, &request, &mut reply, 0)?;
    status_word(reply.words[5])
}

fn unregister_script_filesystem(token: u64) {
    if token == 0 {
        return;
    }
    let mut request = Message::new(protocol::FILESYSTEM, filesystem::SCRIPT_UNREGISTER);
    request.words[3] = token;
    let mut reply = Message::new(protocol::FILESYSTEM, 0);
    let deadline = microsystem_user_rt::clock_now()
        .unwrap_or(1)
        .saturating_add(100_000_000);
    let _ = microsystem_user_rt::ipc_call(
        boot_cap::FILESYSTEM_ENDPOINT,
        &request,
        &mut reply,
        deadline,
    );
}

fn register_script_network(region: CapHandle, token: u64) -> Result<(), Status> {
    let mut request = Message::new(
        protocol::NETWORK,
        microsystem_abi::network::Operation::OpenSession as u16,
    );
    request.words[0] = token;
    request.caps[0] = region;
    let mut reply = Message::new(protocol::NETWORK, 0);
    microsystem_user_rt::ipc_call(boot_cap::NETWORK_ENDPOINT, &request, &mut reply, 0)?;
    status_word(reply.words[5])
}

fn unregister_script_network(token: u64) {
    if token == 0 {
        return;
    }
    let mut request = Message::new(
        protocol::NETWORK,
        microsystem_abi::network::Operation::CloseSession as u16,
    );
    request.words[0] = token;
    let mut reply = Message::new(protocol::NETWORK, 0);
    let deadline = microsystem_user_rt::clock_now()
        .unwrap_or(1)
        .saturating_add(100_000_000);
    let _ =
        microsystem_user_rt::ipc_call(boot_cap::NETWORK_ENDPOINT, &request, &mut reply, deadline);
}

fn status_word(value: u64) -> Result<(), Status> {
    match value as i64 {
        0 => Ok(()),
        -1 => Err(Status::Invalid),
        -2 => Err(Status::BadCapability),
        -3 => Err(Status::AccessDenied),
        -4 => Err(Status::NotFound),
        -5 => Err(Status::NoMemory),
        -6 => Err(Status::Busy),
        -7 => Err(Status::TimedOut),
        -8 => Err(Status::Fault),
        -9 => Err(Status::NotSupported),
        -10 => Err(Status::Io),
        -11 => Err(Status::NoSpace),
        -12 => Err(Status::Corrupt),
        _ => Err(Status::Fault),
    }
}

fn has_permission(input: &str, rule: &str) -> bool {
    input.lines().any(|line| line.trim() == rule)
}

fn manifest_has_permission(input: &str, rule: &str) -> bool {
    for line in input.lines() {
        let line = line.trim();
        if line == rule {
            return true;
        }
        if !line.is_empty() && !line.starts_with("--") {
            break;
        }
    }
    false
}

fn cleanup_script_session(pid: u64) {
    let Some(slot) = pid
        .checked_sub(process::FIRST_APPLICATION_PID)
        .map(|value| value as usize)
    else {
        return;
    };
    let Some(notification) = SCRIPT_NOTIFICATIONS.get(slot) else {
        return;
    };
    let token = SCRIPT_TOKENS[slot].load(Ordering::Acquire);
    if token == 0 {
        return;
    }
    let endpoint_slot = SCRIPT_GUI_ENDPOINT_SLOTS[slot].swap(0, Ordering::AcqRel);
    if endpoint_slot != 0 {
        unregister_script_gui(endpoint_slot as usize - 1, token);
        release_gui_endpoint(endpoint_slot as usize - 1, token);
    }
    SCRIPT_GUI_CLOSE_DEADLINES[slot].store(0, Ordering::Release);
    let events = CapHandle(SCRIPT_GUI_EVENTS[slot].swap(0, Ordering::AcqRel));
    if events != CapHandle::INVALID {
        let _ = microsystem_user_rt::cap_delete(events);
    }
    let commands = CapHandle(SCRIPT_GUI_COMMANDS[slot].swap(0, Ordering::AcqRel));
    if commands != CapHandle::INVALID {
        let _ = microsystem_user_rt::cap_delete(commands);
    }
    let handle = CapHandle(notification.swap(0, Ordering::AcqRel));
    if handle != CapHandle::INVALID {
        let _ = microsystem_user_rt::cap_delete(handle);
    }
    if let Some(token) = SCRIPT_TOKENS.get(slot) {
        let token = token.swap(0, Ordering::AcqRel);
        unregister_script_network(token);
        unregister_script_filesystem(token);
    }
    if let Some(region) = SCRIPT_REGIONS.get(slot) {
        let region = CapHandle(region.swap(0, Ordering::AcqRel));
        if region != CapHandle::INVALID {
            let _ = microsystem_user_rt::cap_delete(region);
        }
    }
    for (child_slot, owner) in SCRIPT_CHILD_OWNER.iter().enumerate() {
        if owner.load(Ordering::Acquire) == pid {
            let child_pid = process::FIRST_APPLICATION_PID + child_slot as u64;
            if microsystem_user_rt::thread_status(child_pid).is_ok_and(|(running, _)| running) {
                let _ = microsystem_user_rt::thread_kill(child_pid, -15);
            }
            owner.store(0, Ordering::Release);
        }
    }
}

fn program_id(name: &str) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in name.as_bytes() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x100_0000_01b3);
    }
    hash
}

fn verify_cross_task_capabilities() {
    let rights = Rights(Rights::READ.0 | Rights::GRANT.0 | Rights::MANAGE.0);
    let root = microsystem_user_rt::object_create(ObjectType::Notification, rights)
        .unwrap_or_else(|_| microsystem_user_rt::exit(9));
    let child = microsystem_user_rt::cap_copy(root, Rights(Rights::READ.0 | Rights::GRANT.0))
        .unwrap_or_else(|_| microsystem_user_rt::exit(10));
    let mut invalid = Message::new(protocol::CONSOLE, 0);
    invalid.caps[0] = child;
    invalid.caps[1] = CapHandle::from_parts(250, 55);
    let mut invalid_reply = Message::new(protocol::CONSOLE, 0);
    if microsystem_user_rt::ipc_call(boot_cap::CONSOLE_ENDPOINT, &invalid, &mut invalid_reply, 0)
        != Err(Status::BadCapability)
        || microsystem_user_rt::cap_revoke(root).ok() != Some(1)
        || microsystem_user_rt::cap_delete(child).is_ok()
        || microsystem_user_rt::cap_delete(root).is_err()
    {
        microsystem_user_rt::exit(11);
    }

    let root = microsystem_user_rt::object_create(ObjectType::Notification, rights)
        .unwrap_or_else(|_| microsystem_user_rt::exit(12));
    let child = microsystem_user_rt::cap_copy(root, Rights(Rights::READ.0 | Rights::GRANT.0))
        .unwrap_or_else(|_| microsystem_user_rt::exit(13));
    let mut request = Message::new(protocol::CONSOLE, 0);
    request.words[0] = 0x20;
    request.caps[0] = child;
    let mut reply = Message::new(protocol::CONSOLE, 0);
    if microsystem_user_rt::ipc_call(boot_cap::CONSOLE_ENDPOINT, &request, &mut reply, 0).is_err()
        || reply.words[0] != 0x21
        || reply.caps[0] == CapHandle::INVALID
        || microsystem_user_rt::cap_revoke(root).ok() != Some(2)
        || microsystem_user_rt::cap_delete(child).is_ok()
        || microsystem_user_rt::cap_delete(reply.caps[0]).is_ok()
        || microsystem_user_rt::cap_delete(root).is_err()
    {
        microsystem_user_rt::exit(14);
    }
    let _ = microsystem_user_rt::debug_write(
        b"[cap] resident cross-task copy-move revoke atomic=true\n",
    );
}

#[panic_handler]
fn panic(_info: &PanicInfo<'_>) -> ! {
    microsystem_user_rt::exit(1)
}
