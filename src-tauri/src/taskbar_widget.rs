//! 任务栏信息窗（B3-A 路线）：把一块**自绘的分层子窗口**嵌进任务栏。
//!
//! ── 形态（选定 B3-A，理由见 PLAYBOOK §E 与 spike `B3-spike-结论.md`）────────
//!   1. **建窗**：`CreateWindowExW(WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE, .., WS_POPUP)`
//!      —— 此时还是**顶层窗**，`WS_EX_LAYERED` 不受「分层属性用于子窗口」的代际检查约束。
//!   2. **改样式**：清 `WS_POPUP`、加 `WS_CHILD`（`SetParent` **不会**自动改，MSDN 明确要求）。
//!   3. **挂载**：`SetParent(hwnd, Shell_TrayWnd)`。
//!   4. **自绘**：预乘 32bpp DIB + `UpdateLayeredWindow(ULW_ALPHA)`。
//!
//! ⛔⛔ **顺序敏感**：必须「先改样式、再 SetParent」。反过来实测 `err=87`
//!   （`INVALID_PARAMETER`）—— 别把它误归因于安全软件或清单。
//!
//! ⛔ **校验用「双判」**：`SetParent` 返回**前一个父窗**，成功时它本来就是 `NULL`
//!   ⇒ 单看返回值有歧义。判据 = 先 `SetLastError(0)`，再「返回 `NULL` **且** 错误码非 0」才算失败；
//!   最后再 `GetParent` 复核一次（TokenBar 的缺陷正是**不校验却直接标成功**）。
//!
//! ── 为什么不用「创建时直接带 `WS_CHILD + WS_EX_LAYERED`」──────────────────
//!   该形态在宿主清单**缺 `<compatibility><supportedOS>`** 时**恒定失败**且 `err=0`
//!   （实测矩阵见 PLAYBOOK §E「建窗前置」）。本仓虽已补清单（`src-tauri/app.manifest`），
//!   但 popup→reparent 路线**不依赖**它，兼容性更好，故仍选此形态。
//!
//! ── 透明与深浅色 ────────────────────────────────────────────────────────
//!   · 背景像素 `alpha = 0` ⇒ **全透明且穿透**（点击落到任务栏）；
//!   · 内容像素 `alpha = 255`；
//!   · 抗锯齿边缘**必须预乘** `(R·A/255, G·A/255, B·A/255, A)` —— 用 straight alpha
//!     会让边缘**过亮泛白**（实测）。
//!   · 内容色：浅色主题用**黑色**，深色主题用**白色**（复用 `windows::system_dark_mode()`）。
//!   · ⚠️ `UpdateLayeredWindow` 是**一次性提交位图**，不是持久绘制目标
//!     ⇒ 主题变化时**必须重新生成 DIB 再提交一次**。
//!
//! ── 本模块的里程碑 ──────────────────────────────────────────────────────
//!   当前实现 **里程碑 1**：建窗 + 挂载 + 自绘一块可辨识的内容（首帧验证色 + 后续真实内容）。
//!   刻意**不含**：多显示器（本机无 `Shell_SecondaryTrayWnd`，无法验证）、
//!   Explorer 重建自愈、Explorer 后 Z 序恢复 —— 这些是里程碑 2/3，见函数级 TODO。

// ⚠️ 本模块的**公共类型与判据**（`MountReport`）在非 Windows 上也必须存在，
//    否则 `#[cfg(not(windows))]` 的桩函数签名对不上。故**不**在文件顶写
//    `#![cfg(target_os = "windows")]`，而是逐项按需 cfg（模块内 Win32 代码统一走 `ffi`）。

/// 自绘内容的高度（逻辑像素）。宽度按内容计算。
///
/// ⚠️ 这是**唯一**的高度定义；`ffi` 模块通过 `use super::WIDGET_H` 引用它，
/// 不另开一份常量（两份硬编码会随改动漂移）。
#[cfg(target_os = "windows")]
const WIDGET_H: i32 = 22;

/// 诊断快照：`hwnd` / `SetParent` 错误码 / `GetParent` 复核结果的原始值。
///
/// ⚠️ 抽成类型而不是直接暴露静态量，是为了让「挂载是否成功」有**单一判据入口**
/// —— 免得调用方各自去读静态量、各自解释。
#[derive(Debug, Clone, Copy, Default)]
pub struct MountReport {
    pub hwnd: isize,
    pub reparent_err: isize,
    /// `GetParent` 的返回值（应等于 `Shell_TrayWnd` 句柄）
    pub parent: isize,
    /// `Shell_TrayWnd` 的句柄（比对用）
    pub taskbar: isize,
}

impl MountReport {
    /// 挂载成功的判据：**三重同时成立**。
    ///
    /// 1. `hwnd != 0`（窗口建出来了）
    /// 2. `reparent_err == 0`（`SetParent` 没报错）
    /// 3. `parent == taskbar` 且 `taskbar != 0`（`GetParent` 复核确实是任务栏）
    ///
    /// ⛔ 只看第 1、2 条不够：`SetParent` 可能「返回成功但没真挂上」
    ///   （TokenBar 就这样把失败标成了成功）。第 3 条是**结果复核**，不可省。
    pub fn ok(&self) -> bool {
        self.hwnd != 0 && self.reparent_err == 0 && self.taskbar != 0 && self.parent == self.taskbar
    }
}

#[cfg(target_os = "windows")]
use crate::process::{append_log, to_wide};
#[cfg(target_os = "windows")]
use std::sync::atomic::{AtomicIsize, Ordering};

/// widget 的窗口句柄（0 = 未创建）。供诊断与后续维护线程读取。
#[cfg(target_os = "windows")]
static WIDGET_HWND: AtomicIsize = AtomicIsize::new(0);

/// `SetParent` 的错误码（0 = 未报错）。**这是「挂载是否成功」的唯一可靠判据之一**。
#[cfg(target_os = "windows")]
static REPARENT_ERR: AtomicIsize = AtomicIsize::new(0);

/// 诊断用的 FFI 集合。集中在一处便于核对「到底调了哪些 API」。
#[cfg(target_os = "windows")]
mod ffi {
    use super::WIDGET_H;
    use windows_sys::Win32::Foundation::{HWND, POINT, SIZE};
    use windows_sys::Win32::Graphics::Gdi::{
        CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, SelectObject, BITMAPINFO,
        BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, HBITMAP, HDC, HGDIOBJ,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DestroyWindow, GetWindowLongPtrW, RegisterClassW,
        SetParent, SetWindowLongPtrW, SetWindowPos, ShowWindow, UpdateLayeredWindow, GWL_STYLE,
        HWND_TOPMOST, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, ULW_ALPHA, WNDCLASSW,
        WS_CHILD, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_POPUP,
    };

    /// 注册 widget 窗口类（幂等）。返回类名（`to_wide` 后的指针由调用方持有）。
    ///
    /// ⚠️ 类**只需注册一次**；重复注册同名类会失败（`ERROR_CLASS_ALREADY_EXISTS`）。
    /// 这里丢弃 atom：窗口用**类名**（而非 atom）创建，且进程生命周期内只注册一次，
    /// 故 atom 没有消费方（保留字段会触发 `dead_code` 闸门）。
    pub unsafe fn register_class(class_name_wide: *const u16) {
        // ⛔⛔ 「可见四条件」的第 4 条：**类必须有背景刷**，且 `WM_ERASEBKGND` 必须放行。
        //    用 `GetStockObject(WHITE_BRUSH)` 而非 `null_mut()` —— 实测传 NULL 时
        //    `UpdateLayeredWindow` 提交的位图**完全不显示**（整块 widget 保持全透明，
        //    连 alpha=255 的内容块也看不见）。ULW 会覆盖整面，故刷子颜色本身无所谓。
        let brush = windows_sys::Win32::Graphics::Gdi::GetStockObject(
            windows_sys::Win32::Graphics::Gdi::WHITE_BRUSH,
        );
        let wc = WNDCLASSW {
            style: 0,
            lpfnWndProc: Some(wnd_proc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: std::ptr::null_mut(),
            hIcon: std::ptr::null_mut(),
            hCursor: std::ptr::null_mut(),
            hbrBackground: brush as _,
            lpszMenuName: std::ptr::null(),
            lpszClassName: class_name_wide,
        };
        // ⚠️ 重复注册同名类会失败（`ERROR_CLASS_ALREADY_EXISTS`）；用**类名**（非 atom）
        //    建窗，故这里只关心「注册是否发生过」，不消费返回值。
        //    进程内只调用一次（见 `spawn_widget` 的调用路径）。
        RegisterClassW(&wc);
    }

    pub unsafe fn create_popup(class_name_wide: *const u16) -> HWND {
        CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            class_name_wide,
            std::ptr::null(),
            WS_POPUP,
            0,
            0,
            1,
            WIDGET_H,
            std::ptr::null_mut(), // 父窗：先建顶层
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    }

    /// 用 `WS_POPUP → WS_CHILD` 的样式切换（**必须在 `SetParent` 之前**）+
    /// `SetParent` 本身。返回 (旧父窗, 错误码)。
    pub unsafe fn reparent(hwnd: HWND, taskbar: HWND) -> (HWND, u32) {
        // ⛔ 顺序：先改样式
        let style = GetWindowLongPtrW(hwnd, GWL_STYLE) as u32;
        SetWindowLongPtrW(
            hwnd,
            GWL_STYLE,
            ((style & !(WS_POPUP as u32)) | (WS_CHILD as u32)) as isize,
        );
        // ⛔ 双判：SetParent 返回「前一个父窗」，NULL 有歧义 ⇒ 先清错误码
        windows_sys::Win32::Foundation::SetLastError(0);
        let old = SetParent(hwnd, taskbar);
        let err = windows_sys::Win32::Foundation::GetLastError();
        (old, err)
    }

    /// 让分层窗口真正显示（可见四条件里的「调一次 SetWindowPos」）。
    pub unsafe fn show(hwnd: HWND) {
        SetWindowPos(
            hwnd,
            HWND_TOPMOST,
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_NOZORDER,
        );
        ShowWindow(hwnd, 5 /* SW_SHOW */);
    }

    pub unsafe fn destroy(hwnd: HWND) {
        DestroyWindow(hwnd);
    }

    // ── DIB / ULW ──────────────────────────────────────────────────────
    pub struct Dib {
        pub memdc: HDC,
        pub bitmap: HBITMAP,
        pub old: HGDIOBJ,
        /// 像素缓冲起始地址（BGRA 顺序、**预乘**）
        pub bits: *mut u32,
        pub w: i32,
        pub h: i32,
    }

    /// 建 32bpp 顶下（top-down）DIB 段。**top-down** 让 `bits` 的第 0 行就是视觉第一行。
    pub unsafe fn create_dib(w: i32, h: i32) -> Option<Dib> {
        let screen = windows_sys::Win32::Graphics::Gdi::GetDC(std::ptr::null_mut());
        let memdc = CreateCompatibleDC(screen);
        windows_sys::Win32::Graphics::Gdi::ReleaseDC(std::ptr::null_mut(), screen);
        if memdc.is_null() {
            return None;
        }
        let mut bmi: BITMAPINFO = std::mem::zeroed();
        bmi.bmiHeader = BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: w,
            // ⛔ 负高度 = top-down DIB
            biHeight: -h,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB,
            ..std::mem::zeroed()
        };
        let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
        let bitmap = CreateDIBSection(
            memdc,
            &bmi,
            DIB_RGB_COLORS,
            &mut bits,
            std::ptr::null_mut(),
            0,
        );
        if bitmap.is_null() || bits.is_null() {
            DeleteDC(memdc);
            return None;
        }
        let old = SelectObject(memdc, bitmap);
        Some(Dib {
            memdc,
            bitmap,
            old,
            bits: bits as *mut u32,
            w,
            h,
        })
    }

    pub unsafe fn free_dib(d: &Dib) {
        SelectObject(d.memdc, d.old);
        DeleteObject(d.bitmap);
        DeleteDC(d.memdc);
    }

    /// 提交 DIB 到分层窗口（`ULW_ALPHA`，**要求预乘**）。
    pub unsafe fn commit(hwnd: HWND, d: &Dib, x: i32, y: i32) -> bool {
        let dst = POINT { x, y };
        let size = SIZE { cx: d.w, cy: d.h };
        let src = POINT { x: 0, y: 0 };
        let blend = windows_sys::Win32::Graphics::Gdi::BLENDFUNCTION {
            BlendOp: 0, // AC_SRC_OVER
            BlendFlags: 0,
            SourceConstantAlpha: 255,
            // ⛔ 必须 AC_SRC_ALPHA(1)：告诉系统源数据**带 per-pixel alpha**
            AlphaFormat: 1,
        };
        UpdateLayeredWindow(
            hwnd,
            std::ptr::null_mut(),
            &dst,
            &size,
            d.memdc,
            &src,
            0,
            &blend,
            ULW_ALPHA,
        ) != 0
    }

    /// 空 WndProc：分层窗口不靠 `WM_PAINT` 绘制（那是 `ULW` 的活），
    /// 但也**不能**把 `WM_ERASEBKGND` 返回 0（会破坏「可见四条件」），故一律交 `DefWindowProcW`。
    unsafe extern "system" fn wnd_proc(hwnd: HWND, msg: u32, wp: usize, lp: isize) -> isize {
        DefWindowProcW(hwnd, msg, wp, lp)
    }
}

/// 挂载 widget 到任务栏（**里程碑 1**：建窗 + 挂载 + 自绘一帧）。
///
/// 返回 `MountReport`，调用方用 `report.ok()` 判定成败并落日志。
/// ⚠️ 本函数**必须在常驻泵消息的线程上调用**（窗口随创建线程退出而销毁）。
#[cfg(target_os = "windows")]
pub fn spawn_widget() -> MountReport {
    let mut report = MountReport {
        hwnd: 0,
        reparent_err: 0,
        parent: 0,
        taskbar: 0,
    };

    // 1) 找任务栏
    let taskbar = unsafe {
        windows_sys::Win32::UI::WindowsAndMessaging::FindWindowW(
            to_wide("Shell_TrayWnd").as_ptr(),
            std::ptr::null(),
        )
    };
    report.taskbar = taskbar as isize;
    if taskbar.is_null() {
        append_log("[widget] Shell_TrayWnd 未找到（Explorer 重启间隙？）⇒ 放弃本次挂载");
        return report;
    }

    // 2) 注册窗口类（幂等）
    let class_name = to_wide("PeriTrayTaskbarWidget");
    unsafe {
        ffi::register_class(class_name.as_ptr());
    }

    // 3) 建 popup（带 WS_EX_LAYERED）
    let hwnd = unsafe { ffi::create_popup(class_name.as_ptr()) };
    report.hwnd = hwnd as isize;
    if hwnd.is_null() {
        let err = unsafe { windows_sys::Win32::Foundation::GetLastError() };
        append_log(&format!("[widget] CreateWindowExW 失败: err={}", err));
        return report;
    }

    // 4) 改样式 + SetParent（**顺序敏感**）
    let (old_parent, err) = unsafe { ffi::reparent(hwnd, taskbar) };
    report.reparent_err = err as isize;
    // 双判：只有「返回 NULL 且 err != 0」才算失败；成功时旧父窗本来就是 NULL
    if old_parent.is_null() && err != 0 {
        append_log(&format!("[widget] SetParent 失败: err={}", err));
    }

    // 5) GetParent 复核（TokenBar 缺的就是这步）
    let parent = unsafe { windows_sys::Win32::UI::WindowsAndMessaging::GetParent(hwnd) };
    report.parent = parent as isize;

    // 6) 自绘一帧（透明背景 + 内容）
    let drawn = draw_frame(hwnd, 0, 0);

    // 7) 显示（可见四条件之一：调一次 SetWindowPos）
    unsafe { ffi::show(hwnd) };

    WIDGET_HWND.store(hwnd as isize, Ordering::SeqCst);
    REPARENT_ERR.store(err as isize, Ordering::SeqCst);

    append_log(&format!(
        "[widget] mount report: hwnd={:#x} taskbar={:#x} reparent_err={} getparent={:#x} ok={} drawn={}",
        report.hwnd,
        report.taskbar,
        report.reparent_err,
        report.parent,
        report.ok(),
        drawn
    ));
    report
}

/// 自绘并提交一帧。
///
/// `x` / `y` 是相对父窗（任务栏）客户区的坐标。**里程碑 1 只画一块可辨识的内容块**，
/// 用于让真机截图能一眼确认「挂上了、且透明背景正确」。
#[cfg(target_os = "windows")]
fn draw_frame(hwnd: *mut core::ffi::c_void, x: i32, y: i32) -> bool {
    let w = 120;
    let h = WIDGET_H;
    let Some(dib) = (unsafe { ffi::create_dib(w, h) }) else {
        append_log("[widget] CreateDIBSection 失败");
        return false;
    };

    // ⛔ 内容色：浅色主题黑、深色主题白（复用既有 system_dark_mode，不写第二套判定）
    let dark = crate::windows::system_dark_mode();
    let content: u32 = if dark { 0x00FF_FFFF } else { 0x0000_0000 }; // 0x00RRGGBB（不含 alpha）

    unsafe {
        let px = std::slice::from_raw_parts_mut(dib.bits, (w * h) as usize);
        // 背景：alpha=0 ⇒ 全透明且穿透
        for p in px.iter_mut() {
            *p = 0;
        }
        // 内容：画一个 8x8 的不透明方块（左上角内缩 4px），**预乘** = 原色（alpha=255 时预乘等于原色）
        let (r, g, b) = (
            (content >> 16) & 0xFF,
            (content >> 8) & 0xFF,
            content & 0xFF,
        );
        let packed = (0xFFu32 << 24) | (r << 16) | (g << 8) | b;
        for yy in 4..(4 + 8) {
            for xx in 4..(4 + 8) {
                px[(yy * w + xx) as usize] = packed;
            }
        }
    }

    let ok = unsafe { ffi::commit(hwnd as _, &dib, x, y) };
    if !ok {
        let err = unsafe { windows_sys::Win32::Foundation::GetLastError() };
        append_log(&format!("[widget] UpdateLayeredWindow 失败: err={}", err));
    }
    unsafe { ffi::free_dib(&dib) };
    ok
}

/// 诊断：读回当前 widget 的挂载状态（不新建窗口）。
///
/// ⭐ 这是**独立于 `spawn_widget` 返回值**的第二判据：`spawn_widget` 读的是「建窗那一刻」
/// 的快照，而本函数**重新 `FindWindowW` + `GetParent`** ⇒ 能发现「挂载后又掉了」
/// （Explorer 重建、任务栏被替换等）。
#[cfg(target_os = "windows")]
pub fn probe() -> MountReport {
    let hwnd = WIDGET_HWND.load(Ordering::SeqCst) as *mut core::ffi::c_void;
    let taskbar = unsafe {
        windows_sys::Win32::UI::WindowsAndMessaging::FindWindowW(
            to_wide("Shell_TrayWnd").as_ptr(),
            std::ptr::null(),
        )
    };
    let parent = if hwnd.is_null() {
        std::ptr::null_mut()
    } else {
        unsafe { windows_sys::Win32::UI::WindowsAndMessaging::GetParent(hwnd) }
    };
    MountReport {
        hwnd: hwnd as isize,
        reparent_err: REPARENT_ERR.load(Ordering::SeqCst),
        parent: parent as isize,
        taskbar: taskbar as isize,
    }
}

/// 销毁 widget 并**清掉句柄记录**（使后续 `probe()` 明确返回「未挂载」）。
///
/// 用途：① 诊断收尾（`PM_DEV_TASKBAR_WIDGET_DESTROY` 门控）；
///       ② 将来「用户在设置页关掉任务栏显示」时的拆除路径。
#[cfg(target_os = "windows")]
pub fn destroy_widget() {
    let hwnd = WIDGET_HWND.swap(0, Ordering::SeqCst) as *mut core::ffi::c_void;
    REPARENT_ERR.store(0, Ordering::SeqCst);
    if !hwnd.is_null() {
        unsafe { ffi::destroy(hwnd) };
        append_log("[widget] destroyed");
    }
}

#[cfg(not(target_os = "windows"))]
pub fn spawn_widget() -> MountReport {
    MountReport::default()
}

#[cfg(not(target_os = "windows"))]
pub fn probe() -> MountReport {
    MountReport::default()
}

#[cfg(not(target_os = "windows"))]
pub fn destroy_widget() {}
