#![no_std]
#![no_main]

extern crate alloc;

use core::panic::PanicInfo;
use microsystem_abi::{CapHandle, Message, Rights, Status, boot_cap, network, protocol, script};
use microsystem_netd::config::Configuration;
use smoltcp::socket::{tcp, udp};
use smoltcp::wire::{IpAddress, Ipv4Address};
use stack::{NetworkStack, SocketRef};

mod administration;
use microsystem_netd::dns::{dns_name_hash, encode_dns_query, parse_dns_response};
mod config_runtime;
mod stack;

const SHARED_VA: u64 = 0x0058_0000;
const FRAME_BYTES: usize = 4096;
const SSH_PORT: u16 = 22;
const SSH_CONNECTIONS: usize = 8;
const NETWORK_SESSION_VA: u64 = 0x0059_0000;
const NETWORK_SESSION_LIMIT: usize = 8;
const CONNECTIONS_PER_SESSION: usize = 8;
const OUTBOUND_CONNECTIONS: usize = network::MAX_INTERFACES * stack::TCP_PER_INTERFACE;
const UDP_CONNECTIONS: usize = network::MAX_INTERFACES * stack::UDP_PER_INTERFACE;
const DNS_PORT: u16 = 53;
const DNS_PACKET_BYTES: usize = 512;

#[derive(Clone, Copy)]
enum Connection {
    Empty,
    Tcp(u8),
    Udp(u8),
}

#[derive(Clone, Copy)]
struct NetworkSession {
    token: u64,
    region: CapHandle,
    connections: [Connection; CONNECTIONS_PER_SESSION],
    generations: [u32; CONNECTIONS_PER_SESSION],
    query_id: u16,
    query_hash: u64,
    query_address: Option<IpAddress>,
    query_type: u16,
    query_failed: bool,
    query_port: usize,
    next_query_id: u16,
}

impl NetworkSession {
    const EMPTY: Self = Self {
        token: 0,
        region: CapHandle::INVALID,
        connections: [Connection::Empty; CONNECTIONS_PER_SESSION],
        generations: [0; CONNECTIONS_PER_SESSION],
        query_id: 0,
        query_hash: 0,
        query_address: None,
        query_type: 1,
        query_failed: false,
        query_port: 0,
        next_query_id: 1,
    };
}

#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    let _ = microsystem_user_rt::debug_write(b"[user] netd service ELF entered EL0\n");
    let rights = Rights(
        Rights::READ.0 | Rights::WRITE.0 | Rights::MAP.0 | Rights::GRANT.0 | Rights::MANAGE.0,
    );
    let frame = microsystem_user_rt::frame_create(boot_cap::NETWORK_MEMORY_POOL, rights)
        .unwrap_or_else(|_| microsystem_user_rt::exit(2));
    microsystem_user_rt::frame_map(frame, SHARED_VA, Rights(Rights::READ.0 | Rights::WRITE.0))
        .unwrap_or_else(|_| microsystem_user_rt::exit(3));

    let mut seed = [0u8; 8];
    microsystem_user_rt::random_fill(boot_cap::RANDOM_SOURCE, &mut seed)
        .unwrap_or_else(|_| microsystem_user_rt::exit(4));
    let configuration_deadline = microsystem_user_rt::clock_now()
        .unwrap_or(0)
        .saturating_add(110_000_000_000);
    let configuration = loop {
        match config_runtime::load() {
            Ok(configuration) => break configuration,
            Err(Status::NotFound) => break Configuration::default(),
            Err(status)
                if matches!(status, Status::TimedOut | Status::Busy | Status::Io)
                    && microsystem_user_rt::clock_now().unwrap_or(configuration_deadline)
                        < configuration_deadline =>
            {
                let _ = microsystem_user_rt::yield_now();
            }
            Err(_) => {
                let _ = microsystem_user_rt::debug_write(
                    b"[net] persisted configuration unavailable; using DHCP/SLAAC\n",
                );
                break Configuration::default();
            }
        }
    };
    let mut sockets = NetworkStack::new(configuration, u64::from_le_bytes(seed))
        .unwrap_or_else(|_| microsystem_user_rt::exit(5));
    let outbound: [SocketRef; OUTBOUND_CONNECTIONS] = sockets
        .ports
        .iter()
        .flat_map(|port| port.outbound.iter().copied())
        .collect::<alloc::vec::Vec<_>>()
        .try_into()
        .unwrap();
    let datagrams: [SocketRef; UDP_CONNECTIONS] = sockets
        .ports
        .iter()
        .flat_map(|port| port.datagrams.iter().copied())
        .collect::<alloc::vec::Vec<_>>()
        .try_into()
        .unwrap();
    let ssh: [SocketRef; SSH_CONNECTIONS] = sockets
        .ports
        .iter()
        .flat_map(|port| port.ssh.iter().copied())
        .collect::<alloc::vec::Vec<_>>()
        .try_into()
        .unwrap();
    let mut sessions = [NetworkSession::EMPTY; NETWORK_SESSION_LIMIT];
    let mut pending_ssh = None;
    let mut pending_network = None;
    let _ = microsystem_user_rt::debug_write(
        b"[net] interfaces initialized; DHCP/SLAAC acquisition asynchronous=true\n",
    );
    let _ = microsystem_user_rt::service_online();
    loop {
        let reset = sockets.poll();
        if reset != 0 {
            for session in &mut sessions {
                for connection in &mut session.connections {
                    let port = match *connection {
                        Connection::Tcp(index) => Some(outbound[index as usize].port),
                        Connection::Udp(index) => Some(datagrams[index as usize].port),
                        Connection::Empty => None,
                    };
                    if port.is_some_and(|port| reset & (1 << port) != 0) {
                        *connection = Connection::Empty;
                    }
                }
                if session.query_id != 0 && reset & (1 << session.query_port) != 0 {
                    session.query_failed = true;
                }
            }
        }
        if service_ssh(&mut sockets, &ssh, frame, &mut pending_ssh).is_err() {
            microsystem_user_rt::exit(6);
        }
        if service_network(
            &mut sockets,
            &outbound,
            &datagrams,
            &mut sessions,
            &mut pending_network,
        )
        .is_err()
        {
            microsystem_user_rt::exit(7);
        }
        let _ = microsystem_user_rt::yield_now();
    }
}

fn service_ssh(
    sockets: &mut NetworkStack,
    ssh: &[SocketRef; SSH_CONNECTIONS],
    frame: microsystem_abi::CapHandle,
    pending: &mut Option<Message>,
) -> Result<(), Status> {
    let mut request = if let Some(request) = pending.take() {
        request
    } else {
        let deadline = microsystem_user_rt::clock_now()?.saturating_add(100_000);
        let mut request = Message::new(protocol::NETWORK, 0);
        match microsystem_user_rt::ipc_recv(boot_cap::SSH_NETWORK_ENDPOINT, &mut request, deadline)
        {
            Err(Status::TimedOut) => return Ok(()),
            Err(status) => return Err(status),
            Ok(()) => request,
        }
    };
    let mut reply = Message::new(protocol::NETWORK, request.opcode);
    reply.words[5] = handle_ssh(&request, &mut reply, sockets, ssh, frame) as i64 as u64;
    let deadline = microsystem_user_rt::clock_now()?;
    match microsystem_user_rt::ipc_reply_recv(
        boot_cap::SSH_NETWORK_ENDPOINT,
        &reply,
        &mut request,
        deadline,
    ) {
        Ok(()) => *pending = Some(request),
        Err(Status::TimedOut) => {}
        Err(status) => return Err(status),
    }
    Ok(())
}

fn handle_ssh(
    request: &Message,
    reply: &mut Message,
    sockets: &mut NetworkStack,
    ssh: &[SocketRef; SSH_CONNECTIONS],
    frame: microsystem_abi::CapHandle,
) -> Status {
    if request.protocol != protocol::NETWORK {
        return Status::Invalid;
    }
    match request.opcode {
        value if value == network::Operation::SshSharedFrame as u16 => {
            reply.caps[0] = frame;
            Status::Ok
        }
        value if value == network::Operation::SshAccept as u16 => {
            for (index, handle) in ssh.iter().enumerate() {
                if sockets.get::<tcp::Socket>(*handle).state() == tcp::State::Established {
                    reply.words[0] = index as u64 + 1;
                    return Status::Ok;
                }
            }
            Status::Busy
        }
        value if value == network::Operation::SshReceive as u16 => {
            let Some(handle) = ssh_connection(ssh, request.words[0]) else {
                return Status::NotFound;
            };
            let maximum = (request.words[1] as usize).min(FRAME_BYTES);
            if maximum == 0 {
                return Status::Invalid;
            }
            let socket = sockets.get_mut::<tcp::Socket>(handle);
            if socket.can_recv() {
                let buffer =
                    unsafe { core::slice::from_raw_parts_mut(SHARED_VA as *mut u8, maximum) };
                match socket.recv_slice(buffer) {
                    Ok(bytes) => {
                        reply.words[0] = bytes as u64;
                        Status::Ok
                    }
                    Err(_) => Status::Io,
                }
            } else if socket.may_recv() {
                Status::Busy
            } else {
                reply.words[0] = 0;
                Status::Ok
            }
        }
        value if value == network::Operation::SshSend as u16 => {
            let Some(handle) = ssh_connection(ssh, request.words[0]) else {
                return Status::NotFound;
            };
            let bytes = request.words[1] as usize;
            if bytes == 0 || bytes > FRAME_BYTES {
                return Status::Invalid;
            }
            let socket = sockets.get_mut::<tcp::Socket>(handle);
            if !socket.can_send() {
                return if socket.may_send() {
                    Status::Busy
                } else {
                    Status::Io
                };
            }
            let input = unsafe { core::slice::from_raw_parts(SHARED_VA as *const u8, bytes) };
            match socket.send_slice(input) {
                Ok(written) => {
                    reply.words[0] = written as u64;
                    Status::Ok
                }
                Err(_) => Status::Io,
            }
        }
        value if value == network::Operation::SshClose as u16 => {
            let Some(handle) = ssh_connection(ssh, request.words[0]) else {
                return Status::NotFound;
            };
            let socket = sockets.get_mut::<tcp::Socket>(handle);
            if request.words[1] == 0 {
                return match socket.state() {
                    tcp::State::Listen => Status::Ok,
                    tcp::State::Closed if socket.remote_endpoint().is_none() => socket
                        .listen(SSH_PORT)
                        .map(|()| Status::Ok)
                        .unwrap_or(Status::Io),
                    tcp::State::Closed => Status::Busy,
                    _ => {
                        socket.abort();
                        Status::Busy
                    }
                };
            }
            match socket.state() {
                tcp::State::Listen => Status::Ok,
                tcp::State::Closed | tcp::State::TimeWait => {
                    socket.abort();
                    socket
                        .listen(SSH_PORT)
                        .map(|()| Status::Ok)
                        .unwrap_or(Status::Io)
                }
                tcp::State::Established | tcp::State::CloseWait => {
                    socket.close();
                    Status::Busy
                }
                _ => Status::Busy,
            }
        }
        value if value == network::Operation::SshStatus as u16 => {
            let Some(handle) = ssh_connection(ssh, request.words[0]) else {
                return Status::NotFound;
            };
            reply.words[0] =
                (sockets.get::<tcp::Socket>(handle).state() == tcp::State::Established) as u64;
            Status::Ok
        }
        _ => Status::Invalid,
    }
}

fn ssh_connection(ssh: &[SocketRef; SSH_CONNECTIONS], connection: u64) -> Option<SocketRef> {
    connection
        .checked_sub(1)
        .and_then(|index| ssh.get(index as usize))
        .copied()
}

fn service_network(
    sockets: &mut NetworkStack,
    outbound: &[SocketRef; OUTBOUND_CONNECTIONS],
    datagrams: &[SocketRef; UDP_CONNECTIONS],
    sessions: &mut [NetworkSession; NETWORK_SESSION_LIMIT],
    pending: &mut Option<Message>,
) -> Result<(), Status> {
    let mut request = if let Some(request) = pending.take() {
        request
    } else {
        let deadline = microsystem_user_rt::clock_now()?.saturating_add(100_000);
        let mut request = Message::new(protocol::NETWORK, 0);
        match microsystem_user_rt::ipc_recv(boot_cap::NETWORK_ENDPOINT, &mut request, deadline) {
            Err(Status::TimedOut) => return Ok(()),
            Err(status) => return Err(status),
            Ok(()) => request,
        }
    };
    let mut reply = Message::new(protocol::NETWORK, request.opcode);
    reply.words[5] =
        handle_network(&request, &mut reply, sockets, outbound, datagrams, sessions) as i64 as u64;
    let deadline = microsystem_user_rt::clock_now()?;
    match microsystem_user_rt::ipc_reply_recv(
        boot_cap::NETWORK_ENDPOINT,
        &reply,
        &mut request,
        deadline,
    ) {
        Ok(()) => *pending = Some(request),
        Err(Status::TimedOut) => {}
        Err(status) => return Err(status),
    }
    Ok(())
}

fn handle_network(
    request: &Message,
    reply: &mut Message,
    sockets: &mut NetworkStack,
    outbound: &[SocketRef; OUTBOUND_CONNECTIONS],
    datagrams: &[SocketRef; UDP_CONNECTIONS],
    sessions: &mut [NetworkSession; NETWORK_SESSION_LIMIT],
) -> Status {
    if request.protocol != protocol::NETWORK {
        return Status::Invalid;
    }
    if request.opcode == network::Operation::Configure as u16 {
        return administration::handle(request, reply, sockets);
    }
    if request.opcode == network::Operation::Stats as u16 {
        if let Some(index) = sockets.default_port() {
            let port = &mut sockets.ports[index];
            reply.words[0] = port
                .interface
                .ipv4_addr()
                .map(|address| u32::from_be_bytes(address.octets()) as u64)
                .unwrap_or(0);
            port.interface.routes_mut().update(|routes| {
                reply.words[1] = routes
                    .iter()
                    .find_map(|route| match route.via_router {
                        IpAddress::Ipv4(address) if route.cidr.prefix_len() == 0 => {
                            Some(u32::from_be_bytes(address.octets()) as u64)
                        }
                        _ => None,
                    })
                    .unwrap_or(0);
            });
            reply.words[2] = match port.dns_server {
                Some(IpAddress::Ipv4(address)) => u32::from_be_bytes(address.octets()) as u64,
                _ => 0,
            };
        }
        reply.words[3] = sessions.iter().filter(|session| session.token != 0).count() as u64;
        let active_connections = active_connection_count(sessions) as u64;
        reply.words[4] = (network::MAX_CONNECTIONS as u64) << 32 | active_connections;
        return Status::Ok;
    }
    if request.opcode == network::Operation::OpenSession as u16 {
        return register_network_session(request, sessions);
    }
    if request.opcode == network::Operation::CloseSession as u16 {
        return close_network_session(request.words[0], sockets, outbound, datagrams, sessions);
    }
    let token = request.words[0];
    let Some(session_index) = sessions
        .iter()
        .position(|session| session.token == token && token != 0)
    else {
        return Status::AccessDenied;
    };
    let region = sessions[session_index].region;
    if microsystem_user_rt::frame_map(
        region,
        NETWORK_SESSION_VA,
        Rights(Rights::READ.0 | Rights::WRITE.0),
    )
    .is_err()
    {
        return Status::BadCapability;
    }
    let status = handle_mapped_network(
        request,
        reply,
        sockets,
        outbound,
        datagrams,
        sessions,
        session_index,
    );
    let _ = microsystem_user_rt::frame_unmap(region, NETWORK_SESSION_VA);
    status
}

fn register_network_session(
    request: &Message,
    sessions: &mut [NetworkSession; NETWORK_SESSION_LIMIT],
) -> Status {
    let token = request.words[0];
    let region = request.caps[0];
    if token == 0
        || region == CapHandle::INVALID
        || sessions.iter().any(|session| session.token == token)
    {
        let _ = microsystem_user_rt::cap_delete(region);
        return Status::Invalid;
    }
    let Some(index) = sessions.iter().position(|session| session.token == 0) else {
        let _ = microsystem_user_rt::cap_delete(region);
        return Status::NoMemory;
    };
    if microsystem_user_rt::frame_map(region, NETWORK_SESSION_VA, Rights::READ).is_err() {
        let _ = microsystem_user_rt::cap_delete(region);
        return Status::BadCapability;
    }
    let header = unsafe { &*(NETWORK_SESSION_VA as *const script::SessionHeaderV1) };
    let valid = header.magic == script::SESSION_MAGIC
        && header.version == script::VERSION
        && header.argv_bytes as usize <= script::STDIN_BYTES
        && header.path_bytes as usize
            <= script::POLICY_BYTES.saturating_sub(header.policy_bytes as usize)
        && header.permission_mask == token;
    let _ = microsystem_user_rt::frame_unmap(region, NETWORK_SESSION_VA);
    if !valid {
        let _ = microsystem_user_rt::cap_delete(region);
        return Status::Invalid;
    }
    sessions[index] = NetworkSession {
        token,
        region,
        ..NetworkSession::EMPTY
    };
    Status::Ok
}

fn close_network_session(
    token: u64,
    sockets: &mut NetworkStack,
    outbound: &[SocketRef; OUTBOUND_CONNECTIONS],
    datagrams: &[SocketRef; UDP_CONNECTIONS],
    sessions: &mut [NetworkSession; NETWORK_SESSION_LIMIT],
) -> Status {
    let Some(index) = sessions
        .iter()
        .position(|session| session.token == token && token != 0)
    else {
        return Status::NotFound;
    };
    for connection in sessions[index].connections {
        match connection {
            Connection::Tcp(connection) => sockets
                .get_mut::<tcp::Socket>(outbound[connection as usize])
                .abort(),
            Connection::Udp(connection) => sockets
                .get_mut::<udp::Socket>(datagrams[connection as usize])
                .close(),
            Connection::Empty => {}
        }
    }
    let region = sessions[index].region;
    sessions[index] = NetworkSession::EMPTY;
    microsystem_user_rt::cap_delete(region)
        .map(|()| Status::Ok)
        .unwrap_or_else(|status| status)
}

fn handle_mapped_network(
    request: &Message,
    reply: &mut Message,
    sockets: &mut NetworkStack,
    outbound: &[SocketRef; OUTBOUND_CONNECTIONS],
    datagrams: &[SocketRef; UDP_CONNECTIONS],
    sessions: &mut [NetworkSession; NETWORK_SESSION_LIMIT],
    session_index: usize,
) -> Status {
    let header = unsafe { &*(NETWORK_SESSION_VA as *const script::SessionHeaderV1) };
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
        return Status::Invalid;
    }
    let source = unsafe {
        core::slice::from_raw_parts(
            (NETWORK_SESSION_VA as *const u8).add(script::SOURCE_OFFSET),
            header.source_bytes as usize,
        )
    };
    let policy = unsafe {
        core::slice::from_raw_parts(
            (NETWORK_SESSION_VA as *const u8).add(script::POLICY_OFFSET),
            header.policy_bytes as usize,
        )
    };
    let (Ok(source), Ok(policy)) = (core::str::from_utf8(source), core::str::from_utf8(policy))
    else {
        return Status::Invalid;
    };
    let source = if header.mode == script::MODE_FILE {
        source
    } else {
        ""
    };
    let payload = unsafe {
        core::slice::from_raw_parts_mut(
            (NETWORK_SESSION_VA as *mut u8).add(script::BROKER_OFFSET),
            script::BROKER_BYTES,
        )
    };

    match request.opcode {
        value if value == network::Operation::Resolve as u16 => {
            let host_bytes = request.words[3] as usize;
            if host_bytes == 0 || host_bytes > 253 || host_bytes > payload.len() {
                return Status::Invalid;
            }
            let Ok(host) = core::str::from_utf8(&payload[..host_bytes]) else {
                return Status::Invalid;
            };
            let browse = request.words[5] == network::BROWSE_REQUEST_MAGIC;
            if !network_host_allowed(source, policy, host, None, browse) {
                return Status::AccessDenied;
            }
            drain_dns_replies(sockets, sessions);
            let query_id = request.words[1] as u16;
            if query_id == 0 {
                if sessions[session_index].query_id != 0 {
                    reply.words[0] = sessions[session_index].query_id as u64;
                    return Status::Busy;
                }
                let Some(port) = sockets.default_port() else {
                    return Status::Busy;
                };
                let Some(server) = sockets.ports[port].dns_server else {
                    return Status::Busy;
                };
                let dns_handle = sockets.ports[port].dns;
                let sequence = sessions[session_index].next_query_id.max(1) & 0x0fff;
                let id = (((session_index + 1) as u16) << 12) | sequence;
                let mut packet = [0u8; DNS_PACKET_BYTES];
                let query_type = if request.words[2] == 6 {
                    28
                } else if request.words[2] == 0 || request.words[2] == 4 {
                    1
                } else {
                    return Status::Invalid;
                };
                let Some(bytes) = encode_dns_query(id, host, query_type, &mut packet) else {
                    return Status::Invalid;
                };
                match sockets
                    .get_mut::<udp::Socket>(dns_handle)
                    .send_slice(&packet[..bytes], (server, DNS_PORT))
                {
                    Ok(()) => {}
                    Err(udp::SendError::BufferFull) => return Status::Busy,
                    Err(_) => return Status::Io,
                }
                sessions[session_index].next_query_id = id.wrapping_add(1).max(1);
                sessions[session_index].query_id = id;
                sessions[session_index].query_port = port;
                sessions[session_index].query_hash = dns_name_hash(host.as_bytes());
                sessions[session_index].query_type = query_type;
                sessions[session_index].query_address = None;
                sessions[session_index].query_failed = false;
                reply.words[0] = id as u64;
                return Status::Busy;
            }
            if query_id != sessions[session_index].query_id {
                return Status::NotFound;
            }
            if sessions[session_index].query_failed {
                sessions[session_index].query_id = 0;
                sessions[session_index].query_failed = false;
                return Status::NotFound;
            }
            if sessions[session_index].query_address.is_none() {
                reply.words[0] = query_id as u64;
                return Status::Busy;
            }
            match sessions[session_index].query_address.unwrap() {
                IpAddress::Ipv4(address) => {
                    reply.words[0] = u32::from_be_bytes(address.octets()) as u64;
                    reply.words[2] = 4;
                }
                IpAddress::Ipv6(address) => {
                    let bytes = address.octets();
                    reply.words[0] = u64::from_be_bytes(bytes[..8].try_into().unwrap());
                    reply.words[1] = u64::from_be_bytes(bytes[8..].try_into().unwrap());
                    reply.words[2] = 6;
                }
            }
            sessions[session_index].query_id = 0;
            sessions[session_index].query_address = None;
            Status::Ok
        }
        value if value == network::Operation::TcpConnect as u16 => {
            let local_handle = request.words[1] as usize;
            let host_bytes = request.words[4] as usize;
            let port = request.words[3] as u16;
            if host_bytes == 0 || host_bytes > 253 || host_bytes > payload.len() || port == 0 {
                return Status::Invalid;
            }
            let Ok(host) = core::str::from_utf8(&payload[..host_bytes]) else {
                return Status::Invalid;
            };
            let browse = request.words[5] == network::BROWSE_REQUEST_MAGIC;
            if !network_host_allowed(source, policy, host, Some(port), browse) {
                return Status::AccessDenied;
            }
            if local_handle == 0 {
                if active_connection_count(sessions) >= network::MAX_CONNECTIONS {
                    return Status::NoMemory;
                }
                let Some(connection_slot) = sessions[session_index]
                    .connections
                    .iter()
                    .position(|connection| matches!(connection, Connection::Empty))
                else {
                    return Status::NoMemory;
                };
                let address = match request_address(host, request.words[2], &payload[host_bytes..])
                {
                    Some(address) => address,
                    None => return Status::Invalid,
                };
                let Some(port) = sockets.route(address) else {
                    return Status::Busy;
                };
                let Some(global) = free_outbound_connection(sessions, port) else {
                    return Status::NoMemory;
                };
                let local_port = sockets.allocate_tcp_port(port);
                if sockets
                    .connect(
                        outbound[global],
                        address,
                        request.words[3] as u16,
                        local_port,
                    )
                    .is_err()
                {
                    return Status::Io;
                }
                sessions[session_index].connections[connection_slot] =
                    Connection::Tcp(global as u8);
                reply.words[0] =
                    new_connection_handle(&mut sessions[session_index], connection_slot);
                return Status::Busy;
            }
            let Some(global) = tcp_session_connection(&sessions[session_index], local_handle)
            else {
                return Status::NotFound;
            };
            let socket = sockets.get::<tcp::Socket>(outbound[global]);
            reply.words[0] = local_handle as u64;
            if socket.state() == tcp::State::Established {
                Status::Ok
            } else if socket.is_open() {
                Status::Busy
            } else {
                Status::Io
            }
        }
        value if value == network::Operation::TcpRead as u16 => {
            let local = request.words[1] as usize;
            let maximum = (request.words[2] as usize).min(payload.len());
            let Some(global) = tcp_session_connection(&sessions[session_index], local) else {
                return Status::NotFound;
            };
            if maximum == 0 {
                return Status::Invalid;
            }
            let socket = sockets.get_mut::<tcp::Socket>(outbound[global]);
            if socket.can_recv() {
                match socket.recv_slice(&mut payload[..maximum]) {
                    Ok(bytes) => {
                        reply.words[0] = bytes as u64;
                        Status::Ok
                    }
                    Err(_) => Status::Io,
                }
            } else if socket.may_recv() {
                Status::Busy
            } else {
                reply.words[0] = 0;
                Status::Ok
            }
        }
        value if value == network::Operation::TcpWrite as u16 => {
            let local = request.words[1] as usize;
            let bytes = request.words[2] as usize;
            let Some(global) = tcp_session_connection(&sessions[session_index], local) else {
                return Status::NotFound;
            };
            if bytes == 0 || bytes > payload.len() {
                return Status::Invalid;
            }
            let socket = sockets.get_mut::<tcp::Socket>(outbound[global]);
            if !socket.can_send() {
                return if socket.may_send() {
                    Status::Busy
                } else {
                    Status::Io
                };
            }
            match socket.send_slice(&payload[..bytes]) {
                Ok(written) => {
                    reply.words[0] = written as u64;
                    Status::Ok
                }
                Err(_) => Status::Io,
            }
        }
        value if value == network::Operation::TcpClose as u16 => {
            let local = request.words[1] as usize;
            let Some(global) = tcp_session_connection(&sessions[session_index], local) else {
                return Status::NotFound;
            };
            sockets.get_mut::<tcp::Socket>(outbound[global]).abort();
            let slot = connection_slot(&sessions[session_index], local).unwrap();
            sessions[session_index].connections[slot] = Connection::Empty;
            Status::Ok
        }
        value if value == network::Operation::UdpOpen as u16 => {
            if active_connection_count(sessions) >= network::MAX_CONNECTIONS {
                return Status::NoMemory;
            }
            if sessions
                .iter()
                .flat_map(|session| session.connections.iter())
                .filter(|connection| matches!(connection, Connection::Udp(_)))
                .count()
                >= 8
            {
                return Status::NoMemory;
            }
            let Some(local_slot) = sessions[session_index]
                .connections
                .iter()
                .position(|connection| matches!(connection, Connection::Empty))
            else {
                return Status::NoMemory;
            };
            let port = if request.words[2] == 0 {
                sockets.default_port()
            } else {
                request.words[2].checked_sub(1).and_then(|index| {
                    sockets
                        .ports
                        .get(index as usize)
                        .filter(|port| port.available())
                        .map(|_| index as usize)
                })
            };
            let Some(port) = port else {
                return Status::Busy;
            };
            let Some(global) = free_udp_connection(sessions, port) else {
                return Status::NoMemory;
            };
            let requested_port = request.words[1] as u16;
            let port = if requested_port == 0 {
                55000 + global as u16
            } else {
                requested_port
            };
            if sockets
                .get_mut::<udp::Socket>(datagrams[global])
                .bind(port)
                .is_err()
            {
                return Status::Io;
            }
            sessions[session_index].connections[local_slot] = Connection::Udp(global as u8);
            reply.words[0] = new_connection_handle(&mut sessions[session_index], local_slot);
            reply.words[1] = port as u64;
            Status::Ok
        }
        value if value == network::Operation::UdpSendTo as u16 => {
            let local = request.words[1] as usize;
            let port = request.words[3] as u16;
            let host_bytes = request.words[4] as usize;
            let data_bytes = request.words[5] as usize;
            let Some(global) = udp_session_connection(&sessions[session_index], local) else {
                return Status::NotFound;
            };
            if port == 0
                || host_bytes == 0
                || host_bytes > 253
                || host_bytes.saturating_add(data_bytes) > payload.len()
            {
                return Status::Invalid;
            }
            let Ok(host) = core::str::from_utf8(&payload[..host_bytes]) else {
                return Status::Invalid;
            };
            if !network_host_allowed(source, policy, host, Some(port), false) {
                return Status::AccessDenied;
            }
            let address = match request_address(host, request.words[2], &payload[host_bytes..]) {
                Some(address) => address,
                None => return Status::Invalid,
            };
            let data_offset = host_bytes
                + if request.words[2] == network::IPV6_ADDRESS {
                    16
                } else {
                    0
                };
            if data_offset.saturating_add(data_bytes) > payload.len() {
                return Status::Invalid;
            }
            let maximum = match address {
                IpAddress::Ipv4(_) => 1472,
                IpAddress::Ipv6(_) => 1452,
            };
            if data_bytes > maximum {
                return Status::Invalid;
            }
            match sockets
                .get_mut::<udp::Socket>(datagrams[global])
                .send_slice(
                    &payload[data_offset..data_offset + data_bytes],
                    (address, port),
                ) {
                Ok(()) => {
                    reply.words[0] = data_bytes as u64;
                    Status::Ok
                }
                Err(udp::SendError::BufferFull) => Status::Busy,
                Err(_) => Status::Io,
            }
        }
        value if value == network::Operation::UdpRecvFrom as u16 => {
            let local = request.words[1] as usize;
            let maximum = (request.words[2] as usize).min(payload.len());
            let Some(global) = udp_session_connection(&sessions[session_index], local) else {
                return Status::NotFound;
            };
            if maximum == 0 {
                return Status::Invalid;
            }
            match sockets
                .get_mut::<udp::Socket>(datagrams[global])
                .recv_slice(&mut payload[..maximum])
            {
                Ok((bytes, endpoint)) => {
                    match endpoint.endpoint.addr {
                        IpAddress::Ipv4(address) => {
                            reply.words[1] = u32::from_be_bytes(address.octets()) as u64
                        }
                        IpAddress::Ipv6(address) => {
                            let bytes = address.octets();
                            reply.words[1] = u64::from_be_bytes(bytes[..8].try_into().unwrap());
                            reply.words[3] = u64::from_be_bytes(bytes[8..].try_into().unwrap());
                            reply.words[4] = 6;
                        }
                    }
                    reply.words[0] = bytes as u64;
                    reply.words[2] = endpoint.endpoint.port as u64;
                    Status::Ok
                }
                Err(udp::RecvError::Exhausted) => Status::Busy,
                Err(_) => Status::Io,
            }
        }
        value if value == network::Operation::UdpClose as u16 => {
            let local = request.words[1] as usize;
            let Some(global) = udp_session_connection(&sessions[session_index], local) else {
                return Status::NotFound;
            };
            sockets.get_mut::<udp::Socket>(datagrams[global]).close();
            let slot = connection_slot(&sessions[session_index], local).unwrap();
            sessions[session_index].connections[slot] = Connection::Empty;
            Status::Ok
        }
        value if value == network::Operation::Cancel as u16 => {
            sessions[session_index].query_id = 0;
            sessions[session_index].query_hash = 0;
            sessions[session_index].query_address = None;
            sessions[session_index].query_failed = false;
            Status::Ok
        }
        value if value == network::Operation::TlsConnect as u16 => Status::NotSupported,
        _ => Status::NotSupported,
    }
}

fn drain_dns_replies(
    sockets: &mut NetworkStack,
    sessions: &mut [NetworkSession; NETWORK_SESSION_LIMIT],
) {
    let mut packet = [0u8; DNS_PACKET_BYTES];
    for port in 0..sockets.ports.len() {
        let dns_handle = sockets.ports[port].dns;
        let server = sockets.ports[port].dns_server;
        loop {
            let received = sockets
                .get_mut::<udp::Socket>(dns_handle)
                .recv_slice(&mut packet);
            let Ok((bytes, endpoint)) = received else {
                break;
            };
            if Some(endpoint.endpoint.addr) != server
                || endpoint.endpoint.port != DNS_PORT
                || bytes < 12
            {
                continue;
            }
            let id = u16::from_be_bytes([packet[0], packet[1]]);
            let Some(session) = sessions
                .iter_mut()
                .find(|session| session.query_id == id && id != 0 && session.query_port == port)
            else {
                continue;
            };
            match parse_dns_response(&packet[..bytes], id, session.query_hash, session.query_type) {
                Some(address) => session.query_address = Some(address),
                None => session.query_failed = true,
            }
        }
    }
}

fn request_address(host: &str, word: u64, extra: &[u8]) -> Option<IpAddress> {
    if word == network::IPV6_ADDRESS {
        return Some(IpAddress::Ipv6(smoltcp::wire::Ipv6Address::from_octets(
            extra.get(..16)?.try_into().ok()?,
        )));
    }
    host.parse::<IpAddress>().ok().or_else(|| {
        Some(IpAddress::Ipv4(Ipv4Address::from_octets(
            (word as u32).to_be_bytes(),
        )))
    })
}

fn free_outbound_connection(
    sessions: &[NetworkSession; NETWORK_SESSION_LIMIT],
    port: usize,
) -> Option<usize> {
    (port * stack::TCP_PER_INTERFACE..(port + 1) * stack::TCP_PER_INTERFACE).find(|candidate| {
        sessions.iter().all(|session| {
            !session
                .connections
                .iter()
                .any(|connection| matches!(connection, Connection::Tcp(index) if *index as usize == *candidate))
        })
    })
}

fn tcp_session_connection(session: &NetworkSession, local: usize) -> Option<usize> {
    let index = connection_slot(session, local)?;
    match *session.connections.get(index)? {
        Connection::Tcp(global) => Some(global as usize),
        _ => None,
    }
}

fn udp_session_connection(session: &NetworkSession, local: usize) -> Option<usize> {
    let index = connection_slot(session, local)?;
    match *session.connections.get(index)? {
        Connection::Udp(global) => Some(global as usize),
        _ => None,
    }
}

fn new_connection_handle(session: &mut NetworkSession, slot: usize) -> u64 {
    session.generations[slot] = session.generations[slot].wrapping_add(1).max(1) & 0x7fff_ffff;
    session.generations[slot] = session.generations[slot].max(1);
    (u64::from(session.generations[slot]) << 32) | slot as u64 + 1
}

fn connection_slot(session: &NetworkSession, handle: usize) -> Option<usize> {
    let slot = (handle as u32).checked_sub(1)? as usize;
    let generation = (handle as u64 >> 32) as u32;
    (generation != 0
        && session
            .generations
            .get(slot)
            .is_some_and(|stored| *stored == generation))
    .then_some(slot)
}

fn free_udp_connection(
    sessions: &[NetworkSession; NETWORK_SESSION_LIMIT],
    port: usize,
) -> Option<usize> {
    (port * stack::UDP_PER_INTERFACE..(port + 1) * stack::UDP_PER_INTERFACE).find(|candidate| sessions.iter().all(|session| !session.connections.iter().any(|connection| matches!(connection, Connection::Udp(index) if *index as usize == *candidate))))
}

fn active_connection_count(sessions: &[NetworkSession; NETWORK_SESSION_LIMIT]) -> usize {
    sessions
        .iter()
        .flat_map(|session| session.connections.iter())
        .filter(|connection| !matches!(connection, Connection::Empty))
        .count()
}

fn network_host_allowed(
    source: &str,
    policy: &str,
    host: &str,
    port: Option<u16>,
    browse: bool,
) -> bool {
    (source.is_empty() || network_rules_allow(source, true, host, port, browse))
        && network_rules_allow(policy, false, host, port, browse)
}

fn network_rules_allow(
    input: &str,
    manifest: bool,
    host: &str,
    port: Option<u16>,
    browse: bool,
) -> bool {
    for line in input.lines() {
        let line = line.trim();
        let rule = if manifest {
            if let Some(rule) = line.strip_prefix("--!allow ") {
                rule
            } else {
                if !line.is_empty() && !line.starts_with("--") {
                    break;
                }
                continue;
            }
        } else {
            line
        };
        if browse && rule == "net.browse" {
            return true;
        }
        let Some(target) = rule.strip_prefix("net.connect:") else {
            continue;
        };
        let Some((allowed_host, allowed_port)) = target.rsplit_once(':') else {
            continue;
        };
        if !allowed_host.eq_ignore_ascii_case(host) {
            continue;
        }
        let Ok(allowed_port) = allowed_port.parse::<u16>() else {
            continue;
        };
        if port.is_none_or(|port| port == allowed_port) {
            return true;
        }
    }
    false
}

#[panic_handler]
fn panic(_info: &PanicInfo<'_>) -> ! {
    microsystem_user_rt::exit(1)
}
