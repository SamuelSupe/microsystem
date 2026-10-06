use crate::{config_runtime, stack::NetworkStack};
use alloc::string::String;
use core::fmt::Write;
use microsystem_abi::{CapHandle, Message, Rights, Status, network};
use microsystem_netd::config::Mac;

const COMMAND_VA: u64 = 0x005a_0000;

pub fn handle(request: &Message, reply: &mut Message, stack: &mut NetworkStack) -> Status {
    let region = request.caps[0];
    let result = mapped(request, reply, stack);
    if region != CapHandle::INVALID {
        let _ = microsystem_user_rt::frame_unmap(region, COMMAND_VA);
        let _ = microsystem_user_rt::cap_delete(region);
    }
    result.unwrap_or_else(|status| status)
}

fn mapped(
    request: &Message,
    reply: &mut Message,
    stack: &mut NetworkStack,
) -> Result<Status, Status> {
    if !matches!(microsystem_user_rt::ipc_peer()?, 5 | 8 | 11) {
        return Err(Status::AccessDenied);
    }
    let length = request.words[0] as usize;
    if length > 512 || request.caps[0] == CapHandle::INVALID {
        return Err(Status::Invalid);
    }
    microsystem_user_rt::frame_map(
        request.caps[0],
        COMMAND_VA,
        Rights(Rights::READ.0 | Rights::WRITE.0),
    )?;
    let frame = unsafe { core::slice::from_raw_parts_mut(COMMAND_VA as *mut u8, 4096) };
    let mut command_bytes = [0; 512];
    command_bytes[..length].copy_from_slice(&frame[..length]);
    let command = core::str::from_utf8(&command_bytes[..length])
        .map_err(|_| Status::Invalid)?
        .trim();
    let mut output = String::new();
    output.try_reserve(4096).map_err(|_| Status::NoMemory)?;
    if command.is_empty() || command == "status" {
        for (index, port) in stack
            .ports
            .iter()
            .enumerate()
            .filter(|(_, port)| port.hardware.status & network::HARDWARE_ACTIVE != 0)
        {
            let _ = write!(
                output,
                "interface={index} mac={} link={} epoch={} ipv4=",
                Mac(port.hardware.mac),
                port.hardware.status & network::HARDWARE_LINK_UP != 0,
                port.hardware.epoch
            );
            if let Some(address) = port.interface.ipv4_addr() {
                let _ = write!(output, "{address}");
            } else {
                output.push('-');
            }
            output.push_str(" ipv6=");
            for cidr in port
                .interface
                .ip_addrs()
                .iter()
                .filter(|cidr| matches!(cidr, smoltcp::wire::IpCidr::Ipv6(_)))
            {
                let _ = write!(output, "{cidr},");
            }
            if let Some(server) = port.dns_server {
                let _ = write!(output, " dns={server}");
            }
            output.push('\n');
        }
        if output.is_empty() {
            output.push_str("no active network interface\n");
        }
    } else if command == "config" {
        output = stack.configuration.encode().map_err(|_| Status::NoMemory)?;
    } else {
        let snapshot =
            unsafe { microsystem_identity::read_shared(microsystem_abi::identity::SNAPSHOT_VA) }?;
        let peer = microsystem_user_rt::ipc_peer()? as u32;
        if (peer == 11 && request.words[3] == 0)
            || !snapshot
                .actor(peer, request.words[3], microsystem_user_rt::clock_now()?)?
                .administrator()
        {
            return Err(Status::AccessDenied);
        }
        let next = microsystem_netd::config::apply_command(&stack.configuration, command)
            .map_err(|_| Status::Invalid)?;
        config_runtime::save(&next)?;
        stack.configuration = next;
        output.push_str("network configuration saved; affected connections will close\n");
    }
    if output.len() > frame.len() {
        return Err(Status::NoSpace);
    }
    frame[..output.len()].copy_from_slice(output.as_bytes());
    reply.words[0] = output.len() as u64;
    Ok(Status::Ok)
}
