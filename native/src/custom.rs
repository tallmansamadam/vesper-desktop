//! User-defined scenes: capture the lights' current colours as a named scene,
//! persist them to custom_scenes.json next to the app's other config, and
//! re-apply them later. Stored as a palette (hue/sat per bulb at full value) +
//! a brightness, exactly like the built-in palette scenes, so applying one
//! distributes its colours across whatever lights are present.

use std::path::PathBuf;
use std::sync::Arc;

use serde_json::{json, Value};

use crate::color::{hsbk_to_rgb, rgb_to_hsbk};
use crate::controller::Controller;
use crate::light::Light;

pub struct CustomScene {
    pub name: String,
    pub pct: f32,
    pub colors: Vec<[u8; 3]>,
}

pub fn load(path: &PathBuf) -> Vec<CustomScene> {
    let mut out = Vec::new();
    let Ok(text) = std::fs::read_to_string(path) else {
        return out;
    };
    let Ok(Value::Array(arr)) = serde_json::from_str::<Value>(crate::config::strip_bom(&text))
    else {
        return out;
    };
    for it in arr {
        let name = it.get("name").and_then(Value::as_str).unwrap_or("").to_string();
        let pct = it.get("pct").and_then(Value::as_f64).unwrap_or(60.0) as f32;
        let colors = it
            .get("colors")
            .and_then(Value::as_array)
            .map(|cs| {
                cs.iter()
                    .filter_map(|c| {
                        let a = c.as_array()?;
                        Some([
                            a.first()?.as_u64()? as u8,
                            a.get(1)?.as_u64()? as u8,
                            a.get(2)?.as_u64()? as u8,
                        ])
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        if !name.is_empty() && !colors.is_empty() {
            out.push(CustomScene { name, pct, colors });
        }
    }
    out
}

pub fn save(path: &PathBuf, scenes: &[CustomScene]) {
    let arr: Vec<Value> = scenes
        .iter()
        .map(|s| {
            json!({
                "name": s.name,
                "pct": s.pct,
                "colors": s.colors.iter().map(|c| json!([c[0], c[1], c[2]])).collect::<Vec<_>>(),
            })
        })
        .collect();
    let _ = std::fs::write(
        path,
        serde_json::to_string_pretty(&Value::Array(arr)).unwrap_or_default(),
    );
}

/// Snapshot the current lights: each bulb's hue/sat at full value becomes a
/// palette entry, and the average brightness becomes the scene's level.
pub fn capture(name: String, lights: &[Arc<Light>]) -> CustomScene {
    let mut colors = Vec::with_capacity(lights.len());
    let mut bri_sum = 0.0;
    for l in lights {
        let [h, s, _b, _k] = l.hsbk();
        colors.push(hsbk_to_rgb(h, s, 65535).into());
        bri_sum += l.brightness_pct();
    }
    let pct = if lights.is_empty() {
        60.0
    } else {
        bri_sum / lights.len() as f32
    };
    CustomScene { name, pct, colors }
}

/// Apply a captured scene: spread its palette across the lights (shuffled), at
/// its stored brightness, and turn them on.
pub fn apply(ctrl: &Controller, lights: &[Arc<Light>], sc: &CustomScene, shuffle: bool) {
    if sc.colors.is_empty() || lights.is_empty() {
        return;
    }
    let mut order: Vec<usize> = (0..sc.colors.len()).collect();
    if shuffle {
        fastrand::Rng::new().shuffle(&mut order);
    }
    let bri = (sc.pct.clamp(0.0, 100.0) / 100.0 * 65535.0) as u16;
    for (i, l) in lights.iter().enumerate() {
        let c = sc.colors[order[i % order.len()]];
        let hsbk = rgb_to_hsbk(c[0], c[1], c[2], 3500);
        ctrl.set_color(l, hsbk[0], hsbk[1], bri, l.hsbk()[3], 800);
        ctrl.set_power(l, true, 800);
    }
}
