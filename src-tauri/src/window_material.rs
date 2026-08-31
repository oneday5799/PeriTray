//! 窗口材质（DWM 背景板）：mica / acrylic / default 三层切换与能力探测。
//! 依赖 hwnd 走 DWM API，与 webview 表面的恒透明（webview 模块）相互独立。

use crate::config;
use crate::process;
use tauri::{Emitter, Manager};

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
