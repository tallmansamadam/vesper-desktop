//! Layered noise / ambient generator: several textures synthesised in real
//! time and mixed into one bed, each with its own level, the whole thing
//! scaled by the master Volume. Independent of the per-scene
//! [`crate::ambience`] so it layers under any scene's soundscape.
//!
//! One persistent [`NoiseGen`] source runs on rodio's audio thread and reads a
//! shared atomic level per layer plus a master gain, so the UI knobs retune
//! the mix live without restarting anything. The sink is paused whenever every
//! layer (or the master) is at zero, so silence costs nothing.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

use rodio::{OutputStream, Sink, Source};

const SR: u32 = 44_100;
const SRF: f32 = 44_100.0;
const TAU: f32 = std::f32::consts::TAU;
pub const N: usize = 10;

#[derive(Clone, Copy)]
pub enum NoiseKind {
    White,
    Pink,
    Brown,
    Surf,
    Rain,
    Wind,
    Waterfall,
    Fountain,
    Chimes,
    Birds,
}

pub const KINDS: [NoiseKind; N] = [
    NoiseKind::White,
    NoiseKind::Pink,
    NoiseKind::Brown,
    NoiseKind::Surf,
    NoiseKind::Rain,
    NoiseKind::Wind,
    NoiseKind::Waterfall,
    NoiseKind::Fountain,
    NoiseKind::Chimes,
    NoiseKind::Birds,
];

impl NoiseKind {
    pub fn label(self) -> &'static str {
        match self {
            NoiseKind::White => "White",
            NoiseKind::Pink => "Pink",
            NoiseKind::Brown => "Brown",
            NoiseKind::Surf => "Surf",
            NoiseKind::Rain => "Rain",
            NoiseKind::Wind => "Wind",
            NoiseKind::Waterfall => "Waterfall",
            NoiseKind::Fountain => "Fountain",
            NoiseKind::Chimes => "Chimes",
            NoiseKind::Birds => "Birds",
        }
    }

    fn idx(self) -> usize {
        self as usize
    }
}

/// Paul Kellet's refined pink-noise filter: shapes white -> ~-3dB/octave.
fn pink_next(b: &mut [f32; 7], w: f32) -> f32 {
    b[0] = 0.99886 * b[0] + w * 0.0555179;
    b[1] = 0.99332 * b[1] + w * 0.0750759;
    b[2] = 0.96900 * b[2] + w * 0.1538520;
    b[3] = 0.86650 * b[3] + w * 0.3104856;
    b[4] = 0.55000 * b[4] + w * 0.5329522;
    b[5] = -0.7616 * b[5] - w * 0.0168980;
    let out = b[0] + b[1] + b[2] + b[3] + b[4] + b[5] + b[6] + w * 0.5362;
    b[6] = w * 0.115926;
    out * 0.11
}

/// A single decaying sine, used for tonal event voices (chimes, bird chirps,
/// fountain droplets). Inactive when its amplitude has decayed away.
#[derive(Clone, Copy, Default)]
struct Voice {
    phase: f32,
    freq: f32,
    sweep: f32, // Hz added per sample, for chirps
    amp: f32,
    decay: f32,
}

impl Voice {
    fn active(&self) -> bool {
        self.amp > 0.0005
    }

    fn tick(&mut self) -> f32 {
        if !self.active() {
            self.amp = 0.0;
            return 0.0;
        }
        let s = (self.phase * TAU).sin() * self.amp;
        self.phase = (self.phase + self.freq / SRF).fract();
        self.freq += self.sweep;
        self.amp *= self.decay;
        s
    }
}

fn spawn(pool: &mut [Voice], v: Voice) {
    if let Some(slot) = pool.iter_mut().find(|s| !s.active()) {
        *slot = v;
    }
}

// C-major-pentatonic across two octaves, for the chimes.
const CHIME_FREQS: [f32; 10] = [
    523.25, 587.33, 659.25, 783.99, 880.0, 1046.5, 1174.66, 1318.51, 1567.98, 1760.0,
];

struct NoiseGen {
    levels: [Arc<AtomicU32>; N], // milli, 0..1000 == 0..100%
    master: Arc<AtomicU32>,      // milli, scales the whole mix
    rng: fastrand::Rng,
    // steady-noise filter state
    pink: [f32; 7],
    brown: f32,
    surf_pink: [f32; 7],
    surf_phase: f32,
    rain_prev: f32,
    wind_pink: [f32; 7],
    wind_lp: f32,
    wind_cut_phase: f32,
    wind_gust_phase: f32,
    wfall_pink: [f32; 7],
    // tonal event voices
    chimes: [Voice; 6],
    birds: [Voice; 4],
    drops: [Voice; 8],
}

impl NoiseGen {
    fn new(levels: [Arc<AtomicU32>; N], master: Arc<AtomicU32>) -> NoiseGen {
        NoiseGen {
            levels,
            master,
            rng: fastrand::Rng::new(),
            pink: [0.0; 7],
            brown: 0.0,
            surf_pink: [0.0; 7],
            surf_phase: 0.0,
            rain_prev: 0.0,
            wind_pink: [0.0; 7],
            wind_lp: 0.0,
            wind_cut_phase: 0.0,
            wind_gust_phase: 0.0,
            wfall_pink: [0.0; 7],
            chimes: [Voice::default(); 6],
            birds: [Voice::default(); 4],
            drops: [Voice::default(); 8],
        }
    }

    fn lvl(&self, i: usize) -> f32 {
        self.levels[i].load(Ordering::Relaxed) as f32 / 1000.0
    }

    fn white(&mut self) -> f32 {
        self.rng.f32() * 2.0 - 1.0
    }

    /// Steady "rushing water" bed shared by Waterfall and Fountain.
    fn water_bed(&mut self) -> f32 {
        let w = self.white();
        let wp = pink_next(&mut self.wfall_pink, w);
        0.55 * w + 0.45 * wp
    }
}

impl Iterator for NoiseGen {
    type Item = f32;

    fn next(&mut self) -> Option<f32> {
        let master = self.master.load(Ordering::Relaxed) as f32 / 1000.0;
        let w = self.white();
        let mut mix = 0.0;

        let lw = self.lvl(0);
        if lw > 0.0 {
            mix += lw * w * 0.35;
        }

        let p = pink_next(&mut self.pink, w);
        mix += self.lvl(1) * p;

        self.brown = (self.brown + 0.02 * w).clamp(-1.0, 1.0);
        mix += self.lvl(2) * self.brown * 3.5;

        let sp = pink_next(&mut self.surf_pink, w);
        self.surf_phase = (self.surf_phase + TAU * 0.09 / SRF) % TAU;
        let env = 0.5 + 0.5 * self.surf_phase.sin();
        mix += self.lvl(3) * sp * env * 1.3;

        let rain = (w - self.rain_prev) * 0.5;
        self.rain_prev = w;
        mix += self.lvl(4) * rain;

        // Wind: pink through a slowly-wandering low-pass, swelling in gusts.
        let lwd = self.lvl(5);
        if lwd > 0.0 {
            let w2 = self.white();
            let wp = pink_next(&mut self.wind_pink, w2);
            self.wind_cut_phase = (self.wind_cut_phase + TAU * 0.05 / SRF) % TAU;
            let cut = 0.03 + 0.05 * (0.5 + 0.5 * self.wind_cut_phase.sin());
            self.wind_lp += cut * (wp - self.wind_lp);
            self.wind_gust_phase = (self.wind_gust_phase + TAU * 0.08 / SRF) % TAU;
            let gust = 0.35 + 0.65 * (0.5 + 0.5 * self.wind_gust_phase.sin());
            mix += lwd * self.wind_lp * gust * 4.0;
        }

        // Waterfall: steady bright rushing bed.
        let lwf = self.lvl(6);
        let lf = self.lvl(7);
        let bed = if lwf > 0.0 || lf > 0.0 {
            self.water_bed()
        } else {
            0.0
        };
        mix += lwf * bed * 0.8;

        // Fountain: quieter water bed + sparkling droplets.
        if lf > 0.0 {
            if self.rng.f32() < 3.0e-4 {
                let f = 2500.0 + self.rng.f32() * 3000.0;
                let decay = 0.02f32.powf(1.0 / (0.03 * SRF));
                spawn(
                    &mut self.drops,
                    Voice { phase: 0.0, freq: f, sweep: 0.0, amp: 0.18, decay },
                );
            }
            let d: f32 = self.drops.iter_mut().map(Voice::tick).sum();
            mix += lf * (bed * 0.4 + d);
        }

        // Chimes: occasional struck pentatonic tones with a long decay.
        let lc = self.lvl(8);
        if lc > 0.0 {
            if self.rng.f32() < 8.0e-6 {
                let f = CHIME_FREQS[self.rng.usize(0..CHIME_FREQS.len())];
                let decay = 0.02f32.powf(1.0 / (1.8 * SRF));
                spawn(
                    &mut self.chimes,
                    Voice { phase: 0.0, freq: f, sweep: 0.0, amp: 0.45, decay },
                );
            }
            let c: f32 = self.chimes.iter_mut().map(Voice::tick).sum();
            mix += lc * c;
        }

        // Birds: occasional short frequency-swept chirps.
        let lbd = self.lvl(9);
        if lbd > 0.0 {
            if self.rng.f32() < 7.0e-6 {
                let f0 = 2200.0 + self.rng.f32() * 2200.0;
                let f1 = if self.rng.bool() { f0 * 1.6 } else { f0 * 0.6 };
                let dur = 0.08 + self.rng.f32() * 0.12;
                let sweep = (f1 - f0) / (dur * SRF);
                let decay = 0.02f32.powf(1.0 / (dur * SRF));
                spawn(
                    &mut self.birds,
                    Voice { phase: 0.0, freq: f0, sweep, amp: 0.3, decay },
                );
            }
            let b: f32 = self.birds.iter_mut().map(Voice::tick).sum();
            mix += lbd * b;
        }

        Some((mix * 0.9 * master).clamp(-1.0, 1.0))
    }
}

impl Source for NoiseGen {
    fn current_frame_len(&self) -> Option<usize> {
        None
    }
    fn channels(&self) -> u16 {
        1
    }
    fn sample_rate(&self) -> u32 {
        SR
    }
    fn total_duration(&self) -> Option<Duration> {
        None
    }
}

pub struct Noise {
    _stream: Option<OutputStream>,
    sink: Option<Sink>,
    levels: [Arc<AtomicU32>; N],
    master: Arc<AtomicU32>,
    playing: bool,
}

impl Noise {
    pub fn new() -> Noise {
        let levels: [Arc<AtomicU32>; N] = std::array::from_fn(|_| Arc::new(AtomicU32::new(0)));
        let master = Arc::new(AtomicU32::new(600)); // tracks the Volume knob (60% default)
        let (stream, sink) = match OutputStream::try_default() {
            Ok((stream, handle)) => {
                let sink = Sink::try_new(&handle).ok();
                if let Some(sink) = &sink {
                    sink.append(NoiseGen::new(levels.clone(), master.clone()));
                    sink.pause();
                }
                (Some(stream), sink)
            }
            Err(_) => (None, None), // no audio device: degrade silently
        };
        Noise {
            _stream: stream,
            sink,
            levels,
            master,
            playing: false,
        }
    }

    pub fn level_pct(&self, k: NoiseKind) -> f32 {
        self.levels[k.idx()].load(Ordering::Relaxed) as f32 / 10.0
    }

    pub fn set_level(&mut self, k: NoiseKind, pct: f32) {
        self.levels[k.idx()].store((pct.clamp(0.0, 100.0) * 10.0) as u32, Ordering::Relaxed);
        self.update_playing();
    }

    /// Master gain, driven by the Volume knob (0..100).
    pub fn set_master(&mut self, pct: f32) {
        self.master
            .store((pct.clamp(0.0, 100.0) * 10.0) as u32, Ordering::Relaxed);
        self.update_playing();
    }

    /// True when any layer is up and the master is open (used to light the
    /// header and to decide whether the sink should run).
    pub fn active(&self) -> bool {
        self.master.load(Ordering::Relaxed) > 0
            && self.levels.iter().any(|l| l.load(Ordering::Relaxed) > 0)
    }

    fn update_playing(&mut self) {
        if let Some(sink) = &self.sink {
            match (self.active(), self.playing) {
                (true, false) => {
                    sink.play();
                    self.playing = true;
                }
                (false, true) => {
                    sink.pause();
                    self.playing = false;
                }
                _ => {}
            }
        }
    }

    pub fn stop(&mut self) {
        if let Some(sink) = &self.sink {
            sink.pause();
        }
        self.playing = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gen(levels_pct: [f32; N]) -> NoiseGen {
        NoiseGen::new(
            std::array::from_fn(|i| Arc::new(AtomicU32::new((levels_pct[i] * 10.0) as u32))),
            Arc::new(AtomicU32::new(1000)), // master fully open for tests
        )
    }

    fn one(kind: usize, pct: f32) -> [f32; N] {
        let mut a = [0.0; N];
        a[kind] = pct;
        a
    }

    fn take(g: &mut NoiseGen, n: usize) -> Vec<f32> {
        (0..n).map(|_| g.next().unwrap()).collect()
    }

    fn zero_cross_rate(s: &[f32]) -> f32 {
        let crossings = s.windows(2).filter(|w| (w[0] <= 0.0) != (w[1] <= 0.0)).count();
        crossings as f32 / s.len() as f32
    }

    fn rms(s: &[f32]) -> f32 {
        (s.iter().map(|x| x * x).sum::<f32>() / s.len() as f32).sqrt()
    }

    #[test]
    fn output_is_bounded_and_audible() {
        let s = take(&mut gen(one(0, 100.0)), 2000);
        assert!(s.iter().all(|x| x.abs() <= 1.0), "clipped past [-1,1]");
        assert!(rms(&s) > 0.01, "white noise too quiet");
    }

    #[test]
    fn all_layers_zero_is_pure_silence() {
        let s = take(&mut gen([0.0; N]), 500);
        assert!(s.iter().all(|x| *x == 0.0), "expected silence");
    }

    #[test]
    fn master_gain_scales_output() {
        let levels = one(0, 100.0);
        let mut full = gen(levels);
        let mut half = NoiseGen::new(
            std::array::from_fn(|i| Arc::new(AtomicU32::new((levels[i] * 10.0) as u32))),
            Arc::new(AtomicU32::new(500)),
        );
        assert!(rms(&take(&mut half, 4000)) < rms(&take(&mut full, 4000)),
            "master gain did not attenuate");
    }

    #[test]
    fn brown_is_much_smoother_than_white() {
        let white = zero_cross_rate(&take(&mut gen(one(0, 100.0)), 5000));
        let brown = zero_cross_rate(&take(&mut gen(one(2, 100.0)), 5000));
        assert!(brown < white * 0.5, "brown zcr {brown} not < half of white {white}");
    }

    #[test]
    fn continuous_layers_make_sound() {
        // Layers 0..=7 have a continuous component, so a short window always
        // has energy. (8 Chimes / 9 Birds are sparse event voices - their
        // synthesis is covered by voice_is_a_decaying_tone below.)
        for k in 0..8 {
            let s = take(&mut gen(one(k, 100.0)), SR as usize / 2);
            assert!(rms(&s) > 1e-4, "layer {k} produced no sound");
        }
    }

    #[test]
    fn voice_is_a_decaying_tone() {
        // The chime/bird/droplet voice: audible on strike, then decays away.
        let mut v = Voice { phase: 0.0, freq: 880.0, sweep: 0.0, amp: 0.5, decay: 0.999 };
        let head: Vec<f32> = (0..200).map(|_| v.tick()).collect();
        assert!(rms(&head) > 0.05, "voice silent on strike");
        for _ in 0..20_000 {
            v.tick();
        }
        assert!(!v.active(), "voice never decayed");
    }
}
