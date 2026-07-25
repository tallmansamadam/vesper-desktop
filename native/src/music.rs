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
    stream: Option<cpal::Stream>,
}

impl Music {
    pub fn new() -> Music {
        Music {
            level: Arc::new(AtomicU32::new(0)),
            stream: None,
        }
    }

    /// A handle the effect engine reads each frame (0..1000 == silence..loud).
    pub fn level_handle(&self) -> Arc<AtomicU32> {
        self.level.clone()
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

        // Fast attack, slow release so the lights punch on transients but ease
        // back down smoothly. `env` lives inside the (FnMut) callback.
        let mut env = 0.0f32;
        let analyse = move |buf: &[f32]| {
            if buf.is_empty() {
                return;
            }
            let sum: f32 = buf.iter().map(|s| s * s).sum();
            let rms = (sum / buf.len() as f32).sqrt();
            let target = (rms * 3.5).min(1.0);
            let a = if target > env { 0.5 } else { 0.04 };
            env += (target - env) * a;
            level.store((env * 1000.0) as u32, Ordering::Relaxed);
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
