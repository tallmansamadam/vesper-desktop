//! Music reactivity: capture the PC's own audio output (WASAPI loopback, via
//! cpal building an input stream on the default *output* device) and expose a
//! smoothed loudness envelope for the Music light effect to follow.
//!
//! The cpal stream runs its callback on the audio thread and writes the
//! envelope into a shared atomic; the effect reads it. Everything degrades
//! gracefully: if loopback can't be opened the level just stays at zero.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

pub struct Music {
    level: Arc<AtomicU32>, // milli 0..1000, smoothed loudness envelope
    tone: Arc<AtomicU32>,  // milli 0..1000, spectral balance: bass .. treble
    stream: Option<cpal::Stream>,
}

impl Music {
    pub fn new() -> Music {
        Music {
            level: Arc::new(AtomicU32::new(0)),
            tone: Arc::new(AtomicU32::new(500)),
            stream: None,
        }
    }

    /// A handle the effect engine reads each frame (0..1000 == silence..loud).
    pub fn level_handle(&self) -> Arc<AtomicU32> {
        self.level.clone()
    }

    /// Spectral balance handle (0 = all bass, 1000 = all treble), which the
    /// Music effect swings the hue with.
    pub fn tone_handle(&self) -> Arc<AtomicU32> {
        self.tone.clone()
    }

    pub fn active(&self) -> bool {
        self.stream.is_some()
    }

    /// Begin loopback capture. Returns false if no output device / loopback
    /// isn't available, in which case the caller should report it.
    pub fn start(&mut self) -> bool {
        if self.stream.is_some() {
            return true;
        }
        let host = cpal::default_host();
        let Some(device) = host.default_output_device() else {
            return false;
        };
        let Ok(supported) = device.default_output_config() else {
            return false;
        };
        let fmt = supported.sample_format();
        let cfg: cpal::StreamConfig = supported.into();
        let level = self.level.clone();
        let tone = self.tone.clone();
        let rate = cfg.sample_rate.0 as f32 * cfg.channels as f32;

        // Fast attack, slow release so the lights punch on transients but ease
        // back down smoothly. Alongside the loudness we track a crude spectral
        // balance: a one-pole low-pass splits each buffer into "bass" (the
        // filtered signal) and "treble" (what the filter removed); their
        // energy ratio swings the hue. State lives in the FnMut callback.
        let mut env = 0.0f32;
        let mut lp = 0.0f32;
        let mut tone_env = 0.5f32;
        let analyse = move |buf: &[f32]| {
            if buf.is_empty() {
                return;
            }
            let mut sum = 0.0f32;
            let mut low = 0.0f32;
            let mut high = 0.0f32;
            for &s in buf {
                sum += s * s;
                lp += 0.06 * (s - lp); // ~450 Hz corner at 44.1k
                low += lp * lp;
                let h = s - lp;
                high += h * h;
            }
            let n = buf.len() as f32;
            let rms = (sum / n).sqrt();
            let target = (rms * 2.6).min(1.0);
            // Smooth in *seconds*, not per callback: cpal hands out buffers of
            // wildly different sizes (here ~100ms), so a fixed per-buffer
            // coefficient made the release ten times slower than intended and
            // the lights lag the music badly.
            let dt = (n / rate).clamp(0.001, 0.5);
            let coeff = |tau: f32| 1.0 - (-dt / tau).exp();
            let a = if target > env { coeff(0.02) } else { coeff(0.25) };
            env += (target - env) * a;
            level.store((env * 1000.0) as u32, Ordering::Relaxed);

            // Only re-aim the tone when there's signal, so silence doesn't
            // drag the hue somewhere arbitrary.
            if rms > 0.002 {
                let t = (high / (low + high + 1e-9)).clamp(0.0, 1.0);
                // treble is weaker by nature; stretch it around the midpoint
                let t = ((t - 0.15) * 2.2 + 0.5).clamp(0.0, 1.0);
                tone_env += (t - tone_env) * coeff(0.45);
                tone.store((tone_env * 1000.0) as u32, Ordering::Relaxed);
            }
        };

        let err = |e| eprintln!("music: stream error: {e}");
        let stream = match fmt {
            cpal::SampleFormat::F32 => {
                let mut a = analyse;
                device.build_input_stream(&cfg, move |d: &[f32], _| a(d), err, None)
            }
            cpal::SampleFormat::I16 => {
                let mut a = analyse;
                device.build_input_stream(
                    &cfg,
                    move |d: &[i16], _| {
                        let f: Vec<f32> = d.iter().map(|s| *s as f32 / 32768.0).collect();
                        a(&f);
                    },
                    err,
                    None,
                )
            }
            _ => return false,
        };

        match stream {
            Ok(s) => {
                let _ = s.play();
                self.stream = Some(s);
                true
            }
            Err(_) => false,
        }
    }

    pub fn stop(&mut self) {
        self.stream = None; // dropping the stream stops capture
        self.level.store(0, Ordering::Relaxed);
    }
}
