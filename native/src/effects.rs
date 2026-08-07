//! Live light animations + screen-sync, ported from effects.py, plus the
//! background driver thread that runs them (the LightDriver analogue).
//!
//! Every effect works from `base` - each bulb's colour when the effect
//! started (the presently selected scene) - and treats that brightness as
//! the ceiling. Effects never impose their own palette.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::controller::Controller;
use crate::light::Light;
use crate::screen::sample_screen;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Breathe,
    Flow,
    Undulate,
    Chase,
    Twinkle,
    Flicker,
    Strobe,
    Complement,
    Augment,
    Music,
}

impl Mode {
    pub fn label(self) -> &'static str {
        match self {
            Mode::Breathe => "Breathe",
            Mode::Flow => "Flow",
            Mode::Undulate => "Undulate",
            Mode::Chase => "Chase",
            Mode::Twinkle => "Twinkle",
            Mode::Flicker => "Flicker",
            Mode::Strobe => "Strobe",
            Mode::Complement => "Complement",
            Mode::Augment => "Augment",
            Mode::Music => "Music",
        }
    }

    fn fps(self) -> f32 {
        match self {
            Mode::Music => 20.0,
            Mode::Flicker | Mode::Strobe | Mode::Chase | Mode::Complement
            | Mode::Augment => 12.0,
            _ => 10.0,
        }
    }
}

struct FxState {
    base: HashMap<String, [u16; 4]>,
    ft: f32,
    h: Option<f32>,
    s: f32,
    v: f32,
    last: Option<(i32, i32, i32)>,
    music: Arc<AtomicU32>,
    music_tone: Arc<AtomicU32>,
    hue_swing: f32,
    /// Undulate's brightness floor, as a fraction of the scene's own max
    /// (x1000, live-adjustable via the SWING knob).
    brightness_swing: Arc<AtomicU32>,
}

fn hsbk_f(h: f32, s: f32, v: f32) -> [u16; 4] {
    [
        ((h * 65535.0) as i64 & 0xffff) as u16,
        (s.clamp(0.0, 1.0) * 65535.0) as u16,
        (v.clamp(0.0, 1.0) * 65535.0) as u16,
        3500,
    ]
}

fn lerp_hsbk(a: [u16; 4], b: [u16; 4], f: f32) -> [u16; 4] {
    // Shortest hue path around the wheel.
    let dh = ((b[0] as i32 - a[0] as i32 + 32768).rem_euclid(65536)) - 32768;
    let hue = (a[0] as i32 + (dh as f32 * f) as i32).rem_euclid(65536) as u16;
    [
        hue,
        (a[1] as f32 + (b[1] as f32 - a[1] as f32) * f) as u16,
        (a[2] as f32 + (b[2] as f32 - a[2] as f32) * f) as u16,
        (a[3] as f32 + (b[3] as f32 - a[3] as f32) * f) as u16,
    ]
}

fn frame(mode: Mode, ctrl: &Controller, lights: &[Arc<Light>], t: f32, st: &mut FxState) {
    match mode {
        Mode::Breathe => {
            let m = 0.12
                + 0.88 * (0.5 - 0.5 * (2.0 * std::f32::consts::PI * t / 6.0).cos());
            for l in lights {
                if let Some(&[h, s, b, k]) = st.base.get(&l.id) {
                    ctrl.stream_color(l, h, s, (b as f32 * m) as u16, k, 260);
                }
            }
        }
        Mode::Flow => {
            // Flow the scene's own colours across the bulbs (no rainbow).
            let palette: Vec<[u16; 4]> = lights
                .iter()
                .filter_map(|l| st.base.get(&l.id).copied())
                .collect();
            let n = palette.len();
            if n == 0 {
                return;
            }
            for (i, l) in lights.iter().enumerate() {
                let p = t * 0.2 + i as f32;
                let i0 = (p as usize) % n;
                let c = lerp_hsbk(palette[i0], palette[(i0 + 1) % n], p.fract());
                ctrl.stream_color(l, c[0], c[1], c[2], c[3], 300);
            }
        }
        Mode::Undulate => {
            // Flow's drifting colour + breathe's brightness envelope, with the
            // breath phase-shifted per bulb so the swell rolls across the
            // lights like a wave rather than pulsing them in unison. Swings
            // between the scene's own max and a floor set by the SWING knob
            // (e.g. 65% means it breathes between 100% and 65% of max).
            let palette: Vec<[u16; 4]> = lights
                .iter()
                .filter_map(|l| st.base.get(&l.id).copied())
                .collect();
            let n = palette.len();
            if n == 0 {
                return;
            }
            let floor = (st.brightness_swing.load(Ordering::Relaxed) as f32 / 1000.0)
                .clamp(0.0, 1.0);
            for (i, l) in lights.iter().enumerate() {
                let p = t * 0.2 + i as f32;
                let i0 = (p as usize) % n;
                let c = lerp_hsbk(palette[i0], palette[(i0 + 1) % n], p.fract());
                let phase = 2.0 * std::f32::consts::PI * t / 6.0 - i as f32 * 0.9;
                let m = floor + (1.0 - floor) * (0.5 - 0.5 * phase.cos());
                ctrl.stream_color(l, c[0], c[1], (c[2] as f32 * m) as u16, c[3], 280);
            }
        }
        Mode::Chase => {
            // A bright bulb travels the row, the rest held low. Distance is
            // measured the short way round so it wraps rather than snapping
            // back at the end.
            let n = lights.len();
            if n == 0 {
                return;
            }
            let pos = (t * 2.0) % n as f32;
            for (i, l) in lights.iter().enumerate() {
                if let Some(&[h, s, b, k]) = st.base.get(&l.id) {
                    let d = (i as f32 - pos).abs();
                    let dist = d.min(n as f32 - d);
                    let m = if dist < 1.0 { 1.0 - 0.85 * dist } else { 0.15 };
                    ctrl.stream_color(l, h, s, (b as f32 * m) as u16, k, 170);
                }
            }
        }
        Mode::Twinkle => {
            // Each bulb swells on its own slow cycle. The golden angle keeps
            // the phases from lining up, and squaring the wave makes it linger
            // dim and brighten briefly - a sparkle, not a pulse. Never reaches
            // black, so it stays usable as room light.
            for (i, l) in lights.iter().enumerate() {
                if let Some(&[h, s, b, k]) = st.base.get(&l.id) {
                    let phase = i as f32 * 2.399963;
                    let rate = 0.55 + 0.25 * ((i * 7919) % 13) as f32 / 13.0;
                    let w = 0.5
                        + 0.5 * (2.0 * std::f32::consts::PI * t * rate + phase).sin();
                    let m = 0.45 + 0.55 * (w * w);
                    ctrl.stream_color(l, h, s, (b as f32 * m) as u16, k, 320);
                }
            }
        }
        Mode::Flicker => {
            if t - st.ft < 0.09 {
                return; // gate by effect-time so SPEED applies
            }
            st.ft = t;
            let mut rng = fastrand::Rng::new();
            for l in lights {
                if let Some(&[h, s, b, k]) = st.base.get(&l.id) {
                    let f = 0.45 + rng.f32() * 0.55;
                    ctrl.stream_color(l, h, s, (b as f32 * f) as u16, k, 110);
                }
            }
        }
        Mode::Strobe => {
            let on = (t * 8.0) as i64 % 2 == 0;
            for l in lights {
                if let Some(&[h, s, b, k]) = st.base.get(&l.id) {
                    ctrl.stream_color(l, h, s, if on { b } else { 0 }, k, 30);
                }
            }
        }
        Mode::Complement | Mode::Augment => {
            let Some((hue, sat, val)) = sample_screen() else { return };
            let (hue, sat, val) = if mode == Mode::Complement {
                (
                    (hue + 0.5).rem_euclid(1.0),
                    (sat * 1.25 + 0.2).min(1.0),
                    (val * 1.1 + 0.1).clamp(0.25, 1.0),
                )
            } else {
                (
                    hue,
                    (sat * 1.7 + 0.15).min(1.0),
                    (val * 1.15 + 0.15).clamp(0.3, 1.0),
                )
            };
            push(ctrl, lights, hue, sat, val, st, 0.4);
        }
        Mode::Music => {
            // Scene colours drift slowly (as in flow) while every bulb's
            // brightness tracks the loudness of the PC's audio - and the hue
            // swings with the music's spectral balance: bass pushes the
            // colours warm/backward round the wheel, treble pushes them
            // cool/forward. The swing is eased so it glides rather than
            // twitching frame to frame.
            let lvl = st.music.load(Ordering::Relaxed) as f32 / 1000.0;
            let tone = st.music_tone.load(Ordering::Relaxed) as f32 / 1000.0;
            let m = 0.15 + 0.85 * lvl;
            // Louder passages swing further, so quiet parts stay near the
            // scene's own colours.
            let target = (tone - 0.5) * 2.0 * 0.16 * (0.35 + 0.65 * lvl);
            st.hue_swing += (target - st.hue_swing) * 0.15;
            let shift = (st.hue_swing * 65536.0) as i32;

            let palette: Vec<[u16; 4]> = lights
                .iter()
                .filter_map(|l| st.base.get(&l.id).copied())
                .collect();
            let n = palette.len();
            if n == 0 {
                return;
            }
            for (i, l) in lights.iter().enumerate() {
                let p = t * 0.15 + i as f32;
                let i0 = (p as usize) % n;
                let c = lerp_hsbk(palette[i0], palette[(i0 + 1) % n], p.fract());
                let hue = (c[0] as i32 + shift).rem_euclid(65536) as u16;
                // Lift saturation a little as it swings so the shift reads as
                // colour rather than a muddy tint.
                let sat = (c[1] as f32 + (65535.0 - c[1] as f32) * 0.25 * lvl) as u16;
                ctrl.stream_color(l, hue, sat, (c[2] as f32 * m) as u16, c[3], 90);
            }
        }
    }
}

/// Ease the target toward the sampled colour (exponential smoothing) so
/// screen motion flows instead of jumping; hue eases along the shortest
/// path around the wheel.
fn push(
    ctrl: &Controller,
    lights: &[Arc<Light>],
    hue: f32,
    sat: f32,
    val: f32,
    st: &mut FxState,
    ease: f32,
) {
    let (sh, ss, sv) = match st.h {
        None => (hue, sat, val),
        Some(ph) => {
            let dh = (hue - ph + 0.5).rem_euclid(1.0) - 0.5;
            (
                (ph + dh * ease).rem_euclid(1.0),
                st.s + (sat - st.s) * ease,
                st.v + (val - st.v) * ease,
            )
        }
    };
    st.h = Some(sh);
    st.s = ss;
    st.v = sv;
    let key = (
        (sh * 1000.0).round() as i32,
        (ss * 1000.0).round() as i32,
        (sv * 1000.0).round() as i32,
    );
    if st.last == Some(key) {
        return; // settled - nothing to send
    }
    st.last = Some(key);
    let [h, s, b, k] = hsbk_f(sh, ss, sv);
    for l in lights {
        ctrl.stream_color(l, h, s, b, k, 220);
    }
}

// --------------------------------------------------------------------------
// Driver
// --------------------------------------------------------------------------

/// Runs one effect/sync at a time on a background thread, streaming colours
/// to every bulb at the mode's frame rate, with a live speed multiplier.
pub struct Driver {
    pub mode: Option<Mode>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    /// Speed multiplier x1000, shared with the running thread.
    pub speed_milli: Arc<AtomicU32>,
    /// Undulate's brightness floor (fraction of scene max, x1000), shared
    /// with the running thread so the SWING knob applies live.
    pub swing_milli: Arc<AtomicU32>,
    /// Music loudness envelope (0..1000), written by the audio capture.
    music_level: Arc<AtomicU32>,
    /// Music spectral balance (0 = bass .. 1000 = treble).
    music_tone: Arc<AtomicU32>,
}

impl Driver {
    pub fn new(music_level: Arc<AtomicU32>, music_tone: Arc<AtomicU32>) -> Driver {
        Driver {
            mode: None,
            stop: Arc::new(AtomicBool::new(false)),
            thread: None,
            speed_milli: Arc::new(AtomicU32::new(1000)),
            swing_milli: Arc::new(AtomicU32::new(650)),
            music_level,
            music_tone,
        }
    }

    pub fn start(&mut self, mode: Mode, ctrl: Arc<Controller>, lights: Vec<Arc<Light>>) {
        self.stop_current();
        if lights.is_empty() {
            return;
        }
        let base: HashMap<String, [u16; 4]> =
            lights.iter().map(|l| (l.id.clone(), l.hsbk())).collect();
        let stop = Arc::new(AtomicBool::new(false));
        self.stop = stop.clone();
        let speed = self.speed_milli.clone();
        let swing = self.swing_milli.clone();
        let music = self.music_level.clone();
        let music_tone = self.music_tone.clone();
        self.mode = Some(mode);
        let interval = Duration::from_secs_f32(1.0 / mode.fps());
        self.thread = Some(std::thread::spawn(move || {
            let mut st = FxState {
                base,
                ft: -1.0,
                h: None,
                s: 0.0,
                v: 0.0,
                last: None,
                music,
                music_tone,
                hue_swing: 0.0,
                brightness_swing: swing,
            };
            let mut prev = Instant::now();
            let mut et = 0.0f32; // speed-scaled "effect time"
            while !stop.load(Ordering::Relaxed) {
                let now = Instant::now();
                et += now.duration_since(prev).as_secs_f32()
                    * (speed.load(Ordering::Relaxed) as f32 / 1000.0);
                prev = now;
                frame(mode, &ctrl, &lights, et, &mut st);
                std::thread::sleep(interval);
            }
        }));
    }

    pub fn stop_current(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        self.mode = None;
    }
}

impl Drop for Driver {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        // Don't join in drop: the thread exits on its own within a frame.
    }
}
