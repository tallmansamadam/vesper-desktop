//! Yeelight provider, ported from yeelight.py: SSDP-style discovery on
//! multicast :1982, JSON-over-TCP control on :55443. The LAN interface
//! enforces ~60 commands/minute per bulb, so streamed frames are throttled
//! per light instead of flooding the quota.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::color::{hsv_to_rgb, rgb_to_hsv};
use crate::light::{Light, Vendor};
use crate::util::{local_ipv4s, mcast_sender, recv_until};

const MCAST: (&str, u16) = ("239.255.255.250", 1982);
const MSEARCH: &str = "M-SEARCH * HTTP/1.1\r\nHOST: 239.255.255.250:1982\r\nMAN: \"ssdp:discover\"\r\nST: wifi_bulb\r\n\r\n";
const STREAM_MIN_INTERVAL: Duration = Duration::from_millis(400);

fn parse_headers(text: &str) -> HashMap<String, String> {
    let mut h = HashMap::new();
    for line in text.split("\r\n") {
        if let Some((k, v)) = line.split_once(':') {
            h.insert(k.trim().to_lowercase(), v.trim().to_string());
        }
    }
    h
}

pub struct Yeelight {
    conns: Mutex<HashMap<String, TcpStream>>,
    msg_id: AtomicU64,
    last_stream: Mutex<HashMap<String, Instant>>,
}

impl Yeelight {
    pub fn new() -> Yeelight {
        Yeelight {
            conns: Mutex::new(HashMap::new()),
            msg_id: AtomicU64::new(0),
            last_stream: Mutex::new(HashMap::new()),
        }
    }

    pub fn discover(&self) -> Vec<Arc<Light>> {
        let mut socks = Vec::new();
        for ip in local_ipv4s() {
            if let Ok(s) = mcast_sender(ip) {
                if s.send_to(MSEARCH.as_bytes(), MCAST).is_ok() {
                    socks.push(s);
                }
            }
        }
        let mut lights: Vec<Arc<Light>> = Vec::new();
        recv_until(&socks, Duration::from_millis(1500), |data, _addr| {
            let text = String::from_utf8_lossy(data);
            let h = parse_headers(&text);
            let Some(loc) = h.get("location") else { return };
            let Some(hostport) = loc.strip_prefix("yeelight://") else {
                return;
            };
            let (ip, port_s) = hostport.split_once(':').unwrap_or((hostport, "55443"));
            let port: u16 = port_s.parse().unwrap_or(55443);
            let ident = h.get("id").cloned().unwrap_or_else(|| ip.to_string());
            if lights.iter().any(|l| l.id == format!("yeelight:{ident}")) {
                return;
            }
            let label = match h.get("name").filter(|n| !n.is_empty()) {
                Some(n) => n.clone(),
                None => format!("Yeelight {}", h.get("model").map(String::as_str).unwrap_or(""))
                    .trim()
                    .to_string(),
            };
            let bri: f32 = h.get("bright").and_then(|b| b.parse().ok()).unwrap_or(100.0);
            let b16 = (bri / 100.0 * 65535.0) as u16;
            let hsbk = match h.get("color_mode").map(String::as_str) {
                Some("1") => {
                    let rgb: u32 = h.get("rgb").and_then(|v| v.parse().ok()).unwrap_or(0);
                    let (r, g, b) = ((rgb >> 16) & 0xff, (rgb >> 8) & 0xff, rgb & 0xff);
                    let (hh, ss, _v) =
                        rgb_to_hsv(r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0);
                    [(hh * 65535.0) as u16, (ss * 65535.0) as u16, b16, 3500]
                }
                Some("3") => {
                    let hue: f32 = h.get("hue").and_then(|v| v.parse().ok()).unwrap_or(0.0);
                    let sat: f32 = h.get("sat").and_then(|v| v.parse().ok()).unwrap_or(0.0);
                    [
                        (hue / 360.0 * 65535.0) as u16,
                        (sat / 100.0 * 65535.0) as u16,
                        b16,
                        3500,
                    ]
                }
                _ => {
                    let ct: u16 = h.get("ct").and_then(|v| v.parse().ok()).unwrap_or(4000);
                    [0, 0, b16, ct]
                }
            };
            let l = Light::new(
                Vendor::Yeelight,
                format!("yeelight:{ident}"),
                ip.to_string(),
                port,
                0,
                label,
            );
            l.set_power_state(h.get("power").map(String::as_str) == Some("on"));
            l.set_hsbk(hsbk);
            lights.push(l);
        });
        lights
    }

    // -- TCP command channel ----------------------------------------------

    fn cmd(&self, light: &Light, method: &str, params: Value) -> Option<Value> {
        let mut conns = self.conns.lock().unwrap();
        for attempt in 0..2 {
            if !conns.contains_key(&light.id) {
                let addr: SocketAddr = format!("{}:{}", light.ip, light.port).parse().ok()?;
                let Ok(sock) = TcpStream::connect_timeout(&addr, Duration::from_secs(2))
                else {
                    if attempt == 1 {
                        return None;
                    }
                    continue;
                };
                let _ = sock.set_read_timeout(Some(Duration::from_millis(1500)));
                let _ = sock.set_write_timeout(Some(Duration::from_millis(1500)));
                conns.insert(light.id.clone(), sock);
            }
            let id = self.msg_id.fetch_add(1, Ordering::Relaxed) + 1;
            let payload =
                json!({"id": id, "method": method, "params": params}).to_string() + "\r\n";
            let sock = conns.get_mut(&light.id).unwrap();
            let ok = sock.write_all(payload.as_bytes()).is_ok();
            if ok {
                if let Some(reply) = Self::read_reply(sock, id) {
                    return Some(reply);
                }
            }
            conns.remove(&light.id);
            if attempt == 1 {
                return None;
            }
        }
        None
    }

    fn read_reply(sock: &mut TcpStream, msg_id: u64) -> Option<Value> {
        let mut buf: Vec<u8> = Vec::new();
        let mut chunk = [0u8; 4096];
        let deadline = Instant::now() + Duration::from_millis(1500);
        while Instant::now() < deadline {
            let n = match sock.read(&mut chunk) {
                Ok(0) => return None, // closed
                Ok(n) => n,
                Err(_) => return None,
            };
            buf.extend_from_slice(&chunk[..n]);
            while let Some(pos) = buf.windows(2).position(|w| w == b"\r\n") {
                let line: Vec<u8> = buf.drain(..pos + 2).collect();
                if let Ok(obj) = serde_json::from_slice::<Value>(&line[..pos]) {
                    if obj.get("id").and_then(Value::as_u64) == Some(msg_id) {
                        return Some(obj);
                    }
                    // else: unsolicited notification; skip
                }
            }
        }
        None
    }

    // -- provider API ------------------------------------------------------

    pub fn refresh(&self, light: &Light) {
        let res = self.cmd(
            light,
            "get_prop",
            json!(["power", "bright", "ct", "rgb", "hue", "sat", "color_mode", "name"]),
        );
        let Some(obj) = res else { return };
        let Some(arr) = obj.get("result").and_then(Value::as_array) else {
            return;
        };
        if arr.len() < 7 {
            return;
        }
        let s = |i: usize| arr.get(i).and_then(Value::as_str).unwrap_or("");
        light.set_power_state(s(0) == "on");
        let bright: f32 = s(1).parse().unwrap_or(100.0);
        let b16 = (bright / 100.0 * 65535.0) as u16;
        match s(6) {
            "1" => {
                let v: u32 = s(3).parse().unwrap_or(0);
                let (r, g, b) = ((v >> 16) & 0xff, (v >> 8) & 0xff, v & 0xff);
                let (hh, ss, _) =
                    rgb_to_hsv(r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0);
                light.set_hsbk([(hh * 65535.0) as u16, (ss * 65535.0) as u16, b16, 3500]);
            }
            "3" => {
                let hue: f32 = s(4).parse().unwrap_or(0.0);
                let sat: f32 = s(5).parse().unwrap_or(0.0);
                light.set_hsbk([
                    (hue / 360.0 * 65535.0) as u16,
                    (sat / 100.0 * 65535.0) as u16,
                    b16,
                    3500,
                ]);
            }
            _ => {
                let ct: u16 = s(2).parse().unwrap_or(4000);
                light.set_hsbk([0, 0, b16, ct]);
            }
        }
        if arr.len() >= 8 && !s(7).is_empty() {
            light.set_label(s(7));
        }
    }

    pub fn set_power(&self, light: &Light, on: bool, duration_ms: u32) {
        self.cmd(
            light,
            "set_power",
            json!([if on { "on" } else { "off" }, "smooth", duration_ms.max(30)]),
        );
        light.set_power_state(on);
    }

    pub fn set_color(&self, light: &Light, h: u16, s: u16, b: u16, k: u16, duration_ms: u32) {
        let pct = ((b as f32 / 65535.0 * 100.0).round() as i64).max(1);
        if s <= 300 {
            self.set_white(light, pct as f32, k, duration_ms);
            return;
        }
        let (r, g, bl) = hsv_to_rgb(h as f32 / 65535.0, s as f32 / 65535.0, 1.0);
        let rgb: u32 = (((r * 255.0) as u32) << 16) | (((g * 255.0) as u32) << 8)
            | ((bl * 255.0) as u32);
        // set_scene sets colour + brightness atomically (and powers on)
        self.cmd(light, "set_scene", json!(["color", rgb, pct]));
        light.set_power_state(true);
        let [_h0, _s0, _b0, k0] = light.hsbk();
        light.set_hsbk([h, s, b, k0]);
    }

    pub fn set_brightness(&self, light: &Light, pct: f32, duration_ms: u32) {
        let p = (pct.clamp(1.0, 100.0)) as i64;
        self.cmd(light, "set_bright", json!([p, "smooth", duration_ms.max(30)]));
        let [h, s, _b, k] = light.hsbk();
        light.set_hsbk([h, s, (pct / 100.0 * 65535.0) as u16, k]);
    }

    pub fn set_white(&self, light: &Light, pct: f32, kelvin: u16, _duration_ms: u32) {
        let k = (kelvin as i64).clamp(1700, 6500);
        let p = (pct.clamp(1.0, 100.0)) as i64;
        self.cmd(light, "set_scene", json!(["ct", k, p]));
        light.set_power_state(true);
        light.set_hsbk([0, 0, (pct / 100.0 * 65535.0) as u16, k as u16]);
    }

    pub fn stream_color(&self, light: &Light, h: u16, s: u16, b: u16, k: u16, duration_ms: u32) {
        // Respect the LAN quota: drop frames faster than the per-light interval.
        {
            let mut last = self.last_stream.lock().unwrap();
            let now = Instant::now();
            if let Some(prev) = last.get(&light.id) {
                if now.duration_since(*prev) < STREAM_MIN_INTERVAL {
                    return;
                }
            }
            last.insert(light.id.clone(), now);
        }
        self.set_color(light, h, s, b, k, duration_ms);
    }
}
