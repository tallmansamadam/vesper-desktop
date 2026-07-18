//! The Vesper palette, matching lifx_gui.py exactly.

use egui::Color32;

pub const BG: Color32 = Color32::from_rgb(0x0d, 0x0f, 0x14);
pub const PANEL: Color32 = Color32::from_rgb(0x17, 0x1a, 0x22);
pub const PANEL_HI: Color32 = Color32::from_rgb(0x1e, 0x22, 0x2c);
pub const STROKE: Color32 = Color32::from_rgb(0x2a, 0x2f, 0x3b);
pub const TEXT: Color32 = Color32::from_rgb(0xec, 0xee, 0xf2);
pub const MUTED: Color32 = Color32::from_rgb(0x8b, 0x93, 0xa3);
pub const ACCENT: Color32 = Color32::from_rgb(0x4c, 0xc2, 0xff);
pub const ACCENT_DK: Color32 = Color32::from_rgb(0x2a, 0x86, 0xc4);
pub const GREEN: Color32 = Color32::from_rgb(0x3a, 0xd2, 0x9f);
pub const TRACK: Color32 = STROKE;
pub const WARM: Color32 = Color32::from_rgb(0xff, 0xcf, 0x8f);
pub const KNOB_FACE: Color32 = Color32::from_rgb(0x10, 0x13, 0x1a);
pub const TOGGLE_OFF: Color32 = Color32::from_rgb(0x39, 0x40, 0x4f);
pub const ERR: Color32 = Color32::from_rgb(0xff, 0x6b, 0x6b);

pub const PRESETS: [[u8; 3]; 9] = [
    [255, 45, 45], [255, 130, 0], [255, 220, 0], [0, 210, 80], [0, 200, 220],
    [40, 90, 255], [150, 0, 255], [255, 70, 160], [255, 240, 230],
];
