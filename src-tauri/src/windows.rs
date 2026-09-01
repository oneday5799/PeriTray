//! 窗口基础设施：设置窗/弹窗的创建与定位（DPI/工作区）、系统暗色、任务栏层级、
//! 圆角、Toast 图标与 AUMID 注册。
//! WebView2 底层（背景色/生命周期）见 webview 模块；DWM 材质见 window_material 模块。

use crate::config;
use crate::process;
use tauri::{Emitter, Manager};

#[cfg(target_os = "windows")]
pub(crate) fn browser_args() -> String {
    let mut args = String::from("--renderer-process-limit=1 --disable-breakpad --disable-features=AudioServiceOutOfProcess,TranslateUI,msWebOOUI,msPdfOOUI,msSmartScreenProtection");
    if !config::with_config(|c| c.hardware_acceleration) {
        args.push_str(
            " --disable-gpu --disable-gpu-compositing --disable-features=GpuProcessPerClient",
        );
    }
    args
}

pub fn open_settings(app: &tauri::AppHandle) {
    open_settings_inner(app, None);
}

pub fn open_settings_tab(app: &tauri::AppHandle, tab: &str) {
    open_settings_inner(app, Some(tab));
}

fn open_settings_inner(app: &tauri::AppHandle, tab: Option<&str>) {
    if let Some(win) = app.get_webview_window("settings") {
        // 已有窗口的重开路径整体移出调用线程（菜单事件在事件线程上分发，
        // DWM 材质调用与 show/focus 一旦阻塞会拖垮整个事件循环）
        let app = app.clone();
        let tab = tab.map(|t| t.to_string());
        std::thread::spawn(move || {
            if let Some(ref t) = tab {
                let _ = app.emit_to("settings", "settings-tab", t);
            }
            #[cfg(target_os = "windows")]
            if let Ok(hwnd) = win.hwnd() {
                let material = config::with_config(|c| c.window_material.clone());
                process::append_log(&format!(
                    "[material] reopen settings, material={}",
                    material
                ));
                crate::window_material::apply_window_material(hwnd.0 as isize, &material);
            }
            let _ = win.unminimize();
            let _ = win.show();
            let _ = win.set_focus();
        });
        return;
    }
    let app = app.clone();
    let url = match tab {
        Some(t) => format!("settings.html#{}", t),
        None => "settings.html".to_string(),
    };
    tauri::async_runtime::spawn(async move {
        let mut builder =
            tauri::WebviewWindowBuilder::new(&app, "settings", tauri::WebviewUrl::App(url.into()))
                .title("设置 - 外设监控")
                .inner_size(960.0, 720.0)
                .resizable(true)
                .visible(false)
                .min_inner_size(720.0, 420.0)
                .prevent_overflow();

        // 恒透明创建：透明能力在窗口诞生时固化，「默认」材质的不透明观感由 CSS 承担
        builder = builder
            .transparent(true)
            .background_color(tauri::utils::config::Color(0, 0, 0, 0));

        #[cfg(target_os = "windows")]
        {
            builder = builder.additional_browser_args(&browser_args());
        }

        if let Ok(win) = builder.build() {
            #[cfg(target_os = "windows")]
            if let Ok(hwnd) = win.hwnd() {
                let material = config::with_config(|c| c.window_material.clone());
                crate::window_material::apply_window_material(hwnd.0 as isize, &material);
                crate::webview::ensure_webview_bg_transparent(win.as_ref());
            }
            // 窗口状态插件恢复后即钳制：跨分辨率/DPI 下恢复的物理尺寸可能越界
            clamp_window_to_work_area(&win);
            std::thread::sleep(std::time::Duration::from_millis(200));
            // 静置后 DPI 已稳定，再次钳制以兜底首帧缩放未就绪
            clamp_window_to_work_area(&win);
            let _ = win.show();
            let _ = win.set_focus();
        }
    });
}

pub fn scale_factor(app: &tauri::AppHandle) -> f64 {
    match app.primary_monitor() {
        Ok(Some(m)) => m.scale_factor(),
        _ => {
            crate::process::append_log("[monitor] primary_monitor unavailable, fallback 1.0");
            1.0
        }
    }
}

/// 物理坐标所在的显示器（`monitor_from_point` 参数为物理像素坐标）
fn monitor_at_point(app: &tauri::AppHandle, x: f64, y: f64) -> Option<tauri::Monitor> {
    app.monitor_from_point(x, y).ok().flatten()
}

/// 物理坐标所在显示器的定位信息：缩放因子 + 逻辑工作区（越过任务栏）。
/// 工作区为物理坐标，需除以 SF 转逻辑坐标供窗口定位使用。
/// 混合 DPI 场景下主屏 SF 会导焦点定位偏移，须按托盘实际所在屏取值。
pub fn monitor_info_at(
    app: &tauri::AppHandle,
    x: f64,
    y: f64,
) -> Option<crate::state::TrayMonitorInfo> {
    let m = monitor_at_point(app, x, y)?;
    let sf = m.scale_factor();
    let wa = m.work_area();
    Some(crate::state::TrayMonitorInfo {
        scale_factor: sf,
        work_x: wa.position.x as f64 / sf,
        work_y: wa.position.y as f64 / sf,
        work_w: wa.size.width as f64 / sf,
        work_h: wa.size.height as f64 / sf,
    })
}

/// 弹窗/设置窗定位通用的工作区（逻辑坐标）：优先托盘所在屏，否则主屏兜底。
pub fn resolve_work_area(app: &tauri::AppHandle) -> crate::state::TrayMonitorInfo {
    if let Some(info) = *crate::state::lock_unpoisoned(crate::state::get_tray_monitor()) {
        return info;
    }
    if let Some(m) = app.primary_monitor().ok().flatten() {
        let sf = m.scale_factor();
        let wa = m.work_area();
        return crate::state::TrayMonitorInfo {
            scale_factor: sf,
            work_x: wa.position.x as f64 / sf,
            work_y: wa.position.y as f64 / sf,
            work_w: wa.size.width as f64 / sf,
            work_h: wa.size.height as f64 / sf,
        };
    }
    crate::state::TrayMonitorInfo {
        scale_factor: 1.0,
        work_x: 0.0,
        work_y: 0.0,
        work_w: 1920.0,
        work_h: 1080.0,
    }
}

/// 定位任务栏所在显示器信息及其通知区锚点（首启用：托盘尚未被点击时据此确定弹窗所在屏）。
/// 返回 `(显示器信息, 通知区近似横坐标[逻辑], 任务栏垂直中心[逻辑])`。
/// 找不到任务栏（Explorer 重启间隙等）返回 None。
#[cfg(target_os = "windows")]
pub fn monitor_info_of_taskbar(
    app: &tauri::AppHandle,
) -> Option<(crate::state::TrayMonitorInfo, f64, f64)> {
    use windows_sys::Win32::Foundation::RECT;
    use windows_sys::Win32::UI::WindowsAndMessaging::{FindWindowW, GetWindowRect};

    let class = crate::process::to_wide("Shell_TrayWnd");
    unsafe {
        let taskbar = FindWindowW(class.as_ptr(), std::ptr::null());
        if taskbar.is_null() {
            crate::process::append_log("[monitor] taskbar not found, fallback primary");
            return None;
        }
        let mut rect = std::mem::zeroed::<RECT>();
        if GetWindowRect(taskbar, &mut rect) == 0 {
            crate::process::append_log("[monitor] GetWindowRect(taskbar) failed, fallback primary");
            return None;
        }
        let cx = (rect.left as f64 + rect.right as f64) / 2.0;
        let cy = (rect.top as f64 + rect.bottom as f64) / 2.0;
        let info = monitor_info_at(app, cx, cy)?;
        let sf = info.scale_factor;
        // 通知区近似位（右缘内缩 80 物理像素）；任务栏垂直中心即真实托盘 y
        let anchor_x = (rect.right as f64 - 80.0) / sf;
        let anchor_y = (rect.top as f64 + rect.bottom as f64) / 2.0 / sf;
        Some((info, anchor_x, anchor_y))
    }
}

#[cfg(not(target_os = "windows"))]
pub fn monitor_info_of_taskbar(
    _app: &tauri::AppHandle,
) -> Option<(crate::state::TrayMonitorInfo, f64, f64)> {
    None
}

/// 将窗口尺寸/位置钳制到其所在显示器工作区内（防跨分辨率/DPI 恢复越界）。
/// 尺寸走 inner 语义（`set_size` 即 inner），位置走 outer 左上角（`set_position` 即 outer）。
fn clamp_window_to_work_area(win: &tauri::WebviewWindow) {
    if win.is_maximized().unwrap_or(false) || win.is_minimized().unwrap_or(false) {
        return;
    }
    let (Ok(inner), Ok(outer), Ok(pos)) =
        (win.inner_size(), win.outer_size(), win.outer_position())
    else {
        return;
    };
    let sf = win.scale_factor().unwrap_or(1.0);
    if sf <= 0.0 {
        return;
    }

    // 外框边距（标题栏+边框，逻辑值）：inner/outer 之差即此
    let frame_w = (outer.width as f64 - inner.width as f64) / sf;
    let frame_h = (outer.height as f64 - inner.height as f64) / sf;

    let app = win.app_handle();
    // 以窗口中心（物理）定位所在显示器，拿不到回退主屏
    let center_x = pos.x as f64 + outer.width as f64 / 2.0;
    let center_y = pos.y as f64 + outer.height as f64 / 2.0;
    let wa = monitor_info_at(app, center_x, center_y).unwrap_or_else(|| resolve_work_area(app));

    const MARGIN: f64 = 16.0;
    const MIN_INNER_W: f64 = 720.0;
    const MIN_INNER_H: f64 = 420.0;

    let max_inner_w = (wa.work_w - MARGIN - frame_w).max(MIN_INNER_W);
    let max_inner_h = (wa.work_h - MARGIN - frame_h).max(MIN_INNER_H);

    let cur_inner_w = inner.width as f64 / sf;
    let cur_inner_h = inner.height as f64 / sf;
    let new_inner_w = cur_inner_w.min(max_inner_w);
    let new_inner_h = cur_inner_h.min(max_inner_h);

    let cur_x = pos.x as f64 / sf;
    let cur_y = pos.y as f64 / sf;
    let new_outer_w = new_inner_w + frame_w;
    let new_outer_h = new_inner_h + frame_h;

    // 越界判定：当前外框完全在目标工作区外则居中，否则「保位置只缩尺寸」平移回屏内
    let right = cur_x + new_outer_w;
    let bottom = cur_y + new_outer_h;
    let fully_out = right < wa.work_x
        || cur_x > wa.work_x + wa.work_w
        || bottom < wa.work_y
        || cur_y > wa.work_y + wa.work_h;

    let (new_x, new_y) = if fully_out {
        (
            wa.work_x + (wa.work_w - new_outer_w) / 2.0,
            wa.work_y + (wa.work_h - new_outer_h) / 2.0,
        )
    } else {
        let mut nx = cur_x;
        let mut ny = cur_y;
        if right > wa.work_x + wa.work_w {
            nx = wa.work_x + wa.work_w - new_outer_w;
        }
        if bottom > wa.work_y + wa.work_h {
            ny = wa.work_y + wa.work_h - new_outer_h;
        }
        if nx < wa.work_x {
            nx = wa.work_x;
        }
        if ny < wa.work_y {
            ny = wa.work_y;
        }
        (nx, ny)
    };

    if (new_inner_w - cur_inner_w).abs() > 0.5 || (new_inner_h - cur_inner_h).abs() > 0.5 {
        let _ = win.set_size(tauri::LogicalSize::new(new_inner_w, new_inner_h));
    }
    if (new_x - cur_x).abs() > 0.5 || (new_y - cur_y).abs() > 0.5 {
        let _ = win.set_position(tauri::Position::Logical(tauri::LogicalPosition {
            x: new_x,
            y: new_y,
        }));
    }
}

#[cfg(target_os = "windows")]
pub fn system_dark_mode() -> bool {
    use windows_sys::core::w;
    use windows_sys::Win32::System::Registry::{
        RegCloseKey, RegOpenKeyExW, RegQueryValueExW, HKEY_CURRENT_USER, KEY_READ, REG_DWORD,
    };
    unsafe {
        let mut hkey = std::ptr::null_mut();
        let status = RegOpenKeyExW(
            HKEY_CURRENT_USER,
            w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize"),
            0,
            KEY_READ,
            &mut hkey,
        );
        if status != 0 {
            return false;
        }
        let mut value: u32 = 1;
        let mut size = std::mem::size_of::<u32>() as u32;
        let mut data_type: u32 = REG_DWORD;
        let status = RegQueryValueExW(
            hkey,
            w!("AppsUseLightTheme"),
            std::ptr::null_mut(),
            &mut data_type,
            &mut value as *mut u32 as *mut u8,
            &mut size,
        );
        RegCloseKey(hkey);
        if status != 0 {
            return false;
        }
        value == 0
    }
}

#[cfg(not(target_os = "windows"))]
pub fn system_dark_mode() -> bool {
    false
}

/// 将窗口插入 topmost 波段内任务栏正下方：仍高于一切普通窗口，但不遮挡任务栏。
/// 用于弹窗动画期间与静止期的统一层级。找不到任务栏（如 Explorer 重启间隙）则保持原 Z 序。
#[cfg(target_os = "windows")]
pub fn place_below_taskbar(hwnd: isize) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        FindWindowW, SetWindowPos, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE,
    };
    let class = crate::process::to_wide("Shell_TrayWnd");
    unsafe {
        let taskbar = FindWindowW(class.as_ptr(), std::ptr::null());
        if !taskbar.is_null() {
            SetWindowPos(
                hwnd as *mut core::ffi::c_void,
                taskbar,
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
            );
        }
    }
}

#[cfg(not(target_os = "windows"))]
pub fn place_below_taskbar(_hwnd: isize) {}

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

// ═══════════════════════════════════════════════════════════════
// Toast 通知图标
// ═══════════════════════════════════════════════════════════════

/// 查找 toast 通知图标，返回可直接传给 `Toast::icon()` 的路径。
///
/// 图标来源（按优先级）：
/// 1. 已安装：exe 同目录 `icon.png`（Tauri 部署 `dist/icon.png` 到此）
/// 2. 开发：`../../dist/icon.png`（即 `src-tauri/dist/icon.png`）
///
/// 找到后复制为 `toast_icon.png` 到 exe 目录，避免 `canonicalize` 产生的
/// `\\?\` 前缀导致 WinRT `file:///` URI 失效。
#[cfg(target_os = "windows")]
pub fn resolve_toast_icon() -> Option<std::path::PathBuf> {
    let exe_path = std::env::current_exe().ok()?;
    let dir = exe_path.parent()?;
    let target = dir.join("toast_icon.png");
    for name in &["icon.png", "../../dist/icon.png"] {
        let src = dir.join(name);
        if src.exists() {
            let _ = std::fs::copy(&src, &target);
            return Some(target);
        }
    }
    None
}

#[cfg(not(target_os = "windows"))]
pub fn resolve_toast_icon() -> Option<std::path::PathBuf> {
    None
}

/// 装配带应用 AUMID 与（可选）圆角图标的 Toast，标题/正文由调用点指定。
/// 通知的 `on_activated` 回调与 `.show()` 由调用点链式补全，便于各自定制行为与日志标签。
#[cfg(target_os = "windows")]
pub fn build_toast(
    title: &str,
    text: &str,
    icon: Option<&std::path::Path>,
) -> tauri_winrt_notification::Toast {
    use tauri_winrt_notification::IconCrop;

    let mut toast = tauri_winrt_notification::Toast::new(AUMID)
        .title(title)
        .text1(text);
    if let Some(path) = icon {
        toast = toast.icon(path, IconCrop::Circular, "");
    }
    toast
}

// ═══════════════════════════════════════════════════════════════
// AUMID 注册（Windows 通知图标依赖）
// ═══════════════════════════════════════════════════════════════

pub(crate) const AUMID: &str = "com.peri.tray";

/// 注册 AUMID 到开始菜单快捷方式，使 Windows 通知显示应用图标。
/// 已存在同名快捷方式时跳过。
#[cfg(target_os = "windows")]
pub fn register_aumid() {
    use std::os::windows::process::CommandExt;
    use std::process::Command;

    let Ok(exe_path) = std::env::current_exe() else {
        process::append_verbose_log("[aumid] failed to get exe path");
        return;
    };
    let exe_dir = exe_path.parent().unwrap_or(exe_path.as_path());
    let exe_str = exe_path.to_string_lossy().replace('\'', "''");
    let dir_str = exe_dir.to_string_lossy().replace('\'', "''");

    // 查找 icon.ico：优先 exe 同目录（已安装），其次 src-tauri/icons（开发）
    let ico_path = if exe_dir.join("icon.ico").exists() {
        exe_dir.join("icon.ico")
    } else {
        let dev_ico = exe_dir.join("../../icons/icon.ico");
        if dev_ico.exists() {
            dev_ico
        } else {
            process::append_verbose_log("[aumid] icon.ico not found, skipping");
            return;
        }
    };
    let icon_str = ico_path.to_string_lossy().replace('\'', "''");

    // 开始菜单 Programs 目录
    let ps = format!(
        r#"
$programs = [Environment]::GetFolderPath('Programs')
$shortcutPath = Join-Path $programs 'PeriTray.lnk'
$shell = New-Object -ComObject WScript.Shell
$shortcut = $shell.CreateShortcut($shortcutPath)
$shortcut.TargetPath = '{exe}'
$shortcut.WorkingDirectory = '{dir}'
$shortcut.AppUserModelID = '{aumid}'
$shortcut.IconLocation = '{ico}'
$shortcut.Save()
"#,
        exe = exe_str,
        dir = dir_str,
        aumid = AUMID,
        ico = icon_str,
    );

    match Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &ps])
        .creation_flags(0x08000000) // CREATE_NO_WINDOW
        .output()
    {
        Ok(out) => {
            if out.status.success() {
                process::append_verbose_log("[aumid] registered successfully");
            } else {
                let stderr = String::from_utf8_lossy(&out.stderr);
                process::append_log(&format!("[aumid] registration failed: {}", stderr.trim()));
            }
        }
        Err(e) => process::append_verbose_log(&format!("[aumid] powershell exec error: {}", e)),
    }
}

#[cfg(not(target_os = "windows"))]
pub fn register_aumid() {}
