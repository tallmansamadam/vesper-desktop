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

const TUBE_W: f32 = 50.0;
const TUBE_H: f32 = 92.0;
const DOME: f32 = 13.0; // height of the domed glass cap
const BASE: f32 = 14.0; // height of the metal base
const PX_PER_STEP: f32 = 14.0; // drag distance for one digit
const DIGIT: f32 = 26.0;

const GLASS_TOP: Color32 = Color32::from_rgb(0x14, 0x15, 0x1b);
/// The neon-orange of a lit cathode, and the near-white core of the glyph
/// itself - a real tube's filament reads brighter than its halo.
const CATHODE: Color32 = Color32::from_rgb(0xff, 0x9a, 0x2e);
const FILAMENT: Color32 = Color32::from_rgb(0xff, 0xef, 0xd6);

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
        // The cathodes sit up in the glass, not level with the base.
        let c = Pos2::new(rect.center().x, rect.min.y + (TUBE_H - BASE) * 0.52);
        let amber = |a: u8| {
            Color32::from_rgba_unmultiplied(CATHODE.r(), CATHODE.g(), CATHODE.b(), a)
        };

        // An upright glass cylinder seen from the side: domed cap on top,
        // metal base at the bottom, and the body shaded dark-light-dark across
        // its width so it reads as round rather than as a flat panel.
        let body = Rect::from_min_max(
            Pos2::new(rect.min.x, rect.min.y + DOME * 0.5),
            Pos2::new(rect.max.x, rect.max.y - BASE),
        );
        let curve = [
            Color32::from_rgb(0x04, 0x04, 0x06),
            GLASS_TOP,
            Color32::from_rgb(0x1c, 0x1e, 0x26),
            GLASS_TOP,
            Color32::from_rgb(0x03, 0x03, 0x05),
        ];
        // dome first, then the body over it, so they read as one envelope
        let dome = Rect::from_min_max(
            Pos2::new(rect.min.x + 2.0, rect.min.y),
            Pos2::new(rect.max.x - 2.0, rect.min.y + DOME * 2.0),
        );
        painter.add(skin::hgrad_multi(dome, &curve));
        painter.rect_filled(
            Rect::from_min_max(dome.min, Pos2::new(dome.max.x, dome.center().y)),
            Rounding {
                nw: DOME,
                ne: DOME,
                sw: 0.0,
                se: 0.0,
            },
            Color32::TRANSPARENT,
        );
        painter.add(skin::hgrad_multi(body, &curve));
        // the darker interior the cathodes sit in
        let cavity = body.shrink2(Vec2::new(5.0, 6.0));
        painter.rect_filled(
            cavity,
            Rounding::same(6.0),
            Color32::from_rgb(0x05, 0x05, 0x07),
        );
        // glass outline, and the seam where the envelope meets the base
        painter.rect_stroke(
            body,
            Rounding::same(3.0),
            Stroke::new(1.0, Color32::from_black_alpha(120)),
        );

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

        // Cathode halo. Concentric discs with a falling alpha, not three fixed
        // rings: a few translucent circles have visible edges and read as a
        // flat orange disc behind the digit rather than light coming off it.
        const HALO: usize = 14;
        for i in (1..=HALO).rev() {
            let f = i as f32 / HALO as f32;
            let a = ((1.0 - f).powf(1.5) * 30.0) as u8 + 1;
            painter.circle_filled(c, TUBE_W * 0.52 * f, amber(a));
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

        // The fine anode mesh in front of the cathodes.
        let mut x = cavity.min.x + 4.0;
        while x < cavity.max.x - 2.0 {
            painter.line_segment(
                [Pos2::new(x, cavity.min.y + 2.0), Pos2::new(x, cavity.max.y - 2.0)],
                Stroke::new(0.5, Color32::from_white_alpha(7)),
            );
            x += 5.0;
        }

        // Specular streak down the left of the glass - the single strongest
        // cue that this is a cylinder standing up, not a flat plate.
        painter.add(skin::hgrad_multi(
            Rect::from_min_max(
                Pos2::new(rect.min.x + 5.0, rect.min.y + DOME * 0.6),
                Pos2::new(rect.min.x + 14.0, rect.max.y - BASE - 4.0),
            ),
            &[
                Color32::from_white_alpha(0),
                Color32::from_white_alpha(26),
                Color32::from_white_alpha(0),
            ],
        ));

        // Metal base with its collar and pins.
        let base = Rect::from_min_max(
            Pos2::new(rect.min.x + 3.0, rect.max.y - BASE),
            Pos2::new(rect.max.x - 3.0, rect.max.y - 3.0),
        );
        painter.add(skin::vgrad(base, theme::steel_top(), theme::steel_bot()));
        painter.rect_stroke(
            base,
            Rounding::same(2.0),
            Stroke::new(1.0, Color32::from_black_alpha(150)),
        );
        for i in 0..3 {
            let px = base.min.x + base.width() * (0.25 + 0.25 * i as f32);
            painter.line_segment(
                [Pos2::new(px, base.max.y), Pos2::new(px, rect.max.y)],
                Stroke::new(1.5, theme::steel_bot()),
            );
        }
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
                    FontId::proportional(9.5),
                    theme::caption(),
                );
            };
            cap(ui, TUBE_W, "HOUR");
            ui.add_space(12.0);
            cap(ui, TUBE_W, "MIN");
        });
    });
    changed
}
