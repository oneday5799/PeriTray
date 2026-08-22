use std::sync::atomic::Ordering;
use tauri::Manager;
use tauri::Emitter;

use crate::windows;
use crate::state::{TRAY_POS, POPUP_POS, ANIMATING};

const POPUP_W: f64 = 360.0;
const POPUP_H: f64 = 520.0;

/// cubic-bezier(0.62, 0, 0.32, 1) easing — same as win11React
fn cubic_bezier(t: f64) -> f64 {
    let p1x = 0.62;
    let p2x = 0.32;
    // solve x(t) = progress for t using Newton-Raphson
    let mut t_param = t;
    for _ in 0..8 {
        let x = 3.0 * p1x * t_param * (1.0 - t_param).powi(2)
            + 3.0 * p2x * t_param.powi(2) * (1.0 - t_param)
            + t_param.powi(3);
        let dx = 3.0 * p1x * (1.0 - t_param).powi(2)
            + 6.0 * (p2x - p1x) * t_param * (1.0 - t_param)
            + 3.0 * (1.0 - p2x) * t_param.powi(2);
        t_param -= (x - t) / dx;
        t_param = t_param.clamp(0.0, 1.0);
    }
    3.0 * t_param.powi(2) * (1.0 - t_param)
        + t_param.powi(3)
}

/// 计算弹窗位置参数：(target_x, target_y, start_y)
fn compute_position(app: &tauri::AppHandle) -> (f64, f64, f64) {
    let sf = windows::scale_factor(app);
    let screen_h = app.primary_monitor()
        .ok()
        .flatten()
        .map(|m| m.size().height as f64 / sf)
        .unwrap_or(1080.0);
    let (tray_x, tray_y) = TRAY_POS.get()
        .map(|m| *m.lock().unwrap_or_else(|e| e.into_inner()))
        .unwrap_or((100.0, screen_h - 50.0));
    let target_x = tray_x - POPUP_W / 2.0;
    let target_y = tray_y - POPUP_H - 15.0;
    let start_y = screen_h + 10.0;
    (target_x, target_y, start_y)
}

pub fn toggle(app: &tauri::AppHandle, tab: &str) {
    if ANIMATING.load(Ordering::Relaxed) {
        return;
    }

    crate::process::append_log(&format!("[tray] toggle enter tab={}", tab));
    let (target_x, target_y, start_y) = compute_position(app);
    crate::process::append_log("[tray] compute_position done");

    if let Some(window) = app.get_webview_window("popup") {
        let visible = window.is_visible().unwrap_or(false);
        crate::process::append_log(&format!("[tray] is_visible -> {}", visible));
        if visible {
            close(&window, target_x, target_y, start_y);
            crate::process::append_log("[tray] close dispatched");
        } else {
            let _ = app.emit("switch-tab", tab);
            show(&window, target_x, start_y, target_y);
            crate::process::append_log("[tray] show dispatched");
        }
    } else {
        create(app, target_x, target_y, tab);
        crate::process::append_log("[tray] create dispatched");
    }
}

pub fn open_popup(app: &tauri::AppHandle, tab: &str) {
    if ANIMATING.load(Ordering::Relaxed) {
        return;
    }

    let (target_x, target_y, start_y) = compute_position(app);
    crate::process::append_log("[tray] open_popup dispatched");

    if let Some(window) = app.get_webview_window("popup") {
        let _ = app.emit("switch-tab", tab);
        if !window.is_visible().unwrap_or(false) {
            show(&window, target_x, start_y, target_y);
        }
    } else {
        create(app, target_x, target_y, tab);
    }
}

fn close(
    window: &tauri::WebviewWindow,
    target_x: f64,
    target_y: f64,
    start_y: f64,
) {
    ANIMATING.store(true, Ordering::Relaxed);
    let _ = window.set_always_on_top(false);
    let (cx, cy) = POPUP_POS.get()
        .map(|m| *m.lock().unwrap_or_else(|e| e.into_inner()))
        .unwrap_or((target_x, target_y));
    let win = window.clone();
    std::thread::spawn(move || {
        animate_close(&win, cx, cy, start_y);
        ANIMATING.store(false, Ordering::Relaxed);
    });
}

pub fn close_popup(app: &tauri::AppHandle) {
    if ANIMATING.load(Ordering::Relaxed) {
        return;
    }
    let (target_x, target_y, start_y) = compute_position(app);
    if let Some(window) = app.get_webview_window("popup") {
        if window.is_visible().unwrap_or(false) {
            close(&window, target_x, target_y, start_y);
        }
    }
}

fn show(
    window: &tauri::WebviewWindow,
    target_x: f64,
    start_y: f64,
    target_y: f64,
) {
    ANIMATING.store(true, Ordering::Relaxed);
    let _ = window.set_always_on_top(false);
    let _ = window.set_position(tauri::Position::Logical(tauri::LogicalPosition {
        x: target_x, y: start_y,
    }));
    let _ = window.show();
    let win = window.clone();
    std::thread::spawn(move || {
        animate_open(&win, target_x, start_y, target_y);
        ANIMATING.store(false, Ordering::Relaxed);
    });
}

fn create(app: &tauri::AppHandle, target_x: f64, target_y: f64, tab: &str) {
    let url = if tab == "volume" {
        "popup.html#volume".to_string()
    } else {
        "popup.html".to_string()
    };

    // 恒透明创建：透明能力在窗口诞生时固化，「默认」材质的不透明观感由 CSS 承担
    #[cfg(target_os = "windows")]
    let builder = {
        let mut b = tauri::WebviewWindowBuilder::new(
            app, "popup", tauri::WebviewUrl::App(url.into()),
        )
        .additional_browser_args(&crate::windows::browser_args())
        .title("外设信息")
        .inner_size(POPUP_W, POPUP_H)
        .decorations(false)
        .resizable(false)
        .skip_taskbar(true)
        .always_on_top(true)
        .position(target_x, target_y);
        b = b.transparent(true)
            .background_color(tauri::utils::config::Color(0, 0, 0, 0));
        b
    };

    #[cfg(not(target_os = "windows"))]
    let builder = tauri::WebviewWindowBuilder::new(
        app, "popup", tauri::WebviewUrl::App(url.into()),
    )
    .title("外设信息")
    .inner_size(POPUP_W, POPUP_H)
    .decorations(false)
    .resizable(false)
    .skip_taskbar(true)
    .always_on_top(true)
    .position(target_x, target_y);

    match builder.build() {
        Ok(win) => {
            #[cfg(target_os = "windows")]
            if let Ok(hwnd) = win.hwnd() {
                windows::set_rounded_corners(hwnd.0 as isize);
                // 应用窗口材质（DWM 层；webview 表面恒透明，COM 一次性设定与材质无关）
                let material = crate::config::with_config(|c| c.window_material.clone());
                windows::apply_window_material(hwnd.0 as isize, &material);
                let wv: &tauri::Webview = win.as_ref();
                windows::ensure_webview_bg_transparent(wv);
            }
            let _ = win.show();
            let _ = win.set_focus();
            if let Some(pos) = POPUP_POS.get() {
                *pos.lock().unwrap_or_else(|e| e.into_inner()) = (target_x, target_y);
            }
        }
        Err(e) => {
            crate::process::append_log(&format!("[popup] create window failed: {}", e));
        }
    }
}

/// 通用滑动动画
fn animate_slide(window: &tauri::WebviewWindow, x: f64, from_y: f64, to_y: f64, duration_ms: u64, frames: u64) {
    let step_ms = duration_ms / frames;
    for i in 0..=frames {
        let t = i as f64 / frames as f64;
        let y = from_y + (to_y - from_y) * cubic_bezier(t);
        let _ = window.set_position(tauri::Position::Logical(tauri::LogicalPosition { x, y }));
        std::thread::sleep(std::time::Duration::from_millis(step_ms));
    }
}

fn animate_open(window: &tauri::WebviewWindow, x: f64, start_y: f64, end_y: f64) {
    animate_slide(window, x, start_y, end_y, 250, 20);
    if let Some(pos) = POPUP_POS.get() {
        *pos.lock().unwrap_or_else(|e| e.into_inner()) = (x, end_y);
    }
    let _ = window.set_always_on_top(true);
    let _ = window.set_focus();
}

fn animate_close(window: &tauri::WebviewWindow, x: f64, start_y: f64, end_y: f64) {
    animate_slide(window, x, start_y, end_y, 200, 16);
    let _ = window.hide();
}
