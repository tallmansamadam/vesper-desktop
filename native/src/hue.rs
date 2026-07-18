//! Philips Hue provider, ported from hue.py: HTTPS REST to the bridge
//! (CLIP v2, self-signed cert), link-button pairing, HSBK -> CIE-xy colour.
//! Credentials live in the same hue.json the Python app used.

use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use serde_json::{json, Value};

use crate::color::{hsv_to_rgb, rgb_to_hsv};
use crate::light::{Light, Vendor};

const APP_NAME: &str = "lifx_control#desktop";

// -- colour conversion (HSBK <-> CIE xy, per Hue's recommended transform) --

fn gamma(c: f64) -> f64 {
    if c > 0.04045 {
        ((c + 0.055) / 1.055).powf(2.4)
    } else {
        c / 12.92
    }
}

fn ungamma(c: f64) -> f64 {
    if c > 0.0031308 {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    } else {
        12.92 * c
    }
}

fn hsbk_to_xy(h: u16, s: u16) -> (f64, f64) {
    let (r, g, b) = hsv_to_rgb(h as f32 / 65535.0, s as f32 / 65535.0, 1.0);
    let (r, g, b) = (gamma(r as f64), gamma(g as f64), gamma(b as f64));
    let x = r * 0.664511 + g * 0.154324 + b * 0.162028;
    let y = r * 0.283881 + g * 0.668433 + b * 0.047685;
    let z = r * 0.000088 + g * 0.072310 + b * 0.986039;
    let total = if x + y + z == 0.0 { 1.0 } else { x + y + z };
    ((x / total * 10000.0).round() / 10000.0, (y / total * 10000.0).round() / 10000.0)
}

fn xy_to_hsbk(x: f64, y: f64, bri_pct: f64, kelvin: u16) -> [u16; 4] {
    let y_safe = if y <= 0.0 { 0.0001 } else { y };
    let yy = 1.0;
    let xx = (yy / y_safe) * x;
    let zz = (yy / y_safe) * (1.0 - x - y_safe);
    let r = xx * 1.656492 - yy * 0.354851 - zz * 0.255038;
    let g = -xx * 0.707196 + yy * 1.655397 + zz * 0.036152;
    let b = xx * 0.051713 - yy * 0.121364 + zz * 1.011530;
    let (r, g, b) = (
        ungamma(r).clamp(0.0, 1.0) as f32,
        ungamma(g).clamp(0.0, 1.0) as f32,
        ungamma(b).clamp(0.0, 1.0) as f32,
    );
    let (h, s, _v) = rgb_to_hsv(r, g, b);
    [
        (h * 65535.0) as u16,
        (s * 65535.0) as u16,
        (bri_pct.clamp(0.0, 100.0) / 100.0 * 65535.0) as u16,
        kelvin,
    ]
}

struct Cfg {
    ip: String,
    app_key: String,
}

pub struct Hue {
    cfg: RwLock<Option<Cfg>>,
    settings: PathBuf,
    /// Insecure-TLS agent for the bridge (self-signed certificate).
    agent: ureq::Agent,
    /// Normal agent for discovery.meethue.com.
    plain: ureq::Agent,
}

impl Hue {
    pub fn from_settings(root: &Path) -> Hue {
        let settings = root.join("hue.json");
        let mut cfg = None;
        if let Ok(text) = std::fs::read_to_string(&settings) {
            if let Ok(v) = serde_json::from_str::<Value>(&text) {
                let ip = v.get("ip").and_then(Value::as_str).unwrap_or("");
                let key = v.get("app_key").and_then(Value::as_str).unwrap_or("");
                if !ip.is_empty() && !key.is_empty() {
                    cfg = Some(Cfg {
                        ip: ip.to_string(),
                        app_key: key.to_string(),
                    });
                }
            }
        }
        let tls = native_tls::TlsConnector::builder()
            .danger_accept_invalid_certs(true)
            .danger_accept_invalid_hostnames(true)
            .build()
            .expect("tls connector");
        Hue {
            cfg: RwLock::new(cfg),
            settings,
            agent: ureq::AgentBuilder::new()
                .timeout(Duration::from_secs(6))
                .tls_connector(Arc::new(tls))
                .build(),
            plain: ureq::AgentBuilder::new()
                .timeout(Duration::from_secs(8))
                .build(),
        }
    }

    pub fn paired(&self) -> bool {
        self.cfg.read().unwrap().is_some()
    }

    fn save(&self, ip: &str, key: &str) {
        let _ = std::fs::write(
            &self.settings,
            serde_json::to_string_pretty(&json!({"ip": ip, "app_key": key}))
                .unwrap_or_default(),
        );
    }

    fn find_bridge_ip(&self) -> Option<String> {
        let resp = self.plain.get("https://discovery.meethue.com/").call().ok()?;
        let v: Value = resp.into_json().ok()?;
        v.get(0)?
            .get("internalipaddress")?
            .as_str()
            .map(String::from)
    }

    /// Press the bridge's round button, then call this. Retries for
    /// `attempts` seconds; `on_wait(remaining)` ticks each second.
    pub fn pair(&self, attempts: u32, on_wait: &dyn Fn(u32)) -> bool {
        let ip = {
            let cfg = self.cfg.read().unwrap();
            cfg.as_ref().map(|c| c.ip.clone())
        }
        .or_else(|| self.find_bridge_ip());
        let Some(ip) = ip else { return false };
        let body = json!({"devicetype": APP_NAME, "generateclientkey": true});
        for i in 0..attempts {
            if let Ok(resp) = self
                .agent
                .post(&format!("https://{ip}/api"))
                .send_json(body.clone())
            {
                if let Ok(v) = resp.into_json::<Value>() {
                    if let Some(user) = v
                        .get(0)
                        .and_then(|e| e.pointer("/success/username"))
                        .and_then(Value::as_str)
                    {
                        *self.cfg.write().unwrap() = Some(Cfg {
                            ip: ip.clone(),
                            app_key: user.to_string(),
                        });
                        self.save(&ip, user);
                        return true;
                    }
                }
            }
            on_wait(attempts - i);
            std::thread::sleep(Duration::from_secs(1));
        }
        false
    }

    // -- REST helpers ------------------------------------------------------

    fn get(&self, path: &str) -> Option<Value> {
        let cfg = self.cfg.read().unwrap();
        let c = cfg.as_ref()?;
        let url = format!("https://{}/clip/v2/resource/{path}", c.ip);
        let resp = self
            .agent
            .get(&url)
            .set("hue-application-key", &c.app_key)
            .call()
            .ok()?;
        resp.into_json().ok()
    }

    fn put(&self, path: &str, payload: Value) {
        let cfg = self.cfg.read().unwrap();
        let Some(c) = cfg.as_ref() else { return };
        let url = format!("https://{}/clip/v2/resource/{path}", c.ip);
        let _ = self
            .agent
            .put(&url)
            .set("hue-application-key", &c.app_key)
            .send_json(payload);
    }

    fn apply_state(light: &Light, it: &Value) {
        light.set_power_state(
            it.pointer("/on/on").and_then(Value::as_bool).unwrap_or(false),
        );
        let bri = it
            .pointer("/dimming/brightness")
            .and_then(Value::as_f64)
            .unwrap_or(100.0);
        if let (Some(x), Some(y)) = (
            it.pointer("/color/xy/x").and_then(Value::as_f64),
            it.pointer("/color/xy/y").and_then(Value::as_f64),
        ) {
            light.set_hsbk(xy_to_hsbk(x, y, bri, 3500));
        } else {
            let mirek = it
                .pointer("/color_temperature/mirek")
                .and_then(Value::as_f64)
                .unwrap_or(250.0)
                .max(1.0);
            light.set_hsbk([0, 0, (bri / 100.0 * 65535.0) as u16, (1_000_000.0 / mirek) as u16]);
        }
    }

    // -- provider API ------------------------------------------------------

    pub fn discover(&self) -> Vec<Arc<Light>> {
        if !self.paired() {
            return Vec::new();
        }
        let bridge_ip = self
            .cfg
            .read()
            .unwrap()
            .as_ref()
            .map(|c| c.ip.clone())
            .unwrap_or_default();
        let Some(v) = self.get("light") else {
            return Vec::new();
        };
        let mut lights = Vec::new();
        for it in v.get("data").and_then(Value::as_array).unwrap_or(&Vec::new()) {
            let Some(rid) = it.get("id").and_then(Value::as_str) else {
                continue;
            };
            let label = it
                .pointer("/metadata/name")
                .and_then(Value::as_str)
                .unwrap_or("Hue")
                .to_string();
            let l = Light::new(
                Vendor::Hue,
                format!("hue:{rid}"),
                bridge_ip.clone(),
                443,
                0,
                label,
            );
            Self::apply_state(&l, it);
            lights.push(l);
        }
        lights
    }

    fn rid(light: &Light) -> &str {
        light.id.strip_prefix("hue:").unwrap_or(&light.id)
    }

    pub fn refresh(&self, light: &Light) {
        if let Some(v) = self.get(&format!("light/{}", Self::rid(light))) {
            if let Some(it) = v.get("data").and_then(Value::as_array).and_then(|a| a.first()) {
                Self::apply_state(light, it);
            }
        }
    }

    pub fn set_power(&self, light: &Light, on: bool, _duration_ms: u32) {
        self.put(&format!("light/{}", Self::rid(light)), json!({"on": {"on": on}}));
        light.set_power_state(on);
    }

    pub fn set_color(&self, light: &Light, h: u16, s: u16, b: u16, k: u16, duration_ms: u32) {
        let pct = ((b as f32 / 65535.0 * 100.0).round() as i64).max(1);
        let mut payload = json!({
            "on": {"on": true},
            "dimming": {"brightness": pct},
            "dynamics": {"duration": duration_ms},
        });
        if s <= 300 {
            let mirek = (1_000_000_f64 / (k.max(1) as f64)).round().clamp(153.0, 500.0);
            payload["color_temperature"] = json!({"mirek": mirek as i64});
        } else {
            let (x, y) = hsbk_to_xy(h, s);
            payload["color"] = json!({"xy": {"x": x, "y": y}});
        }
        self.put(&format!("light/{}", Self::rid(light)), payload);
        light.set_hsbk([h, s, b, k]);
    }

    pub fn set_brightness(&self, light: &Light, pct: f32, duration_ms: u32) {
        self.put(
            &format!("light/{}", Self::rid(light)),
            json!({
                "on": {"on": true},
                "dimming": {"brightness": (pct.clamp(1.0, 100.0)) as i64},
                "dynamics": {"duration": duration_ms},
            }),
        );
        let [h, s, _b, k] = light.hsbk();
        light.set_hsbk([h, s, (pct / 100.0 * 65535.0) as u16, k]);
    }

    pub fn set_white(&self, light: &Light, pct: f32, kelvin: u16, duration_ms: u32) {
        let mirek = (1_000_000_f64 / (kelvin.max(1) as f64)).round().clamp(153.0, 500.0);
        self.put(
            &format!("light/{}", Self::rid(light)),
            json!({
                "on": {"on": true},
                "dimming": {"brightness": (pct.clamp(1.0, 100.0)) as i64},
                "color_temperature": {"mirek": mirek as i64},
                "dynamics": {"duration": duration_ms},
            }),
        );
        light.set_hsbk([0, 0, (pct / 100.0 * 65535.0) as u16, kelvin]);
    }

    pub fn stream_color(&self, light: &Light, h: u16, s: u16, b: u16, k: u16, d: u32) {
        // The bridge rate-limits REST writes (~10/s); best-effort as in Python.
        self.set_color(light, h, s, b, k, d);
    }
}
