use alloc::string::String;
use alloc::vec::Vec;
use core::fmt::Write;
use smoltcp::wire::{IpAddress, Ipv4Address, Ipv4Cidr, Ipv6Address, Ipv6Cidr};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InterfaceConfig {
    pub mac: [u8; 6],
    pub ipv4: Option<Ipv4Cidr>,
    pub gateway4: Option<Ipv4Address>,
    pub dns: Option<IpAddress>,
    pub ipv6: Option<Ipv6Cidr>,
    pub gateway6: Option<Ipv6Address>,
    pub auto6: bool,
}

impl InterfaceConfig {
    pub const fn automatic(mac: [u8; 6]) -> Self {
        Self {
            mac,
            ipv4: None,
            gateway4: None,
            dns: None,
            ipv6: None,
            gateway6: None,
            auto6: true,
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Configuration {
    pub default: Option<[u8; 6]>,
    pub interfaces: Vec<InterfaceConfig>,
}

impl Configuration {
    pub fn for_mac(&self, mac: [u8; 6]) -> InterfaceConfig {
        self.interfaces
            .iter()
            .find(|config| config.mac == mac)
            .copied()
            .unwrap_or(InterfaceConfig::automatic(mac))
    }

    pub fn set(&mut self, config: InterfaceConfig) -> Result<(), ()> {
        validate(config)?;
        if let Some(existing) = self
            .interfaces
            .iter_mut()
            .find(|existing| existing.mac == config.mac)
        {
            *existing = config;
            return Ok(());
        }
        if self.interfaces.len() == microsystem_abi::network::MAX_INTERFACES {
            return Err(());
        }
        self.interfaces.try_reserve(1).map_err(|_| ())?;
        self.interfaces.push(config);
        Ok(())
    }

    pub fn parse(text: &str) -> Result<Self, ()> {
        if text.len() > 4096 {
            return Err(());
        }
        let mut rows = text.lines();
        if rows.next() != Some("MICRONET1") {
            return Err(());
        }
        let mut config = Self::default();
        for row in rows.filter(|row| !row.is_empty()) {
            let fields: Vec<&str> = row.split_ascii_whitespace().collect();
            if fields.len() == 2 && fields[0] == "default" {
                if config.default.is_some() {
                    return Err(());
                }
                config.default = Some(parse_mac(fields[1])?);
                continue;
            }
            if fields.len() != 7 || fields[0] != "iface" {
                return Err(());
            }
            let mac = parse_mac(fields[1])?;
            if config.interfaces.iter().any(|entry| entry.mac == mac) {
                return Err(());
            }
            config.set(InterfaceConfig {
                mac,
                ipv4: if fields[2] == "dhcp" {
                    None
                } else {
                    Some(fields[2].parse().map_err(|_| ())?)
                },
                gateway4: optional(fields[3])?,
                dns: optional(fields[4])?,
                ipv6: if matches!(fields[5], "auto" | "off") {
                    None
                } else {
                    Some(fields[5].parse().map_err(|_| ())?)
                },
                gateway6: optional(fields[6])?,
                auto6: fields[5] == "auto",
            })?;
        }
        Ok(config)
    }

    pub fn encode(&self) -> Result<String, ()> {
        let mut text = String::new();
        text.try_reserve(4096).map_err(|_| ())?;
        text.push_str("MICRONET1\n");
        if let Some(mac) = self.default {
            writeln!(&mut text, "default {}", Mac(mac)).map_err(|_| ())?;
        }
        for entry in &self.interfaces {
            write!(&mut text, "iface {} ", Mac(entry.mac)).map_err(|_| ())?;
            if let Some(address) = entry.ipv4 {
                write!(&mut text, "{address}").map_err(|_| ())?;
            } else {
                text.push_str("dhcp");
            }
            write!(
                &mut text,
                " {} {} ",
                Optional(entry.gateway4),
                Optional(entry.dns)
            )
            .map_err(|_| ())?;
            if let Some(address) = entry.ipv6 {
                write!(&mut text, "{address}").map_err(|_| ())?;
            } else {
                text.push_str(if entry.auto6 { "auto" } else { "off" });
            }
            writeln!(&mut text, " {}", Optional(entry.gateway6)).map_err(|_| ())?;
        }
        Ok(text)
    }
}

fn validate(config: InterfaceConfig) -> Result<(), ()> {
    if config.mac[0] & 1 != 0 || config.mac == [0; 6] {
        return Err(());
    }
    if config.ipv4.is_none() && config.gateway4.is_some() {
        return Err(());
    }
    if config.ipv6.is_none() && config.gateway6.is_some() {
        return Err(());
    }
    if let Some(address) = config.ipv4 {
        if address.address().is_unspecified()
            || address.address().is_multicast()
            || address.prefix_len() == 0
        {
            return Err(());
        }
        if config.gateway4.is_some_and(|gateway| {
            !address.contains_addr(&gateway) || gateway.is_unspecified() || gateway.is_multicast()
        }) {
            return Err(());
        }
    }
    if let Some(address) = config.ipv6 {
        if address.address().is_unspecified() || address.address().is_multicast() {
            return Err(());
        }
    }
    if config
        .gateway6
        .is_some_and(|address| address.is_unspecified() || address.is_multicast())
    {
        return Err(());
    }
    if config
        .dns
        .is_some_and(|address| address.is_unspecified() || address.is_multicast())
    {
        return Err(());
    }
    Ok(())
}

pub fn parse_mac(text: &str) -> Result<[u8; 6], ()> {
    let mut mac = [0; 6];
    let mut fields = text.split(':');
    for byte in &mut mac {
        let field = fields.next().ok_or(())?;
        if field.len() != 2 {
            return Err(());
        }
        *byte = u8::from_str_radix(field, 16).map_err(|_| ())?;
    }
    if fields.next().is_some() || mac == [0; 6] || mac[0] & 1 != 0 {
        return Err(());
    }
    Ok(mac)
}

pub fn apply_command(current: &Configuration, command: &str) -> Result<Configuration, ()> {
    let fields: Vec<&str> = command.split_ascii_whitespace().collect();
    let mut next = current.clone();
    match fields.as_slice() {
        ["reset"] => next = Configuration::default(),
        ["default", mac] => next.default = Some(parse_mac(mac)?),
        ["dhcp", mac] => next.set(InterfaceConfig::automatic(parse_mac(mac)?))?,
        ["static", mac, address, gateway, dns] => {
            let parsed = Configuration::parse(&alloc::format!(
                "MICRONET1\niface {mac} {address} {gateway} {dns} auto -\n"
            ))?;
            next.set(parsed.interfaces[0])?;
        }
        ["static", mac, address, gateway, dns, address6, gateway6] => {
            let parsed = Configuration::parse(&alloc::format!(
                "MICRONET1\niface {mac} {address} {gateway} {dns} {address6} {gateway6}\n"
            ))?;
            next.set(parsed.interfaces[0])?;
        }
        _ => return Err(()),
    }
    Ok(next)
}

fn optional<T: core::str::FromStr>(text: &str) -> Result<Option<T>, ()> {
    if text == "-" {
        Ok(None)
    } else {
        text.parse().map(Some).map_err(|_| ())
    }
}

pub struct Mac(pub [u8; 6]);
impl core::fmt::Display for Mac {
    fn fmt(&self, output: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let mac = self.0;
        write!(
            output,
            "{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
            mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]
        )
    }
}
struct Optional<T>(Option<T>);
impl<T: core::fmt::Display> core::fmt::Display for Optional<T> {
    fn fmt(&self, output: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match &self.0 {
            Some(value) => value.fmt(output),
            None => output.write_str("-"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn config_roundtrips_static_dual_stack_and_automatic_interfaces() {
        let text = "MICRONET1\ndefault 52:54:00:12:34:56\niface 52:54:00:12:34:56 10.0.2.20/24 10.0.2.2 10.0.2.3 fec0::20/64 fe80::2\niface 52:54:00:12:34:57 dhcp - - auto -\n";
        let config = Configuration::parse(text).unwrap();
        assert_eq!(
            Configuration::parse(&config.encode().unwrap()).unwrap(),
            config
        );
        assert_eq!(
            config.interfaces[1],
            InterfaceConfig::automatic([0x52, 0x54, 0, 0x12, 0x34, 0x57])
        );
    }
    #[test]
    fn invalid_routes_duplicates_and_multicast_mac_leave_config_rejected() {
        for row in [
            "iface 01:00:00:00:00:01 dhcp - - auto -",
            "iface 52:54:00:12:34:56 10.0.2.20/24 10.0.3.2 - auto -",
            "iface 52:54:00:12:34:56 dhcp 10.0.2.2 - auto -",
        ] {
            assert!(Configuration::parse(&alloc::format!("MICRONET1\n{row}\n")).is_err());
        }
        assert!(Configuration::parse("MICRONET1\niface 52:54:00:12:34:56 dhcp - - auto -\niface 52:54:00:12:34:56 dhcp - - auto -\n").is_err());
    }
    #[test]
    fn configuration_commands_preserve_other_interfaces_and_reject_invalid_dns() {
        let first = apply_command(
            &Configuration::default(),
            "static 52:54:00:12:34:56 10.0.2.20/24 10.0.2.2 10.0.2.3 fec0::20/64 fe80::2",
        )
        .unwrap();
        let both = apply_command(&first, "dhcp 52:54:00:12:34:57").unwrap();
        assert_eq!(both.interfaces.len(), 2);
        assert_eq!(both.interfaces[0], first.interfaces[0]);
        assert!(apply_command(&both, "static 52:54:00:12:34:56 10.0.2.20/24 10.0.2.2 ::").is_err());
        assert_eq!(
            apply_command(&both, "reset").unwrap(),
            Configuration::default()
        );
    }
}
