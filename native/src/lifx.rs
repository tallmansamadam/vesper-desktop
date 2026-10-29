//! LIFX LAN protocol (UDP 56700), ported from lifx_lan.py.
//!
//! One-shot commands (power/colour) are acknowledged and retried so every
//! bulb actually receives them; `stream_color` stays single-send for effects
//! and screen-sync. Discovery binds per-interface and sends subnet-directed
//! broadcasts so it survives a VPN owning the default route.

use std::collections::HashMap;
use std::net::UdpSocket;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::light::{Light, Vendor};
use crate::util::{directed_broadcast, local_ipv4s, recv_until};

const PORT: u16 = 56700;
const MSG_GET_SERVICE: u16 = 2;
const MSG_STATE_SERVICE: u16 = 3;
const MSG_GET: u16 = 101;
const MSG_STATE: u16 = 107;
const MSG_SET_COLOR: u16 = 102;
const MSG_SET_POWER: u16 = 117;
const MSG_ACK: u16 = 45;
const KELVIN_MIN: u16 = 1500;
const KELVIN_MAX: u16 = 9000;

fn make_packet(
    msg_type: u16,
    payload: &[u8],
    target: u64,
    tagged: bool,
    res_required: bool,
    ack_required: bool,
    sequence: u8,
) -> Vec<u8> {
    let size = (36 + payload.len()) as u16;
    let mut flags: u16 = 1024 | (1 << 12); // protocol + addressable
    if tagged {
        flags |= 1 << 13;
    }
    let mut p = Vec::with_capacity(36 + payload.len());
    p.extend_from_slice(&size.to_le_bytes());
    p.extend_from_slice(&flags.to_le_bytes());
    p.extend_from_slice(&2u32.to_le_bytes()); // source
    p.extend_from_slice(&target.to_le_bytes());
    p.extend_from_slice(&[0u8; 6]);
    p.push((res_required as u8) | ((ack_required as u8) << 1));
    p.push(sequence);
    p.extend_from_slice(&0u64.to_le_bytes());
    p.extend_from_slice(&msg_type.to_le_bytes());
    p.extend_from_slice(&0u16.to_le_bytes());
    p.extend_from_slice(payload);
    p
}

/// (target, sequence, msg_type)
fn parse_header(d: &[u8]) -> Option<(u64, u8, u16)> {
    if d.len() < 36 {
        return None;
    }
    let target = u64::from_le_bytes(d[8..16].try_into().ok()?);
    let seq = d[23];
    let msg_type = u16::from_le_bytes(d[32..34].try_into().ok()?);
    Some((target, seq, msg_type))
}

fn serial_hex(target: u64) -> String {
    let b = target.to_le_bytes();
    b[..6].iter().map(|x| format!("{x:02x}")).collect()
}

pub struct LifxLan {
    sock: UdpSocket,
    seq: AtomicU8,
    rx: Mutex<()>, // serialise reply/ack waits on the shared socket
}

impl LifxLan {
    pub fn new() -> std::io::Result<LifxLan> {
        let sock = UdpSocket::bind("0.0.0.0:0")?;
        sock.set_broadcast(true)?;
        sock.set_read_timeout(Some(Duration::from_millis(600)))?;
        Ok(LifxLan {
            sock,
            seq: AtomicU8::new(0),
            rx: Mutex::new(()),
        })
    }

    fn next_seq(&self) -> u8 {
        self.seq.fetch_add(1, Ordering::Relaxed).wrapping_add(1)
    }

    // -- discovery ---------------------------------------------------------

    pub fn discover(&self) -> Vec<Arc<Light>> {
        let wait = Duration::from_millis(1200);
        let rounds = 3;

        // One broadcast socket per interface + its destinations.
        let mut socks: Vec<UdpSocket> = Vec::new();
        let mut dests: Vec<Vec<String>> = Vec::new();
        for ip in local_ipv4s() {
            if let Ok(s) = UdpSocket::bind((ip, 0)) {
                let _ = s.set_broadcast(true);
                dests.push(vec![
                    directed_broadcast(ip).to_string(),
                    "255.255.255.255".to_string(),
                ]);
                socks.push(s);
            }
        }
        if socks.is_empty() {
            if let Ok(s) = UdpSocket::bind("0.0.0.0:0") {
                let _ = s.set_broadcast(true);
                dests.push(vec!["255.255.255.255".to_string()]);
                socks.push(s);
            }
        }

        let mut found: HashMap<u64, String> = HashMap::new();
        for _ in 0..rounds {
            for (s, ds) in socks.iter().zip(&dests) {
                let pkt = make_packet(
                    MSG_GET_SERVICE, &[], 0, true, false, false, self.next_seq(),
                );
                for d in ds {
                    let _ = s.send_to(&pkt, (d.as_str(), PORT));
                }
            }
            recv_until(&socks, wait, |d, addr| {
                if let Some((target, _seq, mtype)) = parse_header(d) {
                    if mtype == MSG_STATE_SERVICE && d.len() >= 41 && d[36] == 1 {
                        found.entry(target).or_insert(addr.ip().to_string());
                    }
                }
            });
        }

        let mut lights = Vec::new();
        for (target, ip) in found {
            let l = Light::new(
                Vendor::Lifx,
                serial_hex(target),
                ip,
                PORT,
                target,
                String::new(),
            );
            self.refresh(&l);
            if l.label().is_empty() {
                l.set_label(&serial_hex(target));
            }
            lights.push(l);
        }
        lights
    }

    /// Ask a specific address whether it's a LIFX device, returning its serial.
    /// Used by "add by IP" when broadcast discovery can't reach it (other
    /// subnet, AP isolation, VPN), and to reconnect remembered bulbs.
    ///
    /// Uses its own socket rather than the shared command one: a bulb that's
    /// switched off costs the full timeout, and holding the command lock for
    /// that long would stall every power/colour change meanwhile.
    pub fn probe_target(&self, ip: &str) -> Option<u64> {
        let seq = self.next_seq();
        let pkt = make_packet(MSG_GET_SERVICE, &[], 0, false, true, false, seq);
        let sock = UdpSocket::bind("0.0.0.0:0").ok()?;
        sock.set_read_timeout(Some(Duration::from_millis(200))).ok()?;
        let mut buf = [0u8; 1024];
        for _ in 0..2 {
            let _ = sock.send_to(&pkt, (ip, PORT));
            let deadline = Instant::now() + Duration::from_millis(700);
            while Instant::now() < deadline {
                match sock.recv_from(&mut buf) {
                    Ok((n, addr)) => {
                        if addr.ip().to_string() == ip {
                            if let Some((target, _s, m)) = parse_header(&buf[..n]) {
                                if m == MSG_STATE_SERVICE && target != 0 {
                                    return Some(target);
                                }
                            }
                        }
                    }
                    Err(_) => break,
                }
            }
        }
        None
    }

    /// Build a Light for an already-probed serial (no lock held).
    pub fn light_for(&self, ip: &str, target: u64) -> Arc<Light> {
        Light::new(
            Vendor::Lifx,
            serial_hex(target),
            ip.to_string(),
            PORT,
            target,
            serial_hex(target),
        )
    }

    // -- low-level send ----------------------------------------------------

    /// Send with ack_required, retrying on loss. Returns whether the bulb
    /// acknowledged (callers may ignore; the diagnostic mode reports it).
    fn send_ack(&self, light: &Light, msg_type: u16, payload: &[u8]) -> bool {
        let seq = self.next_seq();
        let pkt = make_packet(msg_type, payload, light.target, false, false, true, seq);
        let _guard = self.rx.lock().unwrap();
        let mut buf = [0u8; 1024];
        for _ in 0..3 {
            let _ = self.sock.send_to(&pkt, (light.ip.as_str(), PORT));
            let deadline = Instant::now() + Duration::from_millis(600);
            while Instant::now() < deadline {
                match self.sock.recv_from(&mut buf) {
                    Ok((n, _)) => {
                        if let Some((t, s, m)) = parse_header(&buf[..n]) {
                            if m == MSG_ACK && s == seq && t == light.target {
                                return true;
                            }
                        }
                    }
                    Err(_) => break,
                }
            }
        }
        false
    }

    fn send_query(&self, light: &Light, msg_type: u16, want: u16) -> Option<Vec<u8>> {
        let seq = self.next_seq();
        let pkt = make_packet(msg_type, &[], light.target, false, true, false, seq);
        let _guard = self.rx.lock().unwrap();
        let mut buf = [0u8; 1024];
        for _ in 0..3 {
            let _ = self.sock.send_to(&pkt, (light.ip.as_str(), PORT));
            let deadline = Instant::now() + Duration::from_millis(600);
            while Instant::now() < deadline {
                match self.sock.recv_from(&mut buf) {
                    Ok((n, _)) => {
                        if let Some((t, s, m)) = parse_header(&buf[..n]) {
                            if m == want && s == seq && t == light.target {
                                return Some(buf[..n].to_vec());
                            }
                        }
                    }
                    Err(_) => break,
                }
            }
        }
        None
    }

    // -- provider API ------------------------------------------------------

    pub fn refresh(&self, light: &Light) {
        let Some(d) = self.send_query(light, MSG_GET, MSG_STATE) else {
            return;
        };
        if d.len() < 52 {
            return;
        }
        let hue = u16::from_le_bytes([d[36], d[37]]);
        let sat = u16::from_le_bytes([d[38], d[39]]);
        let bri = u16::from_le_bytes([d[40], d[41]]);
        let kelvin = u16::from_le_bytes([d[42], d[43]]);
        let power = u16::from_le_bytes([d[46], d[47]]);
        light.set_hsbk([hue, sat, bri, kelvin]);
        light.set_power_state(power > 0);
        if d.len() >= 80 {
            let raw = &d[48..80];
            let end = raw.iter().position(|&b| b == 0).unwrap_or(raw.len());
            if let Ok(label) = std::str::from_utf8(&raw[..end]) {
                light.set_label(label);
            }
        }
    }

    pub fn set_power(&self, light: &Light, on: bool, duration_ms: u32) {
        let mut payload = Vec::with_capacity(6);
        payload.extend_from_slice(&(if on { 65535u16 } else { 0 }).to_le_bytes());
        payload.extend_from_slice(&duration_ms.to_le_bytes());
        self.send_ack(light, MSG_SET_POWER, &payload);
        light.set_power_state(on);
    }

    fn color_payload(h: u16, s: u16, b: u16, k: u16, duration_ms: u32) -> Vec<u8> {
        let k = k.clamp(KELVIN_MIN, KELVIN_MAX);
        let mut p = Vec::with_capacity(13);
        p.push(0);
        p.extend_from_slice(&h.to_le_bytes());
        p.extend_from_slice(&s.to_le_bytes());
        p.extend_from_slice(&b.to_le_bytes());
        p.extend_from_slice(&k.to_le_bytes());
        p.extend_from_slice(&duration_ms.to_le_bytes());
        p
    }

    pub fn set_color(&self, light: &Light, h: u16, s: u16, b: u16, k: u16, duration_ms: u32) {
        let payload = Self::color_payload(h, s, b, k, duration_ms);
        self.send_ack(light, MSG_SET_COLOR, &payload);
        light.set_hsbk([h, s, b, k.clamp(KELVIN_MIN, KELVIN_MAX)]);
    }

    /// Diagnostic: re-send the bulb's own current colour with ack required.
    /// Proves the confirmed-command round-trip without visibly changing
    /// anything.
    pub fn ping_ack(&self, light: &Light) -> bool {
        let [h, s, b, k] = light.hsbk();
        let payload = Self::color_payload(h, s, b, k, 0);
        self.send_ack(light, MSG_SET_COLOR, &payload)
    }

    pub fn stream_color(&self, light: &Light, h: u16, s: u16, b: u16, k: u16, duration_ms: u32) {
        let payload = Self::color_payload(h, s, b, k, duration_ms);
        let pkt = make_packet(
            MSG_SET_COLOR, &payload, light.target, false, false, false, self.next_seq(),
        );
        let _ = self.sock.send_to(&pkt, (light.ip.as_str(), PORT));
        light.set_hsbk([h, s, b, k.clamp(KELVIN_MIN, KELVIN_MAX)]);
    }

    pub fn set_brightness(&self, light: &Light, pct: f32, duration_ms: u32) {
        let [h, s, _b, k] = light.hsbk();
        let bri = (pct.clamp(0.0, 100.0) / 100.0 * 65535.0) as u16;
        self.set_color(light, h, s, bri, k, duration_ms);
    }

    pub fn set_white(&self, light: &Light, pct: f32, kelvin: u16, duration_ms: u32) {
        let bri = (pct.clamp(0.0, 100.0) / 100.0 * 65535.0) as u16;
        self.set_color(light, 0, 0, bri, kelvin, duration_ms);
    }
}
