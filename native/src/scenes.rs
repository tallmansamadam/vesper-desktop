//! Scene presets, ambience mapping and scene application, ported verbatim
//! from scenes.py. A scene is pure data; palette assignment is re-shuffled
//! on every application so re-clicking a scene re-mixes the bulbs.

use std::sync::Arc;

use crate::color::rgb_to_hsbk;
use crate::controller::Controller;
use crate::light::Light;

#[derive(Clone, Copy, PartialEq)]
pub enum Kind {
    White,
    Color,
    Palette,
}

pub struct Scene {
    pub name: &'static str,
    pub kind: Kind,
    pub pct: f32,
    pub kelvin: u16,
    pub rgb: [u8; 3],
    pub colors: &'static [[u8; 3]],
}

const fn white(name: &'static str, pct: f32, kelvin: u16) -> Scene {
    Scene { name, kind: Kind::White, pct, kelvin, rgb: [255, 255, 255], colors: &[] }
}

const fn color(name: &'static str, pct: f32, rgb: [u8; 3]) -> Scene {
    Scene { name, kind: Kind::Color, pct, kelvin: 3500, rgb, colors: &[] }
}

const fn palette(name: &'static str, pct: f32, colors: &'static [[u8; 3]]) -> Scene {
    Scene { name, kind: Kind::Palette, pct, kelvin: 3500, rgb: [255, 255, 255], colors }
}

pub static SCENES: &[Scene] = &[
    white("Relax", 28.0, 2700),
    white("Reading", 70.0, 3200),
    white("Focus", 100.0, 4800),
    white("Energize", 100.0, 6500),
    color("Candlelight", 18.0, [255, 110, 20]),
    color("Nightlight", 3.0, [255, 80, 10]),
    color("Cinema", 12.0, [45, 0, 95]),
    palette("Fireplace", 35.0, &[[255, 70, 0], [255, 120, 20], [190, 35, 0]]),
    palette("Sunset", 55.0, &[[255, 94, 0], [255, 40, 70], [150, 0, 130]]),
    palette("Golden Hour", 60.0, &[[255, 175, 45], [255, 110, 30], [220, 60, 40]]),
    palette("Ocean", 60.0, &[[0, 80, 220], [0, 170, 190], [0, 210, 120]]),
    palette("Lagoon", 60.0, &[[0, 175, 150], [60, 195, 100], [20, 135, 120], [110, 180, 70]]),
    palette("Deep Sea", 42.0, &[[0, 40, 120], [0, 90, 150], [0, 140, 140]]),
    palette("Forest", 52.0, &[[30, 170, 40], [120, 190, 0], [0, 150, 95]]),
    palette("Aurora", 65.0, &[[0, 255, 150], [60, 120, 255], [170, 0, 255]]),
    palette("Galaxy", 55.0, &[[40, 0, 120], [120, 0, 200], [220, 0, 160]]),
    palette("Ultraviolet", 60.0, &[[80, 0, 200], [150, 0, 255], [220, 40, 255]]),
    palette("Berry", 60.0, &[[200, 0, 90], [150, 0, 140], [90, 0, 160]]),
    palette("Neon", 85.0, &[[255, 0, 130], [0, 200, 255], [175, 0, 255]]),
    palette("Citrus", 75.0, &[[255, 200, 0], [255, 120, 0], [180, 220, 0]]),
    palette("Tropical", 75.0, &[[255, 90, 0], [255, 0, 120], [0, 200, 180]]),
    palette("Rainbow", 80.0, &[
        [255, 0, 0], [255, 140, 0], [255, 230, 0],
        [0, 200, 60], [0, 120, 255], [150, 0, 255],
    ]),
    palette("Party", 82.0, &[
        [255, 0, 80], [0, 120, 255], [150, 0, 255], [0, 220, 80], [255, 150, 0],
    ]),
    palette("Sunrise", 60.0, &[[255, 160, 90], [255, 110, 150], [255, 205, 120]]),
    palette("Sakura", 60.0, &[[255, 170, 205], [255, 120, 175], [255, 205, 225]]),
    palette("Lavender", 50.0, &[[180, 150, 255], [205, 170, 240], [150, 115, 230]]),
    palette("Mint", 60.0, &[[120, 235, 200], [80, 220, 185], [170, 245, 215]]),
    palette("Storm", 55.0, &[[40, 60, 110], [70, 130, 210], [120, 205, 255]]),
    palette("Lava", 55.0, &[[255, 55, 0], [190, 15, 0], [255, 120, 25]]),
    white("Moonlight", 28.0, 8000),
    palette("Twilight", 50.0, &[[60, 45, 135], [120, 70, 185], [45, 65, 150]]),
    palette("Autumn", 55.0, &[[220, 90, 20], [200, 40, 20], [240, 170, 40]]),
    palette("Snow", 55.0, &[[180, 210, 255], [140, 180, 235], [220, 235, 255]]),
];

/// Approximate RGB tints for white colour temperatures (tile art).
fn white_tint(kelvin: u16) -> [u8; 3] {
    match kelvin {
        2700 => [255, 197, 143],
        3200 => [255, 214, 170],
        4800 => [255, 241, 224],
        6500 => [210, 230, 255],
        8000 => [205, 226, 255],
        _ => [255, 230, 200],
    }
}

fn mul(c: [u8; 3], f: f32) -> [u8; 3] {
    [
        (c[0] as f32 * f).clamp(0.0, 255.0) as u8,
        (c[1] as f32 * f).clamp(0.0, 255.0) as u8,
        (c[2] as f32 * f).clamp(0.0, 255.0) as u8,
    ]
}

fn tint(c: [u8; 3], f: f32) -> [u8; 3] {
    [
        (c[0] as f32 + (255.0 - c[0] as f32) * f) as u8,
        (c[1] as f32 + (255.0 - c[1] as f32) * f) as u8,
        (c[2] as f32 + (255.0 - c[2] as f32) * f) as u8,
    ]
}

/// Gradient stops for a scene's fallback tile art.
pub fn tile_stops(sc: &Scene) -> Vec<[u8; 3]> {
    match sc.kind {
        Kind::Palette => {
            let mut v = vec![mul(sc.colors[0], 0.55)];
            v.extend_from_slice(sc.colors);
            v
        }
        Kind::Color => vec![mul(sc.rgb, 0.45), sc.rgb, tint(sc.rgb, 0.4)],
        Kind::White => {
            let base = white_tint(sc.kelvin);
            vec![mul(base, 0.45), base, tint(base, 0.4)]
        }
    }
}

/// Scene name -> ambience key (mirrors scenes.py AMBIENCE).
pub fn scene_ambience(name: &str) -> &'static str {
    match name {
        "Relax" => "warm", "Reading" => "cafe", "Focus" => "rain",
        "Energize" => "birds", "Candlelight" => "fire", "Nightlight" => "night",
        "Cinema" => "drone", "Fireplace" => "fire", "Sunset" => "seagulls",
        "Golden Hour" => "cicadas", "Ocean" => "ocean", "Lagoon" => "stream",
        "Deep Sea" => "underwater", "Forest" => "birds", "Aurora" => "shimmer",
        "Galaxy" => "deep_space", "Ultraviolet" => "drone", "Berry" => "chimes",
        "Neon" => "city", "Citrus" => "cicadas", "Tropical" => "seagulls",
        "Rainbow" => "club", "Party" => "crowd", "Sunrise" => "birds",
        "Sakura" => "chimes", "Lavender" => "warm", "Mint" => "wind",
        "Storm" => "storm", "Lava" => "fire", "Moonlight" => "night",
        "Twilight" => "night", "Autumn" => "leaves", "Snow" => "wind",
        _ => "warm",
    }
}

/// Synth simulacrum when a real recording (<key>.ogg) is unavailable.
pub fn ambience_fallback(key: &str) -> Option<&'static str> {
    match key {
        "birds" => Some("forest"), "crowd" => Some("club"), "city" => Some("rain"),
        "cicadas" => Some("night"), "stream" => Some("ocean"),
        "seagulls" => Some("ocean"), "cafe" => Some("warm"),
        "chimes" => Some("shimmer"),
        _ => None,
    }
}

fn set_rgb_at(ctrl: &Controller, light: &Light, rgb: [u8; 3], pct: f32, duration_ms: u32) {
    let hsbk = rgb_to_hsbk(rgb[0], rgb[1], rgb[2], 3500);
    let bri = (pct.clamp(0.0, 100.0) / 100.0 * 65535.0) as u16;
    let kelvin = light.hsbk()[3];
    ctrl.set_color(light, hsbk[0], hsbk[1], bri, kelvin, duration_ms);
}

/// Spread a palette across n lights, shuffled so re-applying re-mixes it.
fn assignment(colors: &[[u8; 3]], n: usize, shuffle: bool) -> Vec<[u8; 3]> {
    let mut rng = fastrand::Rng::new();
    let mut out = Vec::with_capacity(n);
    while out.len() < n {
        let mut chunk = colors.to_vec();
        if shuffle {
            rng.shuffle(&mut chunk);
        }
        out.extend(chunk);
    }
    out.truncate(n);
    out
}

pub fn apply_scene(ctrl: &Controller, lights: &[Arc<Light>], sc: &Scene, shuffle: bool) {
    let duration = 800;
    let assign = if sc.kind == Kind::Palette {
        assignment(sc.colors, lights.len(), shuffle)
    } else {
        Vec::new()
    };
    for (i, l) in lights.iter().enumerate() {
        match sc.kind {
            Kind::White => ctrl.set_white(l, sc.pct, sc.kelvin, duration),
            Kind::Color => set_rgb_at(ctrl, l, sc.rgb, sc.pct, duration),
            Kind::Palette => set_rgb_at(ctrl, l, assign[i], sc.pct, duration),
        }
        ctrl.set_power(l, true, duration);
    }
}
