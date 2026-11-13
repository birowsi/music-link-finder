//! Check the authoritative zone before recursive resolvers can cache a fresh
//! Quick Tunnel hostname as nonexistent. Never changes the computer's DNS.
use std::{
    io::Read,
    net::{SocketAddr, ToSocketAddrs, UdpSocket},
    thread,
    time::{Duration, Instant},
};

fn published(packet: &[u8], id: u16) -> Option<bool> {
    if packet.len() < 12 || u16::from_be_bytes([packet[0], packet[1]]) != id {
        return None;
    }
    let flags = u16::from_be_bytes([packet[2], packet[3]]);
    if flags & 0x8400 != 0x8400 {
        return None;
    } // Response, authoritative.
    if flags & 15 == 3 {
        return Some(false);
    }
    if flags & 15 != 0 {
        return None;
    }
    let questions = u16::from_be_bytes([packet[4], packet[5]]);
    let answers = u16::from_be_bytes([packet[6], packet[7]]);
    if questions > 4 || answers > 32 {
        return None;
    }
    let mut offset = 12;
    fn skip_name(packet: &[u8], offset: &mut usize) -> Option<()> {
        for _ in 0..128 {
            let length = *packet.get(*offset)?;
            *offset += 1;
            if length == 0 {
                return Some(());
            }
            if length & 0xc0 == 0xc0 {
                packet.get(*offset)?;
                *offset += 1;
                return Some(());
            }
            if length > 63 {
                return None;
            }
            *offset = offset.checked_add(length as usize)?;
            if *offset > packet.len() {
                return None;
            }
        }
        None
    }
    for _ in 0..questions {
        skip_name(packet, &mut offset)?;
        offset += 4;
        if offset > packet.len() {
            return None;
        }
    }
    for _ in 0..answers {
        skip_name(packet, &mut offset)?;
        let fixed = packet.get(offset..offset + 10)?;
        let kind = u16::from_be_bytes([fixed[0], fixed[1]]);
        let class = u16::from_be_bytes([fixed[2], fixed[3]]);
        let length = u16::from_be_bytes([fixed[8], fixed[9]]) as usize;
        offset += 10;
        packet.get(offset..offset + length)?;
        if kind == 1 && class == 1 && length == 4 {
            return Some(true);
        }
        offset += length;
    }
    Some(false)
}

fn authorities() -> Option<Vec<SocketAddr>> {
    // Query an existing zone, never the newly-created hostname, on a recursive
    // resolver. The NS names are retrieved rather than hard-coded.
    let client = reqwest::blocking::Client::builder()
        .connect_timeout(Duration::from_secs(2))
        .timeout(Duration::from_secs(3))
        .build()
        .ok()?;
    let response = client
        .get("https://cloudflare-dns.com/dns-query")
        .query(&[("name", "trycloudflare.com"), ("type", "NS")])
        .header("accept", "application/dns-json")
        .send()
        .ok()?
        .error_for_status()
        .ok()?;
    let mut body = String::new();
    response.take(65537).read_to_string(&mut body).ok()?;
    let dns: serde_json::Value = serde_json::from_str(&body).ok()?;
    let mut addresses = Vec::new();
    for answer in dns["Answer"]
        .as_array()?
        .iter()
        .filter(|v| v["type"] == 2)
        .take(2)
    {
        let name = answer["data"].as_str()?.trim_end_matches('.');
        if !name.ends_with(".ns.cloudflare.com") {
            continue;
        }
        for address in (name, 53)
            .to_socket_addrs()
            .ok()?
            .filter(|a| a.is_ipv4())
            .take(2)
        {
            if let std::net::IpAddr::V4(ip) = address.ip()
                && !ip.is_private()
                && !ip.is_loopback()
                && !ip.is_link_local()
            {
                addresses.push(address);
            }
        }
    }
    if addresses.is_empty() {
        None
    } else {
        Some(addresses)
    }
}

pub(super) fn wait_for_publication(
    url: &str,
    cancelled: &impl Fn() -> bool,
    progress: &impl Fn(&str),
) -> Result<(), String> {
    let parsed = reqwest::Url::parse(url).map_err(|e| e.to_string())?;
    let host = parsed.host_str().ok_or("중계 호스트가 없습니다.")?;
    if !host.ends_with(".trycloudflare.com") {
        return Ok(());
    }
    let Some(addresses) = authorities() else {
        return Ok(());
    };
    let Ok(socket) = UdpSocket::bind("0.0.0.0:0") else {
        return Ok(());
    };
    let _ = socket.set_read_timeout(Some(Duration::from_millis(800)));
    let id = u16::from_str_radix(&super::token()?[..4], 16).map_err(|e| e.to_string())?;
    let mut query = vec![0u8; 12];
    query[..2].copy_from_slice(&id.to_be_bytes());
    query[5] = 1; // One A question, recursion disabled.
    for label in host.split('.') {
        if label.is_empty() || label.len() > 63 {
            return Err("중계 호스트 형식 오류".into());
        }
        query.push(label.len() as u8);
        query.extend_from_slice(label.as_bytes());
    }
    query.extend_from_slice(&[0, 0, 1, 0, 1]);
    let started = Instant::now();
    loop {
        if cancelled() {
            return Err(super::CANCELLED.into());
        }
        for address in &addresses {
            let _ = socket.send_to(&query, address);
        }
        let mut received = false;
        let mut packet = [0u8; 2048];
        while let Ok((length, peer)) = socket.recv_from(&mut packet) {
            if !addresses.contains(&peer) {
                continue;
            }
            match published(&packet[..length], id) {
                Some(true) => return Ok(()),
                Some(false) => received = true,
                None => {}
            }
        }
        // UDP may be unavailable on a user's network. Continue with normal
        // DNS/HTTPS validation instead of requiring a particular DNS transport.
        if !received {
            return Ok(());
        }
        if started.elapsed() > Duration::from_secs(30) {
            return Err(
                "임시 중계 주소의 DNS 등록이 지연되고 있습니다. 중계 중지 후 다시 준비하세요."
                    .into(),
            );
        }
        progress(&format!(
            "임시 중계 주소 DNS 등록 확인 중 - {}초 경과…",
            started.elapsed().as_secs()
        ));
        thread::sleep(Duration::from_millis(200));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verifies_authority_transaction_and_complete_address_records() {
        let mut packet = vec![0x12, 0x34, 0x84, 0, 0, 0, 0, 1, 0, 0, 0, 0];
        packet.extend_from_slice(&[0, 0, 1, 0, 1, 0, 0, 0, 60, 0, 4, 104, 16, 1, 2]);
        assert_eq!(published(&packet, 0x1234), Some(true));
        assert_eq!(published(&packet, 0x1235), None);
        let mut truncated = packet.clone();
        truncated.pop();
        assert_eq!(published(&truncated, 0x1234), None);
        packet[3] = 3;
        assert_eq!(published(&packet, 0x1234), Some(false));
        packet[2] = 0x80;
        assert_eq!(published(&packet, 0x1234), None);
    }
}
