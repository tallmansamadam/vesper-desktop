//! Govee LAN API provider, ported from govee.py.
//!
//! scan -> multicast 239.255.255.250:4001, replies land on OUR ip:4002;
//! control -> unicast <device>:4003. The provider owns a listener bound to
//! 4002 for its lifetime; if another process holds that port, Govee support
//! degrades gracefully to "no devices found".

use std::net::{Ipv4Addr, SocketAddrV4, UdpSocket};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::color::{hsv_to_rgb, rgb_to_hsv};
use crate::light::{Light, Vendor};
use crate::util::{local_ipv4s, mcast_sender};

const MCAST: (&str, u16) = ("239.255.255.250", 4001);
const RESP_PORT: u16 = 4002;
const CTRL_PORT: u16 = 4003;

fn parse_msg(data: &[u8]) -> Option<(String, Value)> {
    let v: Value = serde_json::from_slice(data).ok()?;
    let msg = v.get("msg")?;
    let cmd = msg.get("cmd")?.as_str()?.to_string();
    let d = msg.get("data").cloned().unwrap_or_else(|| json!({}));
    Some((cmd, d))
}

fn status_to_hsbk(d: &Value) -> [u16; 4] {
    let bri_pct = d.get("brightness").and_then(Value::as_f64).unwrap_or(100.0);
    let bri = (bri_pct.clamp(0.0, 100.0) / 100.0 * 65535.0) as u16;
    let kelvin = d
        .get("colorTemInKelvin")
        .and_then(Value::as_f64)
        .unwrap_or(0.0) as u16;
    if kelvin > 0 {
        return [0, 0, bri, kelvin];
    }
    let c = d.get("color").cloned().unwrap_or_else(|| json!({}));
    let r = c.get("r").and_then(Value::as_f64).unwrap_or(0.0);
    let g = c.get("g").and_then(Value::as_f64).unwrap_or(0.0);
    let b = c.get("b").and_then(Value::as_f64).unwrap_or(0.0);
    if r > 0.0 || g > 0.0 || b > 0.0 {
        let (h, s, _v) = rgb_to_hsv(r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0);
        return [(h * 65535.0) as u16, (s * 65535.0) as u16, bri, 3500];
    }
    [0, 0, bri, 3500]
}

pub struct Govee {
    rx: Option<UdpSocket>,
    tx: UdpSocket,
    io: Mutex<()>,
}

impl Govee {
    pub fn new() -> Govee {
        let rx = (|| -> std::io::Result<UdpSocket> {
            use socket2::{Domain, Protocol, Socket, Type};
            let s = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
            s.set_reuse_address(true)?;
            s.bind(&SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, RESP_PORT).into())?;
            let udp: UdpSocket = s.into();
            udp.set_read_timeout(Some(Duration::from_millis(200)))?;
            Ok(udp)
        })()
        .ok();
        let tx = UdpSocket::bind("0.0.0.0:0").expect("udp bind");
        Govee {
            rx,
            tx,
            io: Mutex::new(()),
        }
    }

    fn send(&self, ip: &str, port: u16, cmd: &str, data: Value) {
        let payload = json!({"msg": {"cmd": cmd, "data": data}}).to_string();
        let _ = self.tx.send_to(payload.as_bytes(), (ip, port));
    }

    fn drain(&self) {
        let Some(rx) = &self.rx else { return };
        let _ = rx.set_read_timeout(Some(Duration::from_millis(1)));
        let mut buf = [0u8; 4096];
        while rx.recv_from(&mut buf).is_ok() {}
        let _ = rx.set_read_timeout(Some(Duration::from_millis(200)));
    }

    fn collect(&self, wait: Duration, mut handler: impl FnMut(&str, &Value, &str)) {
        let Some(rx) = &self.rx else { return };
        let deadline = Instant::now() + wait;
        let mut buf = [0u8; 4096];
        while Instant::now() < deadline {
            if let Ok((n, addr)) = rx.recv_from(&mut buf) {
                if let Some((cmd, d)) = parse_msg(&buf[..n]) {
                    handler(&cmd, &d, &addr.ip().to_string());
                }
            }
        }
    }

    pub fn discover(&self) -> Vec<Arc<Light>> {
        if self.rx.is_none() {
            return Vec::new();
        }
        let _guard = self.io.lock().unwrap();
        self.drain();
        let scan = json!({"msg": {"cmd": "scan", "data": {"account_topic": "reserve"}}})
            .to_string();
        let mut sent = false;
        for ip in local_ipv4s() {
            if let Ok(s) = mcast_sender(ip) {
                if s.send_to(scan.as_bytes(), MCAST).is_ok() {
                    sent = true;
                }
            }
        }
        if !sent {
            let _ = self.tx.send_to(scan.as_bytes(), MCAST);
        }

        let mut lights: Vec<Arc<Light>> = Vec::new();
        self.collect(Duration::from_millis(1500), |cmd, d, src_ip| {
            if cmd != "scan" {
                return;
            }
            let Some(device) = d.get("device").and_then(Value::as_str) else {
                return;
            };
            if lights.iter().any(|l| l.id == format!("govee:{device}")) {
                return;
            }
            let ip = d
                .get("ip")
                .and_then(Value::as_str)
                .unwrap_or(src_ip)
                .to_string();
            let sku = d.get("sku").and_then(Value::as_str).unwrap_or("");
            let label = if sku.is_empty() {
                "Govee".to_string()
            } else {
                format!("Govee {sku}")
            };
            lights.push(Light::new(
                Vendor::Govee,
                format!("govee:{device}"),
                ip,
                CTRL_PORT,
                0,
                label,
            ));
        });
        for l in &lights {
            self.refresh_locked(l);
        }
        lights
    }

    /// Ask a specific address whether it's a Govee device with LAN Control on
    /// (for "add by IP").
    pub fn probe(&self, ip: &str) -> Option<Arc<Light>> {
        self.rx.as_ref()?;
        let _guard = self.io.lock().unwrap();
        self.drain();
        self.send(ip, CTRL_PORT, "devStatus", json!({}));
        let mut state: Option<Value> = None;
        let want = ip.to_string();
        self.collect(Duration::from_millis(900), |cmd, d, src| {
            if cmd == "devStatus" && src == want {
                state = Some(d.clone());
            }
        });
        let d = state?;
        let l = Light::new(
            Vendor::Govee,
            format!("govee:{ip}"),
            ip.to_string(),
            CTRL_PORT,
            0,
            "Govee".to_string(),
        );
        l.set_power_state(d.get("onOff").and_then(Value::as_f64).unwrap_or(0.0) > 0.0);
        l.set_hsbk(status_to_hsbk(&d));
        Some(l)
    }

    fn refresh_locked(&self, light: &Light) {
        self.drain();
        self.send(&light.ip, CTRL_PORT, "devStatus", json!({}));
        let mut state: Option<Value> = None;
        let want_ip = light.ip.clone();
        self.collect(Duration::from_millis(800), |cmd, d, src_ip| {
            if cmd == "devStatus" && src_ip == want_ip {
                state = Some(d.clone());
            }
        });
        if let Some(d) = state {
            light.set_power_state(d.get("onOff").and_then(Value::as_f64).unwrap_or(0.0) > 0.0);
            light.set_hsbk(status_to_hsbk(&d));
        }
    }

    pub fn refresh(&self, light: &Light) {
        if self.rx.is_none() {
            return;
        }
        let _guard = self.io.lock().unwrap();
        self.refresh_locked(light);
    }

    pub fn set_power(&self, light: &Light, on: bool, _duration_ms: u32) {
        self.send(&light.ip, CTRL_PORT, "turn", json!({"value": if on { 1 } else { 0 }}));
        light.set_power_state(on);
    }

    pub fn set_color(&self, light: &Light, h: u16, s: u16, b: u16, k: u16, _duration_ms: u32) {
        let pct = ((b as f32 / 65535.0 * 100.0).round() as i64).max(1);
        if s <= 300 {
            self.set_white(light, pct as f32, k, 0);
            return;
        }
        let (r, g, bl) = hsv_to_rgb(h as f32 / 65535.0, s as f32 / 65535.0, 1.0);
        self.send(
            &light.ip,
            CTRL_PORT,
            "colorwc",
            json!({
                "color": {"r": (r * 255.0) as u8, "g": (g * 255.0) as u8,
                          "b": (bl * 255.0) as u8},
                "colorTemInKelvin": 0,
            }),
        );
        self.send(&light.ip, CTRL_PORT, "brightness", json!({"value": pct}));
        let [_h0, _s0, _b0, k0] = light.hsbk();
        light.set_hsbk([h, s, b, k0]);
    }

    pub fn set_brightness(&self, light: &Light, pct: f32, _duration_ms: u32) {
        let p = (pct.clamp(1.0, 100.0)) as i64;
        self.send(&light.ip, CTRL_PORT, "brightness", json!({"value": p}));
        let [h, s, _b, k] = light.hsbk();
        light.set_hsbk([h, s, (pct / 100.0 * 65535.0) as u16, k]);
    }

    pub fn set_white(&self, light: &Light, pct: f32, kelvin: u16, _duration_ms: u32) {
        let k = (kelvin as i64).clamp(2000, 9000);
        self.send(
            &light.ip,
            CTRL_PORT,
            "colorwc",
            json!({"color": {"r": 0, "g": 0, "b": 0}, "colorTemInKelvin": k}),
        );
        self.send(
            &light.ip,
            CTRL_PORT,
            "brightness",
            json!({"value": (pct.clamp(1.0, 100.0)) as i64}),
        );
        light.set_hsbk([0, 0, (pct / 100.0 * 65535.0) as u16, k as u16]);
    }

    pub fn stream_color(&self, light: &Light, h: u16, s: u16, b: u16, k: u16, _duration_ms: u32) {
        self.set_color(light, h, s, b, k, 0);
    }
}
