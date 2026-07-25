//! System-tray icon and global hotkeys.
//!
//! Both crates deliver their events through process-global crossbeam channels,
//! so a small background thread polls them and forwards to the app via [`Msg`]
//! + `request_repaint` - which keeps the tray responsive even while the window
//! is hidden (minimised to tray), when egui would otherwise not be redrawing.
//!
//! Hotkeys: Ctrl+Alt+O = all off, Ctrl+Alt+L = toggle all lights.

use std::path::Path;
use std::sync::mpsc::Sender;
use std::time::Duration;

use global_hotkey::{
    hotkey::{Code, HotKey, Modifiers},
    GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState,
};
use tray_icon::{
    menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem},
    TrayIcon, TrayIconBuilder,
};

use crate::Msg;

/// Holds the tray icon and hotkey manager alive for the app's lifetime.
pub struct System {
    _tray: Option<TrayIcon>,
    _hotkeys: Option<GlobalHotKeyManager>,
}

fn load_icon(root: &Path) -> Option<tray_icon::Icon> {
    let img = image::open(root.join("branding").join("vesper_icon_256.png")).ok()?;
    let rgba = img.to_rgba8();
    let (w, h) = (rgba.width(), rgba.height());
    tray_icon::Icon::from_rgba(rgba.into_raw(), w, h).ok()
}

pub fn init(root: &Path, tx: Sender<Msg>, ctx: egui::Context) -> System {
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
    let (id_show, id_allon, id_alloff, id_quit) =
        (show.id().clone(), allon.id().clone(), alloff.id().clone(), quit.id().clone());

    let mut builder = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_tooltip("Vesper");
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

    // ---- poll thread: forward tray/menu/hotkey events to the app ----
    let menu_rx = MenuEvent::receiver().clone();
    let hk_rx = GlobalHotKeyEvent::receiver().clone();
    std::thread::spawn(move || loop {
        let mut hit = false;
        while let Ok(ev) = menu_rx.try_recv() {
            let msg = if ev.id == id_show {
                Some(Msg::SysShow)
            } else if ev.id == id_allon {
                Some(Msg::SysPower(true))
            } else if ev.id == id_alloff {
                Some(Msg::SysPower(false))
            } else if ev.id == id_quit {
                Some(Msg::SysQuit)
            } else {
                None
            };
            if let Some(m) = msg {
                let _ = tx.send(m);
                hit = true;
            }
        }
        while let Ok(ev) = hk_rx.try_recv() {
            if ev.state == HotKeyState::Pressed {
                let msg = if ev.id == hk_alloff {
                    Some(Msg::SysPower(false))
                } else if ev.id == hk_toggle {
                    Some(Msg::SysToggle)
                } else {
                    None
                };
                if let Some(m) = msg {
                    let _ = tx.send(m);
                    hit = true;
                }
            }
        }
        if hit {
            ctx.request_repaint();
        }
        std::thread::sleep(Duration::from_millis(80));
    });

    System {
        _tray: tray,
        _hotkeys: hotkeys,
    }
}
