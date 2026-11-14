//! Local IPv4 country lookup.
//!
//! The table is DB-IP Country Lite, packed as sorted ranges. Nothing is sent
//! to a geolocation service. IPv6 addresses are not in this table.

use std::net::IpAddr;
use std::sync::OnceLock;

const TABLE: &[u8] = include_bytes!("../assets/country4.bin");

struct Range {
    start: u32,
    end: u32,
    country: [u8; 2],
}

fn ranges() -> &'static [Range] {
    static RANGES: OnceLock<Vec<Range>> = OnceLock::new();
    RANGES.get_or_init(|| {
        if TABLE.len() < 8 || &TABLE[..4] != b"KC4\0" {
            return Vec::new();
        }
        let count = u32::from_le_bytes(TABLE[4..8].try_into().unwrap()) as usize;
        let mut out = Vec::with_capacity(count);
        let mut offset = 8;
        for _ in 0..count {
            if offset + 10 > TABLE.len() {
                break;
            }
            let start = u32::from_le_bytes(TABLE[offset..offset + 4].try_into().unwrap());
            let end = u32::from_le_bytes(TABLE[offset + 4..offset + 8].try_into().unwrap());
            let country = [TABLE[offset + 8], TABLE[offset + 9]];
            out.push(Range {
                start,
                end,
                country,
            });
            offset += 10;
        }
        out
    })
}

pub fn country(addr: IpAddr) -> Option<String> {
    if is_local(addr) {
        return None;
    }
    let IpAddr::V4(ip) = addr else {
        return None;
    };
    let value = u32::from(ip);
    let table = ranges();
    let mut lo = 0usize;
    let mut hi = table.len();
    while lo < hi {
        let mid = (lo + hi) / 2;
        if value < table[mid].start {
            hi = mid;
        } else if value > table[mid].end {
            lo = mid + 1;
        } else {
            let code = table[mid].country;
            return std::str::from_utf8(&code).ok().map(str::to_owned);
        }
    }
    None
}

fn is_local(addr: IpAddr) -> bool {
    match addr {
        IpAddr::V4(ip) => {
            let octets = ip.octets();
            ip.is_private()
                || ip.is_loopback()
                || ip.is_link_local()
                || ip.is_broadcast()
                || ip.is_unspecified()
                || ip.is_multicast()
                || octets[0] == 100 && (octets[1] & 0xc0) == 64
        }
        IpAddr::V6(ip) => {
            ip.is_loopback()
                || ip.is_unspecified()
                || ip.is_multicast()
                || is_unique_local(&ip)
                || is_link_local_v6(&ip)
        }
    }
}

fn is_unique_local(ip: &std::net::Ipv6Addr) -> bool {
    (ip.segments()[0] & 0xfe00) == 0xfc00
}

fn is_link_local_v6(ip: &std::net::Ipv6Addr) -> bool {
    (ip.segments()[0] & 0xffc0) == 0xfe80
}

pub fn country_of_socket(text: &str) -> Option<String> {
    let ip = if let Ok(socket) = text.parse::<std::net::SocketAddr>() {
        socket.ip()
    } else {
        text.parse::<IpAddr>().ok()?
    };
    country(ip)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    #[test]
    fn a_public_address_has_a_country_and_a_lan_address_does_not() {
        assert_eq!(
            country(IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8))).as_deref(),
            Some("US")
        );
        assert!(country(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 1))).is_none());
    }
}
