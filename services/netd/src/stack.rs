use alloc::{boxed::Box, vec, vec::Vec};
use microsystem_abi::{Status, boot_cap, network};
use microsystem_netd::config::{Configuration, InterfaceConfig};
use smoltcp::iface::{Config, Interface, SocketHandle, SocketSet, SocketStorage};
use smoltcp::phy::{Device, DeviceCapabilities, Medium, RxToken, TxToken};
use smoltcp::socket::{AnySocket, dhcpv4, tcp, udp};
use smoltcp::time::Instant;
use smoltcp::wire::{EthernetAddress, HardwareAddress, IpAddress, IpCidr, Ipv6Address, Ipv6Cidr};

pub const TCP_PER_INTERFACE: usize = 64;
pub const UDP_PER_INTERFACE: usize = 8;
pub const SSH_PER_INTERFACE: usize = 2;

#[derive(Clone, Copy, Debug)]
pub struct SocketRef {
    pub port: usize,
    handle: SocketHandle,
}

pub struct Port {
    pub interface: Interface,
    device: DirectNetwork,
    sockets: SocketSet<'static>,
    pub hardware: network::HardwareInfoV1,
    settings: InterfaceConfig,
    dhcp: Option<SocketHandle>,
    pub dns: SocketRef,
    pub dns_server: Option<IpAddress>,
    pub outbound: Vec<SocketRef>,
    pub datagrams: Vec<SocketRef>,
    pub ssh: Vec<SocketRef>,
    reported_address: Option<IpAddress>,
}

pub struct NetworkStack {
    pub ports: Vec<Port>,
    pub configuration: Configuration,
    next_tcp_port: u16,
}

impl NetworkStack {
    pub fn new(configuration: Configuration, seed: u64) -> Result<Self, Status> {
        let mut ports = Vec::new();
        ports
            .try_reserve_exact(network::MAX_INTERFACES)
            .map_err(|_| Status::NoMemory)?;
        for index in 0..network::MAX_INTERFACES {
            let hardware = microsystem_user_rt::network_interface_info(
                boot_cap::NETWORK_DEVICE,
                index as u32,
            )?;
            let mac = if hardware.status & network::HARDWARE_ACTIVE != 0 {
                hardware.mac
            } else {
                [2, 0, 0, 0, 0, index as u8 + 1]
            };
            let settings = configuration.for_mac(mac);
            let mut device = DirectNetwork::new(index);
            let interface =
                create_interface(&mut device, settings, seed.wrapping_add(index as u64));
            let storage = Box::leak(
                (0..TCP_PER_INTERFACE + UDP_PER_INTERFACE + SSH_PER_INTERFACE + 2)
                    .map(|_| SocketStorage::EMPTY)
                    .collect::<Vec<_>>()
                    .into_boxed_slice(),
            );
            let mut sockets = SocketSet::new(storage);
            let mut add_tcp = |size| SocketRef {
                port: index,
                handle: sockets.add(tcp::Socket::new(
                    tcp::SocketBuffer::new(buffer(size)),
                    tcp::SocketBuffer::new(buffer(size)),
                )),
            };
            let outbound = (0..TCP_PER_INTERFACE).map(|_| add_tcp(2048)).collect();
            let ssh: Vec<_> = (0..SSH_PER_INTERFACE).map(|_| add_tcp(16384)).collect();
            for handle in &ssh {
                sockets
                    .get_mut::<tcp::Socket>(handle.handle)
                    .listen(22)
                    .map_err(|_| Status::Io)?;
            }
            let datagrams = (0..UDP_PER_INTERFACE)
                .map(|_| SocketRef {
                    port: index,
                    handle: sockets.add(udp_socket(network::MAX_TRANSFER_BYTES)),
                })
                .collect();
            let mut dns_socket = udp_socket(512 * 8);
            dns_socket.bind(53053).map_err(|_| Status::Io)?;
            let dns = SocketRef {
                port: index,
                handle: sockets.add(dns_socket),
            };
            let dhcp = settings
                .ipv4
                .is_none()
                .then(|| sockets.add(dhcpv4::Socket::new()));
            ports.push(Port {
                interface,
                device,
                sockets,
                hardware,
                settings,
                dhcp,
                dns,
                dns_server: settings.dns,
                outbound,
                datagrams,
                ssh,
                reported_address: None,
            });
        }
        Ok(Self {
            ports,
            configuration,
            next_tcp_port: 49152 + (seed as u16 & 0x1fff),
        })
    }

    pub fn get<T: AnySocket<'static>>(&self, socket: SocketRef) -> &T {
        self.ports[socket.port].sockets.get(socket.handle)
    }
    pub fn get_mut<T: AnySocket<'static>>(&mut self, socket: SocketRef) -> &mut T {
        self.ports[socket.port].sockets.get_mut(socket.handle)
    }

    pub fn allocate_tcp_port(&mut self, interface: usize) -> u16 {
        for _ in 0..=TCP_PER_INTERFACE {
            let port = self.next_tcp_port;
            self.next_tcp_port = if port == u16::MAX { 49152 } else { port + 1 };
            if !self.ports[interface].outbound.iter().any(|socket| {
                self.get::<tcp::Socket>(*socket)
                    .local_endpoint()
                    .is_some_and(|endpoint| endpoint.port == port)
            }) {
                return port;
            }
        }
        unreachable!()
    }

    pub fn connect(
        &mut self,
        socket: SocketRef,
        address: IpAddress,
        port: u16,
        local_port: u16,
    ) -> Result<(), Status> {
        let interface = &mut self.ports[socket.port];
        interface
            .sockets
            .get_mut::<tcp::Socket>(socket.handle)
            .connect(interface.interface.context(), (address, port), local_port)
            .map_err(|_| Status::Io)
    }

    pub fn default_port(&self) -> Option<usize> {
        self.configuration
            .default
            .and_then(|mac| {
                self.ports
                    .iter()
                    .position(|port| port.hardware.mac == mac && port.available())
            })
            .or_else(|| self.ports.iter().position(|port| port.available()))
    }

    pub fn route(&self, destination: IpAddress) -> Option<usize> {
        let mut best = None;
        let mut prefix = 0;
        for (index, port) in self
            .ports
            .iter()
            .enumerate()
            .filter(|(_, port)| port.available())
        {
            for cidr in port.interface.ip_addrs() {
                if cidr.contains_addr(&destination)
                    && (best.is_none() || cidr.prefix_len() > prefix)
                {
                    best = Some(index);
                    prefix = cidr.prefix_len();
                }
            }
        }
        best.or_else(|| self.default_port().filter(|index| match destination {
            IpAddress::Ipv4(_) => self.ports[*index].interface.ipv4_addr().is_some(),
            IpAddress::Ipv6(_) => self.ports[*index].interface.ip_addrs().iter().any(|cidr| matches!(cidr, IpCidr::Ipv6(address) if !address.address().is_unicast_link_local())),
        }))
    }

    pub fn poll(&mut self) -> u8 {
        let mut reset = 0;
        for (index, port) in self.ports.iter_mut().enumerate() {
            let Ok(hardware) =
                microsystem_user_rt::network_interface_info(boot_cap::NETWORK_DEVICE, index as u32)
            else {
                continue;
            };
            let mac = if hardware.status & network::HARDWARE_ACTIVE != 0 {
                hardware.mac
            } else {
                port.settings.mac
            };
            let settings = self.configuration.for_mac(mac);
            if hardware.epoch != port.hardware.epoch
                || hardware.mac != port.hardware.mac
                || settings != port.settings
                || hardware.status & network::HARDWARE_LINK_UP
                    != port.hardware.status & network::HARDWARE_LINK_UP
            {
                port.reset(settings);
                reset |= 1 << index;
            }
            port.hardware = hardware;
            if !port.available() {
                continue;
            }
            port.interface
                .poll(now(), &mut port.device, &mut port.sockets);
            // abort() retains the remote tuple until poll emits its RST. Listen
            // only afterward, so key revocation also disconnects the client.
            for handle in &port.ssh {
                let socket = port.sockets.get_mut::<tcp::Socket>(handle.handle);
                if socket.state() == tcp::State::Closed && socket.remote_endpoint().is_none() {
                    let _ = socket.listen(22);
                }
            }
            if let Some(dhcp) = port.dhcp {
                let old = port.interface.ipv4_addr();
                match port.sockets.get_mut::<dhcpv4::Socket>(dhcp).poll() {
                    Some(dhcpv4::Event::Configured(lease)) => {
                        port.interface.update_ip_addrs(|addresses| {
                            addresses.retain(|cidr| matches!(cidr, IpCidr::Ipv6(_)));
                            let _ = addresses.push(IpCidr::Ipv4(lease.address));
                        });
                        port.interface.routes_mut().remove_default_ipv4_route();
                        if let Some(router) = lease.router {
                            let _ = port.interface.routes_mut().add_default_ipv4_route(router);
                        }
                        port.dns_server = settings
                            .dns
                            .or_else(|| lease.dns_servers.first().copied().map(IpAddress::Ipv4));
                    }
                    Some(dhcpv4::Event::Deconfigured) => {
                        port.interface.update_ip_addrs(|addresses| {
                            addresses.retain(|cidr| matches!(cidr, IpCidr::Ipv6(_)))
                        });
                        port.interface.routes_mut().remove_default_ipv4_route();
                        port.dns_server = settings.dns;
                    }
                    None => {}
                }
                if old.is_some() && old != port.interface.ipv4_addr() {
                    port.abort_connections();
                    reset |= 1 << index;
                }
            }
            let address = port.interface.ipv4_addr().map(IpAddress::Ipv4);
            if address != port.reported_address {
                if let Some(address) = address {
                    let _ = microsystem_user_rt::debug_write(alloc::format!("[net] netd ready ipv4={address} outbound=true raw-device-isolated=true tcp-owner=true dns=true tcp=64 udp=8 interface={index}\n").as_bytes());
                }
                port.reported_address = address;
            }
        }
        reset
    }
}

impl Port {
    pub fn available(&self) -> bool {
        self.hardware.status & (network::HARDWARE_ACTIVE | network::HARDWARE_LINK_UP)
            == network::HARDWARE_ACTIVE | network::HARDWARE_LINK_UP
    }
    fn abort_connections(&mut self) {
        for handle in self.outbound.iter().chain(self.ssh.iter()) {
            self.sockets.get_mut::<tcp::Socket>(handle.handle).abort();
        }
        for handle in &self.ssh {
            let _ = self
                .sockets
                .get_mut::<tcp::Socket>(handle.handle)
                .listen(22);
        }
        for handle in &self.datagrams {
            self.sockets.get_mut::<udp::Socket>(handle.handle).close();
        }
        let dns = self.sockets.get_mut::<udp::Socket>(self.dns.handle);
        dns.close();
        let _ = dns.bind(53053);
    }
    fn reset(&mut self, settings: InterfaceConfig) {
        self.abort_connections();
        if let Some(handle) = self.dhcp.take() {
            self.sockets.remove(handle);
        }
        self.interface = create_interface(
            &mut self.device,
            settings,
            microsystem_user_rt::clock_now().unwrap_or(0),
        );
        self.dhcp = settings
            .ipv4
            .is_none()
            .then(|| self.sockets.add(dhcpv4::Socket::new()));
        self.settings = settings;
        self.dns_server = settings.dns;
        self.reported_address = None;
    }
}

fn create_interface(device: &mut DirectNetwork, settings: InterfaceConfig, seed: u64) -> Interface {
    let mac = EthernetAddress(settings.mac);
    let mut config = Config::new(HardwareAddress::Ethernet(mac));
    config.random_seed = seed;
    config.slaac = settings.auto6;
    let mut interface = Interface::new(config, device, now());
    interface.update_ip_addrs(|addresses| {
        if let Some(address) = settings.ipv4 {
            let _ = addresses.push(IpCidr::Ipv4(address));
        }
        if settings.auto6 || settings.ipv6.is_some() {
            let mut bytes = [0; 16];
            bytes[0] = 0xfe;
            bytes[1] = 0x80;
            bytes[8..].copy_from_slice(&mac.as_eui_64().unwrap());
            let _ = addresses.push(IpCidr::Ipv6(Ipv6Cidr::new(
                Ipv6Address::from_octets(bytes),
                64,
            )));
        }
        if let Some(address) = settings.ipv6 {
            let _ = addresses.push(IpCidr::Ipv6(address));
        }
    });
    if let Some(router) = settings.gateway4 {
        let _ = interface.routes_mut().add_default_ipv4_route(router);
    }
    if let Some(router) = settings.gateway6 {
        let _ = interface.routes_mut().add_default_ipv6_route(router);
    }
    interface
}

fn buffer(size: usize) -> &'static mut [u8] {
    Box::leak(vec![0; size].into_boxed_slice())
}
fn udp_socket(size: usize) -> udp::Socket<'static> {
    udp::Socket::new(
        udp::PacketBuffer::new(
            Box::leak(vec![udp::PacketMetadata::EMPTY; 8].into_boxed_slice()),
            buffer(size),
        ),
        udp::PacketBuffer::new(
            Box::leak(vec![udp::PacketMetadata::EMPTY; 8].into_boxed_slice()),
            buffer(size),
        ),
    )
}

struct DirectNetwork {
    index: usize,
    rx: [u8; 1536],
    tx: [u8; 1536],
}
impl DirectNetwork {
    fn new(index: usize) -> Self {
        Self {
            index,
            rx: [0; 1536],
            tx: [0; 1536],
        }
    }
}
struct DirectRx<'a>(&'a [u8]);
struct DirectTx<'a> {
    index: usize,
    bytes: &'a mut [u8],
}
impl Device for DirectNetwork {
    type RxToken<'a>
        = DirectRx<'a>
    where
        Self: 'a;
    type TxToken<'a>
        = DirectTx<'a>
    where
        Self: 'a;
    fn receive(&mut self, _: Instant) -> Option<(Self::RxToken<'_>, Self::TxToken<'_>)> {
        if !tx_ready(self.index) {
            return None;
        }
        let length = microsystem_user_rt::net_receive_on(
            boot_cap::NETWORK_DEVICE,
            self.index as u32,
            &mut self.rx,
        )
        .ok()?;
        (length > 0).then(|| {
            (
                DirectRx(&self.rx[..length]),
                DirectTx {
                    index: self.index,
                    bytes: &mut self.tx,
                },
            )
        })
    }
    fn transmit(&mut self, _: Instant) -> Option<Self::TxToken<'_>> {
        tx_ready(self.index).then(|| DirectTx {
            index: self.index,
            bytes: &mut self.tx,
        })
    }
    fn capabilities(&self) -> DeviceCapabilities {
        let mut caps = DeviceCapabilities::default();
        caps.max_transmission_unit = 1514;
        caps.medium = Medium::Ethernet;
        caps
    }
}
fn tx_ready(index: usize) -> bool {
    microsystem_user_rt::network_interface_info(boot_cap::NETWORK_DEVICE, index as u32)
        .is_ok_and(|info| info.status & network::HARDWARE_TX_READY != 0)
}
impl RxToken for DirectRx<'_> {
    fn consume<R, F: FnOnce(&[u8]) -> R>(self, operation: F) -> R {
        operation(self.0)
    }
}
impl TxToken for DirectTx<'_> {
    fn consume<R, F: FnOnce(&mut [u8]) -> R>(self, length: usize, operation: F) -> R {
        let result = operation(&mut self.bytes[..length]);
        let _ = microsystem_user_rt::net_send_on(
            boot_cap::NETWORK_DEVICE,
            self.index as u32,
            &self.bytes[..length],
        );
        result
    }
}
pub fn now() -> Instant {
    Instant::from_micros((microsystem_user_rt::clock_now().unwrap_or(0) / 1000) as i64)
}
