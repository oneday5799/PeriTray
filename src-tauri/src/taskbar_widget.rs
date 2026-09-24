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
//!   里程碑 4（已完成）：**布局重构** —— 图标放大到 32px，电量画图标**右上角**、
//!     音量画**右下角**，缺失一律 `N/A`；widget 在任务栏内**垂直居中**。
//!   里程碑 5（已完成）：**手动拖拽** —— 关掉「固定位置」后可拖动，落点落盘（`taskbar_custom_x`）。
//!   里程碑 6（已完成）：**Z 序维护** —— 挂载时显式 `HWND_TOP` 提顶，之后每 2s **幂等**
//!     重申（`WM_APP_RAISE`）；并把「任务栏换了句柄」（= Explorer 重建）提升为
//!     **立刻重建**的触发条件，不再等 30s 慢节拍。
//!   刻意**不含**：多显示器（本机无 `Shell_SecondaryTrayWnd`，无法验证）。
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

// ══════════════════════════════════════════════════════════════════════
// 布局度量：**标称值一律是 DIP**（96 DPI 基准），绘制前按实际 DPI 换算
// ══════════════════════════════════════════════════════════════════════
//
// ⭐⭐ 为什么必须按 DPI 换算（用户 2026-09-25 报「底衬高度不对」的根因）：
//   FluentFlyout 是 WPF 程序，`TaskbarWidgetControl.xaml` 里的 `Height="40"`
//   是**设备无关单位**，由 `Windows/TaskbarWindow.xaml.cs` 换算成物理像素：
//       double dpiScale = GetDpiForWindow(taskbarHandle) / 96.0;
//       int physicalHeight = (int)(logicalHeight * dpiScale);   // 40 × 1.25 = 50
//   我们原先把 40 直接当**物理像素**用 ⇒ 125% 缩放下底衬比它矮 **10px**。
//   ⚠️ 只换算高度是不够的：内容（图标/字号/间距）不跟着换算就会显得空 ——
//      FluentFlyout 的封面图同样是 `36 DIP × dpiScale`，故**整套**一起换算。
//
// ⛔ 标称值只在本段定义一次；**绘制与测宽必须取同一份 `Metrics`**，
//   各写各的换算必然漂移（一处改了另一处忘了 ⇒ 相邻项重叠，且不报错）。

/// widget 高度（DIP）。⭐ 逐字等于 FluentFlyout 控件的 `Height="40"`。
#[cfg(target_os = "windows")]
const WIDGET_H_DIP: i32 = 40;

/// 内容区左右内边距（DIP）—— 与任务栏左右两端各留一段（避让口径见 §E）。
#[cfg(target_os = "windows")]
const PAD_X_DIP: i32 = 6;

/// 字体像素高度（DIP）—— 电量/音量两行共用。
#[cfg(target_os = "windows")]
const FONT_PX_DIP: i32 = 11;

/// 图标边长（DIP）。图标源 PNG 本身就是 32×32 ⇒ 100% 缩放下是**恒等拷贝**。
#[cfg(target_os = "windows")]
const ICON_PX_DIP: i32 = 32;

/// 图标与右侧两行文本之间的间隔（DIP）。
#[cfg(target_os = "windows")]
const ICON_TEXT_GAP_DIP: i32 = 4;

/// 设备之间的水平间隔（DIP）。
#[cfg(target_os = "windows")]
const ITEM_GAP_DIP: i32 = 10;

/// 单段文本的宽度上限（DIP）—— 极端长文本截断显示，避免挤压其它设备。
#[cfg(target_os = "windows")]
const ITEM_MAX_W_DIP: i32 = 150;

/// hover 底衬的圆角半径（DIP）。
///
/// ⭐ 逐字等于 FluentFlyout `Controls/TaskbarWidgetControl.xaml` 里 `MainBorder` 的
///   `CornerRadius="6"`（**同一份口径**，别再各写各的）。
///   圆角而不是直角：直角矩形贴在任务栏上像一个「色块 bug」，圆角读起来像有意画的「胶囊」。
#[cfg(target_os = "windows")]
const BACKDROP_RADIUS_DIP: i32 = 6;

/// 把上表的 DIP 标称值换算成**当前 DPI 下的物理像素**。
///
/// ⚠️ 抽成结构体而不是散落的 `* scale` 乘法：散落写法在后续加常量时**必然漏一处**
///   （宽度按一套算、绘制按另一套画 ⇒ 相邻项重叠，且不报错、不 panic）。
#[cfg(target_os = "windows")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Metrics {
    /// 换算所用的 DPI（96 = 100%）。留给「按 DIP 现算」的调用点用（见 `dip`）。
    pub dpi: u32,
    pub h: i32,
    pub radius: i32,
    pub icon: i32,
    pub font: i32,
    pub pad_x: i32,
    pub icon_text_gap: i32,
    pub item_gap: i32,
    pub item_max_w: i32,
    /// 图标右侧每行文本的掩码高度 = 图标高度的一半。
    /// ⭐ 电量占**上半行**（= 图标的右上角）、音量占**下半行**（= 图标的右下角）
    ///   —— 用户 2026-09-24 指定的布局。
    pub text_row_h: i32,
}

#[cfg(target_os = "windows")]
impl Metrics {
    /// 按给定 DPI 换算（**纯函数**，可单测）。`dpi == 0` 视为 96（防御）。
    pub fn for_dpi(dpi: u32) -> Self {
        let dpi = if dpi == 0 { 96 } else { dpi };
        let s = |dip: i32| Self::dip_of(dpi, dip);
        let icon = s(ICON_PX_DIP);
        Self {
            dpi,
            h: s(WIDGET_H_DIP),
            radius: s(BACKDROP_RADIUS_DIP),
            icon,
            font: s(FONT_PX_DIP),
            pad_x: s(PAD_X_DIP),
            icon_text_gap: s(ICON_TEXT_GAP_DIP),
            item_gap: s(ITEM_GAP_DIP),
            item_max_w: s(ITEM_MAX_W_DIP),
            text_row_h: icon / 2,
        }
    }

    /// DIP → 物理像素（四舍五入）。`dpi == 0` 视为 96。
    ///
    /// ⚠️ 单独抽出来是给**不在本表里**的 DIP 值用（如 `estimate_text_px` 的
    ///   每字符宽度估算）—— 那些值按字号比例缩放，不适合塞进固定字段。
    pub fn dip(&self, dip: i32) -> i32 {
        Self::dip_of(self.dpi, dip)
    }

    fn dip_of(dpi: u32, dip: i32) -> i32 {
        let dpi = if dpi == 0 { 96 } else { dpi };
        ((dip as f32) * dpi as f32 / 96.0).round() as i32
    }

    /// 当前任务栏 DPI 下的度量。
    ///
    /// ⚠️ 取**任务栏**的 DPI 而不是进程/桌面的：widget 是任务栏的子窗，
    ///   多显示器「各屏缩放不同」时只有任务栏所在屏的 DPI 是对的。
    ///   取不到（Explorer 重建间隙）⇒ 回落 96（宁可小一号，也不要按错的缩放错位）。
    pub fn current() -> Self {
        Self::for_dpi(taskbar_dpi())
    }
}

/// 任务栏所在显示器的 DPI（取不到回落 96）。
#[cfg(target_os = "windows")]
fn taskbar_dpi() -> u32 {
    let taskbar = taskbar_hwnd() as *mut core::ffi::c_void;
    if taskbar.is_null() {
        return 96;
    }
    let dpi = unsafe { windows_sys::Win32::UI::HiDpi::GetDpiForWindow(taskbar) };
    if dpi == 0 {
        96
    } else {
        dpi
    }
}

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

/// 最近一次**尝试挂载**时看到的任务栏句柄（0 = 还没试过）。
///
/// ⭐ 唯一用途：区分两种「窗口不在」——
///   · **任务栏句柄变了** ⇒ Explorer 重建 ⇒ **立刻**重建窗口（否则用户要盯着空任务栏
///     等满 30s 的慢节拍）；
///   · **句柄没变** ⇒ 同一个任务栏上挂不上（环境拦截，PLAYBOOK §E3）⇒ 退回 30s 慢节拍，
///     ⛔ **不能**每 2s 重试一次 —— 实测「反复操作任务栏 → 恶化；静置 → 自愈」。
///
/// ⛔ 必须在**每次尝试之后**（成功或失败）都写入：只在成功时写的话，失败后句柄一直是旧值
///   ⇒ 每 tick 都判成「重建了」⇒ 退化成 2s 一次的挂载风暴，正好踩中上面那条。
#[cfg(target_os = "windows")]
static MOUNTED_TASKBAR: AtomicIsize = AtomicIsize::new(0);

// ── 刷新编排 ────────────────────────────────────────────────────────────

/// 投递给主线程的自定义消息：**「快照已更新，请重绘」**。
///
/// ⚠️ 取 `WM_APP + 1`（`WM_APP` = `0x8000`）—— 这是**应用私有消息**的规范起点，
///   系统不会占用；且与 `WM_PAINT` 不同，分层窗口会正常投递到我们的 `wnd_proc`。
#[cfg(target_os = "windows")]
const WM_APP_REFRESH: u32 = 0x8000 + 1;

/// 投递给主线程的自定义消息：**「请重申 Z 序」**。
///
/// ⭐ 为什么单独一条消息（而不是复用 `WM_APP_REFRESH`）：
///   两者**成本与频率差两个数量级** —— 重绘要建 DIB + 提交（每 30s 或数据变化时），
///   而重申 Z 序只是一次 `GetWindow` 查询（维护节拍，2s 一次，且**幂等**：
///   已经最顶就什么都不做）。合成一条会让「重申」被迫跟着重绘走，白白多画一帧。
///
/// ⛔ 也**不能**由维护线程直接调 `SetWindowPos` —— 窗口属于创建它的线程（主线程）。
#[cfg(target_os = "windows")]
const WM_APP_RAISE: u32 = 0x8000 + 2;

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

// ── hover 状态（**跨线程**：hover 线程写、主线程绘制时读）──────────────────

/// 光标是否**悬停**在 widget 上（决定要不要铺 hover 底衬）。
///
/// ⛔⛔ **为什么必须轮询 `GetCursorPos`，不能等 `WM_MOUSEMOVE`**（2026-09-25 定）：
///   `ULW` 分层窗按 alpha 做命中测试 —— 没有底衬时窗口像素**全透明**
///   ⇒ 鼠标消息**根本不会投递到本窗口**（被放行给下层 `Shell_TrayWnd`）
///   ⇒ 「靠鼠标消息让底衬出现」是**鸡生蛋**，永远等不到第一条消息。
///   轮询光标位置**绕开命中测试**，从外部观测「鼠标在不在我身上」。
/// ⭐ 自洽性：一旦 hover 成立 ⇒ 底衬铺上（alpha > 0）⇒ 命中测试**开始生效**
///   ⇒ 此刻按下左键能收到 `WM_LBUTTONDOWN` ⇒ **拖拽仍然可用**（不需要另开机制）。
/// ⚠️ 拖拽期间恒为 `true`（见 `want_hover`）：拖拽靠 `SetCapture` 维持，
///   光标短暂离开窗口时不该让底衬闪烁。
#[cfg(target_os = "windows")]
static HOVERED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

// ── 拖拽状态（全部只在**主线程**读写，用原子量是为了免锁、免锁序登记）──────
//
// ⚠️ 为什么不用 `Mutex`：本模块所有鼠标消息都投递到**创建窗口的那个线程**
//   （= 主线程），不存在跨线程竞争；而 `Mutex` 会引入锁序登记与中毒处理的负担
//   （见 `state.rs` 模块文档）。用原子量表达「单线程状态机」最省事也最不易错。

/// 是否正在拖拽（`WM_LBUTTONDOWN` 起、`WM_LBUTTONUP` / `WM_CAPTURECHANGED` 止）。
#[cfg(target_os = "windows")]
static DRAG_ACTIVE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// 按下那一刻的光标屏幕 x（物理像素）—— 拖拽位移的**参考原点**。
#[cfg(target_os = "windows")]
static DRAG_ORIGIN_CURSOR_X: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(0);

/// 按下那一刻的窗口左端（**父窗客户区**坐标，物理像素）—— 即 `SetWindowPos` 用的那个坐标系。
///
/// ⚠️ 与 `DRAG_ORIGIN_CURSOR_X`（**屏幕**坐标）不是同一坐标系，但拖拽只用到**增量**：
///   光标 Δ 与窗口 Δ 在纯平移下相等（同一 DPI、无缩放）⇒ 直接相加成立。
///   ⛔ 绝不能把它当屏幕坐标去和任务栏屏幕左端做减法 —— 口径换算一律走 `window_parent_x`。
#[cfg(target_os = "windows")]
static DRAG_ORIGIN_WIN_X: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(0);

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

    use super::{Metrics, WM_APP_RAISE, WM_APP_REFRESH};
    use windows_sys::Win32::Foundation::{HWND, POINT, SIZE};
    use windows_sys::Win32::Graphics::Gdi::{
        CreateCompatibleDC, CreateDIBSection, CreateFontW, DeleteDC, DeleteObject, DrawTextW,
        SelectObject, SetBkMode, SetTextColor, BITMAPINFO, BITMAPINFOHEADER, BI_RGB,
        DIB_RGB_COLORS, HBITMAP, HDC, HFONT, HGDIOBJ, TRANSPARENT,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DestroyWindow, GetWindow, GetWindowLongPtrW, PostMessageW,
        RegisterClassW, SetParent, SetWindowLongPtrW, SetWindowPos, ShowWindow,
        UpdateLayeredWindow, GWL_STYLE, GW_HWNDPREV, HWND_TOP, SWP_NOACTIVATE, SWP_NOMOVE,
        SWP_NOSIZE, SWP_NOZORDER, ULW_ALPHA, WM_CAPTURECHANGED, WM_LBUTTONDOWN, WM_LBUTTONUP,
        WM_MOUSEMOVE, WNDCLASSW, WS_CHILD, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
        WS_POPUP,
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
        // ⚠️ 这里的高度只是**建窗占位**：真正的尺寸由首次 `UpdateLayeredWindow` 设定
        //    （`commit` 的 SIZE 参数同时改窗口形状）。仍按 DPI 取，免得首帧明显不对。
        CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            class_name_wide,
            std::ptr::null(),
            WS_POPUP,
            0,
            0,
            1,
            Metrics::current().h,
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

    /// 让分层窗口真正显示（可见四条件里的「调一次 `SetWindowPos`」）。
    ///
    /// ⚠️ 这里**刻意不动 Z 序**（`SWP_NOZORDER` ⇒ 插入位置参数被忽略，故传 `NULL`）：
    ///   本函数的职责只有「让分层窗显示」这一条。Z 序由 `raise_to_top` 单独负责 ——
    ///   早先这里传的是 `HWND_TOPMOST`，**看起来**在设置顶，实际被 `SWP_NOZORDER`
    ///   忽略 ⇒ 是个**死参数**（从未生效，却让人以为 Z 序已经被设置过）。
    pub unsafe fn show(hwnd: HWND) {
        SetWindowPos(
            hwnd,
            std::ptr::null_mut(),
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_NOZORDER,
        );
        ShowWindow(hwnd, 5 /* SW_SHOW */);
    }

    /// 把 widget 提到**兄弟 Z 序的最顶**。
    ///
    /// ⛔⛔ **为什么必须显式做**（真机实测，2026-09-25）：
    ///   · `SetParent` 确实会把窗口放到兄弟 Z 序最顶 —— 但那是**建窗顺序的副产品**，
    ///     不是可以依赖的契约：**任何后继的 `SetParent`**（另一个任务栏 widget 自愈、
    ///     系统自己的 XAML 岛）都会插到我们之上。实测：用一个同款形态
    ///     （popup → 改样式 → `SetParent`）的兄弟窗，它落第 0 位、**我们被挤到第 1 位**。
    ///   · 另一条建窗路线（`CreateWindowExW` 直接以任务栏为父）落**最底** ——
    ///     参考实现 StockBar 正是因此才要「约 2 秒维护重贴 Z 序」。
    ///   · Explorer 重建后我们会重新挂载，此时**谁先谁后取决于系统时序**，更不该赌。
    /// ⇒ 可见性不能依赖「挂载顺序」，必须**主动重申**（见 `WM_APP_RAISE` 维护路径）。
    ///
    /// ⚠️ 子窗必须用 `HWND_TOP`（= 兄弟 Z 序最顶）；`HWND_TOPMOST` 对**子窗**无意义
    ///   （那是顶层窗的概念，在子窗上会被忽略或产生意外结果）。
    pub unsafe fn raise_to_top(hwnd: HWND) {
        SetWindowPos(
            hwnd,
            HWND_TOP,
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
        );
    }

    /// widget 是否**已经在兄弟 Z 序最顶**。
    ///
    /// ⭐ 用它把维护动作做成**幂等**：已经在上面的情况下一次 `SetWindowPos` 都不发
    ///   ⇒ 维护成本降到一次 `GetWindow` 查询，也避免无谓的 Z 序写入引发重新合成。
    ///
    /// ⚠️ `GW_HWNDPREV` = 「Z 序上位于我**之上**的那个兄弟窗」；返回 `NULL` 即无人压顶。
    ///   ⛔ 别与 `GW_HWNDNEXT` 混：后者是**之下**（`EnumWindows` 的遍历方向）。
    pub unsafe fn is_top_sibling(hwnd: HWND) -> bool {
        GetWindow(hwnd, GW_HWNDPREV).is_null()
    }

    /// 跨线程请求「重申 Z 序」（由主线程的 `wnd_proc` 执行）。
    ///
    /// ⛔ 维护线程**不能**直接调 `SetWindowPos`：窗口属于**创建它的线程**（主线程），
    ///   跨线程操作 Z 序与跨线程 `ULW` 同属未定义行为。用 `PostMessageW` 把动作搬回主线程。
    /// ⚠️ 投递失败（窗口已销毁）静默忽略 —— 属「无状态后果」类，下一次维护会补上。
    pub unsafe fn post_raise(hwnd: HWND) {
        PostMessageW(hwnd, WM_APP_RAISE, 0, 0);
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
    /// 建字体。`bold` ⇒ `FW_BOLD`（700），否则 `FW_NORMAL`（400）。
    ///
    /// ⭐ 用户 2026-09-25 要求「电量、音量信息文字加粗」—— 11px 的细体在浅色任务栏上
    ///   笔画偏虚，加粗后两行数字/`%` 的可读性明显更好。
    /// ⚠️ 加粗会让文本**变宽**：`measure_text` 与绘制共用同一个 HFONT，
    ///   故排版宽度自动跟着变（这正是「测量与绘制必须同字体」那条纪律的收益）。
    pub unsafe fn create_font(px_height: i32, bold: bool) -> HFONT {
        let face: Vec<u16> = "Segoe UI\0".encode_utf16().collect();
        CreateFontW(
            -px_height, // 负 = 字符高度（而非单元格高度）
            0,
            0,
            0,
            if bold { 700 } else { 400 }, // FW_BOLD / FW_NORMAL
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

    /// 几乎为空的 WndProc：分层窗口不靠 `WM_PAINT` 绘制（那是 `ULW` 的活），
    /// 但也**不能**把 `WM_ERASEBKGND` 返回 0（会破坏「可见四条件」），故默认一律交 `DefWindowProcW`。
    ///
    /// ⭐ 例外一：**应用私有的刷新消息**（`WM_APP_REFRESH`）必须自己处理 ——
    ///   它是「后台取数完成，请主线程重绘」的唯一通道（跨线程安全）。
    ///
    /// ⭐ 例外一之二：**重申 Z 序**（`WM_APP_RAISE`）—— 维护线程每 2s 请求一次，
    ///   本函数**幂等**处理：已经最顶就一次 `SetWindowPos` 都不发。
    ///   ⛔ 为什么不让维护线程直接 `SetWindowPos`：窗口属于创建它的线程（这里）。
    ///
    /// ⭐ 例外二：**手动拖拽**。三个鼠标消息 + 一个捕获变更消息都落在**主线程**
    ///   （本窗口由主线程创建、消息只投递到创建线程）⇒ 拖拽是天然的**单线程状态机**，
    ///   状态用原子量表达（见 `DRAG_*`）。
    ///
    /// ⛔⛔ **拖拽期间不得在此 emit 任何事件**：`emit` 在**调用它的线程**上同步跑回调
    ///   （`tauri/src/event/listener.rs`），而这里就是主线程 ⇒ 回调里任何
    ///   `run_on_main_thread(..)`（`apply_from_config` 就有）都会**主线程等主线程**，
    ///   永久死锁（退出码 `0xCFFFFFFF`）。落位只走 `with_config_mut`（它自己会落盘）。
    unsafe extern "system" fn wnd_proc(hwnd: HWND, msg: u32, wp: usize, lp: isize) -> isize {
        if msg == WM_APP_REFRESH {
            // 在**主线程**重绘：读快照 → 建 DIB → 提交。
            super::repaint_from_snapshot(hwnd);
            return 0;
        }
        if msg == WM_APP_RAISE {
            // ⭐ 幂等：已经是最顶兄弟 ⇒ 什么都不做（维护节拍是 2s，绝不能让每次
            //    维护都写一次 Z 序 —— 那会引发无谓的重新合成）。
            if !super::ffi::is_top_sibling(hwnd) {
                super::ffi::raise_to_top(hwnd);
                super::append_log("[widget] Z 序被后来者压住 ⇒ 已重申到兄弟最顶");
            }
            return 0;
        }
        // ── 手动拖拽 ────────────────────────────────────────────────────
        // ⚠️ 只在「固定位置」关掉时接管（`drag_begin` 内部判 `drag_enabled()`）；
        //   固定位置时**不拦截**，交回 `DefWindowProcW` 保持原行为。
        if msg == WM_LBUTTONDOWN {
            if super::drag_begin(hwnd) {
                return 0;
            }
        } else if msg == WM_MOUSEMOVE {
            // 非拖拽期间到达的移动消息由 `drag_move` 自行忽略（判 `DRAG_ACTIVE`）。
            super::drag_move(hwnd);
        } else if msg == WM_LBUTTONUP {
            super::drag_finish(hwnd);
        } else if msg == WM_CAPTURECHANGED {
            // 捕获被别处抢走（任务栏抢焦点、其它窗口 `SetCapture`…）：
            // 窗口停在哪就记哪 —— 总比丢掉位置强。非拖拽期间到达则直接返回。
            super::drag_finish(hwnd);
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
    // ⭐ 无论成败都登记「这次是冲着哪个任务栏去的」——维护循环据此区分
    //   「Explorer 重建」（句柄变了 ⇒ 立刻重建）与「同一个任务栏挂不上」
    //   （句柄没变 ⇒ 退回 30s 慢节拍）。见 `MOUNTED_TASKBAR` 的文档。
    MOUNTED_TASKBAR.store(taskbar as isize, Ordering::SeqCst);
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

    // 8) 显式重申 Z 序：`SetParent` 的「落顶」是建窗顺序的副产品，不是契约
    //    （任何后继 `SetParent` 都会插到我们之上 —— 真机实测）。
    //    ⚠️ 与第 7 步分开：`show` 刻意带 `SWP_NOZORDER`，只管「可见四条件」。
    unsafe { ffi::raise_to_top(hwnd) };

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

/// **单段**文本在 widget 上占用的最大宽度。
///
/// ⭐ 为什么不无限平铺：`PINNED_TASKBAR_LIMIT = 8` ⇒ 8 台 × 每台十几字符会吃掉
///    上千像素，在窄屏/多窗口时与任务栏图标区冲突。给每段一个上限、超出截断，
///    保证「有多少台都画得下」，也让布局可预测（用户选定「全部平铺」+ 上限保护）。
///
/// ⚠️ 判据粒度是**单段**（电量段、音量段各算一次），不是单台设备 ——
///   一台设备的宽度取两段的最大值（见 `draw_items` 的 `per_item`）。
/// ⚠️ 标称值见文件头的 `ITEM_MAX_W_DIP`（DIP）—— 本段只讲「为什么有上限」。

/// hover 底衬的不透明度（0–255）—— **浅色系统主题**。
///
/// ⭐ 取值出处：FluentFlyout `Controls/TaskbarWidgetControl.xaml.cs` 的 `Grid_MouseEnter`
///   浅色分支用 `Color.FromArgb(255,255,255,255)` + `Opacity = 0.6`
///   ⇒ 有效 alpha = 255 × 0.6 = **153**。
#[cfg(target_os = "windows")]
const HOVER_BACKDROP_ALPHA_LIGHT: u32 = 153;

/// hover 底衬的不透明度（0–255）—— **深色系统主题**。
///
/// ⭐ 同出处（深色分支）：`Color.FromArgb(197,255,255,255)` + `Opacity = 0.075`
///   ⇒ 有效 alpha = 197 × 0.075 = 14.775 ⇒ 取 **15**。
/// ⚠️ 两种主题下底衬**都是白色**，只有不透明度不同（FluentFlyout 即如此）。
#[cfg(target_os = "windows")]
const HOVER_BACKDROP_ALPHA_DARK: u32 = 15;

/// hover 检测的轮询间隔（毫秒）。
///
/// ⛔ 为什么必须**轮询**而不是等 `WM_MOUSEMOVE`：见 `HOVERED` 的文档 ——
///   没有底衬时窗口全透明，鼠标消息**根本不会投递到本窗口**，靠消息驱动底衬是鸡生蛋。
/// ⭐ 50ms（20Hz）：人眼感知延迟阈值约 100ms，50ms 足够跟手；
///   单次开销只有 `GetCursorPos` + `GetWindowRect`（微秒级），且**仅在状态翻转时**才触发重绘。
#[cfg(target_os = "windows")]
const HOVER_POLL_MS: u64 = 50;

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

    /// 缩放一张 RGBA 到 `side × side`。
    ///
    /// ⭐ **放大用双线性、缩小用最近邻** —— 两者要解决的问题相反：
    ///   · **缩小**（如 32 → 16，任务栏尺寸图标）：线稿的细笔画只有 1–2px，
    ///     双线性会把它们**糊成灰带** ⇒ 必须最近邻，保住锐利。
    ///   · **放大**（如 32 → 40，125% 缩放下的 DPI 换算）：最近邻会让 1px 笔画
    ///     变成「忽 1px 忽 2px」，曲线出现**台阶**（真机 8 倍放大图实测确认）
    ///     ⇒ 必须双线性，让边缘平滑过渡。
    ///   ⛔ 早期版本一律最近邻 —— 那时只有「缩到 16」一种用途，看不出问题；
    ///     引入 DPI 换算（见 `Metrics`）后 125% 会走到放大分支，才暴露出来。
    ///
    /// ⛔ 双线性**必须先预乘再插值**：图标 PNG 是 straight alpha，透明像素的 RGB
    ///   常为 0（或 255），直接插值会把那个颜色混进半透明边缘 ⇒ 图标外圈出现
    ///   **黑晕/白边**。插完再反预乘回 straight（`A == 0` 时 RGB 无意义，置 0）。
    ///
    /// ⭐ 返回预乘前的 straight RGBA —— 合成到 DIB 时由调用方按需预乘。
    pub fn scale_to(src: &Rgba, side: u32) -> Option<Rgba> {
        let (px, sw, sh) = src;
        let (sw, sh) = (*sw, *sh);
        if sw == 0 || sh == 0 || side == 0 {
            return None;
        }
        if sw == side && sh == side {
            return Some((px.clone(), side, side)); // 恒等：零重采样
        }
        let mut out = vec![0u8; (side * side * 4) as usize];
        let upscale = side > sw || side > sh;
        for y in 0..side {
            for x in 0..side {
                let di = ((y * side + x) * 4) as usize;
                if !upscale {
                    // 最近邻：源坐标 = 目标坐标 × 源边长 / 目标边长
                    let sy = (y as u64 * sh as u64 / side as u64) as u32;
                    let sx = (x as u64 * sw as u64 / side as u64) as u32;
                    let si = ((sy * sw + sx) * 4) as usize;
                    out[di..di + 4].copy_from_slice(&px[si..si + 4]);
                    continue;
                }
                // 双线性：目标像素**中心**映射回源坐标，取四邻域加权。
                // `max(0.0)` 把左/上边缘的外推夹回 0（否则 `floor` 会得到 -1）。
                let fx = ((x as f32 + 0.5) * sw as f32 / side as f32 - 0.5).max(0.0);
                let fy = ((y as f32 + 0.5) * sh as f32 / side as f32 - 0.5).max(0.0);
                let (x0f, y0f) = (fx.floor(), fy.floor());
                let (x0, y0) = (x0f as u32, y0f as u32);
                let (tx, ty) = (fx - x0f, fy - y0f);
                let x1 = (x0 + 1).min(sw - 1);
                let y1 = (y0 + 1).min(sh - 1);
                let mut acc = [0.0f32; 4];
                for (xx, wx) in [(x0, 1.0 - tx), (x1, tx)] {
                    for (yy, wy) in [(y0, 1.0 - ty), (y1, ty)] {
                        let si = ((yy * sw + xx) * 4) as usize;
                        let a = px[si + 3] as f32;
                        let w = wx * wy;
                        // 预乘：R·A/255（A 通道本身不预乘）
                        acc[0] += px[si] as f32 * a / 255.0 * w;
                        acc[1] += px[si + 1] as f32 * a / 255.0 * w;
                        acc[2] += px[si + 2] as f32 * a / 255.0 * w;
                        acc[3] += a * w;
                    }
                }
                let a = acc[3].clamp(0.0, 255.0);
                out[di + 3] = a.round() as u8;
                for (c, v) in acc[..3].iter().enumerate() {
                    out[di + c] = if a <= 0.0 {
                        0 // 全透明 ⇒ RGB 无意义，置 0（避免留下假色）
                    } else {
                        (v * 255.0 / a).round().clamp(0.0, 255.0) as u8
                    };
                }
            }
        }
        Some((out, side, side))
    }
}

/// 当前 `Shell_TrayWnd` 的句柄（**0 = 此刻没有任务栏**，如 Explorer 重启间隙）。
///
/// ⭐ 抽成单一入口的理由与 `taskbar_rect` 相同：`FindWindowW` 在本模块被
///   几何计算、挂载、维护循环多处使用，各写一遍必然漂移（类名写错就会静默失效）。
///
/// ⚠️ 返回值是 `isize` 而非 `HWND`：它要跨线程比对（维护线程 vs 主线程），
///   裸指针不是 `Send`，而句柄本身只是整数 —— 与 `WIDGET_HWND` 同款处理。
#[cfg(target_os = "windows")]
fn taskbar_hwnd() -> isize {
    unsafe {
        windows_sys::Win32::UI::WindowsAndMessaging::FindWindowW(
            to_wide("Shell_TrayWnd").as_ptr(),
            std::ptr::null(),
        ) as isize
    }
}

/// 任务栏窗口矩形 `(left, top, width, height)`（**屏幕**坐标，物理像素）。
///
/// ⭐ 抽成单一入口：`taskbar_left`（相对坐标换算）、`widget_y_offset`（垂直居中）、
///   拖拽的**钳制边界**（左右不能拖出任务栏）三处都要它。
///   ⛔ 三处各写一遍 `FindWindowW + GetWindowRect` 必然在后续改动中漂移。
///
/// ⚠️ 返回 `None` 表示「此刻找不到任务栏」或尺寸非正 —— 调用方必须各自决定降级方式
///   （取 0 / 不居中 / 拒绝拖动），不要在这里编造一个假矩形。
#[cfg(target_os = "windows")]
fn taskbar_rect() -> Option<(i32, i32, i32, i32)> {
    let taskbar = taskbar_hwnd() as *mut core::ffi::c_void;
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
    let w = rc.right - rc.left;
    let h = rc.bottom - rc.top;
    if w <= 0 || h <= 0 {
        return None;
    }
    Some((rc.left, rc.top, w, h))
}

/// widget 在任务栏内的**垂直偏移**（父窗客户区坐标，物理像素）。
///
/// ⭐ 为什么要算而不是写死 0：真机实测任务栏高 **60px**、widget 高 **50px**（40 DIP @125%）
///   ⇒ 贴顶会让 widget 里的图标比任务栏自身的图标**高出约 5px**，一眼能看出没对齐。
///   居中后两者同一水平线。
///
/// ⚠️ 任务栏比 widget 还矮时（异常 DPI/多显示器缩放不一致）退回 0，
///   不产生负偏移（负值会把内容顶到任务栏之外）。
#[cfg(target_os = "windows")]
fn widget_y_offset() -> i32 {
    let h = Metrics::current().h;
    match taskbar_rect() {
        Some((_, _, _, th)) => ((th - h) / 2).max(0),
        None => 0,
    }
}

/// 把窗口左端**钳制**在任务栏可见范围内（父窗客户区坐标）。
///
/// ⭐ 抽成纯函数是为了能单测：钳制写错**不会报错**，只会让窗口在拖到边缘时
///   部分跑出任务栏、甚至整块消失（内容还在、位置在屏幕外）—— 用户只能靠重启恢复。
///
/// ⚠️ 上界是 `taskbar_w - widget_w`（而不是 `taskbar_w`）：右端对齐时窗口右缘
///   恰好贴着任务栏右缘。窗口比任务栏还宽时上界归零 ⇒ 钉在左端（防御分支）。
#[cfg(target_os = "windows")]
fn clamp_rel_x(x: i32, widget_w: i32, taskbar_w: i32) -> i32 {
    let max = (taskbar_w - widget_w).max(0);
    x.clamp(0, max)
}

/// 决定这一帧窗口画在哪个 x（父窗客户区坐标）。**纯函数**，可单测。
///
/// 优先级（用户意图 > 历史 > 自动）：
///   1. `locked == true` ⇒ **永远**用贴靠结果（用户在设置页选了左/中/右）；
///   2. `locked == false` 且用户拖过（`custom_x = Some`）⇒ 用**用户放下的位置**
///      —— 这是显式意图，必须压过其它一切；
///   3. `locked == false` 且没拖过、但画过（`last != 0`）⇒ 沿用上次（「不固定」的旧语义）；
///   4. 都没用过 ⇒ 退回贴靠结果（首帧）。
///
/// ⛔ 为什么要抽出来单测：这四条判据的失效方式**全是静默的** —— 优先级写反只会让
///   窗口「跑到别的地方」，界面上不报错、不崩溃，只能靠肉眼发现。
#[cfg(target_os = "windows")]
fn resolve_rel_x(
    locked: bool,
    aligned: i32,
    custom_x: Option<i32>,
    last: i32,
    widget_w: i32,
    taskbar_w: i32,
) -> i32 {
    let wanted = if locked {
        aligned
    } else {
        match custom_x {
            Some(x) => x,
            // `last == 0` 是「还没画过」的哨兵（见 `LAST_X` 文档）
            None if last != 0 => last,
            None => aligned,
        }
    };
    clamp_rel_x(wanted, widget_w, taskbar_w)
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

/// 单段文本宽度的**保守**估算（物理像素）。
///
/// ⚠️ 为什么按码位分类、而不是统一乘一个系数：ASCII 数字/`%`/`N/A` 在 11 DIP Segoe UI
///   下约 6–7px，而中文（全角）约 11px。若统一按 8px 估，**中文会被低估**
///   ⇒ `find_widget_slot` 可能返回一个比实际内容更窄的槽 ⇒ 内容溢出到邻居上。
///   避让扫描的**方向性要求**：宁可高估（少用一点空间），不可低估（重叠）。
///
/// ⭐ 8/14 是 **DIP** 基准值 ⇒ 经 `m.dip()` 按 DPI 缩放（字号也跟着缩放，比例不变）。
/// ⭐ 可证伪：把非 ASCII 的 14 改回 8，`cjk_is_not_underestimated` 会立刻转红。
#[cfg(target_os = "windows")]
fn estimate_text_px(s: &str, m: &Metrics) -> i32 {
    s.chars()
        .map(|c| m.dip(if (c as u32) < 0x80 { 8 } else { 14 }))
        .sum::<i32>()
        .min(m.item_max_w)
}

#[cfg(target_os = "windows")]
fn estimate_widget_width(items: &[WidgetItem], m: &Metrics) -> i32 {
    // ⚠️ 每台设备占「两段文本里更宽的那一段」—— 与 `draw_items` 的 `per_item` 同口径。
    let text_px: i32 = items
        .iter()
        .map(|it| {
            let bat = estimate_text_px(&format_battery(it), m);
            let vol = estimate_text_px(&format_volume(it), m);
            bat.max(vol)
        })
        .sum();
    let gaps = m.item_gap * (items.len().saturating_sub(1) as i32);
    m.pad_x * 2 + text_px + items.len() as i32 * (m.icon + m.icon_text_gap) + gaps
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

/// 点 `(x, y)` 是否落在宽 `w` 高 `h`、圆角半径 `r` 的**圆角矩形**内。**纯函数**。
///
/// 判法：四个角各挖掉一个半径 `r` 的圆（圆心在内缩 `r` 处），其余区域直接命中。
/// ⚠️ 只在「x 落在左/右角带 **且** y 落在上/下角带」时才做圆判定；任一带不在角上
///   就直接命中 —— 否则中间的直边会被误判成角。
#[cfg(target_os = "windows")]
fn inside_rounded_rect(x: i32, y: i32, w: i32, h: i32, r: i32) -> bool {
    if x < 0 || y < 0 || x >= w || y >= h {
        return false;
    }
    let r = r.min(w / 2).min(h / 2).max(0);
    if r == 0 {
        return true;
    }
    let cx = if x < r {
        r
    } else if x >= w - r {
        w - 1 - r
    } else {
        return true; // 不贴左右边 ⇒ 纵向整条都在矩形内
    };
    let cy = if y < r {
        r
    } else if y >= h - r {
        h - 1 - r
    } else {
        return true; // 不贴上下边 ⇒ 横向整条都在矩形内
    };
    let dx = x - cx;
    let dy = y - cy;
    dx * dx + dy * dy <= r * r
}

/// 给整块 widget 铺一层 **hover 底衬**（白色半透明，**鼠标悬停时才有**）。
///
/// ⭐ 口径来源 = FluentFlyout（用户 2026-09-25 指定「和 FluentFlyout 一致」）：
///   白色 + 圆角 6 + **占满控件高度**（其 `MainBorder` 无 `Margin`、控件 `Height="40"`）；
///   ⚠️ 6 与 40 都是 **DIP** ⇒ 由 `Metrics` 按 DPI 换算成物理像素（见 `Metrics` 的文档）。
///   不透明度按**系统主题**取 153（浅色）/ 15（深色）—— 见 `HOVER_BACKDROP_ALPHA_*`。
///   ⚠️ 与 FluentFlyout 的唯一差别：它用 WPF 的 200ms 淡入淡出动画，我们是逐像素合成、
///     **没有过渡动画**（进出/切主题都是瞬时切换）。
///
/// ⛔ **必须预乘**：`UpdateLayeredWindow(ULW_ALPHA)` 要求 32bpp 位图是预乘的
///   （`0xAARRGGBB` 的 RGB = 原色 × alpha / 255）。白色预乘后 RGB 恰好等于 alpha。
///
/// ⛔⛔ **它同时承担「拖拽命中区」的职责**（2026-09-24 真机实测，不是推测）：
///   `ULW` 分层窗的命中测试**按 alpha 走** —— alpha = 0 的像素把鼠标消息
///   **放行给下层窗口**。沿 widget 垂直中线逐 2px 采样 **68 点**，只有 **11 点**
///   命中 widget，且全部落在**字形笔画**上（图标轮廓 x≈10-12/30-32、数字竖笔 78-100）；
///   内边距、项间隙、字形与图标的中空内部统统穿透给 `Shell_TrayWnd`。
///   ⇒ 「hover 才铺底衬」与「拖拽可用」是**自洽**的：hover 由轮询判定（见 `HOVERED`），
///     底衬铺上后命中测试才开始生效，此时按下左键即可拖。
/// ⚠️ 底衬必须**先**画：后续内容按 `blend_over`（source-over）叠在它**之上** ⇒
///   内容只会盖住它，不会被它抹掉。
#[cfg(target_os = "windows")]
fn fill_hover_backdrop(px: &mut [u32], w: i32, h: i32, alpha: u32, radius: i32) {
    let a = alpha.min(255);
    // 白色预乘：RGB 分量 == alpha（255 × a / 255）
    let packed = (a << 24) | (a << 16) | (a << 8) | a;
    for y in 0..h {
        for x in 0..w {
            if !inside_rounded_rect(x, y, w, h, radius) {
                continue;
            }
            let di = (y * w + x) as usize;
            if di < px.len() {
                px[di] = packed;
            }
        }
    }
}

/// 按**系统主题**选 hover 底衬的不透明度（纯函数，可单测）。
///
/// ⭐ 用**系统**主题（`SystemUsesLightTheme`）而不是 widget 内容色用的**应用**主题
///   （`AppsUseLightTheme`）—— 与 FluentFlyout 完全对齐：它的 `Grid_MouseEnter` 读的正是
///   `GetWindowsTheme(out appTheme, out systemTheme)` 里的 `systemTheme`。
///   两者在「应用深色 + 系统浅色」这类自定义主题下**会不一致**。
#[cfg(target_os = "windows")]
fn hover_backdrop_alpha_for(system_uses_light: bool) -> u32 {
    if system_uses_light {
        HOVER_BACKDROP_ALPHA_LIGHT
    } else {
        HOVER_BACKDROP_ALPHA_DARK
    }
}

/// hover 判定的**纯函数**（可单测）：光标是否落在窗口矩形内，或正在拖拽。
///
/// ⚠️ 矩形口径 = `GetWindowRect` 的**屏幕**坐标，其 `right` / `bottom` 是**排他**边界
///   （宽度 = `right - left`）⇒ 判据用 `x < left + w` 而**不是** `<=`。
///   ⛔ 混用会让最右一列 / 最下一行像素的判定反掉 —— 1px 偏差，肉眼与手测都难查，
///     只有单测能钉住，所以本函数必须保持**无副作用、可直接构造输入**。
///
/// ⭐ 拖拽期间恒为 `true`：拖拽靠 `SetCapture` 维持（光标可以短暂移出窗口），
///   若仍按矩形判会让底衬随光标闪进闪出。
/// ⚠️ 任一输入缺失（取光标失败 / 窗口矩形取不到）⇒ `false`（宁可不显示，也不误显示）。
#[cfg(target_os = "windows")]
fn want_hover(
    cursor: Option<(i32, i32)>,
    rect: Option<(i32, i32, i32, i32)>,
    dragging: bool,
) -> bool {
    if dragging {
        return true;
    }
    let (Some((cx, cy)), Some((left, top, w, h))) = (cursor, rect) else {
        return false;
    };
    cx >= left && cx < left + w && cy >= top && cy < top + h
}

/// 把**预乘**的源像素按 **source-over** 合成到目标像素上（两者都是预乘 `0xAARRGGBB`）。
///
/// ⛔⛔ **为什么不能用「取最大 alpha」**（本函数引入前的写法）：底衬会先把整块区域写成
///   `alpha = 153`，于是**抗锯齿边缘**（覆盖度 < 153）被 `a > dst_a` 判假而**丢弃** ⇒
///   字形笔画被侵蚀、文字看起来**变细**（用户 2026-09-25 报的「hover 时内容会变细」）。
///   alpha=28 的旧底衬下，边缘覆盖度几乎都 > 28，所以那时看不出来；换成 153 后立刻显形。
/// ✅ 正确做法 = 标准 source-over：`out = src + dst × (1 − As/255)`。
///   ⭐ **无底衬时 `dst_a == 0` ⇒ 退化为 `out == src`**，非 hover 路径**逐位零变化**
///     （已由 `blend_over_with_no_backdrop_is_identity` 钉住）。
/// ⚠️ 预乘保证 `pr ≤ a`、`dst_rgb ≤ dst_a` ⇒ `out_rgb ≤ out_a ≤ 255`，**不会溢出**。
#[cfg(target_os = "windows")]
fn blend_over(dst: u32, a: u32, pr: u32, pg: u32, pb: u32) -> u32 {
    let inv = 255 - a;
    let da = dst >> 24;
    let ao = a + da * inv / 255;
    let ro = pr + ((dst >> 16) & 0xFF) * inv / 255;
    let go = pg + ((dst >> 8) & 0xFF) * inv / 255;
    let bo = pb + (dst & 0xFF) * inv / 255;
    (ao << 24) | (ro << 16) | (go << 8) | bo
}

/// 把一段**文本掩码**按覆盖度预乘合成进主缓冲（`(dst_x, dst_y)` = 目标左上角）。
///
/// ⛔ 为什么不能把 GDI 文本直接画进主缓冲：`DrawTextW` 在 32bpp DIB 上写的是
///   `0x00RRGGBB`（**alpha 字节恒为 0**）⇒ 被 `ULW` 当全透明 ⇒ **文字完全不显示**
///   （与「类没设背景刷」的表现一模一样，极易误判成同一个问题）。
///   正确路径 = 白底黑字掩码 → `cov = 255 - gray` → 预乘（详见 `ffi::render_text_mask`）。
///
/// ⭐ 合成走 `blend_over`（source-over），**不是**「取最大 alpha」——
///   后者会在底衬（alpha=153）之上把抗锯齿边缘吃掉，让文字变细。详见 `blend_over`。
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
            let di = (dy * total_w + dx) as usize;
            if di < px.len() {
                px[di] = blend_over(px[di], a, pr, pg, pb);
            }
        }
    }
}

// ══════════════════════════════════════════════════════════════════════════
// 拖拽（全部在**主线程**：鼠标消息只投递给创建窗口的那个线程）
// ══════════════════════════════════════════════════════════════════════════
//
// ⭐ 交互口径（用户 2026-09-24）：设置页「固定位置」**关掉**后，任务栏窗口可以用
//   鼠标拖着走；松开即记住位置，重启后仍在原处。
//
// ⛔ **为什么必须画底衬**：`ULW` 分层窗按 alpha 命中测试，透明像素把鼠标放行给下层
//   ⇒ 只有可见笔画能接住按下（真机实测 68 采样点只中 11 点）。见 `fill_hover_backdrop`。

/// 拖拽是否可用 —— 「固定位置」关掉时才允许。
#[cfg(target_os = "windows")]
fn drag_enabled() -> bool {
    crate::config::with_config(|c| !c.taskbar_position_locked)
}

/// 读窗口当前的**屏幕**矩形 `(left, top, width, height)`（物理像素）。
///
/// ⚠️ `GetWindowRect` 对子窗也返回**屏幕**坐标 —— 拖拽的位移量必须在同一坐标系里算。
#[cfg(target_os = "windows")]
fn window_screen_rect(hwnd: *mut core::ffi::c_void) -> Option<(i32, i32, i32, i32)> {
    let mut rc = windows_sys::Win32::Foundation::RECT {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    };
    if unsafe { windows_sys::Win32::UI::WindowsAndMessaging::GetWindowRect(hwnd, &mut rc) } == 0 {
        return None;
    }
    Some((rc.left, rc.top, rc.right - rc.left, rc.bottom - rc.top))
}

/// 读窗口当前的**父窗客户区**左端 x（物理像素）—— 即 `SetWindowPos` / `ULW` 用的那个坐标系。
///
/// ⭐ 为什么不写成 `GetWindowRect().left - 任务栏屏幕左端`：那样只在
///   「父窗客户区原点 == 父窗窗口左上角」时等价。本机 `Shell_TrayWnd` 实测确实相等
///   （窗口 rect `(0,1380)`、客户区原点也是 `(0,1380)`），但那是**别人的样式**给的巧合，
///   不是我们能依赖的契约。`ScreenToClient` 是显式换算，与父窗样式无关。
///
/// ⚠️ 拖拽**原点**与**落点**都必须走本函数：两处若用不同口径换算，
///   窗口会在第一次 `WM_MOUSEMOVE` 时**跳一段固定偏移**（差值正好是客户区原点偏移）。
#[cfg(target_os = "windows")]
fn window_parent_x(hwnd: *mut core::ffi::c_void) -> Option<i32> {
    let (screen_left, _, _, _) = window_screen_rect(hwnd)?;
    let parent = unsafe { windows_sys::Win32::UI::WindowsAndMessaging::GetParent(hwnd) };
    if parent.is_null() {
        return None;
    }
    let mut pt = windows_sys::Win32::Foundation::POINT {
        x: screen_left,
        y: 0,
    };
    // ⚠️ `ScreenToClient` 在 `Graphics::Gdi` 下（与 `GetWindowRect` / `SetWindowPos` 不同模块）。
    if unsafe { windows_sys::Win32::Graphics::Gdi::ScreenToClient(parent, &mut pt) } == 0 {
        return None;
    }
    Some(pt.x)
}

/// `WM_LBUTTONDOWN`：可拖拽时接管这次按下。返回 `true` 表示已开始拖拽。
#[cfg(target_os = "windows")]
fn drag_begin(hwnd: *mut core::ffi::c_void) -> bool {
    if !drag_enabled() {
        return false;
    }
    let Some(win_parent_x) = window_parent_x(hwnd) else {
        return false;
    };
    let mut pt = windows_sys::Win32::Foundation::POINT { x: 0, y: 0 };
    if unsafe { windows_sys::Win32::UI::WindowsAndMessaging::GetCursorPos(&mut pt) } == 0 {
        return false;
    }
    DRAG_ORIGIN_CURSOR_X.store(pt.x, Ordering::Release);
    DRAG_ORIGIN_WIN_X.store(win_parent_x, Ordering::Release);
    DRAG_ACTIVE.store(true, Ordering::Release);
    // ⭐ 必须 `SetCapture`：光标拖出 widget（甚至拖出任务栏）后仍要收到 `WM_MOUSEMOVE`，
    //    否则拖拽会「粘住」—— 窗口停在光标离开的那一点不再跟随。
    unsafe { windows_sys::Win32::UI::Input::KeyboardAndMouse::SetCapture(hwnd) };
    append_log(&format!(
        "[widget] 拖拽开始: cursor_x={} parent_x={win_parent_x}",
        pt.x
    ));
    true
}

/// `WM_MOUSEMOVE`：拖拽中 → 按光标位移移动窗口。
#[cfg(target_os = "windows")]
fn drag_move(hwnd: *mut core::ffi::c_void) {
    if !DRAG_ACTIVE.load(Ordering::Acquire) {
        return;
    }
    let mut pt = windows_sys::Win32::Foundation::POINT { x: 0, y: 0 };
    if unsafe { windows_sys::Win32::UI::WindowsAndMessaging::GetCursorPos(&mut pt) } == 0 {
        return;
    }
    let Some((_, _, win_w, _)) = window_screen_rect(hwnd) else {
        return;
    };
    let Some((_, _, tb_w, _)) = taskbar_rect() else {
        return;
    };
    let dx = pt.x - DRAG_ORIGIN_CURSOR_X.load(Ordering::Acquire);
    // ⚠️ 原点与目标**同一坐标系**（父窗客户区）⇒ 直接相加，不做任何「屏幕→客户区」换算。
    //    混用两个坐标系会让窗口在首次移动时跳一段固定偏移（见 `window_parent_x`）。
    let wanted = DRAG_ORIGIN_WIN_X.load(Ordering::Acquire) + dx;
    let rel_x = clamp_rel_x(wanted, win_w, tb_w);
    // ⚠️ 子窗的 `SetWindowPos` 坐标同样是**父窗客户区坐标**（与 `commit` 一致）。
    // ⚠️ `SWP_NOSIZE`：宽度由内容决定，拖拽不改变内容 ⇒ 尺寸不该动。
    //     （`ULW` 提交的位图会随窗口一起移动，无需重绘 —— 真机实测确认。）
    unsafe {
        windows_sys::Win32::UI::WindowsAndMessaging::SetWindowPos(
            hwnd,
            std::ptr::null_mut(),
            rel_x,
            widget_y_offset(),
            0,
            0,
            windows_sys::Win32::UI::WindowsAndMessaging::SWP_NOSIZE
                | windows_sys::Win32::UI::WindowsAndMessaging::SWP_NOZORDER
                | windows_sys::Win32::UI::WindowsAndMessaging::SWP_NOACTIVATE,
        );
    }
    LAST_X.store(rel_x, Ordering::Release);
}

/// 结束拖拽：复位状态 → 释放捕获 → 把落点写进配置。
///
/// ⚠️ 这是**拖拽唯一的落盘点**；`WM_LBUTTONUP` 与 `WM_CAPTURECHANGED` 都走它
///   （捕获被别处抢走时，窗口停在哪就记哪 —— 总比丢掉位置强）。
#[cfg(target_os = "windows")]
fn drag_finish(hwnd: *mut core::ffi::c_void) {
    // ⛔ 先 `swap(false)` 再 `ReleaseCapture()`：`ReleaseCapture` 自身会投递一条
    //    `WM_CAPTURECHANGED`，若那时标志仍为 true 会再进本函数（重复落盘 + 重复日志）。
    if !DRAG_ACTIVE.swap(false, Ordering::AcqRel) {
        return;
    }
    unsafe { windows_sys::Win32::UI::Input::KeyboardAndMouse::ReleaseCapture() };
    let Some((_, _, win_w, _)) = window_screen_rect(hwnd) else {
        return;
    };
    let Some(win_parent_x) = window_parent_x(hwnd) else {
        return;
    };
    let Some((_, _, tb_w, _)) = taskbar_rect() else {
        return;
    };
    let rel_x = clamp_rel_x(win_parent_x, win_w, tb_w);
    LAST_X.store(rel_x, Ordering::Release);
    // ⛔⛔ **绝不能在这里 `emit("config-changed")`**：`emit` 在**调用它的那个线程**上
    //    同步逐个执行回调（`tauri/src/event/listener.rs`），而本函数正跑在**主线程**的
    //    窗口过程里 ⇒ 监听里的 `apply_from_config` 会调用 `app.run_on_main_thread(..)`
    //    （内部同步等待主线程）⇒ **主线程等主线程，永久死锁**。
    //    与 AGENTS.md 的 AB/BA 纪律同源。`with_config_mut` 自己会落盘，不需要任何事件。
    crate::config::with_config_mut(|c| c.taskbar_custom_x = Some(rel_x));
    append_log(&format!(
        "[widget] 拖拽落位: rel_x={rel_x}（已写入 taskbar_custom_x）"
    ));
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
    // ⭐ 本帧的布局度量（DIP → 物理像素）。**测量与绘制共用同一份** ⇒ 不会漂移。
    let m = Metrics::current();

    // 空快照：不画任何东西，但仍提交一帧（保持窗口有效且全透明）
    if items.is_empty() {
        return draw_blank(hwnd, m.pad_x * 2);
    }

    // ── 先在**测量用 DC** 上量出每段文本宽度 ────────────────────────────
    // ⚠️ 测量与绘制必须用**同一个字体 + 同样的 DrawTextW 标志**，
    //    否则会出现「按测量宽度排版、实际文本更长」⇒ 相邻项重叠（见 measure_text 注释）。
    let (font, memdc) = unsafe {
        let f = ffi::create_font(m.font, true); // 加粗（用户 2026-09-25）
        if f.is_null() {
            append_log("[widget] CreateFontW 失败，退回空白帧");
            return draw_blank(hwnd, m.pad_x * 2);
        }
        let screen = windows_sys::Win32::Graphics::Gdi::GetDC(std::ptr::null_mut());
        let dc = windows_sys::Win32::Graphics::Gdi::CreateCompatibleDC(screen);
        windows_sys::Win32::Graphics::Gdi::ReleaseDC(std::ptr::null_mut(), screen);
        (f, dc)
    };
    if memdc.is_null() {
        unsafe { ffi::destroy_font(font) };
        return draw_blank(hwnd, m.pad_x * 2);
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
    // 每段记 `(文本实际宽, 是否需要省略号)`：实际宽 = `min(自然宽, item_max_w)`；
    // 自然宽 > 上限 ⇒ 该段要画 `…`（否则会**硬切半个字形**）。
    let measure = |t: &[u16]| -> (i32, bool) {
        let natural = unsafe { ffi::measure_text(memdc, font, t) };
        // ⛔ 每段加上限：极端长文本时截断显示（避免挤压其它设备）
        let clamped = natural.min(m.item_max_w);
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
        .map(|i| m.icon + m.icon_text_gap + bat_w[i].0.max(vol_w[i].0))
        .collect();
    let content_w: i32 = per_item.iter().sum::<i32>() + m.item_gap * (items.len() as i32 - 1);
    let total_w = content_w + m.pad_x * 2;
    // ⛔ GetPixel 扫描必须在后台线程完成（WMI 之外也不能阻塞窗口线程）。
    // 后台快照任务已将估算宽度传给 `find_widget_slot`，这里仅读取原子坐标。
    if !SLOT_VALID.load(Ordering::Acquire) {
        unsafe { ffi::hide(hwnd as _) };
        unsafe { ffi::destroy_font(font) };
        return true;
    }
    // ⚠️ 位置配置**在绘制前一次性取快照**（返回 owned 值）⇒ 锁在 GDI 调用之前就已释放，
    //    不会「持配置锁去做窗口操作」（AGENTS.md 的 AB/BA 死锁纪律）。
    let (position, locked, custom_x) = crate::config::with_config(|c| {
        (
            c.taskbar_position.clone(),
            c.taskbar_position_locked,
            c.taskbar_custom_x,
        )
    });
    // ⚠️ `taskbar_rect()` 取一次同时拿到左端与宽度（宽度供 `resolve_rel_x` 钳制用）。
    //    取不到时给一个「钳制不起作用」的宽度，退回旧行为而不是把窗口钉死在 0。
    let (tb_left, tb_w) = match taskbar_rect() {
        Some((left, _, w, _)) => (left, w),
        None => (0, i32::MAX),
    };
    let slot_rel_x = SLOT_X.load(Ordering::Acquire) - tb_left;
    let slot_w = SLOT_W.load(Ordering::Acquire);
    let aligned = align_in_slot(slot_rel_x, slot_w, total_w, &position);
    // 四档优先级（固定 > 拖拽落点 > 上次 > 贴靠）与钳制都在纯函数里，可单测。
    let rel_x = resolve_rel_x(
        locked,
        aligned,
        custom_x,
        LAST_X.load(Ordering::Acquire),
        total_w,
        tb_w,
    );
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
    let h = m.h;
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
        // ⭐ hover 底衬（白色半透明，**鼠标悬停时才有**）—— 口径与 FluentFlyout 一致。
        //    ⛔ 判据是 `HOVERED`（由轮询光标得出），**不再看 `locked`**：
        //       用户 2026-09-25 要求「和 FluentFlyout 一致 + hover 时才出现」，
        //       而 FluentFlyout 没有「固定位置」概念 ⇒ hover 即显示。
        //    ⚠️ 它同时是**拖拽命中区**（分层窗按 alpha 命中测试）：hover 成立 ⇒ 底衬铺上
        //       ⇒ 命中测试开始生效 ⇒ 此刻按下左键能收到 `WM_LBUTTONDOWN`。自洽性论证见
        //       `fill_hover_backdrop` 与 `HOVERED` 的文档。
        //    ⚠️ 底衬必须**先**画（后续内容按 source-over 叠在它之上 ⇒ 只会盖住它、不会抹掉它）。
        if HOVERED.load(Ordering::Acquire) {
            let alpha = hover_backdrop_alpha_for(crate::windows::system_uses_light_theme());
            fill_hover_backdrop(px, total_w, h, alpha, m.radius);
        }
        // 图标在 widget 里垂直居中（`m.icon` ≤ `h`，余量上下各一半）
        let icon_y = (h - m.icon) / 2;

        let mut cursor = m.pad_x;
        for (i, it) in items.iter().enumerate() {
            // ⚠️ 用户固定的设备（此刻读不出数据）用**半透明**显示，
            //    与「有数据」区分；这是「pin = 强制显示 + 置灰」在 widget 上的落地。
            let alpha_scale: f32 = if it.pinned && it.battery.is_none() && !it.has_audio {
                0.45
            } else {
                1.0
            };

            // ① 图标：解码（带缓存）→ 最近邻缩放到 `m.icon` → 预乘合成
            if let Some(scaled) =
                icons::get(it.icon, dark).and_then(|rgba| icons::scale_to(rgba, m.icon as u32))
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
                        let di = ((icon_y + yy) * total_w + cursor + xx) as usize;
                        if di < px.len() {
                            px[di] = blend_over(px[di], a, pr, pg, pb);
                        }
                    }
                }
            }
            let text_x = cursor + m.icon + m.icon_text_gap;

            // ② 电量：图标**右上角**（两行文本的**上半行**）
            let (bw, b_ell) = bat_w[i];
            if bw > 0 {
                if let Some(mask) =
                    ffi::render_text_mask(bw, m.text_row_h, font, &bat_texts[i], b_ell)
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
                    ffi::render_text_mask(vw, m.text_row_h, font, &vol_texts[i], v_ell)
                {
                    blit_text_mask(
                        px,
                        total_w,
                        &mask,
                        text_x,
                        icon_y + m.text_row_h,
                        (cr, cg, cb),
                        alpha_scale,
                    );
                }
            }
            cursor += per_item[i] + m.item_gap;
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
/// ⭐ 仍要提交：`ULW` 同时也设定窗口尺寸 ⇒ 空帧会把窗口缩到 `w`×`h`，
///   避免残留在上一次的尺寸上（否则内容清空但窗口还占着位置）。
/// ⚠️ 全透明 ⇒ 位置不可见，x 固定 0；y 仍走垂直居中，避免窗口在空帧与实帧之间跳。
#[cfg(target_os = "windows")]
fn draw_blank(hwnd: *mut core::ffi::c_void, w: i32) -> bool {
    let w = w.max(1);
    let h = Metrics::current().h;
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
    draw_blank(hwnd, Metrics::current().pad_x * 2)
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
    let wanted = estimate_widget_width(&items, &Metrics::current());
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

/// 维护循环的 tick 间隔（秒）。
///
/// ⭐ 取 **2**：与参考实现 StockBar 的「约 2 秒维护重贴 Z 序」同量级。
///   ⛔ 但这个 tick **只做廉价动作**（`IsWindow` + `FindWindowW` + 一次 `GetWindow`），
///   **绝不做 WMI 取数** —— 那由 `REFRESH_EVERY_TICKS` 单独控制（30s 一次）。
#[cfg(target_os = "windows")]
const MAINTENANCE_TICK_SECS: u64 = 2;

/// 每多少个 tick 做一次**取数刷新**（`2s × 15 = 30s`）。
///
/// ⛔ 两者必须分开：维护（重申 Z 序 / 探活）要快，取数（WMI 实测 600ms+）要慢。
///   把它们绑在一个节拍上，要么 Z 序恢复慢 15 倍，要么 WMI 被拉爆。
#[cfg(target_os = "windows")]
const REFRESH_EVERY_TICKS: u64 = 15;

/// 维护循环**单个 tick 该做什么**。纯数据，便于单测。
#[cfg(target_os = "windows")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TickPlan {
    /// 请求主线程重申 Z 序（仅在窗口存活时有意义）。
    raise: bool,
    /// 跑一次取数刷新（重）。
    refresh: bool,
    /// 尝试重新挂载。
    remount: bool,
}

/// 决策函数：**纯函数**，可单测（不读任何全局状态）。
///
/// ⛔ 为什么值得抽出来单测：这里的失效方式**全是静默的** ——
///   · `raise` 写漏 ⇒ Z 序被后来者压住后**永不恢复**（窗口还在、只是看不见）；
///   · `remount` 判据写反 ⇒ 要么 Explorer 重建后空等 30s，要么**每 2s 挂一次**
///     （反复操作任务栏会触发「越试越糟」的环境效应，PLAYBOOK §E3）。
///   两者都不报错、不崩溃，只能靠用例钉住。
#[cfg(target_os = "windows")]
fn plan_tick(alive: bool, taskbar_changed: bool, tick: u64) -> TickPlan {
    let due = tick % REFRESH_EVERY_TICKS == 0;
    if !alive {
        return TickPlan {
            raise: false,
            refresh: false,
            // ⭐ 任务栏换了 = Explorer 重建 ⇒ 立刻重建（不等慢节拍）。
            //    否则只有 30s 节拍才重试 —— 那正是「反复挂载」的源头，必须靠节拍隔开。
            remount: taskbar_changed || due,
        };
    }
    TickPlan {
        raise: true,
        refresh: due,
        remount: false,
    }
}

/// hover 轮询线程：**唯一**维护 `HOVERED` 的地方（幂等，重复调用只启动一次）。
///
/// ⭐ 为什么单独一条线程，而不塞进 `start_refresh_loop`：两者**节奏差 40 倍**
///   （维护 2s vs hover 50ms）。塞一起只有两种结果 —— 要么把维护节拍拖到 50ms
///   （触发挂载风暴，见 `plan_tick`），要么让 hover 迟钝到 2s（底衬跟不上鼠标）。
///   **节奏不同就分开。**
///
/// ⚠️ 本线程只做**只读查询**（`IsWindow` / `GetCursorPos` / `GetWindowRect`）——
///   与维护线程里的 `FindWindowW` 同属只读，跨线程安全。
///   ⛔ 真正「碰窗口」的动作（`SetWindowPos` / `ULW` 提交）一律留在主线程；
///     本线程只 `PostMessageW` 请求重绘。
/// ⛔ **必须无条件启动**（不依赖挂载成功）—— 与「监听与兜底循环必须无条件安装」同理：
///   等挂载成功才装的话，勾选那一刻窗口还不存在，就永远等不到第一条 hover。
#[cfg(target_os = "windows")]
fn start_hover_watcher() {
    use std::sync::OnceLock;
    static STARTED: OnceLock<()> = OnceLock::new();
    if STARTED.set(()).is_err() {
        return; // 已启动过
    }
    std::thread::spawn(move || loop {
        std::thread::sleep(std::time::Duration::from_millis(HOVER_POLL_MS));
        // 未挂载 / 句柄已失效 ⇒ 没有窗口可悬停，复位状态后继续空转。
        if !widget_alive() {
            if HOVERED.swap(false, Ordering::AcqRel) {
                append_log("[widget] hover: 窗口已失效 ⇒ 复位");
            }
            continue;
        }
        let hwnd = WIDGET_HWND.load(Ordering::SeqCst) as *mut core::ffi::c_void;
        if hwnd.is_null() {
            continue;
        }
        let cursor = unsafe {
            let mut pt = windows_sys::Win32::Foundation::POINT { x: 0, y: 0 };
            if windows_sys::Win32::UI::WindowsAndMessaging::GetCursorPos(&mut pt) == 0 {
                None
            } else {
                Some((pt.x, pt.y))
            }
        };
        let rect = window_screen_rect(hwnd);
        let dragging = DRAG_ACTIVE.load(Ordering::Acquire);
        let want = want_hover(cursor, rect, dragging);
        // ⭐ 只在**翻转**时动手：既避免每 50ms 一次无谓重绘，也让日志只在真正进出时出现。
        if want != HOVERED.load(Ordering::Acquire) {
            HOVERED.store(want, Ordering::Release);
            append_log(&format!(
                "[widget] hover: {} ⇒ {}底衬",
                if want { "进入" } else { "离开" },
                if want { "显示" } else { "隐藏" }
            ));
            // ⛔ 只重绘、**不取数**：`WM_APP_REFRESH` ⇒ `repaint_from_snapshot` 读**现有**快照重画。
            //    绝不能改用 `refresh_async()` —— 那会触发 600ms+ 的 WMI 取数，
            //    鼠标每进出一次就重拉一遍设备列表。
            unsafe { ffi::post_refresh(hwnd as _) };
        }
    });
}

/// 启动维护线程（幂等，重复调用只起一个）。
///
/// 它一个循环干三件事，**节奏不同**（见 `plan_tick`）：
///   1. **每 2s**：探活 + 请求重申 Z 序（廉价、幂等）；
///   2. **每 30s**：取数刷新（重：WMI 600ms+）；
///   3. **按需**：重新挂载（任务栏换句柄 ⇒ 立刻；同一任务栏挂不上 ⇒ 30s 节拍）。
///
/// ⭐ 为什么还需要 2（已有事件驱动）：事件只覆盖**代码主动 emit 的时刻**。
///   「没有事件但数据变了」确实存在 —— 蓝牙电量自然衰减、系统后台静默切换默认音频设备。
///
/// ⭐ 为什么还需要 1：Z 序**不是**一次性设置。`SetParent` 把我们放上顶只是建窗顺序的
///   副产品；任何后继 `SetParent`（另一个任务栏 widget 自愈、Explorer 重建后的系统子窗）
///   都会插到我们之上 ⇒ **必须周期性重申**。真机实测：同款形态的兄弟窗 `SetParent`
///   之后我们被挤到第 1 位。
///
/// ⭐ 本循环**无条件启动**（不要求「先挂载成功」）—— 未挂载时它兼「自愈」职责。
///
/// ⛔ 参数必须是 `AppHandle`：自愈要走 `apply_from_config`，而窗口只能在主线程建。
pub fn start_refresh_loop(app: &tauri::AppHandle) {
    #[cfg(target_os = "windows")]
    {
        use std::sync::OnceLock;
        static STARTED: OnceLock<()> = OnceLock::new();
        if STARTED.set(()).is_err() {
            return; // 已启动过
        }
        // ⭐ hover 轮询线程与维护循环**节奏不同**（50ms vs 2s），各起一条。
        //    两个启动函数各自幂等，此处重复调用无副作用。
        start_hover_watcher();
        let app = app.clone();
        std::thread::spawn(move || {
            // 首次延迟 3s：让启动流程（托盘/窗口）先跑完，避免与启动期抢 CPU
            std::thread::sleep(std::time::Duration::from_secs(3));
            let mut tick: u64 = 0;
            loop {
                tick += 1;
                let alive = widget_alive();
                let taskbar = taskbar_hwnd();
                // ⭐ 「任务栏换了」= Explorer 重建。**这是唯一能立刻重建的触发条件**；
                //    同一个任务栏上挂不上只能等 30s 节拍（见 `plan_tick` 与 §E3）。
                let changed = taskbar != MOUNTED_TASKBAR.load(Ordering::SeqCst);
                let plan = plan_tick(alive, changed, tick);

                if plan.raise {
                    let hwnd = WIDGET_HWND.load(Ordering::SeqCst) as *mut core::ffi::c_void;
                    if !hwnd.is_null() {
                        // ⭐ 只投递、不查询：`is_top_sibling` 属于「碰窗口」的操作，
                        //    一律留在主线程做（`wnd_proc` 里幂等处理）。
                        unsafe { ffi::post_raise(hwnd) };
                    }
                }
                if plan.refresh {
                    refresh_async();
                }
                if plan.remount {
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
                    // ⛔ 乐观登记：**先**记下这次是冲着哪个任务栏去的，再投递挂载。
                    //    失败时也保持已登记 ⇒ `changed` 变假 ⇒ 退回 30s 慢节拍，
                    //    不会每 2s 重试一次（那会触发「越试越糟」）。
                    MOUNTED_TASKBAR.store(taskbar, Ordering::SeqCst);
                    if should_show() {
                        append_log(&format!(
                            "[widget] 兜底：未挂载 ⇒ 重新挂载（任务栏={taskbar:#x}，{}）",
                            if changed {
                                "Explorer 重建"
                            } else {
                                "30s 节拍"
                            }
                        ));
                        apply_from_config(&app);
                    }
                }
                std::thread::sleep(std::time::Duration::from_secs(MAINTENANCE_TICK_SECS));
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

    /// 100% 缩放的布局度量（测试固定用 96，避免依赖真机 DPI）。
    fn m96() -> Metrics {
        Metrics::for_dpi(96)
    }

    /// ⭐⭐ DPI 换算的核心断言：**125% 下底衬高必须是 50px**。
    ///
    /// 口径出处 = FluentFlyout `Windows/TaskbarWindow.xaml.cs`：
    ///   `physicalHeight = (int)(logicalHeight * dpiScale)`，`logicalHeight = 40`（DIP）
    /// ⇒ 125% 时 40 × 1.25 = **50**。我们原先把 40 当物理像素用，比它矮 10px。
    ///
    /// 可证伪：把 `Metrics::for_dpi` 里的 `dpi as f32 / 96.0` 换成 `1.0`（即不做换算）
    /// ⇒ `m120.h == 40`，本条转红。
    #[test]
    fn metrics_scale_layout_by_dpi() {
        let m = m96();
        assert_eq!(
            (m.h, m.icon, m.radius, m.font, m.text_row_h),
            (40, 32, 6, 11, 16)
        );

        let m120 = Metrics::for_dpi(120); // 本机任务栏 DPI = 120（125%）
        assert_eq!(
            m120.h, 50,
            "125% 下底衬高必须与 FluentFlyout 一致（40 DIP）"
        );
        assert_eq!(m120.icon, 40);
        assert_eq!(m120.radius, 8, "6 DIP × 1.25 = 7.5 → 8");
        assert_eq!(m120.font, 14, "11 DIP × 1.25 = 13.75 → 14");
        assert_eq!(m120.text_row_h, 20, "行高 = 图标高的一半");

        assert_eq!(Metrics::for_dpi(144).h, 60, "150%");
        // 防御：dpi = 0 不得 panic、也不得产生 0 尺寸（那会让窗口彻底不可见）
        assert_eq!(Metrics::for_dpi(0).h, 40);
        assert_eq!(Metrics::for_dpi(0).dpi, 96);
    }

    /// ⛔ 中文**不得**被低估 —— 否则 `find_widget_slot` 可能返回比实际内容更窄的槽，
    /// 内容溢出压到邻居上（正是避让机制要避免的那件事）。
    ///
    /// 可证伪：把 `estimate_text_px` 里非 ASCII 的 `14` 改回 `8`，本条立刻转红。
    #[test]
    fn cjk_is_not_underestimated() {
        let m = m96();
        // 「静音」是 2 个全角字：11px 字体下实际约 22px ⇒ 估算必须 >= 22
        assert!(
            estimate_text_px("静音", &m) >= 22,
            "中文估算过小会低估槽宽: {}",
            estimate_text_px("静音", &m)
        );
        // 与纯 ASCII 段对比：同样 2 个码位，中文必须更宽
        assert!(
            estimate_text_px("静音", &m) > estimate_text_px("N/A", &m),
            "中文段必须比等长的 ASCII 段估得更宽"
        );
        // ASCII 侧维持原口径（`100%` = 4 × 8 = 32）
        assert_eq!(estimate_text_px("100%", &m), 32);
        // ⭐ DPI 缩放后必须同比放大（字号也跟着缩放 ⇒ 比例不变）
        assert_eq!(estimate_text_px("100%", &Metrics::for_dpi(120)), 40);
    }

    /// 宽度估算必须**单调**，且单台时至少装得下「图标 + 间隙」。
    #[test]
    fn estimate_width_is_monotonic_and_covers_icon() {
        let m = m96();
        let one = vec![item(Some(50), Some(0.5), true, Some(false))];
        let mut two = one.clone();
        two.push(item(Some(60), Some(0.6), true, Some(false)));

        let w1 = estimate_widget_width(&one, &m);
        let w2 = estimate_widget_width(&two, &m);
        assert!(
            w1 >= m.pad_x * 2 + m.icon + m.icon_text_gap,
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
        let m = m96();
        // 电量 `5%`（2 字符 → 16px）比音量 `50%`（3 字符 → 24px）窄
        let it = item(Some(5), Some(0.5), true, Some(false));
        let wider = estimate_text_px("50%", &m);
        let narrower = estimate_text_px("5%", &m);
        assert!(
            wider > narrower,
            "样本本身要能区分宽窄，否则本条无区分力（{wider} vs {narrower}）"
        );
        let w = estimate_widget_width(&[it], &m);
        assert!(
            w >= m.pad_x * 2 + m.icon + m.icon_text_gap + wider,
            "宽度必须覆盖更宽的那段文本：{w} < 内边距 + 图标 + 间隙 + {wider}"
        );
    }

    /// 6 台（显示上限）的估算必须仍能塞进避让后的可用区 ——
    /// 否则「避让扫描永远找不到槽」⇒ widget 直接不显示（且不报错）。
    ///
    /// ⭐ 同时覆盖 **125% 缩放**：布局整体放大 25% ⇒ 高 DPI 用户最容易踩到「找不到槽」。
    #[test]
    fn six_items_still_fit_in_a_plausible_slot() {
        let six: Vec<WidgetItem> = (0..WIDGET_MAX_ITEMS)
            .map(|i| item(Some(50 + i as i32), Some(0.5), true, Some(false)))
            .collect();
        // 真机可用区约 1300px（任务栏 2560px 减两端各 100px 再减任务栏自身内容）
        let w = estimate_widget_width(&six, &m96());
        assert!(w < 1300, "6 台估算过宽，会找不到避让槽: {w}");
        let w120 = estimate_widget_width(&six, &Metrics::for_dpi(120));
        assert!(w120 < 1300, "125% 缩放下 6 台估算过宽: {w120}");
        assert!(w120 > w, "125% 必须比 100% 宽: {w120} vs {w}");
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

    /// ⛔ **缩小**必须仍是最近邻：线稿的 1–2px 笔画被双线性糊掉会变成灰带。
    ///
    /// 可证伪：把 `scale_to` 里的 `upscale` 判据改成恒 `true` ⇒ 本条转红（出现中间值）。
    #[test]
    fn scale_down_stays_nearest_neighbor() {
        // 1×2 源：上纯黑不透明、下纯白不透明。缩到 1×1 只能取其中一个 ——
        // 出现 0/255 之外的**中间值**就是双线性的产物。
        let src = (vec![0, 0, 0, 255, 255, 255, 255, 255], 1u32, 2u32);
        let (px, w, h) = icons::scale_to(&src, 1).expect("缩放不应失败");
        assert_eq!((w, h), (1, 1));
        assert!(
            px[0] == 0 || px[0] == 255,
            "缩小时不得出现中间值: {}",
            px[0]
        );
    }

    /// ⭐ **放大**必须平滑：最近邻会让 1px 笔画忽宽忽窄、曲线出现台阶
    ///   （真机 8 倍放大图确认过）。
    ///
    /// 可证伪：把 `upscale` 判据改成恒 `false` ⇒ 本条转红（相邻像素完全相同）。
    #[test]
    fn scale_up_interpolates() {
        // 2×1 的黑白源放大到 4×1 ⇒ 中间两列必须是**渐变**而不是非黑即白
        let src = (vec![0, 0, 0, 255, 255, 255, 255, 255], 2u32, 1u32);
        let (px, w, _) = icons::scale_to(&src, 4).expect("缩放不应失败");
        assert_eq!(w, 4);
        let lum: Vec<u8> = (0..4).map(|i| px[i * 4]).collect(); // 灰度图 R=G=B
        assert!(
            lum[0] < lum[1] && lum[1] < lum[2] && lum[2] < lum[3],
            "放大后必须单调过渡（最近邻会得到 [0,0,255,255]）: {lum:?}"
        );
    }

    /// ⛔ 放大时的双线性**必须先预乘**：straight alpha 下透明像素的 RGB 常为 0（或 255），
    ///   直接插值会把那个颜色混进半透明边缘 ⇒ 图标外圈出现**黑晕/白边**。
    ///
    /// 构造：左 = 不透明**黑**（RGB=0），右 = 完全透明**白**（RGB=255, A=0）。
    ///   · 正确（先预乘）：透明像素的预乘 RGB = 0 ⇒ 任何插值结果的反预乘 RGB 都是 **0**。
    ///   · 错误（直接插值）：中间像素会拿到 RGB ≈ 127 的**灰**（半透明灰边）。
    ///
    /// 可证伪：把 `acc[c] += px[si] as f32 * a / 255.0 * w` 里的 `* a / 255.0` 去掉 ⇒ 转红。
    #[test]
    fn scale_up_does_not_darken_transparent_edges() {
        let src = (vec![0, 0, 0, 255, 255, 255, 255, 0], 2u32, 1u32);
        let (px, _, _) = icons::scale_to(&src, 4).expect("缩放不应失败");
        for i in 0..4 {
            let (r, a) = (px[i * 4], px[i * 4 + 3]);
            assert!(
                a == 0 || r == 0,
                "像素 {i}: alpha={a} 但 RGB={r} ⇒ 透明像素的颜色混进了边缘（缺预乘）"
            );
        }
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

    // ── 拖拽：位置钳制与四档优先级 ─────────────────────────────

    /// 钳制必须**两端都夹住**：左端越界夹到 0，右端越界夹到 `taskbar_w - widget_w`。
    ///
    /// 可证伪：把 `.clamp(0, max)` 改成 `.max(0)`（丢掉右端），第 2 条断言立刻转红 ——
    ///   窗口会被拖出任务栏右缘，内容还在但位置在屏幕外，用户只能重启恢复。
    #[test]
    fn clamp_rel_x_pins_both_ends() {
        assert_eq!(clamp_rel_x(-50, 100, 1000), 0, "左端越界夹到 0");
        assert_eq!(clamp_rel_x(5000, 100, 1000), 900, "右端越界夹到 w-widget_w");
        assert_eq!(clamp_rel_x(300, 100, 1000), 300, "区间内必须原样保留");
    }

    /// 窗口比任务栏还宽（异常 DPI / 内容过多）⇒ 上界归零 ⇒ 钉在左端，**绝不返回负值**。
    ///
    /// 可证伪：把 `(taskbar_w - widget_w).max(0)` 里的 `.max(0)` 去掉，
    ///   上界变成负数 ⇒ `clamp(0, 负)` **会 panic**（`min > max`），本条转红。
    #[test]
    fn clamp_rel_x_never_returns_negative_when_widget_overflows_taskbar() {
        assert_eq!(clamp_rel_x(300, 1200, 1000), 0);
    }

    /// ⭐ 四档优先级的**唯一落地点**：固定 > 拖拽落点 > 上次 > 贴靠。
    ///
    /// 可证伪（三种，各自钉住一档）：
    ///   ① 把 `locked` 分支去掉（改成永远走 `custom_x`）⇒ `locked=true` 那条转红；
    ///   ② 把 `custom_x` 与 `last` 的优先级对调 ⇒ 「拖拽落点压过 last」那条转红；
    ///   ③ 把 `last != 0` 写成 `last > 0` 之外的条件（如恒 true）⇒ 首帧那条转红。
    #[test]
    fn resolve_rel_x_priority_is_locked_then_custom_then_last_then_aligned() {
        let (w, tb) = (100, 1000);
        // ① 固定位置：压过一切（拖拽落点还在配置里，但此刻不该生效）
        assert_eq!(
            resolve_rel_x(true, 700, Some(300), 500, w, tb),
            700,
            "固定位置时必须用贴靠结果，即使有拖拽落点"
        );
        // ② 不固定 + 拖过：用用户放下的位置
        assert_eq!(
            resolve_rel_x(false, 700, Some(300), 500, w, tb),
            300,
            "用户显式拖拽的位置必须压过「上次画过的地方」"
        );
        // ③ 不固定 + 没拖过 + 画过：沿用上次
        assert_eq!(
            resolve_rel_x(false, 700, None, 500, w, tb),
            500,
            "没拖过时应沿用上次位置（「不固定」的旧语义）"
        );
        // ④ 不固定 + 没拖过 + 首帧（哨兵 0）：退回贴靠
        assert_eq!(
            resolve_rel_x(false, 700, None, 0, w, tb),
            700,
            "`last == 0` 是「还没画过」的哨兵，必须退回贴靠结果"
        );
    }

    /// ⭐ 拖拽落点**同样要钳制**：配置里可能存着换分辨率/改缩放之前的旧值。
    ///
    /// 可证伪：把 `resolve_rel_x` 末尾的 `clamp_rel_x` 去掉 ⇒ 本条返回 99999 转红。
    /// ⚠️ 这是「拖拽落点」与「贴靠结果」的关键区别 —— 后者由 `align_in_slot` 保证在槽内，
    ///   前者来自**磁盘上的旧配置**，没有任何上界保证。
    #[test]
    fn resolve_rel_x_clamps_the_persisted_drag_position() {
        assert_eq!(
            resolve_rel_x(false, 0, Some(99999), 0, 100, 1000),
            900,
            "持久化的拖拽落点必须按任务栏宽度钳制"
        );
        assert_eq!(
            resolve_rel_x(false, 0, Some(-99999), 0, 100, 1000),
            0,
            "负数落点同样要被夹回 0"
        );
    }

    // ── 拖拽底衬的圆角判据 ───────────────────────────────────

    /// ⭐ 圆角矩形 ≠ 内切椭圆：**四条直边的中点必须命中**。
    ///
    /// 可证伪：把实现改成椭圆判据（`(x-cx)²/r² + (y-cy)²/r² <= 1` 不分支直接算），
    ///   左/右边中点会落到椭圆外 ⇒ 本条转红。这是最容易写出的错误实现。
    #[test]
    fn inside_rounded_rect_includes_edge_midpoints() {
        let (w, h, r) = (100, 40, 6);
        assert!(inside_rounded_rect(0, h / 2, w, h, r), "左边中点必须命中");
        assert!(
            inside_rounded_rect(w - 1, h / 2, w, h, r),
            "右边中点必须命中"
        );
        assert!(inside_rounded_rect(w / 2, 0, w, h, r), "上边中点必须命中");
        assert!(
            inside_rounded_rect(w / 2, h - 1, w, h, r),
            "下边中点必须命中"
        );
        assert!(inside_rounded_rect(w / 2, h / 2, w, h, r), "中心必须命中");
    }

    /// 四个**角点**必须被挖掉（否则底衬看起来是直角方块，与圆角设计不符）。
    ///
    /// 可证伪：把 `inside_rounded_rect` 改成恒 `true` ⇒ 本条四条断言全红。
    #[test]
    fn inside_rounded_rect_excludes_corners() {
        let (w, h, r) = (100, 40, 6);
        for (x, y) in [(0, 0), (w - 1, 0), (0, h - 1), (w - 1, h - 1)] {
            assert!(
                !inside_rounded_rect(x, y, w, h, r),
                "角点 ({x},{y}) 必须落在圆角之外"
            );
        }
    }

    /// 越界坐标一律不算命中（`fill_hover_backdrop` 依赖它做边界判断）。
    /// `r == 0` 时退化为**实心矩形**（不 panic、不挖角）。
    #[test]
    fn inside_rounded_rect_handles_out_of_range_and_zero_radius() {
        let (w, h) = (100, 40);
        assert!(!inside_rounded_rect(-1, 20, w, h, 6));
        assert!(!inside_rounded_rect(w, 20, w, h, 6));
        assert!(!inside_rounded_rect(50, h, w, h, 6));
        assert!(
            inside_rounded_rect(0, 0, w, h, 0),
            "半径 0 ⇒ 退化为实心矩形"
        );
    }

    // ── 合成：blend_over（source-over）──────────────────────────────────

    /// ⭐ 无底衬（`dst` 全 0）⇒ 合成结果必须**逐位等于**源像素。
    ///
    /// 意义：钉住「改合成**不会回归**未悬停时的外观」—— 那是本次改动最大的回归风险面。
    /// 可证伪：把 `blend_over` 改成「恒返回 `dst`」⇒ 本条三条断言全红。
    #[test]
    fn blend_over_with_no_backdrop_is_identity() {
        assert_eq!(blend_over(0, 128, 0, 0, 0), 128 << 24, "半透明黑字");
        assert_eq!(blend_over(0, 255, 255, 255, 255), 0xFFFF_FFFF, "不透明白");
        assert_eq!(
            blend_over(0, 60, 12, 34, 56),
            (60 << 24) | (12 << 16) | (34 << 8) | 56
        );
    }

    /// 源完全不透明 ⇒ 结果 = 源（底衬被完全覆盖，不留一点白）。
    #[test]
    fn blend_over_opaque_source_replaces_destination() {
        let dst = 0x9999_9999; // alpha = 153 的白色预乘底衬
        assert_eq!(blend_over(dst, 255, 0, 0, 0), 255 << 24, "纯黑不透明");
        assert_eq!(
            blend_over(dst, 255, 255, 255, 255),
            0xFFFF_FFFF,
            "纯白不透明"
        );
    }

    /// ⛔⛔ **本函数存在的理由**：底衬（alpha=153）之上，抗锯齿边缘（覆盖度 **< 153**）
    ///   **必须仍被画出来**。旧实现「取最大 alpha」在这里会保留底衬 ⇒ 字形被侵蚀 ⇒ 变细。
    ///
    /// 可证伪：把 `blend_over` 换回 `if a > dst >> 24 { packed } else { dst }`
    /// ⇒ alpha 断言（177 ≠ 153）立刻转红。
    #[test]
    fn blend_over_antialiased_edge_is_not_eroded() {
        let dst = (153u32 << 24) | (153 << 16) | (153 << 8) | 153; // 白色预乘底衬
        let out = blend_over(dst, 60, 0, 0, 0); // 覆盖度 60 的黑字边缘
                                                // 60 + 153 × (255−60)/255 = 60 + 117 = 177（旧实现给 153）
        assert_eq!(out >> 24, 177, "alpha 必须是叠加后的 177，不是 153");
        // 0 + 153 × 195/255 = 117（底衬被黑字压暗）
        assert_eq!((out >> 16) & 0xFF, 117, "红分量必须是 117");
        assert!((out >> 16) & 0xFF < 153, "必须比纯底衬暗 ⇒ 字确实画上去了");
    }

    // ── hover 底衬：主题 → 不透明度 / 光标命中判定 ──────────────────

    /// ⭐ 两个 alpha 必须**逐字**等于 FluentFlyout 换算出来的值 —— 这是「与 FluentFlyout
    ///   一致」这句承诺的**唯一机械判据**（改了常量却忘了另一处，只有这里会转红）。
    #[test]
    fn hover_backdrop_alpha_matches_fluent_flyout() {
        // 浅色分支：Color.FromArgb(255,255,255,255) × Opacity 0.6 ⇒ 153
        assert_eq!(hover_backdrop_alpha_for(true), 153);
        // 深色分支：Color.FromArgb(197,255,255,255) × Opacity 0.075 ⇒ 14.775 ⇒ 15
        assert_eq!(hover_backdrop_alpha_for(false), 15);
        assert_eq!(HOVER_BACKDROP_ALPHA_LIGHT, 153);
        assert_eq!(HOVER_BACKDROP_ALPHA_DARK, 15);
        // ⛔ 底衬必须**比内容淡**：alpha 到 255 就成了实心白块，会把图标与文字压住。
        assert!(hover_backdrop_alpha_for(true) < 255);
        assert!(hover_backdrop_alpha_for(false) < hover_backdrop_alpha_for(true));
    }

    /// 光标落在窗口矩形内 ⇒ hover 成立（含左上角，与「右/下排他」合起来覆盖全矩形）。
    #[test]
    fn want_hover_true_inside_rect() {
        let rect = Some((100, 200, 80, 40)); // left, top, w, h
        assert!(want_hover(Some((100, 200)), rect, false), "左上角（含）");
        assert!(
            want_hover(Some((179, 239)), rect, false),
            "右下角内侧（含）"
        );
        assert!(want_hover(Some((140, 220)), rect, false), "正中");
    }

    /// ⛔ 边界口径：`right` / `bottom` 是**排他**边界（宽度 = right - left）。
    ///   可证伪：把 `<` 改成 `<=` ⇒ 后两条立刻转红。
    #[test]
    fn want_hover_excludes_exclusive_edges() {
        let rect = Some((100, 200, 80, 40));
        assert!(
            !want_hover(Some((180, 220)), rect, false),
            "right 本身不算（排他边界）"
        );
        assert!(
            !want_hover(Some((140, 240)), rect, false),
            "bottom 本身不算（排他边界）"
        );
        assert!(!want_hover(Some((99, 220)), rect, false), "left 左侧不算");
        assert!(!want_hover(Some((140, 199)), rect, false), "top 上方不算");
    }

    /// ⭐ 拖拽期间恒 `true`：`SetCapture` 下光标可短暂移出窗口，不该让底衬闪进闪出。
    ///   可证伪：去掉 `if dragging { return true; }` ⇒ 第一条转红。
    #[test]
    fn want_hover_always_true_while_dragging() {
        assert!(want_hover(Some((0, 0)), Some((100, 200, 80, 40)), true));
        assert!(
            want_hover(None, None, true),
            "拖拽时即使取不到光标/矩形也要显示底衬"
        );
    }

    /// ⚠️ 任一输入缺失 ⇒ `false`（宁可不显示，也不误显示）。
    #[test]
    fn want_hover_false_when_input_missing() {
        assert!(!want_hover(None, Some((100, 200, 80, 40)), false));
        assert!(!want_hover(Some((140, 220)), None, false));
        assert!(!want_hover(None, None, false));
    }

    // ── plan_tick：维护节拍的决策（Z 序维护 / 取数 / 自愈）────────

    /// ⭐ 两个节拍常量必须真的凑出文档里承诺的 30s —— 改任一常量却忘了另一个，
    ///   文档就变成假话（而这类「文档说 30s、实际 6s」不会有任何报错）。
    #[test]
    fn maintenance_tick_times_refresh_interval_is_thirty_seconds() {
        assert_eq!(
            MAINTENANCE_TICK_SECS * REFRESH_EVERY_TICKS,
            30,
            "维护 tick × 取数间隔 必须等于 30s（文档与日志都这么写）"
        );
    }

    /// 窗口存活时，**每个 tick 都要请求重申 Z 序**（这是「被压住后能自愈」的唯一来源）。
    ///
    /// 可证伪：把 `raise` 改成 `false`，或只在 30s 边界置真 ⇒ 本条立刻转红。
    #[test]
    fn plan_tick_raises_on_every_tick_while_alive() {
        for tick in 1..=40u64 {
            let p = plan_tick(true, false, tick);
            assert!(p.raise, "tick={tick}：存活时必须重申 Z 序");
            assert!(!p.remount, "tick={tick}：已挂载就不该再挂");
        }
    }

    /// 取数（WMI，600ms+）**只能**落在 30s 边界上，不能被 2s 的维护节拍带起来。
    ///
    /// 可证伪：把 `refresh: due` 改成 `true` ⇒ 每 2s 跑一次 WMI（CPU 打爆）。
    #[test]
    fn plan_tick_refreshes_only_on_the_thirty_second_boundary() {
        let refreshed: Vec<u64> = (1..=45u64)
            .filter(|&t| plan_tick(true, false, t).refresh)
            .collect();
        assert_eq!(
            refreshed,
            vec![
                REFRESH_EVERY_TICKS,
                2 * REFRESH_EVERY_TICKS,
                3 * REFRESH_EVERY_TICKS
            ],
            "取数只应落在 30s 边界"
        );
    }

    /// ⭐ **任务栏换句柄 = Explorer 重建 ⇒ 下一个 tick 就重建**（不等 30s 慢节拍）。
    ///
    /// 可证伪：把 `taskbar_changed || due` 改成 `due` ⇒ 本条转红
    /// （用户会盯着空任务栏等满 30s）。
    #[test]
    fn plan_tick_remounts_immediately_when_taskbar_handle_changed() {
        let p = plan_tick(false, true, 1);
        assert!(p.remount, "任务栏换了 ⇒ 立刻重建，不受 30s 节拍限制");
        assert!(!p.raise, "窗口都不在了，重申 Z 序没有意义");
        assert!(!p.refresh, "重建走挂载路径，不需要先取数");
    }

    /// ⛔⛔ **同一个任务栏上挂不上时，绝不能每 tick 重试** —— 那会触发
    ///   「反复操作任务栏 → 恶化；静置 → 自愈」的环境效应（PLAYBOOK §E3）。
    ///
    /// 可证伪：把 `taskbar_changed || due` 改成 `true` ⇒ 15 个 tick 里出现 15 次重建 ⇒ 转红。
    #[test]
    fn plan_tick_does_not_storm_remount_on_the_same_taskbar() {
        let attempts = (1..=45u64)
            .filter(|&t| plan_tick(false, false, t).remount)
            .count();
        assert_eq!(
            attempts, 3,
            "45 个 tick（90s）里只允许 3 次重试（30s 一次）；每 tick 重试 = 挂载风暴"
        );
    }

    /// 窗口不在时**不得**请求重申 Z 序（句柄已失效，投递没有意义）。
    #[test]
    fn plan_tick_never_raises_while_not_alive() {
        for tick in 1..=40u64 {
            assert!(
                !plan_tick(false, false, tick).raise && !plan_tick(false, true, tick).raise,
                "tick={tick}：窗口不在时不该重申 Z 序"
            );
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
