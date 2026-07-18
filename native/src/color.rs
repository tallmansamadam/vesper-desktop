//! Colour conversions shared by every provider (HSBK is the app's lingua
//! franca, as in the Python original).

pub fn rgb_to_hsv(r: f32, g: f32, b: f32) -> (f32, f32, f32) {
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let v = max;
    let d = max - min;
    let s = if max <= 0.0 { 0.0 } else { d / max };
    let h = if d <= 0.0 {
        0.0
    } else if max == r {
        (((g - b) / d) % 6.0) / 6.0
    } else if max == g {
        ((b - r) / d + 2.0) / 6.0
    } else {
        ((r - g) / d + 4.0) / 6.0
    };
    (h.rem_euclid(1.0), s, v)
}

pub fn hsv_to_rgb(h: f32, s: f32, v: f32) -> (f32, f32, f32) {
    let h = h.rem_euclid(1.0) * 6.0;
    let i = h.floor();
    let f = h - i;
    let p = v * (1.0 - s);
    let q = v * (1.0 - s * f);
    let t = v * (1.0 - s * (1.0 - f));
    match i as i32 % 6 {
        0 => (v, t, p),
        1 => (q, v, p),
        2 => (p, v, t),
        3 => (p, q, v),
        4 => (t, p, v),
        _ => (v, p, q),
    }
}

/// RGB (0..255) -> LIFX-style HSBK (each 0..65535, kelvin in K).
pub fn rgb_to_hsbk(r: u8, g: u8, b: u8, kelvin: u16) -> [u16; 4] {
    let (h, s, v) = rgb_to_hsv(r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0);
    [
        (h * 65535.0) as u16,
        (s * 65535.0) as u16,
        (v * 65535.0) as u16,
        kelvin,
    ]
}

/// HSBK -> RGB (0..255) for display. Kelvin is ignored, as in the original.
pub fn hsbk_to_rgb(h: u16, s: u16, b: u16) -> (u8, u8, u8) {
    let (r, g, bl) = hsv_to_rgb(
        h as f32 / 65535.0,
        s as f32 / 65535.0,
        b as f32 / 65535.0,
    );
    ((r * 255.0) as u8, (g * 255.0) as u8, (bl * 255.0) as u8)
}
