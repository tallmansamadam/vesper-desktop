//! System-tray icon and global hotkeys.
//!
//! Both crates deliver their events through process-global crossbeam channels,
//! so a small background thread polls them. Crucially it *acts on them
//! itself* - driving the lights, restoring the window, quitting - rather than
//! posting work into the egui frame loop: while the window is hidden in the
//! tray it may get no repaint cycles at all, so anything that waited for a
//! frame would appear dead until you reopened the window. The UI is only
//! notified afterwards, to refresh its readouts.
//!
//! Hotkeys: Ctrl+Alt+O = all off, Ctrl+Alt+L = toggle all lights.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use global_hotkey::{
    hotkey::{Code, HotKey, Modifiers},
    GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState,
};
use tray_icon::{
    menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem},
    TrayIcon, TrayIconBuilder,
};

use crate::controller::Controller;
use crate::light::Light;
use crate::Msg;

/// Holds the tray icon and hotkey manager alive for the app's lifetime.
pub struct System {
    _tray: Option<TrayIcon>,
    _hotkeys: Option<GlobalHotKeyManager>,
}

/// What the tray thread needs to do its work without the UI.
pub struct Shared {
    pub ctrl: Arc<Controller>,
    pub lights: Arc<Mutex<Vec<Arc<Light>>>>,
    pub quit: Arc<AtomicBool>,
}

fn load_icon(root: &Path) -> Option<tray_icon::Icon> {
    let img = image::open(root.join("branding").join("vesper_icon_256.png")).ok()?;
    let rgba = img.to_rgba8();
    let (w, h) = (rgba.width(), rgba.height());
    tray_icon::Icon::from_rgba(rgba.into_raw(), w, h).ok()
}

/// Drive every light to `on` (or flip them) off the UI thread.
fn power(shared: &Shared, tx: &Sender<Msg>, ctx: &egui::Context, target: Option<bool>) {
    let lights = shared.lights.lock().unwrap().clone();
    if lights.is_empty() {
        let _ = tx.send(Msg::Status("No lights found yet.".into(), true));
        ctx.request_repaint();
        return;
    }
    let on = target.unwrap_or_else(|| !lights.iter().all(|l| l.power()));
    let ctrl = shared.ctrl.clone();
    let tx = tx.clone();
    let ctx = ctx.clone();
    std::thread::spawn(move || {
        for l in &lights {
            ctrl.set_power(l, on, 400);
        }
        let _ = tx.send(Msg::Status(
            format!("All lights {}.", if on { "on" } else { "off" }),
            false,
        ));
        let _ = tx.send(Msg::Refresh);
        ctx.request_repaint();
    });
}

pub fn init(root: &Path, tx: Sender<Msg>, ctx: egui::Context, shared: Shared) -> System {
    // ---- tray menu ----
    let show = MenuItem::new("Show Vesper", true, None);
    let allon = MenuItem::new("All On", true, None);
    let alloff = MenuItem::new("All Off", true, None);
    let quit = MenuItem::new("Quit", true, None);
    let menu = Menu::new();
    let _ = menu.append_items(&[
        &show,
        &PredefinedMenuItem::separator(),
        &allon,
        &alloff,
        &PredefinedMenuItem::separator(),
        &quit,
    ]);
    let (id_show, id_allon, id_alloff, id_quit) = (
        show.id().clone(),
        allon.id().clone(),
        alloff.id().clone(),
        quit.id().clone(),
    );

    let mut builder = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_tooltip("Vesper")
        .with_menu_on_left_click(false);
    if let Some(icon) = load_icon(root) {
        builder = builder.with_icon(icon);
    }
    let tray = builder.build().ok();

    // ---- global hotkeys ----
    let (hotkeys, hk_alloff, hk_toggle) = match GlobalHotKeyManager::new() {
        Ok(m) => {
            let off = HotKey::new(Some(Modifiers::CONTROL | Modifiers::ALT), Code::KeyO);
            let toggle = HotKey::new(Some(Modifiers::CONTROL | Modifiers::ALT), Code::KeyL);
            let _ = m.register(off);
            let _ = m.register(toggle);
            (Some(m), off.id(), toggle.id())
        }
        Err(_) => (None, 0, 0),
    };

    // ---- poll thread: act on tray/menu/hotkey events immediately ----
    let menu_rx = MenuEvent::receiver().clone();
    let hk_rx = GlobalHotKeyEvent::receiver().clone();
    let tray_rx = tray_icon::TrayIconEvent::receiver().clone();
    std::thread::spawn(move || {
        let restore = |ctx: &egui::Context| {
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
            ctx.request_repaint();
        };
        loop {
            while let Ok(ev) = menu_rx.try_recv() {
                if ev.id == id_show {
                    restore(&ctx);
                } else if ev.id == id_allon {
                    power(&shared, &tx, &ctx, Some(true));
                } else if ev.id == id_alloff {
                    power(&shared, &tx, &ctx, Some(false));
                } else if ev.id == id_quit {
                    shared.quit.store(true, Ordering::Relaxed);
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    ctx.request_repaint();
                }
            }
            // Double-clicking the tray icon restores the window.
            while let Ok(ev) = tray_rx.try_recv() {
                if let tray_icon::TrayIconEvent::DoubleClick { .. } = ev {
                    restore(&ctx);
                }
            }
            while let Ok(ev) = hk_rx.try_recv() {
                if ev.state != HotKeyState::Pressed {
                    continue;
                }
                if ev.id == hk_alloff {
                    power(&shared, &tx, &ctx, Some(false));
                } else if ev.id == hk_toggle {
                    power(&shared, &tx, &ctx, None);
                }
            }
            std::thread::sleep(Duration::from_millis(40));
        }
    });

    System {
        _tray: tray,
        _hotkeys: hotkeys,
    }
}
