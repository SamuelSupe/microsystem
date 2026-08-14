#![no_std]
#![no_main]

extern crate alloc;

use alloc::boxed::Box;
use alloc::vec;
use core::cell::UnsafeCell;
use core::panic::PanicInfo;
use microsystem_abi::{CapHandle, Message, Rights, Status, boot_cap, network, protocol, script};
use smoltcp::iface::{Config, Interface, SocketHandle, SocketSet, SocketStorage};
use smoltcp::phy::{Device, DeviceCapabilities, Medium, RxToken, TxToken};
use smoltcp::socket::{tcp, udp};
use smoltcp::time::Instant;
use smoltcp::wire::{EthernetAddress, HardwareAddress, IpAddress, IpCidr, Ipv4Address};

const SHARED_VA: u64 = 0x0058_0000;
const FRAME_BYTES: usize = 4096;
const MAC: [u8; 6] = [0x52, 0x54, 0x00, 0x12, 0x34, 0x56];
const SSH_PORT: u16 = 22;
const SSH_CONNECTIONS: usize = 2;
const NETWORK_SESSION_VA: u64 = 0x0059_0000;
const NETWORK_SESSION_LIMIT: usize = 8;
const CONNECTIONS_PER_SESSION: usize = 8;
const OUTBOUND_CONNECTIONS: usize = 64;
const UDP_CONNECTIONS: usize = 8;
const SOCKET_BYTES: usize = 2048;
const DNS_PORT: u16 = 53;
const DNS_LOCAL_PORT: u16 = 53053;
const DNS_SERVER: Ipv4Address = Ipv4Address::new(10, 0, 2, 3);
const DNS_PACKET_BYTES: usize = 512;

struct Bytes<const N: usize>(UnsafeCell<[u8; N]>);
unsafe impl<const N: usize> Sync for Bytes<N> {}

static SSH_RX: [Bytes<16384>; SSH_CONNECTIONS] =
    [const { Bytes(UnsafeCell::new([0; 16384])) }; SSH_CONNECTIONS];
static SSH_TX: [Bytes<16384>; SSH_CONNECTIONS] =
    [const { Bytes(UnsafeCell::new([0; 16384])) }; SSH_CONNECTIONS];
struct SocketBuffers(UnsafeCell<[[u8; SOCKET_BYTES]; OUTBOUND_CONNECTIONS]>);
unsafe impl Sync for SocketBuffers {}
static OUTBOUND_RX: SocketBuffers =
    SocketBuffers(UnsafeCell::new([[0; SOCKET_BYTES]; OUTBOUND_CONNECTIONS]));
static OUTBOUND_TX: SocketBuffers =
    SocketBuffers(UnsafeCell::new([[0; SOCKET_BYTES]; OUTBOUND_CONNECTIONS]));
struct UdpMetadata(UnsafeCell<[[udp::PacketMetadata; 8]; UDP_CONNECTIONS]>);
unsafe impl Sync for UdpMetadata {}
static UDP_RX_META: UdpMetadata = UdpMetadata(UnsafeCell::new(
    [[udp::PacketMetadata::EMPTY; 8]; UDP_CONNECTIONS],
));
static UDP_TX_META: UdpMetadata = UdpMetadata(UnsafeCell::new(
    [[udp::PacketMetadata::EMPTY; 8]; UDP_CONNECTIONS],
));
struct DnsMetadata(UnsafeCell<[udp::PacketMetadata; NETWORK_SESSION_LIMIT]>);
unsafe impl Sync for DnsMetadata {}
static DNS_RX_META: DnsMetadata = DnsMetadata(UnsafeCell::new(
    [udp::PacketMetadata::EMPTY; NETWORK_SESSION_LIMIT],
));
static DNS_TX_META: DnsMetadata = DnsMetadata(UnsafeCell::new(
    [udp::PacketMetadata::EMPTY; NETWORK_SESSION_LIMIT],
));
struct SocketStore(
    UnsafeCell<
        [SocketStorage<'static>; OUTBOUND_CONNECTIONS + UDP_CONNECTIONS + SSH_CONNECTIONS + 1],
    >,
);
unsafe impl Sync for SocketStore {}
static SOCKET_STORAGE: SocketStore = SocketStore(UnsafeCell::new(
    [SocketStorage::EMPTY; OUTBOUND_CONNECTIONS + UDP_CONNECTIONS + SSH_CONNECTIONS + 1],
));

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
    query_id: u16,
    query_hash: u64,
    query_address: u32,
    query_failed: bool,
    next_query_id: u16,
}

impl NetworkSession {
    const EMPTY: Self = Self {
        token: 0,
        region: CapHandle::INVALID,
        connections: [Connection::Empty; CONNECTIONS_PER_SESSION],
        query_id: 0,
        query_hash: 0,
        query_address: 0,
        query_failed: false,
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

    let mut device = DirectNetwork::new();
    let mut seed = [0u8; 8];
    microsystem_user_rt::random_fill(boot_cap::RANDOM_SOURCE, &mut seed)
        .unwrap_or_else(|_| microsystem_user_rt::exit(4));
    let mut config = Config::new(HardwareAddress::Ethernet(EthernetAddress(MAC)));
    config.random_seed = u64::from_le_bytes(seed);
    let mut interface = Interface::new(config, &mut device, now());
    interface.update_ip_addrs(|addresses| {
        addresses
            .push(IpCidr::new(IpAddress::v4(10, 0, 2, 15), 24))
            .unwrap();
    });
    interface
        .routes_mut()
        .add_default_ipv4_route(Ipv4Address::new(10, 0, 2, 2))
        .unwrap();

    let socket_storage = unsafe { &mut *SOCKET_STORAGE.0.get() };
    let mut sockets = SocketSet::new(&mut socket_storage[..]);
    let first_ssh = sockets.add(ssh_socket(0));
    let mut ssh = [first_ssh; SSH_CONNECTIONS];
    for index in 0..SSH_CONNECTIONS {
        if index != 0 {
            ssh[index] = sockets.add(ssh_socket(index));
        }
        sockets
            .get_mut::<tcp::Socket>(ssh[index])
            .listen(SSH_PORT)
            .unwrap_or_else(|_| microsystem_user_rt::exit(5));
    }
    let mut outbound = [first_ssh; OUTBOUND_CONNECTIONS];
    for index in 0..OUTBOUND_CONNECTIONS {
        let rx = unsafe { &mut *core::ptr::addr_of_mut!((*OUTBOUND_RX.0.get())[index]) };
        let tx = unsafe { &mut *core::ptr::addr_of_mut!((*OUTBOUND_TX.0.get())[index]) };
        let socket = tcp::Socket::new(
            tcp::SocketBuffer::new(&mut rx[..]),
            tcp::SocketBuffer::new(&mut tx[..]),
        );
        outbound[index] = sockets.add(socket);
    }
    let mut datagrams = [first_ssh; UDP_CONNECTIONS];
    for index in 0..UDP_CONNECTIONS {
        let rx_meta = unsafe { &mut *core::ptr::addr_of_mut!((*UDP_RX_META.0.get())[index]) };
        let tx_meta = unsafe { &mut *core::ptr::addr_of_mut!((*UDP_TX_META.0.get())[index]) };
        let rx = Box::leak(vec![0; network::MAX_TRANSFER_BYTES].into_boxed_slice());
        let tx = Box::leak(vec![0; network::MAX_TRANSFER_BYTES].into_boxed_slice());
        let socket = udp::Socket::new(
            udp::PacketBuffer::new(&mut rx_meta[..], &mut rx[..]),
            udp::PacketBuffer::new(&mut tx_meta[..], &mut tx[..]),
        );
        datagrams[index] = sockets.add(socket);
    }
    let dns_rx_meta = unsafe { &mut *DNS_RX_META.0.get() };
    let dns_tx_meta = unsafe { &mut *DNS_TX_META.0.get() };
    let dns_rx = Box::leak(vec![0; DNS_PACKET_BYTES * NETWORK_SESSION_LIMIT].into_boxed_slice());
    let dns_tx = Box::leak(vec![0; DNS_PACKET_BYTES * NETWORK_SESSION_LIMIT].into_boxed_slice());
    let mut dns_socket = udp::Socket::new(
        udp::PacketBuffer::new(&mut dns_rx_meta[..], dns_rx),
        udp::PacketBuffer::new(&mut dns_tx_meta[..], dns_tx),
    );
    dns_socket
        .bind(DNS_LOCAL_PORT)
        .unwrap_or_else(|_| microsystem_user_rt::exit(5));
    let dns = sockets.add(dns_socket);
    let mut sessions = [NetworkSession::EMPTY; NETWORK_SESSION_LIMIT];
    let mut pending_ssh = None;
    let mut pending_network = None;

    let _ = microsystem_user_rt::debug_write(
        b"[net] netd ready ipv4=10.0.2.15 outbound=true raw-device-isolated=true tcp-owner=true dns=true tcp=64 udp=8\n",
    );
    let _ = microsystem_user_rt::service_online();

    loop {
        interface.poll(now(), &mut device, &mut sockets);
        if service_ssh(
            &mut interface,
            &mut device,
            &mut sockets,
            &ssh,
            frame,
            &mut pending_ssh,
        )
        .is_err()
        {
            microsystem_user_rt::exit(6);
        }
        if service_network(
            &mut interface,
            &mut device,
            &mut sockets,
            &outbound,
            &datagrams,
            dns,
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

fn ssh_socket(index: usize) -> tcp::Socket<'static> {
    let rx = unsafe { &mut *SSH_RX[index].0.get() };
    let tx = unsafe { &mut *SSH_TX[index].0.get() };
    tcp::Socket::new(
        tcp::SocketBuffer::new(&mut rx[..]),
        tcp::SocketBuffer::new(&mut tx[..]),
    )
}

fn service_ssh(
    interface: &mut Interface,
    device: &mut DirectNetwork,
    sockets: &mut SocketSet<'static>,
    ssh: &[SocketHandle; SSH_CONNECTIONS],
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
    interface.poll(now(), device, sockets);
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
    sockets: &mut SocketSet<'static>,
    ssh: &[SocketHandle; SSH_CONNECTIONS],
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
                socket.abort();
                return socket
                    .listen(SSH_PORT)
                    .map(|()| Status::Ok)
                    .unwrap_or(Status::Io);
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

fn ssh_connection(ssh: &[SocketHandle; SSH_CONNECTIONS], connection: u64) -> Option<SocketHandle> {
    connection
        .checked_sub(1)
        .and_then(|index| ssh.get(index as usize))
        .copied()
}

fn service_network(
    interface: &mut Interface,
    device: &mut DirectNetwork,
    sockets: &mut SocketSet<'static>,
    outbound: &[SocketHandle; OUTBOUND_CONNECTIONS],
    datagrams: &[SocketHandle; UDP_CONNECTIONS],
    dns_handle: SocketHandle,
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
    reply.words[5] = handle_network(
        &request, &mut reply, interface, sockets, outbound, datagrams, dns_handle, sessions,
    ) as i64 as u64;
    interface.poll(now(), device, sockets);
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
    interface: &mut Interface,
    sockets: &mut SocketSet<'static>,
    outbound: &[SocketHandle; OUTBOUND_CONNECTIONS],
    datagrams: &[SocketHandle; UDP_CONNECTIONS],
    dns_handle: SocketHandle,
    sessions: &mut [NetworkSession; NETWORK_SESSION_LIMIT],
) -> Status {
    if request.protocol != protocol::NETWORK {
        return Status::Invalid;
    }
    if request.opcode == network::Operation::OpenSession as u16 {
        return register_network_session(request, sessions);
    }
    if request.opcode == network::Operation::CloseSession as u16 {
        return close_network_session(
            request.words[0],
            sockets,
            outbound,
            datagrams,
            dns_handle,
            sessions,
        );
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
        interface,
        sockets,
        outbound,
        datagrams,
        dns_handle,
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
    sockets: &mut SocketSet<'static>,
    outbound: &[SocketHandle; OUTBOUND_CONNECTIONS],
    datagrams: &[SocketHandle; UDP_CONNECTIONS],
    dns_handle: SocketHandle,
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
    let _ = dns_handle;
    let region = sessions[index].region;
    sessions[index] = NetworkSession::EMPTY;
    microsystem_user_rt::cap_delete(region)
        .map(|()| Status::Ok)
        .unwrap_or_else(|status| status)
}

fn handle_mapped_network(
    request: &Message,
    reply: &mut Message,
    interface: &mut Interface,
    sockets: &mut SocketSet<'static>,
    outbound: &[SocketHandle; OUTBOUND_CONNECTIONS],
    datagrams: &[SocketHandle; UDP_CONNECTIONS],
    dns_handle: SocketHandle,
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
            drain_dns_replies(sockets, dns_handle, sessions);
            let query_id = request.words[1] as u16;
            if query_id == 0 {
                if sessions[session_index].query_id != 0 {
                    reply.words[0] = sessions[session_index].query_id as u64;
                    return Status::Busy;
                }
                let sequence = sessions[session_index].next_query_id.max(1) & 0x0fff;
                let id = (((session_index + 1) as u16) << 12) | sequence;
                let mut packet = [0u8; DNS_PACKET_BYTES];
                let Some(bytes) = encode_dns_query(id, host, &mut packet) else {
                    return Status::Invalid;
                };
                match sockets
                    .get_mut::<udp::Socket>(dns_handle)
                    .send_slice(&packet[..bytes], (IpAddress::Ipv4(DNS_SERVER), DNS_PORT))
                {
                    Ok(()) => {}
                    Err(udp::SendError::BufferFull) => return Status::Busy,
                    Err(_) => return Status::Io,
                }
                sessions[session_index].next_query_id = id.wrapping_add(1).max(1);
                sessions[session_index].query_id = id;
                sessions[session_index].query_hash = dns_name_hash(host.as_bytes());
                sessions[session_index].query_address = 0;
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
            if sessions[session_index].query_address == 0 {
                reply.words[0] = query_id as u64;
                return Status::Busy;
            }
            reply.words[0] = sessions[session_index].query_address as u64;
            sessions[session_index].query_id = 0;
            sessions[session_index].query_address = 0;
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
                let Some(connection_slot) = sessions[session_index]
                    .connections
                    .iter()
                    .position(|connection| matches!(connection, Connection::Empty))
                else {
                    return Status::NoMemory;
                };
                let Some(global) = free_outbound_connection(sessions) else {
                    return Status::NoMemory;
                };
                let address = Ipv4Address::from_octets(u32::to_be_bytes(request.words[2] as u32));
                let local_port = 49152 + global as u16;
                if sockets
                    .get_mut::<tcp::Socket>(outbound[global])
                    .connect(
                        interface.context(),
                        (IpAddress::Ipv4(address), port),
                        local_port,
                    )
                    .is_err()
                {
                    return Status::Io;
                }
                sessions[session_index].connections[connection_slot] =
                    Connection::Tcp(global as u8);
                reply.words[0] = connection_slot as u64 + 1;
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
            sessions[session_index].connections[local - 1] = Connection::Empty;
            Status::Ok
        }
        value if value == network::Operation::UdpOpen as u16 => {
            if active_connection_count(sessions) >= network::MAX_CONNECTIONS {
                return Status::NoMemory;
            }
            let Some(local_slot) = sessions[session_index]
                .connections
                .iter()
                .position(|connection| matches!(connection, Connection::Empty))
            else {
                return Status::NoMemory;
            };
            let Some(global) = free_udp_connection(sessions) else {
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
            reply.words[0] = local_slot as u64 + 1;
            reply.words[1] = port as u64;
            Status::Ok
        }
        value if value == network::Operation::UdpSendTo as u16 => {
            let local = request.words[1] as usize;
            let address = Ipv4Address::from_octets(u32::to_be_bytes(request.words[2] as u32));
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
            match sockets
                .get_mut::<udp::Socket>(datagrams[global])
                .send_slice(
                    &payload[host_bytes..host_bytes + data_bytes],
                    (IpAddress::Ipv4(address), port),
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
                    let IpAddress::Ipv4(address) = endpoint.endpoint.addr;
                    reply.words[0] = bytes as u64;
                    reply.words[1] = u32::from_be_bytes(address.octets()) as u64;
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
            sessions[session_index].connections[local - 1] = Connection::Empty;
            Status::Ok
        }
        value if value == network::Operation::Cancel as u16 => {
            sessions[session_index].query_id = 0;
            sessions[session_index].query_hash = 0;
            sessions[session_index].query_address = 0;
            sessions[session_index].query_failed = false;
            Status::Ok
        }
        value if value == network::Operation::TlsConnect as u16 => Status::NotSupported,
        _ => Status::NotSupported,
    }
}

fn encode_dns_query(id: u16, host: &str, output: &mut [u8; DNS_PACKET_BYTES]) -> Option<usize> {
    if host.is_empty() || host.len() > 253 {
        return None;
    }
    output[..12].fill(0);
    output[..2].copy_from_slice(&id.to_be_bytes());
    output[2] = 0x01;
    output[5] = 1;
    let mut cursor = 12usize;
    for label in host.split('.') {
        if label.is_empty() || label.len() > 63 || cursor + 1 + label.len() + 5 > output.len() {
            return None;
        }
        output[cursor] = label.len() as u8;
        cursor += 1;
        output[cursor..cursor + label.len()].copy_from_slice(label.as_bytes());
        cursor += label.len();
    }
    output[cursor] = 0;
    cursor += 1;
    output[cursor..cursor + 2].copy_from_slice(&1u16.to_be_bytes());
    output[cursor + 2..cursor + 4].copy_from_slice(&1u16.to_be_bytes());
    Some(cursor + 4)
}

fn drain_dns_replies(
    sockets: &mut SocketSet<'static>,
    dns_handle: SocketHandle,
    sessions: &mut [NetworkSession; NETWORK_SESSION_LIMIT],
) {
    let mut packet = [0u8; DNS_PACKET_BYTES];
    loop {
        let received = sockets
            .get_mut::<udp::Socket>(dns_handle)
            .recv_slice(&mut packet);
        let Ok((bytes, endpoint)) = received else {
            break;
        };
        if endpoint.endpoint.addr != IpAddress::Ipv4(DNS_SERVER)
            || endpoint.endpoint.port != DNS_PORT
            || bytes < 12
        {
            continue;
        }
        let id = u16::from_be_bytes([packet[0], packet[1]]);
        let Some(session) = sessions
            .iter_mut()
            .find(|session| session.query_id == id && id != 0)
        else {
            continue;
        };
        match parse_dns_response(&packet[..bytes], id, session.query_hash) {
            Some(address) => session.query_address = address,
            None => session.query_failed = true,
        }
    }
}

fn parse_dns_response(packet: &[u8], id: u16, expected_name: u64) -> Option<u32> {
    if packet.len() < 12
        || u16::from_be_bytes([packet[0], packet[1]]) != id
        || packet[2] & 0x80 == 0
        || packet[3] & 0x0f != 0
        || u16::from_be_bytes([packet[4], packet[5]]) != 1
    {
        return None;
    }
    let answers = u16::from_be_bytes([packet[6], packet[7]]) as usize;
    let mut cursor = 12usize;
    let question_start = cursor;
    cursor = skip_dns_name(packet, cursor)?;
    if dns_wire_name_hash(&packet[question_start..cursor])? != expected_name
        || cursor.checked_add(4)? > packet.len()
        || u16::from_be_bytes([packet[cursor], packet[cursor + 1]]) != 1
        || u16::from_be_bytes([packet[cursor + 2], packet[cursor + 3]]) != 1
    {
        return None;
    }
    cursor += 4;
    for _ in 0..answers {
        cursor = skip_dns_name(packet, cursor)?;
        if cursor.checked_add(10)? > packet.len() {
            return None;
        }
        let kind = u16::from_be_bytes([packet[cursor], packet[cursor + 1]]);
        let class = u16::from_be_bytes([packet[cursor + 2], packet[cursor + 3]]);
        let bytes = u16::from_be_bytes([packet[cursor + 8], packet[cursor + 9]]) as usize;
        cursor += 10;
        let end = cursor.checked_add(bytes)?;
        if end > packet.len() {
            return None;
        }
        if kind == 1 && class == 1 && bytes == 4 {
            return Some(u32::from_be_bytes(packet[cursor..end].try_into().ok()?));
        }
        cursor = end;
    }
    None
}

fn skip_dns_name(packet: &[u8], mut cursor: usize) -> Option<usize> {
    loop {
        let length = *packet.get(cursor)?;
        if length & 0xc0 == 0xc0 {
            return cursor.checked_add(2).filter(|end| *end <= packet.len());
        }
        cursor += 1;
        if length == 0 {
            return Some(cursor);
        }
        if length > 63 {
            return None;
        }
        cursor = cursor.checked_add(length as usize)?;
        if cursor > packet.len() {
            return None;
        }
    }
}

fn dns_wire_name_hash(name: &[u8]) -> Option<u64> {
    let mut cursor = 0usize;
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    let mut separator = false;
    loop {
        let length = *name.get(cursor)? as usize;
        cursor += 1;
        if length == 0 {
            return (cursor == name.len()).then_some(hash);
        }
        if length > 63 || cursor.checked_add(length)? > name.len() {
            return None;
        }
        if separator {
            hash ^= b'.' as u64;
            hash = hash.wrapping_mul(0x100_0000_01b3);
        }
        for byte in &name[cursor..cursor + length] {
            hash ^= byte.to_ascii_lowercase() as u64;
            hash = hash.wrapping_mul(0x100_0000_01b3);
        }
        separator = true;
        cursor += length;
    }
}

fn dns_name_hash(name: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in name {
        hash ^= byte.to_ascii_lowercase() as u64;
        hash = hash.wrapping_mul(0x100_0000_01b3);
    }
    hash
}

fn free_outbound_connection(sessions: &[NetworkSession; NETWORK_SESSION_LIMIT]) -> Option<usize> {
    (0..OUTBOUND_CONNECTIONS).find(|candidate| {
        sessions.iter().all(|session| {
            !session
                .connections
                .iter()
                .any(|connection| matches!(connection, Connection::Tcp(index) if *index as usize == *candidate))
        })
    })
}

fn tcp_session_connection(session: &NetworkSession, local: usize) -> Option<usize> {
    let index = local.checked_sub(1)?;
    match *session.connections.get(index)? {
        Connection::Tcp(global) => Some(global as usize),
        _ => None,
    }
}

fn udp_session_connection(session: &NetworkSession, local: usize) -> Option<usize> {
    let index = local.checked_sub(1)?;
    match *session.connections.get(index)? {
        Connection::Udp(global) => Some(global as usize),
        _ => None,
    }
}

fn free_udp_connection(sessions: &[NetworkSession; NETWORK_SESSION_LIMIT]) -> Option<usize> {
    (0..UDP_CONNECTIONS).find(|candidate| sessions.iter().all(|session| !session.connections.iter().any(|connection| matches!(connection, Connection::Udp(index) if *index as usize == *candidate))))
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

struct DirectNetwork {
    rx: [u8; 1536],
    tx: [u8; 1536],
    received: usize,
}

impl DirectNetwork {
    const fn new() -> Self {
        Self {
            rx: [0; 1536],
            tx: [0; 1536],
            received: 0,
        }
    }
}

struct DirectRx<'a>(&'a [u8]);
struct DirectTx<'a>(&'a mut [u8]);

impl Device for DirectNetwork {
    type RxToken<'a>
        = DirectRx<'a>
    where
        Self: 'a;
    type TxToken<'a>
        = DirectTx<'a>
    where
        Self: 'a;

    fn receive(&mut self, _timestamp: Instant) -> Option<(Self::RxToken<'_>, Self::TxToken<'_>)> {
        self.received =
            match microsystem_user_rt::net_receive(boot_cap::NETWORK_DEVICE, &mut self.rx) {
                Ok(bytes) => bytes,
                Err(Status::Busy) => return None,
                Err(_) => return None,
            };
        (self.received > 0).then(|| (DirectRx(&self.rx[..self.received]), DirectTx(&mut self.tx)))
    }

    fn transmit(&mut self, _timestamp: Instant) -> Option<Self::TxToken<'_>> {
        Some(DirectTx(&mut self.tx))
    }

    fn capabilities(&self) -> DeviceCapabilities {
        let mut capabilities = DeviceCapabilities::default();
        capabilities.max_transmission_unit = 1500;
        capabilities.medium = Medium::Ethernet;
        capabilities
    }
}

impl RxToken for DirectRx<'_> {
    fn consume<R, F: FnOnce(&[u8]) -> R>(self, operation: F) -> R {
        operation(self.0)
    }
}

impl TxToken for DirectTx<'_> {
    fn consume<R, F: FnOnce(&mut [u8]) -> R>(self, length: usize, operation: F) -> R {
        let result = operation(&mut self.0[..length]);
        loop {
            match microsystem_user_rt::net_send(boot_cap::NETWORK_DEVICE, &self.0[..length]) {
                Ok(()) => break,
                Err(Status::Busy) => {
                    let _ = microsystem_user_rt::yield_now();
                }
                Err(_) => break,
            }
        }
        result
    }
}

fn now() -> Instant {
    Instant::from_micros((microsystem_user_rt::clock_now().unwrap_or(0) / 1000) as i64)
}

#[panic_handler]
fn panic(_info: &PanicInfo<'_>) -> ! {
    microsystem_user_rt::exit(1)
}
