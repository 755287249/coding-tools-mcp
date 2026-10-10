//! Native taskbar/tray icons follow the OS surface, independently of web UI theme.

#[cfg(any(feature = "desktop", test))]
fn icon_pixels(source: &[u8], light_surface: bool) -> Vec<u8> {
    let mut pixels = source.to_vec();
    if light_surface {
        for pixel in pixels.chunks_exact_mut(4) {
            for channel in &mut pixel[..3] {
                *channel = 255 - *channel;
            }
        }
    }
    pixels
}

#[cfg(feature = "desktop")]
mod desktop {
    use super::icon_pixels;
    use std::{collections::HashMap, time::Duration};
    use tauri::{image::Image, AppHandle, Manager, Theme};

    pub const TRAY_ID: &str = "main-tray";

    // Windows allows the taskbar to be light while app windows are dark (or vice
    // versa). AppsUseLightTheme/window.theme() alone cannot choose a tray icon.
    #[cfg(target_os = "windows")]
    fn windows_taskbar_light() -> Option<bool> {
        use windows::{
            core::w,
            Win32::{
                Foundation::ERROR_SUCCESS,
                System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD},
            },
        };
        let mut value = 0u32;
        let mut size = std::mem::size_of::<u32>() as u32;
        let result = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize"),
                w!("SystemUsesLightTheme"),
                RRF_RT_REG_DWORD,
                None,
                Some((&mut value as *mut u32).cast()),
                Some(&mut size),
            )
        };
        (result == ERROR_SUCCESS && size == 4).then_some(value != 0)
    }

    pub fn light_surface(app: &AppHandle) -> bool {
        #[cfg(target_os = "windows")]
        if let Some(light) = windows_taskbar_light() {
            return light;
        }
        app.get_webview_window("main")
            .and_then(|window| window.theme().ok())
            .is_some_and(|theme| theme == Theme::Light)
    }

    pub fn icon(app: &AppHandle, light: bool) -> Option<Image<'static>> {
        let source = app.default_window_icon()?;
        Some(Image::new_owned(
            icon_pixels(source.rgba(), light),
            source.width(),
            source.height(),
        ))
    }

    pub fn start(app: AppHandle) {
        tauri::async_runtime::spawn(async move {
            let mut applied_tray = None;
            let mut applied_windows = HashMap::new();
            let mut interval = tokio::time::interval(Duration::from_secs(2));
            loop {
                interval.tick().await;
                let light = light_surface(&app);
                let windows = app.webview_windows();
                applied_windows.retain(|label, _| windows.contains_key(label));
                if applied_tray == Some(light)
                    && windows
                        .keys()
                        .all(|label| applied_windows.get(label) == Some(&light))
                {
                    continue;
                }
                let Some(image) = icon(&app, light) else {
                    continue;
                };
                if applied_tray != Some(light) {
                    if let Some(tray) = app.tray_by_id(TRAY_ID) {
                        if tray
                            .set_icon_with_as_template(Some(image.clone()), true)
                            .is_ok()
                        {
                            applied_tray = Some(light);
                        }
                    }
                }
                for (label, window) in windows {
                    if applied_windows.get(&label) != Some(&light)
                        && window.set_icon(image.clone()).is_ok()
                    {
                        applied_windows.insert(label, light);
                    }
                }
            }
        });
    }
}

#[cfg(feature = "desktop")]
pub use desktop::{icon, light_surface, start, TRAY_ID};

#[cfg(test)]
mod tests {
    use super::icon_pixels;

    #[test]
    fn native_icon_remains_visible_on_both_surfaces_and_preserves_shape() {
        let source = image::load_from_memory(include_bytes!("../icons/32x32.png"))
            .unwrap()
            .into_rgba8();
        let dark = icon_pixels(source.as_raw(), false);
        let light = icon_pixels(source.as_raw(), true);
        assert_eq!(source.dimensions(), (32, 32));
        assert_eq!(dark, *source.as_raw());
        let mut visible_white = 0;
        let mut transparent = 0;
        for (before, after) in dark.chunks_exact(4).zip(light.chunks_exact(4)) {
            assert_eq!(before[3], after[3], "anti-aliased alpha must survive");
            if before[3] == 0 {
                transparent += 1;
            }
            if before[3] > 200 && before[..3].iter().all(|c| *c > 220) {
                visible_white += 1;
                assert!(
                    after[..3].iter().all(|c| *c < 35),
                    "light surface needs dark ink"
                );
            }
        }
        assert!(
            visible_white > 20,
            "the actual icon must contain visible strokes"
        );
        assert!(
            transparent > 20,
            "do not turn the transparent icon into a tile"
        );
        assert_eq!(
            icon_pixels(&light, true),
            dark,
            "theme switches are reversible"
        );
    }
}
