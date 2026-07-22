//! Vesper - native multi-vendor smart light controller (Rust + egui).
//!
//! The Rust refactor of lifx_gui.py: same dark dial-driven UI, same six
//! vendors, same scenes/effects/screen-sync/ambience - but a single static
//! exe that cold-starts in tens of milliseconds.

#![windows_subsystem = "windows"]

mod ambience;
mod color;
mod controller;
mod effects;
mod govee;
mod hue;
mod knob;
mod lifx;
mod light;
mod nanoleaf;
mod scenes;
mod screen;
mod skin;
mod theme;
mod util;
mod wiz;
mod yeelight;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

use egui::{
    Align, Align2, Color32, FontId, Layout, Pos2, Rect, RichText, Rounding,
    Sense, Stroke, TextureHandle, Vec2,
};

use crate::ambience::Ambience;
use crate::color::hsbk_to_rgb;
use crate::controller::Controller;
use crate::effects::{Driver, Mode};
use crate::knob::{knob, toggle};
use crate::light::Light;
use crate::scenes::{apply_scene, scene_ambience, tile_stops, Scene, SCENES};

const TILE_W: f32 = 152.0;
const TILE_H: f32 = 84.0;

// --------------------------------------------------------------------------
// Cross-thread messages
// --------------------------------------------------------------------------

enum Msg {
    Status(String, bool),
    Discovered(Vec<Arc<Light>>),
    Refresh,
    Texture(&'static str, TextureHandle),
    PlayAmbience(&'static str),
    Rediscover,
}

/// Deferred user intents collected during the UI pass (avoids borrow
/// tangles) plus wheel-debounced commits.
enum Act {
    Discover,
    AllPower(bool),
    ToggleSound,
    PairHue,
    PairNano,
    Scene(usize),
    Motion(Mode),
    MotionOff,
    Power(Arc<Light>, bool),
    Bri(Arc<Light>, f32),
    White(Arc<Light>, f32, f32),
    Rgb(Arc<Light>, [u8; 3]),
    Master(f32),
}

// --------------------------------------------------------------------------
// Assets
// --------------------------------------------------------------------------

fn scene_slug(name: &str) -> String {
    let mut out = String::new();
    let mut last_us = true;
    for ch in name.to_lowercase().chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch);
            last_us = false;
        } else if !last_us {
            out.push('_');
            last_us = true;
        }
    }
    out.trim_end_matches('_').to_string()
}

fn gradient_image(sc: &Scene) -> egui::ColorImage {
    let (w, h) = (TILE_W as usize, TILE_H as usize);
    let stops = tile_stops(sc);
    let angle = (35 + (sc.name.bytes().map(|b| b as u32).sum::<u32>() % 6) * 11) as f32;
    let (ca, sa) = (angle.to_radians().cos(), angle.to_radians().sin());
    let mut pixels = Vec::with_capacity(w * h);
    for y in 0..h {
        for x in 0..w {
            let u = x as f32 / (w - 1) as f32 - 0.5;
            let v = y as f32 / (h - 1) as f32 - 0.5;
            let t = (u * ca + v * sa + 0.5).clamp(0.0, 1.0);
            let ft = t * (stops.len() - 1) as f32;
            let i = (ft as usize).min(stops.len() - 2);
            let f = ft - i as f32;
            let a = stops[i];
            let b = stops[i + 1];
            pixels.push(Color32::from_rgb(
                (a[0] as f32 + (b[0] as f32 - a[0] as f32) * f) as u8,
                (a[1] as f32 + (b[1] as f32 - a[1] as f32) * f) as u8,
                (a[2] as f32 + (b[2] as f32 - a[2] as f32) * f) as u8,
            ));
        }
    }
    egui::ColorImage { size: [w, h], pixels }
}

fn spawn_texture_loader(root: PathBuf, ctx: egui::Context, tx: Sender<Msg>) {
    std::thread::spawn(move || {
        for sc in SCENES {
            let path = root.join("images").join(format!("{}.png", scene_slug(sc.name)));
            let img = match image::open(&path) {
                Ok(im) => {
                    let rgba = im.to_rgba8();
                    let size = [rgba.width() as usize, rgba.height() as usize];
                    egui::ColorImage::from_rgba_unmultiplied(size, rgba.as_raw())
                }
                Err(_) => gradient_image(sc),
            };
            let tex = ctx.load_texture(sc.name, img, egui::TextureOptions::LINEAR);
            if tx.send(Msg::Texture(sc.name, tex)).is_err() {
                return;
            }
            ctx.request_repaint();
        }
    });
}

fn load_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    if let Ok(bytes) = std::fs::read("C:\\Windows\\Fonts\\bahnschrift.ttf") {
        fonts
            .font_data
            .insert("bahnschrift".into(), egui::FontData::from_owned(bytes));
        fonts
            .families
            .get_mut(&egui::FontFamily::Proportional)
            .unwrap()
            .insert(0, "bahnschrift".into());
    }
    if let Ok(bytes) = std::fs::read("C:\\Windows\\Fonts\\consola.ttf") {
        fonts
            .font_data
            .insert("consolas".into(), egui::FontData::from_owned(bytes));
        fonts
            .families
            .get_mut(&egui::FontFamily::Monospace)
            .unwrap()
            .insert(0, "consolas".into());
    }
    ctx.set_fonts(fonts);
}

fn load_icon(root: &Path) -> Option<egui::IconData> {
    let img = image::open(root.join("branding").join("vesper_icon_256.png")).ok()?;
    let rgba = img.to_rgba8();
    let (width, height) = (rgba.width(), rgba.height());
    Some(egui::IconData {
        rgba: rgba.into_raw(),
        width,
        height,
    })
}

// --------------------------------------------------------------------------
// App
// --------------------------------------------------------------------------

struct VesperApp {
    ctrl: Arc<Controller>,
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
    lights: Vec<Arc<Light>>,
    textures: HashMap<&'static str, TextureHandle>,
    status: String,
    status_err: bool,
    discovering: bool,
    sound_on: bool,
    master: f32,
    volume: f32,
    speed_ui: f32,
    driver: Driver,
    audio: Ambience,
    /// Per-light UI mirrors: id -> (brightness knob, white knob).
    knobs: HashMap<String, (f32, f32)>,
    toggles: HashMap<String, bool>,
    pickers: HashMap<String, [u8; 3]>,
    /// Wheel/picker-debounced commits: key -> (due, action).
    pending: HashMap<String, (Instant, Act)>,
}

impl VesperApp {
    fn new(cc: &eframe::CreationContext<'_>, root: PathBuf) -> VesperApp {
        load_fonts(&cc.egui_ctx);
        let mut visuals = egui::Visuals::dark();
        visuals.panel_fill = theme::BG;
        visuals.override_text_color = Some(theme::TEXT);
        cc.egui_ctx.set_visuals(visuals);

        let (tx, rx) = channel();
        let ctrl = Arc::new(Controller::new(&root));
        spawn_texture_loader(root.clone(), cc.egui_ctx.clone(), tx.clone());

        let mut app = VesperApp {
            ctrl,
            tx,
            rx,
            lights: Vec::new(),
            textures: HashMap::new(),
            status: "Ready.".into(),
            status_err: false,
            discovering: false,
            sound_on: true,
            master: 100.0,
            volume: 60.0,
            speed_ui: 100.0,
            driver: Driver::new(),
            audio: Ambience::new(root.join("audio")),
            knobs: HashMap::new(),
            toggles: HashMap::new(),
            pickers: HashMap::new(),
            pending: HashMap::new(),
        };
        app.spawn_discover(&cc.egui_ctx);
        app
    }

    // -- plumbing ----------------------------------------------------------

    fn set_status(&mut self, text: impl Into<String>, err: bool) {
        self.status = text.into();
        self.status_err = err;
    }

    fn sync_from_lights(&mut self) {
        for l in &self.lights {
            self.knobs.insert(
                l.id.clone(),
                (l.brightness_pct(), l.hsbk()[3].clamp(2500, 9000) as f32),
            );
            self.toggles.insert(l.id.clone(), l.power());
        }
    }

    fn spawn_discover(&mut self, ctx: &egui::Context) {
        if self.discovering {
            return;
        }
        self.discovering = true;
        self.driver.stop_current();
        self.set_status("Discovering lights on the LAN …", false);
        let ctrl = self.ctrl.clone();
        let tx = self.tx.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let lights = ctrl.discover();
            let _ = tx.send(Msg::Discovered(lights));
            ctx.request_repaint();
        });
    }

    /// Run `work(ctrl)` on a worker thread, then report `done` as status and
    /// refresh the cards.
    fn spawn_cmd(
        &self,
        ctx: &egui::Context,
        done: String,
        work: impl FnOnce(&Controller) + Send + 'static,
    ) {
        let ctrl = self.ctrl.clone();
        let tx = self.tx.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            work(&ctrl);
            let _ = tx.send(Msg::Status(done, false));
            let _ = tx.send(Msg::Refresh);
            ctx.request_repaint();
        });
    }

    fn act(&mut self, ctx: &egui::Context, act: Act) {
        match act {
            Act::Discover => self.spawn_discover(ctx),
            Act::AllPower(on) => {
                self.driver.stop_current();
                if self.lights.is_empty() {
                    self.set_status("No lights yet — hit Discover.", true);
                    return;
                }
                let lights = self.lights.clone();
                self.set_status(
                    format!("Turning all lights {} …", if on { "on" } else { "off" }),
                    false,
                );
                self.spawn_cmd(
                    ctx,
                    format!("All lights {}.", if on { "on" } else { "off" }),
                    move |c| {
                        for l in &lights {
                            c.set_power(l, on, 400);
                        }
                    },
                );
            }
            Act::ToggleSound => {
                self.sound_on = !self.sound_on;
                self.audio.set_enabled(self.sound_on);
            }
            Act::PairHue => {
                self.set_status("Finding Hue bridge — press its round button now …", false);
                let ctrl = self.ctrl.clone();
                let tx = self.tx.clone();
                let ctx2 = ctx.clone();
                std::thread::spawn(move || {
                    let ok = ctrl.hue.pair(30, &|rem| {
                        let _ = tx.send(Msg::Status(
                            format!("Press the Hue bridge button … {rem}s"),
                            false,
                        ));
                        ctx2.request_repaint();
                    });
                    if ok {
                        let _ = tx.send(Msg::Status(
                            "Hue bridge paired — re-discovering …".into(),
                            false,
                        ));
                        let _ = tx.send(Msg::Rediscover);
                    } else {
                        let _ = tx.send(Msg::Status(
                            "Hue pairing timed out. Press the bridge button and hit \
                             + Hue again."
                                .into(),
                            true,
                        ));
                    }
                    ctx2.request_repaint();
                });
            }
            Act::PairNano => {
                self.set_status(
                    "Hold the Nanoleaf power button 5-7s until the LEDs flash …",
                    false,
                );
                let ctrl = self.ctrl.clone();
                let tx = self.tx.clone();
                let ctx2 = ctx.clone();
                std::thread::spawn(move || {
                    let n = ctrl.nanoleaf.pair(25, &|rem| {
                        let _ = tx.send(Msg::Status(
                            format!("Waiting for Nanoleaf pairing mode … {rem}s"),
                            false,
                        ));
                        ctx2.request_repaint();
                    });
                    if n > 0 {
                        let _ = tx.send(Msg::Status(
                            format!(
                                "Paired {n} Nanoleaf device{} — re-discovering …",
                                if n == 1 { "" } else { "s" }
                            ),
                            false,
                        ));
                        let _ = tx.send(Msg::Rediscover);
                    } else {
                        let _ = tx.send(Msg::Status(
                            "No Nanoleaf paired. Hold its power button 5-7s (LEDs \
                             flash), then hit + Nanoleaf."
                                .into(),
                            true,
                        ));
                    }
                    ctx2.request_repaint();
                });
            }
            Act::Scene(i) => {
                self.driver.stop_current();
                if self.lights.is_empty() {
                    self.set_status("No lights yet — hit Discover.", true);
                    return;
                }
                let sc = &SCENES[i];
                self.set_status(format!("Applying scene: {} …", sc.name), false);
                let lights = self.lights.clone();
                let name = sc.name;
                let tx = self.tx.clone();
                let ctrl = self.ctrl.clone();
                let ctx2 = ctx.clone();
                std::thread::spawn(move || {
                    apply_scene(&ctrl, &lights, &SCENES[i], true);
                    let _ = tx.send(Msg::PlayAmbience(scene_ambience(name)));
                    let _ = tx.send(Msg::Status(format!("Scene: {name}"), false));
                    let _ = tx.send(Msg::Refresh);
                    ctx2.request_repaint();
                });
            }
            Act::Motion(mode) => {
                if self.driver.mode == Some(mode) {
                    self.act(ctx, Act::MotionOff);
                    return;
                }
                if self.lights.is_empty() {
                    self.set_status("No lights yet — hit Discover.", true);
                    return;
                }
                self.driver.start(mode, self.ctrl.clone(), self.lights.clone());
                let label = mode.label();
                match mode {
                    Mode::Complement | Mode::Augment => self.set_status(
                        format!("Screen sync: {label} — matching your screen…"),
                        false,
                    ),
                    _ => self.set_status(format!("Effect: {label}"), false),
                }
            }
            Act::MotionOff => {
                self.driver.stop_current();
                self.sync_from_lights();
                self.set_status("Motion stopped.", false);
                // Re-read true bulb state: streaming leaves the cached state
                // at the last frame sent, not necessarily what stuck.
                let lights = self.lights.clone();
                self.spawn_cmd(ctx, String::new(), move |c| {
                    for l in &lights {
                        c.refresh(l);
                    }
                });
            }
            Act::Power(l, on) => {
                self.driver.stop_current();
                self.spawn_cmd(ctx, String::new(), move |c| c.set_power(&l, on, 400));
            }
            Act::Bri(l, v) => {
                self.driver.stop_current();
                self.spawn_cmd(ctx, String::new(), move |c| c.set_brightness(&l, v, 300));
            }
            Act::White(l, pct, k) => {
                self.driver.stop_current();
                self.spawn_cmd(ctx, String::new(), move |c| {
                    c.set_white(&l, pct, k as u16, 400)
                });
            }
            Act::Rgb(l, rgb) => {
                self.driver.stop_current();
                self.spawn_cmd(ctx, String::new(), move |c| {
                    c.set_rgb(&l, rgb[0], rgb[1], rgb[2], 400)
                });
            }
            Act::Master(v) => {
                self.driver.stop_current();
                if self.lights.is_empty() {
                    return;
                }
                let lights = self.lights.clone();
                self.spawn_cmd(
                    ctx,
                    format!("Master brightness {}%.", v as i32),
                    move |c| {
                        for l in &lights {
                            c.set_brightness(l, v, 300);
                        }
                    },
                );
            }
        }
    }

    fn drain_messages(&mut self, ctx: &egui::Context) {
        while let Ok(msg) = self.rx.try_recv() {
            match msg {
                Msg::Status(s, e) => {
                    if !s.is_empty() {
                        self.set_status(s, e);
                    }
                }
                Msg::Discovered(lights) => {
                    self.discovering = false;
                    self.lights = lights;
                    self.sync_from_lights();
                    if self.lights.is_empty() {
                        self.set_status("No lights found.", true);
                    } else {
                        let avg = self
                            .lights
                            .iter()
                            .map(|l| l.brightness_pct())
                            .sum::<f32>()
                            / self.lights.len() as f32;
                        self.master = avg.round();
                        let n = self.lights.len();
                        self.set_status(
                            format!("Found {n} light{}.", if n == 1 { "" } else { "s" }),
                            false,
                        );
                    }
                }
                Msg::Refresh => self.sync_from_lights(),
                Msg::Texture(name, tex) => {
                    self.textures.insert(name, tex);
                }
                Msg::PlayAmbience(key) => self.audio.play(key),
                Msg::Rediscover => self.spawn_discover(ctx),
            }
        }
    }

    fn flush_pending(&mut self, ctx: &egui::Context) {
        let due: Vec<String> = self
            .pending
            .iter()
            .filter(|(_, (t, _))| t.elapsed() > Duration::from_millis(280))
            .map(|(k, _)| k.clone())
            .collect();
        for key in due {
            if let Some((_, act)) = self.pending.remove(&key) {
                self.act(ctx, act);
            }
        }
    }

    // -- UI ----------------------------------------------------------------

    fn pill(&self, ui: &mut egui::Ui, label: &str, active: bool) -> egui::Response {
        let font = FontId::proportional(10.0);
        let text_w = ui
            .fonts(|f| f.layout_no_wrap(label.to_owned(), font.clone(), theme::TEXT).size().x);
        let (rect, resp) =
            ui.allocate_exact_size(Vec2::new(text_w + 20.0, 22.0), Sense::click());
        if ui.is_rect_visible(rect) {
            let (top, bottom, fg, gloss, glow) = if active {
                (theme::ACCENT_HI, theme::ACCENT_DK, Color32::WHITE, 78,
                 Some(theme::ACCENT.gamma_multiply(0.6)))
            } else if resp.hovered() {
                (theme::BTN_TOP_HOVER, theme::BTN_BOT_HOVER, theme::TEXT, 46, None)
            } else {
                (theme::BTN_TOP, theme::BTN_BOT, theme::TEXT, 30, None)
            };
            skin::button(ui.painter(), rect, top, bottom, theme::EDGE, gloss, glow);
            ui.painter()
                .text(rect.center(), Align2::CENTER_CENTER, label, font, fg);
        }
        resp
    }

    fn caption(ui: &mut egui::Ui, text: &str) {
        ui.label(RichText::new(text).size(8.0).color(theme::MUTED));
    }

    fn scene_tile(&self, ui: &mut egui::Ui, sc: &Scene) -> egui::Response {
        let (rect, resp) =
            ui.allocate_exact_size(Vec2::new(TILE_W, TILE_H), Sense::click());
        if ui.is_rect_visible(rect) {
            let painter = ui.painter();
            match self.textures.get(sc.name) {
                Some(tex) => {
                    painter.image(
                        tex.id(),
                        rect,
                        Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(1.0, 1.0)),
                        Color32::WHITE,
                    );
                }
                None => {
                    painter.rect_filled(rect, Rounding::ZERO, theme::PANEL);
                }
            }
            // name with shadow
            let pos = Pos2::new(rect.min.x + 12.0, rect.max.y - 6.0);
            painter.text(
                pos + Vec2::new(1.0, 1.0),
                Align2::LEFT_BOTTOM,
                sc.name,
                FontId::proportional(12.0),
                Color32::from_rgb(10, 10, 10),
            );
            painter.text(
                pos,
                Align2::LEFT_BOTTOM,
                sc.name,
                FontId::proportional(12.0),
                Color32::WHITE,
            );
            // hover border
            let hover = ui.ctx().animate_value_with_time(
                resp.id.with("hover"),
                if resp.hovered() { 1.0 } else { 0.0 },
                0.15,
            );
            if hover > 0.01 {
                let mix = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * hover) as u8;
                let c = Color32::from_rgb(
                    mix(theme::STROKE.r(), 255),
                    mix(theme::STROKE.g(), 255),
                    mix(theme::STROKE.b(), 255),
                );
                painter.rect_stroke(rect.shrink(1.0), Rounding::ZERO, Stroke::new(2.0, c));
            }
        }
        resp
    }

    fn light_card(&mut self, ui: &mut egui::Ui, l: &Arc<Light>, acts: &mut Vec<Act>) {
        let card_bg = ui.painter().add(egui::Shape::Noop);
        let card = egui::Frame::none()
            .inner_margin(egui::Margin::same(12.0))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());

                // header: swatch | name + vendor·ip | toggle
                ui.horizontal(|ui| {
                    let (srect, _) =
                        ui.allocate_exact_size(Vec2::splat(34.0), Sense::hover());
                    let [h, s, b, _k] = l.hsbk();
                    let (mut r, mut g, mut bl) = hsbk_to_rgb(h, s, b);
                    if !l.power() {
                        r /= 4;
                        g /= 4;
                        bl /= 4;
                    }
                    ui.painter().rect_filled(
                        srect,
                        Rounding::same(9.0),
                        Color32::from_rgb(r, g, bl),
                    );
                    ui.painter().rect_stroke(
                        srect,
                        Rounding::same(9.0),
                        Stroke::new(1.0, theme::STROKE),
                    );
                    ui.vertical(|ui| {
                        ui.label(
                            RichText::new(l.label()).size(13.0).strong().color(theme::TEXT),
                        );
                        ui.label(
                            RichText::new(format!("{} · {}", l.vendor.name(), l.ip))
                                .size(8.0)
                                .color(theme::MUTED),
                        );
                    });
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let mut on = *self.toggles.get(&l.id).unwrap_or(&false);
                        if toggle(ui, egui::Id::new(("tgl", &l.id)), &mut on) {
                            acts.push(Act::Power(l.clone(), on));
                        }
                        self.toggles.insert(l.id.clone(), on);
                    });
                });

                ui.add_space(10.0);

                // knobs: brightness + white temperature
                let (mut bri, mut white) = *self
                    .knobs
                    .get(&l.id)
                    .unwrap_or(&(l.brightness_pct(), 3500.0));
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        let res = knob(
                            ui, &mut bri, 0.0, 100.0, 66.0, theme::ACCENT, 100.0,
                            |v| format!("{}%", v as i32),
                        );
                        if let Some(v) = res.committed {
                            acts.push(Act::Bri(l.clone(), v));
                        } else if res.wheeled {
                            self.pending.insert(
                                format!("b:{}", l.id),
                                (Instant::now(), Act::Bri(l.clone(), bri)),
                            );
                        }
                        Self::caption(ui, "BRIGHTNESS");
                    });
                    ui.add_space(16.0);
                    ui.vertical(|ui| {
                        let res = knob(
                            ui, &mut white, 2500.0, 9000.0, 66.0, theme::WARM, 3500.0,
                            |v| format!("{}K", v as i32),
                        );
                        if let Some(v) = res.committed {
                            acts.push(Act::White(l.clone(), bri, v));
                        } else if res.wheeled {
                            self.pending.insert(
                                format!("w:{}", l.id),
                                (Instant::now(), Act::White(l.clone(), bri, white)),
                            );
                        }
                        Self::caption(ui, "WHITE");
                    });
                });
                self.knobs.insert(l.id.clone(), (bri, white));

                ui.add_space(8.0);

                // colour presets + picker
                ui.horizontal(|ui| {
                    for rgb in theme::PRESETS {
                        let btn = egui::Button::new("")
                            .fill(Color32::from_rgb(rgb[0], rgb[1], rgb[2]))
                            .rounding(Rounding::same(6.0))
                            .min_size(Vec2::splat(20.0));
                        if ui.add(btn).clicked() {
                            acts.push(Act::Rgb(l.clone(), rgb));
                        }
                    }
                    let mut rgb = *self.pickers.entry(l.id.clone()).or_insert([255, 255, 255]);
                    if ui.color_edit_button_srgb(&mut rgb).changed() {
                        self.pending.insert(
                            format!("c:{}", l.id),
                            (Instant::now(), Act::Rgb(l.clone(), rgb)),
                        );
                    }
                    self.pickers.insert(l.id.clone(), rgb);
                });
            });
        ui.painter().set(
            card_bg,
            skin::panel(card.response.rect, theme::PANEL_TOP, theme::PANEL_BOT, theme::EDGE),
        );
    }
}

impl eframe::App for VesperApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.drain_messages(ctx);
        self.flush_pending(ctx);
        if self.driver.mode.is_some() {
            ctx.request_repaint_after(Duration::from_millis(200));
        }
        if !self.pending.is_empty() {
            ctx.request_repaint_after(Duration::from_millis(100));
        }

        let mut acts: Vec<Act> = Vec::new();

        egui::CentralPanel::default()
            .frame(
                egui::Frame::none()
                    .fill(theme::BG_BOT)
                    .inner_margin(egui::Margin::symmetric(14.0, 12.0)),
            )
            .show(ctx, |ui| {
                // window backdrop: a graphite top-to-bottom gradient behind
                // everything (painted first, so all content lands on top).
                ui.painter()
                    .add(skin::vgrad(ui.clip_rect(), theme::BG_TOP, theme::BG_BOT));

                // ---- toolbar -------------------------------------------------
                ui.horizontal(|ui| {
                    ui.scope(|ui| {
                        ui.spacing_mut().item_spacing.x = 0.0;
                        ui.label(RichText::new("Ves").size(18.0).strong().color(theme::TEXT));
                        ui.label(RichText::new("per").size(18.0).strong().color(theme::ACCENT));
                    });
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let label = if self.discovering { "Discovering…" } else { "Discover" };
                        if self.pill(ui, label, false).clicked() && !self.discovering {
                            acts.push(Act::Discover);
                        }
                        if self.pill(ui, "All Off", false).clicked() {
                            acts.push(Act::AllPower(false));
                        }
                        if self.pill(ui, "All On", true).clicked() {
                            acts.push(Act::AllPower(true));
                        }
                        let snd = format!("Sound: {}", if self.sound_on { "On" } else { "Off" });
                        if self.pill(ui, &snd, false).clicked() {
                            acts.push(Act::ToggleSound);
                        }
                        if self.pill(ui, "+ Hue", false).clicked() {
                            acts.push(Act::PairHue);
                        }
                        if self.pill(ui, "+ Nanoleaf", false).clicked() {
                            acts.push(Act::PairNano);
                        }
                    });
                });
                ui.add_space(2.0);
                ui.label(
                    RichText::new(&self.status)
                        .size(10.0)
                        .color(if self.status_err { theme::ERR } else { theme::MUTED }),
                );
                ui.add_space(6.0);

                // ---- master + volume ----------------------------------------
                let master_bg = ui.painter().add(egui::Shape::Noop);
                let master_frame = egui::Frame::none()
                    .inner_margin(egui::Margin::symmetric(16.0, 12.0))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal(|ui| {
                            let mut m = self.master;
                            let res = knob(
                                ui, &mut m, 0.0, 100.0, 74.0, theme::ACCENT, 100.0,
                                |v| format!("{}%", v as i32),
                            );
                            self.master = m;
                            if let Some(v) = res.committed {
                                acts.push(Act::Master(v));
                            } else if res.wheeled {
                                self.pending.insert(
                                    "master".into(),
                                    (Instant::now(), Act::Master(m)),
                                );
                            }
                            ui.add_space(12.0);
                            ui.vertical(|ui| {
                                ui.label(
                                    RichText::new("MASTER").size(13.0).strong()
                                        .color(theme::TEXT),
                                );
                                Self::caption(ui, "BRIGHTNESS · ALL LIGHTS");
                            });
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                ui.vertical(|ui| {
                                    let mut v = self.volume;
                                    let res = knob(
                                        ui, &mut v, 0.0, 100.0, 58.0, theme::GREEN, 60.0,
                                        |x| format!("{}", x as i32),
                                    );
                                    if v != self.volume || res.committed.is_some() {
                                        self.volume = v;
                                        self.audio.set_volume(v);
                                    }
                                    Self::caption(ui, "VOLUME");
                                });
                            });
                        });
                    });
                ui.painter().set(
                    master_bg,
                    skin::panel(
                        master_frame.response.rect,
                        theme::PANELHI_TOP,
                        theme::PANELHI_BOT,
                        theme::EDGE,
                    ),
                );
                ui.add_space(8.0);

                // ---- speed + effects + screen sync --------------------------
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        let mut sp = self.speed_ui;
                        let res = knob(
                            ui, &mut sp, 25.0, 300.0, 56.0, theme::ACCENT, 100.0,
                            |v| format!("{:.1}x", v / 100.0),
                        );
                        if sp != self.speed_ui || res.committed.is_some() {
                            self.speed_ui = sp;
                            self.driver
                                .speed_milli
                                .store((sp * 10.0) as u32, std::sync::atomic::Ordering::Relaxed);
                        }
                        Self::caption(ui, "SPEED");
                    });
                    ui.add_space(10.0);
                    ui.vertical(|ui| {
                        // Effects laid out as a small tree: Breathe and Flow on
                        // the top row, and Undulate branched beneath them (it is
                        // the two combined), with connector lines drawn between.
                        let mut breathe_rect = Rect::NOTHING;
                        let mut flow_rect = Rect::NOTHING;
                        ui.horizontal(|ui| {
                            Self::caption(ui, "EFFECTS");
                            let r = self.pill(ui, "Breathe",
                                self.driver.mode == Some(Mode::Breathe));
                            if r.clicked() { acts.push(Act::Motion(Mode::Breathe)); }
                            breathe_rect = r.rect;
                            let r = self.pill(ui, "Flow",
                                self.driver.mode == Some(Mode::Flow));
                            if r.clicked() { acts.push(Act::Motion(Mode::Flow)); }
                            flow_rect = r.rect;
                            let r = self.pill(ui, "Flicker",
                                self.driver.mode == Some(Mode::Flicker));
                            if r.clicked() { acts.push(Act::Motion(Mode::Flicker)); }
                            let r = self.pill(ui, "Strobe",
                                self.driver.mode == Some(Mode::Strobe));
                            if r.clicked() { acts.push(Act::Motion(Mode::Strobe)); }
                        });
                        ui.add_space(12.0); // room for the connectors
                        let mid_x = (breathe_rect.center().x + flow_rect.center().x) * 0.5;
                        let mut undulate_rect = Rect::NOTHING;
                        ui.horizontal(|ui| {
                            let x0 = ui.cursor().left();
                            ui.add_space((mid_x - x0 - 36.0).max(0.0));
                            let r = self.pill(ui, "Undulate",
                                self.driver.mode == Some(Mode::Undulate));
                            if r.clicked() { acts.push(Act::Motion(Mode::Undulate)); }
                            undulate_rect = r.rect;
                        });
                        // Draw the tree: stubs down from Breathe & Flow to a bus,
                        // then a drop into Undulate. Lights up when active.
                        if undulate_rect != Rect::NOTHING {
                            let active = self.driver.mode == Some(Mode::Undulate);
                            let stroke = Stroke::new(
                                1.5,
                                if active { theme::ACCENT } else { theme::EDGE },
                            );
                            let by = breathe_rect.bottom();
                            let bus_y = by + 6.0;
                            let bx = breathe_rect.center().x;
                            let fx = flow_rect.center().x;
                            let p = ui.painter();
                            p.line_segment([Pos2::new(bx, by), Pos2::new(bx, bus_y)], stroke);
                            p.line_segment([Pos2::new(fx, by), Pos2::new(fx, bus_y)], stroke);
                            p.line_segment([Pos2::new(bx, bus_y), Pos2::new(fx, bus_y)], stroke);
                            p.line_segment(
                                [Pos2::new(mid_x, bus_y),
                                 Pos2::new(undulate_rect.center().x, undulate_rect.top())],
                                stroke,
                            );
                        }
                        ui.add_space(6.0);
                        ui.horizontal(|ui| {
                            Self::caption(ui, "SCREEN SYNC");
                            for mode in [Mode::Complement, Mode::Augment] {
                                if self
                                    .pill(ui, mode.label(), self.driver.mode == Some(mode))
                                    .clicked()
                                {
                                    acts.push(Act::Motion(mode));
                                }
                            }
                            if self.pill(ui, "Off", false).clicked() {
                                acts.push(Act::MotionOff);
                            }
                        });
                    });
                });
                ui.add_space(8.0);

                // ---- scrollable scenes + lights -----------------------------
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        const GAP: f32 = 6.0;

                        // Scenes: fixed-size tiles that wrap to as many columns
                        // as the current width allows (reflow on resize).
                        Self::caption(ui, "SCENES");
                        ui.add_space(4.0);
                        ui.horizontal_wrapped(|ui| {
                            ui.spacing_mut().item_spacing = Vec2::splat(GAP);
                            for (idx, sc) in SCENES.iter().enumerate() {
                                if self.scene_tile(ui, sc).clicked() {
                                    acts.push(Act::Scene(idx));
                                }
                            }
                        });
                        ui.add_space(8.0);

                        Self::caption(ui, "LIGHTS");
                        ui.add_space(4.0);
                        if self.lights.is_empty() && !self.discovering {
                            ui.label(
                                RichText::new(
                                    "No lights found. Make sure they're powered and on \
                                     the same network/subnet as this PC.",
                                )
                                .size(10.0)
                                .color(theme::MUTED),
                            );
                        }
                        // Light cards: reflow into a responsive grid. Pick the
                        // column count from the available width and a ~300px
                        // target, then size every card to fill its share evenly.
                        let lights = self.lights.clone();
                        if !lights.is_empty() {
                            let avail = ui.available_width();
                            let cols = (((avail + GAP) / (300.0 + GAP)).floor() as usize)
                                .clamp(1, lights.len());
                            let card_w = (avail - GAP * (cols as f32 - 1.0)) / cols as f32;
                            for row in lights.chunks(cols) {
                                ui.horizontal_top(|ui| {
                                    ui.spacing_mut().item_spacing.x = GAP;
                                    for l in row {
                                        ui.allocate_ui_with_layout(
                                            Vec2::new(card_w, 0.0),
                                            Layout::top_down(Align::Min),
                                            |ui| self.light_card(ui, l, &mut acts),
                                        );
                                    }
                                });
                                ui.add_space(GAP);
                            }
                        }
                    });
            });

        for act in acts {
            self.act(ctx, act);
        }
    }
}

impl Drop for VesperApp {
    fn drop(&mut self) {
        self.driver.stop_current();
        self.audio.stop();
    }
}

fn main() -> eframe::Result<()> {
    let root = util::assets_root();

    // Headless diagnostic: `vesper.exe --discover [out.txt]` runs one
    // discovery pass and writes what it found (the GUI subsystem has no
    // console to print to).
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("--discover") {
        let ctrl = Controller::new(&root);
        let start = Instant::now();
        let lights = ctrl.discover();
        let mut out = format!(
            "assets_root: {}\ndiscovered {} light(s) in {:?}\n",
            root.display(),
            lights.len(),
            start.elapsed()
        );
        for l in &lights {
            let ack = match l.vendor {
                crate::light::Vendor::Lifx => {
                    let t = Instant::now();
                    let ok = ctrl.lifx.ping_ack(l);
                    format!("ack={} ({}ms)", ok, t.elapsed().as_millis())
                }
                _ => "ack=n/a".to_string(),
            };
            out.push_str(&format!(
                "  {:8} | {:20} | {:15} | on={} | hsbk={:?} | {}\n",
                l.vendor.name(),
                l.label(),
                l.ip,
                l.power(),
                l.hsbk(),
                ack
            ));
        }
        let path = args
            .get(2)
            .cloned()
            .unwrap_or_else(|| "discover_out.txt".into());
        let _ = std::fs::write(path, out);
        return Ok(());
    }
    let mut viewport = egui::ViewportBuilder::default()
        .with_inner_size([544.0, 884.0])
        .with_min_inner_size([496.0, 560.0])
        .with_title("Vesper");
    if let Some(icon) = load_icon(&root) {
        viewport = viewport.with_icon(Arc::new(icon));
    }
    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };
    eframe::run_native(
        "Vesper",
        options,
        Box::new(move |cc| Ok(Box::new(VesperApp::new(cc, root)))),
    )
}
