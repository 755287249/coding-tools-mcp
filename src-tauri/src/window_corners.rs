//! Rounded corners for the frameless, transparent main window.
//!
//! Windows 11 rounds top-level windows through DWM (antialiased, and the
//! native blur backdrop stays inside the corners). Windows 10 has no such
//! option, so there the window is clipped with a rounded region instead,
//! recomputed on every resize and dropped while maximized or fullscreen.
//! The radius matches `--window-radius` in app.css.

#[cfg(windows)]
mod imp {
    use std::sync::atomic::{AtomicU32, Ordering};

    use windows::Win32::Foundation::HWND;
    use windows::Win32::Graphics::Dwm::{
        DwmSetWindowAttribute, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND,
    };
    use windows::Win32::Graphics::Gdi::{CreateRoundRectRgn, DeleteObject, SetWindowRgn};

    /// Logical pixels; keep in sync with `--window-radius` in app.css.
    const RADIUS: f64 = 8.0;
    const FIRST_WINDOWS_11_BUILD: u32 = 22000;
    /// Detected Windows build; 0 while detection is still pending.
    static BUILD: AtomicU32 = AtomicU32::new(0);

    fn uses_region() -> bool {
        let build = BUILD.load(Ordering::Relaxed);
        build != 0 && build < FIRST_WINDOWS_11_BUILD
    }

    pub fn init(window: tauri::WebviewWindow) {
        // Build detection spawns `cmd /c ver`; keep it off the UI thread.
        std::thread::spawn(move || {
            // Unknown builds are treated like Windows 10: a region works everywhere.
            let build = crate::commands::get_windows_build().unwrap_or(1).max(1);
            BUILD.store(build, Ordering::Relaxed);
            let Ok(hwnd) = window.hwnd() else {
                return;
            };
            let hwnd = HWND(hwnd.0);
            if build >= FIRST_WINDOWS_11_BUILD {
                let preference = DWMWCP_ROUND;
                // SAFETY: valid window handle and a correctly sized attribute value.
                unsafe {
                    let _ = DwmSetWindowAttribute(
                        hwnd,
                        DWMWA_WINDOW_CORNER_PREFERENCE,
                        &preference as *const _ as *const core::ffi::c_void,
                        std::mem::size_of_val(&preference) as u32,
                    );
                }
                return;
            }
            if window.is_minimized().unwrap_or(false) {
                return;
            }
            let square =
                window.is_maximized().unwrap_or(false) || window.is_fullscreen().unwrap_or(false);
            if let Ok(size) = window.outer_size() {
                let scale = window.scale_factor().unwrap_or(1.0);
                apply_region(hwnd, size.width, size.height, scale, square);
            }
        });
    }

    /// Re-clips the window after a resize / maximize / DPI change (Windows 10).
    pub fn refresh(window: &tauri::Window) {
        if !uses_region() || window.is_minimized().unwrap_or(false) {
            return;
        }
        let Ok(hwnd) = window.hwnd() else {
            return;
        };
        let square =
            window.is_maximized().unwrap_or(false) || window.is_fullscreen().unwrap_or(false);
        if let Ok(size) = window.outer_size() {
            let scale = window.scale_factor().unwrap_or(1.0);
            apply_region(HWND(hwnd.0), size.width, size.height, scale, square);
        }
    }

    fn apply_region(hwnd: HWND, width: u32, height: u32, scale: f64, square: bool) {
        // SAFETY: plain GDI/User32 calls on a valid window handle. After a
        // successful SetWindowRgn the system owns the region.
        unsafe {
            if square || width == 0 || height == 0 {
                SetWindowRgn(hwnd, None, true);
                return;
            }
            let diameter = (RADIUS * 2.0 * scale.max(1.0)).round() as i32;
            let region = CreateRoundRectRgn(
                0,
                0,
                width as i32 + 1,
                height as i32 + 1,
                diameter,
                diameter,
            );
            if region.is_invalid() {
                return;
            }
            if SetWindowRgn(hwnd, Some(region), true) == 0 {
                let _ = DeleteObject(region.into());
            }
        }
    }
}

#[cfg(windows)]
pub use imp::{init, refresh};

#[cfg(not(windows))]
pub fn init(_window: tauri::WebviewWindow) {}

#[cfg(not(windows))]
pub fn refresh(_window: &tauri::Window) {}
