use smoltcp::wire::{IpAddress, Ipv4Address, Ipv6Address};
const DNS_PACKET_BYTES: usize = 512;

pub fn encode_dns_query(
    id: u16,
    host: &str,
    kind: u16,
    output: &mut [u8; DNS_PACKET_BYTES],
) -> Option<usize> {
    if !matches!(kind, 1 | 28) || host.is_empty() || host.len() > 253 {
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
    output[cursor..cursor + 2].copy_from_slice(&kind.to_be_bytes());
    output[cursor + 2..cursor + 4].copy_from_slice(&1u16.to_be_bytes());
    Some(cursor + 4)
}

pub fn parse_dns_response(
    packet: &[u8],
    id: u16,
    expected_name: u64,
    query_type: u16,
) -> Option<IpAddress> {
    if packet.len() < 12
        || u16::from_be_bytes([packet[0], packet[1]]) != id
        || packet[2] & 0x80 == 0
        || packet[2] & 0x02 != 0
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
        || u16::from_be_bytes([packet[cursor], packet[cursor + 1]]) != query_type
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
        if kind == query_type && class == 1 {
            if kind == 1 && bytes == 4 {
                return Some(IpAddress::Ipv4(Ipv4Address::from_octets(
                    packet[cursor..end].try_into().ok()?,
                )));
            }
            if kind == 28 && bytes == 16 {
                return Some(IpAddress::Ipv6(Ipv6Address::from_octets(
                    packet[cursor..end].try_into().ok()?,
                )));
            }
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

pub fn dns_name_hash(name: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in name {
        hash ^= byte.to_ascii_lowercase() as u64;
        hash = hash.wrapping_mul(0x100_0000_01b3);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ipv6_answers_are_bound_to_the_question_and_reject_truncation() {
        let mut packet = [0; DNS_PACKET_BYTES];
        let question = encode_dns_query(123, "ipv6.maturity.test", 28, &mut packet).unwrap();
        packet[2] = 0x81;
        packet[3] = 0x80;
        packet[7] = 1;
        packet[question..question + 12]
            .copy_from_slice(&[0xc0, 12, 0, 28, 0, 1, 0, 0, 0, 30, 0, 16]);
        let address = "fec0::2".parse::<Ipv6Address>().unwrap();
        packet[question + 12..question + 28].copy_from_slice(&address.octets());
        let reply = &packet[..question + 28];
        assert_eq!(
            parse_dns_response(reply, 123, dns_name_hash(b"IPV6.MATURITY.TEST"), 28),
            Some(IpAddress::Ipv6(address))
        );
        for length in 0..reply.len() {
            assert_eq!(
                parse_dns_response(
                    &reply[..length],
                    123,
                    dns_name_hash(b"ipv6.maturity.test"),
                    28
                ),
                None
            );
        }
        assert_eq!(
            parse_dns_response(reply, 124, dns_name_hash(b"ipv6.maturity.test"), 28),
            None
        );
        assert_eq!(
            parse_dns_response(reply, 123, dns_name_hash(b"other.test"), 28),
            None
        );
        assert_eq!(
            parse_dns_response(reply, 123, dns_name_hash(b"ipv6.maturity.test"), 1),
            None
        );
        packet[2] |= 2;
        assert_eq!(
            parse_dns_response(
                &packet[..question + 28],
                123,
                dns_name_hash(b"ipv6.maturity.test"),
                28
            ),
            None
        );
    }
}
