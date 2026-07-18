//! Screen colour sampling for screen-sync, via GDI (no extra crates - the
//! four functions we need are declared directly against user32/gdi32).
//! Captures the virtual screen downsampled to 48x27 and returns the
//! dominant vivid (hue, sat, val), exactly like effects.py sample_screen.

#![allow(non_snake_case, clippy::too_many_arguments)]

use crate::color::rgb_to_hsv;

type Handle = usize;

#[repr(C)]
struct BitmapInfoHeader {
    biSize: u32,
    biWidth: i32,
    biHeight: i32,
    biPlanes: u16,
    biBitCount: u16,
    biCompression: u32,
    biSizeImage: u32,
    biXPelsPerMeter: i32,
    biYPelsPerMeter: i32,
    biClrUsed: u32,
    biClrImportant: u32,
}

#[repr(C)]
struct BitmapInfo {
    header: BitmapInfoHeader,
    colors: [u32; 3],
}

#[link(name = "user32")]
extern "system" {
    fn GetDC(hwnd: Handle) -> Handle;
    fn ReleaseDC(hwnd: Handle, hdc: Handle) -> i32;
    fn GetSystemMetrics(index: i32) -> i32;
}

#[link(name = "gdi32")]
extern "system" {
    fn CreateCompatibleDC(hdc: Handle) -> Handle;
    fn CreateCompatibleBitmap(hdc: Handle, w: i32, h: i32) -> Handle;
    fn SelectObject(hdc: Handle, obj: Handle) -> Handle;
    fn DeleteObject(obj: Handle) -> i32;
    fn DeleteDC(hdc: Handle) -> i32;
    fn SetStretchBltMode(hdc: Handle, mode: i32) -> i32;
    fn StretchBlt(
        dst: Handle, x: i32, y: i32, w: i32, h: i32,
        src: Handle, sx: i32, sy: i32, sw: i32, sh: i32,
        rop: u32,
    ) -> i32;
    fn GetDIBits(
        hdc: Handle, bmp: Handle, start: u32, lines: u32,
        bits: *mut u8, info: *mut BitmapInfo, usage: u32,
    ) -> i32;
}

const HALFTONE: i32 = 4;
const SRCCOPY: u32 = 0x00CC0020;
const DIB_RGB_COLORS: u32 = 0;
const SM_XVIRTUALSCREEN: i32 = 76;
const SM_YVIRTUALSCREEN: i32 = 77;
const SM_CXVIRTUALSCREEN: i32 = 78;
const SM_CYVIRTUALSCREEN: i32 = 79;

const W: i32 = 48;
const H: i32 = 27;

/// Dominant vivid (hue, sat, val) of the current screen, each 0..1.
pub fn sample_screen() -> Option<(f32, f32, f32)> {
    let mut buf = vec![0u8; (W * H * 4) as usize];
    unsafe {
        let sdc = GetDC(0);
        if sdc == 0 {
            return None;
        }
        let mdc = CreateCompatibleDC(sdc);
        let bmp = CreateCompatibleBitmap(sdc, W, H);
        let old = SelectObject(mdc, bmp);
        SetStretchBltMode(mdc, HALFTONE);
        let vx = GetSystemMetrics(SM_XVIRTUALSCREEN);
        let vy = GetSystemMetrics(SM_YVIRTUALSCREEN);
        let vw = GetSystemMetrics(SM_CXVIRTUALSCREEN).max(1);
        let vh = GetSystemMetrics(SM_CYVIRTUALSCREEN).max(1);
        StretchBlt(mdc, 0, 0, W, H, sdc, vx, vy, vw, vh, SRCCOPY);
        let mut info = BitmapInfo {
            header: BitmapInfoHeader {
                biSize: std::mem::size_of::<BitmapInfoHeader>() as u32,
                biWidth: W,
                biHeight: -H, // top-down
                biPlanes: 1,
                biBitCount: 32,
                biCompression: 0, // BI_RGB
                biSizeImage: 0,
                biXPelsPerMeter: 0,
                biYPelsPerMeter: 0,
                biClrUsed: 0,
                biClrImportant: 0,
            },
            colors: [0; 3],
        };
        let got = GetDIBits(
            mdc, bmp, 0, H as u32, buf.as_mut_ptr(), &mut info, DIB_RGB_COLORS,
        );
        SelectObject(mdc, old);
        DeleteObject(bmp);
        DeleteDC(mdc);
        ReleaseDC(0, sdc);
        if got == 0 {
            return None;
        }
    }

    // Weighted circular mean over hue; weight favours saturated pixels.
    let (mut xs, mut ys, mut ss, mut vs, mut ws) = (0f64, 0f64, 0f64, 0f64, 0f64);
    for px in buf.chunks_exact(4) {
        let (b, g, r) = (px[0], px[1], px[2]); // GDI gives BGRA
        let (h, s, v) = rgb_to_hsv(r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0);
        let w = (s * v + 0.02) as f64;
        let a = 2.0 * std::f64::consts::PI * h as f64;
        xs += a.cos() * w;
        ys += a.sin() * w;
        ss += s as f64 * w;
        vs += v as f64 * w;
        ws += w;
    }
    if ws <= 0.0 {
        return Some((0.0, 0.0, 0.0));
    }
    let hue = (ys.atan2(xs) / (2.0 * std::f64::consts::PI)).rem_euclid(1.0);
    Some((hue as f32, (ss / ws) as f32, (vs / ws) as f32))
}
