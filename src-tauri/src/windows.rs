use tauri::Manager;
use crate::config;

#[cfg(target_os = "windows")]
pub(crate) fn browser_args() -> String {
    let mut args = String::from("--renderer-process-limit=1 --disable-breakpad --disable-features=AudioServiceOutOfProcess,TranslateUI,msWebOOUI,msPdfOOUI,msSmartScreenProtection");
    if !config::with_config(|c| c.hardware_acceleration) {
        args.push_str(" --disable-gpu --disable-gpu-compositing --disable-features=GpuProcessPerClient");
    }
    args
}

pub fn open_settings(app: &tauri::AppHandle) {
    open_or_create_window(app, "settings", "设置 - 外设监控", "settings.html", 600.0, 580.0, true, false);
}

pub fn open_about(app: &tauri::AppHandle) {
    open_or_create_window(app, "about", "关于 外设监控", "about.html", 380.0, 360.0, false, true);
}

fn open_or_create_window(
    app: &tauri::AppHandle,
    label: &str,
    title: &str,
    url: &str,
    width: f64,
    height: f64,
    resizable: bool,
    center: bool,
) {
    if let Some(win) = app.get_webview_window(label) {
        let _ = win.unminimize();
        let _ = win.show();
        let _ = win.set_focus();
        return;
    }
    let app = app.clone();
    let label = label.to_string();
    let url = url.to_string();
    let title = title.to_string();
    tauri::async_runtime::spawn(async move {
        let mut builder = tauri::WebviewWindowBuilder::new(
            &app,
            &label,
            tauri::WebviewUrl::App(url.into()),
        )
        .title(&title)
        .inner_size(width, height)
        .resizable(resizable)
        .visible(false);

        #[cfg(target_os = "windows")]
        {
            builder = builder.additional_browser_args(&browser_args());
        }

        if center {
            builder = builder.center();
        } else {
            builder = builder.min_inner_size(400.0, 300.0)
                .background_color(tauri::utils::config::Color(243, 243, 243, 255));
        }

        if let Ok(win) = builder.build() {
            std::thread::sleep(std::time::Duration::from_millis(100));
            let _ = win.show();
            let _ = win.set_focus();
        }
    });
}

pub fn scale_factor(app: &tauri::AppHandle) -> f64 {
    app.primary_monitor()
        .ok()
        .flatten()
        .map(|m| m.scale_factor())
        .unwrap_or(1.0)
}

#[cfg(target_os = "windows")]
pub fn set_rounded_corners(hwnd: isize) {
    unsafe {
        const DWMWA_WINDOW_CORNER_PREFERENCE: u32 = 33;
        const DWMWCP_ROUND: u32 = 2;
        let preference = DWMWCP_ROUND;
        windows_sys::Win32::Graphics::Dwm::DwmSetWindowAttribute(
            hwnd as *mut core::ffi::c_void,
            DWMWA_WINDOW_CORNER_PREFERENCE,
            &preference as *const _ as *const _,
            std::mem::size_of::<u32>() as u32,
        );
    }
}
