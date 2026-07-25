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

const TUBE_W: f32 = 54.0;
const TUBE_H: f32 = 72.0;
const PX_PER_STEP: f32 = 14.0; // drag distance for one digit
const DIGIT: f32 = 27.0;

const GLASS_TOP: Color32 = Color32::from_rgb(0x14, 0x15, 0x1b);
const GLASS_BOT: Color32 = Color32::from_rgb(0x07, 0x07, 0x0a);
/// The neon-orange of a lit cathode, and the near-white core of the glyph
/// itself - a real tube's filament reads brighter than its halo.
const CATHODE: Color32 = Color32::from_rgb(0xff, 0x9a, 0x2e);
const FILAMENT: Color32 = Color32::from_rgb(0xff, 0xe2, 0xb4);

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
        let c = rect.center();
        let amber = |a: u8| {
            Color32::from_rgba_unmultiplied(CATHODE.r(), CATHODE.g(), CATHODE.b(), a)
        };

        // Glass envelope, and the round face of the tube you're looking into.
        painter.add(skin::vgrad(rect, GLASS_TOP, GLASS_BOT));
        painter.rect_stroke(rect, Rounding::same(8.0), Stroke::new(1.0, theme::edge()));
        painter.circle_filled(c, TUBE_W * 0.46, Color32::from_rgb(0x05, 0x05, 0x07));

        // The unlit digits sit *behind* the lit one - a Nixie stacks its
        // cathodes front-to-back, so they recede in depth rather than sitting
        // above and below like a mechanical reel. Smaller and dimmer reads as
        // "further into the tube" while keeping the face pointed at you.
        for (off, scale, alpha) in [(2i32, 0.66f32, 12u8), (1, 0.82, 20)] {
            let g = ((v as i32 + off).rem_euclid(span as i32)) as u32;
            painter.text(
                Pos2::new(c.x, c.y - 2.0 * scale),
                Align2::CENTER_CENTER,
                format!("{g:02}"),
                FontId::monospace(DIGIT * scale),
                amber(alpha),
            );
        }

        // Cathode halo: soft and wide, kept well under the glyph so it haloes
        // the digit instead of washing it out.
        for (r, a) in [(TUBE_W * 0.44, 14u8), (TUBE_W * 0.30, 16), (TUBE_W * 0.18, 18)] {
            painter.circle_filled(c, r, amber(a));
        }

        // The lit digit: one soft bloom behind (same glyph, larger, centred -
        // so it blooms rather than smears) and the crisp filament on top.
        painter.text(
            c,
            Align2::CENTER_CENTER,
            format!("{v:02}"),
            FontId::monospace(DIGIT + 4.0),
            amber(70),
        );
        painter.text(
            c,
            Align2::CENTER_CENTER,
            format!("{v:02}"),
            FontId::monospace(DIGIT),
            FILAMENT,
        );

        // The fine anode mesh that sits in front of the cathodes, and the
        // curve of the glass catching light down the left.
        let mut x = rect.min.x + 5.0;
        while x < rect.max.x - 4.0 {
            painter.line_segment(
                [Pos2::new(x, rect.min.y + 4.0), Pos2::new(x, rect.max.y - 8.0)],
                Stroke::new(0.5, Color32::from_white_alpha(6)),
            );
            x += 5.0;
        }
        painter.add(skin::vgrad(
            Rect::from_min_max(rect.min, Pos2::new(rect.min.x + 7.0, rect.max.y)),
            Color32::from_white_alpha(20),
            Color32::from_white_alpha(0),
        ));

        // metal base
        painter.rect_filled(
            Rect::from_min_max(Pos2::new(rect.min.x, rect.max.y - 6.0), rect.max),
            Rounding::same(3.0),
            theme::steel_bot(),
        );
    }
    changed
}

/// `HH:MM` as two Nixie rollers, captioned so it's clear what each sets and
/// that they're draggable. Returns true when either changed.
pub fn time(ui: &mut Ui, id_src: &str, hour: &mut f32, minute: &mut f32) -> bool {
    let mut changed = false;
    ui.vertical(|ui| {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            changed |= tube(ui, &format!("{id_src}_h"), hour, 23);
            let (colon, _) = ui.allocate_exact_size(Vec2::new(12.0, TUBE_H), Sense::hover());
            if ui.is_rect_visible(colon) {
                for dy in [-9.0, 9.0] {
                    // glow, then the lit pip
                    ui.painter().circle_filled(
                        Pos2::new(colon.center().x, colon.center().y + dy),
                        5.0,
                        Color32::from_rgba_unmultiplied(
                            CATHODE.r(), CATHODE.g(), CATHODE.b(), 30,
                        ),
                    );
                    ui.painter().circle_filled(
                        Pos2::new(colon.center().x, colon.center().y + dy),
                        2.2,
                        FILAMENT,
                    );
                }
            }
            changed |= tube(ui, &format!("{id_src}_m"), minute, 59);
        });
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            let cap = |ui: &mut Ui, w: f32, t: &str| {
                let (r, _) = ui.allocate_exact_size(Vec2::new(w, 10.0), Sense::hover());
                ui.painter().text(
                    Pos2::new(r.center().x, r.min.y),
                    Align2::CENTER_TOP,
                    t,
                    FontId::proportional(8.0),
                    theme::muted(),
                );
            };
            cap(ui, TUBE_W, "HOUR");
            ui.add_space(12.0);
            cap(ui, TUBE_W, "MIN");
        });
    });
    changed
}
