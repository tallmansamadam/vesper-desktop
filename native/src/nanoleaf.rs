//! Nanoleaf provider, ported from nanoleaf.py: REST on :16021, SSDP
//! discovery, link-button-style pairing (hold the power button 5-7s, then
//! POST /api/v1/new). Tokens persist in the same nanoleaf.json the Python
//! app used, so existing pairings carry over.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::light::{Light, Vendor};
use crate::util::{local_ipv4s, mcast_sender, recv_until};

const SSDP: (&str, u16) = ("239.255.255.250", 1900);
const MSEARCH: &str = "M-SEARCH * HTTP/1.1\r\nHOST: 239.255.255.250:1900\r\nMAN: \"ssdp:discover\"\r\nMX: 2\r\nST: ssdp:all\r\n\r\n";

fn parse_headers(text: &str) -> HashMap<String, String> {
    let mut h = HashMap::new();
    for line in text.split("\r\n") {
        if let Some((k, v)) = line.split_once(':') {
            h.insert(k.trim().to_lowercase(), v.trim().to_string());
        }
    }
    h
}

fn is_nanoleaf(h: &HashMap<String, String>) -> bool {
    let blob = ["st", "usn", "server", "nl-devicename"]
        .iter()
        .filter_map(|k| h.get(*k).map(String::as_str))
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase();
    blob.contains("nanoleaf") || blob.contains("aurora")
}

fn state_to_hsbk(info: &Value) -> [u16; 4] {
    let st = info.get("state").cloned().unwrap_or_else(|| json!({}));
    let val = |key: &str, default: f64| -> f64 {
        st.get(key)
            .and_then(|v| v.get("value"))
            .and_then(Value::as_f64)
            .unwrap_or(default)
    };
    let bri = (val("brightness", 100.0) / 100.0 * 65535.0) as u16;
    if st.get("colorMode").and_then(Value::as_str) == Some("ct") {
        return [0, 0, bri, val("ct", 4000.0) as u16];
    }
    [
        (val("hue", 0.0) / 360.0 * 65535.0) as u16,
        (val("sat", 0.0) / 100.0 * 65535.0) as u16,
        bri,
        3500,
    ]
}

pub struct Nanoleaf {
    /// ip -> (port, token)
    devices: Mutex<HashMap<String, (u16, String)>>,
    settings: PathBuf,
    agent: ureq::Agent,
    last_stream: Mutex<HashMap<String, Instant>>,
}

impl Nanoleaf {
    pub fn from_settings(root: &Path) -> Nanoleaf {
        let settings = root.join("nanoleaf.json");
        let mut devices = HashMap::new();
        if let Ok(text) = std::fs::read_to_string(&settings) {
            if let Ok(v) = serde_json::from_str::<Value>(&text) {
                if let Some(map) = v.as_object() {
                    for (ip, d) in map {
                        let port =
                            d.get("port").and_then(Value::as_u64).unwrap_or(16021) as u16;
                        let token = d
                            .get("token")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string();
                        devices.insert(ip.clone(), (port, token));
                    }
                }
            }
        }
        Nanoleaf {
            devices: Mutex::new(devices),
            settings,
            agent: ureq::AgentBuilder::new()
                .timeout(Duration::from_secs(4))
                .build(),
            last_stream: Mutex::new(HashMap::new()),
        }
    }

    fn save(&self) {
        let devices = self.devices.lock().unwrap();
        let mut obj = serde_json::Map::new();
        for (ip, (port, token)) in devices.iter() {
            obj.insert(ip.clone(), json!({"port": port, "token": token}));
        }
        let _ = std::fs::write(
            &self.settings,
            serde_json::to_string_pretty(&Value::Object(obj)).unwrap_or_default(),
        );
    }

    fn base_url(&self, ip: &str) -> Option<String> {
        let devices = self.devices.lock().unwrap();
        let (port, token) = devices.get(ip)?;
        Some(format!("http://{ip}:{port}/api/v1/{token}"))
    }

    // -- SSDP + pairing ----------------------------------------------------

    pub fn ssdp_search() -> HashMap<String, u16> {
        let mut socks = Vec::new();
        for ip in local_ipv4s() {
            if let Ok(s) = mcast_sender(ip) {
                if s.send_to(MSEARCH.as_bytes(), SSDP).is_ok() {
                    socks.push(s);
                }
            }
        }
        let mut found = HashMap::new();
        recv_until(&socks, Duration::from_millis(1800), |data, addr| {
            let text = String::from_utf8_lossy(data);
            let h = parse_headers(&text);
            if !is_nanoleaf(&h) {
                return;
            }
            let (ip, port) = match h.get("location").and_then(|l| l.strip_prefix("http://")) {
                Some(rest) => {
                    let hostport = rest.split('/').next().unwrap_or(rest);
                    let (ip, p) = hostport.split_once(':').unwrap_or((hostport, "16021"));
                    (ip.to_string(), p.parse().unwrap_or(16021))
                }
                None => (addr.ip().to_string(), 16021),
            };
            found.insert(ip, port);
        });
        found
    }

    /// Mint tokens for panels currently in pairing mode. Returns how many
    /// devices were newly paired; calls `on_wait(seconds_remaining)` while
    /// waiting.
    pub fn pair(&self, attempts: u32, on_wait: &dyn Fn(u32)) -> usize {
        let candidates = Self::ssdp_search();
        let mut fresh: HashMap<String, u16> = {
            let devices = self.devices.lock().unwrap();
            candidates
                .into_iter()
                .filter(|(ip, _)| !devices.contains_key(ip))
                .collect()
        };
        if fresh.is_empty() {
            return 0;
        }
        let mut paired = 0;
        for i in 0..attempts {
            let ips: Vec<(String, u16)> =
                fresh.iter().map(|(a, b)| (a.clone(), *b)).collect();
            for (ip, port) in ips {
                let url = format!("http://{ip}:{port}/api/v1/new");
                if let Ok(resp) = self.agent.post(&url).send_bytes(b"") {
                    if let Ok(v) = resp.into_json::<Value>() {
                        if let Some(token) = v.get("auth_token").and_then(Value::as_str) {
                            self.devices
                                .lock()
                                .unwrap()
                                .insert(ip.clone(), (port, token.to_string()));
                            fresh.remove(&ip);
                            paired += 1;
                            self.save();
                        }
                    }
                }
            }
            if fresh.is_empty() {
                break;
            }
            on_wait(attempts - i);
            std::thread::sleep(Duration::from_secs(1));
        }
        paired
    }

    // -- provider API ------------------------------------------------------

    pub fn discover(&self) -> Vec<Arc<Light>> {
        let device_ips: Vec<(String, u16)> = {
            let devices = self.devices.lock().unwrap();
            devices.iter().map(|(ip, (p, _))| (ip.clone(), *p)).collect()
        };
        let mut lights = Vec::new();
        for (ip, port) in device_ips {
            let Some(url) = self.base_url(&ip) else { continue };
            let Ok(resp) = self.agent.get(&url).call() else {
                continue; // offline right now
            };
            let Ok(info) = resp.into_json::<Value>() else {
                continue;
            };
            let label = info
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("Nanoleaf")
                .to_string();
            let l = Light::new(
                Vendor::Nanoleaf,
                format!("nanoleaf:{ip}"),
                ip,
                port,
                0,
                label,
            );
            l.set_power_state(
                info.pointer("/state/on/value").and_then(Value::as_bool).unwrap_or(false),
            );
            l.set_hsbk(state_to_hsbk(&info));
            lights.push(l);
        }
        lights
    }

    pub fn refresh(&self, light: &Light) {
        let Some(url) = self.base_url(&light.ip) else { return };
        let Ok(resp) = self.agent.get(&url).call() else { return };
        let Ok(info) = resp.into_json::<Value>() else { return };
        light.set_power_state(
            info.pointer("/state/on/value").and_then(Value::as_bool).unwrap_or(false),
        );
        light.set_hsbk(state_to_hsbk(&info));
    }

    fn put_state(&self, light: &Light, payload: Value) {
        let Some(url) = self.base_url(&light.ip) else { return };
        let _ = self.agent.put(&format!("{url}/state")).send_json(payload);
    }

    pub fn set_power(&self, light: &Light, on: bool, _duration_ms: u32) {
        self.put_state(light, json!({"on": {"value": on}}));
        light.set_power_state(on);
    }

    pub fn set_color(&self, light: &Light, h: u16, s: u16, b: u16, k: u16, _duration_ms: u32) {
        let pct = ((b as f32 / 65535.0 * 100.0).round() as i64).max(1);
        if s <= 300 {
            self.set_white(light, pct as f32, k, 0);
            return;
        }
        self.put_state(
            light,
            json!({
                "on": {"value": true},
                "hue": {"value": ((h as f32 / 65535.0 * 360.0).round() as i64) % 360},
                "sat": {"value": (s as f32 / 65535.0 * 100.0).round() as i64},
                "brightness": {"value": pct},
            }),
        );
        light.set_power_state(true);
        let [_h0, _s0, _b0, k0] = light.hsbk();
        light.set_hsbk([h, s, b, k0]);
    }

    pub fn set_brightness(&self, light: &Light, pct: f32, _duration_ms: u32) {
        let p = (pct.clamp(1.0, 100.0)) as i64;
        self.put_state(light, json!({"brightness": {"value": p}}));
        let [h, s, _b, k] = light.hsbk();
        light.set_hsbk([h, s, (pct / 100.0 * 65535.0) as u16, k]);
    }

    pub fn set_white(&self, light: &Light, pct: f32, kelvin: u16, _duration_ms: u32) {
        let k = (kelvin as i64).clamp(1200, 6500);
        let p = (pct.clamp(1.0, 100.0)) as i64;
        self.put_state(
            light,
            json!({"on": {"value": true}, "ct": {"value": k}, "brightness": {"value": p}}),
        );
        light.set_power_state(true);
        light.set_hsbk([0, 0, (pct / 100.0 * 65535.0) as u16, k as u16]);
    }

    pub fn stream_color(&self, light: &Light, h: u16, s: u16, b: u16, k: u16, d: u32) {
        // REST per frame is heavy; throttle streamed frames per light.
        {
            let mut last = self.last_stream.lock().unwrap();
            let now = Instant::now();
            if let Some(prev) = last.get(&light.id) {
                if now.duration_since(*prev) < Duration::from_millis(300) {
                    return;
                }
            }
            last.insert(light.id.clone(), now);
        }
        self.set_color(light, h, s, b, k, d);
    }
}
