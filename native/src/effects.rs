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
    Flicker,
    Strobe,
    Complement,
    Augment,
}

impl Mode {
    pub fn label(self) -> &'static str {
        match self {
            Mode::Breathe => "Breathe",
            Mode::Flow => "Flow",
            Mode::Undulate => "Undulate",
            Mode::Flicker => "Flicker",
            Mode::Strobe => "Strobe",
            Mode::Complement => "Complement",
            Mode::Augment => "Augment",
        }
    }

    fn fps(self) -> f32 {
        match self {
            Mode::Flicker | Mode::Strobe | Mode::Complement | Mode::Augment => 12.0,
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
            // lights like a wave rather than pulsing them in unison.
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
                let phase = 2.0 * std::f32::consts::PI * t / 6.0 - i as f32 * 0.9;
                let m = 0.12 + 0.88 * (0.5 - 0.5 * phase.cos());
                ctrl.stream_color(l, c[0], c[1], (c[2] as f32 * m) as u16, c[3], 280);
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
}

impl Driver {
    pub fn new() -> Driver {
        Driver {
            mode: None,
            stop: Arc::new(AtomicBool::new(false)),
            thread: None,
            speed_milli: Arc::new(AtomicU32::new(1000)),
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
