//! Where Vesper keeps its state, and what it keeps.
//!
//! Settings used to live next to the executable, which is wrong everywhere: on
//! Windows the install directory may be read-only and roaming profiles won't
//! carry it, and on Unix it ignores the platform's config conventions
//! entirely. Everything now lives in the OS's own per-user config location,
//! and anything found beside the exe is migrated there once.

use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::noise::{self, NoiseKind};

/// Files that used to sit beside the executable and are moved on first run.
const OWNED: [&str; 5] = [
    "settings.json",
    "custom_scenes.json",
    "schedules.json",
    "hue.json",
    "nanoleaf.json",
];

/// The per-user config directory for this platform, created if missing:
/// `%APPDATA%\Vesper`, `~/Library/Application Support/Vesper`, or
/// `$XDG_CONFIG_HOME/vesper` (falling back to `~/.config/vesper`).
pub fn dir() -> PathBuf {
    let base = if cfg!(windows) {
        std::env::var_os("APPDATA").map(PathBuf::from)
    } else if cfg!(target_os = "macos") {
        std::env::var_os("HOME")
            .map(|h| PathBuf::from(h).join("Library").join("Application Support"))
    } else {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
    };
    let name = if cfg!(windows) || cfg!(target_os = "macos") {
        "Vesper"
    } else {
        "vesper"
    };
    let dir = match base {
        Some(b) => b.join(name),
        // Last resort: alongside the exe, which is where it used to live.
        None => PathBuf::from("."),
    };
    let _ = std::fs::create_dir_all(&dir);
    dir
}

/// Move any state left beside the executable into the config directory, once.
/// Existing files there win - a migration must never clobber newer state.
pub fn migrate_from(old: &Path) {
    let new = dir();
    if old == new {
        return;
    }
    for name in OWNED {
        let (from, to) = (old.join(name), new.join(name));
        if from.is_file() && !to.exists() {
            if std::fs::rename(&from, &to).is_err() {
                // Different volume, or the source is locked: copy instead.
                if std::fs::copy(&from, &to).is_ok() {
                    let _ = std::fs::remove_file(&from);
                }
            }
        }
    }
}

/// Everything the app remembers between runs. Light state deliberately isn't
/// here: the bulbs are the source of truth for that and are re-read on
/// discovery.
#[derive(Clone)]
pub struct Settings {
    pub theme: usize,
    pub volume: f32,
    pub scene_vol: f32,
    pub noise_vol: f32,
    pub sound_on: bool,
    pub speed: f32,
    pub timer_hour: f32,
    pub timer_min: f32,
    pub wake_hour: f32,
    pub wake_min: f32,
    pub wake_rise_min: f32,
    /// Level per noise layer, and which noise scene is showing (if any).
    pub noise_levels: Vec<f32>,
    pub noise_scene: Option<usize>,
}

impl Default for Settings {
    fn default() -> Settings {
        Settings {
            theme: 0,
            volume: 60.0,
            scene_vol: 100.0,
            noise_vol: 100.0,
            sound_on: true,
            speed: 100.0,
            timer_hour: 23.0,
            timer_min: 0.0,
            wake_hour: 7.0,
            wake_min: 0.0,
            wake_rise_min: 10.0,
            noise_levels: vec![0.0; noise::N],
            noise_scene: None,
        }
    }
}

/// Drop a UTF-8 byte-order mark if one is present. Plenty of editors (and
/// PowerShell's `Set-Content -Encoding utf8`) write one, and a JSON parser
/// rejects it - which would silently reset every setting the user had.
pub fn strip_bom(s: &str) -> &str {
    s.strip_prefix('\u{feff}').unwrap_or(s)
}

fn f(v: &Value, key: &str, default: f32) -> f32 {
    v.get(key).and_then(Value::as_f64).unwrap_or(default as f64) as f32
}

impl Settings {
    pub fn path() -> PathBuf {
        dir().join("settings.json")
    }

    pub fn load() -> Settings {
        let mut s = Settings::default();
        let Ok(text) = std::fs::read_to_string(Settings::path()) else {
            return s;
        };
        let Ok(v) = serde_json::from_str::<Value>(strip_bom(&text)) else {
            return s;
        };
        s.theme = v.get("theme").and_then(Value::as_u64).unwrap_or(0) as usize;
        s.volume = f(&v, "volume", s.volume);
        s.scene_vol = f(&v, "scene_vol", s.scene_vol);
        s.noise_vol = f(&v, "noise_vol", s.noise_vol);
        s.sound_on = v.get("sound_on").and_then(Value::as_bool).unwrap_or(true);
        s.speed = f(&v, "speed", s.speed);
        s.timer_hour = f(&v, "timer_hour", s.timer_hour);
        s.timer_min = f(&v, "timer_min", s.timer_min);
        s.wake_hour = f(&v, "wake_hour", s.wake_hour);
        s.wake_min = f(&v, "wake_min", s.wake_min);
        s.wake_rise_min = f(&v, "wake_rise_min", s.wake_rise_min);
        if let Some(arr) = v.get("noise_levels").and_then(Value::as_array) {
            for (i, x) in arr.iter().take(noise::N).enumerate() {
                s.noise_levels[i] = x.as_f64().unwrap_or(0.0) as f32;
            }
        }
        s.noise_scene = v
            .get("noise_scene")
            .and_then(Value::as_u64)
            .map(|i| i as usize)
            .filter(|i| *i < noise::SCENES.len());
        s
    }

    pub fn save(&self) {
        let v = json!({
            "theme": self.theme,
            "volume": self.volume,
            "scene_vol": self.scene_vol,
            "noise_vol": self.noise_vol,
            "sound_on": self.sound_on,
            "speed": self.speed,
            "timer_hour": self.timer_hour,
            "timer_min": self.timer_min,
            "wake_hour": self.wake_hour,
            "wake_min": self.wake_min,
            "wake_rise_min": self.wake_rise_min,
            "noise_levels": self.noise_levels,
            "noise_scene": self.noise_scene,
        });
        let _ = std::fs::write(
            Settings::path(),
            serde_json::to_string_pretty(&v).unwrap_or_default(),
        );
    }

    /// Read the live noise mix back off the engine, ready to save.
    pub fn capture_noise(&mut self, n: &noise::Noise) {
        self.noise_levels = noise::KINDS.iter().map(|k| n.level_pct(*k)).collect();
    }

    /// Restore a saved mix onto the engine.
    pub fn apply_noise(&self, n: &mut noise::Noise) {
        for (i, k) in noise::KINDS.iter().enumerate() {
            if let Some(v) = self.noise_levels.get(i) {
                n.set_level(*k, *v);
            }
        }
    }
}

/// Convenience for the providers, which each own one credential file.
pub fn file(name: &str) -> PathBuf {
    dir().join(name)
}

#[allow(dead_code)]
fn _kind_count_matches() {
    // Compile-time reminder that the saved vector is per-layer.
    const _: () = assert!(noise::N > 0);
    let _ = NoiseKind::White;
}
