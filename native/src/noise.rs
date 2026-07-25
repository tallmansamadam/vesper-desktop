//! Layered noise / ambience generator: a mixable bed of textures, scaled by
//! the master Volume, independent of the per-scene [`crate::ambience`] so it
//! layers under any scene's soundscape.
//!
//! Two kinds of layer, because synthesis is right for some sounds and wrong
//! for others:
//!
//! * **Synthesised** - the noise *colours* (white/pink/brown) and broadband
//!   textures (rain, wind, surf, waterfall). These are literally filtered
//!   noise, so DSP beats a loop: no seams, no repetition, tiny footprint.
//!   One persistent [`NoiseGen`] source reads a shared atomic per layer, so
//!   the knobs retune the mix live without restarting anything.
//! * **Recorded** - anything that has to sound like a real *thing* (birds,
//!   wind chimes, running water, cicadas, a café). Synthesised versions of
//!   these read as obviously fake, so these layers play the real recordings
//!   in `audio/`, each looping in its own sink with its own gain.
//!
//! Everything is paused whenever it would be silent, so an unused layer -
//! and an unused panel - costs nothing.

use std::collections::HashMap;
use std::fs::File;
use std::io::BufReader;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

use rodio::{Decoder, OutputStream, OutputStreamHandle, Sink, Source};

const SR: u32 = 44_100;
const SRF: f32 = 44_100.0;
const TAU: f32 = std::f32::consts::TAU;
pub const N: usize = 14;

#[derive(Clone, Copy, PartialEq, Eq)]
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
    Cicadas,
    Cafe,
    City,
    Shore,
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
    NoiseKind::Cicadas,
    NoiseKind::Cafe,
    NoiseKind::City,
    NoiseKind::Shore,
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
            NoiseKind::Cicadas => "Cicadas",
            NoiseKind::Cafe => "Café",
            NoiseKind::City => "City",
            NoiseKind::Shore => "Shore",
        }
    }

    fn idx(self) -> usize {
        self as usize
    }

    /// The recording that backs this layer, if it's a recorded one. These are
    /// the sounds a synth can't fake convincingly - real birdsong, real wind
    /// chimes, real running water.
    pub fn sample_name(self) -> Option<&'static str> {
        self.sample()
    }

    fn sample(self) -> Option<&'static str> {
        match self {
            NoiseKind::Fountain => Some("stream"),
            NoiseKind::Chimes => Some("chimes"),
            NoiseKind::Birds => Some("birds"),
            NoiseKind::Cicadas => Some("cicadas"),
            NoiseKind::Cafe => Some("cafe"),
            NoiseKind::City => Some("city"),
            NoiseKind::Shore => Some("seagulls"),
            _ => None,
        }
    }

    /// Per-layer gain trim, so every layer sits at a comparable loudness at
    /// the same knob position.
    fn trim(self) -> f32 {
        match self {
            NoiseKind::Waterfall => 1.0,
            NoiseKind::Chimes => 0.85,
            NoiseKind::Birds => 1.0, // real birdsong is sparse; give it headroom
            _ => 0.9,
        }
    }
}

/// A named mix of layers - the noise panel's equivalent of a light scene.
/// Only the layers a place actually has are listed; the rest go to zero.
pub struct NoiseScene {
    pub name: &'static str,
    pub mix: &'static [(NoiseKind, f32)],
}

use NoiseKind as K;

pub const SCENES: &[NoiseScene] = &[
    NoiseScene { name: "Beach", mix: &[(K::Surf, 72.0), (K::Shore, 30.0), (K::Wind, 22.0)] },
    NoiseScene { name: "Rainstorm", mix: &[(K::Rain, 68.0), (K::Brown, 38.0), (K::Wind, 42.0)] },
    NoiseScene { name: "Forest", mix: &[(K::Birds, 55.0), (K::Wind, 30.0), (K::Pink, 16.0), (K::Cicadas, 12.0)] },
    NoiseScene { name: "Mountain", mix: &[(K::Wind, 72.0), (K::Brown, 32.0), (K::Waterfall, 18.0)] },
    NoiseScene { name: "Temple", mix: &[(K::Chimes, 46.0), (K::Brown, 30.0), (K::Wind, 20.0), (K::Fountain, 16.0)] },
    // Dry by default - rain belongs to the two moods below, not baked into
    // every urban room.
    NoiseScene { name: "City", mix: &[(K::City, 66.0), (K::Cafe, 16.0), (K::Wind, 12.0)] },
    NoiseScene { name: "Café", mix: &[(K::Cafe, 72.0), (K::City, 16.0), (K::Pink, 10.0)] },
    // Wet streets under streetlamps: rain over traffic, with a low rumble
    // doing the work of distance and weather.
    NoiseScene { name: "Sodium Rain", mix: &[(K::Rain, 58.0), (K::City, 44.0), (K::Brown, 28.0), (K::Wind, 18.0)] },
    // Inside, warm and dry, watching it come down on the glass.
    NoiseScene { name: "Window Seat", mix: &[(K::Cafe, 58.0), (K::Rain, 46.0), (K::Brown, 18.0), (K::City, 12.0)] },
    NoiseScene { name: "Waterfall", mix: &[(K::Waterfall, 62.0), (K::Fountain, 28.0), (K::Birds, 14.0)] },
    NoiseScene { name: "Zen Garden", mix: &[(K::Fountain, 64.0), (K::Chimes, 36.0), (K::Birds, 18.0)] },
    NoiseScene { name: "Summer Night", mix: &[(K::Cicadas, 52.0), (K::Brown, 22.0), (K::Wind, 14.0)] },
    NoiseScene { name: "Deep Focus", mix: &[(K::Pink, 56.0), (K::Brown, 26.0), (K::Rain, 20.0)] },
    NoiseScene { name: "Open Sea", mix: &[(K::Surf, 58.0), (K::Brown, 32.0), (K::Wind, 26.0)] },
];

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
    levels: [Arc<AtomicU32>; N],
    master: Arc<AtomicU32>,
    rng: fastrand::Rng,
    pink: [f32; 7],
    brown: f32,
    surf_pink: [f32; 7],
    surf_phase: f32,
    rain_prev: f32,
    wind_pink: [f32; 7],
    wind_lp: f32,
    wind_cut_phase: f32,
    wind_gust_phase: f32,
    // waterfall: a full-bodied rushing bed - broadband hiss over a churning
    // low rumble, with a slow "body" wander so it doesn't sit dead still
    wf_pink: [f32; 7],
    wf_low: f32,
    wf_phase: f32,
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
            wf_pink: [0.0; 7],
            wf_low: 0.0,
            wf_phase: 0.0,
        }
    }

    fn lvl(&self, i: usize) -> f32 {
        self.levels[i].load(Ordering::Relaxed) as f32 / 1000.0
    }

    fn white(&mut self) -> f32 {
        self.rng.f32() * 2.0 - 1.0
    }
}

impl Iterator for NoiseGen {
    type Item = f32;

    fn next(&mut self) -> Option<f32> {
        let master = self.master.load(Ordering::Relaxed) as f32 / 1000.0;
        let w = self.white();
        let mut mix = 0.0;

        mix += self.lvl(0) * w * 0.35;

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

        // Waterfall: bright broadband spray over a churning low body. Gained
        // to sit at a comparable loudness to the other layers.
        let lwf = self.lvl(6);
        if lwf > 0.0 {
            let w2 = self.white();
            let wp = pink_next(&mut self.wf_pink, w2);
            self.wf_low += 0.010 * (w2 - self.wf_low); // deep churn
            self.wf_phase = (self.wf_phase + TAU * 0.13 / SRF) % TAU;
            let body = 0.8 + 0.2 * self.wf_phase.sin();
            let spray = 0.5 * w2 + 1.5 * wp;
            mix += lwf * (spray + self.wf_low * 6.0) * body * 0.9;
        }

        // (Recorded layers - Fountain/Chimes/Birds/Cicadas/Cafe/City/Shore -
        // play in their own sinks; they contribute nothing here.)

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
    handle: Option<OutputStreamHandle>,
    synth: Option<Sink>,
    /// kind index -> looping sink for the recorded layers (made on first use).
    samples: HashMap<usize, Sink>,
    audio_dir: PathBuf,
    levels: [Arc<AtomicU32>; N],
    master: Arc<AtomicU32>,
    synth_playing: bool,
}

impl Noise {
    pub fn new(audio_dir: PathBuf) -> Noise {
        let levels: [Arc<AtomicU32>; N] = std::array::from_fn(|_| Arc::new(AtomicU32::new(0)));
        let master = Arc::new(AtomicU32::new(600)); // tracks the Volume knob
        let (stream, handle, synth) = match OutputStream::try_default() {
            Ok((stream, handle)) => {
                let sink = Sink::try_new(&handle).ok();
                if let Some(sink) = &sink {
                    sink.append(NoiseGen::new(levels.clone(), master.clone()));
                    sink.pause();
                }
                (Some(stream), Some(handle), sink)
            }
            Err(_) => (None, None, None), // no audio device: degrade silently
        };
        Noise {
            _stream: stream,
            handle,
            synth,
            samples: HashMap::new(),
            audio_dir,
            levels,
            master,
            synth_playing: false,
        }
    }

    pub fn level_pct(&self, k: NoiseKind) -> f32 {
        self.levels[k.idx()].load(Ordering::Relaxed) as f32 / 10.0
    }

    fn master_pct(&self) -> f32 {
        self.master.load(Ordering::Relaxed) as f32 / 1000.0
    }

    pub fn set_level(&mut self, k: NoiseKind, pct: f32) {
        self.levels[k.idx()].store((pct.clamp(0.0, 100.0) * 10.0) as u32, Ordering::Relaxed);
        if k.sample().is_some() {
            self.update_sample(k);
        }
        self.update_synth();
    }

    /// Master gain, driven by the Volume knob (0..100).
    pub fn set_master(&mut self, pct: f32) {
        self.master
            .store((pct.clamp(0.0, 100.0) * 10.0) as u32, Ordering::Relaxed);
        for k in KINDS {
            if k.sample().is_some() {
                self.update_sample(k);
            }
        }
        self.update_synth();
    }

    /// True when any layer is up and the master is open.
    pub fn active(&self) -> bool {
        self.master.load(Ordering::Relaxed) > 0
            && self.levels.iter().any(|l| l.load(Ordering::Relaxed) > 0)
    }

    /// Start/refresh a recorded layer's looping sink to match its level.
    fn update_sample(&mut self, k: NoiseKind) {
        let vol = self.level_pct(k) / 100.0 * self.master_pct() * k.trim();
        let idx = k.idx();
        if vol <= 0.0 {
            if let Some(s) = self.samples.get(&idx) {
                s.pause();
            }
            return;
        }
        if !self.samples.contains_key(&idx) {
            let (Some(handle), Some(name)) = (&self.handle, k.sample()) else {
                return;
            };
            // Prefer the real recording; fall back to a synthesised loop of
            // the same name if the .ogg isn't present.
            let sink = ["ogg", "wav"]
                .iter()
                .map(|ext| self.audio_dir.join(format!("{name}.{ext}")))
                .find(|p| p.exists())
                .and_then(|p| File::open(p).ok())
                .and_then(|f| Decoder::new(BufReader::new(f)).ok())
                .and_then(|d| {
                    let sink = Sink::try_new(handle).ok()?;
                    sink.append(d.repeat_infinite());
                    Some(sink)
                });
            match sink {
                Some(s) => {
                    self.samples.insert(idx, s);
                }
                None => return,
            }
        }
        if let Some(s) = self.samples.get(&idx) {
            s.set_volume(vol);
            s.play();
        }
    }

    fn update_synth(&mut self) {
        // The synth sink only needs to run when a synthesised layer is up.
        let synth_up = self.master.load(Ordering::Relaxed) > 0
            && KINDS
                .iter()
                .filter(|k| k.sample().is_none())
                .any(|k| self.levels[k.idx()].load(Ordering::Relaxed) > 0);
        if let Some(sink) = &self.synth {
            match (synth_up, self.synth_playing) {
                (true, false) => {
                    sink.play();
                    self.synth_playing = true;
                }
                (false, true) => {
                    sink.pause();
                    self.synth_playing = false;
                }
                _ => {}
            }
        }
    }

    /// Set every layer to a scene's mix (anything it doesn't name goes off).
    pub fn apply_scene(&mut self, sc: &NoiseScene) {
        for k in KINDS {
            let level = sc
                .mix
                .iter()
                .find(|(kind, _)| *kind == k)
                .map(|(_, v)| *v)
                .unwrap_or(0.0);
            self.set_level(k, level);
        }
    }

    pub fn clear(&mut self) {
        for k in KINDS {
            self.set_level(k, 0.0);
        }
    }

    pub fn stop(&mut self) {
        if let Some(sink) = &self.synth {
            sink.pause();
        }
        for s in self.samples.values() {
            s.pause();
        }
        self.synth_playing = false;
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

    /// Indices of the layers the synth actually renders.
    fn synth_indices() -> Vec<usize> {
        KINDS
            .iter()
            .enumerate()
            .filter(|(_, k)| k.sample().is_none())
            .map(|(i, _)| i)
            .collect()
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
        assert!(
            rms(&take(&mut half, 4000)) < rms(&take(&mut full, 4000)),
            "master gain did not attenuate"
        );
    }

    #[test]
    fn brown_is_much_smoother_than_white() {
        let white = zero_cross_rate(&take(&mut gen(one(0, 100.0)), 5000));
        let brown = zero_cross_rate(&take(&mut gen(one(2, 100.0)), 5000));
        assert!(brown < white * 0.5, "brown zcr {brown} not < half of white {white}");
    }

    #[test]
    fn every_synth_layer_makes_sound() {
        for i in synth_indices() {
            let s = take(&mut gen(one(i, 100.0)), SR as usize / 2);
            assert!(rms(&s) > 1e-4, "synth layer {i} produced no sound");
        }
    }

    #[test]
    fn waterfall_is_clearly_audible() {
        // It was previously so quiet as to be inaudible next to the others;
        // hold it to at least white noise's loudness at the same setting.
        let white = rms(&take(&mut gen(one(0, 60.0)), 20_000));
        let wfall = rms(&take(&mut gen(one(6, 60.0)), 20_000));
        assert!(wfall >= white, "waterfall {wfall} quieter than white {white}");
    }

    #[test]
    fn recorded_layers_are_not_synthesised() {
        // The realism-critical layers must be backed by real recordings, and
        // must contribute nothing to the synth mix.
        for k in [NoiseKind::Birds, NoiseKind::Chimes, NoiseKind::Fountain] {
            assert!(k.sample().is_some(), "{} should be recorded", k.label());
            let s = take(&mut gen(one(k.idx(), 100.0)), 4000);
            assert!(s.iter().all(|x| *x == 0.0), "{} leaked into synth", k.label());
        }
    }
}
