//! 窗口基础设施：设置窗/弹窗的创建与定位（DPI/工作区）、系统暗色、任务栏层级、
//! 圆角、Toast 图标与 AUMID 注册。
//! WebView2 底层（背景色/生命周期）见 webview 模块；DWM 材质见 window_material 模块。

use crate::config;
use crate::process;
use tauri::{Emitter, Manager};

#[cfg(target_os = "windows")]
use crate::{standard_log, verbose_log};
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

/// 设置窗口创建后的「DPI 稳定等待」（P2-10）。
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
            // 等 DPI 稳定（异步 sleep，见 wait_for_dpi_settle 的说明）
            wait_for_dpi_settle().await;
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

/// 读 `SystemUsesLightTheme`（**系统**主题，区别于**应用**主题）。
///
/// ⭐ 为什么单开一个函数而不复用 `system_dark_mode()`：那个读的是 `AppsUseLightTheme`
///   （**应用**主题），用于决定 widget **内容**的明暗；而底衬配色的口径来自 FluentFlyout，
///   它按 **systemTheme**（`SystemUsesLightTheme`）取值
///   （`WindowsThemeDetector.GetWindowsTheme(out appTheme, out systemTheme)`）。
///   两者在「应用深色 + 系统浅色」这类自定义主题下**会不一致**，故分开读。
/// ⚠️ 读失败按 FluentFlyout 的约定**回落 light**（其源码注释：on error, default to light）。
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

// ═══════════════════════════════════════════════════════════════
// Toast 通知图标
// ═══════════════════════════════════════════════════════════════

/// 编译时嵌入的 toast 通知图标（`dist/icon.png`）。
/// 避免运行时文件系统路径歧义（Tauri 2 的 `frontendDist` 嵌入二进制，`resources` 部署到子目录）。
static TOAST_ICON_PNG: &[u8] = include_bytes!("../dist/icon.png");

/// 已写入临时目录的图标路径缓存（进程内只写一次）。
///
/// ⚠️ 必须保留 `#[cfg(target_os = "windows")]`：非 Windows 下只有下面那个返回 `None`
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
/// 结果用 `OnceLock` 缓存（P2-6）：图标内容编译期就已固定，重复写盘既无意义，又会让
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

    /// P2-6：`resolve_toast_icon` 必须**只写一次盘**，其后走 `OnceLock` 缓存。
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

    /// P2-10：`wait_for_dpi_settle` 必须**让出执行器**，而不是阻塞它。
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
