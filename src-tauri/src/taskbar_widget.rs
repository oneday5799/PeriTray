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
//!   里程碑 1（已完成）：建窗 + 挂载 + 自绘一块可辨识的内容。
//!   里程碑 2+3（已完成）：接入真实内容 + 由设置页驱动（挂载 / 拆除 / 重定位）。
//!   里程碑 4（本次）：**布局重构** —— 图标放大到 32px，电量画图标**右上角**、
//!     音量画**右下角**，缺失一律 `N/A`；widget 在任务栏内**垂直居中**。
//!   刻意**不含**：多显示器（本机无 `Shell_SecondaryTrayWnd`，无法验证）、
//!   Explorer 重建自愈、Explorer 后 Z 序恢复、拖拽窗口 —— 见函数级 TODO。
//!
//! ── ⛔⛔ 线程模型（**本模块最容易做错的地方**）───────────────────────────
//!   两条**硬约束**彼此冲突，必须用「后台取数 → 投递主线程 → 主线程重绘」化解：
//!     1. **窗口过程只在创建线程上被调用**，而 `UpdateLayeredWindow` 更新的是窗口表面
//!        ⇒ **重绘只能在主线程**（widget 建在 Tauri `setup` 回调 = 主线程）。
//!     2. **数据获取极慢**（WMI 设备枚举实测 600ms+，见 PLAYBOOK §E）
//!        ⇒ **取数绝不能在主线程**，否则任务栏/整个 UI 卡顿。
//!   ⇒ 数据流：事件/定时线程 → `refresh_async()`（后台取数）
//!     → 写入 `SNAPSHOT`（`Mutex<Option<WidgetSnapshot>>`）
//!     → `PostMessageW(hwnd, WM_APP_REFRESH)`（**跨线程安全**，不阻塞）
//!     → 主线程 `wnd_proc` 收消息 → 读 `SNAPSHOT` → `draw_frame()`。
//!
//!   ⛔ 为什么用自定义消息而不是 `InvalidateRect` + `WM_PAINT`：
//!     分层窗口**不走 `WM_PAINT`**（`ULW` 提交的是整张位图，不是绘制目标），
//!     故自定义消息语义更准，也避免与 `DefWindowProcW` 的擦除逻辑纠缠。
//!
//! ── 刷新策略（用户选定）─────────────────────────────────────────────────
//!   · **事件驱动**：监听已有的全局 `emit` 事件（`config-changed` /
//!     `tray-devices-changed` / `audio-devices-changed` / `volume-changed` 等）
//!     ⇒ 数据一变立即重绘。
//!   · **防抖**：`volume-changed` 在拖动音量条时**连续触发**（实测），
//!     每次都拉一次 WMI 会打爆 CPU ⇒ 用**合并窗口**（`REFRESH_PENDING` 标志），
//!     窗口内的多次请求只取数一次。
//!   · **低频兜底**：30s 一次无条件刷新，覆盖「没有事件但数据变了」的情况
//!     （例如蓝牙电量自然衰减）。

// ⚠️ 本模块的**公共类型与判据**（`MountReport`）在非 Windows 上也必须存在，
//    否则 `#[cfg(not(windows))]` 的桩函数签名对不上。故**不**在文件顶写
//    `#![cfg(target_os = "windows")]`，而是逐项按需 cfg（模块内 Win32 代码统一走 `ffi`）。

/// 自绘内容的高度（物理像素）。宽度按内容计算。
///
/// ⭐ 取 40 的理由：真机实测任务栏高 **60px**（2560×1440 @125% 缩放）⇒ 上下各余
///   10px；同时它刚好容纳「32px 图标 + 右侧两行文本」（`ICON_PX = 32`）。
///   任务栏内**垂直居中**（见 `widget_y_offset`），使图标与任务栏自身图标同一水平线。
///
/// ⚠️ 这是**唯一**的高度定义；`ffi` 模块通过 `use super::WIDGET_H` 引用它，
/// 不另开一份常量（两份硬编码会随改动漂移）。
#[cfg(target_os = "windows")]
const WIDGET_H: i32 = 40;

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

// ══════════════════════════════════════════════════════════════════════════
// 数据快照（后台取数 → 主线程重绘 的**唯一载荷**）
// ══════════════════════════════════════════════════════════════════════════
//
// ⭐ 刻意定义**本模块自己的**条目类型，而不是直接传 `PhysicalDevice`：
//   自绘只关心「要画成什么样」的**少数几个字段**（名 / 电量 / 音量 / 静音 / 是否在放声），
//   直接依赖后端领域类型会让「UI 细节」与「数据聚合」耦合，
//   将来 `PhysicalDevice` 加字段就会牵动绘制代码。

/// 一台设备在 widget 上要显示的**全部**信息。
///
/// ⚠️ 这里的 `Option` **保留三态**，与后端一致：`None` = 读不出（显示占位符），
///    `Some(0)` = 真的是 0（电量耗尽 / 音量静音）——**二者显示必须不同**。
#[derive(Debug, Clone, PartialEq)]
pub struct WidgetItem {
    /// 展示名（已按物理设备身份聚合过的名字）
    ///
    /// ⚠️ 曾用于画在 widget 上；用户 2026-09-24 明确「**设备名以后再说**」
    ///   ⇒ 当前**不显示**，但字段保留（诊断日志、将来恢复显示都要用）。
    pub name: String,
    /// 画哪个图标（鼠标 / 喇叭 / 耳机）。由 `audio_endpoint_name` 归一化而来。
    pub icon: crate::device_identity::AudioKind,
    /// 电量百分比；`None` = 读不出（如纯 USB 音频设备）
    pub battery: Option<i32>,
    /// 音量 `0.0`–`1.0`；`None` = 该设备没有音频端点
    pub volume: Option<f32>,
    /// 是否有音频端点（决定「画不画音量」）。与 `volume.is_some()` 冗余但**语义不同**：
    /// 音量可能**暂时**读不出（`volume=None` 而端点存在），此时应画 `--` 而不是不画。
    pub has_audio: bool,
    /// 端点是否静音（画成 `🔇` 或降低不透明度）
    pub is_muted: Option<bool>,
    /// 是否是系统默认音频设备（前缀标记，帮助用户一眼认出放声口）
    pub is_default: bool,
    /// 用户固定的设备（读不出数据也保留，置灰显示）
    pub pinned: bool,
}

// ⚠️ 快照**只在 Windows 上有消费方**（非 Windows 的 `spawn_widget` 是空桩），
//    故整体 cfg 掉，免得在非 Windows 构建里触发 `dead_code` 闸门。
#[cfg(target_os = "windows")]
mod snapshot {
    use super::WidgetItem;
    use std::sync::Mutex;

    /// 最近一次成功取到的数据快照。主线程 `wnd_proc` 在收到刷新消息时**克隆一份**再绘制
    /// （不持锁绘制：绘制可能耗时，持锁会阻塞后台取数线程）。
    static SNAPSHOT: Mutex<Option<Vec<WidgetItem>>> = Mutex::new(None);

    /// 写入新快照（**后台线程**调）。返回是否真的发生了变化。
    ///
    /// ⭐ 「无变化则不重绘」很重要：30s 兜底 + 事件驱动会产生大量「数据其实没变」的刷新，
    ///    每次都重建 DIB + `ULW` 是纯浪费（实测单帧 0.45ms，虽低但没必要）。
    pub fn store(items: Vec<WidgetItem>) -> bool {
        let mut guard = crate::state::lock_unpoisoned(&SNAPSHOT);
        if guard.as_ref() == Some(&items) {
            return false;
        }
        *guard = Some(items);
        true
    }

    /// 读一份快照副本（**主线程**调）。
    pub fn load() -> Option<Vec<WidgetItem>> {
        crate::state::lock_unpoisoned(&SNAPSHOT).clone()
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

// ── 刷新编排 ────────────────────────────────────────────────────────────

/// 投递给主线程的自定义消息：**「快照已更新，请重绘」**。
///
/// ⚠️ 取 `WM_APP + 1`（`WM_APP` = `0x8000`）—— 这是**应用私有消息**的规范起点，
///   系统不会占用；且与 `WM_PAINT` 不同，分层窗口会正常投递到我们的 `wnd_proc`。
#[cfg(target_os = "windows")]
const WM_APP_REFRESH: u32 = 0x8000 + 1;

/// 防抖状态：`true` = 已有一次刷新在路上（**合并窗口**）。
///
/// ⭐ 为什么需要它：`volume-changed` 在**拖动音量条时连续触发**（实测一个拖动几十次），
///    每次触发都去跑 WMI（600ms）+ 音频枚举，会把 CPU 打满、刷新互相挤压。
///    做法 = 「首次请求立即执行，执行期间的**所有**后续请求只置一个标志，
///    执行完再补跑一次」—— 保证**最终一致**（最后一次状态一定被画出来）且**不雪崩**。
#[cfg(target_os = "windows")]
static REFRESH_PENDING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// 是否已有**前置**的刷新在跑（防止重入）。
#[cfg(target_os = "windows")]
static REFRESH_RUNNING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// 诊断用：累计成功重绘的次数（用于验收「事件驱动确实生效」）。
#[cfg(target_os = "windows")]
static REFRESH_COUNT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// 后台扫描出的视觉空白区左端（物理像素）。主线程只读，不做 GetPixel。
#[cfg(target_os = "windows")]
static SLOT_X: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(100);
/// 上一次分配的槽宽；视觉扫描会把 widget 自己的旧内容排除，避免自我占用导致下一轮消失。
#[cfg(target_os = "windows")]
static SLOT_W: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(1);
/// 是否已有后台扫描结果；没有结果时主线程保持 widget 隐藏，避免首帧压住任务栏内容。
#[cfg(target_os = "windows")]
static SLOT_VALID: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// 「未固定位置」时沿用的上一次窗口左端（**相对任务栏客户区**）。
///
/// ⚠️ `0` 作**哨兵值**表示「还没画过」：任务栏左端恒为 0、槽起点恒 ≥ 100
///   ⇒ 真实的相对 x 不可能是 0，故哨兵不会与合法值撞车。
#[cfg(target_os = "windows")]
static LAST_X: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(0);

/// 「下一次刷新**必须**重绘」的一次性标志。
///
/// ⛔ 为什么需要它（真机实测缺陷）：`fetch_into_snapshot` 的判据是
///   「**数据变了** 或 **槽位移动了**」才重绘 —— 这是为了避免无意义重绘。
///   但**改贴靠位置**这两条都不满足：数据没变、槽位也没动，只是「同一个槽里画到哪儿」
///   变了 ⇒ 判据返回 `false` ⇒ 窗口**留在原地**（日志里只有一句「快照无变化」，
///   不报错、不崩溃，设置页看起来也保存成功了 —— 典型静默失效）。
///   ⇒ 配置变更路径必须能**显式要求重绘**，由 `fetch_into_snapshot` 消费后清零。
///
/// ⚠️ 位置计算本身发生在 `draw_items` 里（每次绘制都重读 `config`），
///   所以「重绘」就足以让新位置生效，无需额外搬运位置状态。
#[cfg(target_os = "windows")]
static FORCE_REPAINT: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// 诊断用的 FFI 集合。集中在一处便于核对「到底调了哪些 API」。
#[cfg(target_os = "windows")]
mod ffi {
    /// 常量：`DrawTextW` 的格式标志（`windows-sys` 未导出，按 WinUser.h 定义）。
    #[cfg(target_os = "windows")]
    const DT_LEFT: u32 = 0x0000_0000;
    #[cfg(target_os = "windows")]
    const DT_SINGLELINE: u32 = 0x0000_0020;
    #[cfg(target_os = "windows")]
    const DT_NOPREFIX: u32 = 0x0000_0800;
    #[cfg(target_os = "windows")]
    const DT_CALCRECT: u32 = 0x0000_0400;
    /// 超出矩形时用 `…` 截断，而不是**硬切半个字形**。
    ///
    /// ⚠️ 没有它会退化成「Steam Streaming Speake」（真机实测）—— 末字被切掉一半，
    ///   看起来像渲染 bug。有它则是「Steam Streami…」，一眼看出是**刻意截断**。
    #[cfg(target_os = "windows")]
    const DT_END_ELLIPSIS: u32 = 0x0000_8000;

    use super::{WIDGET_H, WM_APP_REFRESH};
    use windows_sys::Win32::Foundation::{HWND, POINT, SIZE};
    use windows_sys::Win32::Graphics::Gdi::{
        CreateCompatibleDC, CreateDIBSection, CreateFontW, DeleteDC, DeleteObject, DrawTextW,
        SelectObject, SetBkMode, SetTextColor, BITMAPINFO, BITMAPINFOHEADER, BI_RGB,
        DIB_RGB_COLORS, HBITMAP, HDC, HFONT, HGDIOBJ, TRANSPARENT,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DestroyWindow, GetWindowLongPtrW, PostMessageW,
        RegisterClassW, SetParent, SetWindowLongPtrW, SetWindowPos, ShowWindow,
        UpdateLayeredWindow, GWL_STYLE, HWND_TOPMOST, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE,
        SWP_NOZORDER, ULW_ALPHA, WNDCLASSW, WS_CHILD, WS_EX_LAYERED, WS_EX_NOACTIVATE,
        WS_EX_TOOLWINDOW, WS_POPUP,
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

    /// 空白区装不下时隐藏 widget；下一次重绘重新找到空白区后再显示。
    pub unsafe fn hide(hwnd: HWND) {
        ShowWindow(hwnd, 0 /* SW_HIDE */);
    }

    pub unsafe fn destroy(hwnd: HWND) {
        DestroyWindow(hwnd);
    }

    /// 跨线程通知「快照已更新，请主线程重绘」。
    ///
    /// ⛔ 必须用 `PostMessageW`（**异步投递**）而不是 `SendMessageW`（同步等待）：
    ///   后者会**阻塞后台线程直到主线程处理完**，而主线程可能在忙别的（例如正在
    ///   处理一次 600ms 的取数） ⇒ 后台刷新线程被拖住，防抖逻辑失效。
    /// ⚠️ 投递失败（窗口已销毁）静默忽略 —— 属「无状态后果」类，下一次兜底会补上。
    pub unsafe fn post_refresh(hwnd: HWND) {
        PostMessageW(hwnd, WM_APP_REFRESH, 0, 0);
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

    /// 一次性测量文本宽度（像素）。用 `DrawTextW` 的 `DT_CALCRECT`，不为实际绘制付代价。
    ///
    /// ⚠️ `DT_CALCRECT` 会**修改传入的 `RECT`**（写回测量结果）⇒ 必须传可写指针。
    /// ⚠️ **只在对齐方式与真实绘制一致时，测量值才对得上**；这里统一用
    ///   `DT_LEFT | DT_SINGLELINE | DT_NOPREFIX`（与 `draw_text` 完全一致），
    ///   否则会出现「算 40px、画 46px」的错位（文本被裁切）。
    pub unsafe fn measure_text(memdc: HDC, font: HFONT, text: &[u16]) -> i32 {
        let old = SelectObject(memdc, font as HGDIOBJ);
        let mut rc = windows_sys::Win32::Foundation::RECT {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        };
        DrawTextW(
            memdc,
            text.as_ptr(),
            text.len() as i32,
            &mut rc,
            DT_LEFT | DT_SINGLELINE | DT_NOPREFIX | DT_CALCRECT,
        );
        SelectObject(memdc, old);
        rc.right - rc.left
    }

    /// 把文本画进 DIB 的**覆盖度掩码**，返回 `(像素缓冲, 宽, 高)`。
    ///
    /// ⛔⛔ **为什么不能直接把 GDI 文本画进主缓冲**（本模块最隐蔽的一个坑）：
    ///   GDI 的 `DrawTextW` **不理解 per-pixel alpha** —— 它在 32bpp DIB 上写的是
    ///   `0x00RRGGBB`（**alpha 字节恒为 0**）。直接画进主缓冲，这些文字像素
    ///   `alpha=0` ⇒ 被 `ULW` 当作**全透明** ⇒ **文字完全不显示**。
    ///   （与「没设背景刷」的失败表现**一模一样**，极易误判成同一个问题。）
    /// ⇒ 正确做法：**在白底黑字的掩码上画**，然后按「暗到什么程度」反推覆盖度
    ///   ⇒ `alpha = 255 - gray`。这样抗锯齿边缘的覆盖度是**精确**的。
    ///
    /// ⭐ 掩码用 **GDI 默认 1bpp→32bpp 扩展**：先填白（`0xFF`），GDI 写黑字。
    ///   返回的缓冲是 `BGRA` 顺序（`CreateDIBSection` 的固有顺序）。
    ///
    /// ⚠️ `ellipsis = true` 时用 `DT_END_ELLIPSIS` 画 `…`（文本宽度 > `w` 的截断场景）。
    ///   调用方**只在文本确实装不下时**传 `true` —— 因为 `DT_END_ELLIPSIS` 有额外开销，
    ///   且对能装下的文本会多一次内部测量。判据由调用方用 `measure_text` 的结果给出。
    pub unsafe fn render_text_mask(
        w: i32,
        h: i32,
        font: HFONT,
        text: &[u16],
        ellipsis: bool,
    ) -> Option<(Vec<u8>, i32, i32)> {
        if w <= 0 || h <= 0 {
            return None;
        }
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
            biHeight: -h, // top-down
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
        let old_bmp = SelectObject(memdc, bitmap);

        // 白底（覆盖度 0=未触碰）
        let buf = std::slice::from_raw_parts_mut(bits as *mut u8, (w * h * 4) as usize);
        buf.fill(0xFF);

        // 黑字、透明背景模式（不刷底，保留我们的白）
        let old_font = SelectObject(memdc, font as HGDIOBJ);
        SetBkMode(memdc, TRANSPARENT as i32);
        SetTextColor(memdc, 0x0000_0000);
        let mut rc = windows_sys::Win32::Foundation::RECT {
            left: 0,
            top: 0,
            right: w,
            bottom: h,
        };
        let mut flags = DT_LEFT | DT_SINGLELINE | DT_NOPREFIX;
        if ellipsis {
            // 装不下 ⇒ 用 `…` 而不是硬切半个字形（真机实测过差别）
            flags |= DT_END_ELLIPSIS;
        }
        DrawTextW(memdc, text.as_ptr(), text.len() as i32, &mut rc, flags);
        SelectObject(memdc, old_font);

        let out = buf.to_vec();
        SelectObject(memdc, old_bmp);
        DeleteObject(bitmap);
        DeleteDC(memdc);
        Some((out, w, h))
    }

    /// 建一个与 widget 高度相称的 GUI 字体。
    ///
    /// ⚠️ 用**两级降级**：`Segoe UI` → `Microsoft YaHei UI`（中文界面常见）
    ///   → 系统默认（`lfFaceName` 留空）。字体缺失时 GDI 自动回退，不会失败。
    /// ⚠️ `cleartype = 0`（关掉子像素抗锯齿）：ClearType 会产生**彩色边缘**，
    ///   而我们要的是灰度覆盖度（子像素渲染依赖屏幕的 RGB 排列，在分层窗口上
    ///   还会被预乘过程破坏）。`quality = 5`（CLEARTYPE_QUALITY）→ 改用
    ///   `ANTIALIASED_QUALITY(4)`，得到灰度抗锯齿。
    pub unsafe fn create_font(px_height: i32) -> HFONT {
        let face: Vec<u16> = "Segoe UI\0".encode_utf16().collect();
        CreateFontW(
            -px_height, // 负 = 字符高度（而非单元格高度）
            0,
            0,
            0,
            400, // FW_NORMAL
            0,
            0,
            0,
            0x01, // DEFAULT_CHARSET
            0,    // OUT_DEFAULT_PRECIS
            0,    // CLIP_DEFAULT_PRECIS
            4,    // ANTIALIASED_QUALITY（灰度抗锯齿，**不要** CLEARTYPE）
            0,    // DEFAULT_PITCH
            face.as_ptr(),
        )
    }

    pub unsafe fn destroy_font(f: HFONT) {
        DeleteObject(f as HGDIOBJ);
    }

    /// 空 WndProc：分层窗口不靠 `WM_PAINT` 绘制（那是 `ULW` 的活），
    /// 但也**不能**把 `WM_ERASEBKGND` 返回 0（会破坏「可见四条件」），故一律交 `DefWindowProcW`。
    ///
    /// ⭐ 例外：**应用私有的刷新消息**（`WM_APP_REFRESH`）必须自己处理 ——
    ///   它是「后台取数完成，请主线程重绘」的唯一通道（跨线程安全）。
    unsafe extern "system" fn wnd_proc(hwnd: HWND, msg: u32, wp: usize, lp: isize) -> isize {
        if msg == WM_APP_REFRESH {
            // 在**主线程**重绘：读快照 → 建 DIB → 提交。
            super::repaint_from_snapshot(hwnd);
            return 0;
        }
        DefWindowProcW(hwnd, msg, wp, lp)
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
}

/// 窗口**是否仍然存在**（`IsWindow`）。
///
/// ⛔ 为什么不能只看 `WIDGET_HWND != 0`：Explorer 重建时任务栏窗被销毁，
///   我们的子窗会**随之被系统销毁**，但原子量里仍留着那个**已失效的句柄值**
///   ⇒ 只看非零会永远判成「已挂载」，再也不会自愈（静默失效的典型形态：
///   不报错、不崩溃，窗口就是不回来）。
#[cfg(target_os = "windows")]
fn widget_alive() -> bool {
    let hwnd = WIDGET_HWND.load(Ordering::SeqCst) as *mut core::ffi::c_void;
    if hwnd.is_null() {
        return false;
    }
    unsafe { windows_sys::Win32::UI::WindowsAndMessaging::IsWindow(hwnd) != 0 }
}

/// 复位挂载状态（**只动原子量**，可在任意线程调用，不碰窗口）。
///
/// ⭐ 与 `destroy_widget` 分开的原因：`DestroyWindow` **必须在创建窗口的线程**调用，
///   而后台兜底线程发现「句柄已失效」时只需把记录清掉，不必（也不能）去销毁。
#[cfg(target_os = "windows")]
fn forget_widget() {
    WIDGET_HWND.store(0, Ordering::SeqCst);
    REPARENT_ERR.store(0, Ordering::SeqCst);
    SLOT_VALID.store(false, Ordering::Release);
    // 位置沿用值一并清掉：下次挂载重新按配置贴靠，而不是沿用上一轮窗口的坐标
    LAST_X.store(0, Ordering::Release);
}

/// 挂载 widget 到任务栏（**里程碑 1**：建窗 + 挂载 + 自绘一帧）。
///
/// 返回 `MountReport`，调用方用 `report.ok()` 判定成败并落日志。
/// ⚠️ 本函数**必须在常驻泵消息的线程上调用**（窗口随创建线程退出而销毁）。
#[cfg(target_os = "windows")]
pub fn spawn_widget() -> MountReport {
    // ⭐ 幂等：已挂且窗口仍存活 ⇒ 直接回现状，**不再建第二个窗**。
    //    为什么需要：`apply_from_config` 是**异步投递**（`run_on_main_thread`），
    //    启动期开发门控又会**同步**调一次 ⇒ 两条路可能先后都走到「建窗」。
    //    重复挂载会**泄漏一个窗口**（旧句柄被覆盖后永远销毁不掉，浮在任务栏上）。
    if widget_alive() {
        append_log("[widget] 已挂载且窗口存活 ⇒ 跳过重复挂载");
        return probe();
    }
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

    // 6) 先**取一次任务栏视觉空白区**，决定 widget 的初始 x；
    //    draw_frame 后续每次刷新也会重新计算，故这里不是写死坐标，只是首帧锚点。
    // 首帧只画透明占位，不在 setup 主线程做像素扫描；真实数据到来后再由重绘路径避让。
    let drawn = draw_frame(hwnd);

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

/// **单段**文本在 widget 上占用的最大宽度（像素）。
///
/// ⭐ 为什么不无限平铺：`PINNED_TASKBAR_LIMIT = 8` ⇒ 8 台 × 每台十几字符会吃掉
///    上千像素，在窄屏/多窗口时与任务栏图标区冲突。给每段一个上限、超出截断，
///    保证「有多少台都画得下」，也让布局可预测（用户选定「全部平铺」+ 上限保护）。
///
/// ⚠️ 判据粒度是**单段**（电量段、音量段各算一次），不是单台设备 ——
///   一台设备的宽度取两段的最大值（见 `draw_items` 的 `per_item`）。
#[cfg(target_os = "windows")]
const ITEM_MAX_W: i32 = 150;

/// 设备之间的水平间隔（像素）。
#[cfg(target_os = "windows")]
const ITEM_GAP: i32 = 10;

/// 内容区左右内边距 —— ⛔ 与任务栏左右两端**各留一段**，
/// 使「邻居漂移」时仍有余量（PLAYBOOK §E 避让口径：两端留边距 + 被侵入时重定位）。
#[cfg(target_os = "windows")]
const PAD_X: i32 = 6;

/// 字体像素高度 —— 电量/音量两行文本共用。取 11：在 16px 行高里既能看清
/// 又不至于让两行贴在一起（`%`/数字在 11px Segoe UI 下清晰可辨）。
#[cfg(target_os = "windows")]
const FONT_PX: i32 = 11;

/// 图标绘制边长（方形）。取 32：图标源 PNG 本身就是 32×32（`src-tauri/icons/`），
/// 因此 `scale_to` 是**恒等拷贝**、零重采样损耗；且与任务栏自身图标（约 30px）尺度相当。
#[cfg(target_os = "windows")]
const ICON_PX: i32 = 32;

/// 图标与右侧两行文本之间的间隔。
#[cfg(target_os = "windows")]
const ICON_TEXT_GAP: i32 = 4;

/// 图标右侧每行文本的掩码高度 = 图标高度的一半。
///
/// ⭐ 电量占**上半行**（= 图标的右上角）、音量占**下半行**（= 图标的右下角）——
///   这正是用户 2026-09-24 指定的布局。
/// ⚠️ 与 `FONT_PX` 的关系：11px 字在 16px 行里上下各有余量，`DrawTextW`
///   的 `DT_SINGLELINE` 把字形画在行**顶部** ⇒ 两行之间天然留出空隙。
#[cfg(target_os = "windows")]
const TEXT_ROW_H: i32 = ICON_PX / 2;

/// widget 最多显示的设备台数（用户 2026-09-24 指定）。
///
/// ⭐ 与 `PINNED_TASKBAR_LIMIT = 8` 的关系：那个是**固定上限**（最多能 pin 几台），
///   这个是**显示上限**（任务栏上最多画几台）。显示上限更小，因为有**物理宽度**约束
///   —— 6 台 × (32 图标 + 4 间隙 + 约 30 文本 + 10 间隔) ≈ 460px，能在避让后的可用区里放下。
#[cfg(target_os = "windows")]
const WIDGET_MAX_ITEMS: usize = 6;

/// 电量文本（画在图标**右上角**）。
///
/// `Some(b)` → `"85%"`；读不出 → `"N/A"`。
///
/// ⭐ **为什么不画设备名**：用户 2026-09-24 明确「设备名以后再说」⇒ 当前只画
///   「图标 + 电量 + 音量」。设备名仍保留在 `WidgetItem` 里（诊断日志用）。
///
/// ⭐ **为什么把「格式化」与「绘制」分开**：格式化是**纯函数**，可以单测
///   （不需要窗口、不需要 DIB）；绘制依赖 GDI 无法单测。分开后，
///   「0% 与读不出要显示得不一样」这类**语义**就有测试保护。
#[cfg(target_os = "windows")]
fn format_battery(it: &WidgetItem) -> String {
    // ⛔ `Some(0)` 是合法值（电量耗尽），`None` 是读不出 ⇒ 必须区分：
    //    写反了界面只是少一个百分号，肉眼几乎看不出（典型的静默失效）。
    match it.battery {
        Some(b) => format!("{}%", b),
        None => "N/A".to_string(),
    }
}

/// 音量文本（画在图标**右下角**）。
///
/// 静音 → `"静音"`；`Some(v)` → `"85%"`；**没有音量可显示 → `"N/A"`**。
///
/// ⚠️ 「无音频端点」与「有端点但暂时读不出」**都显示 `N/A`**（用户 2026-09-24 口径：
///   没有音量就显示 N/A）。两者在数据层仍由 `has_audio` 区分 —— 将来若要改成
///   「无端点整段不画」，不必回头动数据。
#[cfg(target_os = "windows")]
fn format_volume(it: &WidgetItem) -> String {
    if !it.has_audio {
        // 键鼠这类无音频端点的设备：用户明确要求显示 N/A（而不是留空）
        return "N/A".to_string();
    }
    // ⭐ 静音**优先于**百分比：端点静音时其音量值仍是旧值（不是 0）
    //    ⇒ 直接显示「静音」，否则会显示成「40%」这种与听觉不符的数字。
    if it.is_muted == Some(true) {
        return "静音".to_string();
    }
    match it.volume {
        Some(v) => format!("{}%", (v * 100.0).round() as i32),
        None => "N/A".to_string(),
    }
}

/// 图标资源与解码（**编译期嵌入 + 进程内缓存**）。
///
/// ⭐ 三个图标 × 深浅两套 = 6 张 PNG，全部 `include_bytes!` 进二进制：
///   · `tray-icon{,-dark}.png`         —— 软件默认鼠标图标（无音频端点的设备）
///   · `tray-speaker-icon{,-dark}.png` —— 扬声器（在音量页、前缀是「扬声器」）
///   · `tray-headphone-icon{,-dark}.png`—— 耳机（在音量页、前缀是「耳机」）
///
/// ⛔ **为什么必须嵌二进制而不是运行时读文件**：MSIX 包安装目录只读、
///   且 Tauri 的 `resources` 部署路径与 exe 不同（见 PLAYBOOK §I）
///   ⇒ 运行期找文件必然踩路径坑。`include_bytes!` 零歧义。
///
/// ⚠️ 解码（PNG → RGBA）用 `image` crate，结果按 `(kind, dark)` 缓存到
///   `OnceLock`：图标内容编译期就固定，重复解码纯浪费。
#[cfg(target_os = "windows")]
mod icons {
    use crate::device_identity::AudioKind;
    use std::sync::OnceLock;

    // 6 张图，编译期嵌入
    static MOUSE_LIGHT: &[u8] = include_bytes!("../icons/tray-icon.png");
    static MOUSE_DARK: &[u8] = include_bytes!("../icons/tray-icon-dark.png");
    static SPEAKER_LIGHT: &[u8] = include_bytes!("../icons/tray-speaker-icon.png");
    static SPEAKER_DARK: &[u8] = include_bytes!("../icons/tray-speaker-icon-dark.png");
    static HEADPHONE_LIGHT: &[u8] = include_bytes!("../icons/tray-headphone-icon.png");
    static HEADPHONE_DARK: &[u8] = include_bytes!("../icons/tray-headphone-icon-dark.png");

    /// 解码后的 RGBA 位图（`(像素, 宽, 高)`）。像素顺序 = RGBA（`image` crate 约定）。
    pub type Rgba = (Vec<u8>, u32, u32);

    // 每种「类别 + 主题」一份缓存。用 6 个独立 OnceLock 而不是 HashMap：
    // 键空间在两维上都极小且固定，静态化可以避免加锁（截图路径常被调用）。
    static CACHE_MOUSE_LIGHT: OnceLock<Option<Rgba>> = OnceLock::new();
    static CACHE_MOUSE_DARK: OnceLock<Option<Rgba>> = OnceLock::new();
    static CACHE_SPEAKER_LIGHT: OnceLock<Option<Rgba>> = OnceLock::new();
    static CACHE_SPEAKER_DARK: OnceLock<Option<Rgba>> = OnceLock::new();
    static CACHE_HEADPHONE_LIGHT: OnceLock<Option<Rgba>> = OnceLock::new();
    static CACHE_HEADPHONE_DARK: OnceLock<Option<Rgba>> = OnceLock::new();

    /// 解码一张 PNG 为 RGBA。失败返回 `None`（**不 panic** —— 缺图标不该拖垮整个 widget）。
    fn decode(bytes: &[u8]) -> Option<Rgba> {
        let img = image::load_from_memory(bytes).ok()?.to_rgba8();
        let (w, h) = (img.width(), img.height());
        Some((img.into_raw(), w, h))
    }

    /// 取指定类别 + 主题的图标（懒解码 + 缓存）。
    ///
    /// ⚠️ 失败结果**同样缓存**：图标内容编译期已固定，解码失败属环境问题、不会自愈，
    ///   没必要每次重绘都重试一遍解码（与 `resolve_toast_icon` 的缓存策略一致）。
    pub fn get(kind: AudioKind, dark: bool) -> Option<&'static Rgba> {
        let (cell, bytes) = match (kind, dark) {
            (AudioKind::Pointer, false) => (&CACHE_MOUSE_LIGHT, MOUSE_LIGHT),
            (AudioKind::Pointer, true) => (&CACHE_MOUSE_DARK, MOUSE_DARK),
            (AudioKind::Speaker, false) => (&CACHE_SPEAKER_LIGHT, SPEAKER_LIGHT),
            (AudioKind::Speaker, true) => (&CACHE_SPEAKER_DARK, SPEAKER_DARK),
            (AudioKind::Headphones, false) => (&CACHE_HEADPHONE_LIGHT, HEADPHONE_LIGHT),
            (AudioKind::Headphones, true) => (&CACHE_HEADPHONE_DARK, HEADPHONE_DARK),
        };
        cell.get_or_init(|| decode(bytes)).as_ref()
    }

    /// 最近邻缩放一张 RGBA 到 `side × side`（图标是**线稿**，缩放必须保持锐利边缘，
    /// 双线性会让 16px 的细线条糊掉）。
    ///
    /// ⭐ 返回预乘前的 straight RGBA —— 合成到 DIB 时由调用方按需预乘。
    pub fn scale_to(src: &Rgba, side: u32) -> Option<Rgba> {
        let (px, sw, sh) = src;
        if *sw == 0 || *sh == 0 || side == 0 {
            return None;
        }
        let mut out = vec![0u8; (side * side * 4) as usize];
        for y in 0..side {
            // 最近邻：源坐标 = 目标坐标 × 源边长 / 目标边长
            let sy = (y as u64 * *sh as u64 / side as u64) as u32;
            for x in 0..side {
                let sx = (x as u64 * *sw as u64 / side as u64) as u32;
                let si = ((sy * *sw + sx) * 4) as usize;
                let di = ((y * side + x) * 4) as usize;
                out[di..di + 4].copy_from_slice(&px[si..si + 4]);
            }
        }
        Some((out, side, side))
    }
}

/// 任务栏**左端 x 坐标**（物理像素）。
///
/// ⭐ 用途：`UpdateLayeredWindow` 的位置参数是**父窗客户区坐标**，而视觉扫描
///   （`find_widget_slot`）拿到的是**屏幕坐标** ⇒ 两者相减才是相对坐标。
#[cfg(target_os = "windows")]
fn taskbar_left() -> i32 {
    let taskbar = unsafe {
        windows_sys::Win32::UI::WindowsAndMessaging::FindWindowW(
            to_wide("Shell_TrayWnd").as_ptr(),
            std::ptr::null(),
        )
    };
    if taskbar.is_null() {
        return 0;
    }
    let mut rc = windows_sys::Win32::Foundation::RECT {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    };
    unsafe {
        windows_sys::Win32::UI::WindowsAndMessaging::GetWindowRect(taskbar, &mut rc);
    }
    rc.left
}

/// widget 在任务栏内的**垂直偏移**（父窗客户区坐标，物理像素）。
///
/// ⭐ 为什么要算而不是写死 0：真机实测任务栏高 **60px**、widget 高 **40px** ⇒
///   贴顶会让 widget 里的图标比任务栏自身的图标**高出约 10px**，一眼能看出没对齐
///   （这正是「22px 高 + 内容贴顶」时期遗留的观感缺陷）。居中后两者同一水平线。
///
/// ⚠️ 任务栏比 widget 还矮时（异常 DPI/多显示器缩放不一致）退回 0，
///   不产生负偏移（负值会把内容顶到任务栏之外）。
#[cfg(target_os = "windows")]
fn widget_y_offset() -> i32 {
    let taskbar = unsafe {
        windows_sys::Win32::UI::WindowsAndMessaging::FindWindowW(
            to_wide("Shell_TrayWnd").as_ptr(),
            std::ptr::null(),
        )
    };
    if taskbar.is_null() {
        return 0;
    }
    let mut rc = windows_sys::Win32::Foundation::RECT {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    };
    unsafe {
        windows_sys::Win32::UI::WindowsAndMessaging::GetWindowRect(taskbar, &mut rc);
    }
    ((rc.bottom - rc.top - WIDGET_H) / 2).max(0)
}

/// 在避让槽内按贴靠策略算出窗口左端。
///
/// ⭐ 抽成**纯函数**是为了能单测：贴靠判据极易写反（`right` 写成 `slot_x` 不报错，
///   只是窗口跑到左边；`center` 少除一次 2 也只是偏一点），肉眼未必立刻发现。
///
/// ⚠️ 贴靠的参照系是**避让后的空白槽**，不是整条任务栏 —— 槽内 `center` 未必等于
///   屏幕居中，但它是唯一「不会被邻居压住」的居中口径（见 PLAYBOOK §E9）。
/// ⚠️ 槽宽小于内容宽时 `max_offset` 归零 ⇒ 三种策略都退化为贴槽左端；
///   调用方在此之前已用 `wanted` 宽度筛过槽，正常不会走到这里（属防御分支）。
#[cfg(target_os = "windows")]
fn align_in_slot(slot_x: i32, slot_w: i32, content_w: i32, position: &str) -> i32 {
    let max_offset = (slot_w - content_w).max(0);
    let offset = match position {
        "left" => 0,
        "right" => max_offset,
        // `center` 及任何未知值（`normalize_config` 已兜住非法值，此处仅防御）
        _ => max_offset / 2,
    };
    slot_x + offset
}

/// 在任务栏上找一段「视觉空白」并留安全边距。
///
/// 判据与 PLAYBOOK §E9 对齐：扫描任务栏像素，二维相邻像素最大差大于 60
/// 视为有内容；两端各留 100px，避免任务栏按钮向右长、第三方 widget 向左长时侵入。
///
/// ⚠️ 这是保守判据：误把背景噪声算作占用只会少用空间，误把内容算成空白才会重叠。
/// 每次重绘重新扫描，故歌词 widget 漂移后会触发重新定位，而不是继续使用旧坐标。
#[cfg(target_os = "windows")]
fn find_widget_slot(wanted: i32) -> Option<(i32, i32)> {
    use windows_sys::Win32::Graphics::Gdi::{
        BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject, GetDC,
        GetDIBits, ReleaseDC, SelectObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, CAPTUREBLT,
        DIB_RGB_COLORS, SRCCOPY,
    };
    let wanted = wanted.max(1);
    let taskbar = unsafe {
        windows_sys::Win32::UI::WindowsAndMessaging::FindWindowW(
            to_wide("Shell_TrayWnd").as_ptr(),
            std::ptr::null(),
        )
    };
    if taskbar.is_null() {
        return None;
    }
    let mut rc = windows_sys::Win32::Foundation::RECT {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    };
    unsafe {
        windows_sys::Win32::UI::WindowsAndMessaging::GetWindowRect(taskbar, &mut rc);
    }
    let width = (rc.right - rc.left).max(0);
    let height = (rc.bottom - rc.top).max(0);
    if width <= 0 || height <= 0 {
        return None;
    }
    // 一次 BitBlt 抓取整个任务栏，再在本进程内扫描像素；
    // ⛔ 不能逐像素 GetPixel（跨进程调用在本机实测约 24 秒/次）。
    let screen = unsafe { GetDC(std::ptr::null_mut()) };
    if screen.is_null() {
        return None;
    }
    let mem = unsafe { CreateCompatibleDC(screen) };
    let bitmap = unsafe { CreateCompatibleBitmap(screen, width, height) };
    if mem.is_null() || bitmap.is_null() {
        if !mem.is_null() {
            unsafe { DeleteDC(mem) };
        }
        if !bitmap.is_null() {
            unsafe { DeleteObject(bitmap) };
        }
        unsafe { ReleaseDC(std::ptr::null_mut(), screen) };
        return None;
    }
    unsafe {
        SelectObject(mem, bitmap);
        if BitBlt(
            mem,
            0,
            0,
            width,
            height,
            screen,
            rc.left,
            rc.top,
            SRCCOPY | CAPTUREBLT,
        ) == 0
        {
            DeleteObject(bitmap);
            DeleteDC(mem);
            ReleaseDC(std::ptr::null_mut(), screen);
            return None;
        }
    }
    let mut bmi: BITMAPINFO = unsafe { std::mem::zeroed() };
    bmi.bmiHeader = BITMAPINFOHEADER {
        biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
        biWidth: width,
        biHeight: -height,
        biPlanes: 1,
        biBitCount: 32,
        biCompression: BI_RGB,
        ..unsafe { std::mem::zeroed() }
    };
    let mut pixels = vec![0u32; (width * height) as usize];
    let copied = unsafe {
        GetDIBits(
            mem,
            bitmap,
            0,
            height as u32,
            pixels.as_mut_ptr() as *mut core::ffi::c_void,
            &mut bmi,
            DIB_RGB_COLORS,
        )
    };
    unsafe {
        DeleteObject(bitmap);
        DeleteDC(mem);
        ReleaseDC(std::ptr::null_mut(), screen);
    }
    if copied == 0 {
        return None;
    }
    let mut occupied = vec![false; width as usize];
    // 12px 采样足够识别任务栏按钮/第三方 widget 的边界，再膨胀 ±12px。
    const SAMPLE: usize = 12;
    for x in (1..width).step_by(SAMPLE) {
        let mut max_diff = 0u32;
        for y in (0..height).step_by(SAMPLE) {
            let a = pixels[(y * width + x) as usize];
            let b = pixels[(y * width + x - 1) as usize];
            max_diff = max_diff.max(color_diff(a, b));
        }
        for y in (SAMPLE as i32..height).step_by(SAMPLE) {
            let a = pixels[(y * width + x) as usize];
            let b = pixels[((y - SAMPLE as i32) * width + x) as usize];
            max_diff = max_diff.max(color_diff(a, b));
        }
        // 旧 widget 自己的可见内容不是邻居；排除其采样列，否则下一轮会把自己判成占用。
        let old_x = SLOT_X.load(Ordering::Acquire) - rc.left;
        let old_w = SLOT_W.load(Ordering::Acquire);
        if SLOT_VALID.load(Ordering::Acquire) && x >= old_x && x < old_x.saturating_add(old_w) {
            continue;
        }
        if max_diff > 60 {
            let lo = x.saturating_sub(SAMPLE as i32) as usize;
            let hi = ((x + SAMPLE as i32).min(width - 1)) as usize;
            for hit in &mut occupied[lo..=hi] {
                *hit = true;
            }
        }
    }

    // 任务栏两端安全边距（PLAYBOOK §E9.10：两端各留 >=100px）。
    let left = 100i32.min(width / 2);
    let right = (width - 100).max(left);
    let mut run_start = left;
    for x in left..=right {
        let blocked = x == right || occupied[x as usize];
        if blocked {
            if x - run_start >= wanted {
                return Some((rc.left + run_start, x - run_start));
            }
            run_start = x + 1;
        }
    }
    None
}

/// 单段文本宽度的**保守**估算（像素）。
///
/// ⚠️ 为什么按码位分类、而不是统一乘一个系数：ASCII 数字/`%`/`N/A` 在 11px Segoe UI 下
///   约 6–7px，而中文（全角）在 11px 下约 11px。若统一按 8px 估，**中文会被低估**
///   ⇒ `find_widget_slot` 可能返回一个比实际内容更窄的槽 ⇒ 内容溢出到邻居上。
///   避让扫描的**方向性要求**：宁可高估（少用一点空间），不可低估（重叠）。
///
/// ⭐ 可证伪：把非 ASCII 的 14 改回 8，`cjk_is_not_underestimated` 会立刻转红。
#[cfg(target_os = "windows")]
fn estimate_text_px(s: &str) -> i32 {
    s.chars()
        .map(|c| if (c as u32) < 0x80 { 8 } else { 14 })
        .sum::<i32>()
        .min(ITEM_MAX_W)
}

#[cfg(target_os = "windows")]
fn estimate_widget_width(items: &[WidgetItem]) -> i32 {
    // ⚠️ 每台设备占「两段文本里更宽的那一段」—— 与 `draw_items` 的 `per_item` 同口径。
    let text_px: i32 = items
        .iter()
        .map(|it| {
            let bat = estimate_text_px(&format_battery(it));
            let vol = estimate_text_px(&format_volume(it));
            bat.max(vol)
        })
        .sum();
    let gaps = ITEM_GAP * (items.len().saturating_sub(1) as i32);
    PAD_X * 2 + text_px + items.len() as i32 * (ICON_PX + ICON_TEXT_GAP) + gaps
}

#[cfg(target_os = "windows")]
fn color_diff(a: u32, b: u32) -> u32 {
    let ar = a & 0xff;
    let ag = (a >> 8) & 0xff;
    let ab = (a >> 16) & 0xff;
    let br = b & 0xff;
    let bg = (b >> 8) & 0xff;
    let bb = (b >> 16) & 0xff;
    (ar.abs_diff(br) + ag.abs_diff(bg) + ab.abs_diff(bb)) / 3
}

/// 把一段**文本掩码**按覆盖度预乘合成进主缓冲（`(dst_x, dst_y)` = 目标左上角）。
///
/// ⛔ 为什么不能把 GDI 文本直接画进主缓冲：`DrawTextW` 在 32bpp DIB 上写的是
///   `0x00RRGGBB`（**alpha 字节恒为 0**）⇒ 被 `ULW` 当全透明 ⇒ **文字完全不显示**
///   （与「类没设背景刷」的表现一模一样，极易误判成同一个问题）。
///   正确路径 = 白底黑字掩码 → `cov = 255 - gray` → 预乘（详见 `ffi::render_text_mask`）。
///
/// ⭐ 重叠像素取**最大 alpha**（不是相加）：电量段与音量段、或字形与图标重叠时
///   不叠加亮度（叠加会在笔画交叉处出现亮斑）。
///
/// ⚠️ 抽成函数而不是在绘制循环里内联两份：电量与音量两行的合成逻辑**逐字相同**，
///   复制两份必然在后续改动中漂移（一处改了预乘、另一处忘了）。
#[cfg(target_os = "windows")]
fn blit_text_mask(
    px: &mut [u32],
    total_w: i32,
    mask: &(Vec<u8>, i32, i32),
    dst_x: i32,
    dst_y: i32,
    color: (u8, u8, u8),
    alpha_scale: f32,
) {
    let (buf, mw, mh) = mask;
    let (cr, cg, cb) = color;
    for yy in 0..*mh {
        let dy = dst_y + yy;
        if dy < 0 {
            continue;
        }
        for xx in 0..*mw {
            let dx = dst_x + xx;
            // ⛔ 必须判 `dx >= total_w`：否则会**绕行到下一行**（行内偏移溢出），
            //    表现为「右端文本的第一个字出现在左端下一行」。
            if dx < 0 || dx >= total_w {
                continue;
            }
            // 掩码里 GDI 写的是**黑字白底**（BGRA 顺序）⇒ 取绿通道算灰度
            // 覆盖度 = 255 - gray（白=未覆盖=0；黑=完全覆盖=255）
            let mi = ((yy * mw + xx) * 4) as usize;
            let cov = 255u32 - buf[mi + 1] as u32;
            if cov == 0 {
                continue;
            }
            let a = ((cov as f32) * alpha_scale).round() as u32;
            if a == 0 {
                continue;
            }
            // ⛔ 预乘：`R·A/255`（straight alpha 会让抗锯齿边缘**过亮泛白**）
            let pr = cr as u32 * a / 255;
            let pg = cg as u32 * a / 255;
            let pb = cb as u32 * a / 255;
            let packed = (a << 24) | (pr << 16) | (pg << 8) | pb;
            let di = (dy * total_w + dx) as usize;
            if di < px.len() && a > px[di] >> 24 {
                px[di] = packed;
            }
        }
    }
}

/// 真实内容的自绘与提交（**主线程**调用）。
///
/// 布局（用户 2026-09-24 指定）：每台设备一段，段内**左侧一个放大图标**，
/// 图标**右上角**画电量、**右下角**画音量（各占图标高度的一半）；
/// 横向按项依次排列，宽度 = 各段实测宽度之和 + 间隔 + 内边距。
/// 窗口宽度随之改变（`ULW` 的 `SIZE` 参数**同时**设定窗口形状 ⇒ 无需 `MoveWindow`）。
///
/// ⛔ 若快照为 `None`（首次刷新尚未完成）⇒ 画一帧**空内容**（全透明）并返回，
///   让窗口先以正确尺寸出现，避免「挂上去但尺寸是建窗时的 1×WIDGET_H」。
#[cfg(target_os = "windows")]
fn draw_items(hwnd: *mut core::ffi::c_void, items: &[WidgetItem]) -> bool {
    let dark = crate::windows::system_dark_mode();

    // 空快照：不画任何东西，但仍提交一帧（保持窗口有效且全透明）
    if items.is_empty() {
        return draw_blank(hwnd, PAD_X * 2);
    }

    // ── 先在**测量用 DC** 上量出每段文本宽度 ────────────────────────────
    // ⚠️ 测量与绘制必须用**同一个字体 + 同样的 DrawTextW 标志**，
    //    否则会出现「按测量宽度排版、实际文本更长」⇒ 相邻项重叠（见 measure_text 注释）。
    let (font, memdc) = unsafe {
        let f = ffi::create_font(FONT_PX);
        if f.is_null() {
            append_log("[widget] CreateFontW 失败，退回空白帧");
            return draw_blank(hwnd, PAD_X * 2);
        }
        let screen = windows_sys::Win32::Graphics::Gdi::GetDC(std::ptr::null_mut());
        let dc = windows_sys::Win32::Graphics::Gdi::CreateCompatibleDC(screen);
        windows_sys::Win32::Graphics::Gdi::ReleaseDC(std::ptr::null_mut(), screen);
        (f, dc)
    };
    if memdc.is_null() {
        unsafe { ffi::destroy_font(font) };
        return draw_blank(hwnd, PAD_X * 2);
    }

    // ── 每项：图标 + 两段文本（右上电量 / 右下音量），各自测量宽度 ──────
    let bat_texts: Vec<Vec<u16>> = items
        .iter()
        .map(|it| format_battery(it).encode_utf16().collect())
        .collect();
    let vol_texts: Vec<Vec<u16>> = items
        .iter()
        .map(|it| format_volume(it).encode_utf16().collect())
        .collect();
    // 每段记 `(文本实际宽, 是否需要省略号)`：实际宽 = `min(自然宽, ITEM_MAX_W)`；
    // 自然宽 > 上限 ⇒ 该段要画 `…`（否则会**硬切半个字形**）。
    let measure = |t: &[u16]| -> (i32, bool) {
        let natural = unsafe { ffi::measure_text(memdc, font, t) };
        // ⛔ 每段加上限：极端长文本时截断显示（避免挤压其它设备）
        let clamped = natural.min(ITEM_MAX_W);
        (clamped, natural > clamped)
    };
    let bat_w: Vec<(i32, bool)> = bat_texts.iter().map(|t| measure(t)).collect();
    let vol_w: Vec<(i32, bool)> = vol_texts.iter().map(|t| measure(t)).collect();

    unsafe {
        windows_sys::Win32::Graphics::Gdi::DeleteDC(memdc);
    }

    // ── 计算总宽并建主 DIB ────────────────────────────────────────────
    // 每项宽 = 图标 + 间隙 + **两段文本里更宽的那段**（两段共用同一列起画点）；
    // 项与项之间再加 `ITEM_GAP`，两端加 `PAD_X`。
    let per_item: Vec<i32> = (0..items.len())
        .map(|i| ICON_PX + ICON_TEXT_GAP + bat_w[i].0.max(vol_w[i].0))
        .collect();
    let content_w: i32 = per_item.iter().sum::<i32>() + ITEM_GAP * (items.len() as i32 - 1);
    let total_w = content_w + PAD_X * 2;
    // ⛔ GetPixel 扫描必须在后台线程完成（WMI 之外也不能阻塞窗口线程）。
    // 后台快照任务已将估算宽度传给 `find_widget_slot`，这里仅读取原子坐标。
    if !SLOT_VALID.load(Ordering::Acquire) {
        unsafe { ffi::hide(hwnd as _) };
        unsafe { ffi::destroy_font(font) };
        return true;
    }
    // ⚠️ 位置配置**在绘制前一次性取快照**（返回 owned 值）⇒ 锁在 GDI 调用之前就已释放，
    //    不会「持配置锁去做窗口操作」（AGENTS.md 的 AB/BA 死锁纪律）。
    let (position, locked) =
        crate::config::with_config(|c| (c.taskbar_position.clone(), c.taskbar_position_locked));
    let slot_rel_x = SLOT_X.load(Ordering::Acquire) - taskbar_left();
    let slot_w = SLOT_W.load(Ordering::Acquire);
    let aligned = align_in_slot(slot_rel_x, slot_w, total_w, &position);
    // 「未固定位置」⇒ 沿用上次画过的位置（哨兵 0 = 还没画过，退回贴靠结果）。
    let rel_x = if locked {
        aligned
    } else {
        match LAST_X.load(Ordering::Acquire) {
            0 => aligned,
            last => last,
        }
    };
    LAST_X.store(rel_x, Ordering::Release);
    // ⭐ 定位结果**必须落日志**：本模块最容易「看起来正常但位置不对」——
    //   贴靠算错、槽位过窄（`max_offset` 归零 ⇒ left/center/right 三者同解）、
    //   未固定时沿用旧值，全都表现为「窗口在那儿，只是不在你以为的地方」。
    //   ⚠️ 重绘是事件驱动的低频操作（非每帧），此处打日志不会淹没有用信息。
    append_log(&format!(
        "[widget] 定位: pos={position} locked={locked} slot=({slot_rel_x},w={slot_w}) \
         content_w={total_w} → rel_x={rel_x}"
    ));
    unsafe { ffi::show(hwnd as _) };
    let h = WIDGET_H;
    let Some(dib) = (unsafe { ffi::create_dib(total_w, h) }) else {
        append_log("[widget] CreateDIBSection 失败");
        unsafe { ffi::destroy_font(font) };
        return false;
    };

    unsafe {
        let px = std::slice::from_raw_parts_mut(dib.bits, (total_w * h) as usize);
        // 背景全透明（alpha=0 ⇒ 透明且穿透）
        px.fill(0);

        // 内容色（预乘前的原色）：浅色主题黑、深色主题白
        let (cr, cg, cb): (u8, u8, u8) = if dark { (255, 255, 255) } else { (0, 0, 0) };
        // 图标在 widget 里垂直居中（`ICON_PX` ≤ `h`，余量上下各一半）
        let icon_y = (h - ICON_PX) / 2;

        let mut cursor = PAD_X;
        for (i, it) in items.iter().enumerate() {
            // ⚠️ 用户固定的设备（此刻读不出数据）用**半透明**显示，
            //    与「有数据」区分；这是「pin = 强制显示 + 置灰」在 widget 上的落地。
            let alpha_scale: f32 = if it.pinned && it.battery.is_none() && !it.has_audio {
                0.45
            } else {
                1.0
            };

            // ① 图标：解码（带缓存）→ 最近邻缩放到 `ICON_PX` → 预乘合成
            if let Some(scaled) =
                icons::get(it.icon, dark).and_then(|rgba| icons::scale_to(rgba, ICON_PX as u32))
            {
                let (ipx, iw, ih) = scaled;
                for yy in 0..(ih as i32).min(h - icon_y) {
                    for xx in 0..(iw as i32).min(total_w - cursor) {
                        let si = ((yy * iw as i32 + xx) * 4) as usize;
                        let a0 = ipx[si + 3] as f32 * alpha_scale;
                        let a = a0.round().clamp(0.0, 255.0) as u32;
                        if a == 0 {
                            continue;
                        }
                        // 图标 PNG 是 straight alpha（`image` 的 RGBA）⇒ 按覆盖度预乘
                        // ⚠️ 图标自带颜色（黑白线稿），不能用 `cr/cg/cb` 覆盖 —— 那条路
                        //    只适用于「GDI 掩码反推的纯色文本」。
                        let r = ipx[si] as u32;
                        let g = ipx[si + 1] as u32;
                        let b = ipx[si + 2] as u32;
                        let pr = r * a / 255;
                        let pg = g * a / 255;
                        let pb = b * a / 255;
                        let packed = (a << 24) | (pr << 16) | (pg << 8) | pb;
                        let di = ((icon_y + yy) * total_w + cursor + xx) as usize;
                        if di < px.len() {
                            let old_a = px[di] >> 24;
                            if a > old_a {
                                px[di] = packed;
                            }
                        }
                    }
                }
            }
            let text_x = cursor + ICON_PX + ICON_TEXT_GAP;

            // ② 电量：图标**右上角**（两行文本的**上半行**）
            let (bw, b_ell) = bat_w[i];
            if bw > 0 {
                if let Some(mask) =
                    ffi::render_text_mask(bw, TEXT_ROW_H, font, &bat_texts[i], b_ell)
                {
                    blit_text_mask(
                        px,
                        total_w,
                        &mask,
                        text_x,
                        icon_y,
                        (cr, cg, cb),
                        alpha_scale,
                    );
                }
            }

            // ③ 音量：图标**右下角**（两行文本的**下半行**）
            let (vw, v_ell) = vol_w[i];
            if vw > 0 {
                if let Some(mask) =
                    ffi::render_text_mask(vw, TEXT_ROW_H, font, &vol_texts[i], v_ell)
                {
                    blit_text_mask(
                        px,
                        total_w,
                        &mask,
                        text_x,
                        icon_y + TEXT_ROW_H,
                        (cr, cg, cb),
                        alpha_scale,
                    );
                }
            }
            cursor += per_item[i] + ITEM_GAP;
        }
    }

    // widget 已是 `Shell_TrayWnd` 的子窗：`UpdateLayeredWindow` 的位置必须是
    // **父窗客户区坐标**（不是屏幕坐标）⇒ x 用上面算出的相对值、y 用垂直居中偏移。
    let ok = unsafe { ffi::commit(hwnd as _, &dib, rel_x, widget_y_offset()) };
    if !ok {
        let err = unsafe { windows_sys::Win32::Foundation::GetLastError() };
        append_log(&format!("[widget] UpdateLayeredWindow 失败: err={}", err));
    }
    unsafe {
        ffi::free_dib(&dib);
        ffi::destroy_font(font);
    }
    ok
}

/// 提交一帧**全透明**的位图（用于「无数据」与「失败回退」）。
///
/// ⭐ 仍要提交：`ULW` 同时也设定窗口尺寸 ⇒ 空帧会把窗口缩到 `w`×`WIDGET_H`，
///   避免残留在上一次的尺寸上（否则内容清空但窗口还占着位置）。
/// ⚠️ 全透明 ⇒ 位置不可见，x 固定 0；y 仍走垂直居中，避免窗口在空帧与实帧之间跳。
#[cfg(target_os = "windows")]
fn draw_blank(hwnd: *mut core::ffi::c_void, w: i32) -> bool {
    let w = w.max(1);
    let h = WIDGET_H;
    let Some(dib) = (unsafe { ffi::create_dib(w, h) }) else {
        return false;
    };
    unsafe {
        let px = std::slice::from_raw_parts_mut(dib.bits, (w * h) as usize);
        px.fill(0);
    }
    let ok = unsafe { ffi::commit(hwnd as _, &dib, 0, widget_y_offset()) };
    unsafe { ffi::free_dib(&dib) };
    ok
}

/// **主线程**重绘入口：读快照 → 绘制。由 `wnd_proc` 收到 `WM_APP_REFRESH` 时调用。
#[cfg(target_os = "windows")]
fn repaint_from_snapshot(hwnd: *mut core::ffi::c_void) {
    let items = snapshot::load().unwrap_or_default();
    let ok = draw_items(hwnd, &items);
    if ok {
        REFRESH_COUNT.fetch_add(1, Ordering::Relaxed);
    } else {
        append_log("[widget] repaint 失败");
    }
}

/// 建窗期的**首帧**：还没有数据，先画空内容占位。
///
/// ⚠️ 不在这里同步取数：`spawn_widget` 在 Tauri `setup` 回调里、即**主线程**，
///   而取数要 600ms+ ⇒ 会拖慢启动。改为「先画空帧，再由后台 `refresh_async` 填充」。
#[cfg(target_os = "windows")]
fn draw_frame(hwnd: *mut core::ffi::c_void) -> bool {
    draw_blank(hwnd, PAD_X * 2)
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
///       ② 「用户在设置页取消全部已选设备」时的拆除路径。
///
/// ⛔ **只能在创建窗口的线程（主线程）调用**：`DestroyWindow` 对非创建线程的窗口无效。
#[cfg(target_os = "windows")]
pub fn destroy_widget() {
    let hwnd = WIDGET_HWND.swap(0, Ordering::SeqCst) as *mut core::ffi::c_void;
    forget_widget();
    if !hwnd.is_null() {
        unsafe { ffi::destroy(hwnd) };
        append_log("[widget] destroyed");
    }
}

// ══════════════════════════════════════════════════════════════════════════
// 数据取数（**后台线程**）与刷新编排
// ══════════════════════════════════════════════════════════════════════════

/// 从后端聚合结果构造 widget 条目列表（**纯函数**，可单测）。
///
/// ⭐⭐ **只画「已勾选的设备」**（用户口径 2026-09-24 拍板）。判据 = `d.pinned`。
///
/// ⛔ 为什么必须显式过滤，而不是「照抄 `group_taskbar_devices` 的输出」：
///   那个函数的保留规则是 `pinned || battery.is_some() || audio_device_id.is_some()`，
///   即「**有数据的设备一律显示**」—— 那是**弹窗/托盘列表**要的语义（本机外设概览）。
///   但任务栏窗口的选择入口是设置页的「选择需要在任务栏信息窗口中显示的设备」，
///   用户勾 1 台却看到 6 台 ⇒ **设置页是死的**（真机实测）。
///   ⇒ 两处口径**刻意不同**：`should_show()` 决定「窗口在不在」，本函数决定「画哪几台」。
///
/// ⚠️ **排序**：把「有数据的」排在前面、「pin 但无数据的」沉底 ——
///   任务栏空间有限，把有效信息放在最显眼处；同时顺序**稳定**（同分时保持后端顺序），
///   避免每次刷新条目跳来跳去（对 30s 兜底刷新尤其重要）。
///   ⚠️ pin 但读不出数据的条目**仍然保留**（置灰绘制）：这是 `3dcbdc7` 的
///   「pin = 强制显示」——用户勾了就该看到，哪怕只有 `--`。
#[cfg(target_os = "windows")]
fn build_items(devices: &[crate::device_identity::PhysicalDevice]) -> Vec<WidgetItem> {
    let mut items: Vec<WidgetItem> = devices
        // ⛔ 过滤必须在 map 之前：`pinned` 是「用户勾选」的唯一标记，
        //    它在 `group_taskbar_devices` 里同时覆盖「命中已选」与「反向补建的空占位」。
        .iter()
        .filter(|d| d.pinned)
        .map(|d| WidgetItem {
            name: d.name.clone(),
            icon: d.audio_kind,
            battery: d.battery,
            volume: d.volume,
            // ⭐ 判据必须是 `is_some()`：`Some(0.0)` 是合法音量（静音到 0）
            has_audio: d.volume.is_some() || d.is_muted.is_some() || d.audio_device_id.is_some(),
            is_muted: d.is_muted,
            is_default: d.is_default == Some(true),
            pinned: d.pinned,
        })
        .collect();
    // 稳定排序：有数据的（电量或音频）优先
    items.sort_by_key(|it| it.battery.is_none() && !it.has_audio);
    // ⭐ 截断到显示上限（用户指定 6 台）——**必须在排序之后**，
    //   否则会把「有数据的」截掉、留下「无数据的占位条目」。
    //   `truncate` 保序 ⇒ 与排序一起保证「最该看的 6 台」被留下。
    if items.len() > WIDGET_MAX_ITEMS {
        items.truncate(WIDGET_MAX_ITEMS);
    }
    items
}

/// 判据的**纯函数形式**（可单测，不读全局状态）。
///
/// ⭐ 抽出来的理由：这是「默认关闭」这条用户口径的**唯一落地点**。
///   若将来有人把它改回「有数据就显示」（`group_taskbar_devices` 的旧保留规则），
///   行为会变回「升级后窗口自己冒出来」—— 那种回归没有任何报错，只能靠用例钉住。
#[cfg(target_os = "windows")]
fn wants_widget(c: &crate::config::Config) -> bool {
    !c.pinned_taskbar_devices.is_empty()
}

/// 窗口是否**应该存在**。
///
/// ⭐ 判据 = **已选设备非空**（用户口径 2026-09-24：默认关闭，只有用户选了设备才显示）。
///   ⇒ 刻意**不新增**「启用」布尔字段：`pinned_taskbar_devices` 本身既是「显示哪些」
///   也是「要不要显示」。多一个开关就多一个可能与设备列表不一致的状态。
///
/// ⛔ 与「当前可见」区分：`pinned` 是「强制显示」语义（读不出数据也保留，见 `3dcbdc7`），
///   所以「非空」不等于「一定有内容」—— 但那正是用户要的：pin 了就该看到（哪怕是 `--`）。
pub fn should_show() -> bool {
    crate::config::with_config(wants_widget)
}

/// 按当前配置把 widget 调整到应有的状态：**挂载 / 拆除 / 重定位**。
///
/// ⭐ 抽成单一入口，让「启动期」与「配置变更期」走**同一判据** ——
///   两处各写一遍是典型分叉点，表现为「改了设置要重启才生效」。
///
/// ⛔ 窗口只能在**主线程**创建与销毁 ⇒ 这里经 `run_on_main_thread` **排队**（不等待）。
/// ⛔ 投递必须发生在**读取配置之后**：`with_config` 的锁此刻已释放。
///   持配置锁去投递主线程任务，正是 AGENTS.md 里那条 AB/BA 死锁的构成条件。
pub fn apply_from_config(app: &tauri::AppHandle) {
    #[cfg(target_os = "windows")]
    {
        // ⛔ 先清理**失效句柄**：Explorer 重建会把我们的子窗一起销毁，但原子量里
        //    还留着旧值 ⇒ 不清就会一直判成「已挂载」，永远不会重建（静默不显示）。
        // ⚠️ 这里只动原子量（`forget_widget`），真正的 `DestroyWindow` 由主线程的
        //    `destroy_widget` 负责 —— 本函数可能在事件回调线程上被调用。
        if WIDGET_HWND.load(Ordering::SeqCst) != 0 && !widget_alive() {
            append_log("[widget] 句柄已失效（窗口被系统销毁，如 Explorer 重建）⇒ 复位挂载状态");
            forget_widget();
        }
        let want = should_show();
        let mounted = widget_alive();
        match (want, mounted) {
            // 该显示但没挂 ⇒ 挂上（在主线程）
            (true, false) => {
                let app = app.clone();
                let queued = app.run_on_main_thread(move || {
                    let report = spawn_widget();
                    append_log(&format!(
                        "[widget] 按配置挂载: ok={} hwnd={:#x}",
                        report.ok(),
                        report.hwnd
                    ));
                    // 挂载后立刻首刷：此刻 `WIDGET_HWND` 已就绪，`refresh_async` 不会早退。
                    refresh_async();
                });
                if let Err(e) = queued {
                    append_log(&format!("[widget] 投递挂载任务失败（事件循环已退出）: {e}"));
                }
            }
            // 不该显示但挂着 ⇒ 拆掉（在主线程）
            (false, true) => {
                let app = app.clone();
                let queued = app.run_on_main_thread(|| {
                    destroy_widget();
                    append_log("[widget] 按配置拆除（已无已选设备）");
                });
                if let Err(e) = queued {
                    append_log(&format!("[widget] 投递拆除任务失败（事件循环已退出）: {e}"));
                }
            }
            // 已挂且该挂 ⇒ 重跑一次刷新，让位置/数据按新配置重算。
            // ⚠️ 必须走 `refresh_async` 而不是直接重绘：位置来自**后台**的视觉扫描。
            // ⛔ 且**必须**先置 `FORCE_REPAINT`：改贴靠位置既不改数据也不改槽位，
            //    仅靠 `refresh_async` 会被「无变化不重绘」吃掉（真机实测：
            //    设置页保存成功、窗口却留在原地，日志只有一句「快照无变化」）。
            (true, true) => {
                FORCE_REPAINT.store(true, Ordering::Release);
                refresh_async();
            }
            (false, false) => {}
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = app;
    }
}

/// 同步取一次数据并写入快照。返回**是否需要重绘**。
///
/// ⭐ 返回「是否需要重绘」而不是「数据是否变化」：**槽位移动也要重绘**
///   （否则「改了贴靠位置」不会生效 —— 数据没变 ⇒ 不投递 ⇒ 窗口留在原地）。
///
/// ⛔ **绝不能在主线程调用**（内部跑 WMI 实测 600ms+，且视觉扫描要 BitBlt）。所有调用点都在后台线程。
#[cfg(target_os = "windows")]
fn fetch_into_snapshot() -> bool {
    let started = std::time::Instant::now();
    // 与 `get_taskbar_devices` 命令**同源**：设备走缓存优先 + 空则自愈现查。
    // ⚠️ 这里直接复用 `commands::devices_for_taskbar`（非 pub 时改用同路径的公开入口），
    //    避免 widget 自带一套「缓存该怎么回落」的规则（分叉会静默显示错数据）。
    let devices = match crate::commands::taskbar_devices_snapshot() {
        Some(d) => d,
        None => {
            append_log("[widget] 取数失败（taskbar_devices_snapshot 返回 None），保留旧快照");
            return false;
        }
    };
    let items = build_items(&devices);
    // 视觉扫描必须留在后台：它要 BitBlt 整条任务栏，不能阻塞窗口线程。
    // 估算宽度略保守（中文/图标按最大字符宽），避免实际绘制超出空白槽。
    let wanted = estimate_widget_width(&items);
    let slot_before = (
        SLOT_X.load(Ordering::Acquire),
        SLOT_W.load(Ordering::Acquire),
        SLOT_VALID.load(Ordering::Acquire),
    );
    if let Some((slot_x, slot_w)) = find_widget_slot(wanted) {
        SLOT_X.store(slot_x, Ordering::Release);
        SLOT_W.store(slot_w, Ordering::Release);
        SLOT_VALID.store(true, Ordering::Release);
    } else {
        SLOT_VALID.store(false, Ordering::Release);
    }
    let slot_after = (
        SLOT_X.load(Ordering::Acquire),
        SLOT_W.load(Ordering::Acquire),
        SLOT_VALID.load(Ordering::Acquire),
    );
    let slot_moved = slot_before != slot_after;
    // ⭐ 一次性强制重绘标志（配置变更路径置位）——**必须在这里消费**：
    //   它代表「数据与槽位都没变，但界面必须重画」（例如改了贴靠位置）。
    let forced = FORCE_REPAINT.swap(false, Ordering::AcqRel);
    let changed = snapshot::store(items);
    let elapsed = started.elapsed().as_millis();
    // ⚠️ 只在**有变化**时打日志：30s 兜底会每轮都调，无变化时打日志会淹没有用信息
    if changed {
        append_log(&format!(
            "[widget] 快照更新 ({} 台, {}ms)",
            devices.len(),
            elapsed
        ));
    } else if slot_moved {
        append_log(&format!(
            "[widget] 槽位移动 → 重绘 (x={} w={}, {}ms)",
            slot_after.0, slot_after.1, elapsed
        ));
    } else if forced {
        append_log(&format!("[widget] 配置要求重绘 → 重绘 ({}ms)", elapsed));
    } else {
        append_log(&format!("[widget] 快照无变化 ({}ms)", elapsed));
    }
    changed || slot_moved || forced
}

/// 请求一次刷新（**任意线程可调**）。带**合并窗口**防抖。
///
/// ⭐ 语义 = 「确保最终会被刷新一次」：
///   · 若当前没有刷新在跑 ⇒ 立即起一个后台任务取数；
///   · 若已有刷新在跑 ⇒ 只置 `REFRESH_PENDING`，由正在跑的那个在结束时**补跑一次**。
///   ⇒ `volume-changed` 拖动期间的几十次请求会合并成「当前一次 + 结束后一次」，
///     既不会雪崩，也保证**最后的状态一定被画出来**。
pub fn refresh_async() {
    #[cfg(target_os = "windows")]
    {
        use std::sync::atomic::Ordering as O;
        // 已在跑 ⇒ 只记一个「待补跑」，由跑完的那个负责
        if REFRESH_RUNNING.swap(true, O::SeqCst) {
            REFRESH_PENDING.store(true, O::SeqCst);
            return;
        }
        // ⚠️ 句柄用 `isize` 传给 `spawn_blocking`（`*mut c_void` **不是 `Send`**，
        //    编译器会拒绝；句柄本身只是整数，跨线程传值安全 —— 真正保证「只在主线程
        //    碰窗口」的是**用法**：后台线程只 `PostMessageW`，不直接操作窗口）
        // ⭐ 未挂载（或句柄已失效）⇒ 没有消费方，直接早退。
        //    ⛔ 判据用 `widget_alive()` 而不是 `handle != 0`：句柄失效时若还往下走，
        //       会白跑一次 600ms 的 WMI 取数，最后 `PostMessageW` 静默失败。
        //    ⚠️ 早退前**必须**把 `REFRESH_RUNNING` 放回去，否则后续刷新全被合并掉、永不再跑。
        let handle = WIDGET_HWND.load(Ordering::SeqCst);
        if !widget_alive() {
            REFRESH_RUNNING.store(false, O::SeqCst);
            return;
        }
        tauri::async_runtime::spawn(async move {
            // 用 spawn_blocking 包住阻塞的取数（WMI 是同步阻塞调用，
            // 直接放在 async 任务里会占住运行时的工作线程）
            let _ = tauri::async_runtime::spawn_blocking(move || {
                let changed = fetch_into_snapshot();
                if changed {
                    // ⛔ 跨线程投递：**不能**在后台线程直接 `UpdateLayeredWindow`
                    //    （窗口属于主线程；ULW 更新窗口表面，跨线程调用行为未定义）。
                    unsafe {
                        ffi::post_refresh(handle as *mut core::ffi::c_void);
                    }
                }
            })
            .await;
            // 收尾：清 running；若期间有积压请求 ⇒ 补跑一次（**不递归**，只补一次）
            REFRESH_RUNNING.store(false, O::SeqCst);
            if REFRESH_PENDING.swap(false, O::SeqCst) {
                refresh_async();
            }
        });
    }
}

/// 启动 30s **低频兜底**刷新线程（幂等，重复调用只起一个）。
///
/// ⭐ 为什么还需要它（已有事件驱动）：事件只覆盖**代码主动 emit 的时刻**。
///   「没有事件但数据变了」的情况确实存在 —— 例如蓝牙设备电量自然衰减、
///   系统在后台静默切换默认音频设备。兜底保证这些也会被看到，代价是 30s 一次 WMI。
///
/// ⭐ 本循环**无条件启动**（不再要求「先挂载成功」），因为未挂载时它还兼一个职责：
///   **自愈**。见循环内两条分支。
///
/// ⛔ 参数必须是 `AppHandle`：自愈要走 `apply_from_config`，而窗口只能在主线程建，
///   需要 `run_on_main_thread` 这条通道。
pub fn start_refresh_loop(app: &tauri::AppHandle) {
    #[cfg(target_os = "windows")]
    {
        use std::sync::OnceLock;
        static STARTED: OnceLock<()> = OnceLock::new();
        if STARTED.set(()).is_err() {
            return; // 已启动过
        }
        let app = app.clone();
        std::thread::spawn(move || {
            // 首次延迟 3s：让启动流程（托盘/窗口）先跑完，避免与启动期抢 CPU
            std::thread::sleep(std::time::Duration::from_secs(3));
            loop {
                if widget_alive() {
                    refresh_async();
                } else {
                    // ── 自愈分支：窗口不在（或句柄已失效）──────────────────────
                    // 走到这里的三种情况：
                    //   ① 启动期 `apply_from_config` 的投递丢了（事件循环尚未就绪）；
                    //   ② Explorer 重建把子窗带走；
                    //   ③ 某次 `config-changed` 未送达。
                    // ⚠️ 句柄失效时要先 `forget_widget`，否则 `apply_from_config`
                    //    仍会判成「已挂载」而不重建（见该函数内注释）。
                    if WIDGET_HWND.load(Ordering::SeqCst) != 0 {
                        append_log("[widget] 兜底：句柄已失效 ⇒ 复位挂载状态");
                        forget_widget();
                    }
                    if should_show() {
                        append_log("[widget] 兜底：配置要求显示但未挂载 ⇒ 重新挂载");
                        apply_from_config(&app);
                    }
                }
                std::thread::sleep(std::time::Duration::from_secs(30));
            }
        });
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = app;
    }
}

/// 诊断：当前快照里的条目（供验收脚本确认「事件驱动确实更新了数据」）。
pub fn snapshot_items() -> Vec<WidgetItem> {
    #[cfg(target_os = "windows")]
    {
        snapshot::load().unwrap_or_default()
    }
    #[cfg(not(target_os = "windows"))]
    {
        Vec::new()
    }
}

/// 诊断：累计成功重绘次数。
pub fn refresh_count() -> usize {
    #[cfg(target_os = "windows")]
    {
        REFRESH_COUNT.load(Ordering::Relaxed)
    }
    #[cfg(not(target_os = "windows"))]
    {
        0
    }
}

/// 订阅**已有的**全局事件，把「数据可能变了」转成一次刷新请求（幂等）。
///
/// ⭐ 为什么监听这些事件而不是自己轮询：这些事件正是**后端已经判定「状态变了」的时机**
///   （改配置、设备列表变化、音频端点变化、电量推送），比盲目轮询精确得多，
///   也是「事件驱动」这个选择的落地点。
///
/// ⛔ **用 `listen` 而不是 `once`**：需要长期生效。返回的 `EventId` 有意丢弃 ——
///   本应用没有「取消订阅」的场景（widget 与进程同生命周期）。
///
/// ⚠️ 事件在**异步线程**回调（见 `commands.rs` 顶部注释），而 `refresh_async()`
///   内部会 `PostMessage` 回主线程 ⇒ 回调返回后重绘才发生，回调本身不做重活。
///
/// ⚠️ 必须在**主线程**调用（`app.listen` 要求；`setup` 回调正是主线程）。
///
/// ⭐ **无条件安装**（不再要求「先挂载成功」）—— 这是「设置页改了没用」的关键修复：
///   用户勾选设备的那一刻窗口**还不存在**，若等挂载成功才装监听，就永远收不到
///   那次 `config-changed`（先有鸡还是先有蛋）。未挂载时这些监听的成本仅为
///   「事件到来后一次立即返回的早退」，可以忽略。
pub fn install_event_listeners(app: &tauri::AppHandle) {
    #[cfg(target_os = "windows")]
    {
        // `listen` 由 `Listener` trait 提供，必须显式引入
        use std::sync::OnceLock;
        use tauri::Listener;
        static INSTALLED: OnceLock<()> = OnceLock::new();
        if INSTALLED.set(()).is_err() {
            return;
        }
        // ── ① 数据类事件：只请求一次刷新（未挂载时 `refresh_async` 自己早退）──
        // ⚠️ 事件名必须与 `emit` 处**逐字一致**（大小写/连字符），否则 `listen` 静默
        //    不生效（不报错、不触发）—— 改名前先 grep `emit(` 核对。
        const DATA_EVENTS: [&str; 5] = [
            "volume-changed",        // ⭐ 用户改音量（`audio_notify.rs` 回调）
            "tray-devices-changed",  // 托盘设备列表变化（含 WMI 重新枚举）
            "devices-changed",       // 设备列表变化
            "audio-devices-changed", // 音频端点增删 / 默认设备切换
            "bt-battery-updated",    // 蓝牙电量推送
        ];
        for ev in DATA_EVENTS {
            // 忽略返回值（EventId）：本应用不取消订阅，见函数注释
            let _ = app.listen(ev, move |_| {
                // ⚠️ `volume-changed` 在**拖动音量条时会连续触发**（实测几十次/秒）
                //    ⇒ 这里**必须**走 `refresh_async()` 的合并窗口，绝不能直接取数，
                //    否则会把 WMI（实测 600ms+）打爆、拖滑块直接卡顿。
                refresh_async();
            });
        }
        // ── ② 配置类事件：**不能只刷新** ────────────────────────────────────
        // ⛔ `config-changed` 会改变「窗口**该不该存在**」（固定/取消固定设备），
        //    只刷新的话：新勾的设备不显示、取消完的窗口留在任务栏上。
        //    ⇒ 必须走**统一入口** `apply_from_config`（挂载/拆除/重定位三合一），
        //      与启动期共用同一判据 —— 各写一遍就是「改了设置要重启才生效」的成因。
        let app_for_config = app.clone();
        let _ = app.listen("config-changed", move |_| {
            apply_from_config(&app_for_config);
        });
        append_log(&format!(
            "[widget] 已订阅 {} 个数据变更事件 + config-changed（配置变更走挂载/拆除/重定位统一入口）",
            DATA_EVENTS.len()
        ));
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = app;
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

// ══════════════════════════════════════════════════════════════════════════
// 单测：只测**纯函数**（不建窗、不碰 GDI）
// ══════════════════════════════════════════════════════════════════════════
//
// ⭐ 为什么只挑**纯函数**测（`format_battery` / `format_volume` / `estimate_*` /
//   `build_items` / `align_in_slot` / 图标解码）：
//   它们承载的正是**语义判据**（「0% ≠ 读不出」「没有音量就 N/A」「中文宽度不能低估」
//   「有数据优先排前」「最多 6 台」「靠右是右端贴槽右端」）—— 这些判据一旦写反，
//   界面上仍会「显示点什么」，肉眼难以察觉（正是本项目反复强调的**静默失效**）。
//   绘制路径依赖真实窗口与 GDI，无法在这些单测里覆盖，改由真机截图验收。
#[cfg(all(test, target_os = "windows"))]
mod tests {
    use super::*;
    use crate::device_identity::{AudioKind, PhysicalDevice};

    /// 造一个 `PhysicalDevice`（只填被测字段，其余给无害默认值）。
    fn dev(
        name: &str,
        battery: Option<i32>,
        volume: Option<f32>,
        is_muted: Option<bool>,
        audio_id: Option<&str>,
        pinned: bool,
    ) -> PhysicalDevice {
        PhysicalDevice {
            key: format!("c:{name}"),
            name: name.to_string(),
            battery,
            audio_device_id: audio_id.map(|s| s.to_string()),
            volume,
            is_muted,
            is_default: None,
            audio_endpoint_name: None,
            audio_kind: AudioKind::Pointer,
            categories: Vec::new(),
            node_count: 1,
            pinned,
        }
    }

    fn item(
        battery: Option<i32>,
        volume: Option<f32>,
        has_audio: bool,
        is_muted: Option<bool>,
    ) -> WidgetItem {
        WidgetItem {
            name: "设备".to_string(),
            icon: AudioKind::Speaker,
            battery,
            volume,
            has_audio,
            is_muted,
            is_default: false,
            pinned: false,
        }
    }

    // ── format_battery / format_volume：两行文本的语义 ────────
    //
    // ⭐ 布局口径（用户 2026-09-24）：电量画在图标**右上角**、音量画在图标**右下角**，
    //   **没有可显示的值一律 `N/A`**。两个格式化函数是这两条口径的**唯一落地点**，
    //   故必须各自有用例钉住（绘制路径依赖 GDI，无法单测）。

    /// ⭐ 核心判据：`Some(0)`（电量耗尽）与 `None`（读不出）**必须显示不同**。
    /// 写反了会看不出错 —— 界面只是少一个百分号。
    #[test]
    fn zero_battery_shows_percent_but_unreadable_shows_na() {
        assert_eq!(format_battery(&item(Some(0), None, false, None)), "0%");
        assert_eq!(
            format_battery(&item(None, None, false, None)),
            "N/A",
            "读不出电量必须是 `N/A`，不能是 `0%`（两者语义相反）"
        );
    }

    /// ⭐ 用户口径：**没有音量就显示 `N/A`**。
    /// 无音频端点的设备（键鼠）与「有端点但暂时读不出」**都**是 `N/A`。
    ///
    /// 可证伪：把 `format_volume` 的 `!has_audio` 分支改成返回空串，本条立刻转红。
    #[test]
    fn missing_volume_always_shows_na() {
        // 无音频端点（`has_audio = false`）
        assert_eq!(
            format_volume(&item(Some(60), None, false, None)),
            "N/A",
            "无音频端点的设备必须显示 N/A（用户明确要求），不能留空"
        );
        // 有音频端点但音量暂时读不出
        assert_eq!(
            format_volume(&item(None, None, true, None)),
            "N/A",
            "有端点但读不出音量，同样是 N/A"
        );
    }

    /// 电量与音量是**两个独立字段**：任一侧缺失不影响另一侧的显示。
    /// （这是「分列图标右上/右下」这个布局改动引入的新语义 —— 旧的单串格式化
    ///  做不到「只缺一边」时的独立降级。）
    #[test]
    fn battery_and_volume_degrade_independently() {
        // 有电量、无音量
        assert_eq!(format_battery(&item(Some(77), None, false, None)), "77%");
        assert_eq!(format_volume(&item(Some(77), None, false, None)), "N/A");
        // 无电量、有音量
        assert_eq!(
            format_battery(&item(None, Some(0.5), true, Some(false))),
            "N/A"
        );
        assert_eq!(
            format_volume(&item(None, Some(0.5), true, Some(false))),
            "50%"
        );
    }

    /// 静音优先于百分比：静音时显示「静音」而不是端点里那个与听觉不符的旧音量值。
    #[test]
    fn muted_device_shows_muted_instead_of_percent() {
        assert_eq!(
            format_volume(&item(Some(80), Some(0.4), true, Some(true))),
            "静音"
        );
        // ⚠️ 不能只断言「不含 40%」这类弱判据 —— 必须断言**就是**「静音」，
        //    否则「静音 + 百分比同时出现」这种回归照样能通过。
    }

    /// 音量取整：`0.4` ⇒ `40%`，`0.996` ⇒ `100%`（防止 99% 因截断显示成 99 而非 100）。
    #[test]
    fn volume_is_rounded_to_nearest_percent() {
        assert_eq!(
            format_volume(&item(None, Some(0.4), true, Some(false))),
            "40%"
        );
        assert_eq!(
            format_volume(&item(None, Some(0.996), true, Some(false))),
            "100%"
        );
        // 0.0 且未静音 = 音量真为 0 ⇒ 显示 0%（合法，不是 N/A）
        assert_eq!(
            format_volume(&item(None, Some(0.0), true, Some(false))),
            "0%"
        );
    }

    /// ⭐ 两段文本**都不含设备名**（用户口径：名字以后再说）。
    /// 可证伪：若哪天有人把 `it.name` 加回格式化函数，这条会立刻转红。
    #[test]
    fn metric_texts_do_not_contain_device_name() {
        let mut it = item(Some(50), Some(0.5), true, Some(false));
        it.name = "罗技MX Master".to_string();
        for s in [format_battery(&it), format_volume(&it)] {
            assert!(
                !s.contains("罗技") && !s.contains("MX"),
                "widget 上不应再出现设备名: {s}"
            );
        }
    }

    // ── 宽度估算：避让扫描的 wanted（方向性：宁可高估，不可低估） ──

    /// ⛔ 中文**不得**被低估 —— 否则 `find_widget_slot` 可能返回比实际内容更窄的槽，
    /// 内容溢出压到邻居上（正是避让机制要避免的那件事）。
    ///
    /// 可证伪：把 `estimate_text_px` 里非 ASCII 的 `14` 改回 `8`，本条立刻转红。
    #[test]
    fn cjk_is_not_underestimated() {
        // 「静音」是 2 个全角字：11px 字体下实际约 22px ⇒ 估算必须 >= 22
        assert!(
            estimate_text_px("静音") >= 22,
            "中文估算过小会低估槽宽: {}",
            estimate_text_px("静音")
        );
        // 与纯 ASCII 段对比：同样 2 个码位，中文必须更宽
        assert!(
            estimate_text_px("静音") > estimate_text_px("N/A"),
            "中文段必须比等长的 ASCII 段估得更宽"
        );
        // ASCII 侧维持原口径（`100%` = 4 × 8 = 32）
        assert_eq!(estimate_text_px("100%"), 32);
    }

    /// 宽度估算必须**单调**，且单台时至少装得下「图标 + 间隙」。
    #[test]
    fn estimate_width_is_monotonic_and_covers_icon() {
        let one = vec![item(Some(50), Some(0.5), true, Some(false))];
        let mut two = one.clone();
        two.push(item(Some(60), Some(0.6), true, Some(false)));

        let w1 = estimate_widget_width(&one);
        let w2 = estimate_widget_width(&two);
        assert!(
            w1 >= PAD_X * 2 + ICON_PX + ICON_TEXT_GAP,
            "单台至少要装下内边距 + 图标 + 间隙: {w1}"
        );
        assert!(w2 > w1, "多一台必须更宽: {w1} vs {w2}");
    }

    /// ⛔ 每台设备的宽度必须按**两段里更宽的那段**算。
    ///
    /// 按较窄的那段算 ⇒ 估算宽度小于实际绘制宽度 ⇒ `find_widget_slot` 选出的槽偏窄
    /// ⇒ 内容溢出压到邻居上（且不报错，只能靠肉眼看出来）。
    ///
    /// ⚠️ **样本必须让两段宽度不等**：若样本里两段一样宽（如都用 `50%`），
    ///   `min` 与 `max` 同解 ⇒ 本条用例**失去区分力**（注入 `min` 仍绿，实测过）。
    /// 可证伪：把 `estimate_widget_width` 里的 `bat.max(vol)` 改成 `bat.min(vol)`，本条转红。
    #[test]
    fn per_item_width_uses_the_wider_of_the_two_texts() {
        // 电量 `5%`（2 字符 → 16px）比音量 `50%`（3 字符 → 24px）窄
        let it = item(Some(5), Some(0.5), true, Some(false));
        let wider = estimate_text_px("50%");
        let narrower = estimate_text_px("5%");
        assert!(
            wider > narrower,
            "样本本身要能区分宽窄，否则本条无区分力（{wider} vs {narrower}）"
        );
        let w = estimate_widget_width(&[it]);
        assert!(
            w >= PAD_X * 2 + ICON_PX + ICON_TEXT_GAP + wider,
            "宽度必须覆盖更宽的那段文本：{w} < 内边距 + 图标 + 间隙 + {wider}"
        );
    }

    /// 6 台（显示上限）的估算必须仍能塞进避让后的可用区 ——
    /// 否则「避让扫描永远找不到槽」⇒ widget 直接不显示（且不报错）。
    #[test]
    fn six_items_still_fit_in_a_plausible_slot() {
        let six: Vec<WidgetItem> = (0..WIDGET_MAX_ITEMS)
            .map(|i| item(Some(50 + i as i32), Some(0.5), true, Some(false)))
            .collect();
        let w = estimate_widget_width(&six);
        // 真机可用区约 1300px（任务栏 2560px 减两端各 100px 再减任务栏自身内容）
        assert!(w < 1300, "6 台估算过宽，会找不到避让槽: {w}");
    }

    // ── build_items：从后端设备构造条目 ─────────────────────
    //
    // ⭐ 下面所有用例的样本都用 `pinned_dev(...)` 造：`build_items` 现在**只画已勾选
    //   的设备**（用户口径 2026-09-24），用 `dev(..., false)` 造样本会得到空列表
    //   ⇒ 用例失去区分力（索引越界 panic 而不是断言失败，掩盖真实缺陷）。

    /// 造一台**已勾选**的设备（= `dev(..., pinned = true)`）。
    fn pinned_dev(
        name: &str,
        battery: Option<i32>,
        volume: Option<f32>,
        is_muted: Option<bool>,
        audio_id: Option<&str>,
    ) -> PhysicalDevice {
        dev(name, battery, volume, is_muted, audio_id, true)
    }

    /// ⛔⛔ **核心口径**（用户 2026-09-24 拍板）：任务栏窗口**只画已勾选的设备**。
    ///
    /// ⭐ 为什么单列一条：`group_taskbar_devices` 的保留规则是「有数据的设备一律留」，
    ///   若直接照抄它的输出，用户勾 1 台却看到 6 台 ⇒ **设置页形同虚设**（真机实测）。
    /// 可证伪：去掉 `build_items` 里的 `.filter(|d| d.pinned)`，本条立刻转红。
    #[test]
    fn only_pinned_devices_are_rendered() {
        let input = vec![
            dev("未勾选-鼠标", Some(80), None, None, None, false),
            dev(
                "已勾选-音箱",
                None,
                Some(0.5),
                Some(false),
                Some("ep"),
                true,
            ),
            dev("未勾选-耳机", Some(60), Some(0.4), None, Some("ep2"), false),
        ];
        let items = build_items(&input);
        assert_eq!(items.len(), 1, "只有已勾选的设备才该进条目，实际 {items:?}");
        assert_eq!(items[0].name, "已勾选-音箱");
        assert!(items[0].pinned);
    }

    /// 一台都没勾 ⇒ 条目为空（`draw_items` 会画一帧全透明并隐藏窗口）。
    #[test]
    fn no_pinned_device_yields_empty_items() {
        let input = vec![
            dev("a", Some(80), None, None, None, false),
            dev("b", None, Some(0.5), None, Some("ep"), false),
        ];
        assert!(build_items(&input).is_empty());
    }

    /// `has_audio` 的判据必须覆盖三种来源之一（音量 / 静音 / 端点 id），
    /// 否则「音量暂时读不出但有端点」的设备会被误判成无音频。
    #[test]
    fn has_audio_true_when_any_audio_signal_present() {
        let cases = [
            pinned_dev("a", None, Some(0.5), None, None),
            pinned_dev("b", None, None, Some(false), None),
            pinned_dev("c", None, None, None, Some("ep-1")),
        ];
        for d in cases {
            let items = build_items(&[d.clone()]);
            assert!(items[0].has_audio, "{} 应判为有音频", d.name);
        }
        // 三者皆无 ⇒ 无音频
        let items = build_items(&[pinned_dev("kbd", Some(50), None, None, None)]);
        assert!(!items[0].has_audio);
    }

    /// ⭐ `audio_kind` 必须从后端**原样透传**到 widget 条目（图标就靠它）。
    #[test]
    fn audio_kind_is_carried_through() {
        let mut ear = pinned_dev("耳机", Some(70), Some(0.5), Some(false), Some("ep"));
        ear.audio_kind = AudioKind::Headphones;
        let mut spk = pinned_dev("音箱", None, Some(0.3), Some(false), Some("ep2"));
        spk.audio_kind = AudioKind::Speaker;
        let items = build_items(&[ear, spk]);
        assert_eq!(items[0].icon, AudioKind::Headphones);
        assert_eq!(items[1].icon, AudioKind::Speaker);
    }

    /// 「有数据的排前面」：pin 但无任何数据的条目必须沉底，
    /// 且排序**稳定**（同组内保持原顺序）—— 否则 30s 兜底刷新会让条目跳来跳去。
    #[test]
    fn items_with_data_sort_before_data_less_ones() {
        let input = vec![
            pinned_dev("空1", None, None, None, None), // 无数据
            pinned_dev("鼠标", Some(80), None, None, None),
            pinned_dev("空2", None, None, None, None), // 无数据
            pinned_dev("音箱", None, Some(0.5), Some(false), Some("ep")),
        ];
        let items = build_items(&input);
        assert_eq!(items.len(), 4);
        // 前两台必须是有数据的
        assert_eq!(items[0].name, "鼠标");
        assert_eq!(items[1].name, "音箱");
        // 后两台是空条目，且保持输入顺序（稳定排序）
        assert_eq!(items[2].name, "空1");
        assert_eq!(items[3].name, "空2");
    }

    /// 电量 `Some(0)` 也算「有数据」—— 耗尽电量比「读不出」更该显示在前面。
    #[test]
    fn zero_battery_still_counts_as_having_data() {
        let input = vec![
            pinned_dev("空", None, None, None, None),
            pinned_dev("耗尽", Some(0), None, None, None),
        ];
        let items = build_items(&input);
        assert_eq!(items[0].name, "耗尽", "电量 0% 也应排在前（它是有效数据）");
    }

    /// ⭐ 最多显示 6 台（用户指定）：**截断发生在排序之后** ⇒ 留下的是「最该看的 6 台」，
    /// 而不是输入顺序的前 6 台。可证伪：把 `truncate` 移到 `sort` 之前，本条会转红。
    #[test]
    fn at_most_six_items_and_keeps_the_data_rich_ones() {
        // 7 台，其中「空」排在最前面（输入序第一），但它无数据 ⇒ 应被排到末尾再截掉
        let mut input = vec![pinned_dev("空", None, None, None, None)];
        for i in 0..6 {
            input.push(pinned_dev(
                &format!("有数据{i}"),
                Some(50 + i),
                None,
                None,
                None,
            ));
        }
        let items = build_items(&input);
        assert_eq!(items.len(), 6, "必须截断到 6 台");
        assert!(
            !items.iter().any(|it| it.name == "空"),
            "无数据的条目应在截断中被丢弃（说明截断发生在排序之后）"
        );
    }

    /// 不足 6 台时不截断，且顺序不变。
    #[test]
    fn fewer_than_six_items_are_not_truncated() {
        let input = vec![
            pinned_dev("a", Some(1), None, None, None),
            pinned_dev("b", Some(2), None, None, None),
        ];
        assert_eq!(build_items(&input).len(), 2);
    }

    /// `pinned` / `is_default` 必须原样透传（widget 据此置灰 / 标记）。
    #[test]
    fn pinned_and_default_flags_are_carried_through() {
        let mut d = pinned_dev("耳机", Some(70), None, None, None);
        d.is_default = Some(true);
        let items = build_items(&[d]);
        assert!(items[0].pinned);
        assert!(items[0].is_default);
    }

    // ── 快照 store/load ──────────────────────────────────

    /// `store` 必须能识别「内容相同」⇒ 返回 false（避免无谓重绘）。
    #[test]
    fn snapshot_store_reports_no_change_for_identical_content() {
        let a = vec![item(Some(80), None, false, None)];
        // 先写一次（无论之前有什么，这次一定"变化"或幂等，取两次比较）
        let _ = snapshot::store(a.clone());
        assert!(!snapshot::store(a.clone()), "内容相同应返回 false");
        // 内容变了 ⇒ true
        let b = vec![item(Some(79), None, false, None)];
        assert!(snapshot::store(b), "内容变化应返回 true");
    }

    /// ⭐ 图标解码 + 缩放：三种类别 × 两套主题都必须**解码成功**（否则真机上会「什么都不画」
    /// 且不报错 —— 正是最难发现的静默失效）。可证伪：删掉任一张 PNG，本条立刻转红。
    #[test]
    fn all_icon_assets_decode_and_scale() {
        for kind in [
            AudioKind::Pointer,
            AudioKind::Speaker,
            AudioKind::Headphones,
        ] {
            for dark in [false, true] {
                let src = icons::get(kind, dark)
                    .unwrap_or_else(|| panic!("图标解码失败: {kind:?} dark={dark}"));
                assert!(src.1 > 0 && src.2 > 0, "解码出的尺寸不能为 0");
                let scaled = icons::scale_to(src, 16).expect("缩放到 16px 不应失败");
                assert_eq!((scaled.1, scaled.2), (16, 16));
                assert_eq!(scaled.0.len(), 16 * 16 * 4, "RGBA 缓冲长度必须是 w*h*4");
            }
        }
    }

    /// 缩放必须**真的**产出非全透明内容（防止「解码成功但像素全 0」这种假绿）。
    #[test]
    fn scaled_icon_has_visible_pixels() {
        let src = icons::get(AudioKind::Speaker, false).expect("扬声器图标应解码成功");
        let (px, ..) = icons::scale_to(src, 16).unwrap();
        let opaque = px.chunks(4).filter(|c| c[3] > 0).count();
        assert!(
            opaque > 16,
            "扬声器图标缩放后应有相当数量的不透明像素，实际 {opaque}"
        );
    }

    // ── align_in_slot：贴靠位置（纯函数）──────────────────────
    //
    // ⭐ 为什么必须测：三种策略**写反了不报错**，只是窗口跑到别处。
    //   最危险的形态是「`right` 写成 `slot_x + slot_w`」—— 不越界检查的话，
    //   窗口右端会**越过槽右端**压到邻居身上，而视觉上只表现为「贴得紧一点」。

    /// 三种策略在同一槽内的相对位置必须**互不相同**且顺序正确。
    ///
    /// 可证伪：把 `right` 的 `max_offset` 写成 `0`（与 left 相同）本条立刻转红。
    #[test]
    fn align_in_slot_orders_left_center_right() {
        let (slot_x, slot_w, content_w) = (600, 300, 100);
        let left = align_in_slot(slot_x, slot_w, content_w, "left");
        let center = align_in_slot(slot_x, slot_w, content_w, "center");
        let right = align_in_slot(slot_x, slot_w, content_w, "right");
        assert_eq!(left, slot_x, "靠左 = 槽左端");
        assert!(left < center && center < right, "三者必须严格递增");
        assert_eq!(center, slot_x + 100, "居中 = 槽左端 + 剩余/2");
    }

    /// ⭐ 真正的语义判据：**右端贴槽右端**（而不是「起点贴槽右端」）。
    ///
    /// 这是最容易写错的一处：`right` 若返回 `slot_x + slot_w`，内容会整段溢出到槽外，
    /// 且 `left < center < right` 那条用例**仍然通过** —— 必须靠本条钉住。
    #[test]
    fn align_in_slot_right_aligns_content_right_edge_to_slot() {
        let (slot_x, slot_w, content_w) = (600, 300, 100);
        let x = align_in_slot(slot_x, slot_w, content_w, "right");
        assert_eq!(
            x + content_w,
            slot_x + slot_w,
            "靠右后内容右端应恰好等于槽右端（不得溢出）"
        );
    }

    /// 未知值（防御分支）与 `center` 同解 —— `normalize_config` 已兜住非法值，
    /// 这里只保证「万一漏进来一个奇怪的值，也不会算出槽外坐标」。
    #[test]
    fn align_in_slot_unknown_position_falls_back_to_center() {
        let args = (600, 300, 100);
        assert_eq!(
            align_in_slot(args.0, args.1, args.2, "middle"),
            align_in_slot(args.0, args.1, args.2, "center"),
            "未知贴靠值必须退化为居中（而不是越界或 panic）"
        );
    }

    /// 内容比槽宽时（`wanted` 筛选本应挡住，属防御分支）：`max_offset` 归零 ⇒
    /// 三种策略**都**退化为贴槽左端，且**绝不返回负偏移**（负偏移会把窗口推到任务栏之外）。
    #[test]
    fn align_in_slot_never_returns_negative_offset_when_content_overflows() {
        let (slot_x, slot_w, content_w) = (600, 80, 200);
        for pos in ["left", "center", "right", "unknown"] {
            let x = align_in_slot(slot_x, slot_w, content_w, pos);
            assert_eq!(x, slot_x, "槽装不下时 `{pos}` 应退化为贴槽左端");
        }
    }

    // ── wants_widget：窗口「该不该存在」的判据 ─────────────────

    /// ⭐ 用户口径（2026-09-24）：「默认关闭，只有用户选择了设备时才开启显示」。
    ///
    /// 可证伪：把判据改成 `true`（或改成旧的「有数据就显示」）本条立刻转红。
    /// ⚠️ 这条口径**改变**了既有行为 —— 未固定任何设备的用户升级后窗口**默认不出现**，
    ///   这正是刻意要的（窗口不再自己冒出来）。
    #[test]
    fn widget_shows_only_when_devices_are_pinned() {
        use crate::config::{Config, PinnedDevice};

        let empty = Config::default();
        assert!(
            !wants_widget(&empty),
            "未固定任何设备 ⇒ 窗口不应存在（默认关闭）"
        );

        let pinned = Config {
            pinned_taskbar_devices: vec![PinnedDevice {
                key: "c:abc".to_string(),
                fallback: None,
                alias: None,
            }],
            ..Default::default()
        };
        assert!(wants_widget(&pinned), "已固定设备 ⇒ 窗口应存在");
    }
}
