//! Per-scene audio ambience via rodio (the VLC dependency is gone - Ogg and
//! WAV decode natively). Same resolution order as the Python Ambience:
//! real recording (audio/<key>.ogg) -> synth loop (audio/<key>.wav) ->
//! synth simulacrum named by the fallback table.

use std::fs::File;
use std::io::BufReader;
use std::path::PathBuf;

use rodio::{Decoder, OutputStream, OutputStreamHandle, Sink, Source};

use crate::scenes::ambience_fallback;

pub struct Ambience {
    /// Keeps the audio device open; dropping it silences everything.
    _stream: Option<OutputStream>,
    handle: Option<OutputStreamHandle>,
    sink: Option<Sink>,
    audio_dir: PathBuf,
    pub enabled: bool,
    pub volume: f32, // 0..100
    current: Option<String>,
}

impl Ambience {
    pub fn new(audio_dir: PathBuf) -> Ambience {
        let (stream, handle) = match OutputStream::try_default() {
            Ok((s, h)) => (Some(s), Some(h)),
            Err(_) => (None, None), // no audio device: degrade silently
        };
        Ambience {
            _stream: stream,
            handle,
            sink: None,
            audio_dir,
            enabled: true,
            volume: 60.0,
            current: None,
        }
    }

    fn resolve(&self, key: &str) -> Option<PathBuf> {
        for ext in ["ogg", "wav"] {
            let p = self.audio_dir.join(format!("{key}.{ext}"));
            if p.exists() {
                return Some(p);
            }
        }
        let fb = ambience_fallback(key)?;
        let p = self.audio_dir.join(format!("{fb}.wav"));
        p.exists().then_some(p)
    }

    pub fn play(&mut self, key: &str) {
        self.current = Some(key.to_string());
        if !self.enabled {
            return;
        }
        let Some(handle) = &self.handle else { return };
        let Some(path) = self.resolve(key) else { return };
        let Ok(file) = File::open(&path) else { return };
        let Ok(decoder) = Decoder::new(BufReader::new(file)) else {
            return;
        };
        if let Some(old) = self.sink.take() {
            old.stop();
        }
        if let Ok(sink) = Sink::try_new(handle) {
            sink.set_volume(self.volume / 100.0);
            sink.append(decoder.repeat_infinite());
            self.sink = Some(sink);
        }
    }

    pub fn stop(&mut self) {
        if let Some(sink) = self.sink.take() {
            sink.stop();
        }
    }

    pub fn set_volume(&mut self, v: f32) {
        self.volume = v.clamp(0.0, 100.0);
        if let Some(sink) = &self.sink {
            sink.set_volume(self.volume / 100.0);
        }
    }

    pub fn set_enabled(&mut self, on: bool) {
        self.enabled = on;
        if on {
            if let Some(key) = self.current.clone() {
                self.play(&key);
            }
        } else {
            self.stop();
        }
    }
}
