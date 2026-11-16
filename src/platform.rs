//! Native window touches the toolkit can't do itself: blurring what's behind
//! the (transparent) window, and clipping it to rounded corners or a capsule.

use eframe::egui;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

#[derive(Default)]
pub struct Glass {
    blurred: bool,
    /// Last region applied: (width, height, corner radius) in physical pixels.
    shape: Option<(i32, i32, i32)>,
}

impl Glass {
    /// Call every frame; does work only when the window size or shape changes.
    pub fn update(&mut self, frame: &eframe::Frame, ctx: &egui::Context, radius_pts: f32) {
        let Some(hwnd) = hwnd(frame) else { return };
        if !self.blurred {
            self.blurred = true;
            if let Ok(handle) = frame.window_handle() {
                let _ = window_vibrancy::apply_blur(handle, Some((10, 10, 14, 0)));
            }
        }
        let (size, ppp, maximized) = ctx.input(|i| {
            let size = i.viewport().inner_rect.map_or(egui::Vec2::ZERO, |r| r.size());
            (size, i.pixels_per_point, i.viewport().maximized.unwrap_or(false))
        });
        let (w, h) = ((size.x * ppp).round() as i32, (size.y * ppp).round() as i32);
        let r = if maximized { 0 } else { (radius_pts * ppp).round() as i32 };
        if w <= 0 || h <= 0 || self.shape == Some((w, h, r)) {
            return;
        }
        self.shape = Some((w, h, r));
        set_region(hwnd, w, h, r);
    }
}

fn hwnd(frame: &eframe::Frame) -> Option<isize> {
    match frame.window_handle().ok()?.as_raw() {
        RawWindowHandle::Win32(h) => Some(h.hwnd.get()),
        _ => None,
    }
}

#[cfg(windows)]
fn set_region(hwnd: isize, w: i32, h: i32, r: i32) {
    use windows_sys::Win32::Graphics::Gdi::{CreateRoundRectRgn, SetWindowRgn};
    // The window owns the region afterwards; it must not be freed here.
    unsafe {
        let rgn = if r > 0 {
            CreateRoundRectRgn(0, 0, w + 1, h + 1, r * 2, r * 2)
        } else {
            CreateRoundRectRgn(0, 0, w + 1, h + 1, 0, 0)
        };
        SetWindowRgn(hwnd as _, rgn, 1);
    }
}

#[cfg(not(windows))]
fn set_region(_: isize, _: i32, _: i32, _: i32) {}
