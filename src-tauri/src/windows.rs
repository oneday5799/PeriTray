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
                apply_window_material(hwnd.0 as isize, &material);
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
                apply_window_material(hwnd.0 as isize, &material);
                ensure_webview_bg_transparent(win.as_ref());
            }
            std::thread::sleep(std::time::Duration::from_millis(200));
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
pub fn monitor_at_point(app: &tauri::AppHandle, x: f64, y: f64) -> Option<tauri::Monitor> {
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

// ═══════════════════════════════════════════════════════════════
// AUMID 注册（Windows 通知图标依赖）
// ═══════════════════════════════════════════════════════════════

const AUMID: &str = "com.periph.monitor";

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
$shortcutPath = Join-Path $programs 'PeriphMonitor.lnk'
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

// ═══════════════════════════════════════════════════════════════
// 窗口材质（Window Material）
// ═══════════════════════════════════════════════════════════════

/// 通过 Tauri with_webview API 设置 WebView2 背景颜色
/// 使用 ICoreWebView2Controller2::SetDefaultBackgroundColor
fn set_webview_bg_color(webview: &tauri::Webview, color: [u8; 4]) {
    let r = webview.with_webview(move |wv| {
        #[cfg(target_os = "windows")]
        unsafe {
            let controller = wv.controller();
            let raw: *mut core::ffi::c_void = std::mem::transmute(controller);
            if raw.is_null() {
                process::append_log("[webview_bg] controller is null");
                return;
            }

            let vtable = *(raw as *const *const usize);
            let iid = windows::core::GUID::from_u128(0xc979903e_d4ca_4228_92eb_47ee3fa96eab);

            type QIFn = unsafe extern "system" fn(
                *mut core::ffi::c_void,
                *const windows::core::GUID,
                *mut *mut core::ffi::c_void,
            ) -> i32;
            let qi: QIFn = std::mem::transmute(*vtable.add(0));
            let mut ptr: *mut core::ffi::c_void = std::ptr::null_mut();
            let hr = qi(raw, &iid, &mut ptr);
            if hr != 0 || ptr.is_null() {
                process::append_log(&format!("[webview_bg] QI failed, hr={}", hr));
                return;
            }

            let vt2 = *(ptr as *const *const usize);

            type SetBgFn = unsafe extern "system" fn(*mut core::ffi::c_void, [u8; 4]) -> i32;
            let set_bg: SetBgFn = std::mem::transmute(*vt2.add(16));
            let hr2 = set_bg(ptr, color);

            type RelFn = unsafe extern "system" fn(*mut core::ffi::c_void) -> u32;
            let rel: RelFn = std::mem::transmute(*vt2.add(2));
            rel(ptr);

            if hr2 != 0 {
                process::append_log(&format!(
                    "[webview_bg] SetDefaultBackgroundColor failed, hr={}",
                    hr2
                ));
            } else {
                process::append_log(&format!("[webview_bg] set to {:?}", color));
            }
        }
    });
    if r.is_err() {
        process::append_log("[webview_bg] with_webview dispatch failed");
    }
}

fn set_webview_bg_transparent(webview: &tauri::Webview) {
    set_webview_bg_color(webview, [0, 0, 0, 0]);
}

/// 带重试的 webview 背景透明设置，用于窗口创建后异步调用
pub fn ensure_webview_bg_transparent(webview: &tauri::Webview) {
    let wb = webview.clone();
    std::thread::spawn(move || {
        for attempt in 1..=4 {
            std::thread::sleep(std::time::Duration::from_millis(300 * attempt));
            set_webview_bg_transparent(&wb);
            process::append_log(&format!("[webview_bg] transparent attempt {}", attempt));
        }
    });
}

// ═══════════════════════════════════════════════════════════════
// WebView2 Suspend / Resume（ICoreWebView2_3 页面生命周期 API）
// ═══════════════════════════════════════════════════════════════
//
// popup 关闭后：put_IsVisible(FALSE) + TrySuspend → 渲染进程完全休眠，
//   系统睡眠时 COM 不活跃，不阻塞事件循环（B 类僵死根治）。
// popup 打开前 / 唤醒后：Resume + put_IsVisible(TRUE) → 恢复渲染。
//
// 调用链：PlatformWebview.controller() → ICoreWebView2Controller
//   → get_CoreWebView2(vt:25) → ICoreWebView2
//   → QI(IID:{A0D6DF20-3B92-416D-AA0C-437A9C727857}) → ICoreWebView2_3
//   → TrySuspend(vt:68) / Resume(vt:69)
//
// vtable 偏移（webview2-com-sys 0.38.2 官方绑定确认）：
//   Controller: IUnknown(0-2), IsVisible(3), SetIsVisible(4), ..., CoreWebView2(25)
//   WebView2_3: IUnknown(0-2)+ICoreWebView2(3-60)+ICoreWebView2_2(61-67) → TrySuspend(68), Resume(69)

/// TrySuspend 完成回调（最小 COM 对象，vtable 指针为首字段的标準布局）
#[cfg(target_os = "windows")]
mod try_suspend_cb {
    use super::process;

    #[repr(C)]
    pub struct Obj {
        vtable: *const Vtable,
    }

    #[repr(C)]
    struct Vtable {
        qi: unsafe extern "system" fn(
            *mut Obj,
            *const windows_sys::core::GUID,
            *mut *mut core::ffi::c_void,
        ) -> i32,
        add_ref: unsafe extern "system" fn(*mut Obj) -> u32,
        release: unsafe extern "system" fn(*mut Obj) -> u32,
        invoke: unsafe extern "system" fn(*mut Obj, i32, i32) -> i32,
    }

    unsafe extern "system" fn qi(
        _this: *mut Obj,
        _iid: *const windows_sys::core::GUID,
        out: *mut *mut core::ffi::c_void,
    ) -> i32 {
        unsafe { *out = core::ptr::null_mut() };
        -2147467262 // E_NOINTERFACE
    }

    unsafe extern "system" fn add_ref(_this: *mut Obj) -> u32 {
        1
    }

    unsafe extern "system" fn release(this: *mut Obj) -> u32 {
        unsafe { drop(Box::from_raw(this)) };
        0
    }

    unsafe extern "system" fn invoke(_this: *mut Obj, error_code: i32, is_successful: i32) -> i32 {
        process::append_log(&format!(
            "[webview] TrySuspend completed: hr=0x{:08X} success={}",
            error_code as u32,
            is_successful != 0
        ));
        0 // S_OK
    }

    static VTABLE: Vtable = Vtable {
        qi,
        add_ref,
        release,
        invoke,
    };

    pub fn create() -> *mut core::ffi::c_void {
        let obj = Box::new(Obj { vtable: &VTABLE });
        Box::into_raw(obj) as *mut core::ffi::c_void
    }

    pub unsafe fn destroy(ptr: *mut core::ffi::c_void) {
        unsafe { drop(Box::from_raw(ptr as *mut Obj)) };
    }
}

/// 从 controller 获取 ICoreWebView2_3 接口指针（内部辅助，调用方负责 Release）。
/// 返回 None 表示任何步骤失败。
#[cfg(target_os = "windows")]
unsafe fn get_webview2_3(controller: *mut core::ffi::c_void) -> Option<*mut core::ffi::c_void> {
    // controller → get_CoreWebView2(vt:25) → ICoreWebView2
    let cvtable = *(controller as *const *const usize);
    type GetCoreWebView2Fn =
        unsafe extern "system" fn(*mut core::ffi::c_void, *mut *mut core::ffi::c_void) -> i32;
    let get_webview: GetCoreWebView2Fn = std::mem::transmute(*cvtable.add(25));
    let mut wv_ptr: *mut core::ffi::c_void = std::ptr::null_mut();
    let hr = get_webview(controller, &mut wv_ptr);
    if hr != 0 || wv_ptr.is_null() {
        return None;
    }

    // ICoreWebView2 → QI(ICoreWebView2_3) → ICoreWebView2_3
    // IID {A0D6DF20-3B92-416D-AA0C-437A9C727857} 来自 webview2-com-sys 官方绑定
    let wv_vtable = *(wv_ptr as *const *const usize);
    type QIFn = unsafe extern "system" fn(
        *mut core::ffi::c_void,
        *const windows_sys::core::GUID,
        *mut *mut core::ffi::c_void,
    ) -> i32;
    let qi: QIFn = std::mem::transmute(*wv_vtable.add(0));
    let mut ptr: *mut core::ffi::c_void = std::ptr::null_mut();
    let iid = windows_sys::core::GUID::from_u128(0xa0d6df20_3b92_416d_aa0c_437a9c727857);
    let hr = qi(wv_ptr, &iid, &mut ptr);

    // 释放 get_CoreWebView2 返回的 ICoreWebView2 引用
    type ReleaseFn = unsafe extern "system" fn(*mut core::ffi::c_void) -> u32;
    let release_wv: ReleaseFn = std::mem::transmute(*wv_vtable.add(2));
    release_wv(wv_ptr);

    if hr != 0 || ptr.is_null() {
        return None;
    }
    Some(ptr)
}

/// Suspend WebView2 渲染进程（popup 关闭后调用）。
/// put_IsVisible(FALSE) + TrySuspend：停止渲染 + 挂起渲染进程。
#[cfg(target_os = "windows")]
pub fn suspend_webview(webview: &tauri::Webview) {
    let wb = webview.clone();
    let r = wb.with_webview(|wv| {
        unsafe {
            let controller: *mut core::ffi::c_void = std::mem::transmute(wv.controller());
            if controller.is_null() {
                return;
            }
            let cvtable = *(controller as *const *const usize);

            // Step1: put_IsVisible(FALSE)——TrySuspend 的前置条件
            type SetIsVisibleFn = unsafe extern "system" fn(*mut core::ffi::c_void, i32) -> i32;
            let set_visible: SetIsVisibleFn = std::mem::transmute(*cvtable.add(4));
            set_visible(controller, 0);

            // Step2: TrySuspend——挂起渲染进程
            if let Some(ptr) = get_webview2_3(controller) {
                type ReleaseFn = unsafe extern "system" fn(*mut core::ffi::c_void) -> u32;
                let vtable3 = *(ptr as *const *const usize);
                type TrySuspendFn = unsafe extern "system" fn(
                    *mut core::ffi::c_void,
                    *mut core::ffi::c_void,
                ) -> i32;
                let try_suspend: TrySuspendFn = std::mem::transmute(*vtable3.add(68));
                let cb_ptr = try_suspend_cb::create();
                let hr = try_suspend(ptr, cb_ptr);
                if hr != 0 {
                    process::append_log(&format!("[webview] TrySuspend call failed: 0x{:08X}", hr));
                    try_suspend_cb::destroy(cb_ptr);
                }
                // Release ICoreWebView2_3
                let release3: ReleaseFn = std::mem::transmute(*vtable3.add(2));
                release3(ptr);
            } else {
                process::append_log("[webview] get ICoreWebView2_3 failed for TrySuspend");
            }
        }
    });
    if r.is_err() {
        process::append_log("[webview] suspend_webview: with_webview dispatch failed");
    }
}

/// Resume WebView2 渲染进程（popup 打开前 / 系统唤醒后调用）。
/// Resume + put_IsVisible(TRUE)：恢复渲染进程 + 恢复渲染。
#[cfg(target_os = "windows")]
pub fn resume_webview(webview: &tauri::Webview) {
    let wb = webview.clone();
    let r = wb.with_webview(|wv| {
        unsafe {
            let controller: *mut core::ffi::c_void = std::mem::transmute(wv.controller());
            if controller.is_null() {
                return;
            }
            let cvtable = *(controller as *const *const usize);

            // Step1: Resume——恢复渲染进程
            if let Some(ptr) = get_webview2_3(controller) {
                type ReleaseFn = unsafe extern "system" fn(*mut core::ffi::c_void) -> u32;
                let vtable3 = *(ptr as *const *const usize);
                type ResumeFn = unsafe extern "system" fn(*mut core::ffi::c_void) -> i32;
                let resume_fn: ResumeFn = std::mem::transmute(*vtable3.add(69));
                let hr = resume_fn(ptr);
                if hr != 0 {
                    process::append_log(&format!("[webview] Resume call failed: 0x{:08X}", hr));
                }
                let release3: ReleaseFn = std::mem::transmute(*vtable3.add(2));
                release3(ptr);
            }

            // Step2: put_IsVisible(TRUE)——恢复渲染
            type SetIsVisibleFn = unsafe extern "system" fn(*mut core::ffi::c_void, i32) -> i32;
            let set_visible: SetIsVisibleFn = std::mem::transmute(*cvtable.add(4));
            set_visible(controller, 1);
        }
    });
    if r.is_err() {
        process::append_log("[webview] resume_webview: with_webview dispatch failed");
    }
}

#[cfg(not(target_os = "windows"))]
pub fn suspend_webview(_webview: &tauri::Webview) {}

#[cfg(not(target_os = "windows"))]
pub fn resume_webview(_webview: &tauri::Webview) {}

#[cfg(target_os = "windows")]
mod material {
    use std::sync::OnceLock;

    const DWMWA_SYSTEMBACKDROP_TYPE: u32 = 38;
    const DWMWA_MICA_EFFECT: u32 = 1029;
    const DWMSBT_NONE: u32 = 1;
    const DWMSBT_MAINWINDOW: u32 = 2;
    const DWMSBT_TRANSIENTWINDOW: u32 = 3;

    const ACCENT_DISABLED: u32 = 0;
    const ACCENT_ENABLE_BLURBEHIND: u32 = 4;

    #[repr(C)]
    #[allow(dead_code)]
    struct AccentPolicy {
        accent_state: u32,
        accent_flags: u32,
        gradient_color: u32,
        animation_id: u32,
    }

    type SetWindowCompositionAttrFn =
        unsafe extern "system" fn(*mut core::ffi::c_void, *const AccentPolicy) -> i32;

    static SET_WINDOW_COMPOSITION: OnceLock<Option<SetWindowCompositionAttrFn>> = OnceLock::new();

    fn get_set_window_composition() -> Option<SetWindowCompositionAttrFn> {
        *SET_WINDOW_COMPOSITION.get_or_init(|| unsafe {
            let user32 =
                windows_sys::Win32::System::LibraryLoader::LoadLibraryA(b"user32.dll\0".as_ptr());
            if user32.is_null() {
                return None;
            }
            let proc = windows_sys::Win32::System::LibraryLoader::GetProcAddress(
                user32,
                b"SetWindowCompositionAttribute\0".as_ptr(),
            );
            proc.map(|f| std::mem::transmute(f))
        })
    }

    unsafe fn set_acrylic(hwnd: isize) -> bool {
        let Some(fn_ptr) = get_set_window_composition() else {
            return false;
        };
        let policy = AccentPolicy {
            accent_state: ACCENT_ENABLE_BLURBEHIND,
            accent_flags: 0,
            gradient_color: 0x01000000,
            animation_id: 0,
        };
        fn_ptr(hwnd as *mut core::ffi::c_void, &policy) == 0
    }

    unsafe fn remove_acrylic(hwnd: isize) -> bool {
        let Some(fn_ptr) = get_set_window_composition() else {
            return false;
        };
        let policy = AccentPolicy {
            accent_state: ACCENT_DISABLED,
            accent_flags: 0,
            gradient_color: 0,
            animation_id: 0,
        };
        fn_ptr(hwnd as *mut core::ffi::c_void, &policy) == 0
    }

    pub unsafe fn apply(hwnd: isize, material: &str) -> bool {
        if material == "default" {
            remove(hwnd);
            return true;
        }

        #[repr(C)]
        struct DwmMargins {
            cx_left: i32,
            cx_right: i32,
            cy_top: i32,
            cy_bottom: i32,
        }
        let margins = DwmMargins {
            cx_left: -1,
            cx_right: -1,
            cy_top: -1,
            cy_bottom: -1,
        };
        let _ = windows_sys::Win32::Graphics::Dwm::DwmExtendFrameIntoClientArea(
            hwnd as *mut core::ffi::c_void,
            &margins as *const _ as *const _,
        );

        let effective = if material == "recommended" {
            "mica"
        } else {
            material
        };

        let backdrop_type = match effective {
            "mica" => DWMSBT_MAINWINDOW,
            "acrylic" => DWMSBT_TRANSIENTWINDOW,
            _ => return false,
        };
        let hr = windows_sys::Win32::Graphics::Dwm::DwmSetWindowAttribute(
            hwnd as *mut core::ffi::c_void,
            DWMWA_SYSTEMBACKDROP_TYPE,
            &backdrop_type as *const _ as *const _,
            std::mem::size_of::<u32>() as u32,
        );
        if hr == 0 {
            return true;
        }

        if effective == "mica" {
            let enabled: u32 = 1;
            let hr2 = windows_sys::Win32::Graphics::Dwm::DwmSetWindowAttribute(
                hwnd as *mut core::ffi::c_void,
                DWMWA_MICA_EFFECT,
                &enabled as *const _ as *const _,
                std::mem::size_of::<u32>() as u32,
            );
            if hr2 == 0 {
                return true;
            }
        }

        if effective == "acrylic" {
            return set_acrylic(hwnd);
        }

        false
    }

    pub unsafe fn remove(hwnd: isize) {
        #[repr(C)]
        struct DwmMargins {
            cx_left: i32,
            cx_right: i32,
            cy_top: i32,
            cy_bottom: i32,
        }
        let margins = DwmMargins {
            cx_left: 0,
            cx_right: 0,
            cy_top: 0,
            cy_bottom: 0,
        };
        let _ = windows_sys::Win32::Graphics::Dwm::DwmExtendFrameIntoClientArea(
            hwnd as *mut core::ffi::c_void,
            &margins as *const _ as *const _,
        );

        let none: u32 = DWMSBT_NONE;
        let _ = windows_sys::Win32::Graphics::Dwm::DwmSetWindowAttribute(
            hwnd as *mut core::ffi::c_void,
            DWMWA_SYSTEMBACKDROP_TYPE,
            &none as *const _ as *const _,
            std::mem::size_of::<u32>() as u32,
        );

        let disabled: u32 = 0;
        let _ = windows_sys::Win32::Graphics::Dwm::DwmSetWindowAttribute(
            hwnd as *mut core::ffi::c_void,
            DWMWA_MICA_EFFECT,
            &disabled as *const _ as *const _,
            std::mem::size_of::<u32>() as u32,
        );

        remove_acrylic(hwnd);
    }
}

#[cfg(target_os = "windows")]
pub fn apply_window_material(hwnd: isize, material: &str) -> bool {
    unsafe { material::apply(hwnd, material) }
}

#[cfg(not(target_os = "windows"))]
pub fn apply_window_material(_hwnd: isize, _material: &str) -> bool {
    false
}

#[cfg(target_os = "windows")]
pub fn check_material_support(material: &str) -> bool {
    if material == "default" {
        return true;
    }
    let effective = if material == "recommended" {
        "mica"
    } else {
        material
    };

    #[repr(C)]
    struct RtlOsVersionInfoEx {
        dw_os_version_info_size: u32,
        dw_major_version: u32,
        dw_minor_version: u32,
        dw_build_number: u32,
        dw_platform_id: u32,
        sz_csd_version: [u16; 128],
        w_service_pack_major: u16,
        w_service_pack_minor: u16,
        w_suite_mask: u16,
        w_product_type: u8,
        w_reserved: u8,
    }

    type RtlGetVersionFn = unsafe extern "system" fn(*mut RtlOsVersionInfoEx) -> i32;

    let build = unsafe {
        let ntdll =
            windows_sys::Win32::System::LibraryLoader::LoadLibraryA(b"ntdll.dll\0".as_ptr());
        if ntdll.is_null() {
            return false;
        }
        let proc = windows_sys::Win32::System::LibraryLoader::GetProcAddress(
            ntdll,
            b"RtlGetVersion\0".as_ptr(),
        );
        let Some(f): Option<RtlGetVersionFn> = proc.map(|f| std::mem::transmute(f)) else {
            return false;
        };
        let mut osvi = RtlOsVersionInfoEx {
            dw_os_version_info_size: std::mem::size_of::<RtlOsVersionInfoEx>() as u32,
            dw_major_version: 0,
            dw_minor_version: 0,
            dw_build_number: 0,
            dw_platform_id: 0,
            sz_csd_version: [0; 128],
            w_service_pack_major: 0,
            w_service_pack_minor: 0,
            w_suite_mask: 0,
            w_product_type: 0,
            w_reserved: 0,
        };
        if f(&mut osvi) != 0 {
            return false;
        }
        osvi.dw_build_number
    };

    match effective {
        "mica" => build >= 22000,
        "acrylic" => build >= 17763,
        _ => false,
    }
}

#[cfg(not(target_os = "windows"))]
pub fn check_material_support(_material: &str) -> bool {
    false
}

// ═══════════════════════════════════════════════════════════════
// Tauri 命令
// ═══════════════════════════════════════════════════════════════

pub fn set_window_material(app: &tauri::AppHandle, material: String) -> Result<bool, String> {
    process::append_log(&format!("[material] set_window_material: {}", material));
    config::with_config_mut(|c| c.window_material = material.clone());

    // 恒透明架构：webview 表面在创建时已一次性设为透明，运行时只切换两层——
    // DWM 背景板（同步可靠）+ 前端 data-material CSS（经 material-changed 事件）。
    // 「默认」材质的不透明观感由 CSS --page-bg 实色承担。
    // 先广播事件：两窗前端立即铺 CSS（默认材质=实色 / 非默认=半透明），
    // 再切换 DWM 背景板。恒透明表面下若先摘背景板后铺实色，会闪现一瞬桌面。
    let _ = app.emit("material-changed", &material);

    let mut any_success = false;
    for label in ["popup", "settings"] {
        if let Some(win) = app.get_webview_window(label) {
            #[cfg(target_os = "windows")]
            if let Ok(hwnd) = win.hwnd() {
                if material == "default" {
                    // 延迟摘除背景板，给前端 CSS 留出渲染帧；执行前复核配置，
                    // 防止快速往返切换时过期任务覆盖新材质
                    let h = hwnd.0 as isize;
                    std::thread::spawn(move || {
                        std::thread::sleep(std::time::Duration::from_millis(120));
                        let cur = config::with_config(|c| c.window_material.clone());
                        if cur == "default" {
                            apply_window_material(h, "default");
                            process::append_log("[material] delayed backdrop removal done");
                        }
                    });
                } else {
                    let ok = apply_window_material(hwnd.0 as isize, &material);
                    process::append_log(&format!(
                        "[material] apply {} to {} -> {}",
                        material, label, ok
                    ));
                    if ok {
                        any_success = true;
                    }
                }
            }
        }
    }

    Ok(any_success)
}
