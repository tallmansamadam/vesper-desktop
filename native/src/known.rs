//! Lights remembered between runs (`lights.json` in the config dir), so
//! startup reconnects to them by address instead of waiting on broadcast
//! discovery - which a VPN, AP isolation or a busy network can starve.
//!
//! Only the bridge-free vendors are kept: Hue and Nanoleaf already reconnect
//! from their own paired credentials. Discovery still runs afterwards to pick
//! up new bulbs and DHCP address changes, and its results update this file.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

use crate::config;
use crate::light::{Light, Vendor};

/// Forget a light that hasn't answered for this long.
const FORGET_AFTER_SECS: u64 = 60 * 24 * 3600;

#[derive(Clone)]
pub struct Known {
    pub vendor: Vendor,
    pub id: String,
    pub ip: String,
    pub label: String,
    pub last_seen: u64,
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn remembered(v: Vendor) -> bool {
    !matches!(v, Vendor::Hue | Vendor::Nanoleaf)
}

pub fn load() -> Vec<Known> {
    let Ok(text) = std::fs::read_to_string(config::file("lights.json")) else {
        return Vec::new();
    };
    let Ok(Value::Array(arr)) = serde_json::from_str::<Value>(config::strip_bom(&text)) else {
        return Vec::new();
    };
    arr.iter()
        .filter_map(|v| {
            let vendor = Vendor::from_name(v.get("vendor")?.as_str()?)?;
            Some(Known {
                vendor,
                id: v.get("id")?.as_str()?.to_string(),
                ip: v.get("ip")?.as_str()?.to_string(),
                label: v.get("label").and_then(Value::as_str).unwrap_or("").to_string(),
                last_seen: v.get("last_seen").and_then(Value::as_u64).unwrap_or(0),
            })
        })
        .filter(|k| remembered(k.vendor))
        .collect()
}

/// Fold the lights currently answering into the remembered set and save it.
/// Lights not seen this time are kept (a bulb switched off at the wall is
/// still yours) until they've been silent for `FORGET_AFTER_SECS`.
pub fn remember(lights: &[Arc<Light>]) {
    let t = now();
    let mut all = load();
    for l in lights.iter().filter(|l| remembered(l.vendor)) {
        let k = Known {
            vendor: l.vendor,
            id: l.id.clone(),
            ip: l.ip.clone(),
            label: l.label(),
            last_seen: t,
        };
        // One entry per device, and per address: if a bulb's old IP now
        // belongs to another bulb, the old entry is stale.
        all.retain(|x| x.id != k.id && x.ip != k.ip);
        all.push(k);
    }
    all.retain(|x| t.saturating_sub(x.last_seen) < FORGET_AFTER_SECS);
    all.sort_by(|a, b| a.label.to_lowercase().cmp(&b.label.to_lowercase()));
    let v: Vec<Value> = all
        .iter()
        .map(|k| {
            json!({
                "vendor": k.vendor.name(),
                "id": k.id,
                "ip": k.ip,
                "label": k.label,
                "last_seen": k.last_seen,
            })
        })
        .collect();
    let _ = std::fs::write(
        config::file("lights.json"),
        serde_json::to_string_pretty(&v).unwrap_or_default(),
    );
}
