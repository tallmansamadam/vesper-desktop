//! Single-instance guard.
//!
//! Without this, launching the shortcut twice gives you two tray icons, two
//! audio engines and two apps issuing conflicting commands to the same bulbs.
//! A named mutex detects the first instance; the second brings that window
//! back to the front (it may be hidden in the tray, which is exactly when
//! someone would click the shortcut again) and exits.
//!
//! Only the GUI takes the lock - the headless modes must still run while the
//! app is open, since that is how scheduled tasks apply scenes.

#![allow(non_snake_case)]

type Handle = usize;

#[link(name = "kernel32")]
extern "system" {
    fn CreateMutexW(attrs: usize, owner: i32, name: *const u16) -> Handle;
    fn GetLastError() -> u32;
}

#[link(name = "user32")]
extern "system" {
    fn FindWindowW(class: *const u16, title: *const u16) -> Handle;
    fn ShowWindow(hwnd: Handle, cmd: i32) -> i32;
    fn SetForegroundWindow(hwnd: Handle) -> i32;
    fn IsIconic(hwnd: Handle) -> i32;
}

const ERROR_ALREADY_EXISTS: u32 = 183;
const SW_SHOW: i32 = 5;
const SW_RESTORE: i32 = 9;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Returns true if another instance already owns the app, after nudging its
/// window to the front. The caller should exit immediately in that case.
///
/// The mutex handle is deliberately leaked: it must live for the whole
/// process, and Windows releases it on exit.
pub fn already_running(window_title: &str) -> bool {
    let name = wide("Local\\VesperSingleInstance");
    let taken = unsafe {
        let h = CreateMutexW(0, 1, name.as_ptr());
        h == 0 || GetLastError() == ERROR_ALREADY_EXISTS
    };
    if !taken {
        return false;
    }
    // Bring the running instance forward instead of silently doing nothing.
    let title = wide(window_title);
    unsafe {
        let hwnd = FindWindowW(std::ptr::null(), title.as_ptr());
        if hwnd != 0 {
            ShowWindow(hwnd, if IsIconic(hwnd) != 0 { SW_RESTORE } else { SW_SHOW });
            SetForegroundWindow(hwnd);
        }
    }
    true
}
