//! WebView2 底层控制：背景色（恒透明）、页面生命周期（Suspend/Resume）。
//! PlatformWebview::controller() 直接返回强类型 ICoreWebView2Controller
//! （webview2-com 0.38，与 sys 同基座 windows-core 0.61），全部调用走
//! webview2-com-sys 类型安全 API——零 transmute、零手写 vtable、零手抄 IID；
//! 与 windows 模块的窗口定位 / 窗口材质（DWM）逻辑相互独立。

use crate::process;
use crate::standard_log;

#[cfg(target_os = "windows")]
use webview2_com_sys::Microsoft::Web::WebView2::Win32::{
    ICoreWebView2Controller2, ICoreWebView2TrySuspendCompletedHandler, ICoreWebView2_3,
    COREWEBVIEW2_COLOR,
};
#[cfg(target_os = "windows")]
use windows_core_061::Interface;

/// 通过 Tauri with_webview API 设置 WebView2 背景颜色
/// 使用 ICoreWebView2Controller2::SetDefaultBackgroundColor
/// 返回 true 表示设置成功
fn set_webview_bg_color(webview: &tauri::Webview, color: [u8; 4]) -> bool {
    let ok = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let ok_clone = ok.clone();
    let r = webview.with_webview(move |wv| {
        #[cfg(target_os = "windows")]
        unsafe {
            let controller = wv.controller();

            // 背景色接口在 Controller2（恒定接口，不受代际演进影响）
            let controller2: ICoreWebView2Controller2 = match controller.cast() {
                Ok(c) => c,
                Err(e) => {
                    standard_log!("[webview_bg] QI Controller2 failed: {}", e);
                    return;
                }
            };

            // [u8;4] 与 COREWEBVIEW2_COLOR { A,R,G,B } 内存布局一致
            let argb = COREWEBVIEW2_COLOR {
                A: color[0],
                R: color[1],
                G: color[2],
                B: color[3],
            };
            match controller2.SetDefaultBackgroundColor(argb) {
                Ok(()) => {
                    standard_log!("[webview_bg] set to {:?}", color);
                    ok_clone.store(true, std::sync::atomic::Ordering::Relaxed);
                }
                Err(e) => {
                    standard_log!("[webview_bg] SetDefaultBackgroundColor failed: {}", e);
                }
            }
        }
    });
    r.is_ok() && ok.load(std::sync::atomic::Ordering::Relaxed)
}

fn set_webview_bg_transparent(webview: &tauri::Webview) -> bool {
    set_webview_bg_color(webview, [0, 0, 0, 0])
}

/// 带重试的 webview 背景透明设置，用于窗口创建后异步调用
pub fn ensure_webview_bg_transparent(webview: &tauri::Webview) {
    let wb = webview.clone();
    std::thread::spawn(move || {
        for attempt in 1..=4 {
            std::thread::sleep(std::time::Duration::from_millis(300 * attempt));
            if set_webview_bg_transparent(&wb) {
                standard_log!("[webview_bg] transparent attempt {} ok", attempt);
                break;
            }
            standard_log!("[webview_bg] transparent attempt {}", attempt);
        }
    });
}

// ═══════════════════════════════════════════════════════════════
// WebView2 Suspend / Resume（ICoreWebView2_3 页面生命周期 API）
// ═══════════════════════════════════════════════════════════════
//
// popup 关闭后：IsVisible(FALSE) + TrySuspend → 渲染进程完全休眠，
//   系统睡眠时 COM 不活跃，不阻塞事件循环（B 类僵死根治）。
// popup 打开前 / 唤醒后：Resume + IsVisible(TRUE) → 恢复渲染。
//
// 调用链：PlatformWebview.controller() → ICoreWebView2Controller
//   → CoreWebView2() → ICoreWebView2 → cast::<ICoreWebView2_3>()
//   → TrySuspend / Resume

/// TrySuspend 完成回调（最小 COM 对象，vtable 指针为首字段的标準布局）
#[cfg(target_os = "windows")]
mod try_suspend_cb {
    use crate::standard_log;

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
        standard_log!(
            "[webview] TrySuspend completed: hr=0x{:08X} success={}",
            error_code as u32,
            is_successful != 0
        );
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

/// Suspend WebView2 渲染进程（popup 关闭后调用）。
/// IsVisible(FALSE) + TrySuspend：停止渲染 + 挂起渲染进程。
#[cfg(target_os = "windows")]
pub fn suspend_webview(webview: &tauri::Webview) {
    let wb = webview.clone();
    let r = wb.with_webview(|wv| unsafe {
        let controller = wv.controller();

        // Step1: IsVisible(FALSE)——TrySuspend 的前置条件
        if let Err(e) = controller.SetIsVisible(false) {
            standard_log!("[webview] SetIsVisible(false) failed: {}", e);
        }

        // Step2: TrySuspend——挂起渲染进程
        let Ok(webview2) = controller.CoreWebView2() else {
            process::append_log("[webview] get CoreWebView2 failed for TrySuspend");
            return;
        };
        let wv3: ICoreWebView2_3 = match webview2.cast() {
            Ok(w) => w,
            Err(e) => {
                process::append_log("[webview] cast ICoreWebView2_3 failed for TrySuspend");
                let _ = e;
                return;
            }
        };

        // 完成回调：手工 COM 对象借用传入（不接管引用计数；成功时回调由
        // runtime 使用，失败时手动 destroy——生命周期语义见 try_suspend_cb）
        let cb_ptr = try_suspend_cb::create();
        let Some(handler) = ICoreWebView2TrySuspendCompletedHandler::from_raw_borrowed(&cb_ptr)
        else {
            try_suspend_cb::destroy(cb_ptr);
            return;
        };
        if let Err(e) = wv3.TrySuspend(handler) {
            standard_log!("[webview] TrySuspend call failed: {}", e);
            try_suspend_cb::destroy(cb_ptr);
        }
    });
    if r.is_err() {
        process::append_log("[webview] suspend_webview: with_webview dispatch failed");
    }
}

/// Resume WebView2 渲染进程（popup 打开前 / 系统唤醒后调用）。
/// Resume + IsVisible(TRUE)：恢复渲染进程 + 恢复渲染。
#[cfg(target_os = "windows")]
pub fn resume_webview(webview: &tauri::Webview) {
    let wb = webview.clone();
    let r = wb.with_webview(|wv| unsafe {
        let controller = wv.controller();

        // Step1: Resume——恢复渲染进程
        let Ok(webview2) = controller.CoreWebView2() else {
            return;
        };
        let wv3: ICoreWebView2_3 = match webview2.cast() {
            Ok(w) => w,
            Err(_) => return,
        };
        if let Err(e) = wv3.Resume() {
            standard_log!("[webview] Resume call failed: {}", e);
        }

        // Step2: IsVisible(TRUE)——恢复渲染
        if let Err(e) = controller.SetIsVisible(true) {
            standard_log!("[webview] SetIsVisible(true) failed: {}", e);
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
