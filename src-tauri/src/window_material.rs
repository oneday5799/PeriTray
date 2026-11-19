//! 窗口材质（DWM 背景板）：mica / acrylic / default 三层切换与能力探测。
//! 依赖 hwnd 走 DWM API，与 webview 表面的恒透明（webview 模块）相互独立。

use crate::config;
use crate::process;
use tauri::{Emitter, Manager};

#[cfg(target_os = "windows")]
use crate::standard_log;
mod material {
    use std::sync::OnceLock;

    const DWMWA_SYSTEMBACKDROP_TYPE: u32 = 38;
    const DWMWA_MICA_EFFECT: u32 = 1029;
    const DWM_BB_ENABLE: u32 = 1;
    const DWMSBT_NONE: u32 = 1;
    const DWMSBT_MAINWINDOW: u32 = 2;
    const DWMSBT_TRANSIENTWINDOW: u32 = 3;

    const ACCENT_DISABLED: u32 = 0;
    // 数值 4 = ACRYLIC（3 才是普通 BLURBEHIND），命名必须与 API 数值一一对应
    const ACCENT_ENABLE_ACRYLICBLURBEHIND: u32 = 4;
    // SWCA 的属性 ID：WCA_ACCENT_POLICY（未文档化；数值与 window-vibrancy 同款调用一致）
    const WCA_ACCENT_POLICY: u32 = 0x13;

    #[repr(C)]
    #[allow(dead_code)]
    struct AccentPolicy {
        accent_state: u32,
        accent_flags: u32,
        gradient_color: u32,
        animation_id: u32,
    }

    /// SetWindowCompositionAttribute 第二实参是**属性数据**结构：Attrib 必须为
    /// WCA_ACCENT_POLICY，pvData 指向 ACCENT_POLICY，cbData = 策略字节数。
    /// ⛔ 不得把 ACCENT_POLICY 裸指针直接当第二实参——按属性数据布局解读得
    /// Attrib = accent_state（4 ≠ 0x13）、cbData = gradient_color（≈16MB）⇒
    /// 调用静默无效；返回 BOOL，**非零**才是成功（0 = 失败）。
    #[repr(C)]
    #[allow(dead_code)]
    struct WindowCompositionAttribData {
        attrib: u32,
        pv_data: *mut core::ffi::c_void,
        cb_data: usize,
    }

    #[repr(C)]
    struct DwmMargins {
        cx_left: i32,
        cx_right: i32,
        cy_top: i32,
        cy_bottom: i32,
    }

    type SetWindowCompositionAttrFn = unsafe extern "system" fn(
        *mut core::ffi::c_void,
        *const WindowCompositionAttribData,
    ) -> i32;

    static SET_WINDOW_COMPOSITION: OnceLock<Option<SetWindowCompositionAttrFn>> = OnceLock::new();

    fn get_set_window_composition() -> Option<SetWindowCompositionAttrFn> {
        *SET_WINDOW_COMPOSITION.get_or_init(|| unsafe {
            // ⚠️ 用 windows-sys 的 `s!`，不要改成 `c"user32.dll".as_ptr()`：
            // `CStr::as_ptr()` 给 `*const i8`，而 `PCSTR` 是 `*const u8` ⇒ 直接传会 `E0308`。
            // `s!` 展开即 `concat!($s, '\0').as_ptr()`，类型天然是 `PCSTR`，NUL 也由编译器保证。
            let user32 = windows_sys::Win32::System::LibraryLoader::LoadLibraryA(windows_sys::s!(
                "user32.dll"
            ));
            if user32.is_null() {
                return None;
            }
            let proc = windows_sys::Win32::System::LibraryLoader::GetProcAddress(
                user32,
                windows_sys::s!("SetWindowCompositionAttribute"),
            );
            proc.map(|f| std::mem::transmute(f))
        })
    }

    /// SWCA 设置/清除的唯一实现点（亚克力降级路径：不支持
    /// DWMWA_SYSTEMBACKDROP_TYPE 的系统）。调用契约见 WindowCompositionAttribData。
    unsafe fn set_accent_policy(hwnd: isize, accent_state: u32, gradient_color: u32) -> bool {
        let Some(fn_ptr) = get_set_window_composition() else {
            return false;
        };
        let policy = AccentPolicy {
            accent_state,
            accent_flags: 0,
            gradient_color,
            animation_id: 0,
        };
        let data = WindowCompositionAttribData {
            attrib: WCA_ACCENT_POLICY,
            pv_data: &policy as *const _ as *mut _,
            cb_data: std::mem::size_of::<AccentPolicy>(),
        };
        fn_ptr(hwnd as *mut core::ffi::c_void, &data) != 0
    }

    /// SWCA 亚克力生效前把 blur behind 的 region 从 tao 的「空区域」改为**满窗**：
    /// 空 region 虽能让窗口透明、accent 也能挂上，但 Win10 系 DWM 上子视觉
    /// （WebView2）的合成会出条纹/白块伪影（客户区磨砂被不透明白底和乱纹
    /// 打断）；满窗 region 后磨砂铺满客户区。依据：Server 20348 裸宿主
    /// A/B 实测（空 region=通道混叠条纹，满 region=磨砂正确虚化背景）。
    /// ⭐ 必须在 accent 挂上之前调用（先 region 后 accent 的顺序与探针
    /// 一致）。位置：唯一实现点（仅 set_acrylic 调用）。
    unsafe fn set_full_region_blur_behind(hwnd: isize) {
        let mut rect = windows_sys::Win32::Foundation::RECT {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        };
        let _ = windows_sys::Win32::UI::WindowsAndMessaging::GetClientRect(
            hwnd as *mut core::ffi::c_void,
            &mut rect,
        );
        let rgn = windows_sys::Win32::Graphics::Gdi::CreateRectRgn(
            rect.left,
            rect.top,
            rect.right,
            rect.bottom,
        );
        let bb = windows_sys::Win32::Graphics::Dwm::DWM_BLURBEHIND {
            dwFlags: DWM_BB_ENABLE,
            fEnable: 1,
            hRgnBlur: rgn,
            fTransitionOnMaximized: 0,
        };
        let _ = windows_sys::Win32::Graphics::Dwm::DwmEnableBlurBehindWindow(
            hwnd as *mut core::ffi::c_void,
            &bb,
        );
        let _ = windows_sys::Win32::Graphics::Gdi::DeleteObject(rgn);
    }

    unsafe fn set_acrylic(hwnd: isize) -> bool {
        set_full_region_blur_behind(hwnd);
        // 底衬 alpha 不得为 0（0 ⇒ 不出现模糊）；0x01 = 近乎全透明黑底衬，
        // 与 window-vibrancy「acrylic alpha=0 时强制 1」口径一致
        set_accent_policy(hwnd, ACCENT_ENABLE_ACRYLICBLURBEHIND, 0x01000000)
    }

    unsafe fn remove_acrylic(hwnd: isize) -> bool {
        set_accent_policy(hwnd, ACCENT_DISABLED, 0)
    }

    /// DWM 玻璃框全幅延伸进客户区（margins 全 -1）。仅 Win11 背景板路径使用
    /// （文档要求）。⛔ Win10 SWCA 路径不得用全幅——玻璃片盖住客户区后
    /// WebView2 子窗口的透明 alpha 隔着玻璃片合成，客户区磨砂消失、只剩 NC
    /// 环磨砂（真机实测）；无框窗调它还会在客户区四周描白圈。
    unsafe fn extend_frame_all_glass(hwnd: isize) {
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
    }

    pub unsafe fn apply(hwnd: isize, material: &str) -> bool {
        if material == "default" {
            remove(hwnd);
            return true;
        }

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
            // 背景板路径（Win11）：按文档要求扩展框架进客户区
            extend_frame_all_glass(hwnd);
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
            // ⛔ mica 在 Win10（build < 22000）两处 DWM 调用必败：到此返回
            // false 会让「推荐」进入未定义态——前端 data-material 已置透明、
            // DWM 层却残留旧值（默认→推荐=透出桌面 / 亚克力→推荐=观感错乱，
            // 真机实测）。「推荐」的语义是本系统最佳可用磨砂 ⇒ fallback 到
            // SWCA 亚克力（CSS 半透明规则对 data-material 无值限定，两端一致）。
            return set_acrylic(hwnd);
        }

        if effective == "acrylic" {
            // ⛔ 两窗均已无框（settings 已废除原生标题栏，网页自绘）：SWCA 路径
            // 零 extend-frame——无框窗调任何玻璃片形态，DWM 都会以为存在边框
            // 而描圈/盖磨砂（真机实测）。钉死单测：swca_frameless_paths_skip_extend。
            return set_acrylic(hwnd);
        }

        false
    }

    pub unsafe fn remove(hwnd: isize) {
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

    #[cfg(test)]
    mod tests {
        use super::*;

        /// SWCA 属性数据 ABI（x64/arm64）：{ u32, 4 字节对齐垫, 指针, usize }
        /// ⇒ size 24、pv_data 偏移 8、cb_data 偏移 16。repr 或字段顺序错动
        /// 会让调用按错误偏移取值，在此必然失败。
        #[test]
        fn swca_attrib_data_layout() {
            assert_eq!(std::mem::size_of::<WindowCompositionAttribData>(), 24);
            assert_eq!(
                std::mem::offset_of!(WindowCompositionAttribData, pv_data),
                8
            );
            assert_eq!(
                std::mem::offset_of!(WindowCompositionAttribData, cb_data),
                16
            );
        }

        /// ACCENT_POLICY ABI：四个 u32 紧排 16 字节（与 window-vibrancy 同构）。
        #[test]
        fn accent_policy_layout() {
            assert_eq!(std::mem::size_of::<AccentPolicy>(), 16);
            assert_eq!(std::mem::offset_of!(AccentPolicy, gradient_color), 8);
        }

        /// ⛔⛔ **调用点钉死**：SWCA 实参必须是「属性数据包裹 ACCENT_POLICY」结构
        ///   （attrib = WCA_ACCENT_POLICY），返回按 BOOL 判（非零 = 成功）。亚克力
        ///   渲染无单测可看 ⇒ 只能钉调用点文本（判据切片先剔掉本函数自身，防
        ///   「断言字符串自己命中」）。
        /// 可证伪：退回「裸 AccentPolicy 直传」或「`== 0` 反判成功」（修复前的
        ///   两种形态）⇒ 转红。
        #[test]
        fn swca_call_wraps_policy_in_attrib_data() {
            let src = include_str!("window_material.rs");
            let fn_start = src
                .find("fn swca_call_wraps_policy_in_attrib_data")
                .expect("判据函数自身必须还在");
            let body = &src[..fn_start];
            assert!(
                body.contains("attrib: WCA_ACCENT_POLICY,"),
                "SWCA 调用必须以属性数据包裹（attrib = WCA_ACCENT_POLICY）"
            );
            assert!(
                body.contains("pv_data: &policy"),
                "属性数据必须指向 ACCENT_POLICY，不得传裸指针"
            );
            assert!(
                body.contains(", &data) != 0"),
                "返回值必须按 BOOL 判（非零 = 成功）"
            );
            assert!(
                !body.contains("&policy) == 0"),
                "⛔ 不得退回「裸 AccentPolicy 直传 + `== 0` 反判」的形态"
            );
        }

        /// ⛔ **调用点钉死**：两窗均已无框（settings 已废除原生标题栏，网页自绘），
        ///   SWCA 降级路径必须零玻璃片——无框窗调任何 DwmExtendFrameIntoClientArea
        ///   形态，DWM 都会以为存在边框而描圈/盖磨砂（真机实测）。
        /// 可证伪：SWCA 路径重新出现玻璃片调用或 decorated 分流 ⇒ 转红。
        #[test]
        fn swca_frameless_paths_skip_extend() {
            let src = include_str!("window_material.rs");
            let fn_start = src
                .find("fn swca_frameless_paths_skip_extend")
                .expect("判据函数自身必须还在");
            let body = &src[..fn_start];
            assert!(
                !body.contains("extend_frame_nc_only"),
                "⛔ NC-only 玻璃片已随有框窗退役，不得复活"
            );
            assert!(
                !body.contains("if decorated"),
                "⛔ decorated 分流已随无框化退役，不得复活"
            );
            // extend_frame_all_glass 只许出现在 Win11 背景板路径（hr == 0 分支）：
            // 从 acrylic 分支起（SWCA 降级段）不得再有任何 extend-frame 形态
            let swca = body
                .rfind("if effective == \"acrylic\" {")
                .expect("SWCA acrylic 分支必须还在");
            assert!(
                !body[swca..].contains("extend_frame"),
                "SWCA 降级段不得出现任何 extend-frame 形态"
            );
            assert!(
                body.contains("return set_acrylic(hwnd);"),
                "SWCA 分支必须仍以 set_acrylic 收尾（不得绕过 SWCA 挂载）"
            );
        }
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
            windows_sys::Win32::System::LibraryLoader::LoadLibraryA(windows_sys::s!("ntdll.dll"));
        if ntdll.is_null() {
            return false;
        }
        let proc = windows_sys::Win32::System::LibraryLoader::GetProcAddress(
            ntdll,
            windows_sys::s!("RtlGetVersion"),
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
    standard_log!("[material] set_window_material: {}", material);
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
                    standard_log!("[material] apply {} to {} -> {}", material, label, ok);
                    if ok {
                        any_success = true;
                    }
                }
            }
        }
    }

    Ok(any_success)
}
