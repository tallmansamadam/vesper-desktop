//! WiZ provider: JSON-over-UDP on port 38899 (setPilot/getPilot), ported
//! from wiz.py. Bridge-free like LIFX, so streaming effects work.

use std::net::UdpSocket;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};

use crate::color::{hsv_to_rgb, rgb_to_hsv};
use crate::light::{Light, Vendor};
use crate::util::{directed_broadcast, local_ipv4s, recv_until};

const PORT: u16 = 38899;

fn state_to_hsbk(res: &Value) -> [u16; 4] {
    let dim = res.get("dimming").and_then(Value::as_f64).unwrap_or(100.0);
    let bri = (dim.clamp(0.0, 100.0) / 100.0 * 65535.0) as u16;
    if let Some(temp) = res.get("temp").and_then(Value::as_f64) {
        if temp > 0.0 {
            return [0, 0, bri, temp as u16];
        }
    }
    let r = res.get("r").and_then(Value::as_f64).unwrap_or(0.0);
    let g = res.get("g").and_then(Value::as_f64).unwrap_or(0.0);
    let b = res.get("b").and_then(Value::as_f64).unwrap_or(0.0);
    if r > 0.0 || g > 0.0 || b > 0.0 {
        let (h, s, _v) = rgb_to_hsv(r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0);
        return [(h * 65535.0) as u16, (s * 65535.0) as u16, bri, 3500];
    }
    [0, 0, bri, 3500]
}

pub struct Wiz {
    cmd: UdpSocket,
    io: Mutex<()>, // serialise send+recv exchanges (refresh)
}

impl Wiz {
    pub fn new() -> std::io::Result<Wiz> {
        let cmd = UdpSocket::bind("0.0.0.0:0")?;
        cmd.set_broadcast(true)?;
        cmd.set_read_timeout(Some(Duration::from_millis(500)))?;
        Ok(Wiz {
            cmd,
            io: Mutex::new(()),
        })
    }

    pub fn discover(&self) -> Vec<Arc<Light>> {
        let payload = json!({"method": "getPilot", "params": {}}).to_string();
        let mut socks = Vec::new();
        let mut dests: Vec<Vec<String>> = Vec::new();
        for ip in local_ipv4s() {
            if let Ok(s) = UdpSocket::bind((ip, 0)) {
                let _ = s.set_broadcast(true);
                let _ = s.set_read_timeout(Some(Duration::from_millis(100)));
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
                let _ = s.set_read_timeout(Some(Duration::from_millis(100)));
                dests.push(vec!["255.255.255.255".to_string()]);
                socks.push(s);
            }
        }

        let mut lights: Vec<Arc<Light>> = Vec::new();
        for _round in 0..2 {
            for (s, ds) in socks.iter().zip(&dests) {
                for d in ds {
                    let _ = s.send_to(payload.as_bytes(), (d.as_str(), PORT));
                }
            }
            recv_until(&socks, Duration::from_millis(1200), |data, addr| {
                let Ok(v) = serde_json::from_slice::<Value>(data) else {
                    return;
                };
                let res = &v["result"];
                let Some(mac) = res.get("mac").and_then(Value::as_str) else {
                    return;
                };
                let ip = addr.ip().to_string();
                if lights.iter().any(|l| l.ip == ip) {
                    return;
                }
                let tail: String = mac.chars().rev().take(6).collect::<Vec<_>>()
                    .into_iter().rev().collect();
                let l = Light::new(
                    Vendor::Wiz,
                    format!("wiz:{mac}"),
                    ip,
                    PORT,
                    0,
                    format!("WiZ {tail}"),
                );
                l.set_power_state(res.get("state").and_then(Value::as_bool).unwrap_or(false));
                l.set_hsbk(state_to_hsbk(res));
                lights.push(l);
            });
        }
        lights
    }

    fn send(&self, light: &Light, params: Value) {
        let msg = json!({"method": "setPilot", "params": params}).to_string();
        let _ = self.cmd.send_to(msg.as_bytes(), (light.ip.as_str(), PORT));
    }

    pub fn refresh(&self, light: &Light) {
        let _guard = self.io.lock().unwrap();
        let msg = json!({"method": "getPilot", "params": {}}).to_string();
        if self.cmd.send_to(msg.as_bytes(), (light.ip.as_str(), PORT)).is_err() {
            return;
        }
        let mut buf = [0u8; 2048];
        if let Ok((n, _)) = self.cmd.recv_from(&mut buf) {
            if let Ok(v) = serde_json::from_slice::<Value>(&buf[..n]) {
                let res = &v["result"];
                if let Some(state) = res.get("state").and_then(Value::as_bool) {
                    light.set_power_state(state);
                }
                light.set_hsbk(state_to_hsbk(res));
            }
        }
    }

    pub fn set_power(&self, light: &Light, on: bool, _duration_ms: u32) {
        self.send(light, json!({"state": on}));
        light.set_power_state(on);
    }

    pub fn set_color(&self, light: &Light, h: u16, s: u16, b: u16, k: u16, _duration_ms: u32) {
        let pct = (b as f32 / 65535.0 * 100.0).round() as i64;
        if pct <= 0 {
            self.send(light, json!({"state": false}));
            light.set_hsbk([h, s, 0, k]);
            return;
        }
        let mut params = json!({"state": true, "dimming": pct.clamp(10, 100)});
        if s <= 300 {
            params["temp"] = json!((k as i64).clamp(2200, 6500));
        } else {
            let (r, g, bl) = hsv_to_rgb(h as f32 / 65535.0, s as f32 / 65535.0, 1.0);
            params["r"] = json!((r * 255.0) as u8);
            params["g"] = json!((g * 255.0) as u8);
            params["b"] = json!((bl * 255.0) as u8);
            params["c"] = json!(0);
            params["w"] = json!(0);
        }
        self.send(light, params);
        light.set_hsbk([h, s, b, k]);
    }

    pub fn set_brightness(&self, light: &Light, pct: f32, _duration_ms: u32) {
        let pct = pct.clamp(0.0, 100.0);
        if pct <= 0.0 {
            self.send(light, json!({"state": false}));
        } else {
            self.send(
                light,
                json!({"state": true, "dimming": (pct as i64).max(10)}),
            );
        }
        let [h, s, _b, k] = light.hsbk();
        light.set_hsbk([h, s, (pct / 100.0 * 65535.0) as u16, k]);
    }

    pub fn set_white(&self, light: &Light, pct: f32, kelvin: u16, _duration_ms: u32) {
        self.send(
            light,
            json!({
                "state": true,
                "dimming": (pct.clamp(0.0, 100.0) as i64).max(10),
                "temp": (kelvin as i64).clamp(2200, 6500),
            }),
        );
        light.set_hsbk([0, 0, (pct / 100.0 * 65535.0) as u16, kelvin]);
    }

    pub fn stream_color(&self, light: &Light, h: u16, s: u16, b: u16, k: u16, _duration_ms: u32) {
        self.set_color(light, h, s, b, k, 0);
    }
}
