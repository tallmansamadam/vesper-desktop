//! Theme table. Each entry is a complete look; every other colour the skin
//! needs (gradient stops, bevels, hover states, the steel knob bezel) is
//! derived from it, so a palette is nine colours rather than thirty.
//!
//! The active theme lives in an atomic, and the accessors below read it, so
//! any widget re-skins the moment it changes - no restart, no plumbing.

use egui::Color32;
use std::sync::atomic::{AtomicUsize, Ordering};

#[derive(Clone, Copy)]
pub struct Theme {
    pub name: &'static str,
    pub bg: [u8; 3],      // window base, darkest tone
    pub panel: [u8; 3],   // card / panel body
    pub edge: [u8; 3],    // bevels and outlines
    pub text: [u8; 3],
    pub muted: [u8; 3],   // captions
    pub accent: [u8; 3],  // primary (arcs, active buttons)
    pub accent2: [u8; 3], // secondary (volume, toggles)
    pub warm: [u8; 3],    // white-temperature dial
}

pub const THEMES: &[Theme] = &[
    Theme { name: "Vesper", bg: [13, 15, 20], panel: [30, 34, 45], edge: [60, 69, 87],
            text: [236, 238, 242], muted: [139, 147, 163], accent: [76, 194, 255],
            accent2: [58, 210, 159], warm: [255, 207, 143] },
    Theme { name: "Graphite", bg: [16, 17, 19], panel: [38, 40, 44], edge: [78, 82, 90],
            text: [238, 240, 243], muted: [150, 154, 162], accent: [190, 196, 208],
            accent2: [140, 200, 255], warm: [255, 214, 170] },
    Theme { name: "Amber CRT", bg: [18, 12, 4], panel: [48, 34, 12], edge: [104, 74, 26],
            text: [255, 236, 200], muted: [176, 142, 92], accent: [255, 176, 32],
            accent2: [255, 120, 40], warm: [255, 206, 140] },
    Theme { name: "Neon Violet", bg: [16, 8, 30], panel: [44, 28, 76], edge: [96, 66, 150],
            text: [240, 235, 255], muted: [156, 140, 196], accent: [180, 110, 255],
            accent2: [0, 229, 255], warm: [255, 200, 230] },
    Theme { name: "Vaporwave", bg: [24, 10, 40], panel: [58, 28, 76], edge: [122, 62, 140],
            text: [255, 240, 250], muted: [186, 146, 196], accent: [255, 110, 199],
            accent2: [110, 240, 255], warm: [255, 214, 236] },
    Theme { name: "Matrix", bg: [4, 12, 6], panel: [16, 44, 24], edge: [34, 92, 50],
            text: [214, 255, 224], muted: [120, 176, 138], accent: [60, 255, 110],
            accent2: [180, 255, 190], warm: [200, 255, 200] },
    Theme { name: "Ice", bg: [8, 16, 24], panel: [26, 48, 68], edge: [56, 96, 128],
            text: [230, 245, 255], muted: [140, 172, 198], accent: [120, 230, 255],
            accent2: [235, 250, 255], warm: [214, 236, 255] },
    Theme { name: "Blood Orange", bg: [22, 8, 8], panel: [58, 22, 16], edge: [116, 46, 32],
            text: [255, 232, 220], muted: [186, 134, 118], accent: [255, 92, 40],
            accent2: [255, 200, 60], warm: [255, 190, 150] },
    Theme { name: "Midnight", bg: [6, 12, 28], panel: [22, 36, 72], edge: [50, 76, 140],
            text: [226, 236, 255], muted: [136, 154, 196], accent: [90, 150, 255],
            accent2: [180, 210, 255], warm: [220, 226, 255] },
    Theme { name: "Deep Sea", bg: [4, 16, 20], panel: [14, 46, 54], edge: [30, 92, 104],
            text: [214, 250, 246], muted: [122, 172, 176], accent: [40, 220, 200],
            accent2: [130, 255, 240], warm: [190, 240, 236] },
    Theme { name: "Copper", bg: [20, 12, 8], panel: [60, 36, 22], edge: [122, 74, 44],
            text: [255, 234, 214], muted: [186, 146, 116], accent: [232, 140, 70],
            accent2: [255, 210, 150], warm: [255, 206, 160] },
    Theme { name: "Toxic Lime", bg: [12, 16, 4], panel: [36, 50, 12], edge: [74, 100, 26],
            text: [240, 255, 214], muted: [166, 186, 122], accent: [190, 255, 40],
            accent2: [255, 240, 90], warm: [236, 255, 170] },
    Theme { name: "Magenta Rush", bg: [20, 6, 20], panel: [60, 16, 58], edge: [122, 38, 116],
            text: [255, 230, 252], muted: [190, 138, 184], accent: [255, 60, 220],
            accent2: [120, 240, 255], warm: [255, 200, 240] },
    Theme { name: "Sakura", bg: [22, 12, 18], panel: [58, 32, 44], edge: [120, 70, 90],
            text: [255, 236, 244], muted: [194, 152, 168], accent: [255, 150, 190],
            accent2: [255, 205, 225], warm: [255, 214, 220] },
    Theme { name: "Forest", bg: [8, 14, 10], panel: [26, 46, 32], edge: [56, 92, 64],
            text: [226, 244, 230], muted: [138, 172, 146], accent: [110, 210, 120],
            accent2: [200, 230, 130], warm: [230, 240, 190] },
    Theme { name: "Paper White", bg: [232, 232, 236], panel: [246, 246, 250], edge: [188, 190, 198],
            text: [26, 28, 34], muted: [104, 108, 118], accent: [40, 120, 220],
            accent2: [0, 150, 120], warm: [190, 130, 40] },
];

static CURRENT: AtomicUsize = AtomicUsize::new(0);

pub fn set(i: usize) {
    CURRENT.store(i.min(THEMES.len() - 1), Ordering::Relaxed);
}

pub fn index() -> usize {
    CURRENT.load(Ordering::Relaxed).min(THEMES.len() - 1)
}

pub fn current() -> &'static Theme {
    &THEMES[index()]
}

// -- derivation helpers ----------------------------------------------------

fn c(v: [u8; 3]) -> Color32 {
    Color32::from_rgb(v[0], v[1], v[2])
}

/// Move a colour toward white by `f`.
fn lighten(v: [u8; 3], f: f32) -> Color32 {
    Color32::from_rgb(
        (v[0] as f32 + (255.0 - v[0] as f32) * f) as u8,
        (v[1] as f32 + (255.0 - v[1] as f32) * f) as u8,
        (v[2] as f32 + (255.0 - v[2] as f32) * f) as u8,
    )
}

/// Move a colour toward black by `f`.
fn darken(v: [u8; 3], f: f32) -> Color32 {
    let k = 1.0 - f.clamp(0.0, 1.0);
    Color32::from_rgb(
        (v[0] as f32 * k) as u8,
        (v[1] as f32 * k) as u8,
        (v[2] as f32 * k) as u8,
    )
}

/// True when the active theme is a light one, so bevels and glosses can flip
/// (a white sheen on a white panel is invisible).
pub fn is_light() -> bool {
    let t = current();
    (t.bg[0] as u32 + t.bg[1] as u32 + t.bg[2] as u32) > 3 * 128
}

// -- the palette the rest of the app reads ---------------------------------

pub fn bg() -> Color32 { c(current().bg) }
pub fn panel() -> Color32 { c(current().panel) }
pub fn text() -> Color32 { c(current().text) }
pub fn muted() -> Color32 { c(current().muted) }
pub fn accent() -> Color32 { c(current().accent) }
pub fn green() -> Color32 { c(current().accent2) }
pub fn warm() -> Color32 { c(current().warm) }
pub fn edge() -> Color32 { c(current().edge) }
pub fn stroke() -> Color32 { c(current().edge) }
pub fn track() -> Color32 { darken(current().edge, 0.25) }

pub fn accent_dk() -> Color32 { darken(current().accent, 0.42) }
pub fn accent_hi() -> Color32 { lighten(current().accent, 0.35) }

pub fn bg_top() -> Color32 { lighten(current().bg, 0.07) }
pub fn bg_bot() -> Color32 { darken(current().bg, 0.30) }
pub fn panel_top() -> Color32 { lighten(current().panel, 0.10) }
pub fn panel_bot() -> Color32 { darken(current().panel, 0.42) }
pub fn panelhi_top() -> Color32 { lighten(current().panel, 0.18) }
pub fn panelhi_bot() -> Color32 { darken(current().panel, 0.30) }
pub fn btn_top() -> Color32 { lighten(current().panel, 0.20) }
pub fn btn_bot() -> Color32 { darken(current().panel, 0.28) }
pub fn btn_top_hover() -> Color32 { lighten(current().panel, 0.34) }
pub fn btn_bot_hover() -> Color32 { darken(current().panel, 0.12) }
pub fn steel_top() -> Color32 { lighten(current().edge, 0.22) }
pub fn steel_bot() -> Color32 { darken(current().edge, 0.58) }
pub fn knob_face() -> Color32 { darken(current().bg, 0.35) }
pub fn toggle_off() -> Color32 { lighten(current().panel, 0.12) }

pub fn err() -> Color32 {
    if is_light() {
        Color32::from_rgb(190, 40, 40)
    } else {
        Color32::from_rgb(255, 107, 107)
    }
}

pub const PRESETS: [[u8; 3]; 9] = [
    [255, 45, 45], [255, 130, 0], [255, 220, 0], [0, 210, 80], [0, 200, 220],
    [40, 90, 255], [150, 0, 255], [255, 70, 160], [255, 240, 230],
];
