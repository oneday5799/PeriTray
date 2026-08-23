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

/// 弹窗位置计算结果（含诊断字段，避免日志重复枚举显示器）
struct Placement {
    target_x: f64,
    target_y: f64,
    start_y: f64,
    sf: f64,
    screen_h: f64,
    tray: Option<(f64, f64)>,
}

/// 计算弹窗位置参数
fn compute_position(app: &tauri::AppHandle) -> Placement {
    let sf = windows::scale_factor(app);
    let screen_h = app.primary_monitor()
        .ok()
        .flatten()
        .map(|m| m.size().height as f64 / sf)
        .unwrap_or(1080.0);
    let tray = TRAY_POS.get()
        .map(|m| *crate::state::lock_unpoisoned(m));
    let (tray_x, tray_y) = tray.unwrap_or((100.0, screen_h - 50.0));
    Placement {
        target_x: tray_x - POPUP_W / 2.0,
        target_y: tray_y - POPUP_H - 15.0,
        start_y: screen_h + 10.0,
        sf,
        screen_h,
        tray,
    }
}

pub fn toggle(app: &tauri::AppHandle, tab: &str) {
    if ANIMATING.load(Ordering::Relaxed) {
        return;
    }

    crate::process::append_log(&format!("[popup] toggle enter tab={}", tab));
    let p = compute_position(app);
    crate::process::append_log(&format!(
        "[popup] coords tray={:?} sf={} screen_h={} target=({}, {}) start_y={}",
        p.tray, p.sf, p.screen_h, p.target_x, p.target_y, p.start_y
    ));

    if let Some(window) = app.get_webview_window("popup") {
        let visible = window.is_visible().unwrap_or(false);
        crate::process::append_log(&format!("[popup] is_visible -> {}", visible));
        if visible {
            close(&window, p.target_x, p.target_y, p.start_y);
            crate::process::append_log("[popup] close dispatched");
        } else {
            let _ = app.emit("switch-tab", tab);
            show(&window, p.target_x, p.start_y, p.target_y);
            crate::process::append_log("[popup] show dispatched");
        }
    } else {
        create(app, p.target_x, p.target_y, tab);
        crate::process::append_log("[popup] create dispatched");
    }
}

pub fn open_popup(app: &tauri::AppHandle, tab: &str) {
    if ANIMATING.load(Ordering::Relaxed) {
        return;
    }

    let p = compute_position(app);
    crate::process::append_log("[popup] open_popup dispatched");

    if let Some(window) = app.get_webview_window("popup") {
        let _ = app.emit("switch-tab", tab);
        if !window.is_visible().unwrap_or(false) {
            show(&window, p.target_x, p.start_y, p.target_y);
        }
    } else {
        create(app, p.target_x, p.target_y, tab);
    }
}

fn close(
    window: &tauri::WebviewWindow,
    target_x: f64,
    target_y: f64,
    start_y: f64,
) {
    ANIMATING.store(true, Ordering::Relaxed);
    // 下滑全程保持低于任务栏（防御性重沉：若窗口曾被抬回波段顶则归位）
    if let Ok(hwnd) = window.hwnd() {
        windows::place_below_taskbar(hwnd.0 as isize);
    }
    let (cx, cy) = POPUP_POS.get()
        .map(|m| *crate::state::lock_unpoisoned(m))
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
    let p = compute_position(app);
    if let Some(window) = app.get_webview_window("popup") {
        if window.is_visible().unwrap_or(false) {
            close(&window, p.target_x, p.target_y, p.start_y);
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
    // 先移到屏幕外，再置顶，最后显示：滑动全程位于其他窗口之上，
    // 避免非置顶状态下被前台窗口遮挡（表现为动画"丢失"或部分不可见）
    let _ = window.set_position(tauri::Position::Logical(tauri::LogicalPosition {
        x: target_x, y: start_y,
    }));
    let _ = window.set_always_on_top(true);
    let _ = window.show();
    // 沉到 topmost 波段内任务栏正下方：动画期间高于普通窗口、不遮挡任务栏
    if let Ok(hwnd) = window.hwnd() {
        windows::place_below_taskbar(hwnd.0 as isize);
    }
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
            // 首启无滑动动画，但层级不变式一致：低于任务栏、高于普通窗口
            if let Ok(hwnd) = win.hwnd() {
                windows::place_below_taskbar(hwnd.0 as isize);
            }
            if let Some(pos) = POPUP_POS.get() {
                *crate::state::lock_unpoisoned(pos) = (target_x, target_y);
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
        if let Err(e) = window.set_position(tauri::Position::Logical(tauri::LogicalPosition { x, y })) {
            crate::process::append_log(&format!("[popup] set_position FAILED frame={}: {}", i, e));
        }
        std::thread::sleep(std::time::Duration::from_millis(step_ms));
    }
    crate::process::append_log(&format!(
        "[popup] slide done x={} from_y={} to_y={}",
        x, from_y, to_y
    ));
}

fn animate_open(window: &tauri::WebviewWindow, x: f64, start_y: f64, end_y: f64) {
    animate_slide(window, x, start_y, end_y, 250, 20);
    if let Some(pos) = POPUP_POS.get() {
        *crate::state::lock_unpoisoned(pos) = (x, end_y);
    }
    let _ = window.set_focus();
    // set_focus 的激活可能把窗口抬回波段顶，重新沉降到任务栏之下（静止态恒低于任务栏）
    if let Ok(hwnd) = window.hwnd() {
        windows::place_below_taskbar(hwnd.0 as isize);
    }
}

fn animate_close(window: &tauri::WebviewWindow, x: f64, start_y: f64, end_y: f64) {
    animate_slide(window, x, start_y, end_y, 200, 16);
    let _ = window.hide();
}
