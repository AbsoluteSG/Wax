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

// ------------------------------------------------------- window geometry

/// The monitor a window is on, in physical pixels.
#[derive(Clone, Copy, Debug)]
pub struct Monitor {
    pub bounds: egui::Rect,
    /// Excludes the taskbar.
    pub work: egui::Rect,
    pub refresh_hz: f32,
}

pub fn hwnd_of(frame: &eframe::Frame) -> Option<isize> {
    hwnd(frame)
}

#[cfg(windows)]
mod win {
    use super::Monitor;
    use eframe::egui::{pos2, Pos2, Rect};
    use windows_sys::Win32::Foundation::{POINT, RECT};
    use windows_sys::Win32::Graphics::Gdi::{
        EnumDisplaySettingsW, GetMonitorInfoW, MonitorFromWindow, DEVMODEW, ENUM_CURRENT_SETTINGS, MONITORINFO,
        MONITORINFOEXW, MONITOR_DEFAULTTONEAREST,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetCursorPos, GetWindowRect, SetWindowPos, SWP_NOACTIVATE, SWP_NOSIZE, SWP_NOZORDER,
    };

    fn rect(r: RECT) -> Rect {
        Rect::from_min_max(pos2(r.left as f32, r.top as f32), pos2(r.right as f32, r.bottom as f32))
    }

    pub fn cursor() -> Option<Pos2> {
        let mut p = POINT { x: 0, y: 0 };
        (unsafe { GetCursorPos(&mut p) } != 0).then(|| pos2(p.x as f32, p.y as f32))
    }

    pub fn window_rect(hwnd: isize) -> Option<Rect> {
        let mut r = RECT { left: 0, top: 0, right: 0, bottom: 0 };
        (unsafe { GetWindowRect(hwnd as _, &mut r) } != 0).then(|| rect(r))
    }

    pub fn move_window(hwnd: isize, x: i32, y: i32) {
        unsafe {
            SetWindowPos(hwnd as _, std::ptr::null_mut(), x, y, 0, 0, SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE);
        }
    }

    pub fn monitor(hwnd: isize) -> Option<Monitor> {
        unsafe {
            let mon = MonitorFromWindow(hwnd as _, MONITOR_DEFAULTTONEAREST);
            let mut info: MONITORINFOEXW = std::mem::zeroed();
            info.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
            if GetMonitorInfoW(mon, &mut info as *mut MONITORINFOEXW as *mut MONITORINFO) == 0 {
                return None;
            }
            let mut mode: DEVMODEW = std::mem::zeroed();
            mode.dmSize = std::mem::size_of::<DEVMODEW>() as u16;
            let hz = if EnumDisplaySettingsW(info.szDevice.as_ptr(), ENUM_CURRENT_SETTINGS, &mut mode) != 0 {
                mode.dmDisplayFrequency as f32
            } else {
                60.0
            };
            Some(Monitor {
                bounds: rect(info.monitorInfo.rcMonitor),
                work: rect(info.monitorInfo.rcWork),
                refresh_hz: if hz > 1.0 { hz } else { 60.0 },
            })
        }
    }
}

#[cfg(windows)]
pub use win::{cursor, monitor, move_window, window_rect};

#[cfg(not(windows))]
pub fn cursor() -> Option<egui::Pos2> {
    None
}
#[cfg(not(windows))]
pub fn window_rect(_: isize) -> Option<egui::Rect> {
    None
}
#[cfg(not(windows))]
pub fn move_window(_: isize, _: i32, _: i32) {}
#[cfg(not(windows))]
pub fn monitor(_: isize) -> Option<Monitor> {
    None
}
