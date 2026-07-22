//! Layered noise generator: several noise "colours" synthesised in real time
//! and mixed into one bed, each with its own level. Independent of the
//! per-scene [`crate::ambience`] so you can run, say, brown + rain under any
//! scene's soundscape.
//!
//! One persistent [`NoiseGen`] source runs on rodio's audio thread and reads a
//! shared atomic level per layer, so the UI knobs retune the mix live without
//! restarting anything. The sink is paused whenever every layer is at zero, so
//! silence costs nothing.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

use rodio::{OutputStream, Sink, Source};

const SR: u32 = 44_100;
pub const N: usize = 5;

#[derive(Clone, Copy)]
pub enum NoiseKind {
    White,
    Pink,
    Brown,
    Surf,
    Rain,
}

pub const KINDS: [NoiseKind; N] = [
    NoiseKind::White,
    NoiseKind::Pink,
    NoiseKind::Brown,
    NoiseKind::Surf,
    NoiseKind::Rain,
];

impl NoiseKind {
    pub fn label(self) -> &'static str {
        match self {
            NoiseKind::White => "White",
            NoiseKind::Pink => "Pink",
            NoiseKind::Brown => "Brown",
            NoiseKind::Surf => "Surf",
            NoiseKind::Rain => "Rain",
        }
    }

    fn idx(self) -> usize {
        match self {
            NoiseKind::White => 0,
            NoiseKind::Pink => 1,
            NoiseKind::Brown => 2,
            NoiseKind::Surf => 3,
            NoiseKind::Rain => 4,
        }
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

struct NoiseGen {
    levels: [Arc<AtomicU32>; N], // milli, 0..1000 == 0..100%
    rng: fastrand::Rng,
    pink: [f32; 7],
    brown: f32,
    surf_pink: [f32; 7],
    surf_phase: f32,
    rain_prev: f32,
}

impl NoiseGen {
    fn lvl(&self, i: usize) -> f32 {
        self.levels[i].load(Ordering::Relaxed) as f32 / 1000.0
    }
}

impl Iterator for NoiseGen {
    type Item = f32;

    fn next(&mut self) -> Option<f32> {
        let w = self.rng.f32() * 2.0 - 1.0; // white, -1..1

        // pink: -3dB/oct
        let p = pink_next(&mut self.pink, w);

        // brown: leaky-integrated white, -6dB/oct, deep rumble
        self.brown = (self.brown + 0.02 * w).clamp(-1.0, 1.0);
        let br = self.brown * 3.5;

        // surf: an independent pink layer swelling in and out like waves
        let sp = pink_next(&mut self.surf_pink, w);
        self.surf_phase += std::f32::consts::TAU * 0.09 / SR as f32;
        if self.surf_phase > std::f32::consts::TAU {
            self.surf_phase -= std::f32::consts::TAU;
        }
        let env = 0.5 + 0.5 * self.surf_phase.sin();
        let surf = sp * env * 1.3;

        // rain: high-passed white -> bright, airy hiss
        let rain = (w - self.rain_prev) * 0.5;
        self.rain_prev = w;

        let mix = self.lvl(0) * w * 0.35
            + self.lvl(1) * p
            + self.lvl(2) * br
            + self.lvl(3) * surf
            + self.lvl(4) * rain;

        Some((mix * 0.9).clamp(-1.0, 1.0))
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
    playing: bool,
}

impl Noise {
    pub fn new() -> Noise {
        let levels: [Arc<AtomicU32>; N] = std::array::from_fn(|_| Arc::new(AtomicU32::new(0)));
        let (stream, sink) = match OutputStream::try_default() {
            Ok((stream, handle)) => {
                let sink = Sink::try_new(&handle).ok();
                if let Some(sink) = &sink {
                    sink.append(NoiseGen {
                        levels: levels.clone(),
                        rng: fastrand::Rng::new(),
                        pink: [0.0; 7],
                        brown: 0.0,
                        surf_pink: [0.0; 7],
                        surf_phase: 0.0,
                        rain_prev: 0.0,
                    });
                    sink.pause(); // silent until a layer is raised
                }
                (Some(stream), sink)
            }
            Err(_) => (None, None), // no audio device: degrade silently
        };
        Noise {
            _stream: stream,
            sink,
            levels,
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

    /// True when any layer is audible (used to highlight the header).
    pub fn active(&self) -> bool {
        self.levels.iter().any(|l| l.load(Ordering::Relaxed) > 0)
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
        NoiseGen {
            levels: std::array::from_fn(|i| {
                Arc::new(AtomicU32::new((levels_pct[i] * 10.0) as u32))
            }),
            rng: fastrand::Rng::with_seed(1),
            pink: [0.0; 7],
            brown: 0.0,
            surf_pink: [0.0; 7],
            surf_phase: 0.0,
            rain_prev: 0.0,
        }
    }

    fn take(g: &mut NoiseGen, n: usize) -> Vec<f32> {
        (0..n).map(|_| g.next().unwrap()).collect()
    }

    /// Fraction of adjacent samples that cross zero: high for bright noise,
    /// low for noise whose energy sits at low frequencies.
    fn zero_cross_rate(s: &[f32]) -> f32 {
        let crossings = s.windows(2).filter(|w| (w[0] <= 0.0) != (w[1] <= 0.0)).count();
        crossings as f32 / s.len() as f32
    }

    #[test]
    fn output_is_bounded_and_audible() {
        let s = take(&mut gen([100.0, 0.0, 0.0, 0.0, 0.0]), 2000);
        assert!(s.iter().all(|x| x.abs() <= 1.0), "clipped past [-1,1]");
        let rms = (s.iter().map(|x| x * x).sum::<f32>() / s.len() as f32).sqrt();
        assert!(rms > 0.01, "white noise too quiet: rms={rms}");
    }

    #[test]
    fn all_layers_zero_is_pure_silence() {
        let s = take(&mut gen([0.0; N]), 500);
        assert!(s.iter().all(|x| *x == 0.0), "expected silence");
    }

    #[test]
    fn brown_is_much_smoother_than_white() {
        // The spectral shaping should be real: brown (leaky-integrated) has its
        // energy at low frequencies, so far fewer zero crossings than white.
        let white = zero_cross_rate(&take(&mut gen([100.0, 0.0, 0.0, 0.0, 0.0]), 5000));
        let brown = zero_cross_rate(&take(&mut gen([0.0, 0.0, 100.0, 0.0, 0.0]), 5000));
        assert!(brown < white * 0.5, "brown zcr {brown} not < half of white {white}");
    }

    #[test]
    fn layers_mix_additively() {
        // Two layers together should be louder than either alone.
        let one = take(&mut gen([60.0, 0.0, 0.0, 0.0, 0.0]), 4000);
        let two = take(&mut gen([60.0, 0.0, 60.0, 0.0, 0.0]), 4000);
        let rms = |s: &[f32]| (s.iter().map(|x| x * x).sum::<f32>() / s.len() as f32).sqrt();
        assert!(rms(&two) > rms(&one), "mixing a layer did not add energy");
    }
}
