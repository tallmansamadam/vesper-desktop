//! The vendor-neutral device model. A `Light` is pure data + last-known
//! state; every command is dispatched through the `Controller` to the
//! provider matching `light.vendor` (the Rust analogue of the Python
//! `light.provider` delegation).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Vendor {
    Govee,
    Hue,
    Lifx,
    Nanoleaf,
    Wiz,
    Yeelight,
}

impl Vendor {
    pub fn name(self) -> &'static str {
        match self {
            Vendor::Govee => "Govee",
            Vendor::Hue => "Hue",
            Vendor::Lifx => "LIFX",
            Vendor::Nanoleaf => "Nanoleaf",
            Vendor::Wiz => "WiZ",
            Vendor::Yeelight => "Yeelight",
        }
    }
}

pub struct Light {
    pub vendor: Vendor,
    /// Stable unique key ("d073d5xxxxxx", "wiz:<mac>", "hue:<rid>", ...).
    pub id: String,
    pub ip: String,
    pub port: u16,
    /// LIFX 64-bit serial; 0 for other vendors.
    pub target: u64,
    label: Mutex<String>,
    power: AtomicBool,
    hsbk: Mutex<[u16; 4]>,
}

impl Light {
    pub fn new(
        vendor: Vendor,
        id: String,
        ip: String,
        port: u16,
        target: u64,
        label: String,
    ) -> Arc<Light> {
        Arc::new(Light {
            vendor,
            id,
            ip,
            port,
            target,
            label: Mutex::new(label),
            power: AtomicBool::new(false),
            hsbk: Mutex::new([0, 0, 0, 3500]),
        })
    }

    pub fn label(&self) -> String {
        self.label.lock().unwrap().clone()
    }

    pub fn set_label(&self, s: &str) {
        if !s.is_empty() {
            *self.label.lock().unwrap() = s.to_string();
        }
    }

    pub fn power(&self) -> bool {
        self.power.load(Ordering::Relaxed)
    }

    pub fn set_power_state(&self, on: bool) {
        self.power.store(on, Ordering::Relaxed);
    }

    pub fn hsbk(&self) -> [u16; 4] {
        *self.hsbk.lock().unwrap()
    }

    pub fn set_hsbk(&self, v: [u16; 4]) {
        *self.hsbk.lock().unwrap() = v;
    }

    pub fn brightness_pct(&self) -> f32 {
        self.hsbk()[2] as f32 / 65535.0 * 100.0
    }
}
