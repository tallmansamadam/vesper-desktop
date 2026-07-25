//! Vesper - native multi-vendor smart light controller (Rust + egui).
//!
//! The Rust refactor of lifx_gui.py: same dark dial-driven UI, same six
//! vendors, same scenes/effects/screen-sync/ambience - but a single static
//! exe that cold-starts in tens of milliseconds.

#![windows_subsystem = "windows"]

mod ambience;
mod color;
mod controller;
mod custom;
mod effects;
mod govee;
mod hue;
mod knob;
mod lifx;
mod light;
mod music;
mod nanoleaf;
mod nixie;
mod noise;
mod scenes;
mod schedule;
mod screen;
mod single;
mod skin;
mod splash;
mod system;
mod theme;
mod util;
mod wiz;
mod yeelight;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
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
    SunriseDone,
    AddLight(Arc<Light>),
}

/// The "how to add these lights" dialogs. Adding a brand always needs a step
/// on the hardware itself, so each button explains what to do before it starts
/// waiting for it.
#[derive(Clone, Copy, PartialEq)]
enum Dialog {
    Hue,
    Nanoleaf,
    ByIp,
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
    ApplyCustom(usize),
    SaveScene,
    DeleteScene(usize),
    AddSchedule,
    DeleteSchedule(usize),
    AddByIp,
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

/// Seconds from now until the next occurrence of local clock time `h:m`
/// (today if it's still ahead, otherwise tomorrow).
fn secs_until_clock(h: u32, m: u32) -> i64 {
    use chrono::{Local, NaiveTime, TimeZone};
    let now = Local::now();
    let Some(t) = NaiveTime::from_hms_opt(h.min(23), m.min(59), 0) else {
        return 0;
    };
    let mut target = now.date_naive().and_time(t);
    if target <= now.naive_local() {
        target += chrono::Duration::days(1);
    }
    match Local.from_local_datetime(&target).single() {
        Some(dt) => (dt - now).num_seconds().max(0),
        None => 0, // DST gap; caller falls back
    }
}

/// "in 7:20" / "in 14m" - how long until an instant, for the timer readouts.
fn fmt_until(end: Instant) -> String {
    let s = end.saturating_duration_since(Instant::now()).as_secs();
    if s >= 3600 {
        format!("in {}:{:02}", s / 3600, (s % 3600) / 60)
    } else if s >= 60 {
        format!("in {}m", s / 60)
    } else {
        format!("in {s}s")
    }
}

#[cfg(test)]
mod clock_tests {
    use super::*;
    use chrono::Timelike;

    #[test]
    fn resolves_to_the_next_occurrence() {
        let now = chrono::Local::now();

        // A time a couple of minutes ahead is today: ~120s away.
        let soon = now + chrono::Duration::minutes(2);
        let s = secs_until_clock(soon.hour(), soon.minute());
        assert!((30..200).contains(&s), "2 min ahead resolved to {s}s");

        // A time just past rolls to tomorrow: ~24h away, never negative.
        let past = now - chrono::Duration::minutes(2);
        let p = secs_until_clock(past.hour(), past.minute());
        assert!(p > 23 * 3600, "2 min ago resolved to {p}s, expected ~24h");
    }

    #[test]
    fn out_of_range_input_is_clamped_not_panicking() {
        assert!(secs_until_clock(99, 99) >= 0);
    }
}

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

/// Push the active theme into egui's own visuals (window fill, default text),
/// so the parts egui draws for us match the parts we paint ourselves.
fn apply_visuals(ctx: &egui::Context) {
    let mut visuals = if theme::is_light() {
        egui::Visuals::light()
    } else {
        egui::Visuals::dark()
    };
    visuals.panel_fill = theme::bg();
    visuals.override_text_color = Some(theme::text());
    visuals.widgets.inactive.bg_fill = theme::panel();
    visuals.widgets.hovered.bg_fill = theme::btn_top_hover();
    visuals.widgets.active.bg_fill = theme::accent_dk();
    visuals.selection.bg_fill = theme::accent_dk();
    ctx.set_visuals(visuals);
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
    music: music::Music,
    noise: noise::Noise,
    noise_open: bool,
    /// Which noise scene is showing, cleared as soon as a dial is touched.
    noise_scene: Option<usize>,
    timer_open: bool,
    /// Sleep: the clock time to fade off at, and the instant that resolves to.
    timer_hour: f32,
    timer_min: f32,
    timer_end: Option<Instant>,
    /// Wake/sunrise: the clock time to wake at, the ramp length, the resolved
    /// instant, and a cancel flag for the ramp thread once it's running.
    wake_hour: f32,
    wake_min: f32,
    wake_rise_min: f32,
    wake_end: Option<Instant>,
    sunrise_stop: Option<Arc<AtomicBool>>,
    custom_scenes: Vec<custom::CustomScene>,
    custom_path: PathBuf,
    custom_name: String,
    schedules: Vec<schedule::Schedule>,
    sched_path: PathBuf,
    sched_open: bool,
    sched_hour: f32,
    sched_min: f32,
    sched_days: [bool; 7],
    sched_scene: String,
    _system: system::System,
    /// Set by the tray's Quit; shared so the tray thread can set it without
    /// waiting for a UI frame.
    really_quit: Arc<AtomicBool>,
    /// The discovered lights, shared with the tray thread so its actions work
    /// while the window is hidden.
    shared_lights: Arc<std::sync::Mutex<Vec<Arc<Light>>>>,
    splash: splash::Splash,
    theme_idx: usize,
    settings_path: PathBuf,
    manual_ip: String,
    dialog: Option<Dialog>,
    /// Cleared once the window has been placed on-screen (see `place_window`).
    needs_placing: bool,
    layout_logged: bool,
    /// When the UI began assembling itself under the fading splash.
    intro_start: Option<Instant>,
    /// A brief tint in a scene's own colour, as feedback that it was applied.
    scene_flash: Option<(Instant, Color32)>,
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
        let settings_path = root.join("settings.json");
        let theme_idx = std::fs::read_to_string(&settings_path)
            .ok()
            .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
            .and_then(|v| v.get("theme").and_then(serde_json::Value::as_u64))
            .unwrap_or(0) as usize;
        theme::set(theme_idx);
        apply_visuals(&cc.egui_ctx);
        // Float the scrollbar over the content instead of insetting the right
        // edge, so the scrolled area keeps the same left/right margins as the
        // fixed panels above it.
        cc.egui_ctx.style_mut(|s| {
            s.spacing.scroll = egui::style::ScrollStyle::floating();
        });

        let (tx, rx) = channel();
        let ctrl = Arc::new(Controller::new(&root));
        spawn_texture_loader(root.clone(), cc.egui_ctx.clone(), tx.clone());
        let shared_lights = Arc::new(std::sync::Mutex::new(Vec::new()));
        let really_quit = Arc::new(AtomicBool::new(false));
        let sys = system::init(
            &root,
            tx.clone(),
            cc.egui_ctx.clone(),
            system::Shared {
                ctrl: ctrl.clone(),
                lights: shared_lights.clone(),
                quit: really_quit.clone(),
            },
        );
        let music = music::Music::new();

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
            driver: Driver::new(music.level_handle(), music.tone_handle()),
            audio: Ambience::new(root.join("audio")),
            music,
            noise: noise::Noise::new(root.join("audio")),
            noise_open: false,
            noise_scene: None,
            timer_open: false,
            timer_hour: 23.0,
            timer_min: 0.0,
            timer_end: None,
            wake_hour: 7.0,
            wake_min: 0.0,
            wake_rise_min: 10.0,
            wake_end: None,
            sunrise_stop: None,
            custom_scenes: custom::load(&root.join("custom_scenes.json")),
            custom_path: root.join("custom_scenes.json"),
            custom_name: String::new(),
            schedules: schedule::load(&root.join("schedules.json")),
            sched_path: root.join("schedules.json"),
            sched_open: false,
            sched_hour: 7.0,
            sched_min: 0.0,
            sched_days: [true; 7],
            sched_scene: "Relax".to_string(),
            _system: sys,
            really_quit,
            shared_lights,
            splash: splash::Splash::new(),
            theme_idx: theme::index(),
            settings_path,
            manual_ip: String::new(),
            dialog: None,
            needs_placing: true,
            layout_logged: false,
            intro_start: None,
            scene_flash: None,
            knobs: HashMap::new(),
            toggles: HashMap::new(),
            pickers: HashMap::new(),
            pending: HashMap::new(),
        };
        app.spawn_discover(&cc.egui_ctx);
        app
    }

    // -- plumbing ----------------------------------------------------------

    /// Centre the window on the monitor, and never let it start above or left
    /// of the origin - that placed the title bar and left edge off-screen,
    /// where they can't be grabbed. Runs once, as soon as the monitor size is
    /// known (it isn't on the very first frame).
    fn place_window(&mut self, ctx: &egui::Context) {
        if !self.needs_placing {
            return;
        }
        let (monitor, outer) = ctx.input(|i| {
            (
                i.viewport().monitor_size,
                i.viewport().outer_rect.map(|r| r.size()),
            )
        });
        let (Some(monitor), Some(size)) = (monitor, outer) else {
            ctx.request_repaint(); // sizes not published yet; try next frame
            return;
        };
        let pos = Pos2::new(
            ((monitor.x - size.x) * 0.5).max(0.0),
            ((monitor.y - size.y) * 0.5).max(0.0),
        );
        ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(pos));
        self.needs_placing = false;
    }

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
        // Master mirrors the room: after a scene (or any command) it shows the
        // lights' average brightness rather than whatever it was last dragged
        // to. Skipped while a knob is mid-drag so it can't fight the user.
        if !self.lights.is_empty() {
            let avg = self.lights.iter().map(|l| l.brightness_pct()).sum::<f32>()
                / self.lights.len() as f32;
            self.master = avg.round();
        }
    }

    /// Kick off a sunrise ramp: power the lights on dim + warm, then step
    /// brightness up (and cool the white) over `rise_min` on a worker thread.
    /// Returns the cancel flag so a later wake/cancel can stop it.
    fn start_sunrise(&self, ctx: &egui::Context, rise_min: f32) -> Arc<AtomicBool> {
        let stop = Arc::new(AtomicBool::new(false));
        let lights = self.lights.clone();
        let ctrl = self.ctrl.clone();
        let tx = self.tx.clone();
        let ctx = ctx.clone();
        let flag = stop.clone();
        std::thread::spawn(move || {
            const STEPS: u32 = 60;
            let dt = (rise_min * 60.0 / STEPS as f32).max(0.05);
            for l in &lights {
                ctrl.set_power(l, true, 1000);
            }
            for i in 0..=STEPS {
                if flag.load(Ordering::Relaxed) {
                    return;
                }
                let t = i as f32 / STEPS as f32;
                let pct = 2.0 + 98.0 * t; // 2% -> 100%
                let kelvin = (2000.0 + 2500.0 * t) as u16; // amber -> soft white
                for l in &lights {
                    ctrl.set_white(l, pct, kelvin, (dt * 1000.0) as u32);
                }
                let _ = tx.send(Msg::Status(format!("Sunrise … {}%", pct as i32), false));
                let _ = tx.send(Msg::Refresh);
                ctx.request_repaint();
                std::thread::sleep(Duration::from_secs_f32(dt));
            }
            let _ = tx.send(Msg::Status("Sunrise complete — good morning.".into(), false));
            let _ = tx.send(Msg::SunriseDone);
        });
        stop
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
                let stops = tile_stops(sc);
                let mid = stops[stops.len() / 2];
                self.scene_flash =
                    Some((Instant::now(), Color32::from_rgb(mid[0], mid[1], mid[2])));
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
            Act::ApplyCustom(i) => {
                self.driver.stop_current();
                if self.lights.is_empty() {
                    self.set_status("No lights yet — hit Discover.", true);
                    return;
                }
                let Some(sc) = self.custom_scenes.get(i) else { return };
                let name = sc.name.clone();
                let colors = sc.colors.clone();
                let pct = sc.pct;
                let mid = colors[colors.len() / 2];
                self.scene_flash =
                    Some((Instant::now(), Color32::from_rgb(mid[0], mid[1], mid[2])));
                self.set_status(format!("Applying scene: {name} …"), false);
                let lights = self.lights.clone();
                let ctrl = self.ctrl.clone();
                let tx = self.tx.clone();
                let ctx2 = ctx.clone();
                std::thread::spawn(move || {
                    let cs = custom::CustomScene { name: name.clone(), pct, colors };
                    custom::apply(&ctrl, &lights, &cs, true);
                    let _ = tx.send(Msg::Status(format!("Scene: {name}"), false));
                    let _ = tx.send(Msg::Refresh);
                    ctx2.request_repaint();
                });
            }
            Act::SaveScene => {
                let name = self.custom_name.trim().to_string();
                if name.is_empty() {
                    self.set_status("Name your scene first.", true);
                    return;
                }
                if self.lights.is_empty() {
                    self.set_status("No lights to capture — hit Discover.", true);
                    return;
                }
                let sc = custom::capture(name.clone(), &self.lights);
                // Replace an existing scene of the same name, else append.
                if let Some(slot) = self.custom_scenes.iter_mut().find(|s| s.name == name) {
                    *slot = sc;
                } else {
                    self.custom_scenes.push(sc);
                }
                custom::save(&self.custom_path, &self.custom_scenes);
                self.custom_name.clear();
                self.set_status(format!("Saved scene “{name}”."), false);
            }
            Act::DeleteScene(i) => {
                if i < self.custom_scenes.len() {
                    let name = self.custom_scenes.remove(i).name;
                    custom::save(&self.custom_path, &self.custom_scenes);
                    self.set_status(format!("Deleted scene “{name}”."), false);
                }
            }
            Act::AddSchedule => {
                let exe = match std::env::current_exe() {
                    Ok(e) => e,
                    Err(e) => {
                        self.set_status(format!("Can't locate the app: {e}"), true);
                        return;
                    }
                };
                let hour = self.sched_hour as u32;
                let minute = self.sched_min as u32;
                let slug = scene_slug(&self.sched_scene);
                let id = format!("{hour:02}{minute:02}_{slug}_{}", fastrand::u32(1000..9999));
                let sc = schedule::Schedule {
                    id,
                    scene: self.sched_scene.clone(),
                    hour,
                    minute,
                    days: self.sched_days,
                };
                match schedule::create_task(&exe, &sc) {
                    Ok(()) => {
                        self.set_status(
                            format!("Scheduled {} at {}.", sc.scene, sc.time_str()),
                            false,
                        );
                        self.schedules.push(sc);
                        schedule::save(&self.sched_path, &self.schedules);
                    }
                    Err(e) => self.set_status(format!("Schedule failed: {e}"), true),
                }
            }
            Act::DeleteSchedule(i) => {
                if i < self.schedules.len() {
                    let s = self.schedules.remove(i);
                    let _ = schedule::delete_task(&s.id);
                    schedule::save(&self.sched_path, &self.schedules);
                    self.set_status(format!("Removed schedule for {}.", s.scene), false);
                }
            }
            Act::AddByIp => {
                let ip = self.manual_ip.trim().to_string();
                if ip.is_empty() {
                    self.set_status("Enter the light's IP address first.", true);
                    return;
                }
                if self.lights.iter().any(|l| l.ip == ip) {
                    self.set_status(format!("{ip} is already in the list."), true);
                    return;
                }
                self.set_status(format!("Looking for a light at {ip} …"), false);
                let ctrl = self.ctrl.clone();
                let tx = self.tx.clone();
                let ctx2 = ctx.clone();
                std::thread::spawn(move || {
                    match ctrl.add_by_ip(&ip) {
                        Some(l) => {
                            let _ = tx.send(Msg::Status(
                                format!("Added {} at {ip}.", l.vendor.name()),
                                false,
                            ));
                            let _ = tx.send(Msg::AddLight(l));
                        }
                        None => {
                            let _ = tx.send(Msg::Status(
                                format!(
                                    "Nothing answered at {ip}. Check the address, and \
                                     that LAN control is enabled for the device."
                                ),
                                true,
                            ));
                        }
                    }
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
                if mode == Mode::Music {
                    if !self.music.start() {
                        self.set_status(
                            "Couldn't capture system audio — play something first.",
                            true,
                        );
                        return;
                    }
                }
                self.driver.start(mode, self.ctrl.clone(), self.lights.clone());
                let label = mode.label();
                match mode {
                    Mode::Complement | Mode::Augment => self.set_status(
                        format!("Screen sync: {label} — matching your screen…"),
                        false,
                    ),
                    Mode::Music => self.set_status(
                        "Music: lights follow your PC's audio.",
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
                    if let Ok(mut shared) = self.shared_lights.lock() {
                        shared.clone_from(&self.lights);
                    }
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
                Msg::SunriseDone => self.sunrise_stop = None,
                Msg::AddLight(l) => {
                    if !self.lights.iter().any(|x| x.id == l.id) {
                        self.lights.push(l);
                        self.lights.sort_by(|a, b| {
                            (a.vendor.name(), a.label().to_lowercase())
                                .cmp(&(b.vendor.name(), b.label().to_lowercase()))
                        });
                        if let Ok(mut shared) = self.shared_lights.lock() {
                            shared.clone_from(&self.lights);
                        }
                        self.manual_ip.clear();
                        self.sync_from_lights();
                    }
                }
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
            .fonts(|f| f.layout_no_wrap(label.to_owned(), font.clone(), theme::text()).size().x);
        let (rect, resp) =
            ui.allocate_exact_size(Vec2::new(text_w + 20.0, 22.0), Sense::click());
        if ui.is_rect_visible(rect) {
            let (top, bottom, fg, gloss, glow) = if active {
                (theme::accent_hi(), theme::accent_dk(), Color32::WHITE, 78,
                 Some(theme::accent().gamma_multiply(0.6)))
            } else if resp.hovered() {
                (theme::btn_top_hover(), theme::btn_bot_hover(), theme::text(), 46, None)
            } else {
                (theme::btn_top(), theme::btn_bot(), theme::text(), 30, None)
            };
            skin::button(ui.painter(), rect, top, bottom, theme::edge(), gloss, glow);
            ui.painter()
                .text(rect.center(), Align2::CENTER_CENTER, label, font, fg);
        }
        resp
    }

    /// Opacity for the `idx`-th section of the intro cascade: the panels fade
    /// in one after another as the splash hands over, so the app assembles
    /// itself instead of appearing all at once.
    fn intro_alpha(&self, idx: usize) -> f32 {
        let Some(start) = self.intro_start else {
            return 0.0;
        };
        let ms = start.elapsed().as_secs_f32() * 1000.0 - idx as f32 * 85.0;
        let a = (ms / 400.0).clamp(0.0, 1.0);
        1.0 - (1.0 - a) * (1.0 - a) // ease out
    }

    /// The add-a-brand dialog: what to do on the hardware, then the button
    /// that starts waiting for it. Dismissed by its own Cancel or by clicking
    /// away from it.
    fn dialog_ui(&mut self, ctx: &egui::Context, acts: &mut Vec<Act>) {
        let Some(kind) = self.dialog else { return };
        let screen = ctx.screen_rect();

        // Scrim: dims the app and swallows the click that dismisses.
        egui::Area::new(egui::Id::new("dlg_scrim"))
            .order(egui::Order::Middle)
            .fixed_pos(screen.min)
            .show(ctx, |ui| {
                let r = ui.allocate_response(screen.size(), Sense::click());
                ui.painter()
                    .rect_filled(screen, Rounding::ZERO, Color32::from_black_alpha(150));
                if r.clicked() {
                    self.dialog = None;
                }
            });

        let (title, body, action) = match kind {
            Dialog::Hue => (
                "Add Philips Hue",
                "Hue bulbs are paired through the Bridge, not found on the network.\n\n\
                 1.  Press the round link button on top of your Hue Bridge.\n\
                 2.  Choose Start pairing below within 30 seconds.\n\n\
                 Once paired, Vesper remembers the Bridge - you only do this once.",
                Some("Start pairing"),
            ),
            Dialog::Nanoleaf => (
                "Add Nanoleaf",
                "Nanoleaf panels pair from the controller itself.\n\n\
                 1.  Hold the panel's power button for 5-7 seconds, until the\n     LEDs flash in sequence.\n\
                 2.  Choose Start pairing below within 25 seconds.\n\n\
                 Once paired, Vesper remembers the panels - you only do this once.",
                Some("Start pairing"),
            ),
            Dialog::ByIp => (
                "Add a light by address",
                "LIFX, WiZ, Yeelight and Govee are normally found automatically by\n\
                 Discover. Use this when one doesn't appear - on a guest network,\n\
                 another subnet, or with AP isolation on.\n\n\
                 Govee and Yeelight also need LAN control switched on in their own\n\
                 app first. Enter the light's IP address:",
                Some("Add light"),
            ),
        };

        egui::Area::new(egui::Id::new("dlg"))
            .order(egui::Order::Foreground)
            .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                let bg = ui.painter().add(egui::Shape::Noop);
                let frame = egui::Frame::none()
                    .inner_margin(egui::Margin::same(18.0))
                    .show(ui, |ui| {
                        ui.set_width(400.0);
                        ui.label(
                            RichText::new(title).size(15.0).strong().color(theme::text()),
                        );
                        ui.add_space(8.0);
                        ui.label(RichText::new(body).size(11.0).color(theme::muted()));
                        if kind == Dialog::ByIp {
                            ui.add_space(8.0);
                            ui.add(
                                egui::TextEdit::singleline(&mut self.manual_ip)
                                    .hint_text("192.168.1.42")
                                    .desired_width(180.0),
                            );
                        }
                        ui.add_space(14.0);
                        ui.horizontal(|ui| {
                            if let Some(label) = action {
                                if self.pill(ui, label, true).clicked() {
                                    match kind {
                                        Dialog::Hue => acts.push(Act::PairHue),
                                        Dialog::Nanoleaf => acts.push(Act::PairNano),
                                        Dialog::ByIp => acts.push(Act::AddByIp),
                                    }
                                    self.dialog = None;
                                }
                            }
                            if self.pill(ui, "Cancel", false).clicked() {
                                self.dialog = None;
                            }
                        });
                    });
                ui.painter().set(
                    bg,
                    skin::panel(
                        frame.response.rect,
                        theme::panelhi_top(),
                        theme::panelhi_bot(),
                        theme::edge(),
                    ),
                );
            });
    }

    fn caption(ui: &mut egui::Ui, text: &str) {
        ui.label(RichText::new(text).size(8.0).color(theme::muted()));
    }

    fn scene_tile(&self, ui: &mut egui::Ui, sc: &Scene, size: Vec2) -> egui::Response {
        let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
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
                    painter.rect_filled(rect, Rounding::ZERO, theme::panel());
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
                    mix(theme::stroke().r(), 255),
                    mix(theme::stroke().g(), 255),
                    mix(theme::stroke().b(), 255),
                );
                painter.rect_stroke(rect.shrink(1.0), Rounding::ZERO, Stroke::new(2.0, c));
            }
        }
        resp
    }

    /// A tile for a user-saved scene: its palette painted as a gradient, the
    /// name, and a corner ✕ to delete it. Left-click applies; the returned
    /// bool is true when the ✕ was clicked.
    fn custom_tile(&self, ui: &mut egui::Ui, sc: &custom::CustomScene, size: Vec2) -> (egui::Response, bool) {
        let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
        let mut delete = false;
        if ui.is_rect_visible(rect) {
            let cols: Vec<Color32> = sc
                .colors
                .iter()
                .map(|c| Color32::from_rgb(c[0], c[1], c[2]))
                .collect();
            let painter = ui.painter();
            painter.add(skin::hgrad_multi(rect, &cols));
            // darken the lower strip so the name reads
            painter.rect_filled(
                Rect::from_min_max(Pos2::new(rect.min.x, rect.max.y - 22.0), rect.max),
                Rounding::ZERO,
                Color32::from_black_alpha(120),
            );
            let pos = Pos2::new(rect.min.x + 10.0, rect.max.y - 6.0);
            painter.text(
                pos,
                Align2::LEFT_BOTTOM,
                &sc.name,
                FontId::proportional(12.0),
                Color32::WHITE,
            );
            // delete ✕ hot-corner (top-right)
            let x_rect = Rect::from_min_size(
                Pos2::new(rect.max.x - 18.0, rect.min.y + 2.0),
                Vec2::splat(16.0),
            );
            let x_hover = ui.rect_contains_pointer(x_rect);
            painter.text(
                x_rect.center(),
                Align2::CENTER_CENTER,
                "✕",
                FontId::proportional(12.0),
                if x_hover { theme::err() } else { Color32::from_white_alpha(150) },
            );
            if resp.clicked() && x_hover {
                delete = true;
            }
            painter.rect_stroke(
                rect,
                Rounding::ZERO,
                Stroke::new(1.0, if resp.hovered() { theme::edge() } else { theme::stroke() }),
            );
        }
        (resp, delete)
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
                        Stroke::new(1.0, theme::stroke()),
                    );
                    ui.vertical(|ui| {
                        ui.label(
                            RichText::new(l.label()).size(13.0).strong().color(theme::text()),
                        );
                        ui.label(
                            RichText::new(format!("{} · {}", l.vendor.name(), l.ip))
                                .size(8.0)
                                .color(theme::muted()),
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
                            ui, &mut bri, 0.0, 100.0, 66.0, theme::accent(), 100.0,
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
                            ui, &mut white, 2500.0, 9000.0, 66.0, theme::warm(), 3500.0,
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
            skin::panel(card.response.rect, theme::panel_top(), theme::panel_bot(), theme::edge()),
        );
    }

    /// Every scene name selectable in a schedule: built-ins, the user's saved
    /// scenes, and the special "All Off".
    fn all_scene_names(&self) -> Vec<String> {
        let mut v: Vec<String> = SCENES.iter().map(|s| s.name.to_string()).collect();
        v.extend(self.custom_scenes.iter().map(|s| s.name.clone()));
        v.push("All Off".to_string());
        v
    }

    /// The always-on scheduler section: lists existing schedules and an add
    /// row (time knobs, day chips, scene picker) that registers OS tasks.
    fn schedule_ui(&mut self, ui: &mut egui::Ui, acts: &mut Vec<Act>) {
        let arrow = if self.sched_open { "-" } else { "+" };
        let header = format!("{arrow}  Schedules (always-on)");
        if self.pill(ui, &header, !self.schedules.is_empty()).clicked() {
            self.sched_open = !self.sched_open;
        }
        if !self.sched_open {
            return;
        }
        ui.add_space(6.0);

        // existing schedules
        let mut del = None;
        for (i, s) in self.schedules.iter().enumerate() {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(format!("{}   {}", s.time_str(), s.scene))
                        .size(12.0)
                        .monospace()
                        .color(theme::text()),
                );
                ui.label(RichText::new(s.days_label()).size(10.0).color(theme::muted()));
                if self.pill(ui, "✕", false).clicked() {
                    del = Some(i);
                }
            });
        }
        if let Some(i) = del {
            acts.push(Act::DeleteSchedule(i));
        }

        ui.add_space(8.0);
        // add row: HOUR / MIN knobs + scene picker
        ui.horizontal(|ui| {
            nixie::time(ui, "sched", &mut self.sched_hour, &mut self.sched_min);
            ui.add_space(14.0);
            ui.vertical(|ui| {
                ui.add_space(10.0);
                let names = self.all_scene_names();
                egui::ComboBox::from_id_salt("sched_scene")
                    .selected_text(self.sched_scene.clone())
                    .show_ui(ui, |ui| {
                        for name in names {
                            ui.selectable_value(&mut self.sched_scene, name.clone(), name);
                        }
                    });
                Self::caption(ui, "SCENE");
            });
        });
        ui.add_space(6.0);
        // day chips + add
        ui.horizontal(|ui| {
            const ABBR: [&str; 7] = ["Mo", "Tu", "We", "Th", "Fr", "Sa", "Su"];
            for i in 0..7 {
                if self.pill(ui, ABBR[i], self.sched_days[i]).clicked() {
                    self.sched_days[i] = !self.sched_days[i];
                }
            }
            ui.add_space(12.0);
            if self.pill(ui, "Add schedule", false).clicked() {
                acts.push(Act::AddSchedule);
            }
        });
        ui.label(
            RichText::new(
                "Runs via Windows Task Scheduler — fires even when Vesper is closed, \
                 while you're signed in.",
            )
            .size(8.0)
            .color(theme::muted()),
        );
    }
}

impl eframe::App for VesperApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.place_window(ctx);
        self.drain_messages(ctx);
        self.flush_pending(ctx);

        // Release the audio capture whenever we're not in Music mode (however
        // the mode ended - MotionOff, a scene tap, discovery, etc).
        if self.driver.mode != Some(Mode::Music) && self.music.active() {
            self.music.stop();
        }

        // Close button minimises to the tray instead of quitting (unless the
        // tray's Quit was chosen, which sets really_quit).
        if ctx.input(|i| i.viewport().close_requested())
            && !self.really_quit.load(Ordering::Relaxed)
        {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
            self.set_status("Minimised to tray — right-click the tray icon for options.", false);
        }
        if self.driver.mode.is_some() {
            ctx.request_repaint_after(Duration::from_millis(200));
        }
        if !self.pending.is_empty() {
            ctx.request_repaint_after(Duration::from_millis(100));
        }

        // Sleep timer: when it elapses, fade every light off and silence the
        // audio beds. Checked here (not on a thread) and kept ticking by a
        // repaint request so the countdown stays live even when idle.
        if let Some(end) = self.timer_end {
            if Instant::now() >= end {
                self.timer_end = None;
                self.audio.stop();
                self.noise.stop();
                let lights = self.lights.clone();
                self.set_status("Sleep timer: fading everything off …", false);
                self.spawn_cmd(ctx, "Sleep timer: lights off.".into(), move |c| {
                    for l in &lights {
                        c.set_power(l, false, 3000); // 3s fade where the vendor supports it
                    }
                });
            } else {
                ctx.request_repaint_after(Duration::from_millis(500));
            }
        }

        // Wake/sunrise alarm: at the set time, start the sunrise ramp.
        if let Some(end) = self.wake_end {
            if Instant::now() >= end {
                self.wake_end = None;
                self.sunrise_stop = Some(self.start_sunrise(ctx, self.wake_rise_min));
            } else {
                ctx.request_repaint_after(Duration::from_millis(500));
            }
        }

        // The moment the splash starts fading, begin assembling the UI beneath
        // it so the two overlap instead of cutting.
        if self.intro_start.is_none() && self.splash.handover() > 0.0 {
            self.intro_start = Some(Instant::now());
        }
        if self
            .intro_start
            .is_some_and(|s| s.elapsed() < Duration::from_millis(1400))
        {
            ctx.request_repaint();
        }

        let mut acts: Vec<Act> = Vec::new();

        egui::CentralPanel::default()
            .frame(
                egui::Frame::none()
                    .fill(theme::bg_bot())
                    .inner_margin(egui::Margin::symmetric(14.0, 12.0)),
            )
            .show(ctx, |ui| {
                // window backdrop: a graphite top-to-bottom gradient behind
                // everything (painted first, so all content lands on top).
                ui.painter()
                    .add(skin::vgrad(ui.clip_rect(), theme::bg_top(), theme::bg_bot()));

                // Scene feedback: the room's new colour washes down the top of
                // the window and fades - a nod that the scene landed, not a
                // flash that demands attention.
                if let Some((t0, col)) = self.scene_flash {
                    let e = t0.elapsed().as_secs_f32();
                    if e >= 1.1 {
                        self.scene_flash = None;
                    } else {
                        let a = (1.0 - e / 1.1).powi(2) * 0.22;
                        let r = ui.clip_rect();
                        let wash = Rect::from_min_max(
                            r.min,
                            Pos2::new(r.max.x, r.min.y + r.height() * 0.45),
                        );
                        ui.painter().add(skin::vgrad(
                            wash,
                            Color32::from_rgba_unmultiplied(
                                col.r(),
                                col.g(),
                                col.b(),
                                (a * 255.0) as u8,
                            ),
                            Color32::from_rgba_unmultiplied(col.r(), col.g(), col.b(), 0),
                        ));
                        ctx.request_repaint();
                    }
                }

                // ---- toolbar -------------------------------------------------
                let avail_w = ui.available_width();
                let intro0 = self.intro_alpha(0);
                ui.set_opacity(intro0);
                // Wrapping, not right-aligned: right-to-left pushed the
                // overflow off the left edge instead of onto a second row.
                let toolbar_used = ui.horizontal_wrapped(|ui| {
                    {
                        ui.scope(|ui| {
                            ui.spacing_mut().item_spacing.x = 0.0;
                            ui.label(
                                RichText::new("Ves").size(18.0).strong().color(theme::text()),
                            );
                            ui.label(
                                RichText::new("per").size(18.0).strong().color(theme::accent()),
                            );
                        });
                        ui.add_space(6.0);
                        let label = if self.discovering { "Discovering…" } else { "Discover" };
                        if self.pill(ui, label, false).clicked() && !self.discovering {
                            acts.push(Act::Discover);
                        }
                        if self.pill(ui, "All On", true).clicked() {
                            acts.push(Act::AllPower(true));
                        }
                        if self.pill(ui, "All Off", false).clicked() {
                            acts.push(Act::AllPower(false));
                        }
                        let snd = format!("Sound: {}", if self.sound_on { "On" } else { "Off" });
                        if self.pill(ui, &snd, false).clicked() {
                            acts.push(Act::ToggleSound);
                        }
                        if self.pill(ui, "+ Hue", false).clicked() {
                            self.dialog = Some(Dialog::Hue);
                        }
                        if self.pill(ui, "+ Nanoleaf", false).clicked() {
                            self.dialog = Some(Dialog::Nanoleaf);
                        }
                        if self.pill(ui, "+ By IP", false).clicked() {
                            self.dialog = Some(Dialog::ByIp);
                        }
                        // theme picker
                        let before = self.theme_idx;
                        egui::ComboBox::from_id_salt("theme_pick")
                            .selected_text(theme::current().name)
                            .width(96.0)
                            .show_ui(ui, |ui| {
                                for (i, t) in theme::THEMES.iter().enumerate() {
                                    ui.selectable_value(&mut self.theme_idx, i, t.name);
                                }
                            });
                        if self.theme_idx != before {
                            theme::set(self.theme_idx);
                            apply_visuals(ui.ctx());
                            let _ = std::fs::write(
                                &self.settings_path,
                                format!("{{\n  \"theme\": {}\n}}\n", self.theme_idx),
                            );
                        }
                        ui.min_rect().width()
                    }
                })
                .inner;
                // One-shot layout probe (VESPER_LAYOUT_LOG=<file>): what the
                // rows actually need vs the width they get.
                if let Ok(path) = std::env::var("VESPER_LAYOUT_LOG") {
                    if self.intro_start.is_some() && !self.layout_logged {
                        self.layout_logged = true;
                        let _ = std::fs::write(
                            path,
                            format!(
                                "available width: {avail_w:.0}\ntoolbar needs:   {toolbar_used:.0}\n\
                                 overflow:        {:.0}\n",
                                (toolbar_used - avail_w).max(0.0)
                            ),
                        );
                    }
                }
                ui.add_space(2.0);
                ui.label(
                    RichText::new(&self.status)
                        .size(10.0)
                        .color(if self.status_err { theme::err() } else { theme::muted() }),
                );
                ui.add_space(6.0);

                // Everything below the toolbar scrolls as one. The master
                // panel grows a lot when Noise/Timer are expanded, so if only
                // the lower half scrolled it would push the rest out of the
                // window entirely.
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {

                // ---- master + volume ----------------------------------------
                ui.set_opacity(self.intro_alpha(1));
                let master_bg = ui.painter().add(egui::Shape::Noop);
                let master_frame = egui::Frame::none()
                    .inner_margin(egui::Margin::symmetric(16.0, 12.0))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal(|ui| {
                            let mut m = self.master;
                            let res = knob(
                                ui, &mut m, 0.0, 100.0, 74.0, theme::accent(), 100.0,
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
                                        .color(theme::text()),
                                );
                                Self::caption(ui, "BRIGHTNESS · ALL LIGHTS");
                            });
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                ui.vertical(|ui| {
                                    let mut v = self.volume;
                                    let res = knob(
                                        ui, &mut v, 0.0, 100.0, 58.0, theme::green(), 60.0,
                                        |x| format!("{}", x as i32),
                                    );
                                    if v != self.volume || res.committed.is_some() {
                                        self.volume = v;
                                        self.audio.set_volume(v);
                                        self.noise.set_master(v);
                                    }
                                    Self::caption(ui, "VOLUME");
                                });
                            });
                        });

                        // ---- noise generator (collapsible) ------------------
                        // Layered noise colours, mixable, tucked into the same
                        // bar as the master dials. Header toggles the section.
                        ui.add_space(8.0);
                        let arrow = if self.noise_open { "-" } else { "+" };
                        if self
                            .pill(ui, &format!("{arrow}  Noise"), self.noise.active())
                            .clicked()
                        {
                            self.noise_open = !self.noise_open;
                        }
                        if self.noise_open {
                            ui.add_space(6.0);
                            // Places, as mixes of the layers below - the noise
                            // panel's answer to the light scenes.
                            ui.horizontal_wrapped(|ui| {
                                ui.spacing_mut().item_spacing = Vec2::new(4.0, 4.0);
                                for (i, ns) in noise::SCENES.iter().enumerate() {
                                    if self
                                        .pill(ui, ns.name, self.noise_scene == Some(i))
                                        .clicked()
                                    {
                                        self.noise.apply_scene(ns);
                                        self.noise_scene = Some(i);
                                    }
                                }
                                if self.pill(ui, "Clear", false).clicked() {
                                    self.noise.clear();
                                    self.noise_scene = None;
                                }
                            });
                            ui.add_space(8.0);
                            // Grid the noise dials so every one stays visible,
                            // stacking into fewer columns as the width shrinks.
                            let avail = ui.available_width();
                            let cols = (((avail + 10.0) / 62.0).floor() as usize)
                                .clamp(1, noise::KINDS.len());
                            for row in noise::KINDS.chunks(cols) {
                                ui.horizontal(|ui| {
                                    ui.spacing_mut().item_spacing.x = 10.0;
                                    for &k in row {
                                        ui.vertical(|ui| {
                                            let mut v = self.noise.level_pct(k);
                                            let before = v;
                                            let res = knob(
                                                ui, &mut v, 0.0, 100.0, 46.0, theme::green(),
                                                0.0, |x| format!("{}", x as i32),
                                            );
                                            if v != before
                                                || res.committed.is_some()
                                                || res.wheeled
                                            {
                                                self.noise.set_level(k, v);
                                                // It's their mix now, not the
                                                // scene's.
                                                self.noise_scene = None;
                                            }
                                            Self::caption(ui, k.label());
                                        });
                                    }
                                });
                                ui.add_space(8.0);
                            }
                        }

                        // ---- sleep timer (collapsible) ----------------------
                        ui.add_space(8.0);
                        let t_arrow = if self.timer_open { "-" } else { "+" };
                        if self
                            .pill(ui, &format!("{t_arrow}  Timer"), self.timer_end.is_some())
                            .clicked()
                        {
                            self.timer_open = !self.timer_open;
                        }
                        if self.timer_open {
                            ui.add_space(6.0);
                            Self::caption(ui, "SLEEP · FADE OFF AT");
                            ui.add_space(4.0);
                            ui.horizontal(|ui| {
                                if let Some(end) = self.timer_end {
                                    ui.label(
                                        RichText::new(format!(
                                            "{:02}:{:02}  {}",
                                            self.timer_hour as i32,
                                            self.timer_min as i32,
                                            fmt_until(end)
                                        ))
                                        .size(14.0)
                                        .monospace()
                                        .color(theme::text()),
                                    );
                                    ui.add_space(12.0);
                                    if self.pill(ui, "Cancel", false).clicked() {
                                        self.timer_end = None;
                                        self.set_status("Sleep timer cancelled.", false);
                                    }
                                } else {
                                    nixie::time(
                                        ui,
                                        "sleep",
                                        &mut self.timer_hour,
                                        &mut self.timer_min,
                                    );
                                    ui.add_space(12.0);
                                    if self.pill(ui, "Set sleep", false).clicked() {
                                        let secs = secs_until_clock(
                                            self.timer_hour as u32,
                                            self.timer_min as u32,
                                        );
                                        self.timer_end =
                                            Some(Instant::now() + Duration::from_secs(secs as u64));
                                        self.set_status(
                                            format!(
                                                "Sleep at {:02}:{:02} — fades everything off then.",
                                                self.timer_hour as i32, self.timer_min as i32
                                            ),
                                            false,
                                        );
                                    }
                                }
                            });

                            // wake / sunrise alarm
                            ui.add_space(8.0);
                            Self::caption(ui, "WAKE · SUNRISE AT");
                            ui.add_space(4.0);
                            ui.horizontal(|ui| {
                                if self.sunrise_stop.is_some() {
                                    ui.label(
                                        RichText::new("Sunrise running")
                                            .size(13.0)
                                            .color(theme::warm()),
                                    );
                                    ui.add_space(12.0);
                                    if self.pill(ui, "Stop", false).clicked() {
                                        if let Some(s) = self.sunrise_stop.take() {
                                            s.store(true, Ordering::Relaxed);
                                        }
                                        self.set_status("Sunrise stopped.", false);
                                    }
                                } else if let Some(end) = self.wake_end {
                                    ui.label(
                                        RichText::new(format!(
                                            "{:02}:{:02}  {}",
                                            self.wake_hour as i32,
                                            self.wake_min as i32,
                                            fmt_until(end)
                                        ))
                                        .size(14.0)
                                        .monospace()
                                        .color(theme::text()),
                                    );
                                    ui.add_space(12.0);
                                    if self.pill(ui, "Cancel", false).clicked() {
                                        self.wake_end = None;
                                        self.set_status("Wake alarm cancelled.", false);
                                    }
                                } else {
                                    nixie::time(
                                        ui,
                                        "wake",
                                        &mut self.wake_hour,
                                        &mut self.wake_min,
                                    );
                                    ui.add_space(10.0);
                                    ui.vertical(|ui| {
                                        let mut m = self.wake_rise_min;
                                        let res = knob(
                                            ui, &mut m, 1.0, 30.0, 46.0, theme::warm(),
                                            10.0, |v| format!("{}m", v as i32),
                                        );
                                        if m != self.wake_rise_min
                                            || res.committed.is_some()
                                            || res.wheeled
                                        {
                                            self.wake_rise_min = m;
                                        }
                                        Self::caption(ui, "RISE");
                                    });
                                    ui.add_space(12.0);
                                    if self.pill(ui, "Set wake", false).clicked() {
                                        // The ramp should *finish* at the set
                                        // time, so start it a rise-length early
                                        // - you wake to full light, not to the
                                        // first ember.
                                        let secs = secs_until_clock(
                                            self.wake_hour as u32,
                                            self.wake_min as u32,
                                        );
                                        let lead = (self.wake_rise_min * 60.0) as i64;
                                        let start = (secs - lead).max(0) as u64;
                                        self.wake_end =
                                            Some(Instant::now() + Duration::from_secs(start));
                                        self.set_status(
                                            format!(
                                                "Wake at {:02}:{:02} — {} min sunrise starts before.",
                                                self.wake_hour as i32,
                                                self.wake_min as i32,
                                                self.wake_rise_min as i32
                                            ),
                                            false,
                                        );
                                    }
                                }
                            });
                        }
                    });
                ui.painter().set(
                    master_bg,
                    skin::panel(
                        master_frame.response.rect,
                        theme::panelhi_top(),
                        theme::panelhi_bot(),
                        theme::edge(),
                    ),
                );
                ui.add_space(8.0);

                // ---- speed + effects + screen sync --------------------------
                ui.set_opacity(self.intro_alpha(2));
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        let mut sp = self.speed_ui;
                        let res = knob(
                            ui, &mut sp, 25.0, 300.0, 56.0, theme::accent(), 100.0,
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
                                if active { theme::accent() } else { theme::edge() },
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
                            Self::caption(ui, "SYNC");
                            for mode in [Mode::Complement, Mode::Augment, Mode::Music] {
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

                // ---- scenes + lights ----------------------------------------
                ui.set_opacity(self.intro_alpha(3));
                {
                        const GAP: f32 = 6.0;

                        // Scenes: a responsive grid whose tiles grow to fill the
                        // row exactly (like the light cards below), so both left
                        // and right margins stay equal at any width. Column count
                        // comes from a ~150px target; tiles keep their aspect.
                        // My scenes: a save row + a grid of the user's saved
                        // scenes, sized like the built-ins.
                        Self::caption(ui, "MY SCENES");
                        ui.add_space(4.0);
                        ui.horizontal(|ui| {
                            ui.add(
                                egui::TextEdit::singleline(&mut self.custom_name)
                                    .hint_text("name this look")
                                    .desired_width(160.0),
                            );
                            if self.pill(ui, "Save current", false).clicked() {
                                acts.push(Act::SaveScene);
                            }
                        });
                        if !self.custom_scenes.is_empty() {
                            ui.add_space(6.0);
                            let avail = ui.available_width();
                            let c_cols =
                                (((avail + GAP) / (TILE_W + GAP)).floor() as usize).max(1);
                            let cw = (avail - GAP * (c_cols as f32 - 1.0)) / c_cols as f32;
                            let csz = Vec2::new(cw, cw * (TILE_H / TILE_W));
                            let n = self.custom_scenes.len();
                            for row_start in (0..n).step_by(c_cols) {
                                ui.horizontal(|ui| {
                                    ui.spacing_mut().item_spacing.x = GAP;
                                    for i in row_start..(row_start + c_cols).min(n) {
                                        let (resp, del) =
                                            self.custom_tile(ui, &self.custom_scenes[i], csz);
                                        if del {
                                            acts.push(Act::DeleteScene(i));
                                        } else if resp.clicked() {
                                            acts.push(Act::ApplyCustom(i));
                                        }
                                    }
                                });
                                ui.add_space(GAP);
                            }
                        }
                        ui.add_space(8.0);

                        Self::caption(ui, "SCENES");
                        ui.add_space(4.0);
                        let avail = ui.available_width();
                        let s_cols = (((avail + GAP) / (TILE_W + GAP)).floor() as usize).max(1);
                        let tile_w = (avail - GAP * (s_cols as f32 - 1.0)) / s_cols as f32;
                        let tile_sz = Vec2::new(tile_w, tile_w * (TILE_H / TILE_W));
                        for row in SCENES.chunks(s_cols) {
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = GAP;
                                for sc in row {
                                    let idx = SCENES.iter().position(|s| s.name == sc.name).unwrap();
                                    if self.scene_tile(ui, sc, tile_sz).clicked() {
                                        acts.push(Act::Scene(idx));
                                    }
                                }
                            });
                            ui.add_space(GAP);
                        }
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
                                .color(theme::muted()),
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

                        ui.add_space(10.0);
                        self.schedule_ui(ui, &mut acts);
                }
                }); // scroll area
            });

        // The add-a-brand dialog sits above the app but below the splash.
        let mut dlg_acts: Vec<Act> = Vec::new();
        self.dialog_ui(ctx, &mut dlg_acts);
        acts.extend(dlg_acts);

        for act in acts {
            self.act(ctx, act);
        }

        // Launch animation, painted over the app (which lays itself out and
        // finishes discovery underneath) and cross-fading away.
        self.splash.show(ctx);
    }
}

impl Drop for VesperApp {
    fn drop(&mut self) {
        self.driver.stop_current();
        self.audio.stop();
        self.noise.stop();
        self.music.stop();
    }
}

fn main() -> eframe::Result<()> {
    let root = util::assets_root();

    // Headless diagnostic: `vesper.exe --discover [out.txt]` runs one
    // discovery pass and writes what it found (the GUI subsystem has no
    // console to print to).
    let args: Vec<String> = std::env::args().collect();

    // Headless apply: `vesper.exe --apply "<scene>"` (or "All Off") discovers
    // the lights, applies the scene, and exits. This is what the scheduled
    // Task Scheduler entries run, so schedules fire without the GUI open.
    if args.get(1).map(String::as_str) == Some("--apply") {
        let name = args.get(2).cloned().unwrap_or_default();
        let ctrl = Controller::new(&root);
        let lights = ctrl.discover();
        if name.eq_ignore_ascii_case("All Off") || name.eq_ignore_ascii_case("off") {
            for l in &lights {
                ctrl.set_power(l, false, 400);
            }
        } else if let Some(sc) = SCENES.iter().find(|s| s.name == name) {
            apply_scene(&ctrl, &lights, sc, true);
        } else if let Some(cs) =
            custom::load(&root.join("custom_scenes.json")).iter().find(|c| c.name == name)
        {
            custom::apply(&ctrl, &lights, cs, true);
        }
        return Ok(());
    }

    // Headless audio-loopback self-test: play a tone and check the music
    // capture's level responds. `vesper.exe --audiocheck [out.txt]`.
    if args.get(1).map(String::as_str) == Some("--audiocheck") {
        use rodio::Source;
        let mut maxv = 0u32;
        let started;
        {
            let (_stream, handle) = rodio::OutputStream::try_default().expect("audio out");
            let sink = rodio::Sink::try_new(&handle).expect("sink");
            sink.append(
                rodio::source::SineWave::new(440.0)
                    .amplify(0.3)
                    .take_duration(Duration::from_secs(3)),
            );
            let mut m = music::Music::new();
            started = m.start();
            let lvl = m.level_handle();
            for _ in 0..28 {
                std::thread::sleep(Duration::from_millis(100));
                maxv = maxv.max(lvl.load(std::sync::atomic::Ordering::Relaxed));
            }
        }
        let path = args.get(2).cloned().unwrap_or_else(|| "audiocheck.txt".into());
        let _ = std::fs::write(
            path,
            format!(
                "loopback stream started: {started}\n\
                 max captured level (0..1000): {maxv}\n\
                 verdict: {}\n",
                if maxv > 20 { "LOOPBACK WORKS" } else { "no signal captured" }
            ),
        );
        return Ok(());
    }

    // Headless check that every noise layer actually reaches the speakers -
    // synthesised and recorded alike - by raising each one alone and watching
    // the loopback capture. `vesper.exe --noisecheck [out.txt]`.
    if args.get(1).map(String::as_str) == Some("--noisecheck") {
        let mut m = music::Music::new();
        let started = m.start();
        let lvl = m.level_handle();
        let mut n = noise::Noise::new(root.join("audio"));
        n.set_master(100.0);
        let mut report = format!("loopback started: {started}\n");
        for k in noise::KINDS {
            n.set_level(k, 100.0);
            std::thread::sleep(Duration::from_millis(400)); // let it spin up
            let mut peak = 0u32;
            for _ in 0..12 {
                std::thread::sleep(Duration::from_millis(80));
                peak = peak.max(lvl.load(std::sync::atomic::Ordering::Relaxed));
            }
            n.set_level(k, 0.0);
            std::thread::sleep(Duration::from_millis(250)); // let it decay
            report.push_str(&format!(
                "  {:10} {:>5}  {}  {}\n",
                k.label(),
                peak,
                if k.sample_name().is_some() { "recorded " } else { "synth    " },
                if peak > 15 { "AUDIBLE" } else { "SILENT" }
            ));
        }
        n.stop();
        let path = args.get(2).cloned().unwrap_or_else(|| "noisecheck.txt".into());
        let _ = std::fs::write(path, report);
        return Ok(());
    }

    // Headless check of "add by IP": `vesper.exe --probe <ip> [out.txt]`.
    if args.get(1).map(String::as_str) == Some("--probe") {
        let ip = args.get(2).cloned().unwrap_or_default();
        let ctrl = Controller::new(&root);
        let t = Instant::now();
        let found = ctrl.add_by_ip(&ip);
        let out = match &found {
            Some(l) => format!(
                "FOUND {} '{}' at {} in {:?}\n  power={} hsbk={:?}\n",
                l.vendor.name(),
                l.label(),
                l.ip,
                t.elapsed(),
                l.power(),
                l.hsbk()
            ),
            None => format!("nothing answered at {ip} ({:?})\n", t.elapsed()),
        };
        let path = args.get(3).cloned().unwrap_or_else(|| "probe.txt".into());
        let _ = std::fs::write(path, out);
        return Ok(());
    }

    // Headless check that every noise scene is audible, and roughly how loud
    // each sits. `vesper.exe --scenecheck [out.txt]`.
    if args.get(1).map(String::as_str) == Some("--scenecheck") {
        let mut m = music::Music::new();
        let started = m.start();
        let lvl = m.level_handle();
        let mut n = noise::Noise::new(root.join("audio"));
        n.set_master(100.0);
        let mut report = format!("loopback started: {started}\n");
        for sc in noise::SCENES {
            n.apply_scene(sc);
            std::thread::sleep(Duration::from_millis(500));
            let mut peak = 0u32;
            for _ in 0..14 {
                std::thread::sleep(Duration::from_millis(80));
                peak = peak.max(lvl.load(std::sync::atomic::Ordering::Relaxed));
            }
            report.push_str(&format!(
                "  {:14} {:>5}  {}  [{}]\n",
                sc.name,
                peak,
                if peak > 15 { "AUDIBLE" } else { "SILENT " },
                sc.mix
                    .iter()
                    .map(|(k, v)| format!("{} {}", k.label(), *v as i32))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
            n.clear();
            std::thread::sleep(Duration::from_millis(300));
        }
        n.stop();
        let path = args.get(2).cloned().unwrap_or_else(|| "scenecheck.txt".into());
        let _ = std::fs::write(path, report);
        return Ok(());
    }

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
    // Past the headless modes: only one GUI at a time. A second launch brings
    // the running one forward (it's probably hidden in the tray, which is
    // exactly when someone clicks the shortcut again) rather than stacking up
    // another tray icon, audio engine and set of commands to the same bulbs.
    if single::already_running("Vesper") {
        return Ok(());
    }

    // Wide enough for the toolbar's full run of buttons (measured at ~578px
    // of content plus margins); everything below wraps or scrolls, so this is
    // a comfortable size rather than a required one.
    let mut viewport = egui::ViewportBuilder::default()
        .with_inner_size([660.0, 900.0])
        .with_min_inner_size([430.0, 520.0])
        .with_title("Vesper");
    if let Some(icon) = load_icon(&root) {
        viewport = viewport.with_icon(Arc::new(icon));
    }
    let options = eframe::NativeOptions {
        viewport,
        // Don't restore the saved window geometry: if the window was last
        // closed on a monitor that's since been unplugged (or moved partly
        // off-screen), it comes back somewhere you can't reach it.
        persist_window: false,
        ..Default::default()
    };
    eframe::run_native(
        "Vesper",
        options,
        Box::new(move |cc| Ok(Box::new(VesperApp::new(cc, root)))),
    )
}
