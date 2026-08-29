use std::sync::atomic::Ordering;
use tauri::Emitter;
use tauri::Manager;

use crate::state::{ANIMATING, POPUP_POS, TRAY_POS};
use crate::windows;

const POPUP_W: f64 = 360.0;
/// 弹窗首选高度（工作区足够时采用）
const POPUP_H: f64 = 520.0;
/// 弹窗动态 clamp 下限（极低分屏下避免退化为不可用高度）
const POPUP_MIN_H: f64 = 200.0;
/// 弹窗距屏幕左右边缘的留白（逻辑像素＝物理 26×缩放因子，随缩放等比）
const POPUP_EDGE_MARGIN: f64 = 26.0;
/// 弹窗底边与任务栏上缘的固定间隙（13px 逻辑＝物理 13×缩放因子，随缩放等比）
const POPUP_TASKBAR_GAP: f64 = 13.0;

/// cubic-bezier(0.62, 0, 0.32, 1) easing — same as win11React
fn cubic_bezier(t: f64) -> f64 {
    let p1x = 0.62;
    let p2x = 0.32;
    // 以牛顿迭代法求解贝塞尔方程 x(t) = progress
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
    3.0 * t_param.powi(2) * (1.0 - t_param) + t_param.powi(3)
}

/// 弹窗位置计算结果
struct Placement {
    target_x: f64,
    target_y: f64,
    start_y: f64,
    popup_h: f64,
}

/// 计算弹窗位置参数
fn compute_position(app: &tauri::AppHandle) -> Placement {
    // 托盘所在屏工作区（fallback 主屏）：见 windows::resolve_work_area
    let info = windows::resolve_work_area(app);
    placement_from(info.work_x, info.work_y, info.work_w, info.work_h)
}

/// 依据工作区（逻辑坐标）算出弹窗落点与实高
fn placement_from(work_x: f64, work_y: f64, work_w: f64, work_h: f64) -> Placement {
    let tray_x = TRAY_POS
        .get()
        .map(|m| crate::state::lock_unpoisoned(m).0)
        .unwrap_or(work_x + work_w / 2.0);

    // 弹窗高度按工作区动态 clamp：低分屏不越出上缘，留 45px 上下余量
    let max_h = (work_h - 45.0).max(POPUP_MIN_H);
    let popup_h = POPUP_H.min(max_h).max(POPUP_MIN_H);

    // 底边与任务栏上缘固定留 POPUP_TASKBAR_GAP 间隙（不再依赖托盘图标 y 精度）
    let target_y = (work_y + work_h - popup_h - POPUP_TASKBAR_GAP).max(work_y + 8.0);

    // 水平钳制到工作区：居中托盘但不得溢出左右（留 POPUP_EDGE_MARGIN 边距）
    let min_x = work_x + POPUP_EDGE_MARGIN;
    let max_x = (work_x + work_w - POPUP_W - POPUP_EDGE_MARGIN).max(min_x);
    let target_x = (tray_x - POPUP_W / 2.0).clamp(min_x, max_x);

    Placement {
        target_x,
        target_y,
        start_y: work_y + work_h + 10.0,
        popup_h,
    }
}

pub fn toggle(app: &tauri::AppHandle, tab: &str) {
    if ANIMATING.load(Ordering::Relaxed) {
        return;
    }

    crate::process::append_log(&format!("[popup] toggle tab={}", tab));
    let p = compute_position(app);

    if let Some(window) = app.get_webview_window("popup") {
        let visible = window.is_visible().unwrap_or(false);
        if visible {
            close(&window, p.target_x, p.target_y, p.start_y);
        } else {
            let _ = app.emit("switch-tab", tab);
            show(&window, p.target_x, p.start_y, p.target_y, p.popup_h);
        }
    } else {
        create(app, p.target_x, p.target_y, p.popup_h, tab);
    }
}

pub fn open_popup(app: &tauri::AppHandle, tab: &str) {
    if ANIMATING.load(Ordering::Relaxed) {
        return;
    }

    let p = compute_position(app);

    if let Some(window) = app.get_webview_window("popup") {
        let _ = app.emit("switch-tab", tab);
        if !window.is_visible().unwrap_or(false) {
            show(&window, p.target_x, p.start_y, p.target_y, p.popup_h);
        }
    } else {
        create(app, p.target_x, p.target_y, p.popup_h, tab);
    }
}

fn close(window: &tauri::WebviewWindow, target_x: f64, target_y: f64, start_y: f64) {
    ANIMATING.store(true, Ordering::Relaxed);
    // 下滑全程保持低于任务栏（防御性重沉：若窗口曾被抬回波段顶则归位）
    if let Ok(hwnd) = window.hwnd() {
        windows::place_below_taskbar(hwnd.0 as isize);
    }
    let (cx, cy) = POPUP_POS
        .get()
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

fn show(window: &tauri::WebviewWindow, target_x: f64, start_y: f64, target_y: f64, popup_h: f64) {
    // popup 打开前 Resume WebView2 渲染进程（可能因关闭后 Suspend 或系统唤醒处于挂起状态）
    let wv: &tauri::Webview = window.as_ref();
    windows::resume_webview(wv);

    // 按当前工作区动态高度调整窗口（换显示器/换分辨率后高度可能变化）
    let _ = window.set_size(tauri::LogicalSize::new(POPUP_W, popup_h));

    ANIMATING.store(true, Ordering::Relaxed);
    // 先移到屏幕外，再置顶，最后显示：滑动全程位于其他窗口之上，
    // 避免非置顶状态下被前台窗口遮挡（表现为动画"丢失"或部分不可见）
    let _ = window.set_position(tauri::Position::Logical(tauri::LogicalPosition {
        x: target_x,
        y: start_y,
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

fn create(app: &tauri::AppHandle, target_x: f64, target_y: f64, popup_h: f64, tab: &str) {
    let url = if tab == "volume" {
        "popup.html#volume".to_string()
    } else {
        "popup.html".to_string()
    };

    // 恒透明创建：透明能力在窗口诞生时固化，「默认」材质的不透明观感由 CSS 承担
    #[cfg(target_os = "windows")]
    let builder = {
        let mut b =
            tauri::WebviewWindowBuilder::new(app, "popup", tauri::WebviewUrl::App(url.into()))
                .additional_browser_args(&crate::windows::browser_args())
                .title("外设信息")
                .inner_size(POPUP_W, popup_h)
                .decorations(false)
                .resizable(false)
                .skip_taskbar(true)
                .always_on_top(true)
                .position(target_x, target_y);
        b = b
            .transparent(true)
            .background_color(tauri::utils::config::Color(0, 0, 0, 0));
        b
    };

    #[cfg(not(target_os = "windows"))]
    let builder =
        tauri::WebviewWindowBuilder::new(app, "popup", tauri::WebviewUrl::App(url.into()))
            .title("外设信息")
            .inner_size(POPUP_W, popup_h)
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
            // 首启即按 show() 语义重设尺寸/位置：窗口先按创建屏 SF 诞生再被移到目标屏，
            // 混合 DPI 下 builder 的 inner_size 用了创建屏 SF，移动后物理尺寸失真；
            // 此时窗口已就位于目标屏，按逻辑重设即可用目标屏 SF 正确换算。
            let _ = win.set_size(tauri::LogicalSize::new(POPUP_W, popup_h));
            let _ = win.set_position(tauri::Position::Logical(tauri::LogicalPosition {
                x: target_x,
                y: target_y,
            }));
            let _ = win.show();
            let _ = win.set_focus();
            // 首启无滑动动画，但层级不变式一致：低于任务栏、高于普通窗口
            if let Ok(hwnd) = win.hwnd() {
                windows::place_below_taskbar(hwnd.0 as isize);
            }
            if let Some(pos) = POPUP_POS.get() {
                *crate::state::lock_unpoisoned(pos) = (target_x, target_y);
            }
            // WM_DPICHANGED 异步：延迟按目标屏 SF 补一次尺寸，杜绝首启残留创建屏尺寸
            let rewin = win.clone();
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(200));
                crate::process::append_verbose_log(&format!(
                    "[popup] create size reapply, popup_h={}",
                    popup_h
                ));
                let _ = rewin.set_size(tauri::LogicalSize::new(POPUP_W, popup_h));
            });
        }
        Err(e) => {
            crate::process::append_log(&format!("[popup] create window failed: {}", e));
        }
    }
}

/// 通用滑动动画
fn animate_slide(
    window: &tauri::WebviewWindow,
    x: f64,
    from_y: f64,
    to_y: f64,
    duration_ms: u64,
    frames: u64,
) {
    let step_ms = duration_ms / frames;
    for i in 0..=frames {
        let t = i as f64 / frames as f64;
        let y = from_y + (to_y - from_y) * cubic_bezier(t);
        if let Err(e) =
            window.set_position(tauri::Position::Logical(tauri::LogicalPosition { x, y }))
        {
            crate::process::append_log(&format!("[popup] set_position FAILED frame={}: {}", i, e));
        }
        std::thread::sleep(std::time::Duration::from_millis(step_ms));
    }
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
    // popup 关闭后 Suspend WebView2 渲染进程：
    // - 释放 CPU/内存（渲染进程休眠）
    // - 系统睡眠时已处于 Suspended 状态，不阻塞事件循环（B 类僵死根治）
    let wv: &tauri::Webview = window.as_ref();
    windows::suspend_webview(wv);
}
