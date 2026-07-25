//! Custom widgets: the VST-style rotary knob (270-degree arc, pointer,
//! digital readout - drag up/down, scroll wheel, double-click to reset) and
//! the iOS-style sliding toggle. Ported from the Tk canvases in lifx_gui.py.

use egui::{Align2, Color32, FontId, Pos2, Sense, Shape, Stroke, Ui, Vec2};

use crate::skin;
use crate::theme;

const SWEEP: f32 = 270.0;
const START: f32 = 225.0; // degrees, 0 = east, CCW positive (Tk convention)

pub struct KnobResult {
    /// Value confirmed by releasing a drag or double-clicking.
    pub committed: Option<f32>,
    /// Value nudged by the scroll wheel this frame (caller debounces).
    pub wheeled: bool,
    /// Where it landed, for callers that draw connections between knobs.
    pub rect: egui::Rect,
}

fn arc_points(c: Pos2, r: f32, start_deg: f32, sweep_deg: f32) -> Vec<Pos2> {
    let n = 40;
    (0..=n)
        .map(|i| {
            let a = (start_deg + sweep_deg * i as f32 / n as f32).to_radians();
            Pos2::new(c.x + r * a.cos(), c.y - r * a.sin())
        })
        .collect()
}

pub fn knob(
    ui: &mut Ui,
    value: &mut f32,
    min: f32,
    max: f32,
    size: f32,
    accent: Color32,
    default_value: f32,
    fmt: impl Fn(f32) -> String,
) -> KnobResult {
    let (rect, response) =
        ui.allocate_exact_size(Vec2::splat(size), Sense::click_and_drag());
    let mut committed = None;
    let mut wheeled = false;
    let span = (max - min).max(1e-6);

    if response.double_clicked() {
        *value = default_value;
        committed = Some(default_value);
    } else if response.dragged() {
        let delta = -response.drag_delta().y / 200.0 * span; // 200px = range
        *value = (*value + delta).clamp(min, max);
    } else if response.drag_stopped() {
        committed = Some(*value);
    }
    if response.hovered() {
        let scroll = ui.input(|i| i.raw_scroll_delta.y);
        if scroll.abs() > 0.0 {
            let step = span * 0.04 * scroll.signum();
            *value = (*value + step).clamp(min, max);
            wheeled = true;
        }
    }

    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        let c = rect.center();
        let f = ((*value - min) / span).clamp(0.0, 1.0);

        // metallic bezel: a round brushed-steel disc (circular, so the corners
        // stay transparent and blend into the panel behind it)
        let bez_r = size / 2.0 - 2.0;
        painter.add(skin::vgrad_circle(
            c,
            bez_r,
            theme::steel_top(),
            theme::steel_bot(),
        ));
        painter.circle_stroke(c, bez_r, Stroke::new(1.0, Color32::from_black_alpha(140)));

        // recessed face + top bevel highlight
        let face_r = size / 2.0 - 7.0;
        painter.circle_filled(c, face_r, theme::knob_face());
        painter.circle_stroke(c, face_r, Stroke::new(1.0, Color32::from_black_alpha(120)));
        // glassy sheen: a soft highlight blob in the upper half of the face
        painter.circle_filled(
            Pos2::new(c.x, c.y - face_r * 0.42),
            face_r * 0.55,
            Color32::from_white_alpha(16),
        );

        // track + value arc
        let arc_r = size / 2.0 - 7.0;
        painter.add(Shape::line(
            arc_points(c, arc_r, START, -SWEEP),
            Stroke::new(4.0, theme::track()),
        ));
        if f > 0.0 {
            painter.add(Shape::line(
                arc_points(c, arc_r, START, -SWEEP * f),
                Stroke::new(4.0, accent),
            ));
        }

        // pointer
        let ang = (START - SWEEP * f).to_radians();
        let dir = Vec2::new(ang.cos(), -ang.sin());
        painter.line_segment(
            [c + dir * (size / 2.0 - 18.0), c + dir * (size / 2.0 - 9.0)],
            Stroke::new(3.0, Color32::WHITE),
        );

        // digital readout
        painter.text(
            c,
            Align2::CENTER_CENTER,
            fmt(*value),
            FontId::monospace(9.0),
            theme::text(),
        );
    }

    KnobResult { committed, wheeled, rect }
}

/// iOS-style sliding on/off switch. Returns true when the user toggled it.
pub fn toggle(ui: &mut Ui, id: egui::Id, on: &mut bool) -> bool {
    let (rect, response) = ui.allocate_exact_size(Vec2::new(48.0, 26.0), Sense::click());
    let mut changed = false;
    if response.clicked() {
        *on = !*on;
        changed = true;
    }
    if ui.is_rect_visible(rect) {
        let pos = ui
            .ctx()
            .animate_value_with_time(id, if *on { 1.0 } else { 0.0 }, 0.18);
        let painter = ui.painter();
        let off = theme::toggle_off();
        let green = theme::green();
        let mix = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * pos) as u8;
        let fill = Color32::from_rgb(
            mix(off.r(), green.r()),
            mix(off.g(), green.g()),
            mix(off.b(), green.b()),
        );
        let track = egui::Rect::from_min_max(
            rect.min + Vec2::new(3.0, 4.0),
            rect.min + Vec2::new(45.0, 22.0),
        );
        painter.rect_filled(track, 9.0, fill);
        let kx = rect.min.x + 13.0 + 22.0 * pos;
        painter.circle_filled(Pos2::new(kx, rect.min.y + 13.0), 8.0, Color32::WHITE);
    }
    changed
}
