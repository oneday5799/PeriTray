//! 窗口基础设施：设置窗/弹窗的创建与定位（DPI/工作区）、系统暗色、任务栏层级、
//! 圆角、Toast 图标与 AUMID 注册。
//! WebView2 底层（背景色/生命周期）见 webview 模块；DWM 材质见 window_material 模块。

use crate::config;
use crate::process;
use tauri::{Emitter, Manager};

#[cfg(target_os = "windows")]
use crate::{standard_log, verbose_log};
// 最大化钳工作区子类（maximize_workarea_proc）：MINMAXINFO/显示器查询/
// comctl32 子类链，均在 WindowsAndMessaging / Gdi / Shell 三个已启用 feature 内
#[cfg(target_os = "windows")]
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
#[cfg(target_os = "windows")]
use windows_sys::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MonitorFromRect, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST,
};
#[cfg(target_os = "windows")]
use windows_sys::Win32::UI::Shell::{DefSubclassProc, SetWindowSubclass};
#[cfg(target_os = "windows")]
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetWindowRect, IsIconic, IsZoomed, SetWindowPos, MINMAXINFO, SWP_NOMOVE, SWP_NOSIZE, WINDOWPOS,
};
pub(crate) fn browser_args() -> String {
    let mut args = String::from("--renderer-process-limit=1 --disable-breakpad --disable-features=AudioServiceOutOfProcess,TranslateUI,msWebOOUI,msPdfOOUI,msSmartScreenProtection");
    // 调试开关：`PM_DEV_CDP_PORT` 存在时开启 WebView2 远程调试（tools/cdp-eval.mjs
    // 的前置）。✗ 不用 WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS 环境变量：应用
    // 显式设置 additional_browser_args 时该变量不参与 WebView2 命令行（实测无效）
    if std::env::var_os("PM_DEV_CDP_PORT").is_some() {
        args.push_str(" --remote-debugging-port=9222");
    }
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

/// 设置窗口创建后的「DPI 稳定等待」。
///
/// 必须是 **异步** sleep：调用点在 `tauri::async_runtime::spawn` 的 async 块里，
/// 运行于 tokio 工作线程；`std::thread::sleep` 会占死一个执行器线程
/// （worker 数 ≈ CPU 核数），`tokio::time::sleep` 则让出线程、只注册一个定时器。
/// 语义上两者都是「等 200ms 再继续」，故对调用方无差别。
///
/// 抽成具名函数不只是为了可读：**「让出执行器」这条性质因此可被单测证伪**
/// （见 `dpi_settle_wait_yields_the_executor`）——若改回阻塞式 sleep，
/// 同一运行时上的多次等待会串行化，用例即转红。
async fn wait_for_dpi_settle() {
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
}

/// 设置窗标题栏形态判据：Win11+（build ≥ 22000）走**原生标题栏**
/// （decorations(true)）——SystemBackdrop（mica）与原生 NC 区是系统标准组合，
/// min/max/restore 系统动画 / 贴靠 / 窗口菜单全部白拿；Win10 走无框自绘——
/// SWCA 路径无法让原生 NC 区参与磨砂、描边环无开关（判据见 settings.html
/// 头注释与 Wiki 07 §17.1），无框形态 min/max/restore 为瞬时切换（动画不补，
/// 判据见 FramelessProfile）。
/// ✓ `PM_DEV_FORCE_FRAMELESS=1` 在 Win11 本机强制走无框路径：装机分布上
/// Win10 不可复测，用它在 Win11 上做无框路径回归。✗ 只决定「设置窗」形态；
/// popup 恒无框自绘（悬浮面板设计，且无 min/max 需求）。位置：本函数唯一实现。
#[cfg(target_os = "windows")]
pub fn native_titlebar_mode() -> bool {
    if std::env::var_os("PM_DEV_FORCE_FRAMELESS").is_some() {
        return false;
    }
    windows_version::OsVersion::current().build >= 22000
}

fn open_settings_inner(app: &tauri::AppHandle, tab: Option<&str>) {
    if let Some(win) = app.get_webview_window("settings") {
        // 已有窗口的重开路径整体移出调用线程（本函数也由同步命令 open_settings 调用，
        // 同步命令在调用线程上执行；托盘菜单那条路径则由 tao 主循环在主线程派发 ⇒
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
                standard_log!("[material] reopen settings, material={}", material);
                crate::window_material::apply_window_material(hwnd.0 as isize, &material);
            }
            let _ = win.unminimize();
            let _ = win.show();
            let _ = win.set_focus();
            // ✗ 无框专属工具链只许在无框形态重装（判据同创建路径；原生形态
            // strip 会摘掉系统三键、子窗对齐会顶进原生 NC 区）
            // 无框修剪跟在 show/set_focus 之后：tao 在边界事件重写 GWL_STYLE
            // （与 popup.rs show 路径同款时序），FRAMECHANGED 重算经 strip 统一获得
            #[cfg(target_os = "windows")]
            if let Ok(hwnd) = win.hwnd() {
                if !native_titlebar_mode() {
                    strip_sysmenu_style(hwnd.0 as isize, FramelessProfile::Settings);
                    // 子类必须装在窗口所属线程（主线程），见 ensure 的 ✗ 注
                    let h = hwnd.0 as isize;
                    let _ = app.run_on_main_thread(move || {
                        ensure_maximize_workarea_subclass(h);
                    });
                }
            }
            #[cfg(target_os = "windows")]
            if !native_titlebar_mode() {
                spawn_frameless_healer(win.clone(), FramelessProfile::Settings);
            }
        });
        return;
    }
    let app = app.clone();
    // 标题栏形态（判据见 native_titlebar_mode）：形态随创建固化，同时经 URL
    // 参数同步给前端（settings.js 读 location.search 在首帧前移除自绘标题栏，
    // 零闪烁；不走异步 command——会晚于首帧）
    #[cfg(target_os = "windows")]
    let native = native_titlebar_mode();
    #[cfg(not(target_os = "windows"))]
    let native = false;
    let url = match tab {
        Some(t) if native => format!("settings.html?ntb=1#{}", t),
        Some(t) => format!("settings.html#{}", t),
        None if native => "settings.html?ntb=1".to_string(),
        None => "settings.html".to_string(),
    };
    tauri::async_runtime::spawn(async move {
        let mut builder =
            tauri::WebviewWindowBuilder::new(&app, "settings", tauri::WebviewUrl::App(url.into()))
                .title("设置 - 外设监控")
                .inner_size(960.0, 720.0)
                // Win11：原生标题栏 + 原生缩放边框 + 原生阴影（系统动画/贴靠/
                // 窗口菜单白拿）；Win10：无框自绘。✗ 无框分支的判据：
                // 标题带根治依赖 tao 本地 patch（Cargo.toml [patch.crates-io]：
                // 无框窗不产 WS_CAPTION|WS_SYSMENU ⇒ DWM 无 caption 渲染缓存，
                // title 无任何消费点）；resizable(true)+decorations(false) 会让
                // tauri-runtime-wry 创建 TAURI_DRAG_RESIZE_BORDERS 子窗口盖在
                // webview 上吃掉边缘点击（真机实测），缩放改由前端热区经
                // start_resize_dragging 发起
                .decorations(native)
                .shadow(native)
                .resizable(native)
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
                if !native {
                    // 无框形态专属：DWM 圆角偏好（原生形态 DWM 自管，勿干预）
                    set_rounded_corners(hwnd.0 as isize);
                }
                let material = config::with_config(|c| c.window_material.clone());
                crate::window_material::apply_window_material(hwnd.0 as isize, &material);
                crate::webview::ensure_webview_bg_transparent(win.as_ref());
            }
            // 窗口状态插件恢复后即钳制：跨分辨率/DPI 下恢复的物理尺寸可能越界
            clamp_window_to_work_area(&win);
            // 等 DPI 稳定（异步 sleep，见 wait_for_dpi_settle 的说明）
            wait_for_dpi_settle().await;
            // 静置后 DPI 已稳定，再次钳制以兜底首帧缩放未就绪
            clamp_window_to_work_area(&win);
            let _ = win.show();
            let _ = win.set_focus();
            // ✗ 无框专属工具链（修剪/子类/healer）只许在无框形态安装——它们
            // 为「client=window」的无框形态而设：strip 会摘掉原生三键、子窗对
            // 齐按全窗矩形推 webview 会顶进原生 NC 区、zoomed 矫正按无框钳制
            // 语义工作。原生形态（Win11）的系统动画/贴靠/圆角全部原生接管。
            // 无框修剪跟在 show/set_focus 之后：tao 在边界事件重写 GWL_STYLE
            // （与 popup.rs show 路径同款时序），FRAMECHANGED 重算经 strip 统一获得
            #[cfg(target_os = "windows")]
            if let Ok(hwnd) = win.hwnd() {
                if !native {
                    strip_sysmenu_style(hwnd.0 as isize, FramelessProfile::Settings);
                    // 子类必须装在窗口所属线程（主线程），见 ensure 的 ✗ 注
                    let h = hwnd.0 as isize;
                    let _ = app.run_on_main_thread(move || {
                        ensure_maximize_workarea_subclass(h);
                    });
                }
            }
            #[cfg(target_os = "windows")]
            if !native {
                spawn_frameless_healer(win.clone(), FramelessProfile::Settings);
            }
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

/// 读 `SystemUsesLightTheme`（**系统**主题，区别于**应用**主题）。
///
/// ✓ 为什么单开一个函数而不复用 `system_dark_mode()`：那个读的是 `AppsUseLightTheme`
///   （**应用**主题），用于决定 widget **内容**的明暗；而底衬配色按**系统主题**取值
///   （`SystemUsesLightTheme`，见
///   `WindowsThemeDetector.GetWindowsTheme(out appTheme, out systemTheme)`）。
///   两者在「应用深色 + 系统浅色」这类自定义主题下**会不一致**，故分开读。
/// ⚠ 读失败按 `WindowsThemeDetector` 的约定**回落 light**（其源码注释：on error, default to light）。
#[cfg(target_os = "windows")]
pub fn system_uses_light_theme() -> bool {
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
            return true;
        }
        let mut value: u32 = 1;
        let mut size = std::mem::size_of::<u32>() as u32;
        let mut data_type: u32 = REG_DWORD;
        let status = RegQueryValueExW(
            hkey,
            w!("SystemUsesLightTheme"),
            std::ptr::null_mut(),
            &mut data_type,
            &mut value as *mut u32 as *mut u8,
            &mut size,
        );
        RegCloseKey(hkey);
        if status != 0 {
            return true;
        }
        value != 0
    }
}

#[cfg(not(target_os = "windows"))]
pub fn system_uses_light_theme() -> bool {
    true
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

/// 自愈步进的一步结果：healer 线程据此决定线程去留与「是否需要抖动回正」。
pub enum SysmenuHealOutcome {
    /// 窗口已隐藏或句柄失效：线程退出（show 路径会重新拉起）
    WindowGone,
    /// 窗口在场且样式干净：本步零 poke
    Clean,
    /// 窗口在场但脏位刚被摘除：tao 的异步重写被回滚，客户区矩形可能翻转
    Repaired,
}

/// 无框窗口的修剪画像：两窗均已无框（原生标题栏废除，网页自绘），脏位掩码
/// 相同（SYSMENU|CAPTION|THICKFRAME 全摘）。⚠ settings 仅 Win10 处于本画像
/// （Win11 走原生标题栏 = decorated，本画像工具链对其整体停用，判据见
/// native_titlebar_mode）；popup 恒无框。✗ settings 不得为「边缘缩放」
/// 保留 THICKFRAME——保留它 DWM 会在激活切换/帧重算时绘制 NC 框架带（浅色
/// 主题白色，叠在 webview 外沿 = 左/右/下白框；NCRP_DISABLED 只在部分合成
/// 趟生效、激活切换与样式重算会无视它，真机实测）。缩放改由前端边缘热区经
/// `start_resize_dragging` 发起（settings.js initTitleBar 装配，ACL
/// core:window:allow-start-resize-dragging），且 builder 必须
/// resizable(false)：resizable(true)+decorations(false) 会让 tauri-runtime-wry
/// 创建 TAURI_DRAG_RESIZE_BORDERS 子窗口盖住边缘吃掉点击（真机实测）。
/// ✗ 不得为「系统最小化/最大化动画」保留 CAPTION 位：位一旦出现 DWM 即画
/// 系统标题 band + 三键且**事后摘位不消**（创建期缓存 + 事后补位单向上锁，
/// NCRP_DISABLED 在 26300 已移除返回 E_INVALIDARG，2026-11 实测否决，
/// → Wiki 07 §17.10）。动画不做补齐：Win10 无框态 min/max/restore 为瞬时
/// 切换（历史上一直如此，接受）；系统动画只由 Win11 原生标题栏形态自带
/// （判据见 native_titlebar_mode）。
#[derive(Clone, Copy)]
pub enum FramelessProfile {
    Popup,
    Settings,
}

impl FramelessProfile {
    /// 该画像下视为「脏」的样式位掩码：任一位被 tao 的样式重写写回即触发修剪
    fn dirty_mask(self) -> i32 {
        const WS_SYSMENU: i32 = 0x0008_0000;
        const WS_CAPTION: i32 = 0x00C0_0000;
        const WS_THICKFRAME: i32 = 0x0004_0000;
        match self {
            // ✗ Popup 不设脏位：Win11 fork 为 topmost 无框窗**有意保留**
            // CAPTION|SYSMENU（SystemBackdrop 只认创建期样式，→ Wiki 07 §17.6），
            // Win10 fork 才不产位——掩码含位 ⇒ healer 每 250ms 摘位与 fork 拉锯：
            // 样式位中途翻转诱发 DWM 重绘系统三键 + FRAMECHANGED 边框抖动
            // （真机探针实测：开窗 151ms 摘位、166ms 宽度 +1px 抖动）。
            FramelessProfile::Popup => 0,
            FramelessProfile::Settings => WS_SYSMENU | WS_CAPTION | WS_THICKFRAME,
        }
    }
}

/// 无框窗口自愈单步：读回 GWL_STYLE——窗口隐藏则返回 WindowGone（值守线程
/// 退出）；画像脏位任一被 tao 的样式重写写回（show/always_on_top/set_focus/
/// DPI 等边界事件触发；Win10 DWM 在 accent 背景板下会为它们绘制原生三键并
/// 压平 WebView2 子视觉 alpha）则修剪并返回 Repaired；可见且干净返回 Clean。
/// ✗ 只许对无框窗口调用。位置：spawn_frameless_healer 调用，本函数唯一实现。
#[cfg(target_os = "windows")]
pub fn sysmenu_heal_step(hwnd: isize, profile: FramelessProfile) -> SysmenuHealOutcome {
    unsafe {
        const GWL_STYLE: i32 = -16;
        const WS_VISIBLE: i32 = 0x1000_0000;
        let style = windows_sys::Win32::UI::WindowsAndMessaging::GetWindowLongPtrW(
            hwnd as *mut core::ffi::c_void,
            GWL_STYLE,
        ) as i32;
        if (style & WS_VISIBLE) == 0 {
            return SysmenuHealOutcome::WindowGone;
        }
        if (style & profile.dirty_mask()) != 0 {
            strip_sysmenu_style(hwnd, profile);
            return SysmenuHealOutcome::Repaired;
        }
        SysmenuHealOutcome::Clean
    }
}

/// 无框窗口的样式修剪：把 GWL_STYLE 恢复成 tao 创建期的无框形态（摘
/// profile.dirty_mask()；settings 为 SYSMENU|CAPTION|THICKFRAME 三位，
/// popup 掩码为 0 不修剪——见 dirty_mask）。tao 的无框修正只在创建期生效，
/// 样式重写会把位写回。位置：sysmenu_heal_step 与 open_settings_inner 的
/// show 后修剪调用，本函数唯一实现。
#[cfg(target_os = "windows")]
pub fn strip_sysmenu_style(hwnd: isize, profile: FramelessProfile) {
    unsafe {
        const GWL_STYLE: i32 = -16;
        const SWP_NOSIZE: u32 = 1;
        const SWP_NOMOVE: u32 = 2;
        const SWP_NOZORDER: u32 = 4;
        const SWP_FRAMECHANGED: u32 = 0x20;
        let style = windows_sys::Win32::UI::WindowsAndMessaging::GetWindowLongPtrW(
            hwnd as *mut core::ffi::c_void,
            GWL_STYLE,
        ) as i32;
        windows_sys::Win32::UI::WindowsAndMessaging::SetWindowLongPtrW(
            hwnd as *mut core::ffi::c_void,
            GWL_STYLE,
            (style & !profile.dirty_mask()) as isize,
        );
        // SetWindowLongPtr 只改样式值，渲染帧不会自动重算：必须补一次
        // SWP_FRAMECHANGED 让 DWM 按新样式重算边框，否则带框渲染条滞留——
        // popup 顶部透出系统标题栏（真机实测，strip 与 poke 必须成对）。
        // 位置：所有 strip 调用点（show / animate_open / create / heal）经本函数
        // 统一获得即时重算。
        windows_sys::Win32::UI::WindowsAndMessaging::SetWindowPos(
            hwnd as *mut core::ffi::c_void,
            core::ptr::null_mut(),
            0,
            0,
            0,
            0,
            SWP_NOSIZE | SWP_NOMOVE | SWP_NOZORDER | SWP_FRAMECHANGED,
        );
    }
}

/// webview 子窗口对齐：把 popup 的全部子窗口（WebView2 层级）对齐到**全窗
/// 矩形**（映射到客户坐标，可带负 x/y 以覆盖客户原点内缩带）。tao 的样式位
/// 拉锯会让客户区矩形在「全窗」与「内缩」两态间翻转，子窗口停留在旧位置
/// ⇒ 窗口边缘露出白边、webview 内容错位成左右两条。返回是否真的修了东西
/// （客户区被强制重算或子窗口被挪动），healer 据此决定是否抖动回正。
/// ✓ SWP_FRAMECHANGED 强制重算**只许在客户区实测 stale 时执行**（原点/尺寸
/// 与全窗不符）：样式位回滚后的帧重算由 strip_sysmenu_style 自带 poke 负责，
/// 不在本函数重复。两条件皆否时零 poke——无条件 poke + 无条件抖动 = 每
/// 250ms 一次的 1px 边框抖动（真机实测肉眼可见）。位置：popup.rs 自愈线程
/// 调用，本函数唯一实现。
#[cfg(target_os = "windows")]
pub fn fix_webview_child_bounds(top: isize) -> bool {
    use core::sync::atomic::{AtomicBool, AtomicI32, Ordering};
    use windows_sys::Win32::Foundation::{POINT, RECT};
    use windows_sys::Win32::Graphics::Gdi::{
        ClientToScreen, GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        EnumChildWindows, GetClientRect, GetWindowRect, IsIconic, IsZoomed, SetWindowPos,
    };
    const SWP_NOSIZE: u32 = 1;
    const SWP_NOMOVE: u32 = 2;
    const SWP_NOZORDER: u32 = 4;
    const SWP_FRAMECHANGED: u32 = 0x20;
    unsafe {
        let top_h = top as *mut core::ffi::c_void;
        let mut wr = RECT {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        };
        let _ = GetWindowRect(top_h, &mut wr);
        // ✗ 最大化态必须以「最近显示器工作区」为唯一目标矩形：tao set_maximized
        // 落到工作区后 ~7ms 内有一次样式重写（CAPTION|SYSMENU 写入 + FRAMECHANGED），
        // Windows 对带边框样式的 zoomed 窗内部重排出「原生过扫描最大化矩形」
        // （真机 toggleMaximize 探针：2578x1398@-9,-9，四周 ~9px 出屏；DWM 帧边界
        // 同值=物理生效）——该重排不经过带实值的 WINDOWPOSCHANGING / MINMAXINFO，
        // 钳制层拦不到 ⇒ 只能在这里每轮纠正：顶层窗物理钳回工作区 + 子窗对齐
        // 目标=工作区（GetWindowRect/GetClientRect/DWM 帧边界此刻全部按幽灵矩形
        // 回报，一律不可采信）。非最大化路径零变化。位置：本函数唯一实现。
        let zoomed = IsZoomed(top_h) != 0 && IsIconic(top_h) == 0;
        let mut zoom_fixed = false;
        if zoomed {
            let hmon = MonitorFromWindow(top_h, MONITOR_DEFAULTTONEAREST);
            let mut mi: MONITORINFO = core::mem::zeroed();
            mi.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
            if GetMonitorInfoW(hmon, &mut mi) != 0 {
                let work_w = mi.rcWork.right - mi.rcWork.left;
                let work_h = mi.rcWork.bottom - mi.rcWork.top;
                if wr.left != mi.rcWork.left
                    || wr.top != mi.rcWork.top
                    || (wr.right - wr.left) != work_w
                    || (wr.bottom - wr.top) != work_h
                {
                    let _ = SetWindowPos(
                        top_h,
                        core::ptr::null_mut(),
                        mi.rcWork.left,
                        mi.rcWork.top,
                        work_w,
                        work_h,
                        SWP_NOZORDER,
                    );
                    zoom_fixed = true;
                }
                // 目标矩形一律取工作区（不采信任何回报矩形）
                wr = mi.rcWork;
            }
        }
        let mut origin = POINT { x: 0, y: 0 };
        let _ = ClientToScreen(top_h, &mut origin);
        let mut cr = RECT {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        };
        let _ = GetClientRect(top_h, &mut cr);
        // 客户区 stale 探测：无框窗修剪后客户区必须与全窗重合（原点重合 + 尺寸
        // 相等）。样式位拉锯会把客户区滞留在「内缩」态（框架 7px 未被 tao 的
        // NCCALCSIZE 处理摘掉）⇒ wry 每次按客户区推子窗口 bounds，与全窗目标
        // 永久错位。SWP_FRAMECHANGED 让 tao 的处理器重跑，客户区恢复全窗。
        // ✗ 最大化态跳过本探测：幽灵矩形下 client 与全窗「一致地」错（探测判假
        // 恒不触发），而样式中间态下 client 瞬时内缩、poke 反而给重排添乱——
        // 目标已由工作区接管，探测无意义。
        let client_stale = !zoomed
            && (origin.x != wr.left
                || origin.y != wr.top
                || (cr.right - cr.left) != (wr.right - wr.left)
                || (cr.bottom - cr.top) != (wr.bottom - wr.top));
        let fixed_any = client_stale || zoom_fixed;
        if client_stale {
            let _ = SetWindowPos(
                top_h,
                core::ptr::null_mut(),
                0,
                0,
                0,
                0,
                SWP_NOSIZE | SWP_NOMOVE | SWP_NOZORDER | SWP_FRAMECHANGED,
            );
            // 重算后重读窗口矩形与客户原点：目标矩形必须基于修好的客户区
            let _ = GetWindowRect(top_h, &mut wr);
            let _ = ClientToScreen(top_h, &mut origin);
        }
        // 目标矩形（客户坐标）与客户原点经静态槽传给枚举回调（回调无法捕获局部变量）
        static TX: AtomicI32 = AtomicI32::new(0);
        static TY: AtomicI32 = AtomicI32::new(0);
        static TW: AtomicI32 = AtomicI32::new(0);
        static TH: AtomicI32 = AtomicI32::new(0);
        static OX: AtomicI32 = AtomicI32::new(0);
        static OY: AtomicI32 = AtomicI32::new(0);
        static MOVED: AtomicBool = AtomicBool::new(false);
        MOVED.store(false, Ordering::Relaxed);
        unsafe extern "system" fn enum_cb(h: *mut core::ffi::c_void, _: isize) -> i32 {
            let tx = TX.load(Ordering::Relaxed);
            let ty = TY.load(Ordering::Relaxed);
            let tw = TW.load(Ordering::Relaxed);
            let th = TH.load(Ordering::Relaxed);
            let mut r = RECT {
                left: 0,
                top: 0,
                right: 0,
                bottom: 0,
            };
            let _ = GetWindowRect(h, &mut r);
            // GetWindowRect 是屏幕坐标，目标 TX/TY 是客户坐标：必须先换算再比较，
            // ✗ 直接比较会在「窗口不在屏幕原点」时恒判错位 ⇒ 每 tick 白挪一次
            // 子窗口 + healer 抖动自持（真机实测：popup 恒在屏幕 (1150,291)）
            let cl = r.left - OX.load(Ordering::Relaxed);
            let ct = r.top - OY.load(Ordering::Relaxed);
            // 尺寸容差 ±1px：Chromium 内层（Chrome_RenderWidgetHostHWND）尺寸由
            // Chromium 自管，分数 DPI 缩放下宽度与窗口恒差 1px；为它挪动会与
            // wry 的重推形成自持振荡（healer 挪回 → Chromium 推开 → 抖动回正
            // → 循环，Win11 26300 真机实测窗口边缘持续抖动）。原缺陷是样式翻转
            // 后的整带错位（数 px 级），1px 差异肉眼不可见，不值得 poke。
            const SIZE_TOL: u32 = 1;
            if cl != tx
                || ct != ty
                || (r.right - r.left).abs_diff(tw) > SIZE_TOL
                || (r.bottom - r.top).abs_diff(th) > SIZE_TOL
            {
                let _ = SetWindowPos(h, core::ptr::null_mut(), tx, ty, tw, th, SWP_NOZORDER);
                MOVED.store(true, Ordering::Relaxed);
            }
            1
        }
        TX.store(wr.left - origin.x, Ordering::Relaxed);
        TY.store(wr.top - origin.y, Ordering::Relaxed);
        TW.store(wr.right - wr.left, Ordering::Relaxed);
        TH.store(wr.bottom - wr.top, Ordering::Relaxed);
        OX.store(origin.x, Ordering::Relaxed);
        OY.store(origin.y, Ordering::Relaxed);
        EnumChildWindows(top_h, Some(enum_cb), 0isize);
        fixed_any || MOVED.load(Ordering::Relaxed)
    }
}

// ── 最大化钳到工作区（settings 无框窗） ─────────────────────

static MAX_SUBCLASS_HWND: core::sync::atomic::AtomicIsize = core::sync::atomic::AtomicIsize::new(0);

/// 最大化态几何闸口的共用判定：zoomed ∧ 非图标态 ∧ rect 任一边 ≥ 最近显示器
/// 整屏 ⇒ Some(该显示器工作区)。图标矩形是特殊坐标（钳它 = 6448e87 缩条
/// 复发）；还原矩形远小于整屏天然豁免。判据全文见 maximize_workarea_proc
/// 两分支注释与 Wiki 07 §17.9。位置：本模块唯一实现。
#[cfg(target_os = "windows")]
fn zoomed_workarea_clamp(hwnd: HWND, rect: &RECT) -> Option<RECT> {
    unsafe {
        if IsZoomed(hwnd) == 0 || IsIconic(hwnd) != 0 {
            return None;
        }
        let hmon = MonitorFromRect(rect, MONITOR_DEFAULTTONEAREST);
        let mut mi: MONITORINFO = core::mem::zeroed();
        mi.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        if GetMonitorInfoW(hmon, &mut mi) == 0 {
            return None;
        }
        let mon_w = mi.rcMonitor.right - mi.rcMonitor.left;
        let mon_h = mi.rcMonitor.bottom - mi.rcMonitor.top;
        if (rect.right - rect.left) >= mon_w || (rect.bottom - rect.top) >= mon_h {
            Some(mi.rcWork)
        } else {
            None
        }
    }
}

/// 「系统最大化钳到工作区」子类过程：先让默认链（tao 的 min/max 约束 +
/// DefWindowProc）填充 MINMAXINFO，再覆写 ptMaxSize/ptMaxPosition 为最近
/// 显示器工作区。判据：无框窗系统最大化的窗口矩形默认=整屏，DWM 判定
/// 全屏窗口后 explorer 隐藏任务栏（真机实测，客户区钳制救不了——任务栏
/// 看的是窗口矩形）；覆写后系统最大化（含 Win+Up 快捷键）的窗口矩形=
/// 工作区，任务栏保持可见。另承担最大化态的圆角摘除（见 WM_SIZE 分支）与
/// 幽灵过扫描矩形的两层钳制（见 WINDOWPOSCHANGING / WINDOWPOSCHANGED 分支）。
#[cfg(target_os = "windows")]
/// 圆角偏好翻转（最大化态 DONOTROUND / 浮动态 ROUND），APPLIED 缓存防高频
/// 重复调 DWM（拖缘缩放期每条 WM_SIZE）。位置：本函数唯一实现。
#[cfg(target_os = "windows")]
fn corner_pref_apply(hwnd: isize, maximized: bool) {
    static APPLIED: core::sync::atomic::AtomicI32 = core::sync::atomic::AtomicI32::new(0);
    const DWMWA_WINDOW_CORNER_PREFERENCE: u32 = 33;
    const DWMWCP_DONOTROUND: u32 = 1;
    let want = if maximized { 1 } else { 0 };
    if APPLIED.load(core::sync::atomic::Ordering::Relaxed) != want {
        unsafe {
            if maximized {
                let pref = DWMWCP_DONOTROUND;
                windows_sys::Win32::Graphics::Dwm::DwmSetWindowAttribute(
                    hwnd as HWND,
                    DWMWA_WINDOW_CORNER_PREFERENCE,
                    &pref as *const _ as *const _,
                    std::mem::size_of::<u32>() as u32,
                );
            } else {
                set_rounded_corners(hwnd);
            }
        }
        APPLIED.store(want, core::sync::atomic::Ordering::Relaxed);
    }
}

unsafe extern "system" fn maximize_workarea_proc(
    hwnd: HWND,
    umsg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _uidsubclass: usize,
    _dwrefdata: usize,
) -> LRESULT {
    const WM_GETMINMAXINFO: u32 = 0x0024;
    const WM_SIZE: u32 = 0x0005;
    const WM_WINDOWPOSCHANGING: u32 = 0x0046;
    const WM_WINDOWPOSCHANGED: u32 = 0x0047;
    const SWP_NOZORDER: u32 = 4;
    if umsg == WM_WINDOWPOSCHANGING {
        // ✗ 最大化态的几何落点必须在最后闸口钳回工作区：MINMAXINFO 只拦
        // 「查询式」最大化（窗口尚无 WS_MAXIMIZE 位的首次 ShowWindow），tao
        // set_maximized 是先写 WS_MAXIMIZE 样式位再 ShowWindow——位已置 ⇒
        // 系统不再按 MINMAXINFO 查询 ⇒ 落点=原生过扫描矩形（真机
        // toggleMaximize 探针：2578x1398@-9,-9，四周 ~9px 出屏 = 「窗口放大
        // 一圈、内容被裁一圈」）。WINDOWPOSCHANGING 是 ShowWindow /
        // SetWindowPos 一切几何路径的共同前哨，写回 x/y/cx/cy 即终值。
        // NOSIZE|NOMOVE 的帧重算提案不拦（改了也不生效）。
        let wp = &mut *(lparam as *mut WINDOWPOS);
        if (wp.flags & (SWP_NOSIZE | SWP_NOMOVE)) == 0 {
            let proposed = RECT {
                left: wp.x,
                top: wp.y,
                right: wp.x + wp.cx,
                bottom: wp.y + wp.cy,
            };
            if let Some(work) = zoomed_workarea_clamp(hwnd, &proposed) {
                wp.x = work.left;
                wp.y = work.top;
                wp.cx = work.right - work.left;
                wp.cy = work.bottom - work.top;
            }
        }
    }
    if umsg == WM_WINDOWPOSCHANGED {
        // ✗ 幽灵矩形必须在本条消息内即刻纠正：样式重写（CAPTION|SYSMENU +
        // FRAMECHANGED）触发的内部重排把 zoomed 窗推到过扫描矩形，且不经过
        // 带实值的 WINDOWPOSCHANGING / MINMAXINFO（钳制层拦不到）——healer
        // 下一轮（≤250ms）才纠正 ⇒ 十几帧可见跳变。WM_WINDOWPOSCHANGED 在
        // 重排落地的同一次 SetWindowPos 分发内到达，此处同步钳回 = 重排与
        // 纠正发生在 DWM 合成之前 ⇒ 用户不可见。豁免判据与 CHANGING 分支
        // 同一份（zoomed_workarea_clamp：图标态跳过防缩条复发、还原矩形
        // 远小于整屏天然豁免）；自纠正重入时矩形=工作区 → 判等零动作。
        // 位置：本子类唯一实现。
        let mut wr = RECT {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        };
        let _ = GetWindowRect(hwnd, &mut wr);
        if let Some(work) = zoomed_workarea_clamp(hwnd, &wr) {
            if wr.left != work.left
                || wr.top != work.top
                || (wr.right - wr.left) != (work.right - work.left)
                || (wr.bottom - wr.top) != (work.bottom - work.top)
            {
                let _ = SetWindowPos(
                    hwnd,
                    core::ptr::null_mut(),
                    work.left,
                    work.top,
                    work.right - work.left,
                    work.bottom - work.top,
                    SWP_NOZORDER,
                );
            }
        }
    }
    if umsg == WM_GETMINMAXINFO {
        let result = DefSubclassProc(hwnd, umsg, wparam, lparam);
        let mmi = &mut *(lparam as *mut MINMAXINFO);
        let mut mi: MONITORINFO = core::mem::zeroed();
        mi.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        let hmon = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
        if GetMonitorInfoW(hmon, &mut mi) != 0 {
            mmi.ptMaxSize.x = mi.rcWork.right - mi.rcWork.left;
            mmi.ptMaxSize.y = mi.rcWork.bottom - mi.rcWork.top;
            mmi.ptMaxPosition.x = mi.rcWork.left;
            mmi.ptMaxPosition.y = mi.rcWork.top;
            mmi.ptMaxTrackSize = mmi.ptMaxSize;
        }
        return result;
    }
    if umsg == WM_SIZE {
        // ✗ 最大化态必须摘圆角（DONOTROUND），还原恢复 ROUND：最大化矩形被
        // 钳到工作区（不满整屏），DWM 不认定最大化 ⇒ 按浮动窗绘制圆角 +
        // 边框圈——弧线裁掉页面四角、暗圈压住四边（真机实测：pref=ROUND
        // 时四角 ~9px 弧外露背景，DONOTROUND 后直角到位、暗圈消失）。
        // 还原路径必须复用 set_rounded_corners（与创建期同一来源）；Win10
        // 无 attribute 33，调用失败无害。位置：corner_pref_apply 唯一实现。
        let maximized = windows_sys::Win32::UI::WindowsAndMessaging::IsZoomed(hwnd) != 0;
        corner_pref_apply(hwnd as isize, maximized);
    }
    DefSubclassProc(hwnd, umsg, wparam, lparam)
}

/// 给窗口安装最大化钳制子类（见 maximize_workarea_proc 的判据）。
/// ✗ SetWindowSubclass 与窗口子类链按线程绑定，必须经 run_on_main_thread
/// 在窗口所属线程安装；同 hwnd 幂等（重开路径不重复装）。
#[cfg(target_os = "windows")]
pub fn ensure_maximize_workarea_subclass(hwnd: isize) {
    use core::sync::atomic::Ordering;
    if MAX_SUBCLASS_HWND.load(Ordering::Acquire) == hwnd {
        return;
    }
    let ok = unsafe {
        SetWindowSubclass(
            hwnd as *mut core::ffi::c_void,
            Some(maximize_workarea_proc),
            0x4D58, // 子类 id（'MX'），窗口内任意唯一即可
            0,
        )
    };
    if ok != 0 {
        MAX_SUBCLASS_HWND.store(hwnd, Ordering::Release);
        standard_log!("[window] maximize workarea subclass installed");
    } else {
        standard_log!("[window] maximize workarea subclass install failed");
    }
}

/// 每画像一个换代计数器：新拉起 healer 时 +1，旧线程发现代数不符即在下一
/// 个 250ms 周期退出。settings 可见期间被重开（托盘反复点「设置」）会重复
/// 调 spawn——纯 AtomicBool 存活标志有「旧线程恰在退出、新 spawn 被跳过」
/// 的竞态窗口，换代计数让旧线程必然让位、新线程必然在场。
fn healer_generation(profile: FramelessProfile) -> &'static core::sync::atomic::AtomicU64 {
    use core::sync::atomic::AtomicU64;
    static POPUP: AtomicU64 = AtomicU64::new(0);
    static SETTINGS: AtomicU64 = AtomicU64::new(0);
    match profile {
        FramelessProfile::Popup => &POPUP,
        FramelessProfile::Settings => &SETTINGS,
    }
}

/// 无框窗口自愈线程（popup / settings 共用）：tao 在 show/hide/focus/DPI 等
/// **边界事件**会重写 GWL_STYLE（画像脏位写回 = 三键重现 + 客户区矩形翻转
/// ⇒ WebView2 视觉与窗口错位），且翻转时窗口矩形会被带框样式撑大
/// （AdjustWindowRectEx 按带框样式补框架一圈 = 「窗口大一圈」）。
/// 窗口可见期间每 250ms 值守一步：样式读回脏了才修；子窗口偏了才对齐。
/// 窗口隐藏（WS_VISIBLE 清零）线程退出，show / 重开路径重新拉起（换代计数
/// 保证同画像同时只有一个值守线程）。
///
/// ✓ 尾部的「+1px 抖动回正」只许在**本步真的修过东西**（样式位被回滚或
/// 子窗口被挪正）时执行：同尺寸的 SetWindowPos 不触发 WM_SIZE，wry 就不会
/// 重推 WebView2 控制器 bounds——样式位翻转后控制器 bounds 停在旧客户区
/// 原点，视觉整体错位（洋红覆盖层只出现在窗口右侧一段），+1px 触发一次
/// 真实 WM_SIZE 让 wry 重推，60ms 后回正。✗ 干净时零 poke——无条件抖动 =
/// 每 250ms 一次的 1px 边框抖动，真机实测肉眼可见。
/// 基线尺寸动态读 `outer_size()`（物理像素）：settings 可缩放，固定值会与
/// 用户拉锯；✗ 最大化态跳过抖动——对最大化窗口 set_size 会破坏最大化形态。
#[cfg(target_os = "windows")]
pub fn spawn_frameless_healer(window: tauri::WebviewWindow, profile: FramelessProfile) {
    let gen = healer_generation(profile).fetch_add(1, core::sync::atomic::Ordering::Relaxed) + 1;
    std::thread::spawn(move || {
        loop {
            std::thread::sleep(std::time::Duration::from_millis(250));
            if healer_generation(profile).load(core::sync::atomic::Ordering::Relaxed) != gen {
                return;
            }
            let Ok(hwnd) = window.hwnd() else { return };
            // ✗ 最小化态整轮跳过（含 heal step 与子窗对齐）：最小化窗口的
            // GetWindowRect / outer_size 塌缩成图标矩形（Win11 实测 199x34），
            // 客户端 stale 探测恒真 ⇒ poke + 「宽 +1px 抖动回正」在图标态执行，
            // base 取到图标尺寸 ⇒ 窗口被永久缩成一条三键窄条（真机实测：
            // 最小化↔恢复快切 6 轮后卡死 199x34）；恢复后的下一轮（≤250ms）
            // 自然补上对齐。位置：healer 主循环唯一实现点。
            if window.is_minimized().unwrap_or(false) {
                continue;
            }
            let dirty = match sysmenu_heal_step(hwnd.0 as isize, profile) {
                SysmenuHealOutcome::WindowGone => return,
                SysmenuHealOutcome::Clean => false,
                SysmenuHealOutcome::Repaired => true,
            };
            let child_fixed = fix_webview_child_bounds(hwnd.0 as isize);
            if !dirty && !child_fixed {
                continue;
            }
            if window.is_maximized().unwrap_or(true) {
                continue;
            }
            // 窗口矩形回正：宽 +1px 触发一次真实 WM_SIZE 让 wry 重推 WebView2
            // 控制器 bounds，60ms 后比对基线回正；基线取抖动前的 outer_size()
            let Ok(base) = window.outer_size() else {
                return;
            };
            let _ = window.set_size(tauri::PhysicalSize::new(base.width + 1, base.height));
            std::thread::sleep(std::time::Duration::from_millis(60));
            if let Ok(now) = window.outer_size() {
                if now.width != base.width || now.height != base.height {
                    let _ = window.set_size(tauri::PhysicalSize::new(base.width, base.height));
                }
            }
        }
    });
}

// ═══════════════════════════════════════════════════════════════
// Toast 通知图标
// ═══════════════════════════════════════════════════════════════

/// 编译时嵌入的 toast 通知图标（`dist/icon.png`）。
/// 避免运行时文件系统路径歧义（Tauri 2 的 `frontendDist` 嵌入二进制，`resources` 部署到子目录）。
static TOAST_ICON_PNG: &[u8] = include_bytes!("../dist/icon.png");

/// 已写入临时目录的图标路径缓存（进程内只写一次）。
///
/// ⚠ 必须保留 `#[cfg(target_os = "windows")]`：非 Windows 下只有下面那个返回 `None`
/// 的桩函数，缓存本体不存在。
#[cfg(target_os = "windows")]
static TOAST_ICON_PATH: std::sync::OnceLock<Option<std::path::PathBuf>> =
    std::sync::OnceLock::new();

/// 将嵌入的图标写入临时目录 `PeriTray_toast_icon.png`，返回路径供 WinRT toast 使用。
///
/// WinRT `file:///` URI 要求绝对路径且无 `\\?\` 前缀，
/// 因此每次写入固定文件名而非使用 `canonicalize`。
/// 写入临时目录而非 exe 目录（MSIX 包目录只读）。
///
/// 结果用 `OnceLock` 缓存：图标内容编译期就已固定，重复写盘既无意义，又会让
/// `%TEMP%\PeriTray_toast_icon.png` 的 mtime 每次弹通知都变（不利于排查「图标为何
/// 不更新」这类问题）。**失败结果同样缓存**：临时目录不可写属于环境问题、不会自愈，
/// 没必要每次弹通知都重试一遍磁盘 I/O。
#[cfg(target_os = "windows")]
pub fn resolve_toast_icon() -> Option<std::path::PathBuf> {
    TOAST_ICON_PATH
        .get_or_init(|| {
            let target = std::env::temp_dir().join("PeriTray_toast_icon.png");
            std::fs::write(&target, TOAST_ICON_PNG).ok()?;
            Some(target)
        })
        .clone()
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

/// `kernel32!GetCurrentPackageFamilyName` 的签名。
#[cfg(target_os = "windows")]
type GetCurrentPackageFamilyNameFn = unsafe extern "system" fn(*mut u32, *mut u16) -> i32;

/// 取 `kernel32!GetCurrentPackageFamilyName` 的函数指针；系统过旧时返回 `None`。
///
/// 走 `GetProcAddress` 动态取址而非静态绑定：该 API 自 Win8 才引入，且本仓的
/// `windows` crate 未启用其所在 feature。抽成独立函数是为了让
/// [`is_msix_context`] 与 [`package_family_name`] 共用同一份 `transmute` 样板。
#[cfg(target_os = "windows")]
fn get_current_package_family_name_fn() -> Option<GetCurrentPackageFamilyNameFn> {
    let fn_ptr = unsafe {
        windows::Win32::System::LibraryLoader::GetProcAddress(
            windows::Win32::System::LibraryLoader::GetModuleHandleW(windows::core::w!(
                "kernel32.dll"
            ))
            .unwrap_or_default(),
            windows::core::s!("GetCurrentPackageFamilyName"),
        )
    }?;
    // SAFETY：`fn_ptr` 来自 kernel32 中确知签名的导出符号。
    // 类型标注必须写在**绑定**上（而非 `transmute::<_, T>`）：clippy 的
    // `missing_transmute_annotations` 只认这种形态。
    let get_cur_pkg: GetCurrentPackageFamilyNameFn = unsafe { std::mem::transmute(fn_ptr) };
    Some(get_cur_pkg)
}

/// 检测当前是否运行在 MSIX 包上下文中。
/// 通过 kernel32!GetCurrentPackageFamilyName 判断：返回
/// ERROR_SUCCESS 或 ERROR_INSUFFICIENT_BUFFER 即为 MSIX 上下文。
#[cfg(target_os = "windows")]
pub(crate) fn is_msix_context() -> bool {
    let Some(get_cur_pkg) = get_current_package_family_name_fn() else {
        return false;
    };
    let mut buf_len: u32 = 0;
    let status = unsafe { get_cur_pkg(&mut buf_len, std::ptr::null_mut()) };
    // ERROR_SUCCESS(0) 或 ERROR_INSUFFICIENT_BUFFER(122) 均表示在包上下文中
    status == 0 || status == 122
}

/// 当前进程的 MSIX 包家族名（Package Family Name）；不在包上下文中则返回 `None`。
///
/// **结果必须缓存**：调用方 [`crate::process::writable_root`] 会被 `log_path()`
/// 在**每一批日志落盘**时经过，不缓存就是每批一次
/// `GetModuleHandleW` + `GetProcAddress` + `GetCurrentPackageFamilyName`。
///
/// 用纯 Win32 `GetCurrentPackageFamilyName` 而非 WinRT
/// `ApplicationModel::Package::Current()`：前者对 COM/WinRT 初始化**零要求**，
/// 可在 `run_blocking` 的任意线程上安全调用（WinRT 静态方法要求线程已 `RoInitialize`），
/// 且本仓 `windows` crate 未启用 `Storage` feature。
#[cfg(target_os = "windows")]
pub(crate) fn package_family_name() -> Option<&'static str> {
    static PFN: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    PFN.get_or_init(|| {
        let get_cur_pkg = get_current_package_family_name_fn()?;
        // 第一次调用只为探长度：传空缓冲，成功时返回 ERROR_INSUFFICIENT_BUFFER(122)，
        // 并把所需长度（**含结尾 NUL**）写进 buf_len。
        let mut buf_len: u32 = 0;
        let status = unsafe { get_cur_pkg(&mut buf_len, std::ptr::null_mut()) };
        // 实测（Win11 26100，**真实 MSIX 包身份下**）：首次以空缓冲调用返回
        // ERROR_INSUFFICIENT_BUFFER(122)，并把所需长度写进 buf_len。
        // 这里**同时接受 ERROR_SUCCESS(0)**，与 [`is_msix_context`] 的判据保持同源——
        // 两者对「是否在包上下文中」必须一致，否则会出现「判在包内、却拿不到 PFN」
        // 从而静默退回只读目录的分裂。
        if (status != 122 && status != 0) || buf_len == 0 {
            return None;
        }
        let mut buf: Vec<u16> = vec![0; buf_len as usize];
        let status = unsafe { get_cur_pkg(&mut buf_len, buf.as_mut_ptr()) };
        if status != 0 {
            return None;
        }
        // 取到第一个 NUL 为止，不依赖 `buf_len` 的返回值
        let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        String::from_utf16(&buf[..len]).ok()
    })
    .as_deref()
}

/// 开始菜单 Programs 目录下的快捷方式路径
/// （`%APPDATA%\Microsoft\Windows\Start Menu\Programs\PeriTray.lnk`，
/// 与脚本里 `[Environment]::GetFolderPath('Programs')` 的结果一致）。
#[cfg(target_os = "windows")]
fn start_menu_shortcut_path() -> Option<std::path::PathBuf> {
    let appdata = std::env::var_os("APPDATA")?;
    Some(
        std::path::Path::new(&appdata)
            .join("Microsoft")
            .join("Windows")
            .join("Start Menu")
            .join("Programs")
            .join("PeriTray.lnk"),
    )
}

/// 判断已存在的快捷方式是否仍指向当前 exe 且带当前 AUMID。
///
/// 只判「文件存在」是不够的：用户换过安装目录后，旧 `.lnk` 会指向已删除的文件，
/// 此时跳过会**永久**留下一个坏掉的开始菜单项，通知图标也一并丢失。
/// 判据取「`.lnk` 字节中同时出现当前 exe 路径与 AUMID」——两者都是 ASCII 字面量，
/// 由 `WScript.Shell` 写入时原样落盘。该判据只会**假阴性**（匹配不到 → 退化为重建，无害），
/// 不会假阳性（不可能把坏快捷方式误判成最新）。
#[cfg(target_os = "windows")]
fn shortcut_is_current(lnk: &std::path::Path, exe_path: &std::path::Path) -> bool {
    let Ok(bytes) = std::fs::read(lnk) else {
        return false;
    };
    let has =
        |needle: &[u8]| !needle.is_empty() && bytes.windows(needle.len()).any(|w| w == needle);
    has(exe_path.to_string_lossy().as_bytes()) && has(AUMID.as_bytes())
}

/// 注册 AUMID 到开始菜单快捷方式，使 Windows 通知显示应用图标。
/// MSIX 包自带 AUMID，无需创建快捷方式；仅 NSIS 安装需要。
/// 已存在且仍指向当前 exe / 当前 AUMID 的快捷方式时跳过（幂等）。
#[cfg(target_os = "windows")]
pub fn register_aumid() {
    if is_msix_context() {
        process::append_verbose_log("[aumid] MSIX context, skipping shortcut creation");
        return;
    }
    use std::os::windows::process::CommandExt;
    use std::process::Command;

    let Ok(exe_path) = std::env::current_exe() else {
        process::append_verbose_log("[aumid] failed to get exe path");
        return;
    };

    // 幂等快速路径：快捷方式已存在且仍指向当前 exe 时直接返回。
    // 价值在于**避免每次启动都拉起一次 PowerShell**（冷启动 300ms~1.5s），
    // 与下方「移出启动关键路径」互补：前者省掉进程创建，后者兜住首次启动的开销。
    if let Some(lnk) = start_menu_shortcut_path() {
        if lnk.exists() && shortcut_is_current(&lnk, &exe_path) {
            process::append_verbose_log("[aumid] shortcut exists, skip");
            return;
        }
    }

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

    // 绝对路径调用系统 PowerShell：`Command::new("powershell")` 走 PATH 解析，
    // 存在被同名可执行文件劫持（或 PATH 缺失时静默失败）的面。
    // 优先由 %SystemRoot% 拼出（兼容系统盘非 C: 的机器），缺失时退回惯用路径。
    let powershell = std::env::var_os("SystemRoot")
        .map(std::path::PathBuf::from)
        .map(|root| {
            root.join("System32")
                .join("WindowsPowerShell")
                .join("v1.0")
                .join("powershell.exe")
        })
        .filter(|p| p.exists())
        .unwrap_or_else(|| {
            std::path::PathBuf::from(r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe")
        });

    match Command::new(powershell)
        .args(["-NoProfile", "-NonInteractive", "-Command", &ps])
        .creation_flags(0x08000000) // CREATE_NO_WINDOW
        .output()
    {
        Ok(out) => {
            if out.status.success() {
                process::append_verbose_log("[aumid] registered successfully");
            } else {
                let stderr = String::from_utf8_lossy(&out.stderr);
                standard_log!("[aumid] registration failed: {}", stderr.trim());
            }
        }
        Err(e) => verbose_log!("[aumid] powershell exec error: {}", e),
    }
}

#[cfg(not(target_os = "windows"))]
pub fn register_aumid() {}

#[cfg(all(test, target_os = "windows"))]
mod tests {
    use super::*;

    /// `resolve_toast_icon` 必须**只写一次盘**，其后走 `OnceLock` 缓存。
    ///
    /// 判据用 mtime：若每次调用都重写，第二次的 mtime 必然前进。
    /// 两次调用之间 sleep 一下以越过文件系统的 mtime 精度。
    #[test]
    fn toast_icon_is_written_once_then_cached() {
        let first = resolve_toast_icon().expect("临时目录应可写");
        let mtime_first = std::fs::metadata(&first)
            .and_then(|m| m.modified())
            .expect("应能读到 mtime");

        std::thread::sleep(std::time::Duration::from_millis(20));
        let second = resolve_toast_icon().expect("缓存命中后仍应返回路径");
        let mtime_second = std::fs::metadata(&second)
            .and_then(|m| m.modified())
            .expect("应能读到 mtime");

        assert_eq!(second, first, "两次调用应返回同一路径");
        assert_eq!(mtime_first, mtime_second, "第二次调用不应重写文件");
    }

    /// `wait_for_dpi_settle` 必须**让出执行器**，而不是阻塞它。
    ///
    /// 判据用「多次等待的总耗时」：在 **current_thread** 运行时上并发跑 3 次等待，
    /// - 异步 sleep ⇒ 三个定时器重叠，总耗时 ≈ 1×（200ms）；
    /// - 阻塞 sleep ⇒ 三次串行，总耗时 ≈ 3×（600ms）。
    ///
    /// 之所以选 current_thread 运行时：单线程下「阻塞」无处可躲，
    /// 多线程运行时会被其他 worker 吸收掉，测不出差别。
    #[test]
    fn dpi_settle_wait_yields_the_executor() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .expect("应能构建 current_thread 运行时");

        let started = std::time::Instant::now();
        rt.block_on(async {
            // 不用 `tokio::join!`：它需要 `macros` feature（本项目只启用了 rt/time），
            // 为一个单测引入新 feature 不值得。`spawn` 在 current_thread 运行时上
            // 同样共用唯一线程，判据等价。
            let a = tokio::spawn(wait_for_dpi_settle());
            let b = tokio::spawn(wait_for_dpi_settle());
            let c = tokio::spawn(wait_for_dpi_settle());
            a.await.expect("等待任务不应 panic");
            b.await.expect("等待任务不应 panic");
            c.await.expect("等待任务不应 panic");
        });
        let elapsed = started.elapsed();

        assert!(
            elapsed < std::time::Duration::from_millis(450),
            "3 次等待必须重叠（预期 ≈200ms）；实测 {elapsed:?}。\
             若接近 600ms，说明 wait_for_dpi_settle 退化成了阻塞 sleep（P2-10 回归）"
        );
        assert!(
            elapsed >= std::time::Duration::from_millis(150),
            "等待不得被跳过（实测 {elapsed:?}）"
        );
    }
}
