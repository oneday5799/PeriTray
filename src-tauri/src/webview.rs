//! WebView2 底层控制：背景色（恒透明）与页面生命周期（Suspend/Resume）。
//! 全部经 PlatformWebview.controller() 原始 COM vtable 调用，
//! 与 windows 模块的窗口定位 / 窗口材质（DWM）逻辑相互独立。

use crate::process;

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
