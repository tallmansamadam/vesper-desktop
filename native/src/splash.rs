//! The launch animation, the desktop counterpart to the mobile splash.dart:
//! the dial sweeps up from its baseline, the arc filling with spectrum colour
//! as the pointer traces it, the core igniting from ember to white-hot, and
//! the space around it flooding with whatever hue the dial is currently on.
//!
//! Painted as a foreground overlay so the app can lay itself out underneath
//! (and finish discovery) while it plays, then cross-fades away. egui has no
//! additive blending, so the glows are layered translucent circles - on the
//! near-black backdrop that reads the same way.

use std::time::Instant;

use egui::{Color32, Id, Order, Pos2, Sense, Stroke, Vec2};

use crate::color::hsv_to_rgb;
use crate::theme;

const SWEEP_MS: f32 = 1500.0;
const HOLD_MS: f32 = 250.0;
const FADE_MS: f32 = 380.0;

const START_DEG: f32 = 135.0; // lower-left, clockwise (y down)
const SWEEP_DEG: f32 = 270.0;
const HUE_FROM: f32 = 0.02;
const HUE_TO: f32 = 0.80;

fn hue_at(t: f32) -> Color32 {
    let h = HUE_FROM + (HUE_TO - HUE_FROM) * t.clamp(0.0, 1.0);
    let (r, g, b) = hsv_to_rgb(h, 0.95, 1.0);
    Color32::from_rgb((r * 255.0) as u8, (g * 255.0) as u8, (b * 255.0) as u8)
}

/// The dial's travel: rest at exactly zero for the first tenth, then ease from
/// zero velocity. (An ease-*out* leaves the baseline at full speed, which
/// reads as the animation starting partway up the sweep.)
fn sweep_curve(t: f32) -> f32 {
    let t = ((t - 0.10) / 0.90).clamp(0.0, 1.0);
    3.0 * t * t - 2.0 * t * t * t
}

fn ease_out(t: f32) -> f32 {
    1.0 - (1.0 - t.clamp(0.0, 1.0)).powi(3)
}

/// A soft radial glow: concentric translucent discs, brightest at the centre.
fn bloom(painter: &egui::Painter, c: Pos2, radius: f32, col: Color32, peak: f32) {
    const STEPS: usize = 16;
    for i in (1..=STEPS).rev() {
        let f = i as f32 / STEPS as f32;
        let a = peak * (1.0 - f).powf(1.6) / 2.2;
        if a <= 0.002 {
            continue;
        }
        painter.circle_filled(
            c,
            radius * f,
            Color32::from_rgba_unmultiplied(col.r(), col.g(), col.b(), (a * 255.0) as u8),
        );
    }
}

pub struct Splash {
    start: Instant,
    done: bool,
}

impl Splash {
    pub fn new() -> Splash {
        Splash {
            start: Instant::now(),
            done: false,
        }
    }

    /// Paint one frame of the overlay; a no-op once the animation has ended,
    /// so callers can simply call it every frame.
    pub fn show(&mut self, ctx: &egui::Context) {
        if self.done {
            return;
        }
        let ms = self.start.elapsed().as_secs_f32() * 1000.0;
        let p = sweep_curve((ms / SWEEP_MS).clamp(0.0, 1.0));
        let fade_start = SWEEP_MS + HOLD_MS;
        let alpha = if ms <= fade_start {
            1.0
        } else {
            1.0 - ((ms - fade_start) / FADE_MS).clamp(0.0, 1.0)
        };
        if alpha <= 0.0 {
            self.done = true;
            return;
        }

        let screen = ctx.screen_rect();
        egui::Area::new(Id::new("vesper_splash"))
            .order(Order::Foreground)
            .fixed_pos(screen.min)
            .interactable(true)
            .show(ctx, |ui| {
                // Swallow input while the splash covers the app, and let a
                // click skip it.
                let resp = ui.allocate_response(screen.size(), Sense::click());
                if resp.clicked() {
                    self.done = true;
                    return;
                }

                let a = |x: f32| (x.clamp(0.0, 1.0) * alpha * 255.0) as u8;
                let painter = ui.painter();
                let c = screen.center();
                let s = screen.width().min(screen.height()) * 0.62;
                let r = s * 0.335;
                let arc_w = s * 0.058;
                let swept = SWEEP_DEG * p;
                let hue_now = hue_at(p);

                // backdrop
                painter.rect_filled(
                    screen,
                    egui::Rounding::ZERO,
                    Color32::from_rgba_unmultiplied(
                        theme::BG_BOT.r(),
                        theme::BG_BOT.g(),
                        theme::BG_BOT.b(),
                        a(1.0),
                    ),
                );

                // the space around the dial, flooded with the live hue
                if p > 0.01 {
                    bloom(painter, c, s * (0.42 + 0.46 * p), hue_now, 0.55 * p * alpha);
                }

                // dial face
                painter.circle_filled(
                    c,
                    s * 0.262,
                    Color32::from_rgba_unmultiplied(0x12, 0x14, 0x1b, a(1.0)),
                );

                // spectrum arc: halo pass (wide, dim) then the crisp arc
                if p > 0.02 {
                    let steps = (200.0 * p).max(2.0) as usize;
                    let pt = |deg: f32| {
                        let rad = deg.to_radians();
                        Pos2::new(c.x + r * rad.cos(), c.y + r * rad.sin())
                    };
                    for (width, fade) in [(arc_w * 2.0, 0.22), (arc_w, 1.0)] {
                        for i in 0..steps {
                            let t0 = i as f32 / steps as f32;
                            let t1 = (i + 1) as f32 / steps as f32;
                            let col = hue_at(t0 * p);
                            painter.line_segment(
                                [pt(START_DEG + swept * t0), pt(START_DEG + swept * t1)],
                                Stroke::new(
                                    width,
                                    Color32::from_rgba_unmultiplied(
                                        col.r(),
                                        col.g(),
                                        col.b(),
                                        a(fade),
                                    ),
                                ),
                            );
                        }
                    }
                }

                // core ignites as the dial rises
                let k = ease_out((p * 1.15).min(1.0));
                bloom(painter, c, s * 0.20, Color32::from_rgb(255, 176, 80),
                      (0.25 + 0.75 * k) * 0.85 * alpha);
                bloom(painter, c, s * 0.105, Color32::from_rgb(255, 236, 190), k * 0.95 * alpha);
                bloom(painter, c, s * 0.055, Color32::WHITE, k * alpha);

                // pointer, its tip riding the band it draws
                let ang = (START_DEG + swept).to_radians();
                let dir = Vec2::new(ang.cos(), ang.sin());
                if p > 0.01 {
                    // the nib: colour deposited where the tip meets the ring
                    bloom(painter, c + dir * r, s * 0.085, hue_now, 0.85 * alpha);
                    bloom(painter, c + dir * r, s * 0.030, Color32::WHITE, 0.95 * alpha);
                }
                painter.line_segment(
                    [c + dir * (s * 0.145), c + dir * (s * 0.302)],
                    Stroke::new(
                        s * 0.034,
                        Color32::from_rgba_unmultiplied(255, 255, 255, a(1.0)),
                    ),
                );

                // wordmark under the dial
                painter.text(
                    Pos2::new(c.x, c.y + s * 0.60),
                    egui::Align2::CENTER_CENTER,
                    "VESPER",
                    egui::FontId::proportional(s * 0.075),
                    Color32::from_rgba_unmultiplied(0xec, 0xee, 0xf2, a(0.55 * k)),
                );
            });

        ctx.request_repaint();
    }
}
