//! Tauri's set_icon only sets ICON_SMALL on Windows. Keep a separate owned
//! ICON_BIG alive for the taskbar and Alt+Tab until it is replaced or closed.
use windows::Win32::{
    Foundation::{HWND, LPARAM, WPARAM},
    UI::WindowsAndMessaging::{
        CreateIcon, DestroyIcon, SendMessageW, HICON, ICON_BIG, WM_GETICON, WM_SETICON,
    },
};

pub(super) struct TaskbarIcon(isize);
impl TaskbarIcon {
    pub(super) fn new(rgba: &[u8], width: u32, height: u32) -> windows::core::Result<Self> {
        let mut bgra = rgba.to_vec();
        let stride = width.div_ceil(32) as usize * 4;
        let mut mask = vec![0u8; stride * height as usize];
        for (i, pixel) in bgra.chunks_exact_mut(4).enumerate() {
            pixel.swap(0, 2);
            if pixel[3] == 0 {
                let x = i % width as usize;
                let y = i / width as usize;
                mask[y * stride + x / 8] |= 0x80 >> (x % 8);
            }
        }
        let handle = unsafe {
            CreateIcon(
                None,
                width as i32,
                height as i32,
                1,
                32,
                mask.as_ptr(),
                bgra.as_ptr(),
            )?
        };
        Ok(Self(handle.0 as isize))
    }

    pub(super) fn is_installed(&self, window: HWND) -> bool {
        unsafe {
            SendMessageW(window, WM_GETICON, Some(WPARAM(ICON_BIG as usize)), None).0 == self.0
        }
    }

    pub(super) fn install(&self, window: HWND) {
        // This is synchronous: the previous owned icon may be dropped only
        // after Windows has stopped using it. Never destroy the returned handle,
        // which may belong to Tauri/the window class rather than this module.
        unsafe {
            SendMessageW(
                window,
                WM_SETICON,
                Some(WPARAM(ICON_BIG as usize)),
                Some(LPARAM(self.0)),
            );
        }
    }
}
impl Drop for TaskbarIcon {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyIcon(HICON(self.0 as *mut _));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::{
        core::w,
        Win32::UI::WindowsAndMessaging::{CreateWindowExW, DestroyWindow, WS_OVERLAPPEDWINDOW},
    };
    #[test]
    fn taskbar_big_icon_is_installed_and_replaced_independently_of_small_icon() {
        let window = unsafe {
            CreateWindowExW(
                Default::default(),
                w!("STATIC"),
                w!("CTMCP icon verification"),
                WS_OVERLAPPEDWINDOW,
                0,
                0,
                64,
                64,
                None,
                None,
                None,
                None,
            )
            .unwrap()
        };
        let mut pixels = vec![255; 32 * 32 * 4];
        let light = TaskbarIcon::new(&pixels, 32, 32).unwrap();
        light.install(window);
        assert_eq!(
            unsafe { SendMessageW(window, WM_GETICON, Some(WPARAM(ICON_BIG as usize)), None) }.0,
            light.0
        );
        for p in pixels.chunks_exact_mut(4) {
            p[..3].fill(0);
        }
        let dark = TaskbarIcon::new(&pixels, 32, 32).unwrap();
        dark.install(window);
        drop(light);
        assert_eq!(
            unsafe { SendMessageW(window, WM_GETICON, Some(WPARAM(ICON_BIG as usize)), None) }.0,
            dark.0
        );
        unsafe {
            DestroyWindow(window).unwrap();
        }
    }
}
