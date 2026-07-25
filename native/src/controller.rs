//! Multi-vendor controller: owns every provider, discovers across all of
//! them in parallel, and dispatches each command to the provider matching
//! the light's vendor tag (the Rust analogue of devices.py).

use std::path::Path;
use std::sync::Arc;

use crate::govee::Govee;
use crate::hue::Hue;
use crate::lifx::LifxLan;
use crate::light::{Light, Vendor};
use crate::nanoleaf::Nanoleaf;
use crate::wiz::Wiz;
use crate::yeelight::Yeelight;

pub struct Controller {
    pub lifx: LifxLan,
    pub wiz: Wiz,
    pub yeelight: Yeelight,
    pub govee: Govee,
    pub nanoleaf: Nanoleaf,
    pub hue: Hue,
}

impl Controller {
    pub fn new(root: &Path) -> Controller {
        Controller {
            lifx: LifxLan::new().expect("udp socket"),
            wiz: Wiz::new().expect("udp socket"),
            yeelight: Yeelight::new(),
            govee: Govee::new(),
            nanoleaf: Nanoleaf::from_settings(root),
            hue: Hue::from_settings(root),
        }
    }

    /// Discover across every provider in parallel; wall-clock cost is the
    /// slowest single provider, not the sum.
    pub fn discover(&self) -> Vec<Arc<Light>> {
        let mut all: Vec<Arc<Light>> = Vec::new();
        std::thread::scope(|s| {
            let h1 = s.spawn(|| self.lifx.discover());
            let h2 = s.spawn(|| self.wiz.discover());
            let h3 = s.spawn(|| self.yeelight.discover());
            let h4 = s.spawn(|| self.govee.discover());
            let h5 = s.spawn(|| self.nanoleaf.discover());
            let h6 = s.spawn(|| self.hue.discover());
            for h in [h1, h2, h3, h4, h5, h6] {
                if let Ok(mut v) = h.join() {
                    all.append(&mut v);
                }
            }
        });
        all.sort_by(|a, b| {
            (a.vendor.name(), a.label().to_lowercase())
                .cmp(&(b.vendor.name(), b.label().to_lowercase()))
        });
        all
    }

    pub fn set_power(&self, l: &Light, on: bool, d: u32) {
        match l.vendor {
            Vendor::Lifx => self.lifx.set_power(l, on, d),
            Vendor::Wiz => self.wiz.set_power(l, on, d),
            Vendor::Yeelight => self.yeelight.set_power(l, on, d),
            Vendor::Govee => self.govee.set_power(l, on, d),
            Vendor::Nanoleaf => self.nanoleaf.set_power(l, on, d),
            Vendor::Hue => self.hue.set_power(l, on, d),
        }
    }

    pub fn set_color(&self, l: &Light, h: u16, s: u16, b: u16, k: u16, d: u32) {
        match l.vendor {
            Vendor::Lifx => self.lifx.set_color(l, h, s, b, k, d),
            Vendor::Wiz => self.wiz.set_color(l, h, s, b, k, d),
            Vendor::Yeelight => self.yeelight.set_color(l, h, s, b, k, d),
            Vendor::Govee => self.govee.set_color(l, h, s, b, k, d),
            Vendor::Nanoleaf => self.nanoleaf.set_color(l, h, s, b, k, d),
            Vendor::Hue => self.hue.set_color(l, h, s, b, k, d),
        }
    }

    pub fn set_brightness(&self, l: &Light, pct: f32, d: u32) {
        match l.vendor {
            Vendor::Lifx => self.lifx.set_brightness(l, pct, d),
            Vendor::Wiz => self.wiz.set_brightness(l, pct, d),
            Vendor::Yeelight => self.yeelight.set_brightness(l, pct, d),
            Vendor::Govee => self.govee.set_brightness(l, pct, d),
            Vendor::Nanoleaf => self.nanoleaf.set_brightness(l, pct, d),
            Vendor::Hue => self.hue.set_brightness(l, pct, d),
        }
    }

    pub fn set_white(&self, l: &Light, pct: f32, kelvin: u16, d: u32) {
        match l.vendor {
            Vendor::Lifx => self.lifx.set_white(l, pct, kelvin, d),
            Vendor::Wiz => self.wiz.set_white(l, pct, kelvin, d),
            Vendor::Yeelight => self.yeelight.set_white(l, pct, kelvin, d),
            Vendor::Govee => self.govee.set_white(l, pct, kelvin, d),
            Vendor::Nanoleaf => self.nanoleaf.set_white(l, pct, kelvin, d),
            Vendor::Hue => self.hue.set_white(l, pct, kelvin, d),
        }
    }

    pub fn stream_color(&self, l: &Light, h: u16, s: u16, b: u16, k: u16, d: u32) {
        match l.vendor {
            Vendor::Lifx => self.lifx.stream_color(l, h, s, b, k, d),
            Vendor::Wiz => self.wiz.stream_color(l, h, s, b, k, d),
            Vendor::Yeelight => self.yeelight.stream_color(l, h, s, b, k, d),
            Vendor::Govee => self.govee.stream_color(l, h, s, b, k, d),
            Vendor::Nanoleaf => self.nanoleaf.stream_color(l, h, s, b, k, d),
            Vendor::Hue => self.hue.stream_color(l, h, s, b, k, d),
        }
    }

    pub fn refresh(&self, l: &Light) {
        match l.vendor {
            Vendor::Lifx => self.lifx.refresh(l),
            Vendor::Wiz => self.wiz.refresh(l),
            Vendor::Yeelight => self.yeelight.refresh(l),
            Vendor::Govee => self.govee.refresh(l),
            Vendor::Nanoleaf => self.nanoleaf.refresh(l),
            Vendor::Hue => self.hue.refresh(l),
        }
    }

    /// Add a light by address, for when broadcast discovery can't reach it
    /// (another subnet, AP isolation, a VPN in the way). Asks each bridge-free
    /// brand in turn and returns whichever answers - so the user types an IP
    /// and doesn't have to know the make.
    ///
    /// Hue and Nanoleaf are absent by design: they need pairing, not an
    /// address, which is what the "+ Hue" / "+ Nanoleaf" buttons do.
    pub fn add_by_ip(&self, ip: &str) -> Option<Arc<Light>> {
        if let Some(target) = self.lifx.probe_target(ip) {
            let l = self.lifx.light_for(ip, target);
            self.lifx.refresh(&l);
            return Some(l);
        }
        if let Some(l) = self.wiz.probe(ip) {
            return Some(l);
        }
        if let Some(l) = self.yeelight.probe(ip) {
            return Some(l);
        }
        self.govee.probe(ip)
    }

    pub fn set_rgb(&self, l: &Light, r: u8, g: u8, b: u8, d: u32) {
        let hsbk = crate::color::rgb_to_hsbk(r, g, b, 3500);
        let cur = l.hsbk();
        // Keep the current brightness, as the Python set_rgb did.
        self.set_color(l, hsbk[0], hsbk[1], cur[2], cur[3], d);
    }
}
