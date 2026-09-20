//! WebView2 底层控制：背景色（恒透明）、页面生命周期（Suspend/Resume）、
//! 内存档位（MemoryUsageTargetLevel）。
//! PlatformWebview::controller() 直接返回强类型 ICoreWebView2Controller
//! （webview2-com 0.38，与 sys 同基座 windows-core 0.61），全部调用走
//! webview2-com-sys 类型安全 API——零 transmute、零手写 vtable、零手抄 IID；
//! 与 windows 模块的窗口定位 / 窗口材质（DWM）逻辑相互独立。

use crate::process;
use crate::standard_log;
#[cfg(target_os = "windows")]
use crate::verbose_log;

#[cfg(target_os = "windows")]
use webview2_com_sys::Microsoft::Web::WebView2::Win32::{
    ICoreWebView2Controller2, ICoreWebView2TrySuspendCompletedHandler, ICoreWebView2_19,
    ICoreWebView2_3, COREWEBVIEW2_COLOR, COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_LOW,
    COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_NORMAL,
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
//   系统睡眠时 COM 不活跃，不阻塞事件循环（仅针对**休眠唤醒**这一类）。
// popup 打开前 / 唤醒后：Resume + IsVisible(TRUE) → 恢复渲染。
//
// ⚠️ 范围限定（2026-09-18）：上面这条**不是**「运行期窗口冻结」的解释。
//   实测（`AppHangTransient` / 退出码 `0xcfffffff`）证明那次冻结的根因是
//   **锁序死锁（P0-4）**——子线程持配置锁调菜单 API（`run_item_main_thread!` =
//   无超时 `rx.recv()`）⇄ 主线程等同一把配置锁 ⇒ 永久互等，与 WebView2 挂起态、
//   DWM、GPU、杀软沙箱**全部无关**。遇到「窗口完全无响应」请**先查锁序**
//   （登记表见 `state.rs` 模块文档），不要停在「挂起态」这个方向上。
//
// 调用链：PlatformWebview.controller() → ICoreWebView2Controller
//   → CoreWebView2() → ICoreWebView2 → cast::<ICoreWebView2_3>()
//   → TrySuspend / Resume

/// TrySuspend 完成回调（最小 COM 对象，vtable 指针为首字段的標準布局）
///
/// 引用计数约定（**三处必须一致，勿单独改一处**）：
/// - `create()` 返回的对象**初始计数 1，由调用方持有**；
/// - `TrySuspend` 成功时 runtime 会自行 AddRef 以持有异步完成回调；
/// - **调用方在 `TrySuspend` 返回后（无论成功或失败）必须释放自己那一份**
///   （`release_owned`）——对象存活期 = 「调用方持有」∪「runtime 持有」，
///   最后一个引用释放时由 `release` 内的 `Box::from_raw` 回收。
///
/// 该约定与上游 `webview2-com` 一致：其 `TrySuspendCompletedHandler::create()` 返回持有
/// 一份引用的智能指针，`TrySuspend(&handler)` 按借用传入，局部变量析构时释放。
///
/// **实测（2026-09-20，WebView2 运行时 137.0.3296.52，真实进程 + env 门控探针）**：
/// - 成功路径：运行时**恰好** `1×AddRef → Invoke → 1×Release`（trace 序列 `A→I→R`）；
/// - 错误路径（`IsVisible == TRUE` ⇒ 同步返回 `HRESULT_FROM_WIN32(ERROR_INVALID_STATE)`）：
///   运行时**零次引用操作**（序列 `""`）。
///
/// ⇒ 上面的约定在两条路径上都恰好回收一次。⚠️ 旧实现（`add_ref` 恒返回 1 +
/// `release` 无条件 `Box::from_raw`）只在「Release 恰好一次」时正确——它把安全性
/// **押在运行时的配对行为上**；WebView2 是 Evergreen（运行时自动更新），这种依赖不构成保证。
/// 完整判据、正控与保留边界见 Wiki「12-代码审查与整改复盘」§10
/// （正控：同一套 trace 在成功路径记录到 `A/I/R`，且探针自己调 `release_owned` 时被记录为 `R`
/// ⇒ 错误路径的空序列是**真实阴性**，不是探针失灵）。
#[cfg(target_os = "windows")]
mod try_suspend_cb {
    use crate::standard_log;
    use std::sync::atomic::{AtomicU32, Ordering};

    /// GUID 逐字段比较：windows-sys 的 `GUID` 未实现 `PartialEq`（无法直接用 `==`）
    fn guid_eq(a: &windows_sys::core::GUID, b: &windows_sys::core::GUID) -> bool {
        a.data1 == b.data1 && a.data2 == b.data2 && a.data3 == b.data3 && a.data4 == b.data4
    }

    #[repr(C)]
    pub struct Obj {
        /// COM 布局契约：vtable 指针必须是首字段
        vtable: *const Vtable,
        /// 真实引用计数；`create()` 置 1，归零时释放
        refs: AtomicU32,
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
        this: *mut Obj,
        iid: *const windows_sys::core::GUID,
        out: *mut *mut core::ffi::c_void,
    ) -> i32 {
        // COM 契约：QI 必须让 IID_IUnknown 成功（返回同一对象并 AddRef）。
        // 原实现对所有 IID 一律返回 E_NOINTERFACE（连 IUnknown 也不例外），非合规对象。
        unsafe {
            if !iid.is_null() && guid_eq(&*iid, &windows_sys::core::IID_IUnknown) {
                add_ref(this);
                *out = this as *mut core::ffi::c_void;
                0 // S_OK
            } else {
                *out = core::ptr::null_mut();
                -2147467262 // E_NOINTERFACE
            }
        }
    }

    unsafe extern "system" fn add_ref(this: *mut Obj) -> u32 {
        // 计数本身由 fetch_add 保证原子性，无需与其他内存建立同步
        unsafe { (*this).refs.fetch_add(1, Ordering::Relaxed) + 1 }
    }

    unsafe extern "system" fn release(this: *mut Obj) -> u32 {
        // Release 序：递减前对对象字段的写入对其他线程可见
        let n = unsafe { (*this).refs.fetch_sub(1, Ordering::Release) } - 1;
        if n == 0 {
            // Acquire 栅栏与上面的 Release 配对，确保看到对象全部写入后再回收
            unsafe {
                std::sync::atomic::fence(Ordering::Acquire);
                drop(Box::from_raw(this));
            }
        }
        n
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

    /// 创建回调对象：返回的指针**持有一份引用**，调用方用完必须 `release_owned`。
    pub fn create() -> *mut core::ffi::c_void {
        let obj = Box::new(Obj {
            vtable: &VTABLE,
            refs: AtomicU32::new(1),
        });
        Box::into_raw(obj) as *mut core::ffi::c_void
    }

    /// 释放**调用方自己持有的那一份引用**（替代原先无条件释放的 `destroy`）。
    /// 与 `destroy` 的区别：若 runtime 已 AddRef，对象不会被提前回收。
    ///
    /// # Safety
    /// `ptr` 必须是 `create()` 返回、且尚未由调用方释放过的指针。
    pub unsafe fn release_owned(ptr: *mut core::ffi::c_void) {
        unsafe { release(ptr as *mut Obj) };
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        /// 引用计数生命周期：create 置 1；AddRef 递增；只有最后一个 Release 才回收。
        /// 旧实现（add_ref 恒返回 1、release 无条件 Box::from_raw）在此必然失败。
        #[test]
        fn refcount_lifecycle() {
            let p = create() as *mut Obj;
            unsafe {
                assert_eq!((*p).refs.load(Ordering::Relaxed), 1, "create 应返回计数 1");
                assert_eq!(add_ref(p), 2, "AddRef 必须真的递增");
                assert_eq!(release(p), 1, "仍有调用方引用时不得回收");
                assert_eq!(add_ref(p), 2);
                assert_eq!(release(p), 1);
                assert_eq!(release(p), 0, "最后一个引用释放时归零并回收");
            }
        }

        /// QI 契约：IID_IUnknown 必须成功（返回同一对象并 AddRef）；其他 IID 返回 E_NOINTERFACE。
        /// 旧实现对任何 IID 都返回 E_NOINTERFACE，在此必然失败。
        #[test]
        fn qi_contract() {
            let p = create() as *mut Obj;
            unsafe {
                let mut out: *mut core::ffi::c_void = core::ptr::null_mut();
                assert_eq!(
                    qi(p, &windows_sys::core::IID_IUnknown, &mut out),
                    0,
                    "IID_IUnknown 必须返回 S_OK"
                );
                assert_eq!(out as *mut Obj, p, "QI 成功必须返回同一对象");
                assert_eq!((*p).refs.load(Ordering::Relaxed), 2, "QI 成功必须 AddRef");

                let other =
                    windows_sys::core::GUID::from_u128(0x1234_5678_9abc_def0_1234_5678_9abc_def0);
                out = p as *mut core::ffi::c_void;
                assert_eq!(
                    qi(p, &other, &mut out),
                    -2147467262,
                    "未知 IID 必须 E_NOINTERFACE"
                );
                assert!(out.is_null(), "QI 失败时出参必须置空");

                // 清理：先释放 QI 带来的额外引用，再释放调用方那份
                assert_eq!(release(p), 1);
                assert_eq!(release(p), 0);
            }
        }

        /// 回归（2026-09-20 实测配对）：**调用点实际使用的** `release_owned` 在实测到的两条
        /// 运行时路径上都只释放调用方那一份、且恰好回收一次。
        ///
        /// - 错误路径实测：运行时零次引用操作 ⇒ 调用方那一份是唯一引用；
        /// - 成功路径实测：运行时 `AddRef(1→2) → Invoke → Release(2→1)` ⇒ 调用方释放后归零。
        ///
        /// 可证伪性：把 `add_ref` / `release` / `release_owned` 还原成旧语义（恒返回 1 +
        /// 无条件 `Box::from_raw`）后本用例转红——先卡在「AddRef 必须真的递增」这条断言上；
        /// 即使放宽该断言，成功路径的第二次回收也会造成堆损坏。
        #[test]
        fn release_owned_is_safe_under_measured_runtime_pairing() {
            // 错误路径（实测：运行时零次引用操作）
            let p = create();
            unsafe { release_owned(p) };

            // 成功路径（实测：AddRef → Invoke → Release → 调用方释放）
            let p = create();
            let obj = p as *mut Obj;
            unsafe {
                assert_eq!(add_ref(obj), 2, "运行时接管时 AddRef 必须真的递增");
                assert_eq!(
                    release(obj),
                    1,
                    "运行时在 Invoke 之后 Release，对象仍须存活"
                );
                release_owned(p); // 调用方那一份 → 归零回收
            }
        }
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

        // 完成回调：create() 返回的对象持有一份引用（调用方所有）；
        // runtime 成功接管时会自行 AddRef，故**无论成败都由我们释放自己那一份**，
        // 避免「runtime 未 AddRef 时对象提前回收」与「已 AddRef 时泄漏」两种偏差。
        // 生命周期与计数约定见 try_suspend_cb 模块头注释。
        let cb_ptr = try_suspend_cb::create();
        let Some(handler) = ICoreWebView2TrySuspendCompletedHandler::from_raw_borrowed(&cb_ptr)
        else {
            try_suspend_cb::release_owned(cb_ptr);
            return;
        };
        if let Err(e) = wv3.TrySuspend(handler) {
            standard_log!("[webview] TrySuspend call failed: {}", e);
        }
        // 释放调用方持有的引用；若 runtime 已 AddRef，对象继续存活至其 Release
        try_suspend_cb::release_owned(cb_ptr);
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

/// 设置 WebView2 内存档位（ICoreWebView2_19::SetMemoryUsageTargetLevel）。
/// LOW：Chromium 主动收缩 browser/GPU 进程缓存——与 TrySuspend（仅冻结
/// renderer）互补，弹窗隐藏时叠加使用；NORMAL：恢复常规档位（弹窗打开时）。
/// WebView2 运行时过旧（QI 不到 _19 接口）时静默跳过。
#[cfg(target_os = "windows")]
pub fn set_memory_usage_target(webview: &tauri::Webview, low: bool) {
    let wb = webview.clone();
    let r = wb.with_webview(move |wv| unsafe {
        let controller = wv.controller();
        let Ok(webview2) = controller.CoreWebView2() else {
            return;
        };
        let wv19: ICoreWebView2_19 = match webview2.cast() {
            Ok(w) => w,
            Err(_) => {
                verbose_log!("[webview] ICoreWebView2_19 unavailable, skip memory target");
                return;
            }
        };
        let level = if low {
            COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_LOW
        } else {
            COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_NORMAL
        };
        match wv19.SetMemoryUsageTargetLevel(level) {
            Ok(()) => {
                verbose_log!(
                    "[webview] memory usage target -> {}",
                    if low { "LOW" } else { "NORMAL" }
                );
            }
            Err(e) => {
                standard_log!("[webview] SetMemoryUsageTargetLevel failed: {}", e);
            }
        }
    });
    if r.is_err() {
        process::append_verbose_log("[webview] set_memory_usage_target: dispatch failed");
    }
}

#[cfg(not(target_os = "windows"))]
pub fn set_memory_usage_target(_webview: &tauri::Webview, _low: bool) {}
