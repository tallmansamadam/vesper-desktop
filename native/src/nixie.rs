//! A Nixie-tube time selector: `00:00`, where each pair of digits is its own
//! roller you drag vertically (hours 00-23, minutes 00-59, both wrapping).
//!
//! Each tube is smoked glass with a warm cathode glow: the lit digit sits in
//! front of dimly-lit "ghost" neighbours, the way a real tube shows the whole
//! digit stack behind the one that's on. egui can't blur, so the glow is the
//! digit redrawn a few times at growing offsets and falling alpha - cheap, and
//! it reads correctly at these sizes.

use egui::{Align2, Color32, FontId, Pos2, Rect, Rounding, Sense, Stroke, Ui, Vec2};

use crate::skin;
use crate::theme;

const TUBE_W: f32 = 44.0;
const TUBE_H: f32 = 60.0;
const PX_PER_STEP: f32 = 14.0; // drag distance for one digit

const GLASS_TOP: Color32 = Color32::from_rgb(0x14, 0x15, 0x1b);
const GLASS_BOT: Color32 = Color32::from_rgb(0x07, 0x07, 0x0a);
const CATHODE: Color32 = Color32::from_rgb(0xff, 0xa8, 0x3c);

/// One tube showing `value` (00..=max), draggable. Returns true if it changed.
fn tube(ui: &mut Ui, id_src: &str, value: &mut f32, max: u32) -> bool {
    let (rect, resp) =
        ui.allocate_exact_size(Vec2::new(TUBE_W, TUBE_H), Sense::click_and_drag());
    let id = ui.make_persistent_id(id_src);
    let mut changed = false;

    // Drag: accumulate pixels so a slow drag still rolls smoothly, and wrap
    // around at both ends like a real roller.
    let span = max as f32 + 1.0;
    if resp.dragged() {
        let mut acc = ui.data_mut(|d| d.get_temp::<f32>(id).unwrap_or(0.0));
        acc += -resp.drag_delta().y;
        let steps = (acc / PX_PER_STEP).trunc();
        if steps != 0.0 {
            *value = (*value + steps).rem_euclid(span);
            acc -= steps * PX_PER_STEP;
            changed = true;
        }
        ui.data_mut(|d| d.insert_temp(id, acc));
    } else {
        ui.data_mut(|d| d.insert_temp(id, 0.0f32));
    }
    if resp.hovered() {
        let scroll = ui.input(|i| i.raw_scroll_delta.y);
        if scroll.abs() > 0.0 {
            *value = (*value + scroll.signum()).rem_euclid(span);
            changed = true;
        }
    }

    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        let v = value.round().rem_euclid(span) as u32;

        // smoked glass envelope
        painter.add(skin::vgrad(rect, GLASS_TOP, GLASS_BOT));
        painter.rect_stroke(rect, Rounding::same(4.0), Stroke::new(1.0, theme::edge()));

        // the digit stack behind the lit one, barely glowing
        let ghost = |off: i32, alpha: u8, dy: f32| {
            let g = ((v as i32 + off).rem_euclid(span as i32)) as u32;
            painter.text(
                Pos2::new(rect.center().x, rect.center().y + dy),
                Align2::CENTER_CENTER,
                format!("{g:02}"),
                FontId::monospace(17.0),
                Color32::from_rgba_unmultiplied(CATHODE.r(), CATHODE.g(), CATHODE.b(), alpha),
            );
        };
        ghost(-1, 26, -19.0);
        ghost(1, 26, 19.0);

        // cathode glow, then the lit digit
        let c = rect.center();
        for (r, a) in [(20.0, 26u8), (13.0, 30), (8.0, 34)] {
            painter.circle_filled(
                c,
                r,
                Color32::from_rgba_unmultiplied(CATHODE.r(), CATHODE.g(), CATHODE.b(), a),
            );
        }
        for (off, a) in [(1.6f32, 60u8), (0.9, 90), (0.0, 255)] {
            let col = if off == 0.0 {
                Color32::from_rgb(255, 214, 170)
            } else {
                Color32::from_rgba_unmultiplied(CATHODE.r(), CATHODE.g(), CATHODE.b(), a)
            };
            painter.text(
                Pos2::new(c.x + off, c.y + off),
                Align2::CENTER_CENTER,
                format!("{v:02}"),
                FontId::monospace(22.0),
                col,
            );
        }

        // glass highlight down the left edge, and the tube's metal base
        painter.add(skin::vgrad(
            Rect::from_min_max(rect.min, Pos2::new(rect.min.x + 6.0, rect.max.y)),
            Color32::from_white_alpha(16),
            Color32::from_white_alpha(0),
        ));
        painter.rect_filled(
            Rect::from_min_max(Pos2::new(rect.min.x, rect.max.y - 5.0), rect.max),
            Rounding::same(2.0),
            theme::steel_bot(),
        );
    }
    changed
}

/// `HH:MM` as two Nixie rollers. Returns true when either changed.
pub fn time(ui: &mut Ui, id_src: &str, hour: &mut f32, minute: &mut f32) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 3.0;
        changed |= tube(ui, &format!("{id_src}_h"), hour, 23);
        let (colon, _) = ui.allocate_exact_size(Vec2::new(10.0, TUBE_H), Sense::hover());
        if ui.is_rect_visible(colon) {
            for dy in [-6.0, 6.0] {
                ui.painter().circle_filled(
                    Pos2::new(colon.center().x, colon.center().y + dy),
                    2.0,
                    CATHODE,
                );
            }
        }
        changed |= tube(ui, &format!("{id_src}_m"), minute, 59);
    });
    changed
}
