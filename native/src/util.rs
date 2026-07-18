//! Shared plumbing: interface enumeration (VPN-aware discovery) and locating
//! the shared asset tree (images/, audio/, branding/) from wherever the exe
//! runs.

use std::net::{Ipv4Addr, SocketAddrV4, UdpSocket};
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// All local IPv4 interface addresses, minus loopback. Binding discovery
/// sockets per-interface (rather than trusting the default route) is what
/// keeps broadcast discovery working when a VPN owns the default route.
pub fn local_ipv4s() -> Vec<Ipv4Addr> {
    let mut out = Vec::new();
    if let Ok(ifaces) = if_addrs::get_if_addrs() {
        for iface in ifaces {
            if let if_addrs::IfAddr::V4(v4) = iface.addr {
                if !v4.ip.is_loopback() {
                    out.push(v4.ip);
                }
            }
        }
    }
    out
}

/// The /24 subnet-directed broadcast for an interface address.
pub fn directed_broadcast(ip: Ipv4Addr) -> Ipv4Addr {
    let o = ip.octets();
    Ipv4Addr::new(o[0], o[1], o[2], 255)
}

/// A UDP socket bound to one interface with its outgoing multicast interface
/// pinned to it, so an M-SEARCH/scan actually leaves via that interface
/// rather than whatever the (possibly VPN-owned) routing table picks.
pub fn mcast_sender(ip: Ipv4Addr) -> std::io::Result<UdpSocket> {
    use socket2::{Domain, Protocol, Socket, Type};
    let s = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
    s.bind(&SocketAddrV4::new(ip, 0).into())?;
    s.set_multicast_if_v4(&ip)?;
    let udp: UdpSocket = s.into();
    udp.set_read_timeout(Some(Duration::from_millis(100)))?;
    Ok(udp)
}

/// Round-robin receive across several sockets until the deadline, feeding
/// each datagram to `on_pkt`. The sockets' own short read timeouts throttle
/// the loop; nothing is lost while another socket is being polled (the OS
/// buffers it).
pub fn recv_until(
    socks: &[UdpSocket],
    total: Duration,
    mut on_pkt: impl FnMut(&[u8], std::net::SocketAddr),
) {
    let deadline = Instant::now() + total;
    let mut buf = [0u8; 4096];
    while Instant::now() < deadline {
        for s in socks {
            if let Ok((n, addr)) = s.recv_from(&mut buf) {
                on_pkt(&buf[..n], addr);
            }
        }
        if socks.is_empty() {
            break;
        }
    }
}

/// Find the repo root holding the shared assets. The exe lives in
/// native/target/release/, so walk ancestors (of both the exe and the cwd)
/// until a directory containing audio/ + images/ appears.
pub fn assets_root() -> PathBuf {
    let mut candidates = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        for a in exe.ancestors().skip(1) {
            candidates.push(a.to_path_buf());
        }
    }
    if let Ok(cwd) = std::env::current_dir() {
        candidates.push(cwd.clone());
        for a in cwd.ancestors().skip(1) {
            candidates.push(a.to_path_buf());
        }
    }
    for c in &candidates {
        if c.join("audio").is_dir() && c.join("images").is_dir() {
            return c.clone();
        }
    }
    PathBuf::from(".")
}
