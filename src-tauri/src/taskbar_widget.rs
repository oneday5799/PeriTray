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

// ═══════════════════════════════════════════════════════════════════
// 单击 vs 拖拽的区分（音乐面板的按钮点击依赖它）
// ═══════════════════════════════════════════════════════════════════

/// 判定「这次按下是一次**点击**而不是拖拽窗口」的两个阈值。
///
/// ⛔ **为什么必须有这个判据**：`WM_LBUTTONDOWN` 现行实现是**无条件**
///   `drag_begin` → `SetCapture`（见 `drag_begin`）⇒ 鼠标按下即进入拖拽态。
///   音乐面板的三键与「切换」按钮靠**单击**触发 ⇒ 没有阈值就分不出
///   「点一下按钮」与「按住拖窗口」，表现为**点按钮没反应 / 拖一下就误触发按钮**。
///
/// ⭐ 两个条件都满足才算点击：
///   · 位移 < [`CLICK_SLOP_PX`]（默认 Windows 的拖拽阈值量级）
///   · 按下到抬起 < [`CLICK_TIME_MS`]
#[cfg(target_os = "windows")]
const CLICK_SLOP_PX: i32 = 4;
#[cfg(target_os = "windows")]
const CLICK_TIME_MS: u64 = 400;

#[cfg(target_os = "windows")]
static PRESS_CURSOR: std::sync::Mutex<Option<(i32, i32, u64)>> = std::sync::Mutex::new(None);

/// 当前**被按下**的按钮（`-1` = 无）。用户 2026-09-29 要求所有可点按钮按下时变灰。
///
/// ⭐ 存的是**布局里那一项的编号**，不是屏幕坐标：
///   · 命中判定用**已发布的布局**（`music_layout` / `DEV_SWITCH_RECT`），
///     与绘制同源 —— 绝不现算矩形（那是「绘制与命中必须同源」那条纪律）。
///   · 编号在「按下 → 抬起」之间不会因窗口宽度变化而漂移。
const PRESS_NONE: i32 = -1;
const PRESS_MUSIC_PREV: i32 = 0;
const PRESS_MUSIC_PLAY: i32 = 1;
const PRESS_MUSIC_NEXT: i32 = 2;
const PRESS_MUSIC_SWITCH: i32 = 3;
const PRESS_DEV_SWITCH: i32 = 4;

static PRESSED: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(PRESS_NONE);

/// 三键的编号（与 [`press_target_at`] 的映射严格对应）。
#[cfg(target_os = "windows")]
fn music_btn_id(i: usize) -> i32 {
    match i {
        0 => PRESS_MUSIC_PREV,
        1 => PRESS_MUSIC_PLAY,
        _ => PRESS_MUSIC_NEXT,
    }
}

/// 只改状态、不发消息（测试与「无窗口」场景用；生产走 [`set_pressed`]）。
#[cfg(test)]
fn set_pressed_raw(id: i32) {
    PRESSED.store(id, std::sync::atomic::Ordering::Release);
}

/// 读当前按下态（绘制时用）。
#[cfg(target_os = "windows")]
pub fn pressed_id() -> i32 {
    PRESSED.load(Ordering::Acquire)
}

/// 命中测试：坐标落在哪个按钮上（**只查已发布布局**）。
#[cfg(target_os = "windows")]
pub fn press_target_at(local: (i32, i32)) -> i32 {
    // ⛔ 动画期间一律「没命中」：内容正在横向移动，而 `LAST_ITEM_RECTS` 是
    //   上一帧的坐标 ⇒ 按它判定会把点击落到**已经滑走**的元素上。
    //   150ms 窗口很短，忽略这一次点击的代价远小于「点到了错的东西」。
    if switch_anim_active() {
        return PRESS_NONE;
    }
    // ⭐⭐ 切换按钮**只在 hover 时可点**（用户 2026-09-29：「hover 时才显示」）。
    //   ⚠️ 必须与绘制用**同一个判据**（`switch_icon_alpha`）：只改绘制不改这里，
    //   就会留下一个「看不见但点得到」的按钮 —— 那比按钮常驻更糟，
    //   因为用户点了有反应却**看不到自己点了什么**。
    //   （`want_hover` 用窗口矩形判光标在内 ⇒ 能按到切换键时必然已 hover。）
    let hovered_now = HOVERED.load(Ordering::Acquire) && switch_clickable(true);
    match current_panel() {
        Some(crate::config::TaskbarPanel::Music) => match hit_test_music(local) {
            MusicHit::Prev => PRESS_MUSIC_PREV,
            MusicHit::PlayPause => PRESS_MUSIC_PLAY,
            MusicHit::Next => PRESS_MUSIC_NEXT,
            // 未 hover ⇒ 切换键不可点
            MusicHit::Switch if hovered_now => PRESS_MUSIC_SWITCH,
            MusicHit::Switch | MusicHit::None => PRESS_NONE,
        },
        Some(crate::config::TaskbarPanel::Devices) => {
            if hovered_now && dev_switch_hit(local) {
                PRESS_DEV_SWITCH
            } else {
                PRESS_NONE
            }
        }
        None => PRESS_NONE,
    }
}

/// `WM_LBUTTONDOWN`：记下按到哪个按钮（没按在任何按钮上则清空）。
///
/// ⛔ **必须在 `drag_begin` 之前调用**：`drag_begin` 会 `SetCapture`，
///   而 `SetCapture` 可能立刻触发 `WM_CAPTURECHANGED`（那里会清按下态）。
#[cfg(target_os = "windows")]
fn press_begin(hwnd: *mut core::ffi::c_void) {
    let mut pt = windows_sys::Win32::Foundation::POINT { x: 0, y: 0 };
    if unsafe { windows_sys::Win32::UI::WindowsAndMessaging::GetCursorPos(&mut pt) } == 0 {
        set_pressed(PRESS_NONE, hwnd);
        return;
    }
    let (wx, wy, _, _) = window_screen_rect(hwnd).unwrap_or((0, 0, 0, 0));
    set_pressed(press_target_at((pt.x - wx, pt.y - wy)), hwnd);
}

/// 置/清按下态；**状态真变了**才请求重绘（否则一次按下会触发两次无谓重绘）。
///
/// ⛔ 只 `post_refresh`（重绘、读现有快照），**绝不** `refresh_async()`——
///   后者会触发 600ms+ 的 WMI 取数，按一下就重拉一遍设备列表。
#[cfg(target_os = "windows")]
fn set_pressed(id: i32, hwnd: *mut core::ffi::c_void) {
    if PRESSED.swap(id, Ordering::AcqRel) == id {
        return;
    }
    FORCE_REPAINT.store(true, Ordering::Release);
    unsafe { ffi::post_refresh(hwnd as _) };
}

/// 记下按下时的光标与时刻（`WM_LBUTTONDOWN` 时调用）。
#[cfg(target_os = "windows")]
fn press_record() {
    let mut pt = windows_sys::Win32::Foundation::POINT { x: 0, y: 0 };
    if unsafe { windows_sys::Win32::UI::WindowsAndMessaging::GetCursorPos(&mut pt) } == 0 {
        return;
    }
    *crate::state::lock_unpoisoned(&PRESS_CURSOR) = Some((pt.x, pt.y, now_ms()));
}

/// 抬起时判定这次是不是「点击」，并给出**按下时的窗口局部坐标**。
#[cfg(target_os = "windows")]
fn press_take_click() -> Option<(i32, i32)> {
    let pressed = crate::state::lock_unpoisoned(&PRESS_CURSOR).take()?;
    let (px, py, t0) = pressed;
    let mut pt = windows_sys::Win32::Foundation::POINT { x: 0, y: 0 };
    if unsafe { windows_sys::Win32::UI::WindowsAndMessaging::GetCursorPos(&mut pt) } == 0 {
        return None;
    }
    let dx = (pt.x - px).abs();
    let dy = (pt.y - py).abs();
    if dx > CLICK_SLOP_PX || dy > CLICK_SLOP_PX {
        return None; // 位移超阈值 ⇒ 是拖拽
    }
    if now_ms().saturating_sub(t0) > CLICK_TIME_MS {
        return None; // 按太久 ⇒ 长按，不是点击
    }
    let handle = WIDGET_HWND.load(Ordering::SeqCst);
    let (wx, wy, _, _) = window_screen_rect(handle as _)?;
    Some((px - wx, py - wy))
}

/// 处理一次「点击」（`WM_LBUTTONUP` 时调用）。
///
/// ⭐ 判据与**音乐面板已发布的布局**比对（不现算矩形），与绘制同源。
/// ⛔ 只发命令给后台线程 / 改配置，**绝不** `emit`、**绝不**持锁调窗口 API。
#[cfg(target_os = "windows")]
pub fn on_click(hwnd: *mut core::ffi::c_void, local: (i32, i32)) {
    match current_panel() {
        Some(crate::config::TaskbarPanel::Music) => {
            let hit = hit_test_music(local);
            if hit != MusicHit::None {
                if crate::config::verbose_log_enabled() {
                    append_log(&format!("[widget] 音乐面板点击: {hit:?} @({})", local.0));
                }
                activate_music(hwnd, hit);
            }
        }
        Some(crate::config::TaskbarPanel::Devices) => {
            // 设备面板的设备项**没有**点击语义（按下即拖拽），但**最右的切换按钮有**
            if dev_switch_hit(local) {
                if crate::config::verbose_log_enabled() {
                    append_log(&format!("[widget] 设备面板点击: Switch @({})", local.0));
                }
                advance_switch_target(hwnd);
                return;
            }
            if crate::config::verbose_log_enabled() {
                append_log(&format!("[widget] 设备面板点击（无动作）@({})", local.0));
            }
        }
        None => {}
    }
}

// ═══════════════════════════════════════════════════════════════════
// 音乐面板的交互：面板分派、挂载判据、单击、滚轮闸、切换状态机
// ═══════════════════════════════════════════════════════════════════

/// 音乐数据变化时的**唯一入口**（由 `taskbar_music` 后台线程调用）。
///
/// ⛔⛔ **它跑在 SMTC 的回调线程上**，因此只做两件事：
///   ① 置 `FORCE_REPAINT`（重绘判据「数据变了 ∨ 槽位移动 ∨ FORCE_REPAINT」里的一项）
///   ② 投递一条**异步**的挂载重估
/// **绝不**在这里等主线程、也**绝不**调 Tauri 的窗口 API
/// （`app.run_on_main_thread` 内部是 `rx.recv()` 无超时等主线程 ⇒ 在回调线程上
/// 同步调它就是 AB/BA 死锁的前兆）。这与本文件「`wnd_proc` 绝不 emit」同源。
///
/// ⭐ 为什么必须能触发**挂载/卸载**而不只是重绘：音乐面板是**内容级**存在，
///   「有没有会话」决定组件**在不在**；只重绘的话，无会话→有会话时窗口仍然挂着空窗。
#[cfg(target_os = "windows")]
pub fn on_music_changed() {
    // ⛔⛔ **只重绘、绝不取数**（2026-09-29 修：播放/暂停图标延迟数秒才变）。
    //
    // 根因：这里原先调 `refresh_async()`，而那是**设备**刷新通道 ——
    //   `spawn_blocking(fetch_into_snapshot)` 会跑一轮 **WMI 设备枚举（实测 600ms+）**，
    //   跑完才 `post_refresh`。音乐面板的内容全部来自 SMTC 快照，
    //   而快照在进到这里之前**已经更新完毕** ⇒ 这一轮 WMI **纯浪费**，
    //   且它的耗时直接表现为「点暂停后音乐立刻停了、按钮却迟迟不变」。
    //
    // ⚠️ 也**不再置 `FORCE_REPAINT`**：那个标志是 `fetch_into_snapshot` 内部消费的，
    //   绕开取数后由本路径置位就会**一直残留**，直到下一轮 30s 慢刷新才被误消费
    //   （表现为「无缘无故重绘一次」）。本条路径是「快照变了」，不是「配置要求重绘」。
    //
    // ⭐ 这与 `set_pressed` / hover 轮询是**同一条纪律**（本文件里已写了两遍）：
    //   `WM_APP_REFRESH` = 读现有快照重画；`refresh_async` = 重新取数。
    //   凡是「数据已经在内存里了、只是想重画」的场景，一律走前者。
    if let Some(app) = crate::taskbar_music::app() {
        // ⛔ **异步**投递（`run_on_main_thread` 本身已是排队语义），不在此阻塞
        let _ = app.run_on_main_thread(request_repaint);
    } else {
        // 没有 app 句柄（极早期）⇒ 只靠 2s 维护循环也会收敛
        request_repaint();
    }
}

/// 请求**立刻**重绘一帧（读现有快照，**不取任何数据**）。**任意线程可调**。
///
/// ⛔ 与 [`refresh_async`] 的区别就是本函数存在的全部理由：后者会跑 WMI 取数
///   （600ms+），凡是「数据已就位、只是画面没跟上」的场景都必须用本函数。
#[cfg(target_os = "windows")]
pub fn request_repaint() {
    let handle = WIDGET_HWND.load(Ordering::SeqCst);
    // ⚠️ 未挂载 ⇒ 没有消费方，直接早退（否则消息投进虚空，日志里什么都没有）
    if !widget_alive() {
        return;
    }
    unsafe { ffi::post_refresh(handle as *mut core::ffi::c_void) };
}

/// 启动音乐后台线程（应用启动时调一次）。
#[cfg(target_os = "windows")]
pub fn start_music(app: &tauri::AppHandle) {
    crate::taskbar_music::start(app.clone());
}

/// 音乐面板的命中结果（一次点击落在这张表的哪一格）。
#[cfg(target_os = "windows")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MusicHit {
    None,
    /// 上一首
    Prev,
    /// 播放 / 暂停
    PlayPause,
    /// 下一首
    Next,
    /// 「切换」按钮
    Switch,
}

#[cfg(target_os = "windows")]
fn point_in(r: &windows_sys::Win32::Foundation::RECT, p: (i32, i32)) -> bool {
    p.0 >= r.left && p.0 < r.right && p.1 >= r.top && p.1 < r.bottom
}

/// 把光标局部坐标解析成音乐面板的命中结果（**读已发布的布局**，不现算）。
#[cfg(target_os = "windows")]
pub fn hit_test_music(cursor: (i32, i32)) -> MusicHit {
    let Some(l) = music_layout() else {
        return MusicHit::None;
    };
    if let Some(sb) = l.switch_btn {
        if point_in(&sb, cursor) {
            return MusicHit::Switch;
        }
    }
    if !l.hovered {
        return MusicHit::None; // 静态形态无可点区域
    }
    // 点在面板主体之外 ⇒ 不触发任何命令（否则「点空白」会误触发）
    if !point_in(&l.item, cursor) {
        return MusicHit::None;
    }
    for (i, r) in l.buttons.iter().enumerate() {
        if r.right > r.left && point_in(r, cursor) {
            return match i {
                0 => MusicHit::Prev,
                1 => MusicHit::PlayPause,
                _ => MusicHit::Next,
            };
        }
    }
    MusicHit::None
}

/// 「切换」按钮的语义（用户 2026-09-28 指定的循环顺序）：
///
/// ```text
///   会话1 → 会话2 → … → 会话N → 换组件 → 会话1 → …
/// ```
///
/// ⛔⛔ **「只有一个会话」时，会话列表在第一次点击前就已经走完** ⇒ 下一步应当是
///   **换组件**。我第一版写的守卫是 `if !need_panel_switch && n <= 1 { return; }`
///   —— 它把「没有下一个会话」误当成「无处可切」，于是 N=1（绝大多数情况）时
///   **点切换完全没反应**（用户 2026-09-28 实测报出）。
///   ⇒ 正确判据是「**还有没有下一个会话**」，而不是「会话数是否 > 1」。
fn advance_switch_target(hwnd: *mut core::ffi::c_void) {
    let snap = crate::taskbar_music::snapshot();
    let n = snap.sessions.len();
    let cur_panel = current_panel();

    // ① ⭐⭐ **会话轮转只在音乐面板里做**（2026-09-29 改）。
    //
    //   原先不分面板：设备面板上点「切换」也先切会话 ⇒ **从设备面板切到音乐
    //   面板要点两次**（真机实测），而按钮就画在最右缘、提示写的就是「切换」，
    //   用户的预期是「换一块显示的内容」。
    //   ⇒ 设备面板上这个按钮**只切面板**，一次到位；
    //     音乐面板上它才是「下一个会话」，会话走完再交给 ② 切面板。
    //
    //   ⚠️ 这也顺带修掉「永远切不过去」：`current` 由 worker 按系统当前重算，
    //     不钉住的话第①步恒成立（见 `taskbar_music::pinned_session_id`）。
    let on_music_panel = cur_panel == Some(crate::config::TaskbarPanel::Music);
    if on_music_panel && n > 1 && snap.current + 1 < n {
        crate::taskbar_music::select_session(snap.current + 1);
        return;
    }
    // ② 另一个面板可用就换过去
    if let (Some(from), Some(other)) = (cur_panel, other_panel_if_available(cur_panel)) {
        // ⭐ **先落配置，再启动画**（顺序即契约）：配置是**持久状态**，
        //   必须在任何可能失败的步骤之前就位（AGENTS.md「失败路径不得留下
        //   成功状态」的镜像要求：这里反过来——先落定，出错才不会退回旧面板
        //   却已经动过画面）。动画失败也只是「这次没播」，面板已换对。
        crate::config::with_config_mut(|c| c.taskbar_panel = other);
        // ⭐⭐ **进入音乐面板一律从会话 0 开始**——这就是「环」的闭合点。
        //
        //   用户 2026-09-29 选定「单按钮 + 循环语义」，整个环是：
        //   ```text
        //   Devices --点--> Music/会话0 --点--> 会话1 --点--> Devices --点--> …
        //   ```
        //   ⚠️ 若**不**归零：从音乐面板离开时 `current` 停在 `n-1` 且被钉住，
        //   再进来点一下又是「没有下一个」⇒ **直接切面板**
        //   ⇒ **会话 0 永久不可达**（真机实测：音乐面板上点「切换」只会切面板）。
        //   归零同时也让每次进入音乐面板的起点可预期。
        //
        //   ⚠️ 归零会带来「换会话 ⇒ 封面要重新解码」的窗口（worker 异步），
        //   那由 `select_session` **同步清 `cover_hash`** 来保证快照自洽
        //   （见 `taskbar_music::select_session`）——两处必须一起改，
        //   只改其一会退回「闪现旧封面」。
        crate::taskbar_music::select_session(0);
        // ⛔ 会话轮转（上面的 ①）**不进动画**：同一面板内换歌，宽度不变，
        //   套一层横向滑动只会让整段歌名平移，观感更差。
        if !begin_switch_anim(hwnd, from, other) {
            // 拿不到守卫 / 画不出素材 ⇒ 落回**瞬时切换**（与动画引入前一致）
            FORCE_REPAINT.store(true, Ordering::Release);
            refresh_async();
        }
        append_log(&format!("[widget] 切换组件 → {other:?}"));
        return;
    }
    // ③ 没有可换的面板，但有多会话 ⇒ 会话内回绕（同样只在音乐面板）
    if on_music_panel && n > 1 {
        crate::taskbar_music::select_session(0);
        return;
    }
    // ④ 无处可切（此时按钮本就不该显示，见 switch_visible）
    if crate::config::verbose_log_enabled() {
        append_log(&format!(
            "[widget] 切换无可用目标: 面板={cur_panel:?} 会话数={n}"
        ));
    }
}

/// 另一个面板是否**也可用**（可用才换过去）。
fn other_panel_if_available(
    cur: Option<crate::config::TaskbarPanel>,
) -> Option<crate::config::TaskbarPanel> {
    let music_ok = crate::taskbar_music::snapshot().available();
    let dev_ok = crate::config::with_config(crate::config::taskbar_devices_available);
    let (other, other_ok) = match cur? {
        crate::config::TaskbarPanel::Music => (crate::config::TaskbarPanel::Devices, dev_ok),
        crate::config::TaskbarPanel::Devices => (crate::config::TaskbarPanel::Music, music_ok),
    };
    other_ok.then_some(other)
}

// ══ 面板切换动画（Music ↔ Devices）══════════════════════════════════════
//
// 形态（用户 2026-09-29 选定）：**横向滑动**，旧面板左移淡出、新面板自右滑入，
// 时长 **150ms**。⛔ 会话轮转（同一面板内的上一首/下一首会话）**不参与**动画 ——
// 它不换面板、宽度也不变，套一层滑动只会让歌名整段平移，观感更差。
//
// ── 线程模型（与全仓一致，AGENTS.md）────────────────────────────────────
//   点击（**主线程**，`activate_music`）→ 渲染两块面板各一次、缓存成位图
//   → 起一条**动画线程**，它只按 16ms 节拍 `PostMessageW(WM_APP_REFRESH)`，
//     **绝不碰 GDI**（`UpdateLayeredWindow` / 分层窗都属创建线程 = 主线程）
//   → 主线程每帧把两张缓存位图错位合成一帧再提交
//   → 动画线程在最后投 `WM_APP_SWITCH_END`，主线程释放缓存 + 清状态 + 补一帧常规重绘。

/// 动画时长（毫秒）—— 用户 2026-09-29 选定 150ms。
const SWITCH_ANIM_MS: u64 = 150;

/// 逐帧节拍。150 / 16 ≈ 9 帧。
const SWITCH_ANIM_FRAME_MS: u64 = 16;

/// 动画标志的超时自愈上限（毫秒）。
///
/// ⛔ 必须**显著大于** [`SWITCH_ANIM_MS`]：判据是「已占用**且**未超时」，
///   上限贴着动画时长的话，尾部那几帧会被误判成「线程已死」并强行夺回
///   ⇒ 动画被截断在 ~90%（肉眼是「滑到一半硬停」）。
const SWITCH_ANIM_TIMEOUT_MS: u64 = 2_000;

/// 面板绘制函数（[`draw_music_render`] / [`draw_items_render`]）的出口。
///
/// ⛔ 不用 `Option<(Dib, i32)>`：**常规路径里位图是在函数内部提交并释放的**，
///   返回它就等于让包装层再提交一次 ⇒ 同一帧提交两遍（多一次合成），
///   而更糟的是「已释放的位图被返回」——用它是 use-after-free。
///   两条出口语义完全不同，必须由类型区分，不能靠约定。
#[cfg(target_os = "windows")]
enum Painted {
    /// 已按常规路径**提交完毕**（`bool` = 提交是否成功）。
    Committed(bool),
    /// 位图**留给调用方**合成（动画路径）。位图所有权转移给调用方。
    Bitmap(ffi::Dib, i32),
}

#[cfg(target_os = "windows")]
impl Painted {
    fn committed_ok(self) -> bool {
        matches!(self, Painted::Committed(true))
    }
}

/// 面板编码（存进原子量，比枚举更省事且不依赖 `TaskbarPanel` 的可序列化性）。
const PANEL_CODE_NONE: u8 = 0;
const PANEL_CODE_MUSIC: u8 = 1;
const PANEL_CODE_DEVICES: u8 = 2;

#[cfg(target_os = "windows")]
fn panel_code(p: crate::config::TaskbarPanel) -> u8 {
    match p {
        crate::config::TaskbarPanel::Music => PANEL_CODE_MUSIC,
        crate::config::TaskbarPanel::Devices => PANEL_CODE_DEVICES,
    }
}

#[cfg(target_os = "windows")]
fn panel_from_code(c: u8) -> Option<crate::config::TaskbarPanel> {
    match c {
        PANEL_CODE_MUSIC => Some(crate::config::TaskbarPanel::Music),
        PANEL_CODE_DEVICES => Some(crate::config::TaskbarPanel::Devices),
        _ => None,
    }
}

// ── 动画状态 ────────────────────────────────────────────────────────────
/// 本模块专属的单飞标志。⛔ **不复用** `state::ANIMATING`（那是弹窗的）：
///   共用会导致「弹窗开合期间点任务栏切换，那一次点击被整个吞掉」。
static WIDGET_ANIMATING: AtomicBool = AtomicBool::new(false);
/// 本模块专属的动画起始时刻（[`crate::state::monotonic_ms`] 刻度），`0` = 无动画。
///
/// ⚠️ 必须与 [`WIDGET_ANIMATING`] **成对**——见 `state::try_begin_animation_on`
///   的说明：release 是 `panic = "abort"`，`Drop` 复位根本不跑，
///   唯一的兜底就是这个单调时钟。
static SWITCH_ANIM_STARTED: AtomicU64 = AtomicU64::new(0);
static SWITCH_ANIM_FROM_CODE: AtomicU8 = AtomicU8::new(PANEL_CODE_NONE);
static SWITCH_ANIM_TO_CODE: AtomicU8 = AtomicU8::new(PANEL_CODE_NONE);

/// 动画是否正在进行。命中测试与布局发布都要靠它让路（内容在动，不能按老坐标点）。
#[cfg(target_os = "windows")]
fn switch_anim_active() -> bool {
    SWITCH_ANIM_STARTED.load(Ordering::SeqCst) != 0
}

// ── 纯函数层（可确定性地单测，不碰窗口）─────────────────────────────────

/// 第 `now` 毫秒时的动画进度（`0..=1`）。
///
/// ⚠️ `started == 0` 视为「没有动画」⇒ 返回 `1.0`（**直接到终态**）。
///   绝不能返回 `0.0`：那会让窗口停死在「旧面板刚要滑走」的那一帧，
///   看起来就是「面板点坏了、切不过去」。
#[cfg(target_os = "windows")]
fn switch_progress(started: u64, now: u64, dur_ms: u64) -> f64 {
    if started == 0 || dur_ms == 0 {
        return 1.0;
    }
    (now.saturating_sub(started) as f64 / dur_ms as f64).clamp(0.0, 1.0)
}

/// 缓动：**ease-out cubic**（起步快、收尾稳）。`f(0)=0`、`f(1)=1`、单调不减。
///
/// ⭐ 为什么不用线性：150ms 很短，线性会显得「匀速推过去、然后硬停」；
///   ease-out 把变化集中在前 1/3 帧（立刻响应点击），尾部慢慢落位。
#[cfg(target_os = "windows")]
fn ease_out_cubic(t: f64) -> f64 {
    let t = t.clamp(0.0, 1.0);
    let inv = 1.0 - t;
    1.0 - inv * inv * inv
}

/// 第 `progress` 帧的错位量：返回 `(from_dx, to_dx, from_mul)`。
///
/// · `from_dx` = 旧面板左移量（`-off`，终态 `-slide`，完全移出左侧）
/// · `to_dx`   = 新面板左缘（`slide - off`，终态 `0`，完全就位）
/// · `from_mul`= 旧面板的整体强度（`0..=256`，`256` = 原样），随左移同步淡出
///
/// ⭐ `slide` 取**帧宽**（即新面板宽）⇒ 终态恰好是「旧面板全出、新面板全进」，
///   与切换后的常规帧**逐像素等价** ⇒ 收尾不会「跳一下」。
#[cfg(target_os = "windows")]
fn switch_offsets(progress: f64, slide: i32) -> (i32, i32, u32) {
    let p = ease_out_cubic(progress);
    let off = (p * slide as f64).round() as i32;
    let from_dx = -off;
    let to_dx = slide - off;
    let from_mul = (((1.0 - p) * 256.0).round() as i32).clamp(0, 256) as u32;
    (from_dx, to_dx, from_mul)
}

/// 逐通道乘一个 `0..=256` 的标量（`256` = 原样）。预乘空间对标量乘是封闭的。
#[cfg(target_os = "windows")]
fn mul_pixel(px: u32, m: u32) -> u32 {
    if m >= 256 {
        return px;
    }
    let r = ((px & 0xff) * m) >> 8;
    let g = (((px >> 8) & 0xff) * m) >> 8;
    let b = (((px >> 16) & 0xff) * m) >> 8;
    let a = ((px >> 24) * m) >> 8;
    r | (g << 8) | (b << 16) | (a << 24)
}

/// 预乘 source-over 横向 blit：把 `src` 以偏移 `dx` 合成进 `dst`，整体强度 `mul`。
///
/// ⭐ 全部用**整数**（`>> 8`）而非 `f32`：逐像素浮点在 15k 像素 × 2 张 × 9 帧
///   下不算大，但整数化后这段代码**不需要窗口就能单测**（喂 `Vec<u32>` 即可），
///   而混合公式（预乘 + source-over）恰恰是最容易写错、最该有判据的一段。
///
/// ⛔ 像素是 **BGRA 小端 `u32`** ⇒ alpha 在**高 8 位**（`px >> 24`）。
///   这条写反了不会编译报错，只会让整块画面变透明或变全黑。
#[cfg(target_os = "windows")]
fn blit_premul_scaled(
    dst: &mut [u32],
    dst_w: i32,
    dst_h: i32,
    src: &[u32],
    src_w: i32,
    src_h: i32,
    dx: i32,
    mul: u32,
) {
    if mul == 0 || src_w <= 0 || src_h <= 0 || dst_w <= 0 || dst_h <= 0 {
        return;
    }
    // ⚠️ 契约：`src` 恰为 `src_w * src_h` 个元素、`dst` 恰为 `dst_w * dst_h` 个。
    //   这里**不**做运行时校验（每帧每通道都查会拖慢合成），但写错时开发期立刻炸；
    //   写错的后果是越界读 ⇒ 直接崩在切片索引上，现场极难看。
    debug_assert!(src.len() >= (src_w * src_h) as usize, "源缓冲长度不足");
    debug_assert!(dst.len() >= (dst_w * dst_h) as usize, "目标缓冲长度不足");
    // 目标列区间 [max(dx,0), ..)，对应源列区间 [max(-dx,0), ..)
    let dst_x0 = dx.max(0);
    let src_x0 = (-dx).max(0);
    let cols = (src_w - src_x0).min(dst_w - dst_x0);
    if cols <= 0 {
        return;
    }
    let cols = cols as usize;
    for row in 0..src_h.min(dst_h) {
        let s = &src[(row as usize * src_w as usize + src_x0 as usize)..][..cols];
        let d = &mut dst[(row as usize * dst_w as usize + dst_x0 as usize)..][..cols];
        for (dp, sp) in d.iter_mut().zip(s.iter()) {
            let sp = mul_pixel(*sp, mul);
            if sp == 0 {
                continue;
            }
            // 预乘 source-over：dst = src + dst * (1 - src.a)
            // ⚠️ `inv` 恰好是 `255 - src.a` ⇒ 结果 alpha ≤ 255，**不会溢出**。
            let inv = 255 - (sp >> 24);
            *dp = sp + mul_pixel(*dp, inv);
        }
    }
}

// ── 动画期间缓存的两张面板位图 ──────────────────────────────────────────

/// `Dib` 的**只读视图**：可以安全带出锁的 POD。
///
/// ⛔ 刻意**不**给 `Dib` 加 `#[derive(Copy)]`：`free_dib` 不可幂等，
///   一旦 `Dib` 可复制，「同一张位图被释放两次」就成了可能（崩溃在 GDI 内，
///   现场极难看）。这里只需要一份能穿过锁边界的读引用。
#[cfg(target_os = "windows")]
#[derive(Clone, Copy)]
struct DibView {
    bits: *mut u32,
    w: i32,
    h: i32,
}

#[cfg(target_os = "windows")]
impl DibView {
    fn of(d: &ffi::Dib) -> Self {
        Self {
            bits: d.bits,
            w: d.w,
            h: d.h,
        }
    }
}

/// 一次切换动画所需的全部素材（**只在动画开始时算一次**）。
///
/// ⭐ 为什么缓存而**不是每帧重画两块面板**：面板内容在这 150ms 内不会变
///   （封面/歌名/电量都是慢变量），而重画一次要走字体创建 + 文本测量 +
///   逐像素合成。逐帧重画 ⇒ 9 帧 × 2 面板 = 18 次全量绘制，
///   在任务栏这种每次都可见的位置上是白烧 CPU。
///   ⇒ 每帧只做「建一张帧位图 + 两次 blit + 一次提交」。
#[cfg(target_os = "windows")]
struct AnimFrames {
    from: ffi::Dib,
    to: ffi::Dib,
    /// 提交用的父窗客户区 x —— 取**新面板**那份（动画期间窗口就是新宽度）。
    rel_x: i32,
}

// SAFETY: `Dib` 含 `*mut u32` 等裸指针 ⇒ `!Send`，而 `static Mutex<T>` 要求
//   `T: Send`。这里包一层并手写 `Send`：**这些位图只在主线程被创建、绘制与释放**
//   （GDI 与分层窗都属创建线程 = 主线程），`Mutex` 只是为了让编译器接受这个
//   static。跨线程时发生的唯一动作是「点击线程 → 动画线程 → 主线程」的
//   **所有权移交**，任何线程都不会去解引用里面的指针。
// SAFETY: `ANIM_FRAMES` 的所有读写都在主线程（`begin_switch_anim` /
//   `finish_switch_anim` 收到的都是主线程投递的消息），故实际无并发。
#[cfg(target_os = "windows")]
unsafe impl Send for AnimFrames {}

#[cfg(target_os = "windows")]
static ANIM_FRAMES: Mutex<Option<AnimFrames>> = Mutex::new(None);

/// 只画不提交地渲染指定面板（动画素材用）。
#[cfg(target_os = "windows")]
fn render_panel(
    hwnd: *mut core::ffi::c_void,
    panel: crate::config::TaskbarPanel,
) -> Option<(ffi::Dib, i32)> {
    let painted = match panel {
        crate::config::TaskbarPanel::Music => draw_music_render(hwnd, false),
        crate::config::TaskbarPanel::Devices => {
            let items = snapshot::load().unwrap_or_default();
            draw_items_render(hwnd, &items, false)
        }
    };
    match painted {
        Painted::Bitmap(d, x) => Some((d, x)),
        // 画不出来（无设备、槽位无效、DIB 失败）⇒ 不做动画，走瞬时切换
        Painted::Committed(_) => None,
    }
}

/// 启动一次面板切换动画。返回 `false` 表示**没启动**，调用方须落回瞬时切换。
///
/// ⛔ **守卫占用点固定在函数顶部**（AGENTS.md）：不能推到「起线程」那一行——
///   中间要渲染两块面板，那段时间里动画状态若处于「无守卫」，第二次点击就能
///   同时进来，两轮动画互相覆盖缓存位图 ⇒ 双重释放。
#[cfg(target_os = "windows")]
fn begin_switch_anim(
    hwnd: *mut core::ffi::c_void,
    from: crate::config::TaskbarPanel,
    to: crate::config::TaskbarPanel,
) -> bool {
    // ⚠️ 先看动画标志、再取守卫，**两道门缺一不可**：
    //   守卫由动画线程在末尾释放，而缓存位图的释放要等主线程处理
    //   `WM_APP_SWITCH_END`。两者之间有个「守卫已放、状态未清」的窗口，
    //   此时若只查守卫，新的动画会进来并**覆盖缓存**，随后到达的结束消息
    //   会把**新**缓存释放掉并清状态 ⇒ 正在播的动画凭空消失。
    if switch_anim_active() {
        return false;
    }
    let Some(guard) = crate::state::try_begin_animation_on(
        &WIDGET_ANIMATING,
        &SWITCH_ANIM_STARTED,
        SWITCH_ANIM_TIMEOUT_MS,
        "[widget] 切换",
    ) else {
        return false;
    };

    let Some((from_dib, _)) = render_panel(hwnd, from) else {
        return false; // guard 随栈帧释放
    };
    let Some((to_dib, rel_x)) = render_panel(hwnd, to) else {
        unsafe { ffi::free_dib(&from_dib) };
        return false;
    };
    {
        // ⚠️ 持锁区内**只有一次指针写入**，没有任何 GDI 调用
        let mut slot = crate::state::lock_unpoisoned(&ANIM_FRAMES);
        *slot = Some(AnimFrames {
            from: from_dib,
            to: to_dib,
            rel_x,
        });
    }
    SWITCH_ANIM_FROM_CODE.store(panel_code(from), Ordering::SeqCst);
    SWITCH_ANIM_TO_CODE.store(panel_code(to), Ordering::SeqCst);
    SWITCH_ANIM_STARTED.store(crate::state::monotonic_ms(), Ordering::SeqCst);
    append_log(&format!("[widget] 切换动画启动: {from:?} → {to:?}"));
    spawn_switch_anim(guard, hwnd as isize);
    true
}

/// 动画线程：按节拍**只投递**刷新消息，绝不碰 GDI。
///
/// ⭐ `guard` 是**必需参数**（而不是在函数体里取）：漏传即 `error[E0061]`，
///   是**类型级拦截**，比「记得在这里取守卫」的 lint 提醒可靠（AGENTS.md）。
#[cfg(target_os = "windows")]
fn spawn_switch_anim(guard: crate::state::SingleFlightGuard<'static>, hwnd: isize) {
    std::thread::spawn(move || {
        let start = crate::state::monotonic_ms();
        loop {
            if crate::state::monotonic_ms().saturating_sub(start) >= SWITCH_ANIM_MS {
                break;
            }
            // ⛔ 跨线程只投递：`UpdateLayeredWindow` / GDI 都属创建线程（主线程）
            // ⚠️ 句柄用 `isize` 传（与 `refresh_async` 同款）：`*mut c_void` 不是 `Send`，
            //   编译器会直接拒绝；句柄本身只是整数，跨线程传值安全，
            //   真正保证「只在主线程碰窗口」的是**用法**——本线程只 `PostMessageW`。
            unsafe { ffi::post_refresh(hwnd as *mut core::ffi::c_void) };
            // ⚠️ 这里**必须**用 `thread::sleep`：`tokio::time::sleep` 在没有
            //   运行时的真线程里会 panic（AGENTS.md 明确写了这条反向纪律）
            std::thread::sleep(std::time::Duration::from_millis(SWITCH_ANIM_FRAME_MS));
        }
        // 收尾也回主线程：释放缓存位图 + 清状态 + 补一帧常规重绘
        unsafe { ffi::post_switch_end(hwnd as *mut core::ffi::c_void) };
        drop(guard);
    });
}

/// **主线程**收尾：释放缓存位图 → 清状态 → 补一帧常规重绘。
#[cfg(target_os = "windows")]
fn finish_switch_anim(hwnd: *mut core::ffi::c_void) {
    // ⚠️ 先清标志再释放：反过来的话，释放途中若有重绘进来会看到
    //   「状态已清 ⇒ 走常规路径」但位图正在被 free 的窗口。
    SWITCH_ANIM_STARTED.store(0, Ordering::SeqCst);
    let taken = {
        let mut slot = crate::state::lock_unpoisoned(&ANIM_FRAMES);
        slot.take()
    };
    let from = panel_from_code(SWITCH_ANIM_FROM_CODE.swap(PANEL_CODE_NONE, Ordering::SeqCst));
    let to = panel_from_code(SWITCH_ANIM_TO_CODE.swap(PANEL_CODE_NONE, Ordering::SeqCst));
    if let Some(f) = taken {
        unsafe {
            ffi::free_dib(&f.from);
            ffi::free_dib(&f.to);
        }
    }
    if crate::config::verbose_log_enabled() {
        append_verbose_log(&format!("[widget] 切换动画结束: {from:?} → {to:?}"));
    }
    // 终态（p=1）与「新面板的常规帧」逐像素等价 ⇒ 这一帧不会「跳一下」
    repaint_from_snapshot(hwnd);
}

/// **主线程**合成一帧动画（两块缓存位图错位叠加）。
fn draw_switch_transition(hwnd: *mut core::ffi::c_void) -> bool {
    let started = SWITCH_ANIM_STARTED.load(Ordering::SeqCst);
    // 读视图带出锁外；真正的 `Dib` 仍留在锁里，由 `finish_switch_anim` 释放
    let (from, to, rel_x) = {
        let slot = crate::state::lock_unpoisoned(&ANIM_FRAMES);
        let Some(f) = slot.as_ref() else {
            return false;
        };
        (DibView::of(&f.from), DibView::of(&f.to), f.rel_x)
    };

    let slide = to.w;
    let p = switch_progress(started, crate::state::monotonic_ms(), SWITCH_ANIM_MS);
    let (from_dx, to_dx, from_mul) = switch_offsets(p, slide);
    let w = slide;
    let h = from.h.max(to.h);

    let Some(frame) = (unsafe { ffi::create_dib(w, h) }) else {
        append_log("[widget] 切换动画: 帧 DIB 创建失败");
        return false;
    };
    let ok = unsafe {
        let px = std::slice::from_raw_parts_mut(frame.bits, (w * h) as usize);
        px.fill(0);
        if from_mul > 0 {
            blit_premul_scaled(
                px,
                w,
                h,
                std::slice::from_raw_parts(from.bits, (from.w * from.h) as usize),
                from.w,
                from.h,
                from_dx,
                from_mul,
            );
        }
        // 新面板**不透明**地压在上面：它才是终态那一帧该看到的东西
        blit_premul_scaled(
            px,
            w,
            h,
            std::slice::from_raw_parts(to.bits, (to.w * to.h) as usize),
            to.w,
            to.h,
            to_dx,
            256,
        );
        let ok = ffi::commit(hwnd as _, &frame, rel_x, widget_y_offset());
        ffi::free_dib(&frame);
        ok
    };
    if crate::config::verbose_log_enabled() {
        append_verbose_log(&format!(
            "[widget] 切换动画帧: p={p:.3} 旧dx={from_dx} 强度={from_mul}/256 新dx={to_dx} 帧宽={w}"
        ));
    }
    ok
}

/// 执行一次音乐面板的点击。
#[cfg(target_os = "windows")]
pub fn activate_music(hwnd: *mut core::ffi::c_void, hit: MusicHit) {
    if switch_anim_active() {
        return;
    }
    match hit {
        MusicHit::Prev => crate::taskbar_music::cmd_previous(),
        MusicHit::PlayPause => crate::taskbar_music::cmd_play_pause(),
        MusicHit::Next => crate::taskbar_music::cmd_next(),
        MusicHit::Switch => advance_switch_target(hwnd),
        MusicHit::None => {}
    }
}

// ═══════════════════════════════════════════════════════════════════
// 音乐面板：布局、绘制、命中
// ═══════════════════════════════════════════════════════════════════

/// 音乐面板的**本帧布局**（绘制时发布，命中时读取 —— 与设备面板的
/// `LAST_ITEM_RECTS` 同一纪律：**绘制与命中必须同源**）。
///
/// ⛔ 面板内部有三类可点区域（上一首 / 播放暂停 / 下一首）+ 一个切换按钮，
///   它们**不是** `LAST_ITEM_RECTS` 的下标，而是各自的子矩形。
///   若命中时现算，就会出现「第二个布局来源」⇒ 点到别的按钮上且不报错。
#[cfg(target_os = "windows")]
#[derive(Clone, Copy, Default)]
pub struct MusicLayout {
    /// 封面 + 文本/按钮的整块区域（tooltip 命中用）。
    pub item: windows_sys::Win32::Foundation::RECT,
    /// 三个控制键：`[上一首, 播放/暂停, 下一首]`。宽度为 0 表示该键不可用。
    pub buttons: [windows_sys::Win32::Foundation::RECT; 3],
    /// 「切换」按钮（`None` = 本帧不显示）。
    pub switch_btn: Option<windows_sys::Win32::Foundation::RECT>,
    /// 本帧是否 hover（决定静态/控制形态）。
    pub hovered: bool,
}

#[cfg(target_os = "windows")]
static MUSIC_LAYOUT: std::sync::Mutex<Option<MusicLayout>> = std::sync::Mutex::new(None);

/// 读回音乐面板的布局（命中判定用）。
#[cfg(target_os = "windows")]
pub fn music_layout() -> Option<MusicLayout> {
    *crate::state::lock_unpoisoned(&MUSIC_LAYOUT)
}

#[cfg(target_os = "windows")]
fn publish_music_layout(l: MusicLayout) {
    *crate::state::lock_unpoisoned(&MUSIC_LAYOUT) = Some(l);
}

/// 音乐面板当前该显示哪一块（`None` = 音乐不可用）。
///
/// ⭐ 判据在 `config::taskbar_panel_for`（纯函数、可单测）；这里只补上
///   「音乐会话是否存在」这个**配置层问不到**的事实。
#[cfg(target_os = "windows")]
pub fn current_panel() -> Option<crate::config::TaskbarPanel> {
    let music_available = crate::taskbar_music::snapshot().available();
    crate::config::with_config(|c| crate::config::taskbar_panel_for(c, music_available))
}

/// 「切换」按钮本帧是否该显示。
///
/// ⭐ 判据（用户 2026-09-28）：**有东西可切才显示**
///   · 两个开关都开            ⇒ 能在「设备 / 音乐」之间切
///   · 音乐面板 ∧ 会话数 > 1   ⇒ 能在多个媒体会话之间切
///   · 只开一个开关且只有一个会话 ⇒ 无处可切 ⇒ **不显示**（占位也是噪音）
#[cfg(target_os = "windows")]
fn switch_visible(panel: crate::config::TaskbarPanel) -> bool {
    let both_on = crate::config::with_config(|c| {
        crate::config::taskbar_devices_available(c) && c.taskbar_music_enabled
    });
    let multi_session = matches!(panel, crate::config::TaskbarPanel::Music)
        && crate::taskbar_music::snapshot().sessions.len() > 1;
    both_on || multi_session
}

/// 设备面板的「切换」按钮矩形（`None` = 本帧不显示）。
///
/// ⛔⛔ **两个面板都必须有它**：切换按钮只画在音乐面板时，��到设备面板就是
/// **单程票**——用户再也回不去音乐面板，只能去设置页（实测 2026-09-28：
/// 修好「记住的选择」后组件确实切到了设备面板，却发现**回不来了**）。
/// 用户原话是「在**任务栏组件**的最右边」，组件指整体而非音乐面板。
#[cfg(target_os = "windows")]
static DEV_SWITCH_RECT: std::sync::Mutex<Option<windows_sys::Win32::Foundation::RECT>> =
    std::sync::Mutex::new(None);

#[cfg(target_os = "windows")]
fn publish_dev_switch(r: Option<windows_sys::Win32::Foundation::RECT>) {
    *crate::state::lock_unpoisoned(&DEV_SWITCH_RECT) = r;
}

/// 设备面板的切换按钮是否被点到（命中判定与绘制同源，见 [`publish_dev_switch`]）。
#[cfg(target_os = "windows")]
fn dev_switch_hit(local: (i32, i32)) -> bool {
    hit_rect(*crate::state::lock_unpoisoned(&DEV_SWITCH_RECT), local)
}

/// ⭐⭐ 「切换」按钮只在 **hover** 时显示（用户 2026-09-29）。
///
/// 抽成纯函数是为了让「显示」与「可点」**由同一判据驱动**——
///
/// ⛔ 两者若各写一份，迟早出现「看不见但点得到」的按钮：那比按钮常驻更糟，
///   因为用户点了会有反应，却**看不到自己点了什么**。
///   （本仓同类教训：「绘制与命中必须同源」，见 `press_target_at` 的注释。）
///
/// **宽度不跟着变**（用户 2026-09-28 明确要求 hover 前后长度一致）：
/// `switch_w` 恒计入 `content_w`，未 hover 时那块是**全透明**的
///（分层窗按像素 alpha）⇒ 看不见空洞，而 `want_hover` 用**窗口矩形**判
/// 光标在内 ⇒ 那块 45px 正是 hover 区的一部分 ⇒ 鼠标移过去按钮就浮现，
/// **没有「必须先 hover 到别处才冒出来」的死区**。
#[cfg(target_os = "windows")]
fn switch_icon_alpha(hovered: bool) -> f32 {
    if hovered {
        1.0
    } else {
        0.0
    }
}

/// 切换按钮此刻**是否可点**（= 是否显示）。
#[cfg(target_os = "windows")]
fn switch_clickable(hovered: bool) -> bool {
    switch_icon_alpha(hovered) > 0.0
}

/// ⭐⭐ 音乐面板文字的**绘制框**：返回 `(绘制宽度, 是否需要省略号)`。
///
/// ⛔⛔ **上限必须与布局侧是同一个值**（2026-09-29 修「文字被截断、右侧却留大片空白」）。
///
///   当时**只改了布局侧**：面板宽度按音乐面板自己的 `MUSIC_TEXT_MAX_W_DIP`
///   （320 DIP ⇒ 125% 下 400px）算，而**绘制侧仍在用 `m.item_max_w`**
///   （150 DIP ⇒ 125% 下 188px —— 那个字段的注释里明写是
///   「**给设备面板的**电量 / 音量那几行短数字设计的」）
///   ⇒ 面板画到 400px 宽、文字只画到 188px 就打「…」
///   ⇒ 右侧约 212px 空白（用户 2026-09-29 实测报出：hover 时能明显看到后面空着）。
///
///   ⇒ 抽成本函数，**布局与绘制都从这里取**，杜绝「两份上限」再次分叉
///   （同源纪律，与「绘制与测宽必须同源」同源）。
#[cfg(target_os = "windows")]
fn music_text_box(natural: i32, cap: i32) -> (i32, bool) {
    let w = natural.min(cap).max(0);
    (w, natural > w)
}

/// 点是否落在矩形内。
#[cfg(target_os = "windows")]
fn hit_rect(r: Option<windows_sys::Win32::Foundation::RECT>, local: (i32, i32)) -> bool {
    let Some(r) = r else { return false };
    local.0 >= r.left && local.0 < r.right && local.1 >= r.top && local.1 < r.bottom
}

/// 音乐面板正文区（文字 / 三键）的左缘 —— **静态与 hover 共用这一个入口**。
///
/// ⛔ 分成两处写就是「切形态时横向跳」那类闪的来源：两处一旦漂移，肉眼只看到
///   「动一下指针内容就错位」，几乎无法定位。
///
/// ⭐⭐ **`cover_gap` 由调用方传进来，不在这里重算**（2026-09-30）。
/// 本函数原先自己算了一遍 `m.dip(MUSIC_COVER_GAP_DIP)`，而 `draw_music_render`
/// 里也有一份 `cover_gap` ⇒ **同一口径写了两遍**。
/// ⇒ 一旦「封面↔文字间距」的口径变了（用户 2026-09-30 就在反复调这一项），
///   两处很容易只改一处 ⇒ **正文起点与封面右缘错位**，表现为
///   「封面到双排信息的距离变了」（用户实测报的就是这个）。
/// ⇒ 现在**只有 `draw_music_render` 那一处**算 `cover_gap`，本函数只做加法。
///
/// ⚠️ `pad_x` / `icon` 传的是**封面那一侧**的度量（当前是固定档 `m_art`）：
///   两者必须**同源**，否则正文起点会跟着设置漂。
#[cfg(target_os = "windows")]
fn music_body_x(pad_x: i32, icon: i32, cover_gap: i32) -> i32 {
    pad_x + icon + cover_gap
}

/// 把一个**控件**在 `h` 高的底衬里**按自己的尺寸**垂直居中（**纯函数**，可单测）。
///
/// ⭐⭐ 音乐面板的三个控件（封面 / 三键 / 切换键）**必须各走本函数、各按自己的尺寸**
///   —— **不许借别人的坐标**。
///
/// ⚠️⚠️ 借坐标只在**尺寸相等**时恰好居中：都从 `y` 起画时，`[y, y + size]`
///   的中心才是 `h / 2` —— 而这要求所有控件的 `size` 与 `y` 同源同值。
///   用户 2026-09-30 要求「缩放设置只管三键与切换键、封面恒定」
///   ⇒ 封面与三键**不再相等** ⇒ 复用 `icon_y` 会让三键**偏上**：
///   125% 下 `h=50`、封面 40、三键 32 ⇒ 封面 `[5,45]` 中心 25，
///   三键 `[5,37]` 中心 21 ⇒ **偏上 4px**（用户实测报「缩小的三键没垂直居中」）。
///
/// ⛔ 这正是「同一口径写了两份」的变种：此处若各写各的 `(h - size) / 2`，
///   改一处就会重新漂。
#[cfg(target_os = "windows")]
fn center_y(h: i32, size: i32) -> i32 {
    (h - size) / 2
}

/// 音乐面板绘制（**主线程**）。
///
/// 形态（用户 2026-09-28 指定）：
/// · 静态 = 左边封面 + 上排标题 / 下排艺人
/// · hover = 封面 + 三键（上一首 / 播放暂停 / 下一首）
/// · 「切换」按钮恒在**最右侧**（仅在 `switch_visible` 时）
///
/// ⭐ 宽度**随形态变化**（用户选择）：hover 时变宽、离开时变窄。
///   已核对不会自激：`want_hover` 用实际窗口矩形判「光标在内」，
///   而**宽窗口包含窄窗口** ⇒ 变宽后光标必仍在内 ⇒ 不会「变宽→出界→变窄」的振荡。
#[cfg(target_os = "windows")]
pub fn draw_music(hwnd: *mut core::ffi::c_void) -> bool {
    draw_music_render(hwnd, true).committed_ok()
}

/// [`draw_music`] 的本体。`publish = false` 时**只画不提交**：
/// 面板切换动画要把**旧面板**也画一份留作滑走的起点，而那块位图
/// **绝不能提交**（一提交就等于「面板瞬间换掉了」，动画等于没播）。
///
/// ⚠️ 此时还必须跳过 `publish_music_layout` / `publish_item_rects`：
///   发布的是**动画中间态**的坐标 ⇒ 命中测试会照着移动中的矩形判定，
///   用户的点击会落在「已经滑走」的位置上。
#[cfg(target_os = "windows")]
fn draw_music_render(hwnd: *mut core::ffi::c_void, publish: bool) -> Painted {
    let dark = crate::windows::system_dark_mode();
    let m = Metrics::current_content();
    // ⭐ 封面 + 双排信息**恒定**（用户 2026-09-30：缩放设置对它们不生效）；
    //   三键 / 切换键 / 各项间距 / 左右留白仍跟随 `m`（同轮逐条确认「都按 A」）。
    let m_art = Metrics::art_fixed();
    let snap = crate::taskbar_music::snapshot();
    let hovered = HOVERED.load(Ordering::Acquire);
    let cur = snap.current_session().cloned().unwrap_or_default();

    // ── 建字体 + 测量（与设备面板同一套度量/测量入口）────────────────
    let (font, memdc) = unsafe {
        // ⚠️ **不加粗**（用户 2026-09-29）。设备面板的「电量 / 音量」曾按 2026-09-25
        //   的要求加粗，同日用户又要求设备面板一并取消 ⇒ 现在两侧同为常规字重；
        //   但歌名 / 歌手是**连续文字**，加粗后 Segoe UI Variable Text 在 15px 下
        //   笔画粘连、字腔变窄，观感偏「糊成一团」。
        //   ⭐ 顺带与 **tooltip 对齐**了：tooltip 走 `create_font_cleartype(.., false)`
        //   本来就是常规字重，之前面板加粗 / tooltip 常规 ⇒ 同一首歌在两处粗细不一。
        // ⛔ 字号取 `m_art`（固定档）：双排信息不随缩放设置变
        let f = ffi::create_font(m_art.font, false);
        if f.is_null() {
            return Painted::Committed(draw_blank(hwnd, m_art.pad_x * 2));
        }
        let screen = windows_sys::Win32::Graphics::Gdi::GetDC(std::ptr::null_mut());
        let dc = windows_sys::Win32::Graphics::Gdi::CreateCompatibleDC(screen);
        windows_sys::Win32::Graphics::Gdi::ReleaseDC(std::ptr::null_mut(), screen);
        // ⚠️ **必须与 `draw_items_render` 同款判据**（2026-09-29 补齐）。
        //   缺了它**不会闪退**（实测：NULL HDC 下 `measure_text` 只是**静默返回 0**，
        //   判据 `measure_text_with_null_dc_does_not_crash_but_returns_garbage`），
        //   但「文字宽度 = 0」会让 `body_w` 算窄 ⇒ **面板过窄、文本被裁切**，
        //   且**零日志** —— 与「静默丢数据」同类的哑故障。
        //   两条路径（设备 / 音乐）此前判据不一致，属于典型的分叉点。
        if dc.is_null() {
            windows_sys::Win32::Graphics::Gdi::DeleteObject(f);
            return Painted::Committed(draw_blank(hwnd, m_art.pad_x * 2));
        }
        (f, dc)
    };
    if memdc.is_null() {
        unsafe { ffi::destroy_font(font) };
        return Painted::Committed(draw_blank(hwnd, m_art.pad_x * 2));
    }

    let title_wide: Vec<u16> = if cur.title.is_empty() {
        "未在播放".encode_utf16().collect()
    } else {
        cur.title.encode_utf16().collect()
    };
    let artist_wide: Vec<u16> = cur.artist.encode_utf16().collect();
    // ⚠️ **标题有「未在播放」兜底、艺人没有** —— 艺人可能是空串（实测某播放器上报
    //   `artist = ""`），这是 2026-09-29 闪退的根因所在（见 `measure_text` 的注释）。
    let (t_nat, a_nat) = unsafe {
        (
            ffi::measure_text(memdc, font, &title_wide),
            ffi::measure_text(memdc, font, &artist_wide),
        )
    };
    unsafe { windows_sys::Win32::Graphics::Gdi::DeleteDC(memdc) };

    // ⭐ **两个尺寸是分别定的，别再让它们相等**（用户 2026-09-28 实测报「切换太大、三键太小」）：
    //   · 三键占**满高度**（`m.h`）—— 它们是「有边框 + 内容」的线稿图标，32px 下边框只有
    //     1.6px、暂停双竖条只有 25.6/1024 ≈ **0.8px** ⇒ 几乎看不见。占满高度后边框 2px、
    //     竖条 1px，才读得出是三个键。控制行占满高也是常规做法。
    //   · 「切换」图标**保持原尺寸**（= `m.icon`，与控制键同一边长）——
    //     用户 2026-09-28 复核后要求改回。三键占满高度是它**自身线稿太细**所致
    //     （内部元素 25.6/1024 ⇒ 32px 下 0.8px），与切换图标无关，两者不必一起改。
    let switch_px = m.icon;
    // ⭐ **所有间隙一律 = `m.icon_text_gap`**（用户 2026-09-29：「缩短三键的间距，
    //   改为和封面到上一首按钮的间距一致」）。
    //   ⛔ 末段（下一首→切换）的**基准值**也用它：否则最短宽度下会出现
    //   「5 / 5 / 5 / 13」这种一眼可见的不齐（撑长的余量仍然只加在末段）。
    //   原先用 `m.item_gap`（10 DIP）⇒ 125% 下 13px，比 5px 宽一倍多。
    // ⛔ 固定（用户 2026-09-30 选 BBB）：按键间距不随缩放设置变
    let gap = m_art.icon_text_gap;

    // ⭐ **封面右侧的间隙另算，比按键之间宽**（用户 2026-09-29：「把封面到上一首
    //   按钮和到两排文字的距离同时增加一些」）。
    //   · `gap`       = 按键↔按键、末段基准（保持 5px 不变）
    //   · `cover_gap` = 封面↔文字（静态）/ 封面↔第一键（hover）
    //   两种形态的正文都从封面右缘起算 ⇒ **必须同值**，否则切形态时文字/按键
    //   会横向跳一下。
    //   ⛔ 不复用 `m.icon_text_gap`：那是**设备面板**共用的（封面↔电量文字），
    //     改它会连带把设备面板间距也改掉 ⇒ 音乐面板单开一个 DIP。
    // ⛔ 固定（BBB）：封面↔文字 / 封面↔第一键 的间距不随设置变
    let cover_gap = m_art.dip(MUSIC_COVER_GAP_DIP);
    // ⛔⛔ 文字宽度**上限必须大于「固定段」**，否则「撑长」永远看不到
    //   （用户 2026-09-29 实测报「没变化啊」）。
    //   根因：原先复用了 `m.item_max_w`（= 150 DIP，**给设备面板的电量/音量
    //   那几行短数字设计的**）⇒ 125% 下上限 188px，而固定段（三键 3×50 + 间隙
    //   2×13）已有 176px ⇒ **最多只能撑 12px**，肉眼根本看不出来。
    //   ⇒ 音乐面板用自己的上限 `MUSIC_TEXT_MAX_W_DIP`，并强制它**大于固定段**。
    const MUSIC_TEXT_MAX_W_DIP: i32 = 320; // 125% 下 400px，够长的歌名也能撑开
                                           // ⚠️ 上限也用 `m_art`：否则设置变小 ⇒ 上限变小 ⇒ **歌名被更早截断**，
                                           //   那等于设置仍在影响双排信息（与本条要求矛盾）。见 `art_fixed` 的说明。
    let text_cap = m_art.dip(MUSIC_TEXT_MAX_W_DIP);
    // ⚠️ 用 `btn`（= `m.icon`）而不是 `m.h`：`m.h`(50) > `btn`(40) 会让这个
    //   「固定段」估算偏大 ⇒ `text_cap` 与下面那条 `debug_assert` 都跟着放宽，
    //   断言因此**比预期弱**（本批顺手改正，125% 下不影响最终值）。
    let strip_w_guess = m.icon * 3 + gap * 2;
    let text_cap = text_cap.max(strip_w_guess + m.h * 2);
    let text_w = t_nat.min(text_cap).max(a_nat.min(text_cap));

    // ⭐⭐ **组件宽度恒定：正文区宽度只按静态形态定一次，两种形态共用**
    //   （用户 2026-09-28：「以当前非 hover 时的长度为准，让 hover 时的长度固定一致」）。
    //   此前 hover 用 `btn * 3 + gap * 2`（50×3+13×2 = 176）而静态是 164
    //   ⇒ 指针一进组件，窗口宽度从 262 跳到 ~300，**整个面板左右窜动**。
    //   现在 `body_w` 与 hovered **无关** ⇒ 两种形态的 `content_w` 逐字节相同。
    //   ⚠️ 代价（可接受、且是这条要求的直接推论）：三键的边长不再恒为 `m.h`，
    //   而要**在同一条带内均分**（下式）。`min(.., m.h)` 保证标题很长时也不会
    //   超过控件高度。
    // ⭐⭐ **封面 ↔ 三键的间距恒定**（用户 2026-09-29 明确要求）：
    //   「封面-音乐控制3键之间固定间距，固定后的音乐组件总宽度就是最短宽度，
    //     当音乐信息长度超过这个长度后则撑长音乐组件长度，
    //     但仍不改变封面-音乐控制3键之间的间距，
    //     只改变下一首按钮到切换按钮之间的间距」
    //
    // ⇒ 三键**固定边长**（不再随歌名均分），撑长出来的余量**全部**落在
    //   「下一首 → 切换」那一段；静态形态则由文字吃掉同一段余量。
    // ⇒ `body_w` 取 `max(文字宽, 固定段宽)`：文字短于固定段时组件就是**最短宽度**。
    //
    // ⚠️ 顺带修掉一个一直存在的重复计数：旧式把 `m.icon + m.icon_text_gap`
    //   **又加了一遍**（`body_w` 本该只是正文区），导致文字与切换键之间恒定多出
    //   一段死空白。现在这段空白变成了规格里那个「可变间距」。
    // ⭐ 固定边长 = **封面边长**（`m.icon`），不放大（用户 2026-09-29：「不要让三键变大」）。
    //
    // ⚠️⚠️ 格子与图标尺寸必须**一致**（这一条踩过两次）：
    //   · 格子 `m.h`(50) 而图标按 `m.icon`(40) 画 ⇒ 每格四周空 5px，
    //     「两键之间」的**视觉**间隙 = 5+5+5+5 = 15px，而「封面↔第一键」只有
    //     5+5 = 10px ⇒ 代码同一口径、看上去却三键更散（用户报「缩短三键之间的间距」）。
    //   · 反过来把图标放大到填满 50px 格子 ⇒ 间隙对了，但按键**变大了**（用户不要）。
    //   ⇒ 正解：**格子也用 `m.icon`**，图标填满格子，视觉间隙 == `gap` 本身，
    //     而按键尺寸保持不变。
    let btn = m.icon;
    let strip_w = btn * 3 + gap * 2; // 固定段（hover 形态下三键占的宽度）
    debug_assert!(text_cap > strip_w, "文字上限必须大于固定段，否则撑长不可见");
    let body_w = text_w.max(strip_w);
    let show_switch = switch_visible(crate::config::TaskbarPanel::Music);
    let switch_w = if show_switch { switch_px + gap } else { 0 };
    // ⭐ 首项是**封面**（固定）；间距 / 正文 / 切换键仍跟随
    let content_w = m_art.icon + cover_gap + body_w + switch_w;
    let desired_w = content_w + m_art.pad_x * 2;
    if !SLOT_VALID.load(Ordering::Acquire) {
        unsafe {
            ffi::hide(hwnd as _);
            ffi::destroy_font(font)
        };
        return Painted::Committed(true);
    }
    let (position, locked, custom_x) = crate::config::with_config(|c| {
        (
            c.taskbar_position.clone(),
            c.taskbar_position_locked,
            c.taskbar_custom_x,
        )
    });
    let (tb_left, tb_w) = match taskbar_rect() {
        Some((left, _, w, _)) => (left, w),
        None => (0, i32::MAX),
    };
    let slot_rel_x = SLOT_X.load(Ordering::Acquire) - tb_left;
    let slot_w = SLOT_W.load(Ordering::Acquire);
    let total_w = desired_w.min(tb_w.max(min_run_w(&m)));
    // ⛔ 固定（BBB）：否则改设置会**移动整个面板**在任务栏上的落点
    let edge_margin = m_art.dip(EDGE_MARGIN_DIP);
    let aligned = align_in_slot(slot_rel_x, slot_w, total_w, &position, edge_margin);
    let rel_x = resolve_rel_x(
        locked,
        aligned,
        custom_x,
        LAST_X.load(Ordering::Acquire),
        total_w,
        tb_w,
    );
    LAST_X.store(rel_x, Ordering::Release);
    // ⭐ 与设备面板**同款定位日志**（详细级）：本模块最容易「看起来正常但位置不对」，
    //   音乐面板刚引入时缺这条日志 ⇒ 出现「面板跑到别处」时无从判断是贴靠算错
    //   还是根本没走定位。**两个面板的诊断口径必须对称**。
    if crate::config::verbose_log_enabled() {
        append_log(&format!(
            "[widget] 定位: pos={position} locked={locked} area=({slot_rel_x},w={slot_w})              content_w={total_w} 余量={} → rel_x={rel_x} panel=Music hovered={hovered}              键={btn}px 切换={switch_px}px              文字={text_w} 固定段={strip_w} 正文={body_w} 末段={}",
            slot_w - total_w,
            // ⭐ 末段间距（下一首 → 切换）：撑长时**只有它**会变宽
            body_w - strip_w + gap,
        ));
    }
    unsafe { ffi::show(hwnd as _) };

    let h = m.h;
    // ⭐ 三个控件**各按自己的尺寸**居中，一律走 `center_y`（见其文档：
    //   借别人的坐标只在尺寸相等时恰好居中，而封面与三键已不再相等）
    let icon_y = center_y(h, m_art.icon); // 封面：固定边长
                                          // ⭐⭐ 三键**按自己的尺寸**居中，**不**复用封面的 `icon_y`。
                                          //
                                          // ⚠️⚠️ 复用 `icon_y` 只有在 `btn == 封面边长` 时才恰好居中（两者都从
                                          //   `icon_y` 起画，`[icon_y, icon_y + size]` 的中心才是 `h/2`）。
                                          //   而用户 2026-09-30 要求「缩放设置只管三键与切换键」、封面恒定
                                          //   ⇒ 两者**不再相等** ⇒ 复用会让三键整体偏上：
                                          //   125% 下 h=50、封面 40、三键 32 ⇒ 封面 [5,45] 中心 25，
                                          //   三键 [5,37] 中心 21 ⇒ **偏上 4px**（用户实测报「缩小的三键没垂直居中」）。
                                          //
                                          //   ⭐ 与「切换」键同款做法（`let sy = (h - switch_px) / 2`）：
                                          //   **每个控件按自己的尺寸居中**，不借封面的位置。
    let btn_y = center_y(h, btn); // 三键：跟随缩放设置
    let Some(dib) = (unsafe { ffi::create_dib(total_w, h) }) else {
        append_log("[widget] 音乐面板 CreateDIBSection 失败");
        unsafe { ffi::destroy_font(font) };
        return Painted::Committed(false);
    };

    // ── 布局一次算清并发布（绘制与命中共用这一份）───────────────────
    let mut item_rect = windows_sys::Win32::Foundation::RECT {
        left: m_art.pad_x,
        top: 0,
        right: m_art.pad_x + m_art.icon + cover_gap + body_w,
        bottom: h,
    };
    let mut buttons = [windows_sys::Win32::Foundation::RECT {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    }; 3];
    let mut switch_btn = None;
    if hovered {
        // ⭐ 三键**顶格左对齐**排布：余量不摊给它们，而是全留给「下一首 → 切换」那一段
        //   （见上面 `body_w` 那段规格）。**不能居中**——居中会让封面到第一个键的
        //   距离随歌名变化，正是用户要求恒定的那一段。
        let mut x = music_body_x(m_art.pad_x, m_art.icon, cover_gap);
        for slot in buttons.iter_mut() {
            *slot = windows_sys::Win32::Foundation::RECT {
                left: x,
                top: btn_y,
                right: x + btn,
                bottom: btn_y + btn,
            };
            x += btn + gap;
        }
    }
    if show_switch {
        let x = item_rect.right + gap;
        // ⭐ 命中矩形用**切换图标自己的边长**（比三键小）⇒ 点空白不会误触发「切换」。
        //   垂直居中：图标比三键矮，要在三键的行内居中才视觉对齐。
        let sy = center_y(h, switch_px); // 切换键：跟随缩放设置
        switch_btn = Some(windows_sys::Win32::Foundation::RECT {
            left: x,
            top: sy,
            right: x + switch_px,
            bottom: sy + switch_px,
        });
        item_rect.right = x + switch_px;
    }
    let tip_text = if cur.title.is_empty() {
        "未在播放".to_string()
    } else {
        format!("{}\n{}", cur.title, cur.artist)
    };
    if publish {
        publish_music_layout(MusicLayout {
            item: item_rect,
            buttons,
            switch_btn,
            hovered,
        });
    }
    // ⭐ **同时发布 `LAST_ITEM_RECTS`**：音乐面板也必须走**同一条** hover 轮询与
    //   tooltip 锚点路径（`hovered_item_index` / `item_rect_on_screen` 都读它）。
    //   不发布 ⇒ hover 永远判不出「落在第几项」⇒ 提示不出现、且没有任何报错
    //   （这正是 `LAST_ITEM_RECTS` 当初被立为「四路同源单一来源」的原因）。
    //   下标语义：`[0]` = 面板主体，`[1]` = 切换按钮（若有）。
    if publish {
        let mut rects = vec![item_rect];
        if let Some(sb) = switch_btn {
            rects.push(sb);
        }
        publish_item_rects(&rects);
    }

    unsafe {
        let px = std::slice::from_raw_parts_mut(dib.bits, (total_w * h) as usize);
        px.fill(0);
        if hovered {
            let alpha = hover_backdrop_alpha_for(crate::windows::system_uses_light_theme());
            fill_hover_backdrop(px, total_w, h, alpha, m.radius);
        }
        let (cr, cg, cb): (u8, u8, u8) = if dark { (255, 255, 255) } else { (0, 0, 0) };

        // ① 封面（已解码 + 已预乘，直接 source-over）
        if let Some(cov) = crate::taskbar_music::cover(snap.cover_hash) {
            // ⛔ 走**预乘直通**：封面缓存里存的就是预乘数据（见 taskbar_music::COVER_IMAGE）
            if let Some((cpx, cw, chh)) = resample::scale_cached_premul(
                resample::NS_COVER | (snap.cover_hash as u32),
                m_art.icon as u32, // 封面边长固定
                &(cov.data.clone(), cov.px, cov.px),
            ) {
                for yy in 0..(chh as i32).min(h - icon_y) {
                    for xx in 0..(cw as i32).min(total_w - item_rect.left) {
                        let si = ((yy * cw as i32 + xx) * 4) as usize;
                        let a = cpx[si + 3] as u32;
                        if a == 0 {
                            continue;
                        }
                        let di = ((icon_y + yy) * total_w + item_rect.left + xx) as usize;
                        if di < px.len() {
                            px[di] = blend_over(
                                px[di],
                                a,
                                cpx[si] as u32,
                                cpx[si + 1] as u32,
                                cpx[si + 2] as u32,
                            );
                        }
                    }
                }
            }
        }

        let body_x = music_body_x(m_art.pad_x, m_art.icon, cover_gap);
        if hovered {
            // ② 三键：不可用的键画**半透明**（与 FluentFlyout 一致：不隐藏、只置灰）
            let slots = [
                music_icons::Icon::Prev,
                if cur.playing {
                    music_icons::Icon::Pause
                } else {
                    music_icons::Icon::Play
                },
                music_icons::Icon::Next,
            ];
            for (i, slot) in buttons.iter().enumerate() {
                if slot.right <= slot.left {
                    continue;
                }
                let enabled = match i {
                    0 => cur.can_prev,
                    1 => cur.can_play_pause,
                    _ => cur.can_next,
                };
                // ⭐ 「按下变灰」（用户 2026-09-29）：不可用档 0.5，按下再乘 0.55。
                //   两个维度**相乘**而不是二选一 —— 不可用的键同时按下时仍然更暗，
                //   不会出现「按下反而变亮」。
                let scale = if enabled { 1.0f32 } else { 0.5f32 }
                    * if pressed_id() == music_btn_id(i) {
                        0.55f32
                    } else {
                        1.0f32
                    };
                // ⭐⭐ 图标必须按**格子尺寸 `btn`** 画，不能按 `m.icon`。
                //   两者不等时每格四周会空出 `(btn − m.icon)/2`（125% 下 5px），
                //   于是「两键之间的**视觉**间隙」= 内部留白×2 + 基准间隙
                //   = 5+5+5+5 = **20px**，而「封面↔第一键」只有 5+5 = 10px
                //   ⇒ 明明代码里两处用的是同一个 `gap`，看上去三键却明显更散
                //   （用户 2026-09-29 报「缩短三键之间的间距」）。
                //   按 `btn` 画满格子后，视觉间隙 == `gap` 本身。
                if let Some(scaled) = music_icons::get(slots[i], dark)
                    .and_then(|r| music_icons::scale_to_slot(slots[i], r, btn as u32))
                {
                    let (ipx, iw, ih) = scaled;
                    for yy in 0..(ih as i32).min(h - slot.top) {
                        for xx in 0..(iw as i32).min(total_w - slot.left) {
                            let si = ((yy * iw as i32 + xx) * 4) as usize;
                            let a = (ipx[si + 3] as f32 * scale).round().clamp(0.0, 255.0) as u32;
                            if a == 0 {
                                continue;
                            }
                            let di = ((slot.top + yy) * total_w + slot.left + xx) as usize;
                            if di < px.len() {
                                px[di] = blend_over(
                                    px[di],
                                    a,
                                    ipx[si] as u32 * a / 255,
                                    ipx[si + 1] as u32 * a / 255,
                                    ipx[si + 2] as u32 * a / 255,
                                );
                            }
                        }
                    }
                }
            }
        } else {
            // ③ 静态：上排标题 / 下排艺人（两行，与设备面板的电量/音量同款行高）
            //
            // ⛔⛔ 上限用 **`text_cap`**（音乐面板自己的），**不是** `m.item_max_w`
            //   （那是设备面板的 150 DIP）。用错的后果：面板按 400px 画、
            //   文字按 188px 画 ⇒ 右侧 212px 空白 + 无谓的「…」（2026-09-29 实测）。
            let (t_clamped, t_ellipsis) = music_text_box(t_nat, text_cap);
            let (a_clamped, a_ellipsis) = music_text_box(a_nat, text_cap);
            if t_clamped > 0 {
                if let Some(mask) = ffi::render_text_mask(
                    t_clamped,
                    m_art.text_row_h,
                    font,
                    &title_wide,
                    t_ellipsis,
                ) {
                    blit_text_mask(px, total_w, &mask, body_x, icon_y, (cr, cg, cb), 1.0);
                }
            }
            if a_clamped > 0 {
                if let Some(mask) = ffi::render_text_mask(
                    a_clamped,
                    m_art.text_row_h,
                    font,
                    &artist_wide,
                    a_ellipsis,
                ) {
                    blit_text_mask(
                        px,
                        total_w,
                        &mask,
                        body_x,
                        icon_y + m_art.text_row_h, // 第二行随**字号**档，不随设置
                        (cr, cg, cb),
                        0.5,
                    );
                }
            }
        }

        // ④ 「切换」按钮恒在最右
        if let Some(sb) = switch_btn {
            if let Some(scaled) = music_icons::get(music_icons::Icon::Switch, dark).and_then(|r| {
                music_icons::scale_to_slot(music_icons::Icon::Switch, r, switch_px as u32)
            }) {
                let (ipx, iw, ih) = scaled;
                for yy in 0..(ih as i32).min(h - sb.top) {
                    for xx in 0..(iw as i32).min(total_w - sb.left) {
                        let si = ((yy * iw as i32 + xx) * 4) as usize;
                        // ⭐ 按下变灰（用户 2026-09-29）
                        let press = if pressed_id() == PRESS_MUSIC_SWITCH {
                            0.55f32
                        } else {
                            1.0f32
                        };
                        // ⭐⭐ 未 hover ⇒ alpha 归零 ⇒ **切换按钮不显示**
                        //   （用户 2026-09-29：「要求切换按钮在 hover 时才显示」）
                        let a = (ipx[si + 3] as f32 * press * switch_icon_alpha(hovered)).round()
                            as u32;
                        if a == 0 {
                            continue;
                        }
                        let di = ((sb.top + yy) * total_w + sb.left + xx) as usize;
                        if di < px.len() {
                            px[di] = blend_over(
                                px[di],
                                a,
                                ipx[si] as u32 * a / 255,
                                ipx[si + 1] as u32 * a / 255,
                                ipx[si + 2] as u32 * a / 255,
                            );
                        }
                    }
                }
            }
        }

        // ⑤ tooltip 条目（音乐面板只有一条 + 切换按钮一条）
        let mut entries: Vec<crate::taskbar_tooltip::TipEntry> =
            vec![crate::taskbar_tooltip::TipEntry {
                text: tip_text,
                rect: item_rect,
            }];
        if let Some(sb) = switch_btn {
            entries.push(crate::taskbar_tooltip::TipEntry {
                text: "切换".to_string(),
                rect: sb,
            });
        }
        crate::taskbar_tooltip::sync(hwnd as _, &entries);
    }

    // ⚠️ 字体必须在这里销毁：动画路径（`publish = false`）不走下面的提交/释放，
    //   每帧漏一个 `destroy_font` 就会把 GDI 字体对象泄漏光。
    unsafe { ffi::destroy_font(font) };
    if !publish {
        return Painted::Bitmap(dib, rel_x);
    }
    let ok = unsafe { ffi::commit(hwnd, &dib, rel_x, widget_y_offset()) };
    unsafe { ffi::free_dib(&dib) };
    Painted::Committed(ok)
}

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
// ⭐ 12 DIP（用户 2026-09-29：「字体看起来很小」；原 11）。
//   仍放得下：两行各占 `text_row_h = m.icon / 2`（125% 下 20px），
//   12 DIP × 1.25 = 15px < 20px ✔（「偏小」档 12px < 16px ✔）。
const FONT_PX_DIP: i32 = 12;

/// 图标边长（DIP）。图标源 PNG 本身就是 32×32 ⇒ 100% 缩放下是**恒等拷贝**。
#[cfg(target_os = "windows")]
const ICON_PX_DIP: i32 = 32;

/// 图标与右侧两行文本之间的间隔（DIP）。
#[cfg(target_os = "windows")]
const ICON_TEXT_GAP_DIP: i32 = 4;

/// 设备之间的水平间隔（DIP）。
#[cfg(target_os = "windows")]
const ITEM_GAP_DIP: i32 = 10;

/// 音乐面板：**封面右缘 → 正文**（文字 / 三键）的间隙（DIP）。
///
/// ⭐ 独立于设备面板的 `ICON_TEXT_GAP_DIP`：那个是「封面↔电量/音量文字」共用的，
///   改它会连带改到设备面板（用户 2026-09-29 只要求改音乐面板）。
/// ⛔ 静态形态（文字）与 hover 形态（三键）**必须同值**，否则来回移指针时
///   内容会横向跳一下。
const MUSIC_COVER_GAP_DIP: i32 = 8;

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
    /// **底衬**口径的 DPI（96 = 100%），恒为**系统/任务栏 DPI**。
    ///
    /// ⛔ 用户 2026-09-25 明确要求「底衬仍跟随系统缩放，不跟随『任务栏内容缩放大小』
    ///   设置改变」⇒ `h` / `radius` **只**由它换算，`taskbar_content_scale` 不得影响。
    pub dpi: u32,
    /// **内容**口径的 DPI。`icon` / `font` / `pad_x` / `icon_text_gap` / `item_gap` /
    /// `item_max_w` / `text_row_h` 由它换算，也由 `dip()` 消费。
    ///
    /// ⚠️ 由 `taskbar_content_scale` 决定：`Default` ⇒ 等于 `dpi`；
    ///   `Default` ⇒ 恒 **96**（不随系统放大）。解析见 `for_scales`。
    pub content_dpi: u32,
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
    /// 按给定 DPI 换算，**底衬与内容同口径**（= `Default` 档，也是本设置引入前的行为）。
    ///
    /// ⚠️ 保留这个签名是给**只消费底衬量**的调用点用的（`create_popup` /
    ///   `widget_y_offset` / `draw_blank` / `draw_frame`）：它们只用 `h` / `radius`，
    ///   与内容档位无关 ⇒ 不必为它们引入配置读取。需要内容口径的入口见 `current_content`。
    pub fn for_dpi(dpi: u32) -> Self {
        Self::for_dpis(dpi, dpi)
    }

    /// 按「底衬 DPI + 内容缩放档位」换算（**纯函数**，可单测）。
    ///
    /// ⛔ 底衬量（`h` / `radius`）**恒**走 `backdrop_dpi`；内容量走按档位解析出的内容 DPI。
    ///   `Default` 档下内容恒按 96 ⇒ 系统 125% 时图标 32 / 字号 11，而底衬仍是 50 / 圆角 8。
    pub fn for_scales(
        backdrop_dpi: u32,
        content_scale: crate::config::TaskbarContentScale,
    ) -> Self {
        let bd = if backdrop_dpi == 0 { 96 } else { backdrop_dpi };
        let cd = match content_scale {
            // 「默认」= 与底衬同口径（系统 DPI）
            crate::config::TaskbarContentScale::Default => bd,
            // 「偏小」= 比系统**低一档**
            crate::config::TaskbarContentScale::Smaller => Self::step_down_dpi(bd),
        };
        Self::for_dpis(bd, cd)
    }

    /// 「偏小」档的内容 DPI = **比系统低一档**（用户 2026-09-29 定义：
    /// 「把内容的大小降一档，例如当前系统缩放是 125%，则使用 100% 的缩放」）。
    ///
    /// ⭐ 走 **Windows 标准缩放阶梯**（100/125/150/175/200/225/250/300/350/400/
    ///   500/600/800/1000）取**上一档**，而不是「乘 0.75」——
    ///   后者在 125% 上会得到 93.75%（不是任何一档），而用户要的是**100%**。
    ///
    /// ⛔ **已在最小档（100%）时保持 100%，不再往下** —— 再降就是 75%，
    ///   那会让内容小于设计基准（图标 24px），与「偏小」的字面含义不符。
    /// ⛔ 不在阶梯上的 DPI（自定义缩放）⇒ 退回「乘 3/4 后不低于 96」。
    pub fn step_down_dpi(dpi: u32) -> u32 {
        const LADDER: [u32; 14] = [
            96, 120, 144, 168, 192, 216, 240, 288, 336, 384, 480, 576, 768, 960,
        ];
        let d = if dpi == 0 { 96 } else { dpi };
        // 找阶梯里 <= d 的最大下标
        let mut idx = 0usize;
        for (i, v) in LADDER.iter().enumerate() {
            if *v <= d {
                idx = i;
            } else {
                break;
            }
        }
        if idx == 0 {
            return LADDER[0]; // 已在 100%，不再降
        }
        if LADDER[idx] == d {
            LADDER[idx - 1]
        } else {
            // 自定义缩放：按比例降一档，但不低于 100%
            ((d as f64 * 0.75).round() as u32).max(LADDER[0])
        }
    }

    /// 真正的换算实现：**两条 DPI 各自成组**，字段归属见结构体文档。
    ///
    /// ⛔ 拆成两个闭包（`b` 底衬 / `c` 内容）而不是一个 —— 单一 `s` 闭包会让
    ///   「某个字段该走哪条」只能靠读代码判断，改错一处就是「底衬跟着内容缩放」
    ///   或反之，两者都不报错。分成两组后，字段归组在**同一行**可见。
    fn for_dpis(backdrop_dpi: u32, content_dpi: u32) -> Self {
        let backdrop_dpi = if backdrop_dpi == 0 { 96 } else { backdrop_dpi };
        let content_dpi = if content_dpi == 0 { 96 } else { content_dpi };
        let b = |dip: i32| Self::dip_of(backdrop_dpi, dip);
        let c = |dip: i32| Self::dip_of(content_dpi, dip);
        let icon = c(ICON_PX_DIP);
        Self {
            dpi: backdrop_dpi,
            content_dpi,
            // ── 底衬：恒按系统 DPI（用户口径，不受内容档位影响）──
            h: b(WIDGET_H_DIP),
            radius: b(BACKDROP_RADIUS_DIP),
            // ── 内容：按内容 DPI（受 `taskbar_content_scale` 影响）──
            icon,
            font: c(FONT_PX_DIP),
            pad_x: c(PAD_X_DIP),
            icon_text_gap: c(ICON_TEXT_GAP_DIP),
            item_gap: c(ITEM_GAP_DIP),
            item_max_w: c(ITEM_MAX_W_DIP),
            text_row_h: icon / 2,
        }
    }

    /// DIP → 物理像素（四舍五入），按**内容** DPI。`dpi == 0` 视为 96。
    ///
    /// ⚠️ 单独抽出来是给**不在本表里**的 DIP 值用（如 `estimate_text_px` 的
    ///   每字符宽度估算）—— 那些值按字号比例缩放，不适合塞进固定字段。
    /// ⚠️ 走**内容**口径：文本宽度属于内容，必须与 `font` 同步缩放，
    ///   否则「字号小、估宽按大字号」⇒ 窗口比内容宽（留白）或反之（重叠）。
    ///
    /// ⭐ 生产调用点：`draw_items` 里换算 `EDGE_MARGIN_DIP`（靠左/靠右的留白）。
    pub fn dip(&self, dip: i32) -> i32 {
        Self::dip_of(self.content_dpi, dip)
    }

    fn dip_of(dpi: u32, dip: i32) -> i32 {
        let dpi = if dpi == 0 { 96 } else { dpi };
        ((dip as f32) * dpi as f32 / 96.0).round() as i32
    }

    /// 当前任务栏 DPI 下的**底衬**度量（内容口径 = 系统口径，即 `Default`）。
    ///
    /// ⚠️ 取**任务栏**的 DPI 而不是进程/桌面的：widget 是任务栏的子窗，
    ///   多显示器「各屏缩放不同」时只有任务栏所在屏的 DPI 是对的。
    ///   取不到（Explorer 重建间隙）⇒ 回落 96（宁可小一号，也不要按错的缩放错位）。
    ///
    /// ⚠️ 保持**纯函数**（不读配置）：只消费 `h` / `radius` 的调用点用它，无锁风险。
    pub fn current() -> Self {
        Self::for_dpi(taskbar_dpi())
    }

    /// 当前**内容**口径的度量：底衬取任务栏 DPI，内容按 `taskbar_content_scale` 解析。
    ///
    /// ⛔⛔ 这是「绘制」（`draw_items`）与「测宽」（`fetch_into_snapshot`）的**共同入口**
    ///   —— 两处各写各的换算必然漂移：窗口按旧口径找避让槽、内容按新口径画
    ///   ⇒ 文字压到邻居上，且不报错、不 panic（见 PLAYBOOK §E10.3）。
    pub fn current_content() -> Self {
        // ⚠️ `with_config` 的闭包只返回一个 `Copy` 枚举值 ⇒ guard 不泄漏到锁外，
        //    与 AGENTS.md 的「持锁区不得做窗口操作」纪律不冲突。
        let scale = crate::config::with_config(|c| c.taskbar_content_scale);
        Self::for_scales(taskbar_dpi(), scale)
    }

    /// 音乐组件的**封面 + 双排信息**专用度量：**恒为默认档**，不随
    /// 「任务栏内容缩放大小」改变（用户 2026-09-30 明确要求）。
    ///
    /// ⭐ 只作用于音乐面板里的**这两样**；同一面板的**三键 / 切换键 / 各项间距 /
    ///   左右留白**仍跟随设置（用户同轮逐条确认「都按 A」＝跟随）。
    ///
    /// ⚠️⚠️ **为什么这两样要钉住、其余跟随**（别再合并成一个档位）：
    ///   封面是**正方形图片**、双排信息是**两行连续文字**，它们与设备面板那种
    ///   「一串电量/音量数字」对尺寸缩放的敏感度完全不同 ⇒ 一起缩放会让
    ///   **封面与文字的比例**失真（用户 2026-09-30 实测后要求拆开）。
    ///   而按键是**图标控件**，与设备面板的图标同性质 ⇒ 跟着设备那档走才对。
    ///
    /// ⚛️ 连带一处必须一起钉住：**文字宽度上限** `MUSIC_TEXT_MAX_W_DIP` 的换算。
    ///   它若跟着设置缩放，设置变小 ⇒ 上限变小 ⇒ **歌名被更早截断**
    ///   ⇒ 那等于「设置仍然在影响双排信息」，与本条要求直接矛盾。
    ///
    /// ⛔ 由 `draw_music_render` 与 `content_px`（tooltip）**共用**：
    ///   否则用户设「偏小」时**提示会比它描述的文字大**（`content_px` 的硬红线）。
    pub fn art_fixed() -> Self {
        Self::art_fixed_for(taskbar_dpi())
    }

    /// [`Self::art_fixed`] 的**纯函数**版本（给定底衬 DPI），便于单测。
    ///
    /// ⚠️ 为什么要单独抽一个：`art_fixed()` 内部读 `taskbar_dpi()`（真任务栏窗口），
    ///   在 `#[test]` 里会回落 96 ⇒ **测不到任何非 96 的情形**，判据会退化成
    ///   「只验证了一个像素值」。同 `for_scales` 的思路。
    pub fn art_fixed_for(backdrop_dpi: u32) -> Self {
        Self::for_scales(backdrop_dpi, crate::config::TaskbarContentScale::Default)
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

/// 把一个 **DIP** 标称值换算成本机物理像素，**走「内容」口径**。///
/// ⭐ 供 `taskbar_tooltip` 复用 ⇒ tooltip 的字号/宽度与 widget 的文字**同一尺度**。
/// ⛔⛔ **必须是内容口径、不是系统 DPI**（早先误用了 `taskbar_dpi()`，125% 下得 14px）：
/// widget 上的文字按**内容**口径解析（`Default` 档 = 96 DPI 基准 ⇒ 11px），
/// 而底衬/任务栏按系统 DPI（125% ⇒ 50px）。两者是**故意**分开的（`E14` 的硬红线）。
/// 误用系统 DPI ⇒ 125% 下 tooltip 文字 14px 而 widget 文字 11px
/// ⇒ **提示比它所描述的内容还大**，与 FluentFlyout 实测「tooltip 与 widget 文字等大」相反。
///
/// ⭐ 走**当前显示的面板**对应的内容尺度，不是单一全局档位：
///   · 音乐面板 ⇒ [`Metrics::art_fixed`]（封面 + 双排信息**固定档**）
///   · 设备面板 / 无面板 ⇒ [`Metrics::current_content`]（跟随设置）
///
///   ⚠️ 若这里只读全局档位，用户设「偏小」时**音乐那块的提示会比它描述的文字大**
///   —— 正是上面那条硬红线。音乐面板的文字档与封面档**恒定**，提示必须跟着恒定。
#[cfg(target_os = "windows")]
pub fn content_px(dip: i32) -> i32 {
    match current_panel() {
        Some(crate::config::TaskbarPanel::Music) => Metrics::art_fixed().dip(dip),
        _ => Metrics::current_content().dip(dip),
    }
}

/// 任务栏矩形 `(left, top, width, height)`（**屏幕坐标**，物理像素）。
///
/// ⭐ 供 `taskbar_tooltip` 定位用：提示要落在**任务栏外缘**，
/// 而原生 tooltip 默认贴着工具窗口（＝本 widget，在任务栏**内**）摆 ⇒ 压在任务栏上。
#[cfg(target_os = "windows")]
pub fn taskbar_rect_tuple() -> Option<(i32, i32, i32, i32)> {
    taskbar_rect()
}

/// 主屏宽度（物理像素）。给 tooltip 做「不出屏」钳制。
#[cfg(target_os = "windows")]
pub fn screen_size() -> (i32, i32) {
    unsafe {
        (
            windows_sys::Win32::UI::WindowsAndMessaging::GetSystemMetrics(0), // SM_CXSCREEN
            windows_sys::Win32::UI::WindowsAndMessaging::GetSystemMetrics(1), // SM_CYSCREEN
        )
    }
}

/// 第 `index` 个设备条目在**屏幕**上的 `(x, width)`。
///
/// ⭐ 单一来源 = [`item_rects`]（`draw_items` 算出的那份）⇒ tooltip 的横向锚点
/// 与视觉位置**不可能分叉**（与「绘制与测宽同源」同一条纪律）。
#[cfg(target_os = "windows")]
pub fn item_rect_on_screen(index: usize) -> Option<(i32, i32)> {
    // 中毒时恢复（原来 `.ok()?` 会让 tooltip 锚点**永久**失效）。
    let rects = crate::state::lock_unpoisoned(&LAST_ITEM_RECTS);
    let r = *rects.get(index)?;
    // `item_rects` 出的是 widget **客户区**坐标；这里换算到屏幕。
    let (widget_x, _, w, _) = window_screen_rect(WIDGET_HWND.load(Ordering::SeqCst) as _)?;
    // ⛔ 拒绝**占位矩形**：建窗时 widget 是 `(0,0,1,1)`，`GetWindowRect` 会**成功**
    //   返回 `w=1` —— 此时算出的锚点 x 是 0，tooltip 会闪现在屏幕**最左端**。
    //   （`GetWindowRect` 只在 hwnd 无效时才失败；占位矩形是**合法**的返回值。）
    //   真正的防线是调用侧「先 `commit` 再发布」，这里是第二道。
    if w <= 1 {
        return None;
    }
    Some((widget_x + r.left, r.right - r.left))
}

/// 最近一帧的逐项矩形（**主线程写、任意线程读**）。
///
/// ⛔ 用 `Mutex` 而非裸静态：这是**跨线程**共享的容器。
///   锁内**只做 clone、不做别的** ⇒ 持锁时间极短，符合持锁区纪律。
/// 第 `index` 个设备命中的**屏幕** `(x, width)`（诊断转发）。
#[cfg(target_os = "windows")]
pub fn item_anchor_screen(index: usize) -> Option<(i32, i32)> {
    item_rect_on_screen(index)
}

/// 已发布命中的条目数（诊断用）。
#[cfg(target_os = "windows")]
pub fn item_rect_count() -> usize {
    // 诊断量：中毒时给真实长度（原来 `unwrap_or(0)` 会让排查时**谎报 0 条**）。
    crate::state::lock_unpoisoned(&LAST_ITEM_RECTS).len()
}

static LAST_ITEM_RECTS: std::sync::Mutex<Vec<windows_sys::Win32::Foundation::RECT>> =
    std::sync::Mutex::new(Vec::new());

/// 记录本帧的逐项矩形，供 tooltip 定位使用（`draw_items` 末尾调用，主线程）。
#[cfg(target_os = "windows")]
fn publish_item_rects(rects: &[windows_sys::Win32::Foundation::RECT]) {
    // 中毒时照样发布：静默跳过会让上一帧矩形**永久**留着（命中判定错位）。
    *crate::state::lock_unpoisoned(&LAST_ITEM_RECTS) = rects.to_vec();
}

// ── 音量滚轮：窗口线程只做「命中 + 记账」，写音量交给后台线程 ──────────
//
// ⛔ **为什么不能在 `wnd_proc` 里直接写音量**：`set_device_volume` 要 COM
//   `Activate` + `SetMasterVolumeLevelScalar`，是**阻塞**调用；`wnd_proc` 跑在
//   窗口线程（= 主线程）⇒ 一次卡住就表现为「任务栏窗口冻结、点不动」。
//   与本模块既有纪律一致（拖拽只改原子量、绘制只读快照）。
//
// ⭐ **为什么用「单后台线程 + 通道」而不是每次 `thread::spawn`**：读—改—写必须
//   **串行**。快速滚动时若并发执行，每个线程读到的都是同一个旧基准
//   ⇒ 滚 5 格只生效 1 格（用户看到「滚轮失灵」）。用通道把请求排队、由一个线程
//   按序处理 ⇒ 步进不丢，且**不需要新增全局锁**（也就不必在 `state.rs` 的
//   锁序表里登记新边）。

/// 取（并按需启动）音量滚轮工作线程的发送端。
#[cfg(target_os = "windows")]
fn volume_worker_sender() -> Option<std::sync::mpsc::Sender<(String, bool)>> {
    type Chan = std::sync::mpsc::Sender<(String, bool)>;
    static SLOT: std::sync::OnceLock<std::sync::Mutex<Option<Chan>>> = std::sync::OnceLock::new();
    let slot = SLOT.get_or_init(|| std::sync::Mutex::new(None));
    // 中毒时恢复：原来 `Err(_) => return None` ⇒ sender **永久**拿不到
    // ⇒ 滚轮调音量彻底失效且无日志。
    let mut guard = crate::state::lock_unpoisoned(slot);
    if guard.is_none() {
        let (tx, rx) = std::sync::mpsc::channel::<(String, bool)>();
        let spawned = std::thread::Builder::new()
            .name("pm-wheel-volume".to_string())
            .spawn(move || volume_worker_loop(rx));
        if let Err(e) = spawned {
            append_log(&format!("[widget] 音量滚轮线程启动失败: {e}"));
            return None;
        }
        *guard = Some(tx);
    }
    guard.clone()
}

/// 工作线程主循环：按序「读当前值 → 加一格 → 写回」。
#[cfg(target_os = "windows")]
fn volume_worker_loop(rx: std::sync::mpsc::Receiver<(String, bool)>) {
    while let Ok((device_id, up)) = rx.recv() {
        // ⚠️ 精细调节**每格现读**：用户可能在设置里中途改开关
        let fine = crate::config::with_config(|c| c.volume_fine_adjust);
        match crate::audio::get_device_volume(&device_id) {
            Ok(cur) => {
                let next = apply_wheel_volume(cur, up, fine);
                if (next - cur).abs() < f32::EPSILON {
                    // 已在 0% 或 100%：页面里滑块同样不动，不算失败
                    if crate::config::verbose_log_enabled() {
                        append_log(&format!("[widget] 滚轮到边界: {cur} dir={up}"));
                    }
                } else if let Err(e) = crate::audio::set_device_volume(&device_id, next) {
                    append_log(&format!("[widget] 滚轮写音量失败: {e}"));
                } else {
                    // ⭐⭐ **乐观更新 + 立即重绘**（用户 2026-09-28：滚动时音量信息
                    //   刷新率太低、不实时）。原先只调 `refresh_async()`，它要先跑
                    //   完一整轮 WMI 枚举（数百毫秒）才重画 ⇒ 数字明显滞后于滚动。
                    //   ⇒ 先把**刚写进去的值**就地落到快照，再直接 `post_refresh`
                    //   让主线程马上重绘这一帧（单帧 0.45ms，见 `snapshot::store`）。
                    //   真实值仍由随后那轮枚举校正（不会漂移：写的就是真值）。
                    if snapshot::update_volume(&device_id, next) {
                        let handle = WIDGET_HWND.load(std::sync::atomic::Ordering::SeqCst);
                        if widget_alive() {
                            unsafe { ffi::post_refresh(handle as *mut core::ffi::c_void) };
                        }
                    }
                    if crate::config::verbose_log_enabled() {
                        append_log(&format!("[widget] 滚轮音量 id={device_id} {cur} -> {next}"));
                    }
                }
            }
            Err(e) => append_log(&format!("[widget] 滚轮读音量失败: {e}")),
        }
        // ⭐ 复用「FORCE_REPAINT + refresh_async()」这条**已过实战验证**的重绘路径
        //   （`refresh_async` 自带防抖与「未挂载早退」），不新开通道。
        FORCE_REPAINT.store(true, std::sync::atomic::Ordering::Release);
        refresh_async();
    }
}

/// `WM_MOUSEWHEEL`：光标落在某项的音量行上 ⇒ 受理一次调音量请求。
///
/// 返回 `true` = 命中并已受理（消息必须被吃掉，不能落到 `DefWindowProcW`——
/// 那会被转发给父窗口（任务栏），变成「滚动整个任务栏」）。
#[cfg(target_os = "windows")]
fn wheel_adjust_volume(hwnd: *mut core::ffi::c_void, wp: usize) -> bool {
    // wParam 高 16 位是滚轮增量（120/格），低 16 位是按键状态（忽略）
    let delta = ((wp >> 16) & 0xFFFF) as u16 as i16;
    let Some(up) = wheel_direction(delta) else {
        return false;
    };
    // ⚠️ 用 `GetCursorPos` 而不是 lParam 里的屏幕坐标：多显示器下 lParam 存的是
    //   16 位有符号坐标、跨屏会溢出（本模块的 hover 轮询也统一用 `GetCursorPos`）。
    let mut pt = windows_sys::Win32::Foundation::POINT { x: 0, y: 0 };
    if unsafe { windows_sys::Win32::UI::WindowsAndMessaging::GetCursorPos(&mut pt) } == 0 {
        return false;
    }
    // ⛔⛔ **面板闸**（2026-09-28 引入音乐组件时加）：本函数用命中下标去
    //   `snapshot::load()`（**设备**快照）取要调音量的设备。音乐面板发布的矩形
    //   下标空间与之**完全不同**（封面+三键=1 项，可能还有切换按钮）⇒ 不设闸就会
    //   「在音乐面板上滚滚轮，把**别的设备**的音量改了」，而且**不报错、日志正常**
    //   ——这是本仓最怕的那类静默失效。
    // ⚠️ 动画期间同样不受理：此刻 `current_panel()` 已是**新**面板，
    //   而 `LAST_ITEM_RECTS` 还是**旧**面板的矩形 ⇒ 下标空间与数据源错位，
    //   与上面那条闸是同一类静默失效。
    if switch_anim_active() {
        return false;
    }
    if !matches!(current_panel(), Some(crate::config::TaskbarPanel::Devices)) {
        if crate::config::verbose_log_enabled() {
            append_log(&format!(
                "[widget] 滚轮未受理: 当前面板={:?}（滚轮调音量只属于设备面板）",
                current_panel()
            ));
        }
        return false;
    }
    // 命中矩形是**窗口局部坐标** ⇒ 减去窗口原点
    let Some((wx, wy, _, _)) = window_screen_rect(hwnd) else {
        return false;
    };
    // ⭐ 触发区用 `LAST_ITEM_RECTS` —— 与 tooltip **同一份**矩形（用户 2026-09-28
    //   要求「触发区域与 tooltip 一致」）。它是 widget 局部坐标 ⇒ 减去窗口原点。
    let rects = crate::state::lock_unpoisoned(&LAST_ITEM_RECTS).clone();
    let local = (pt.x - wx, pt.y - wy);
    let Some(idx) = hit_test_item_rects(&rects, local) else {
        // 未命中也要留痕：「滚轮没反应」时这是**唯一**能区分
        // 「压根没收到消息」与「收到了但位置不对」的信息。
        if crate::config::verbose_log_enabled() {
            let desc: Vec<String> = rects
                .iter()
                .map(|r| format!("({},{},{},{})", r.left, r.top, r.right, r.bottom))
                .collect();
            append_log(&format!(
                "[widget] 滚轮未命中: 光标=({},{}) 局部={:?} 触发区={desc:?}",
                pt.x, pt.y, local
            ));
        }
        return false;
    };
    // ⛔ 下面这些早退**必须留痕**：滚轮的第一报障是「滚轮没反应」，而命中判定之后的
    //   每一条早退都曾是**一行日志都不打**的静默 return ⇒ 现场完全无法归因
    //   （2026-09-28 实测：验收脚本一直往**无音频端点**的 #0 上注入，日志里什么都没有，
    //   差点被误读成「滚轮功能坏了」）。判据：命中之后不许有静默 return。
    let reject = |why: &str| {
        if crate::config::verbose_log_enabled() {
            append_log(&format!("[widget] 滚轮未受理: idx={idx} 原因={why}"));
        }
        false
    };
    // 该项必须**真的有音频端点**（键鼠也能 hover 出 tooltip，但不可调音量）
    let items = match snapshot::load() {
        Some(v) => v,
        None => return reject("快照为空"),
    };
    let Some(it) = items.get(idx) else {
        return reject("索引越界");
    };
    if !it.has_audio {
        return reject("该项无音频端点（键鼠类设备不可调音量）");
    }
    // 端点 id 随快照带出来（`WidgetItem::audio_device_id`），**不在这里按名字反查**
    //   —— 那是身份判定，必须留在后端同一判据里。
    let Some(device_id) = it.audio_device_id.clone() else {
        return reject("有音频但拿不到端点 id");
    };
    let Some(tx) = volume_worker_sender() else {
        return reject("拿不到后台写线程");
    };
    // 记标准级：这是**用户动作的结果**（「滚轮没反应」是最可能的报障）
    append_log(&format!(
        "[widget] 滚轮调音量: idx={idx} dir={} step={:.1}%",
        if up { "up" } else { "down" },
        volume_step(crate::config::with_config(|c| c.volume_fine_adjust))
    ));
    tx.send((device_id, up)).is_ok()
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
    /// 该设备的**音频端点 id**；滚轮调音量必须靠它写回（`None` = 没有端点）。
    ///
    /// ⭐ 为什么 `volume` 不够：滚轮要做**读—改—写**，而 `volume` 只是**上次快照**
    ///   的值；用它当基准会在快速滚动时丢步（每格都基于同一个陈旧起点）。
    ///   而「按名字/身份反查端点」是身份判定，必须留在后端同一处判据里
    ///   （见 `device_identity::audio_endpoint_key`），故随设备把 id 带出来。
    pub audio_device_id: Option<String>,
    /// 是否是系统默认音频设备（前缀标记，帮助用户一眼认出放声口）
    pub is_default: bool,
    /// 用户固定的设备（读不出数据也保留；仅离线占位条目置灰）
    pub pinned: bool,
    /// 是否有设备页枚举到的在线节点；用于避免已连接但暂时无电量数据的设备被置灰。
    pub connected: bool,
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

    /// 就地更新某端点的音量（**滚轮乐观更新**）。返回是否真的改了。
    ///
    /// ⭐ 为什么需要（用户 2026-09-28：滚动时音量刷新率太低）：`refresh_async`
    ///   要先跑完一整轮 WMI/端点枚举（数百毫秒）才重画 ⇒ 数字明显滞后于滚动。
    ///   这里先落**刚写进去的那个值**，主线程立即重绘；随后那轮枚举会用真值校正。
    /// ⚠️ 只按 `audio_device_id` 匹配（**不是**按名字/身份键）：端点 id 是
    ///   `AudioDevice` 的天然主键，而此处拿不到端点枚举上下文。
    pub fn update_volume(device_id: &str, volume: f32) -> bool {
        let mut guard = crate::state::lock_unpoisoned(&SNAPSHOT);
        let Some(items) = guard.as_mut() else {
            return false;
        };
        let mut changed = false;
        for it in items.iter_mut() {
            if it.audio_device_id.as_deref() == Some(device_id) && it.volume != Some(volume) {
                it.volume = Some(volume);
                changed = true;
            }
        }
        changed
    }

    /// 读一份快照副本（**主线程**调）。
    pub fn load() -> Option<Vec<WidgetItem>> {
        crate::state::lock_unpoisoned(&SNAPSHOT).clone()
    }
}

#[cfg(target_os = "windows")]
use crate::process::{append_log, append_verbose_log, to_wide};
#[cfg(target_os = "windows")]
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU64, AtomicU8, Ordering};
use std::sync::Mutex;

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
/// 「把提示挪到第 `wp` 个设备上方」——由 50ms hover 轮询线程投递（**跨线程只投递**）。
///
/// ⛔ 单独一条消息而不是复用 `WM_APP_REFRESH`：定位**不涉及重绘**，
///   混进去会让「仅切换悬停设备」也触发一整帧 DIB 重建（4~6ms + 逐像素合成）。
const WM_APP_TOOLTIP_SHOW: u32 = 0x8000 + 3;

/// 「面板切换动画播完了」——由动画线程在最后一帧后投递。
///
/// ⛔ 必须**独立成一条消息**，不能靠「驱动线程把 `SWITCH_ANIM_STARTED` 清零」
///   就完事：清零发生在**后台线程**，而缓存的两张面板位图是 GDI 对象、
///   `rel_x` 也要在主线程算 ⇒ 清理与收尾帧必须回到主线程做。
///   若让后台线程直接清状态，主线程会「在动画已停、状态已清」的窗口里
///   收到一帧刷新请求 ⇒ 走常规路径重画（其实也对），但缓存位图就**没人释放**了
///   ⇒ 每切一次泄漏两张 DIB（DC + HBITMAP），切几十次后耗尽 GDI 资源。
const WM_APP_SWITCH_END: u32 = 0x8000 + 4;

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

/// 可用区（= 整条任务栏）左端的屏幕 x（物理像素）。
///
/// ⚠️ 自 2026-09-26 第二次方案变更后，它**恒等于任务栏左缘**（典型为 0）——
///   像素扫描被移除（见 `find_widget_slot`），这里保留为原子量只是为了
///   让「可用区」与窗口位置在绘制线程上仍走同一套传递路径。
#[cfg(target_os = "windows")]
static SLOT_X: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(0);
/// 可用区宽度（= 任务栏宽度）。同上，恒为整条任务栏宽度。
#[cfg(target_os = "windows")]
static SLOT_W: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(1);
/// 是否已取到任务栏矩形；取不到时主线程保持 widget 隐藏（防御：Explorer 未就绪）。
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

/// 光标当前落在**第几个**设备条目上（`-1` = 未命中/间隙）。
///
/// ⛔ `isize` 而非 `usize`：**需要一个「无」的值**，用 `usize` 就得拿 `MAX` 当哨兵，
/// 而哨兵值参与算术极易出错（`index + 1` 溢出成 0）。
/// ⭐ 由 hover 轮询线程**写**、主线程经 `WM_APP_TOOLTIP_SHOW` **读**（经消息传参，
///   不直接读这个原子量 ⇒ 无锁序登记）。
static HOVERED_ITEM: std::sync::atomic::AtomicIsize = std::sync::atomic::AtomicIsize::new(-1);

/// 「本轮悬停」的时间起点（`GetTickCount64` 毫秒）。
///
/// ⭐ 三态编码（**不是** `Option`，省一次分支也省一个锁）：
/// · `0`   —— 当前不在任何设备上（无计时）
/// · `>0`  —— 悬停起点毫秒数（正在计时）
/// · `-1`  —— 本轮已显示过（**不重复投递**，否则每 50ms 重画一次）
#[cfg(target_os = "windows")]
static HOVER_SINCE: std::sync::atomic::AtomicIsize = std::sync::atomic::AtomicIsize::new(0);

/// 单调毫秒时钟（`GetTickCount64`）——⛔ **不能**用 `SystemTime`：
///   它随时可能被用户改钟表/夏令时回拨，用它算「过了 500ms」会算出负数或天文数字。
#[cfg(target_os = "windows")]
fn now_ms() -> u64 {
    unsafe { windows_sys::Win32::System::SystemInformation::GetTickCount64() }
}

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

/// 系统主题（深/浅）变更后，请求任务栏窗口重绘（**任意线程可调**）。
///
/// ── 为什么需要它（真机实测缺陷）─────────────────────────────────────
/// `refresh_async` 的判据是「**数据变了** / **槽位移动了** / `FORCE_REPAINT`」三者之一。
/// 系统切换深/浅模式时**三者全不满足**：设备数据没变、槽位没动，
/// 只是**渲染参数**（底衬 alpha、内容明暗）变了 ⇒ 判据返回 `false` ⇒ 窗口不重绘。
/// 表现就是「系统改了主题，任务栏窗口还是旧配色，**必须 hover 一下才更新**」——
/// 因为 hover 的 50ms 轮询会走另一条重绘路径。
///
/// ⇒ 与「改贴靠位置」是**同一类缺陷**（见 `FORCE_REPAINT` 的注释）：
///   渲染参数变化不会体现在数据判据里，必须**显式要求重绘**。
///
/// ⭐ 复用 `FORCE_REPAINT + refresh_async()` 这个**已经过实战验证**的组合，
///   而不是新开一条重绘路径 —— 后者会绕过防抖与「未挂载早退」，制造第二套语义。
///
/// ⚠️ 不直接调 `repaint_from_snapshot`：主题变更时窗口可能**尚未挂载**
///   （`should_show()` 为假），此时重绘请求应由 `refresh_async` 内部的
///   `widget_alive()` 早退掉，而不是在这里各判一次。
#[cfg(target_os = "windows")]
pub fn notify_system_theme_changed() {
    use std::sync::atomic::Ordering as O;
    FORCE_REPAINT.store(true, O::Release);
    refresh_async();
}

#[cfg(not(target_os = "windows"))]
pub fn notify_system_theme_changed() {}

/// 诊断用的 FFI 集合。集中在一处便于核对「到底调了哪些 API」。
#[cfg(target_os = "windows")]
pub(crate) mod ffi {
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

    use super::{Metrics, WM_APP_RAISE, WM_APP_REFRESH, WM_APP_SWITCH_END, WM_APP_TOOLTIP_SHOW};
    use windows_sys::Win32::Foundation::{HWND, POINT, SIZE};
    use windows_sys::Win32::Graphics::Gdi::{
        CreateCompatibleDC, CreateDIBSection, CreateFontW, DeleteDC, DeleteObject, DrawTextW,
        GetTextFaceW, SelectObject, SetBkMode, SetTextColor, BITMAPINFO, BITMAPINFOHEADER, BI_RGB,
        DIB_RGB_COLORS, HBITMAP, HDC, HFONT, HGDIOBJ, TRANSPARENT,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DestroyWindow, GetWindow, GetWindowLongPtrW, PostMessageW,
        RegisterClassW, SetParent, SetWindowLongPtrW, SetWindowPos, ShowWindow,
        UpdateLayeredWindow, GWL_STYLE, GW_HWNDPREV, HWND_TOP, SWP_NOACTIVATE, SWP_NOMOVE,
        SWP_NOSIZE, SWP_NOZORDER, ULW_ALPHA, WM_CAPTURECHANGED, WM_LBUTTONDOWN, WM_LBUTTONUP,
        WM_MOUSEMOVE, WM_MOUSEWHEEL, WNDCLASSW, WS_CHILD, WS_EX_LAYERED, WS_EX_NOACTIVATE,
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
        SetWindowLongPtrW(hwnd, GWL_STYLE, ((style & !WS_POPUP) | WS_CHILD) as isize);
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

    /// 投递给主线程：**面板切换动画已播完，请收尾**（释放缓存位图 + 清状态）。
    ///
    /// ⛔ 同样只投递、不碰窗口 —— 缓存的 DIB 是在主线程建的，
    ///   释放走主线程才与「窗口/GDI 属主线程」这条一致。
    pub unsafe fn post_switch_end(hwnd: HWND) {
        PostMessageW(hwnd, WM_APP_SWITCH_END, 0, 0);
    }

    /// 请求**显示**第 `index` 条提示。**任意线程可调**（只投递，不碰窗口）。
    ///
    /// ⛔ `index < 0` 表示「隐藏」——而不是「显示第 -1 条」：
    ///   鼠标离开 widget 时必须**立即消失**，不能留一个 5s 超时的残影。
    pub unsafe fn post_tooltip_show(hwnd: HWND, index: isize) {
        PostMessageW(hwnd, WM_APP_TOOLTIP_SHOW, index as usize, 0);
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
    /// ⛔⛔ **空串必须在这里拦下**（2026-09-29 实测闪退，根因）：
    ///
    /// `Vec::<u16>::new().as_ptr()` 是**悬垂的对齐哨兵指针**（u16 对齐 = 2），
    /// **不是有效内存**。而 `DrawTextW` 即使 `cch = 0` 也会去解引用它
    /// ⇒ **访问违例**：进程直接消失，**无 panic、无 WER、panic 文件为空**
    /// （这正是它此前被反复误判成「不是 panic、是访问违例之外的某种怪东西」的原因）。
    ///
    /// 触发条件很窄：**某个会话上报 `artist = ""`** ⇒ 同一首歌画得好好的，
    /// 「切换媒体会话」后崩 ⇒ 看起来像会话切换的 bug，实际是**空串**。
    /// ⚠️ 凡是「把 `&[u16]` 交给 GDI」的入口都要有这道闸
    /// （本函数 / `render_text_mask` / `taskbar_tooltip::draw_text`），漏一处就还能崩。
    pub unsafe fn measure_text(memdc: HDC, font: HFONT, text: &[u16]) -> i32 {
        if text.is_empty() {
            return 0;
        }
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
        // ⚠️ 同样不能把**空切片**的悬垂指针交给 GDI（根因见 `measure_text` 的注释）。
        //   跳过绘制、仍返回全白掩码 ⇒ 契约不变（调用方的合成照常走，只是没有笔画）。
        if !text.is_empty() {
            DrawTextW(memdc, text.as_ptr(), text.len() as i32, &mut rc, flags);
        }
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
    /// ⭐ `bold` 参数目前**两个调用点都传 `false`**（用户 2026-09-29：设备面板与
    ///   音乐面板文字都取消加粗，tooltip 一直是常规字重 ⇒ 三处统一）。
    ///   ⚠️ **别把它当成死参数**：2026-09-25 曾要求「电量/音量加粗」（理由是 11px 细体
    ///   在浅色任务栏上偏虚），2026-09-29 又推翻 —— 说明这条是**观感取舍、不是技术上不可行**，
    ///   换字体后基础条件变了结论就会变，改前先量一下实际效果。
    /// ⚠️ 加粗会让文本**变宽**：`measure_text` 与绘制共用同一个 HFONT，
    ///   故排版宽度自动跟着变（这正是「测量与绘制必须同字体」那条纪律的收益）。
    /// 面板用的字体族（**带回落**，Win10 上必须能降级）。
    ///
    /// ⚠️⚠️ **为什么不能直接写死 "Segoe UI Variable Text"**：
    ///   `CreateFontW` 找不到请求的字体时**不报错**，而是**静默替换**成字体映射器
    ///   挑的别的字体（不崩溃、不返回 NULL）⇒ 那样在 Windows 10 上会显示成某个
    ///   随机字体，且**没有任何日志**。
    ///   ⇒ 这里用 `GetTextFaceW` **验货**：把请求的族名和 GDI 实际选中的比对，
    ///   对不上就换下一个候选。
    ///
    /// ⭐ 候选顺序 = Windows 11 系统字体优先：
    ///   `Segoe UI Variable Text`（系统 UI 字体，字腔更开）→ `Segoe UI`（Win10）。
    ///   只探测一次（`OnceLock`），之后每次建字体只读这个结果。
    pub fn widget_face() -> &'static [u16] {
        static FACE: std::sync::OnceLock<Vec<u16>> = std::sync::OnceLock::new();
        FACE.get_or_init(|| {
            for cand in ["Segoe UI Variable Text", "Segoe UI"] {
                let wide: Vec<u16> = format!("{cand}\0").encode_utf16().collect();
                if unsafe { face_available(&wide) } {
                    crate::process::append_log(&format!("[widget] 面板字体族 = {cand}"));
                    return wide;
                }
                crate::process::append_log(&format!("[widget] 字体族 {cand} 不可用，尝试下一个"));
            }
            crate::process::append_log("[widget] 所有候选字体族都不可用，回落 GDI 默认字体");
            "\0".encode_utf16().collect()
        })
    }

    /// 请求 `want` 这个族，看 GDI 实际选中的到底是不是它。
    unsafe fn face_available(want: &[u16]) -> bool {
        let hfont = CreateFontW(-12, 0, 0, 0, 400, 0, 0, 0, 0x01, 0, 0, 0, 0, want.as_ptr());
        if hfont.is_null() {
            return false;
        }
        let screen = windows_sys::Win32::Graphics::Gdi::GetDC(std::ptr::null_mut());
        let memdc = CreateCompatibleDC(screen);
        windows_sys::Win32::Graphics::Gdi::ReleaseDC(std::ptr::null_mut(), screen);
        if memdc.is_null() {
            DeleteObject(hfont);
            return false;
        }
        let old = SelectObject(memdc, hfont);
        let mut buf = [0u16; 64];
        let n = GetTextFaceW(memdc, buf.len() as i32, buf.as_mut_ptr());
        SelectObject(memdc, old);
        DeleteDC(memdc);
        DeleteObject(hfont);
        if n <= 0 {
            return false;
        }
        let want_nz: Vec<u16> = want.iter().copied().take_while(|c| *c != 0).collect();
        let got: Vec<u16> = buf[..n as usize]
            .iter()
            .copied()
            .take_while(|c| *c != 0)
            .collect();
        // GDI 回显可能附带样式后缀（族名 + style），**前缀相同**即算命中
        !want_nz.is_empty() && got.len() >= want_nz.len() && got[..want_nz.len()] == want_nz[..]
    }

    pub unsafe fn create_font(px_height: i32, bold: bool) -> HFONT {
        let face: &[u16] = widget_face();
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
            // ⭐⭐ `DEFAULT_QUALITY`(0) 而非 `ANTIALIASED_QUALITY`(4)：
            //   **差在 hinting**——4 会把字形轮廓平滑掉、**关掉网格对齐**，
            //   14px 下笔画边缘被「摊平」成灰边（真机观感=「发虚」，
            //   本函数上方 `create_font_cleartype` 的注释早就预言了这一点）。
            //   0 保留灰度抗锯齿、但**打开 hinting**：竖干与横画吸附到像素网格，
            //   小字号下明显更实、更清晰。
            //   ⛔ 仍是**灰度**、不是 ClearType：面板底衬半透明，而 ClearType 的
            //   彩色子像素边缘依赖「底色已知且不透明」，叠上去会脏（tooltip 能用
            //   ClearType 是因为它那个气泡是实色的）。
            0, // DEFAULT_QUALITY（灰度 AA + hinting）
            0, // DEFAULT_PITCH
            face.as_ptr(),
        )
    }

    /// 建一个 **ClearType** 字体（**质量 5**），供「**不透明**背景上直接画字」用。
    ///
    /// ⭐ 为什么 tooltip 要另开一个、而不是复用上面的 `create_font`：
    ///   分层窗口的**通用**画字路径是「白底黑字掩码 → 覆盖度 → 预乘」（见
    ///   `render_text_mask`），因为 widget 的底衬**半透明**，直接画进去的文字
    ///   alpha 字节恒为 0 ⇒ 会被 `ULW` 当全透明。
    ///   但 tooltip 的气泡是**不透明**的（`--flyout-bg` 实色）⇒ 可以直接在一个
    ///   **实色** DIB 上用 GDI 画字 ⇒ 能用 **ClearType** 子像素抗锯齿。
    ///   ⚠️ 灰度 AA（`ANTIALIASED_QUALITY`）在 14px 下笔画边缘会被「摊平」成灰边
    ///   ⇒ 真机观感就是用户说的「**发虚**」。子像素渲染按 RGB 排列分别着色三遍，
    ///   边缘因此**锐利得多**。
    /// ⛔ 前提是**必须实色背景**：ClearType 的彩色边缘依赖「底色已知且不透明」，
    ///   一旦叠到半透明底衬上就会脏（`taskbar_widget` 头部记着这条）。
    pub unsafe fn create_font_cleartype(px_height: i32, bold: bool) -> HFONT {
        // ⭐ 与面板**同一个字体族**（用户 2026-09-29）：tooltip 是面板的延续，
        // 族名若与面板不同，同一首歌在两处的字形/字距就不一样（此前是
        // 面板 Segoe UI + tooltip 也 Segoe UI，但字重不同 ⇒ 观感仍不一致）。
        let face: &[u16] = widget_face();
        CreateFontW(
            -px_height,
            0,
            0,
            0,
            if bold { 700 } else { 400 },
            0,
            0,
            0,
            0x01, // DEFAULT_CHARSET
            0,    // OUT_DEFAULT_PRECIS
            0,    // CLIP_DEFAULT_PRECIS
            5,    // CLEARTYPE_QUALITY
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
        if msg == WM_APP_SWITCH_END {
            // ⭐ 收尾**必须**在主线程：释放缓存的两张面板位图（GDI 对象）、
            //   清动画状态、补一帧常规重绘。后台线程直接清状态的话，
            //   这两张 DIB 就没有释放时机 ⇒ 每切一次泄漏一次。
            super::finish_switch_anim(hwnd);
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
        if msg == WM_APP_TOOLTIP_SHOW {
            // ⭐ 显示/隐藏提示（主线程执行）。
            // ⛔ 只发消息、不取数、不重绘 widget（提示的显隐与 widget 绘制是两件事）。
            //   ⛔⛔ 提示**绝不能**在这里触发 widget 重绘：重绘会走
            //   `repaint_from_snapshot` → `draw_items` → `sync` → 重入本分支。
            let idx = wp as isize;
            // ⭐ **每次**决策都记标准级：包括「要显示」与「要隐藏」两条路径。
            //   此前只记成功路径 ⇒ 「提示停在最左端」这类**位置类**故障
            //   在日志里没有现场，无法定位（真机连报三次）。
            super::append_log(&format!(
                "[widget] tooltip: 收到 SHOW idx={} 锚点={:?} rects={} widget={:?}",
                idx,
                if idx >= 0 {
                    super::item_anchor_screen(idx as usize)
                } else {
                    None
                },
                super::item_rect_count(),
                super::window_screen_rect(hwnd)
            ));
            // ⛔ 动画期间**不弹**提示：`LAST_ITEM_RECTS` 还是**上一帧**的坐标，
            //   而画面上内容正在横向移动 ⇒ 提示会停在已经滑走的位置上。
            if idx < 0 || super::switch_anim_active() {
                crate::taskbar_tooltip::hide();
            } else if (idx as usize) <= 32 {
                crate::taskbar_tooltip::show(idx as usize);
            }
            return 0;
        }
        if msg == WM_MOUSEWHEEL {
            // ⭐ 音量滚轮（用户 2026-09-28）。命中判定与记账都在**窗口线程**完成
            //   （纯内存：读上一帧发布的矩形 + 取方向），**实际写音量在后台线程**。
            // ⛔ 必须 `return 0` 吃掉这条消息：落到 `DefWindowProcW` 会被转发给
            //   父窗口（任务栏）⇒ 变成「滚动整个任务栏」。
            if super::wheel_adjust_volume(hwnd, wp) {
                return 0;
            }
        }
        // ⚠️ 只在「固定位置」关掉时接管（`drag_begin` 内部判 `drag_enabled()`）；
        //   固定位置时**不拦截**，交回 `DefWindowProcW` 保持原行为。
        if msg == WM_LBUTTONDOWN {
            // ⭐ 记按下点与时刻（供 UP 时区分「点击」与「拖拽」——音乐面板的按钮
            //   靠点击触发，而 `drag_begin` 是**无条件** SetCapture 的）
            super::press_record();
            // ⭐ 记下按到哪个按钮（用户 2026-09-29：按下要变灰）。
            //   ⛔ **必须在 `drag_begin` 之前**：`drag_begin` 会 `SetCapture`，
            //   而 `SetCapture` 可能立刻触发 `WM_CAPTURECHANGED`（见下）把状态清掉。
            super::press_begin(hwnd);
            if super::drag_begin(hwnd) {
                return 0;
            }
        } else if msg == WM_MOUSEMOVE {
            // 非拖拽期间到达的移动消息由 `drag_move` 自行忽略（判 `DRAG_ACTIVE`）。
            super::drag_move(hwnd);
        } else if msg == WM_LBUTTONUP {
            // ⛔ **顺序要紧**：先判点击、再 `drag_finish`。
            //   `drag_finish` 会清 `DRAG_ACTIVE` 并把落点写进配置；若它先跑，
            //   「点一下按钮」会被当成「在原地松手」= 拖拽结束 ⇒ 位置被记一次
            //   （无害），但反过来（先判点击）才能拿到按下时的局部坐标。
            // ⛔ **先清按下态再判点击**：清早了会让这一帧的重绘看不到「按下」，
            //   用户根本看不到变灰效果；清晚（同理）则会闪一帧不消失的灰。
            //   这里先算、再清、再触发动作，`post_refresh` 只在状态真变时发。
            if let Some(local) = super::press_take_click() {
                super::set_pressed(super::PRESS_NONE, hwnd);
                super::on_click(hwnd, local);
            } else {
                super::set_pressed(super::PRESS_NONE, hwnd);
            }
            super::drag_finish(hwnd);
        } else if msg == WM_CAPTURECHANGED {
            // 捕获被别处抢走（任务栏抢焦点、其它窗口 `SetCapture`…）：
            // 窗口停在哪就记哪 —— 总比丢掉位置强。非拖拽期间到达则直接返回。
            //
            // ⭐⭐ 这里**必须**清按下态（用户 2026-09-29 的「按下变灰」）：
            //   捕获被抢走时**不会**有 `WM_LBUTTONUP` 到来 ⇒ 漏清的话按钮会
            //   **永久停在按下态**，而画面上没有任何东西会再去纠正它。
            super::set_pressed(super::PRESS_NONE, hwnd);
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
///
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
///   —— 8 台 × (32 图标 + 4 间隙 + 约 30 文本 + 10 间隔) ≈ 620px，能在避让后的可用区里放下。
#[cfg(target_os = "windows")]
const WIDGET_MAX_ITEMS: usize = 8;

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
fn format_volume(it: &WidgetItem, fine_adjust: bool) -> String {
    if !it.has_audio {
        // 键鼠这类无音频端点的设备：用户明确要求显示 N/A（而不是留空）
        return "N/A".to_string();
    }
    // ⭐ 静音**优先于**百分比：端点静音时其音量值仍是旧值（不是 0）
    //    ⇒ 直接显示「静音」，否则会显示成「40%」这种与听觉不符的数字。
    if it.is_muted == Some(true) {
        return "静音".to_string();
    }
    // ⭐ **是否带小数由「音量精细调节」开关决定**（用户 2026-09-28）：
    //   · 开 ⇒ `12.5%`：此时滚轮是 0.1% 步进，显示整数会让「滚轮动了但数字没变」
    //     看起来像失效；
    //   · 关 ⇒ `40%`：此时滚轮是 1% 步进，多余的小数位是噪音。
    // ⚠️ **参数传入而不是这里读配置**：本函数被单测直接调用，且绘制路径要求
    //   「GDI 调用之前不持锁」（见 `draw_items` 里的配置快照）⇒ 由调用方读一次传进来。
    match it.volume {
        Some(v) if fine_adjust => format!("{:.1}%", v * 100.0),
        Some(v) => format!("{}%", (v * 100.0).round() as i32),
        None => "N/A".to_string(),
    }
}

// ── 滚轮调音量：纯函数（可单测）────────────────────────────────
//
// ⭐ 与「弹出窗口·音量控制页」的滑块滚轮**逐字对齐**（`popup-audio.js`）：
//   ① 滑块值域是 `0..100`（百分点），COM 给的是 `0..1` 分数；
//   ② 滑块**初值先取整**（`fineAdjustEnabled ? Math.round(v*1000)/10
//      : Math.round(v*100)`，`popup-audio.js:181`）；
//   ③ 步进：精细 ⇒ `round((v ± 0.1) * 10) / 10`；普通 ⇒ 上 `floor(v)+1`、
//      下 `ceil(v)-1`（**不是** `round(v±1)`）。
// ⛔ ②③ 任何一条照抄错，手感都会与页面对不上（② 错 ⇒ 每格少 1%，
//   实测 `0.12f32` = 11.9999997% 不取整则 floor 得 11 ⇒ 向上只到 12%）。

/// 滚轮一格的步进量（0.1 / 1 个百分点）。**纯函数**：便于钉住「与页面一致」。
fn volume_step(fine_adjust: bool) -> f32 {
    if fine_adjust {
        0.1
    } else {
        1.0
    }
}

/// `WM_MOUSEWHEEL` 的 wParam 高字（滚轮增量，120/格）⇒ 方向。
/// 返回 `None` 表示**不该处理**（增量为 0：某些触控板/精密滚轮会发 0）。
fn wheel_direction(delta: i16) -> Option<bool> {
    match delta {
        d if d > 0 => Some(true),  // 向上滚 = 增大音量
        d if d < 0 => Some(false), // 向下滚 = 减小音量
        _ => None,
    }
}

/// 按一格滚轮算出新音量。**f64 中间量**对齐 JS（页面滑块是双精度）。
fn apply_wheel_volume(current: f32, up: bool, fine_adjust: bool) -> f32 {
    let v = current as f64;
    // ① 换算到百分点 + ② 照抄页面的「初值先取整」
    let base = if fine_adjust {
        (v * 1000.0).round() / 10.0
    } else {
        (v * 100.0).round()
    };
    let step = volume_step(fine_adjust) as f64; // 0.1 或 1（百分点）
    let next = if fine_adjust {
        ((base + if up { step } else { -step }) * 10.0).round() / 10.0
    } else if up {
        base.floor() + 1.0
    } else {
        base.ceil() - 1.0
    };
    (next.clamp(0.0, 100.0) / 100.0) as f32
}

/// 命中测试：光标（窗口局部坐标）落在哪一项的**整块**区域内。
///
/// ⭐ **触发区必须与 tooltip 完全一致**（用户 2026-09-28 明确要求）：两者都用
///   `item_rects` 发布的同一份矩形。第一版只把「音量文本那一行」当触发区，
///   于是「鼠标停在图标上滚」完全无效 —— 而那恰恰是最顺手的位置。
///   ⛔ 分叉后既不一致（图标上滚没反应）又不易察觉（tooltip 明明弹了）。
fn hit_test_item_rects(
    rects: &[windows_sys::Win32::Foundation::RECT],
    pt: (i32, i32),
) -> Option<usize> {
    rects.iter().enumerate().find_map(|(i, r)| {
        if pt.0 >= r.left && pt.0 < r.right && pt.1 >= r.top && pt.1 < r.bottom {
            Some(i)
        } else {
            None
        }
    })
}

/// 图标资源与解码（**编译期嵌入 + 进程内缓存**）。
///
/// ⭐ 五类语义图标 × 深浅两套 = 10 张 PNG，全部 `include_bytes!` 进二进制：
///   · `tray-icon{,-dark}.png`          —— 默认图标，同时作为 2.4G 鼠标图标
///   · `tray-keyboard-icon{,-dark}.png` —— 2.4G 键盘
///   · `tray-gamepad-icon{,-dark}.png`  —— 2.4G 手柄
///   · `tray-speaker-icon{,-dark}.png`  —— 扬声器（音频端点前缀是「扬声器」）
///   · `tray-headphone-icon{,-dark}.png`—— 耳机（音频端点前缀是「耳机」）
///
/// ⛔ **为什么必须嵌二进制而不是运行时读文件**：MSIX 包安装目录只读、
///   且 Tauri 的 `resources` 部署路径与 exe 不同（见 PLAYBOOK §I）
///   ⇒ 运行期找文件必然踩路径坑。`include_bytes!` 零歧义。
///
/// ⚠️ 解码（PNG → RGBA）用 `image` crate，结果按 `(kind, dark)` 缓存到
///   `OnceLock`：图标内容编译期就固定，重复解码纯浪费。
#[cfg(target_os = "windows")]
/// 图标重采样（**预乘空间**）+ 结果缓存。
///
/// ⛔⛔ **为什么需要它（2026-09-29 用户报「跟随系统缩放后图标变糊，且越大越糊」）**：
///   母图只有 **32×32**，而 `m.icon = 32 × content_dpi/96`（125% ⇒ 40、150% ⇒ 48……）。
///   100% 时 32→32 走**恒等分支、零重采样** ⇒ 锐利；一旦 >100% 就变成
///   **32→N 的放大**——放大**不可能凭空造出细节**，双线性只能把每个源像素摊成
///   渐变块，于是 1px 笔画（暂停双竖条只有 25.6/1024）糊成灰带，**缩放越大越糊**。
///   ⇒ 解法不是换一个更好的**放大**滤波器，而是**换母图**：生成脚本输出
///   **256×256** 母图，于是任何现实目标尺寸（≤128px，即 400%）都走**缩小**，
///   而缩小是重采样**准确**的方向（面积平均 = 超采样渲染）。
#[cfg(target_os = "windows")]
mod resample {
    use super::RgbaAlias;

    /// 缓存键的命名空间（避免不同图标的键撞车）。
    /// 设备图标 `0..16`、音乐图标 `16..32`、封面用 `NS_COVER | hash`。
    pub const NS_DEVICE: u32 = 0;
    pub const NS_MUSIC: u32 = 16;
    pub const NS_COVER: u32 = 0x8000_0000;
    /// 封面缩小后的**轻锐化强度**（见 `unsharp_mask_premul` 的实测数据）。
    /// ⛔ >1.0 收益趋平且开始显形（实测细节能量：k=0.5 → 63108、k=0.8 → 65382、
    ///   k=1.0 → 66616，基准未锐化 58294）⇒ **0.8 是性价比拐点**。
    ///   ⛔ 上面的 **overshoot 钳制**是「敢给到 0.8」的前提——没有它，
    ///   理想阶跃边上 k=0.5 就会冲出「越过 90% 再跌回」的振铃波形。
    const COVER_SHARPEN: f64 = 0.8;

    /// `(槽位, 目标边长) -> 结果`。键空间 = 图标数 × 尺寸数，天然有界。
    static CACHE: std::sync::Mutex<Vec<(u32, u32, RgbaAlias)>> = std::sync::Mutex::new(Vec::new());

    /// 取缓存的缩放结果，没有就现算并记下。
    ///
    /// ⭐ **必须有缓存**：重采样是 O(母图像素数) 的，而 `scale_to` 在**每帧重绘**
    ///   时对每个图标都调一次（此前还会每次新分配一个 `Vec`）。缓存键只含
    ///   「图标 + 尺寸」，两者都要用户改设置/换主题才会变 ⇒ 稳态下**零重算**。
    pub fn scale_cached(slot: u32, side: u32, src: &RgbaAlias) -> Option<RgbaAlias> {
        {
            let c = crate::state::lock_unpoisoned(&CACHE);
            if let Some((_, _, r)) = c.iter().find(|(s, d, _)| *s == slot && *d == side) {
                return Some(r.clone());
            }
        }
        let r = resample(src, side)?;
        let mut c = crate::state::lock_unpoisoned(&CACHE);
        // 上限只是防御：正常键空间是「图标数 × 用过的尺寸数」，不会逼近
        if c.len() < 64 {
            c.push((slot, side, r.clone()));
        }
        Some(r)
    }

    /// ⛔ **给「已经是预乘」的数据用**（音乐封面缓存就是预乘的）。
    ///
    /// 直插会把透明像素的 RGB 混进半透明边缘 ⇒ 图标外圈出现黑晕/白边，
    /// 所以直插必须**先预乘再插值、插完再反预乘**；可输入若**本身就是预乘**的，
    /// 再走一遍「预乘 → 反预乘」就是**二次预乘** ⇒ 画出来偏暗。
    /// 预乘空间对加权平均是**封闭**的（平均的预乘 = 预乘的平均），所以直接平均即可。
    pub fn scale_cached_premul(slot: u32, side: u32, src: &RgbaAlias) -> Option<RgbaAlias> {
        {
            let c = crate::state::lock_unpoisoned(&CACHE);
            if let Some((_, _, r)) = c.iter().find(|(s, d, _)| *s == slot && *d == side) {
                return Some(r.clone());
            }
        }
        let (px, sw, sh) = src;
        let (sw, sh) = (*sw, *sh);
        if sw == 0 || sh == 0 || side == 0 {
            return None;
        }
        let mut out = vec![0u8; (side * side * 4) as usize];
        if sw == side && sh == side {
            out.copy_from_slice(px);
        } else if side <= sw {
            // ⭐ 封面用 **Lanczos3** 而不是面积平均（见 `lanczos3_downscale_premul` 的实测数据）
            lanczos3_downscale_premul(px, sw, sh, side, &mut out);
            unsharp_mask_premul(&mut out, side, side, COVER_SHARPEN);
        } else {
            catmull_rom_upscale_premul(px, sw, sh, side, &mut out);
        }
        let r = (out, side, side);
        let mut c = crate::state::lock_unpoisoned(&CACHE);
        if c.len() < 64 {
            c.push((slot, side, r.clone()));
        }
        Some(r)
    }

    /// 预乘空间里的重采样，**先预乘再插值、插完再反预乘**（直插会把透明像素的
    /// RGB 混进半透明边缘 ⇒ 图标外圈出现黑晕/白边）。
    fn resample(src: &RgbaAlias, side: u32) -> Option<RgbaAlias> {
        let (px, sw, sh) = src;
        let (sw, sh) = (*sw, *sh);
        if sw == 0 || sh == 0 || side == 0 {
            return None;
        }
        if sw == side && sh == side {
            return Some((px.clone(), side, side)); // 恒等：零重采样
        }
        let mut out = vec![0u8; (side * side * 4) as usize];
        if side <= sw {
            box_downscale(px, sw, sh, side, &mut out);
        } else {
            catmull_rom_upscale(px, sw, sh, side, &mut out);
        }
        Some((out, side, side))
    }

    /// **面积平均**缩小：对每个目标像素求它覆盖的源区域的**加权平均**。
    ///
    /// ⭐ 这是缩小的**正确**滤波器：它等价于「先在高分辨率下渲染再降采样」
    ///   （超采样），既不会像最近邻那样丢像素（细笔画断裂、斜边出现台阶），
    ///   也不会像双线性那样在 5:1 的比例下只取到 4 个采样点（严重混叠）。
    fn box_downscale(px: &[u8], sw: u32, sh: u32, side: u32, out: &mut [u8]) {
        box_accumulate(px, sw, sh, side, out, true)
    }

    /// ⭐ **Lanczos3（窗化 sinc）可分离缩小**，输入输出**都是预乘**。
    ///
    /// ⚠️⛔⛔ **封面不能用面积平均**（用户 2026-09-29 实测报「封面非常模糊」，
    ///   且 128→40 仍糊）。两者差别在**模糊半径**：3.2:1 的 box 平均在**输出**尺度上
    ///   约糊 1.6px，照片上非常明显；实测（对照「单步 400→40 lanczos3」）：
    ///   ```text
    ///   128→40 nearest 13.40 │ cubic 2.03 │ mitchell 1.89 │ lanczos3 1.80
    ///   400→40 单步 cubic 0.65 │ mitchell 0.67
    ///   ```
    ///   图标（线稿）用 box 没问题，所以**只给封面这条路径换**，不动图标。
    ///
    /// ⭐ 预乘空间对**插值**是封闭的（预乘的插值 = 插值的预乘），所以这里
    ///   直接对预乘值加权即可，不需要「先反预乘再插值」那套。
    fn lanczos3_downscale_premul(px: &[u8], sw: u32, sh: u32, side: u32, out: &mut [u8]) {
        /// Lanczos3 核：`sinc(x)·sinc(x/3)`，半宽 3。
        fn lanczos(x: f64) -> f64 {
            const A: f64 = 3.0;
            let ax = x.abs();
            if ax >= A {
                0.0
            } else if ax < 1e-9 {
                1.0
            } else {
                let t = std::f64::consts::PI * x;
                (t.sin() / t) * ((t / A).sin() / (t / A).sin())
            }
        }
        /// 每个目标像素在源轴上的采样点 + 归一化权重。
        fn axis_weights(src: u32, dst: u32) -> Vec<Vec<(u32, f64)>> {
            const A: f64 = 3.0;
            let scale = src as f64 / dst as f64;
            // ⚠️⚠️ **缩小必须把核「拉宽」到 `scale` 倍**（`max`，不是 `min`）：
            //   6.4:1 缩小时若仍用 ±3 源像素的核，就等于**点采样**——混叠严重
            //   （`image` crate 也是按 `support = A * max(scale,1)` 做的）。
            let fscale = scale.max(1.0);
            let support = A * fscale;
            let mut table = Vec::with_capacity(dst as usize);
            for i in 0..dst {
                // ⛔⛔ 目标 → 源的映射是**乘** `scale`，不是除 —— 我第一版写成了
                //   `(i + 0.5) / scale - 0.5`，于是 40 个输出像素的采样中心全部
                //   落在源图 x∈[0,9]（40×6.4 的正确位置应是 x∈[0,256]），
                //   屏幕上表现为「封面变成一块纯色」（那张封面左上角正好是天空）。
                let center = (i as f64 + 0.5) * scale - 0.5;
                let lo = ((center - support).ceil() as i64).max(0) as u32;
                let hi = (((center + support).floor() as i64 + 1) as u32).min(src);
                let mut taps: Vec<(u32, f64)> = Vec::new();
                let mut sum = 0.0;
                for x in lo..hi {
                    let v = lanczos((x as f64 - center) / fscale);
                    if v != 0.0 {
                        taps.push((x, v));
                        sum += v;
                    }
                }
                if sum != 0.0 {
                    for t in taps.iter_mut() {
                        t.1 /= sum;
                    }
                }
                table.push(taps);
            }
            table
        }
        // 横向一遍
        let hx = axis_weights(sw, side);
        let mut tmp = vec![0f64; (side * sh * 4) as usize];
        for y in 0..sh {
            for (i, taps) in hx.iter().enumerate() {
                let mut acc = [0f64; 4];
                for &(sx, w) in taps {
                    let si = ((y * sw + sx) * 4) as usize;
                    for c in 0..4 {
                        acc[c] += px[si + c] as f64 * w;
                    }
                }
                let di = ((y * side + i as u32) * 4) as usize;
                tmp[di..di + 4].copy_from_slice(&acc);
            }
        }
        // 纵向一遍
        let vy = axis_weights(sh, side);
        for (j, taps) in vy.iter().enumerate() {
            for i in 0..side {
                let mut acc = [0f64; 4];
                for &(sy, w) in taps {
                    let si = ((sy * side + i) * 4) as usize;
                    for c in 0..4 {
                        acc[c] += tmp[si + c] as f64 * w;
                    }
                }
                let di = ((j as u32 * side + i) * 4) as usize;
                for c in 0..4 {
                    out[di + c] = acc[c].clamp(0.0, 255.0).round() as u8;
                }
            }
        }
    }

    /// ⭐ **轻锐化**（unsharp mask）：`out += k · (out − blur(out))`。
    ///
    /// ⚠️ **只在封面这条路径上用**。缩小必然低通——Lanczos3 已经接近理论最优，
    ///   但「最优」≠「看起来够锐」：拿真机封面实测（256→40，中间 8 行的
    ///   相邻像素差分和，越大越锐）：
    ///   ```text
    ///   Nearest 22071 │ Lanczos3 18036 │ Cubic 16929 │ Mitchell 15502
    ///   Lanczos3 + 轻锐化 24827
    ///   ```
    ///   ⇒ Lanczos3 之后还差约 **27%** 的锐度。`Nearest` 虽然最锐，但它是
    ///   **点采样**（斜边锯齿、细纹理断续），照片上更难看，所以不取。
    ///
    /// ⭐ 用 **[1,2,1]/4** 而不是高斯：小半径、便宜，而且我们要的只是把
    ///   低通造成的高频损失**补回一点**，不是做艺术化锐化。
    /// ⛔ `k` 不能大：>0.8 会在平坦色块上产生**振铃**（白边/黑边），
    ///   对封面这种大面积渐变尤其明显。实测 0.5 安全。
    fn unsharp_mask_premul(buf: &mut [u8], w: u32, h: u32, k: f64) {
        if k <= 0.0 {
            return;
        }
        // ⭐ **overshoot 钳制**：锐化结果限制在**原图 3×3 邻域的极值**内。
        //   没有这一步，unsharp 在**理想阶跃边**上必然振铃（实测 k=0.5 就能把
        //   一个完美阶跃的边沿冲出「先越过 90% 再跌回去」的波形 ⇒ 出现白边）。
        //   钳制后**结构上不可能**产生新极值，k 可以放心给到 0.5。
        let orig = buf.to_vec();
        let mut blur = vec![0f64; (w * h * 4) as usize];
        // 水平 [1,2,1]/4
        for y in 0..h {
            for x in 0..w {
                let mut acc = [0f64; 4];
                for (dx, ww) in [(x as i64 - 1, 1.0), (x as i64, 2.0), (x as i64 + 1, 1.0)] {
                    if dx < 0 || dx >= w as i64 {
                        continue;
                    }
                    let si = ((y * w + dx as u32) * 4) as usize;
                    for c in 0..4 {
                        acc[c] += buf[si + c] as f64 * ww;
                    }
                }
                let di = ((y * w + x) * 4) as usize;
                for c in 0..4 {
                    blur[di + c] = acc[c] / 4.0;
                }
            }
        }
        // 竖直 + 相减
        for y in 0..h {
            for x in 0..w {
                let mut acc = [0f64; 4];
                for (dy, ww) in [(y as i64 - 1, 1.0), (y as i64, 2.0), (y as i64 + 1, 1.0)] {
                    if dy < 0 || dy >= h as i64 {
                        continue;
                    }
                    let si = ((dy as u32 * w + x) * 4) as usize;
                    for c in 0..4 {
                        acc[c] += blur[si + c] * ww;
                    }
                }
                let di = ((y * w + x) * 4) as usize;
                // 原图 3×3 邻域的逐通道极值（钳制用）
                let mut nmin = [255f64; 4];
                let mut nmax = [0f64; 4];
                for ny in y.saturating_sub(1)..(y + 1).min(h) {
                    for nx in x.saturating_sub(1)..(x + 1).min(w) {
                        let ni = ((ny * w + nx) * 4) as usize;
                        for c in 0..4 {
                            let v = orig[ni + c] as f64;
                            if v < nmin[c] {
                                nmin[c] = v;
                            }
                            if v > nmax[c] {
                                nmax[c] = v;
                            }
                        }
                    }
                }
                for c in 0..4 {
                    let b = acc[c] / 4.0;
                    let o = orig[di + c] as f64;
                    let v = (o + k * (o - b)).clamp(nmin[c], nmax[c]).clamp(0.0, 255.0);
                    buf[di + c] = v.round() as u8;
                }
            }
        }
    }

    /// 测试可见别名（同文件 `mod tests` 用；生产路径走 `box_accumulate`）。
    #[cfg(test)]
    pub(super) fn box_accumulate_for_test(px: &[u8], sw: u32, sh: u32, side: u32, out: &mut [u8]) {
        box_accumulate(px, sw, sh, side, out, true)
    }

    /// 测试可见别名（同文件 `mod tests` 用）。
    #[cfg(test)]
    pub(super) fn lanczos3_for_test(px: &[u8], sw: u32, sh: u32, side: u32, out: &mut [u8]) {
        lanczos3_downscale_premul(px, sw, sh, side, out)
    }

    /// 测试可见别名（同文件 `mod tests` 用）。
    #[cfg(test)]
    pub(super) fn unsharp_for_test(buf: &mut [u8], w: u32, h: u32, k: f64) {
        unsharp_mask_premul(buf, w, h, k)
    }

    /// 面积平均的**公共内核**：`premul_in` 为真表示输入已是预乘（不再预乘、
    /// 也不反预乘，直接写回）；否则走「预乘 → 平均 → 反预乘」。
    fn box_accumulate(px: &[u8], sw: u32, sh: u32, side: u32, out: &mut [u8], premul_in: bool) {
        for y in 0..side {
            let sy0 = y as f64 * sh as f64 / side as f64;
            let sy1 = (y + 1) as f64 * sh as f64 / side as f64;
            for x in 0..side {
                let sx0 = x as f64 * sw as f64 / side as f64;
                let sx1 = (x + 1) as f64 * sw as f64 / side as f64;
                let mut acc = [0f64; 4];
                let mut wsum = 0f64;
                let y_lo = sy0.floor() as u32;
                let y_hi = ((sy1.ceil() as u32).min(sh)).max(y_lo + 1);
                let x_lo = sx0.floor() as u32;
                let x_hi = ((sx1.ceil() as u32).min(sw)).max(x_lo + 1);
                for yy in y_lo..y_hi.min(sh) {
                    let wy = sy1.min(yy as f64 + 1.0) - sy0.max(yy as f64);
                    if wy <= 0.0 {
                        continue;
                    }
                    for xx in x_lo..x_hi.min(sw) {
                        let wx = sx1.min(xx as f64 + 1.0) - sx0.max(xx as f64);
                        if wx <= 0.0 {
                            continue;
                        }
                        let w = wx * wy;
                        let si = ((yy * sw + xx) * 4) as usize;
                        let a = px[si + 3] as f64;
                        if premul_in {
                            acc[0] += px[si] as f64 * w;
                            acc[1] += px[si + 1] as f64 * w;
                            acc[2] += px[si + 2] as f64 * w;
                        } else {
                            acc[0] += px[si] as f64 * a / 255.0 * w;
                            acc[1] += px[si + 1] as f64 * a / 255.0 * w;
                            acc[2] += px[si + 2] as f64 * a / 255.0 * w;
                        }
                        acc[3] += a * w;
                        wsum += w;
                    }
                }
                store_pixel_ex(out, ((y * side + x) * 4) as usize, &acc, wsum, premul_in);
            }
        }
    }

    /// **Catmull-Rom 双三次**放大（母图小于目标时才会走到）。
    ///
    /// ⚠️ 母图换成 256 之后**正常设备上走不到这条路**（任何现实 DPI 都 ≤128px），
    ///   保留它只为兜底：那 3 张没有矢量源的设备图标仍是 32px 母图，
    ///   双三次比双线性**锐利得多**（负瓣带来的边缘对比度），是退而求其次的改善。
    fn catmull_rom_upscale(px: &[u8], sw: u32, sh: u32, side: u32, out: &mut [u8]) {
        catmull_rom_upscale_ex(px, sw, sh, side, out, false)
    }

    fn catmull_rom_upscale_premul(px: &[u8], sw: u32, sh: u32, side: u32, out: &mut [u8]) {
        catmull_rom_upscale_ex(px, sw, sh, side, out, true)
    }

    fn catmull_rom_upscale_ex(
        px: &[u8],
        sw: u32,
        sh: u32,
        side: u32,
        out: &mut [u8],
        premul_in: bool,
    ) {
        // 标准 Catmull-Rom 权重（t ∈ [0,1)）。⚠️ **第 i 个权重对应偏移 i-1**
        //   （即 -1, 0, +1, +2）—— 索引时必须写 `x0 - 1 + i`。
        //   ⛔ 我第一版写成 `x0 + i`，**整体偏移了一个像素**：t=0 时权重 [0,1,0,0]
        //   落在 `x0+1` 上，于是「放大 2×1 的黑白」得到 [255,255,255,255]
        //   （全白）而不是 [0, ~52, ~203, 255]。判据 `scale_up_interpolates` 抓到。
        let weights = |t: f64| -> [f64; 4] {
            let t2 = t * t;
            let t3 = t2 * t;
            [
                -0.5 * t3 + t2 - 0.5 * t,
                1.5 * t3 - 2.5 * t2 + 1.0,
                -1.5 * t3 + 2.0 * t2 + 0.5 * t,
                0.5 * t3 - 0.5 * t2,
            ]
        };
        for y in 0..side {
            let fy = ((y as f64 + 0.5) * sh as f64 / side as f64 - 0.5).max(0.0);
            let wy = weights(fy - fy.floor());
            let y0 = fy.floor() as i64;
            for x in 0..side {
                let fx = ((x as f64 + 0.5) * sw as f64 / side as f64 - 0.5).max(0.0);
                let wx = weights(fx - fx.floor());
                let x0 = fx.floor() as i64;
                let mut acc = [0f64; 4];
                let mut wsum = 0f64;
                for (j, wyj) in wy.iter().enumerate() {
                    let yy = (y0 - 1 + j as i64).clamp(0, sh as i64 - 1) as u32;
                    for (i, wxi) in wx.iter().enumerate() {
                        let xx = (x0 - 1 + i as i64).clamp(0, sw as i64 - 1) as u32;
                        let w = wxi * wyj;
                        let si = ((yy * sw + xx) * 4) as usize;
                        let a = px[si + 3] as f64;
                        if premul_in {
                            acc[0] += px[si] as f64 * w;
                            acc[1] += px[si + 1] as f64 * w;
                            acc[2] += px[si + 2] as f64 * w;
                        } else {
                            acc[0] += px[si] as f64 * a / 255.0 * w;
                            acc[1] += px[si + 1] as f64 * a / 255.0 * w;
                            acc[2] += px[si + 2] as f64 * a / 255.0 * w;
                        }
                        acc[3] += a * w;
                        wsum += w;
                    }
                }
                store_pixel_ex(out, ((y * side + x) * 4) as usize, &acc, wsum, premul_in);
            }
        }
    }

    /// 把**预乘**累加值写成 straight RGBA（`A == 0` 时 RGB 无意义，置 0）。
    fn store_pixel_ex(out: &mut [u8], di: usize, acc: &[f64; 4], wsum: f64, premul_in: bool) {
        if wsum <= 0.0 {
            return;
        }
        let a = (acc[3] / wsum).clamp(0.0, 255.0);
        out[di + 3] = a.round() as u8;
        if a <= 0.0 {
            out[di] = 0;
            out[di + 1] = 0;
            out[di + 2] = 0;
            return;
        }
        for c in 0..3 {
            if premul_in {
                // 预乘直通：Σ(c·w) / Σw（预乘空间对平均封闭）
                out[di + c] = (acc[c] / wsum).clamp(0.0, 255.0).round() as u8;
            } else {
                // 反预乘：Σ(c·a/255·w) / Σ(a·w) · 255
                out[di + c] = (acc[c] / acc[3].max(1e-9) * 255.0)
                    .clamp(0.0, 255.0)
                    .round() as u8;
            }
        }
    }
}

/// 解码后的 RGBA 位图（`(像素, 宽, 高)`）。像素顺序 = RGBA（`image` crate 约定）。
#[cfg(target_os = "windows")]
type RgbaAlias = (Vec<u8>, u32, u32);

mod icons {
    use crate::device_identity::AudioKind;
    use std::sync::OnceLock;

    // 10 张图，编译期嵌入
    static MOUSE_LIGHT: &[u8] = include_bytes!("../icons/tray-widget-mouse-icon.png");
    static MOUSE_DARK: &[u8] = include_bytes!("../icons/tray-widget-mouse-icon-dark.png");
    static KEYBOARD_LIGHT: &[u8] = include_bytes!("../icons/tray-keyboard-icon.png");
    static KEYBOARD_DARK: &[u8] = include_bytes!("../icons/tray-keyboard-icon-dark.png");
    static GAMEPAD_LIGHT: &[u8] = include_bytes!("../icons/tray-gamepad-icon.png");
    static GAMEPAD_DARK: &[u8] = include_bytes!("../icons/tray-gamepad-icon-dark.png");
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
    static CACHE_KEYBOARD_LIGHT: OnceLock<Option<Rgba>> = OnceLock::new();
    static CACHE_KEYBOARD_DARK: OnceLock<Option<Rgba>> = OnceLock::new();
    static CACHE_GAMEPAD_LIGHT: OnceLock<Option<Rgba>> = OnceLock::new();
    static CACHE_GAMEPAD_DARK: OnceLock<Option<Rgba>> = OnceLock::new();
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
            (AudioKind::Keyboard, false) => (&CACHE_KEYBOARD_LIGHT, KEYBOARD_LIGHT),
            (AudioKind::Keyboard, true) => (&CACHE_KEYBOARD_DARK, KEYBOARD_DARK),
            (AudioKind::Gamepad, false) => (&CACHE_GAMEPAD_LIGHT, GAMEPAD_LIGHT),
            (AudioKind::Gamepad, true) => (&CACHE_GAMEPAD_DARK, GAMEPAD_DARK),
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
    /// 设备图标的**缓存槽位**（`NS_DEVICE` 命名空间内唯一，与 [`AudioKind`] 一一对应）。
    pub fn slot_of(kind: AudioKind) -> u32 {
        super::resample::NS_DEVICE
            + match kind {
                AudioKind::Pointer => 0,
                AudioKind::Keyboard => 1,
                AudioKind::Gamepad => 2,
                AudioKind::Speaker => 3,
                AudioKind::Headphones => 4,
            }
    }

    /// 缩放到 `side × side`（**带缓存**，见 [`resample`]）。
    ///
    /// ⚠️ `slot` 必须**按图标内容唯一**（设备图标用 `NS_DEVICE + 序号`、
    ///   音乐图标用 `NS_MUSIC + 序号`、封面用 `NS_COVER | hash`）——它是缓存键，
    ///   撞车会让一个图标**显示成另一个**。⛔ 不要用 `AudioKind as u32` 直接当槽：
    ///   那只是碰巧不撞，没有语义保护。
    /// ⚠️⚠️ **这不是假设，本仓库已经踩过**：两个单测各用 `900_002/4` 缩**不同**的
    ///   合成图，并行跑时谁先谁污染 ⇒ 另一个拿到**别人的像素**
    ///   （表现为 `scale_up_interpolates` 偶发失败、单跑却通过）。
    ///   ⇒ **每个调用点都要独占一个槽位**；测试里用互不相交的大数字。
    pub fn scale_to(slot: u32, src: &Rgba, side: u32) -> Option<Rgba> {
        super::resample::scale_cached(slot, side, src)
    }
}

/// 音乐控制组件的图标（**独立键空间**）。
///
/// ⛔ **不复用 `AudioKind`**：设备图标的 `icons::get` 对 `AudioKind` 做**穷尽 match**
///   （`icons::get` 内部 + `main.rs::format_item_debug`），往里加变体会同时打断两处
///   编译点。音乐图标与设备类别无关，用自己的枚举更干净。
///
/// ⭐ **线宽基准 51/1024**：播放控制 4 图标的外框标称 51.2（生成脚本实测 51.0），
///   切换图标由生成脚本从 80 腐蚀到 51.2 ⇒ 与现有设备图标**同量级**。
///   生成脚本 `generate_music_icons.mjs`（一次性、不入库）在 **1024 分辨率**下
///   实测描边厚度 —— 32px 下量会被像素量化主导（1.6px 的描边量出来只有 1px，
///   无论真实线宽多少都量出 32.0）。这个坑我踩过一次。
#[cfg(target_os = "windows")]
mod music_icons {
    use std::sync::OnceLock;

    static PLAY_LIGHT: &[u8] = include_bytes!("../icons/tray-music-play-icon.png");
    static PLAY_DARK: &[u8] = include_bytes!("../icons/tray-music-play-icon-dark.png");
    static PAUSE_LIGHT: &[u8] = include_bytes!("../icons/tray-music-pause-icon.png");
    static PAUSE_DARK: &[u8] = include_bytes!("../icons/tray-music-pause-icon-dark.png");
    static PREV_LIGHT: &[u8] = include_bytes!("../icons/tray-music-prev-icon.png");
    static PREV_DARK: &[u8] = include_bytes!("../icons/tray-music-prev-icon-dark.png");
    static NEXT_LIGHT: &[u8] = include_bytes!("../icons/tray-music-next-icon.png");
    static NEXT_DARK: &[u8] = include_bytes!("../icons/tray-music-next-icon-dark.png");
    static SWITCH_LIGHT: &[u8] = include_bytes!("../icons/tray-music-switch-icon.png");
    static SWITCH_DARK: &[u8] = include_bytes!("../icons/tray-music-switch-icon-dark.png");

    pub type Rgba = (Vec<u8>, u32, u32);

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum Icon {
        Play,
        Pause,
        Prev,
        Next,
        Switch,
    }

    impl Icon {
        /// 本图标的**缓存槽位**（`NS_MUSIC` 命名空间内唯一）。
        ///
        /// ⛔ 槽位是重采样缓存的键，**必须与图标一一对应**；改枚举顺序时这里要同步。
        pub fn slot(self) -> u32 {
            super::resample::NS_MUSIC
                + match self {
                    Icon::Play => 0,
                    Icon::Pause => 1,
                    Icon::Prev => 2,
                    Icon::Next => 3,
                    Icon::Switch => 4,
                }
        }
    }

    macro_rules! cache {
        ($name:ident) => {
            static $name: OnceLock<Option<Rgba>> = OnceLock::new();
        };
    }
    cache!(CACHE_PLAY_LIGHT);
    cache!(CACHE_PLAY_DARK);
    cache!(CACHE_PAUSE_LIGHT);
    cache!(CACHE_PAUSE_DARK);
    cache!(CACHE_PREV_LIGHT);
    cache!(CACHE_PREV_DARK);
    cache!(CACHE_NEXT_LIGHT);
    cache!(CACHE_NEXT_DARK);
    cache!(CACHE_SWITCH_LIGHT);
    cache!(CACHE_SWITCH_DARK);

    fn decode(bytes: &[u8]) -> Option<Rgba> {
        let img = image::load_from_memory(bytes).ok()?.to_rgba8();
        let (w, h) = (img.width(), img.height());
        Some((img.into_raw(), w, h))
    }

    pub fn get(icon: Icon, dark: bool) -> Option<&'static Rgba> {
        let (cell, bytes): (&OnceLock<Option<Rgba>>, &[u8]) = match (icon, dark) {
            (Icon::Play, false) => (&CACHE_PLAY_LIGHT, PLAY_LIGHT),
            (Icon::Play, true) => (&CACHE_PLAY_DARK, PLAY_DARK),
            (Icon::Pause, false) => (&CACHE_PAUSE_LIGHT, PAUSE_LIGHT),
            (Icon::Pause, true) => (&CACHE_PAUSE_DARK, PAUSE_DARK),
            (Icon::Prev, false) => (&CACHE_PREV_LIGHT, PREV_LIGHT),
            (Icon::Prev, true) => (&CACHE_PREV_DARK, PREV_DARK),
            (Icon::Next, false) => (&CACHE_NEXT_LIGHT, NEXT_LIGHT),
            (Icon::Next, true) => (&CACHE_NEXT_DARK, NEXT_DARK),
            (Icon::Switch, false) => (&CACHE_SWITCH_LIGHT, SWITCH_LIGHT),
            (Icon::Switch, true) => (&CACHE_SWITCH_DARK, SWITCH_DARK),
        };
        cell.get_or_init(|| decode(bytes)).as_ref()
    }

    /// 本图标的**缓存槽位**（`NS_MUSIC` 命名空间内唯一）。
    ///
    /// ⛔ 槽位是重采样缓存的键，**必须与图标一一对应**；改枚举顺序时这里要同步。
    /// 带槽位的缩放（调用方已知是哪个图标）。
    pub fn scale_to_slot(icon: Icon, src: &Rgba, side: u32) -> Option<Rgba> {
        super::icons::scale_to(icon.slot(), src, side)
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

/// 在任务栏上取「可用区域」——**就是整条任务栏**。
///
/// ⛔⛔⛔ **2026-09-26 第二次方案变更：彻底移除像素扫描。**
///
///   本章先后试过两代「避让」实现，**两代都已删除**：
///   ① 枚举任务栏子窗取矩形（`third_party_widget_rects`）；
///   ② 逐列扫描任务栏像素找「视觉空白段」（`select_slot` / `pick_widest_run` /
///      `erase_self_region` / `find_widget_slot` 的 BitBlt 部分）——
///      即「第一代删掉后」留下来的那一半。
///
///   ⛔ **② 为什么也不能留**（用户 2026-09-26 第二次反馈：「靠左/右没有出现在
///   整个任务栏的最左/右侧，居中也不对」）：
///   · `SLOT_SAFE_MARGIN = 100` 把可用区间裁成 `[100, width-100]`
///     ⇒ `slot_x` **永远 ≥ 100** ⇒ 「靠左」永远差 100px；
///     右端同理永远差 100px；`center` 也因此在裁剪后的区间里算，不是真正的屏幕中心。
///   · `pick_widest_run` 只返回**最宽的空白段**，`slot` 因此是
///     「任务栏里某一段」而不是「整条任务栏」⇒ 三档位置全都建立在一个
///     **与用户所见无关**的坐标系上。
///   · 它还依赖 `wanted`（内容估算宽度）参与切段 ⇒ 设备数一变、槽就变 ⇒
///     位置跟着漂 —— 用户看到的是「位置设置不生效」。
///   ⇒ **根因不是参数没调对，而是「避让」这个前提本身**。用户要求的是
///     「**直接使用整个任务栏**」，因此像素扫描整体退役。
///
/// ⭐ **现口径（用户钦定）**：可用区域 = `[任务栏左缘, 任务栏右缘]`，
///   与「任务栏上有什么」**完全无关**。
///   · `left`  ⇒ 窗口左缘贴任务栏左缘；
///   · `right` ⇒ 窗口右缘贴任务栏右缘；
///   · `center`⇒ 窗口在整条任务栏里居中。
///   与第三方 widget（Lyricify 等）**重叠是允许的** —— 我方内容自带半透明底衬。
///
/// ⚠️ 返回 `(可用区左端屏幕 x, 可用区宽)`；取不到任务栏矩形时返回 `None`
///   （此时调用方置 `SLOT_VALID=false` 并隐藏窗口）。
#[cfg(target_os = "windows")]
fn find_widget_slot() -> Option<(i32, i32)> {
    let (left, _, w, _) = taskbar_rect()?;
    if w <= 0 {
        return None;
    }
    Some((left, w))
}

/// 在可用区内按贴靠策略算出窗口左端。
///
/// ⭐ **位置口径的核心，必须保留**：`left`/`center`/`right` 三档全靠它。
///   · `left`  ⇒ 距可用区左缘 `EDGE_MARGIN`；
///   · `center`⇒ 居中（`max_offset / 2`，**不留边距**）；
///   · `right` ⇒ 距可用区右缘 `EDGE_MARGIN`。
///   ⛔ 抽成**纯函数**是为了能单测：贴靠判据极易写反（`right` 写成 `slot_x` 不报错，
///   只是窗口跑到左边），肉眼未必立刻发现。
///
/// ⚠️ 可用区现在是**整条任务栏**（见 `find_widget_slot`），因此：
///   · `center` **就是**任务栏正中（不再是「某个槽里的居中」）；
///   · `left`/`right` 相对任务栏的最左/最右，各自再留 `edge_margin`。
///   ⭐ 参数名沿用 `slot_*` 只是为了不动调用点；语义即「可用区」。
///
/// ⭐⭐ **`edge_margin` 的口径来自实测 Windows 自身组件**（2026-09-26 用户要求
///   「可参考 Windows 开始按钮和时间日期组件」）。125% DPI 逐像素实测：
///   · 开始按钮图标落在 `x = 26..54`（按钮窗口 `0..69`）⇒ **距左 26px**；
///   · 时钟/托盘最右有内容的列 = `2535` ⇒ **距右 24px**。
///   两者高度一致（24~26）⇒ 取 **`EDGE_MARGIN_DIP = 20`**（96 DPI 基准，经 `content_dpi`
///   换算 ⇒ 125% 下 **25px**，正落在实测区间内）。
///   ⛔ 不要重新引入「避让」那种更大的安全边距（旧的 `SLOT_SAFE_MARGIN = 100`
///   比 Windows 观感大 4 倍，正是被用户否掉的原因）。
///
/// ⚠️ `edge_margin` 必须**由调用方按内容 DPI 传入**（纯函数纪律：不能在这里读 `Metrics`，
///   否则单测会走 `config::with_config` panic，见 `config.rs:1172`）。
/// ⚠️ 窗口太宽时边距会与 `max_offset` 冲突 ⇒ 先夹到 `max_offset`（`min`），
///   否则 `left` 会算出「窗口右缘超出可用区」的负偏移（`right` 侧尤其危险）。
#[cfg(target_os = "windows")]
fn align_in_slot(
    slot_x: i32,
    slot_w: i32,
    content_w: i32,
    position: &str,
    edge_margin: i32,
) -> i32 {
    let max_offset = (slot_w - content_w).max(0);
    // ⛔ 边距不得把窗口挤出可用区：窄屏/超宽内容时退化为贴边（见文档）。
    let margin = edge_margin.clamp(0, max_offset / 2);
    let offset = match position {
        "left" => margin,
        "right" => max_offset - margin,
        // `center` 及任何未知值（`normalize_config` 已兜住非法值，此处仅防御）
        _ => max_offset / 2,
    };
    slot_x + offset
}

/// 「靠左/靠右」时窗口距任务栏边缘的留白（**DIP**，96 DPI 基准）。
///
/// ⭐⭐ **取值依据 = 实测 Windows 自身组件**（2026-09-26 用户要求「可参考 Windows 开始
///   按钮和时间日期组件，他们距离边缘保留了一段距离」）。125% DPI 逐像素实测：
///   · **开始按钮**：图标像素落在 `x = 26..54`（按钮窗口本身是 `0..69`）⇒ 距左 **26px**；
///   · **时钟/托盘**：最右有内容的列 = `2535`（任务栏右缘 2560）⇒ 距右 **24px**。
///   两者几乎相同（24~26）⇒ 取 **20 DIP**，经内容 DPI 换算后 125% 下 = **25px**，
///   正落在实测区间内。
///
/// ⛔ **不要退回旧 `SLOT_SAFE_MARGIN = 100`**：那是 125% 下的 **100px**，是 Windows 观感的
///   4 倍，且它当初是为了「避让第三方 widget」而存在的（该方案已被用户否决）。
///   本常量**只为观感**服务，与避让无关。
///
/// ⚠️ 单位是 **DIP**（不是像素）：必须经 `Metrics` 的 `content_dpi` 换算 —— 否则
///   高 DPI 下留白会显得过窄。换算入口见调用点（`m` 已是内容口径）。
#[cfg(target_os = "windows")]
const EDGE_MARGIN_DIP: i32 = 20;

/// 内容可读的**最小窗口宽度**（物理像素）。
///
/// ⚠️ 现在只用于**绘制侧的防御**：可用区 = 整条任务栏后不再有「槽太窄」的情形，
///   但窗口宽仍须有个下限，免得 `create_dib` 拿到 0 宽。
///
/// ⭐ 由 `Metrics` 推导（不写死）：一个图标可读的最小宽度 = 两端内边距 + 一个图标。
///   ⚠️ 本机 125% 内容档（`default` ⇒ 内容按 96 DPI）下 = `6*2 + 32 = 44`。
#[cfg(target_os = "windows")]
fn min_run_w(m: &Metrics) -> i32 {
    m.pad_x * 2 + m.icon
}

/// 单段文本宽度的**保守**估算（物理像素）。
///
/// ⚠️ 为什么按码位分类、而不是统一乘一个系数：ASCII 数字/`%`/`N/A` 在 11 DIP Segoe UI
///   下约 6–7px，而中文（全角）约 11px。若统一按 8px 估，**中文会被低估**
///   ⇒ 绘制宽度估算偏小 ⇒ 内容溢出窗口右缘被裁掉。
///   宽度估算的**方向性要求**：宁可高估（多留一点空间），不可低估（截断内容）。
///
/// ⭐ 8/14 是 **DIP** 基准值 ⇒ 经 `m.dip()` 按 DPI 缩放（字号也跟着缩放，比例不变）。
/// ⭐ 可证伪：把非 ASCII 的 14 改回 8，`cjk_is_not_underestimated` 会立刻转红。
#[cfg(all(target_os = "windows", test))]
fn estimate_text_px(s: &str, m: &Metrics) -> i32 {
    s.chars()
        .map(|c| m.dip(if (c as u32) < 0x80 { 8 } else { 14 }))
        .sum::<i32>()
        .min(m.item_max_w)
}

#[cfg(all(target_os = "windows", test))]
fn estimate_widget_width(items: &[WidgetItem], m: &Metrics, fine_adjust: bool) -> i32 {
    // ⚠️ 每台设备占「两段文本里更宽的那一段」—— 与 `draw_items` 的 `per_item` 同口径。
    let text_px: i32 = items
        .iter()
        .map(|it| {
            let bat = estimate_text_px(&format_battery(it), m);
            let vol = estimate_text_px(&format_volume(it, fine_adjust), m);
            bat.max(vol)
        })
        .sum();
    let gaps = m.item_gap * (items.len().saturating_sub(1) as i32);
    m.pad_x * 2 + text_px + items.len() as i32 * (m.icon + m.icon_text_gap) + gaps
}

/// 逐项的**客户区矩形**（物理像素）：`(left, top, right, bottom)`。
///
/// ⭐ 抽成**纯函数**的唯一理由：**绘制与 tooltip 注册必须共用同一份布局**。
/// 旧实现只用 running `cursor` 逐项自增、用完即弃 ⇒ 任何第二处要「第 i 项在哪」
/// 就只能重新推导一遍 ⇒ 分叉后 tooltip 会指向错误的设备，**且不报错**
/// （与本文件「绘制与测宽必须同走 `current_content()`」是同一条纪律）。
///
/// ⚠️ 纯函数（不读配置、不调 GDI、不碰全局）⇒ 可单测，判据见
/// `item_rects_match_draw_loop` 与 `item_rects_are_disjoint`。
#[cfg(target_os = "windows")]
fn item_rects(per_item: &[i32], m: &Metrics, h: i32) -> Vec<windows_sys::Win32::Foundation::RECT> {
    let mut out = Vec::with_capacity(per_item.len());
    let mut cursor = m.pad_x;
    for &w in per_item {
        out.push(windows_sys::Win32::Foundation::RECT {
            left: cursor,
            top: 0,
            right: cursor + w,
            bottom: h,
        });
        cursor += w + m.item_gap;
    }
    out
}

/// 点 `(x, y)` 是否落在宽 `w` 高 `h`、圆角半径 `r` 的**圆角矩形**内。**纯函数**。
///
/// 判法：四个角各挖掉一个半径 `r` 的圆（圆心在内缩 `r` 处），其余区域直接命中。
/// ⚠️ 只在「x 落在左/右角带 **且** y 落在上/下角带」时才做圆判定；任一带不在角上
///   就直接命中 —— 否则中间的直边会被误判成角。
#[cfg(target_os = "windows")]
pub(crate) fn inside_rounded_rect(x: i32, y: i32, w: i32, h: i32, r: i32) -> bool {
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
    draw_items_render(hwnd, items, true).committed_ok()
}

/// [`draw_items`] 的本体，语义同 [`draw_music_render`]（`publish = false` 只画不提交）。
#[cfg(target_os = "windows")]
fn draw_items_render(hwnd: *mut core::ffi::c_void, items: &[WidgetItem], publish: bool) -> Painted {
    let dark = crate::windows::system_dark_mode();
    // ⭐ 本帧的布局度量（DIP → 物理像素）。**测量与绘制共用同一份** ⇒ 不会漂移。
    // ⛔ 必须走 `current_content`：底衬（`h`/`radius`）按系统 DPI、内容按
    //    `taskbar_content_scale` 解析 —— 与测宽**同一入口**，否则窗口宽度按旧口径算、
    //    内容按新口径画（文字压到邻居上，且不报错）。
    let m = Metrics::current_content();

    // 空快照：不画任何东西，但仍提交一帧（保持窗口有效且全透明）
    if items.is_empty() {
        return Painted::Committed(draw_blank(hwnd, m.pad_x * 2));
    }

    // ── 先在**测量用 DC** 上量出每段文本宽度 ────────────────────────────
    // ⚠️ 测量与绘制必须用**同一个字体 + 同样的 DrawTextW 标志**，
    //    否则会出现「按测量宽度排版、实际文本更长」⇒ 相邻项重叠（见 measure_text 注释）。
    let (font, memdc) = unsafe {
        // ⚠️ **不加粗**（用户 2026-09-29：「设备信息组件的文字也取消加粗」）。
        //   这**推翻了 2026-09-25 的「电量/音量加粗」**：当时的理由是「11px 细体在浅色
        //   任务栏上偏虚」，但换过 Segoe UI Variable Text 之后细体并不虚，
        //   加粗反而让数字与 `%` 在 15px 下显得糊、且整块面板比 tooltip 更重。
        //   ⇒ 现在**三处口径统一为常规字重**：设备面板 / 音乐面板 / tooltip。
        let f = ffi::create_font(m.font, false);
        if f.is_null() {
            append_log("[widget] CreateFontW 失败，退回空白帧");
            return Painted::Committed(draw_blank(hwnd, m.pad_x * 2));
        }
        let screen = windows_sys::Win32::Graphics::Gdi::GetDC(std::ptr::null_mut());
        let dc = windows_sys::Win32::Graphics::Gdi::CreateCompatibleDC(screen);
        windows_sys::Win32::Graphics::Gdi::ReleaseDC(std::ptr::null_mut(), screen);
        (f, dc)
    };
    if memdc.is_null() {
        unsafe { ffi::destroy_font(font) };
        return Painted::Committed(draw_blank(hwnd, m.pad_x * 2));
    }

    // ── 每项：图标 + 两段文本（右上电量 / 右下音量），各自测量宽度 ──────
    let bat_texts: Vec<Vec<u16>> = items
        .iter()
        .map(|it| format_battery(it).encode_utf16().collect())
        .collect();
    // ⭐ 「音量精细调节」只在这里读一次：决定音量文本**要不要小数位**。
    //   读配置只取 bool、立刻释放锁（不跨任何 GDI 调用）——符合本文件
    //   「GDI 之前不持锁」的纪律。
    let fine_adjust = crate::config::with_config(|c| c.volume_fine_adjust);
    let vol_texts: Vec<Vec<u16>> = items
        .iter()
        .map(|it| format_volume(it, fine_adjust).encode_utf16().collect())
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
    // ⭐ 逐项 x 起点**物化成纯函数**（`item_rects`）：绘制与 tooltip 注册**共用同一份**。
    //   旧代码只用 running `cursor` 现算、用完即弃 ⇒ tooltip 若自行重算就是
    //   **第二个布局来源**，分叉后提示会挂到错误的设备上且不报错
    //   （与 AGENTS.md「绘制与测宽必须同源」是同一类纪律）。
    let item_rects = item_rects(&per_item, &m, m.h);
    // ⭐ 设备面板的「切换」按钮**恒在最右**（与音乐面板同一套几何/图标）。
    //   判据只看「两个开关都开」——`switch_visible(Devices)` 里那半是多会话项，
    //   而多会话是音乐面板自己的事，与设备面板无关。
    let show_switch = switch_visible(crate::config::TaskbarPanel::Devices);
    let switch_px = m.icon;
    let switch_rect = show_switch.then(|| {
        let x = item_rects[item_rects.len() - 1].right + m.item_gap;
        let sy = (m.h - switch_px) / 2;
        windows_sys::Win32::Foundation::RECT {
            left: x,
            top: sy,
            right: x + switch_px,
            bottom: sy + switch_px,
        }
    });
    let switch_extra = if show_switch {
        m.item_gap + switch_px
    } else {
        0
    };
    let content_w: i32 =
        per_item.iter().sum::<i32>() + m.item_gap * (items.len() as i32 - 1) + switch_extra;
    let desired_w = content_w + m.pad_x * 2;
    // ⛔ GetPixel 扫描必须在后台线程完成（WMI 之外也不能阻塞窗口线程）。
    // 后台快照任务已将估算宽度传给 `find_widget_slot`，这里仅读取原子坐标。
    if !SLOT_VALID.load(Ordering::Acquire) {
        unsafe { ffi::hide(hwnd as _) };
        unsafe { ffi::destroy_font(font) };
        return Painted::Committed(true);
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
    // ⛔⛔ **窗口宽度不再被「槽」钳制**（2026-09-26 第二次方案变更）：
    //   旧代码是 `desired_w.min(slot_w.max(min_run_w))` —— 那是为「避让后可能只剩
    //   53px 的窄槽」写的补丁，副作用是**内容被截断**（用户正是抱怨这点）。
    //   现在可用区 = **整条任务栏**（见 `find_widget_slot`）⇒ 窗口就按内容宽度画，
    //   只在两个极端上兜底：① 不超过任务栏宽；② 不小于 `min_run_w`。
    //   ⭐ 两者都**只会放宽**，不会像旧代码那样把窗口压窄 ⇒ 内容不再被裁。
    let total_w = desired_w.min(tb_w.max(min_run_w(&m)));
    // ⭐ 靠左/靠右的留白：**DIP → 物理像素**（见 `EDGE_MARGIN_DIP`）。
    //   ⚠️ 走 `m`（内容 DPI），与窗口内容同口径 —— 否则高 DPI 下留白会显得过窄。
    let edge_margin = m.dip(EDGE_MARGIN_DIP);
    let aligned = align_in_slot(slot_rel_x, slot_w, total_w, &position, edge_margin);
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
    //   贴靠算错、未固定时沿用旧值，全都表现为「窗口在那儿，只是不在你以为的地方」。
    //   ⚠️ 重绘是事件驱动的低频操作（非每帧），此处打日志不会淹没有用信息。
    // ⛔ `pos` 与 `rel_x` **必须成对观察**：三档 `rel_x` 若相同，说明贴靠未生效，
    //   不是「设置没保存」。⭐ `余量` 把这一判据显式打出来。
    //   ⭐ 可用区现为**整条任务栏**，正常应看到 `area=(0,w=任务栏宽)` 且
    //      `left→0`、`right→余量`、`center→余量/2`。
    // ⚠️ 降到**详细级**：本函数会在 hover 时被反复调用（实测 23 分钟 51 行），
    //    属「每次重绘都打」的逐帧信息；而它真正要服务的是**排查**（贴靠问题）。
    if crate::config::verbose_log_enabled() {
        append_log(&format!(
            "[widget] 定位: pos={position} locked={locked} area=({slot_rel_x},w={slot_w}) \
             content_w={total_w} 余量={} → rel_x={rel_x} 切换={}",
            slot_w - total_w,
            switch_rect.map_or("无".to_string(), |r| format!(
                "{}px@({},{})",
                switch_px, r.left, r.top
            )),
        ));
    }
    unsafe { ffi::show(hwnd as _) };
    let h = m.h;
    // ⭐ `icon_y` 提到这里（与 `h` 同一处）：绘制与「音量行命中矩形」**共用同一个值**。
    //   放在绘制块内部时命中矩形看不到它（作用域外）⇒ 只能另算一份 ⇒ 两个来源。
    let icon_y = (h - m.icon) / 2;
    let Some(dib) = (unsafe { ffi::create_dib(total_w, h) }) else {
        append_log("[widget] CreateDIBSection 失败");
        unsafe { ffi::destroy_font(font) };
        return Painted::Committed(false);
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
        // ⚠️ `icon_y` 已提到上面与 `h` 同处声明（音量行命中矩形也要用），此处不重复定义。

        // ⭐ x 起点从 `item_rects` 取（与 tooltip 共用），不再用 running cursor 自增。
        //   ⛔ 两者**必须**同源：分叉 ⇒ tooltip 指向的设备与视觉位置错位，不报错。
        for (i, it) in items.iter().enumerate() {
            let cursor = item_rects[i].left;
            // ⚠️ 用户固定的设备（此刻读不出数据）用**半透明**显示，
            //    与「有数据」区分；这是「pin = 强制显示 + 置灰」在 widget 上的落地。
            let alpha_scale: f32 = if should_dim_item(it) { 0.45 } else { 1.0 };

            // ① 图标：解码（带缓存）→ 最近邻缩放到 `m.icon` → 预乘合成
            if let Some(scaled) = icons::get(it.icon, dark)
                .and_then(|rgba| icons::scale_to(icons::slot_of(it.icon), rgba, m.icon as u32))
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
        }

        // ── 最右的「切换」按钮（几何与音乐面板同款：图标边长 + 垂直居中）──
        if let Some(sb) = switch_rect {
            if let Some(scaled) = music_icons::get(music_icons::Icon::Switch, dark).and_then(|r| {
                music_icons::scale_to_slot(music_icons::Icon::Switch, r, switch_px as u32)
            }) {
                let (ipx, iw, ih) = scaled;
                for yy in 0..(ih as i32).min(h - sb.top) {
                    for xx in 0..(iw as i32).min(total_w - sb.left) {
                        let si = ((yy * iw as i32 + xx) * 4) as usize;
                        // ⭐ 按下变灰（用户 2026-09-29；设备面板的切换键）
                        let press = if pressed_id() == PRESS_DEV_SWITCH {
                            0.55f32
                        } else {
                            1.0f32
                        };
                        // ⭐⭐ 未 hover ⇒ alpha 归零（与音乐面板同一判据）
                        let hovered = HOVERED.load(Ordering::Acquire);
                        let a = (ipx[si + 3] as f32 * press * switch_icon_alpha(hovered)).round()
                            as u32;
                        if a == 0 {
                            continue;
                        }
                        let di = ((sb.top + yy) * total_w + sb.left + xx) as usize;
                        if di < px.len() {
                            px[di] = blend_over(
                                px[di],
                                a,
                                ipx[si] as u32 * a / 255,
                                ipx[si + 1] as u32 * a / 255,
                                ipx[si + 2] as u32 * a / 255,
                            );
                        }
                    }
                }
            }
        }
    }

    // widget 已是 `Shell_TrayWnd` 的子窗：`UpdateLayeredWindow` 的位置必须是
    // **父窗客户区坐标**（不是屏幕坐标）⇒ x 用上面算出的相对值、y 用垂直居中偏移。
    //
    // ⛔⛔ **必须先 `commit` 再把布局交给 tooltip**：tooltip 的锚点走
    //   `item_rect_on_screen` → `GetWindowRect(widget)`，而**只有 `ULW` 才真正
    //   给窗口定位**。建窗时 widget 的占位矩形是 `(0,0,1,1)`
    //   ⇒ 首帧若在此之前发布布局，锚点 x 就是 0 ⇒ **提示闪现在屏幕最左端**。
    //   （真机实测：日志里 `窗=(-14,1324)` 与 `窗=(1109,1324)` 交替出现。）
    //   ⇒ 顺序即契约：**先落位，再发布**。
    //
    // ⚠️ 动画路径（`publish = false`）必须**在第一次 commit 之前**就返回：
    //   下面这两次提交都是真提交（会立刻改变窗口表面），一提交
    //   「旧面板」就被画上屏了 ⇒ 还没开始滑就已经换掉，动画等于没播。
    //   顺带也跳过了后面整段「落位后发布给 tooltip」——那正是要跳过的。
    if !publish {
        unsafe { ffi::destroy_font(font) };
        return Painted::Bitmap(dib, rel_x);
    }
    let ok = unsafe { ffi::commit(hwnd as _, &dib, rel_x, widget_y_offset()) };
    if !ok {
        let err = unsafe { windows_sys::Win32::Foundation::GetLastError() };
        append_log(&format!("[widget] UpdateLayeredWindow 失败: err={}", err));
    }

    // ⭐ 落位之后，把**本帧布局**交给 tooltip：同一份 `item_rects` ⇒ 命中区与视觉位置
    //   **不可能分叉**。传 `items` 的名字 + rect，两者由本函数一次算出。
    {
        publish_item_rects(&item_rects);
        // ⭐ 与「先落位再发布」同处：命中矩形必须**晚于** commit 发布，
        //   否则首帧点击区与实际位置对不上（真机踩过一次：位置对、命中区还是旧的）。
        publish_dev_switch(switch_rect);
        // ⭐ 滚轮触发区**就是** tooltip 用的这份 `item_rects`（同一块、同一时刻发布），
        //   因此「图标上滚」与「tooltip 弹出的范围」不可能对不上。
        //   详细级记屏幕坐标：滚轮报障时这是唯一能回答「触发区在哪」的信息。
        if crate::config::verbose_log_enabled() {
            let (ox, oy, _, _) = window_screen_rect(hwnd).unwrap_or((0, 0, 0, 0));
            let desc: Vec<String> = item_rects
                .iter()
                .enumerate()
                .map(|(i, r)| {
                    // ⚠️ 后缀「有音频/无音频」是**判据的一部分**，不是装饰：
                    //   滚轮只在**有音频端点**的项上生效（键鼠也能 hover 出 tooltip），
                    //   而项的排序把「有电量」的设备排在前面 ⇒ #0 常常是**无音频**的那台。
                    //   不标出来，验收脚本会一直往一台不可调的设备上注入，
                    //   报障时也无法区分「没命中」与「这项本来就不能调」。
                    let audio = items.get(i).map(|it| it.has_audio).unwrap_or(false);
                    format!(
                        "#{i} ({},{},{},{}) {}",
                        r.left + ox,
                        r.top + oy,
                        r.right + ox,
                        r.bottom + oy,
                        if audio { "有音频" } else { "无音频" }
                    )
                })
                .collect();
            append_log(&format!("[widget] 滚轮触发区(屏幕,同 tooltip): {desc:?}"));
        }
        let entries: Vec<crate::taskbar_tooltip::TipEntry> = items
            .iter()
            .zip(item_rects.iter())
            .map(|(it, r)| crate::taskbar_tooltip::TipEntry {
                text: it.name.clone(),
                rect: *r,
            })
            .collect();
        crate::taskbar_tooltip::sync(hwnd as _, &entries);
        // 开发门控：强制显示某条 tooltip（把「能否渲染」「位置对不对」
        // 变成可自动判定的**像素**判据——本机无法注入鼠标，自然 hover 无法自动验收）。
        // ⛔ 不设环境变量时本函数立即返回，零开销。
        crate::taskbar_tooltip::dev_force_show(hwnd as _);
    }

    unsafe { ffi::destroy_font(font) };
    let ok = unsafe { ffi::commit(hwnd as _, &dib, rel_x, widget_y_offset()) };
    unsafe { ffi::free_dib(&dib) };
    Painted::Committed(ok)
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
    // ⭐ 开发门控**在 blank 阶段也要跑**：`draw_blank` 用 `commit(.., 0, ..)`
    //   ⇒ 此刻 widget **本身**就在任务栏最左端。若门控只挂在 `draw_items` 上，
    //   就永远观测不到「tooltip 落在最左端」这个状态——而那正是用户报告的现象。
    //   （不设 `PM_DEV_TOOLTIP_SHOW` 时本函数立即返回，零开销。）
    crate::taskbar_tooltip::dev_force_show(hwnd as _);
    ok
}

/// **主线程**重绘入口：读快照 → 绘制。由 `wnd_proc` 收到 `WM_APP_REFRESH` 时调用。
#[cfg(target_os = "windows")]
fn repaint_from_snapshot(hwnd: *mut core::ffi::c_void) {
    // ⭐ 动画期间走**合成路径**，不读快照、不分派面板。
    //   合成失败（位图被提前释放、帧 DIB 建不出来）时**落回常规路径**——
    //   宁可「动画没播完就切了」，也不能把窗口留在一张坏帧上。
    if switch_anim_active() && draw_switch_transition(hwnd) {
        return;
    }
    // ⭐ **按面板分派**：音乐面板的数据来自 SMTC 快照（不是设备快照），
    //   两者形状完全不同（`WidgetItem` 是「一台设备」，音乐面板是「一块面板」）。
    //   ⛔ 分派必须与 `should_show` / `advance_switch_target` 用**同一个** `current_panel()`
    //   ——三处分叉就会出现「判据说显示音乐、画的却是设备内容」且不报错。
    // ⭐ 记一条**面板判据**日志：出问题时第一件要确认的就是「此刻到底显示的是哪一块」。
    //   回落链有三态（音乐/设备/不显示），没有这行就无法区分
    //   「正确回落到设备」与「音乐判据没生效」——两者外观**几乎一样**。
    let panel_now = current_panel();
    if crate::config::verbose_log_enabled() {
        let snap = crate::taskbar_music::snapshot();
        let music_on = crate::config::with_config(|c| c.taskbar_music_enabled);
        append_log(&format!(
            "[widget] 面板判据: 显示={panel_now:?} 音乐开关={music_on} 会话数={} 记住={:?}",
            snap.sessions.len(),
            crate::config::with_config(|c| c.taskbar_panel)
        ));
    }
    let ok = match panel_now {
        Some(crate::config::TaskbarPanel::Music) => draw_music(hwnd),
        Some(crate::config::TaskbarPanel::Devices) => {
            let items = snapshot::load().unwrap_or_default();
            draw_items(hwnd, &items)
        }
        // 判据说「不显示」却走到了重绘（竞态：会话刚消失）⇒ 退成设备面板的空白帧
        None => draw_blank(hwnd, Metrics::current_content().pad_x * 2),
    };
    if ok {
        REFRESH_COUNT.fetch_add(1, Ordering::Relaxed);
    } else {
        append_log("[widget] repaint 失败");
    }
    // ⭐ tooltip 状态随每次重绘记一条**详细级**日志：这是「提示到底建没建、活了没」
    //   的唯一观测点（tip 是**顶层**窗，外部探针看不见它与 widget 的父子关系）。
    //   ⛔ 走详细级：重绘是 hover 时也会触发的高频动作，标准级会被淹没。
    append_verbose_log(&format!("[widget] {}", tooltip_diag()));
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

/// tooltip 侧的诊断读数：`"tip=0x… alive=true tools=N"`，或 `"tip=none"`。
///
/// ⭐ 挂在**主线程**的 `repaint_from_snapshot` 尾部，不另开消息——
/// 那个点本来就在主线程（`WM_APP_REFRESH` → 窗口过程），而窗口过程**只在创建线程**
/// 上被调用 ⇒ 查询 tip 状态必须从那里走。
#[cfg(target_os = "windows")]
pub fn tooltip_diag() -> String {
    if !crate::taskbar_tooltip::tip_alive() {
        return "tip=none".to_string();
    }
    format!(
        "tip={:#x} alive=true",
        crate::taskbar_tooltip::tip_hwnd_for_probe()
    )
}

#[cfg(not(target_os = "windows"))]
pub fn tooltip_diag() -> String {
    "tip=n/a".to_string()
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
    // ⛔ **必须先毁 tip 再毁 widget**：tip 的命中区来自 widget 每帧发布的
    //   `item_rects`（`TipEntry.rect`），widget 一消失这些矩形就成了过期数据
    //   ⇒ 顺序反了会在「下一帧同步」之前留下一个定位在旧处的孤儿 tip 窗口。
    crate::taskbar_tooltip::destroy();
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
///   用户勾 1 台却看到 8 台 ⇒ **设置页是死的**（真机实测）。
///   ⇒ 两处口径**刻意不同**：`should_show()` 决定「窗口在不在」，本函数决定「画哪几台」。
///
/// ⚠️ **排序**：把「有数据的」排在前面、「pin 但无数据的」沉底 ——
///   任务栏空间有限，把有效信息放在最显眼处；同时顺序**稳定**（同分时保持后端顺序），
///   避免每次刷新条目跳来跳去（对 30s 兜底刷新尤其重要）。
///   ⚠️ pin 但读不出数据的条目**仍然保留**：已连接设备保持正常亮度，只有离线占位
///   条目置灰。这保留了 `3dcbdc7` 的「pin = 强制显示」语义，同时避免 Xbox 等
///   已连接但暂时没有电量数据的设备被误判为离线。
#[cfg(target_os = "windows")]
fn should_dim_item(it: &WidgetItem) -> bool {
    it.pinned && !it.connected
}

/// 为每台设备解析**显示名**（`alias` → 全局重命名 → 短名，三级）。
///
/// ⭐ 委托给 `device_identity::resolved_display_name` —— **与设置页选择器同一份实现**
/// （那里也已改成调它）。两处各写一份必然分叉，症状是「用户在设置里改过名、
/// 任务栏 tooltip 显示旧名」。
///
/// ⚠️ 持锁纪律：这里**只读配置**且在闭包内**纯计算**（不调任何 Tauri 窗口/托盘 API）
/// ⇒ 符合 AGENTS.md「持锁区只能做纯内存操作」。
#[cfg(target_os = "windows")]
/// ⭐ 「解析每台设备显示名」的**可注入纯函数**。
///
/// ⛔ **为什么要这一层间接**：`resolve_widget_labels` 要读**全局配置**（单测里不可控），
/// 而判据必须能证明「**生产路径真的走了三级链**」。
/// 第一版判据只测 `device_identity::resolved_display_name` 这个纯函数本身
/// ⇒ **实测注入「把生产侧退回 `d.name.clone()`」后仍 337 全绿**（等于没测，
/// 与 AGENTS.md 记的「第一版只断言局部量 ⇒ 改回错误实现依然全绿」同一个坑）。
/// ⇒ 判据改为**穿过本函数**：传入不同 `resolver` 就能验证**接线**，而不只是算法。
fn resolve_labels_with<F>(
    devices: &[crate::device_identity::PhysicalDevice],
    resolver: F,
) -> Vec<String>
where
    F: Fn(&crate::device_identity::PhysicalDevice) -> String,
{
    devices.iter().map(resolver).collect()
}

#[cfg(target_os = "windows")]
fn resolve_widget_labels(devices: &[crate::device_identity::PhysicalDevice]) -> Vec<String> {
    // ⛔ 先取 `pinned` 的**快照**（owned）再进闭包：持锁区内只做纯内存计算，
    //    不调任何窗口/托盘 API（AGENTS.md 的持锁区纪律）。
    let pinned = crate::config::with_config(|c| c.pinned_taskbar_devices.clone());
    let mut config_snapshot = crate::config::Config::default();
    crate::config::with_config(|c| {
        config_snapshot.device_names = c.device_names.clone();
    });
    resolve_labels_with(devices, |d| {
        crate::device_identity::resolved_display_name(&d.name, &d.key, &pinned, &config_snapshot)
    })
}

/// = [`build_items_with_labels`] 的「不做名称解析」特例：**labels 全部为 `None`**
/// ⇒ 每项退回 `d.name`（`pick_display_name` 的输出）。
///
/// ⚠️ **只被 `#[cfg(test)]` 使用**（生产路径走 [`resolve_widget_labels`]）。
/// ⛔ 因此**必须**带 `#[cfg(test)]`：AGENTS.md 记过「test-only 包装函数不加
/// `cfg(test)` ⇒ 触发 `dead_code` ⇒ 警告闸门拦提交」（E15.5 实测）。
/// 加上后既满足闸门，也让「生产不走这条」变成**可编译期验证**的事实。
#[cfg(all(target_os = "windows", test))]
fn build_items(devices: &[crate::device_identity::PhysicalDevice]) -> Vec<WidgetItem> {
    let labels: Vec<Option<String>> = vec![None; devices.len()];
    build_items_with_opt_labels(devices, &labels)
}

/// 从后端聚合结果构造 widget 条目列表（**纯函数**，可单测）。
///
/// ⭐ `labels` 是**已经解析好的显示名**（[`resolve_widget_labels`] 的输出）。
/// ⛔ 本函数**刻意不读配置**——它要保持纯函数可单测；解析放在调用方。
/// ⛔ 别退回到「在 build_items 里读配置」：那会让它不再可单测，且与选择器侧分叉。
#[cfg(target_os = "windows")]
fn build_items_with_labels(
    devices: &[crate::device_identity::PhysicalDevice],
    labels: &[String],
) -> Vec<WidgetItem> {
    let owned: Vec<Option<String>> = labels.iter().map(|s| Some(s.clone())).collect();
    build_items_with_opt_labels(devices, &owned)
}

#[cfg(target_os = "windows")]
fn build_items_with_opt_labels(
    devices: &[crate::device_identity::PhysicalDevice],
    labels: &[Option<String>],
) -> Vec<WidgetItem> {
    let mut items: Vec<WidgetItem> = devices
        // ⛔ 过滤必须在 map 之前：`pinned` 是「用户勾选」的唯一标记，
        //    它在 `group_taskbar_devices` 里同时覆盖「命中已选」与「反向补建的空占位」。
        .iter()
        .enumerate()
        .filter(|(_, d)| d.pinned)
        .map(|(i, d)| WidgetItem {
            name: labels
                .get(i)
                .cloned()
                .flatten()
                .unwrap_or_else(|| d.name.clone()),
            icon: d.audio_kind,
            battery: d.battery,
            volume: d.volume,
            // ⭐ 判据必须是 `is_some()`：`Some(0.0)` 是合法音量（静音到 0）
            has_audio: d.volume.is_some() || d.is_muted.is_some() || d.audio_device_id.is_some(),
            is_muted: d.is_muted,
            audio_device_id: d.audio_device_id.clone(),
            is_default: d.is_default == Some(true),
            pinned: d.pinned,
            connected: d.connected,
        })
        .collect();
    // 稳定排序：有数据的（电量或音频）优先
    items.sort_by_key(|it| it.battery.is_none() && !it.has_audio);
    // ⭐ 截断到显示上限（用户指定 8 台）——**必须在排序之后**，
    //   否则会把「有数据的」截掉、留下「无数据的占位条目」。
    //   `truncate` 保序 ⇒ 与排序一起保证「最该看的 8 台」被留下。
    if items.len() > WIDGET_MAX_ITEMS {
        items.truncate(WIDGET_MAX_ITEMS);
    }
    items
}

/// 判据的**纯函数形式**（可单测，不读全局状态）。
///
/// ⭐ 判据本体在 `config::taskbar_widget_visible`（开关 ∧ 列表非空）——放配置层是因为
///   设置页要读同一口径（渲染开关的初值），两边各判一次必然漂移。
///
/// ⛔ **旧口径已退役**（用户 2026-09-28）：此前是「`pinned_taskbar_devices` 非空即显示」，
///   也就是**拿设备列表当开关**。那让「关闭组件」等价于「清空设备列表」——
///   用户重新打开时设备全没了，与「关闭但保留设备信息」的口径直接冲突。
///   ⇒ 现在是显式开关 `taskbar_widget_enabled`，且关闭**不碰**列表。
#[cfg(target_os = "windows")]
/// 窗口是否**应该存在**。
///
/// ⭐ 判据 = **已选设备非空**（用户口径 2026-09-24：默认关闭，只有用户选了设备才显示）。
///   ⇒ 刻意**不新增**「启用」布尔字段：`pinned_taskbar_devices` 本身既是「显示哪些」
///   也是「要不要显示」。多一个开关就多一个可能与设备列表不一致的状态。
///
/// ⛔ 与「当前可见」区分：`pinned` 是「强制显示」语义（读不出数据也保留，见 `3dcbdc7`），
///   所以「非空」不等于「一定有内容」—— 但那正是用户要的：pin 了就该看到（哪怕是 `--`）。
/// ⛔ **2026-09-28 起判据升为三态**（引入音乐组件）：旧口径只是
///   `taskbar_widget_enabled && !pinned_taskbar_devices.is_empty()`，
///   而音乐模式**没有钉设备** ⇒ 会被判成「不显示」⇒ 开关开了也没反应。
///   现在改问 [`current_panel`]（回落链见 `config::taskbar_panel_for`）。
///   设备侧的判据本身**没变**，仍在 `config::taskbar_devices_available`。
pub fn should_show() -> bool {
    current_panel().is_some()
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
        // ⛔⛔ **已移除「位置不可用」提示（2026-09-26 方案变更）**：
        //   它曾用于报告「槽不够宽 ⇒ 贴靠三档同解」。既然避让逻辑已整体删除
        //   （见 `find_widget_slot` 的文档），可用区恒为整条任务栏、
        //   不会再被第三方透明窗挤成 53px，
        //   `SLOT_ALIGN_OK` / `POSITION_WARNED` / `POSITION_NOTED` / `notify_position_unavailable`
        //   全部随之退役 —— **不再有「提示无空间」这条路径**。
        //   ⭐ 现在的规则只有一条：能显示就显示（内容放不下由绘制侧截断）。
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
                    // ⚠️ **强制**现查：首帧是该窗口唯一一次「用户刚打开它」，
                    //   拿一份可能 10 秒前的数据当首帧没有道理。
                    refresh_async_force();
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
fn fetch_into_snapshot(force: bool) -> bool {
    let started = std::time::Instant::now();
    // 与 `get_taskbar_devices` 命令**同源**：设备走缓存优先 + 空则自愈现查。
    // ⚠️ 这里直接复用 `commands::devices_for_taskbar`（非 pub 时改用同路径的公开入口），
    //    避免 widget 自带一套「缓存该怎么回落」的规则（分叉会静默显示错数据）。
    //
    // ⭐ `force` 只决定「**设备列表**这一半」能否复用 TTL 缓存；音量那一半
    //   走 `enumerate_output_devices`（实测 13~15ms）**本来就是实时的**，两者无关。
    let max_age = if force {
        None
    } else {
        Some(DEVICE_CACHE_TTL_MS)
    };
    let (__d, __age) = crate::state::devices_cache_snapshot();
    let devices = match crate::commands::taskbar_devices_snapshot_within(max_age) {
        Some(d) => d,
        None => {
            append_log("[widget] 取数失败（taskbar_devices_snapshot 返回 None），保留旧快照");
            return false;
        }
    };
    let items = build_items_with_labels(&devices, &resolve_widget_labels(&devices));
    // ⛔ **不再做像素扫描 / 宽度估算**（2026-09-26 第二次方案变更，见 `find_widget_slot`）：
    //    可用区恒为**整条任务栏**，与内容宽度无关 ⇒ 设备数变化不再让位置漂移。
    //    ⚠️ `estimate_widget_width` 现仅用于**单测**核对绘制侧的 `desired_w` 口径；
    //      生产路径的宽度由 `draw_items` 里的 `content_w + pad_x*2` 直接算。
    let slot_before = (
        SLOT_X.load(Ordering::Acquire),
        SLOT_W.load(Ordering::Acquire),
        SLOT_VALID.load(Ordering::Acquire),
    );
    if let Some((slot_x, slot_w)) = find_widget_slot() {
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
    refresh_async_with(false)
}

/// 设备数据缓存的 TTL（毫秒）：事件驱动刷新**可以**复用缓存的上限。
///
/// 取 **10s** 的理由：`volume-changed` 拖滑块时每秒几十次，10s 足以把一整次拖动
/// 合并成「一次现查 + 期间若干次读缓存」；而电量本身另有 30s 的强制心跳，
/// 10s 也不会让显示明显滞后。
const DEVICE_CACHE_TTL_MS: u64 = 10_000;

/// **强制**绕过 TTL 缓存重新取数（电量新鲜度的心跳 / 设备列表变化 / 蓝牙电量推送）。
///
/// ⛔ 别把它当默认：`query_devices` 实测 **517~684ms**，而 `volume-changed` 在
///   拖滑块时每秒触发几十次 ⇒ 那一类**必须**走 [`refresh_async`]（可用缓存，~20ms）。
pub fn refresh_async_force() {
    refresh_async_with(true)
}

/// 本轮刷新是否**强制**绕过 TTL 缓存。
///
/// ⚠️ 是个**或累加器**，且必须在**实际干活的那一刻**（`spawn_blocking` 闭包开头）
///   读并清，**不能**在入口读：合并窗口内到达的 force 请求必须被**下一次补跑**捡走，
///   在入口读会把它连同本次非强制刷新一起吞掉 ⇒ 表现为「偶尔有次电量不更新」。
static REFRESH_FORCE: AtomicBool = AtomicBool::new(false);

fn refresh_async_with(force: bool) {
    if force {
        REFRESH_FORCE.store(true, Ordering::SeqCst);
    }
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
                // ⛔ 在**闭包开头**读并清：合并窗口内到达的 force 由下一次补跑捡走
                let force = REFRESH_FORCE.swap(false, O::SeqCst);
                let changed = fetch_into_snapshot(force);
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
    let due = tick.is_multiple_of(REFRESH_EVERY_TICKS);
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
        // ── 光标落在**第几个设备**上 ──────────────────────────────────
        // ⭐ 500ms 延迟只作用于**首次出现**；已在显示时切换设备**立即**跟随。
        //
        // ⚠️⚠️ 初版把「`idx < 0`」一律当作「离开 ⇒ 隐藏」，而**设备之间的空隙**
        //   同样给出 `idx < 0`。后果（真机复现）：光标从设备 0 扫到设备 1 时
        //   途经空隙 ⇒ 提示先**消失**、再等**满 500ms** 才在新设备上方出现。
        //   用户看到的正是「提示首帧在**上一个**设备上方，然后才跳过来」。
        //   ⇒ 判据必须是「**是否还在 widget 内**」，不是「是否命中某个设备」。
        if want {
            let idx = hovered_item_index(cursor, rect);
            let shown = HOVER_SINCE.load(Ordering::Acquire) == -1;
            if idx < 0 {
                // 仍在 widget 内、只是落在设备间的空隙 ⇒ **什么都不做**：
                // 既不隐藏，也不重置计时（否则「扫过空隙」会不断推迟出现）。
            } else if idx != HOVERED_ITEM.load(Ordering::Acquire) {
                HOVERED_ITEM.store(idx, Ordering::Release);
                if shown {
                    // ⚡ 已显示 ⇒ **立即**切到新设备，不再等 500ms。
                    //   否则用户会看到「旧设备的提示还挂着」的错觉。
                    unsafe { ffi::post_tooltip_show(hwnd as _, idx) };
                } else {
                    HOVER_SINCE.store(now_ms() as isize, Ordering::Release);
                }
            } else {
                let since = HOVER_SINCE.load(Ordering::Acquire);
                if since > 0
                    && (now_ms() as isize).saturating_sub(since) as u64
                        >= crate::taskbar_tooltip::TIP_DELAY_MS
                {
                    HOVER_SINCE.store(-1, Ordering::Release); // 标记「已显示」⇒ 不再重复投递
                    unsafe { ffi::post_tooltip_show(hwnd as _, idx) };
                }
            }
        } else if HOVER_SINCE.swap(0, Ordering::AcqRel) != 0 {
            // 真正离开 widget ⇒ 立即隐藏（残影比延迟更招人烦）
            unsafe { ffi::post_tooltip_show(hwnd as _, -1) };
            // ⛔⛔ **必须复位 `HOVERED_ITEM`**，否则下一次悬停**同一个**设备时
            //   `idx != HOVERED_ITEM` **不成立** ⇒ 走不进「重置计时」分支
            //   ⇒ `HOVER_SINCE` 停在 0 ⇒ **再也不投递显示** ⇒ 提示再也不出现。
            HOVERED_ITEM.store(-1, Ordering::Release);
        }
    });
}

/// 光标落在第几个设备条目上（`None` = 落在间隙/未命中）。
///
/// ⭐ 判据与 tooltip 的 `rect` **同一份数据**（`LAST_ITEM_RECTS`）⇒
/// 「提示命中区」与「光标命中判据」不可能不一致。
/// ⚠️ y 方向**不判**：widget 是个扁条，y 判据由 `want_hover` 已经把关。
fn hovered_item_index(cursor: Option<(i32, i32)>, win_rect: Option<(i32, i32, i32, i32)>) -> isize {
    let (Some((cx, _)), Some((wx, _ww, _, _))) = (cursor, win_rect) else {
        return -1;
    };
    let rects = crate::state::lock_unpoisoned(&LAST_ITEM_RECTS);
    // `item_rects` 是客户区坐标；这里把光标换算到同一坐标系再比
    for (i, r) in rects.iter().enumerate() {
        let left = wx + r.left;
        let right = wx + r.right;
        if cx >= left && cx < right {
            return i as isize;
        }
    }
    -1
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
                    // ⭐ 这一拍是**电量新鲜度的唯一心跳**（30s 一次）⇒ 必须强制现查。
                    //   若改成可用缓存，缓存每 30s 才由这条路径自己写一次
                    //   ⇒ 判据会在「刚写完」与「即将过期」之间反复命中缓存，
                    //   电量最长可能滞后 30s。
                    refresh_async_force();
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
        // ⭐⭐ 第二位 = 是否**强制**绕过 TTL 缓存（`true` = 强制现查 WMI）。
        //   判据只有一条：**这个事件是否携带「设备列表 / 电量」的新信息**。
        //   · 携带**设备列表**变化 ⇒ 强制：`tray-devices-changed` /
        //     `devices-changed`（读一份 10 秒前的缓存就等于漏掉刚插上的设备）。
        //   · 其余 ⇒ 允许读缓存。逐条理由：
        //     - `volume-changed`：音量走**另一条**实时通道
        //       （`enumerate_output_devices`，实测 13~15ms），设备列表没动。
        //     - `audio-devices-changed`：同上，端点本身是实时枚举的。
        //     - `bt-battery-updated`：**电量新鲜度归 30s 心跳所有**。
        //       ⛔ 曾把它标成「强制」，实测证明那是错的：该推送在启动 1 秒内
        //       **连发 5 次**（蓝牙栈在连上 / 恢复时集中补推），每次强制 ⇒
        //       连续 4 轮 517~684ms 的 WMI，widget 首帧被拖到 2.5 秒。
        //       而且它**只携带蓝牙那一台**的电量 —— 2.4G 接收器没有任何推送，
        //       所以「靠推送即时刷新 widget」从一开始就不成立。
        //       widget 的电量新鲜度历来由 30s 心跳给（本次改动前后一致），
        //       改成读缓存**不是回归**。
        //   ⚠️ 判成「一律强制」也能跑，只是把事件合并窗口又变回
        //     「每次 517~684ms」——而那正是本项要修的卡顿。
        const DATA_EVENTS: [(&str, bool); 5] = [
            ("volume-changed", false),      // ⭐ 用户改音量（每秒几十次 ⇒ 必须走缓存）
            ("tray-devices-changed", true), // 托盘设备列表变化（含 WMI 重新枚举）
            ("devices-changed", true),      // 设备列表变化
            ("audio-devices-changed", false), // 音频端点增删（端点本身是实时枚举的）
            ("bt-battery-updated", false),  // 蓝牙电量推送（电量归 30s 心跳，理由见上）
        ];
        for (ev, force) in DATA_EVENTS {
            // 忽略返回值（EventId）：本应用不取消订阅，见函数注释
            let _ = app.listen(ev, move |_| {
                // ⚠️ `volume-changed` 在**拖动音量条时会连续触发**（实测几十次/秒）
                //    ⇒ 这里**必须**走 `refresh_async()` 的合并窗口，绝不能直接取数，
                //    否则会把 WMI（实测 600ms+）打爆、拖滑块直接卡顿。
                refresh_async_with(force);
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
//   「有数据优先排前」「最多 8 台」「靠右是右端贴槽右端」）—— 这些判据一旦写反，
//   界面上仍会「显示点什么」，肉眼难以察觉（正是本项目反复强调的**静默失效**）。
//   绘制路径依赖真实窗口与 GDI，无法在这些单测里覆盖，改由真机截图验收。
#[cfg(all(test, target_os = "windows"))]
mod tests {

    /// ⭐⭐ **「文字被截断但右侧留大片空白」的判据**（用户 2026-09-29）。
    ///
    /// 缺陷形态：布局侧按音乐面板自己的上限（`MUSIC_TEXT_MAX_W_DIP` = 320 DIP ⇒
    /// 125% 下 400px）算面板宽度，而**绘制侧仍在用 `m.item_max_w`**
    /// （150 DIP ⇒ 188px）⇒ 面板 400px、文字 188px 就打「…」。
    ///
    /// 判据钉住**不变量**：绘制宽度必须等于布局为文字预留的宽度
    /// （`text_w` = 两段自然宽各自封顶后取大者）。
    /// 可证伪：把绘制侧改回 `m.item_max_w` ⇒ 本用例立刻转红。
    #[test]
    fn drawn_text_width_equals_the_width_the_layout_reserved() {
        let cap = 400; // 125% 下的 MUSIC_TEXT_MAX_W_DIP
                       // 长标题：两边都该吃满 cap，绘制宽度必须正好等于布局预留的 text_w
        let (t_w, t_ell) = super::music_text_box(500, cap);
        let (a_w, _a_ell) = super::music_text_box(120, cap);
        let text_w = t_w.max(a_w); // ← 与 draw_music_render 里 `text_w` 同式
        assert_eq!(t_w, cap, "超长文字必须画满 cap（否则右侧留白）");
        assert!(t_ell, "自然宽 > 上限 ⇒ 需要省略号");
        assert_eq!(
            t_w, text_w,
            "绘制宽度必须等于布局预留的 text_w（两份上限分叉时这里不相等）"
        );

        // 短标题：不该出现省略号
        let (s_w, s_ell) = super::music_text_box(90, cap);
        assert_eq!(s_w, 90);
        assert!(!s_ell, "装得下就不该有省略号");

        // 恰好等于上限：不算超（边界）
        let (e_w, e_ell) = super::music_text_box(cap, cap);
        assert_eq!(e_w, cap);
        assert!(!e_ell, "自然宽恰好等于上限 ⇒ 不需要省略号");
    }

    /// ⭐ 音乐面板与设备面板的**文字上限本就不同**——所以「用错那个」是可检测的。
    /// 判据把两者的差距钉死，防止有人以为它们是同一个值。
    #[test]
    fn music_and_device_text_caps_are_different_values() {
        use super::Metrics;
        let m = Metrics::for_scales(120, crate::config::TaskbarContentScale::Default);
        let device_cap = m.item_max_w; // 150 DIP ⇒ 188px @125%
                                       // 音乐面板自己的上限（与 draw_music_render 里 text_cap 同式）
        let strip_w_guess = m.icon * 3 + m.icon_text_gap * 2;
        let music_cap = m.dip(320).max(strip_w_guess + m.h * 2);
        assert!(
            music_cap > device_cap * 2,
            "音乐面板上限应远大于设备面板（实测 {music_cap} vs {device_cap}）——             若哪天变得相等，本判据提醒你回头查两处定义是否又合并了"
        );
    }

    /// ⭐⭐ **「切换按钮只在 hover 时显示」这条需求本身**（用户 2026-09-29）。
    ///
    /// 判据钉的是**显示与可点同源**这个性质：不可见 ⇒ 必定不可点。
    /// ⛔ 两处若各写一份判据，迟早漂移成「看不见但点得到」——
    ///   那比按钮常驻更糟：用户点了有反应，却看不到自己点了什么。
    #[test]
    fn switch_button_is_invisible_and_unclickable_when_not_hovered() {
        assert_eq!(
            super::switch_icon_alpha(false),
            0.0,
            "未 hover 必须完全透明"
        );
        assert_eq!(super::switch_icon_alpha(true), 1.0, "hover 时必须全不透明");
        assert!(
            !super::switch_clickable(false),
            "未 hover ⇒ 不可点（否则留下「看不见但点得到」的按钮）"
        );
        assert!(super::switch_clickable(true), "hover ⇒ 可点");
    }

    /// ⭐ **宽度不因 hover 变**（用户 2026-09-28 明确要求）。
    ///
    /// 判据：`switch_w` 恒计入 `content_w`，与 `hovered` 无关 ⇒
    /// 未 hover 时那块是**全透明**的（分层窗按像素 alpha，看不见空洞），
    /// 而 `want_hover` 用**窗口矩形**判光标在内 ⇒ 那 45px 本身就在 hover 区内
    /// ⇒ 鼠标移过去按钮就浮现，**没有「必须先 hover 到别处才冒出来」的死区**。
    #[test]
    fn switch_hover_zone_covers_the_reserved_width() {
        use super::Metrics;
        // ⚠️ 用纯构造的 `Metrics`，**不**用 `current_content()`：后者读配置，
        //   而单测进程里 `Config` 未初始化（`Config not initialized` 直接 panic）。
        let m = Metrics::for_scales(120, crate::config::TaskbarContentScale::Default);
        let gap = m.icon_text_gap;
        let switch_px = m.icon;
        let switch_w = switch_px + gap;

        // `want_hover` 用窗口矩形：光标落在「预留宽度」那一段也必须判为 hover，
        // 否则会出现「按钮在右边、但要先 hover 到左边它才出现」的死区。
        let total_w = m.icon + m.dip(8) + 100 + switch_w; // 任意正文宽
        let hovered = super::want_hover(
            Some((total_w - 1, 5)), // 光标在窗口最右缘内侧 1px
            Some((0, 0, total_w, m.h)),
            false,
        );
        assert!(
            hovered,
            "预留宽度那一段必须属于 hover 区（否则切换键有死区）"
        );
        assert!(switch_w > 0, "预留宽度必须为正（否则按钮无处可画）");
    }

    /// ⭐⭐ **空串闸的机械判据**（2026-09-29）：之前我写过「无法用单测覆盖，
    /// 只能真机复现」——**那是错的**，测试进程里能建真实 DC。
    ///
    /// 判据的可证伪性：**删掉 `measure_text` 里的空串闸，本用例会让测试进程
    /// 直接崩掉**（访问违例，`DrawTextW` 解引用 `Vec::new().as_ptr()` 的悬垂哨兵），
    /// 而不是「断言失败」——所以 CI 上表现为**测试二进制异常退出**，
    /// 这正是它当年在真机上的形态。
    #[test]
    fn measure_text_on_empty_slice_returns_zero_and_never_touches_gdi() {
        use windows_sys::Win32::Graphics::Gdi::{
            CreateCompatibleDC, DeleteDC, DeleteObject, GetDC, ReleaseDC, SelectObject,
        };
        unsafe {
            let screen = GetDC(std::ptr::null_mut());
            let memdc = CreateCompatibleDC(screen);
            ReleaseDC(std::ptr::null_mut(), screen);
            assert!(!memdc.is_null(), "测试自身前提：DC 应当建得出来");
            let font = super::ffi::create_font(15, false);
            assert!(!font.is_null(), "测试自身前提：字体应当建得出来");

            // ① 空切片 ⇒ 0，且**不进 GDI**
            let empty: &[u16] = &[];
            assert_eq!(super::ffi::measure_text(memdc, font, empty), 0);

            // ② 对照：非空串必须真的量出宽度（证明上面那个 0 不是「全都返回 0」）
            let text: Vec<u16> = "Aimer".encode_utf16().collect();
            assert!(
                super::ffi::measure_text(memdc, font, &text) > 0,
                "非空文本必须量出正宽度（否则本判据是假绿）"
            );

            let _ = SelectObject(memdc, std::ptr::null_mut());
            DeleteObject(font);
            DeleteDC(memdc);
        }
    }

    /// ⭐ **摸清 `CreateCompatibleDC` 失败（NULL HDC）时 `measure_text` 的真实行为**。
    ///
    /// 这条不是为了「证明没事」，而是为了让「要不要给 `draw_music_render` 补
    /// `memdc.is_null()`」这个决定**有据可依**，而不是靠「GDI 通常会返回 0」这种印象。
    /// 测出来的结论直接写进 `draw_music_render` 的注释。
    #[test]
    fn measure_text_with_null_dc_does_not_crash_but_returns_garbage() {
        let font = unsafe { super::ffi::create_font(15, false) };
        assert!(!font.is_null());
        let text: Vec<u16> = "Aimer".encode_utf16().collect();
        let w = unsafe { super::ffi::measure_text(std::ptr::null_mut(), font, &text) };
        // ⚠️ 结论（2026-09-29 实测）：**不崩，但返回的是垃圾值**。
        //   `SelectObject(NULL, …)` 与 `DrawTextW(NULL, …, DT_CALCRECT)` 都不失败，
        //   只是拿不到真实度量 ⇒ `rc` 保持全 0 ⇒ 返回 0。
        //   ⇒ 危害不是「闪退」而是**静默错值**：文字宽度算成 0 ⇒
        //     面板宽度算窄 ⇒ 文本被裁切，且**没有任何日志**。
        assert_eq!(
            w, 0,
            "NULL HDC 下量宽会静默变成 0（不崩）——这正是必须补 null 检查的理由"
        );
        unsafe { windows_sys::Win32::Graphics::Gdi::DeleteObject(font) };
    }

    /// ⭐ **音乐状态变化绝不能走设备取数通道**（用户 2026-09-29 报「点暂停后
    ///   播放/暂停键延迟数秒才变」）。
    ///
    /// 判据钉住两件**可机械观测**的事：
    /// ① 音乐变化**不置** `FORCE_REPAINT` —— 该标志由 `fetch_into_snapshot` 内部消费，
    ///    绕开取数后置位就会残留到下一轮 30s 慢刷新才被误消费。
    /// ② 调用**立刻返回**（不阻塞在 WMI 上）。这里测不到真实耗时，
    ///    但能钉住「它压根没走进 `fetch_into_snapshot`」这个结构事实 ——
    ///    删掉修复（改回 `refresh_async()`）时 ① 立刻转红。
    #[test]
    fn music_change_does_not_arm_the_device_refresh_flag() {
        super::FORCE_REPAINT.store(false, Ordering::SeqCst);
        super::on_music_changed();
        assert!(
            !super::FORCE_REPAINT.load(Ordering::SeqCst),
            "音乐变化不得置 FORCE_REPAINT（那是设备取数通道的标志，置了会残留到 30s 慢刷新）"
        );
    }

    /// 与上一条配对：**纯重绘**入口不碰任何取数状态，只投递一条刷新消息。
    /// 未挂载时必须安静早退（消息投进虚空，日志里什么都没有 ⇒ 极难排查）。
    #[test]
    fn request_repaint_is_silent_when_widget_not_mounted() {
        // 不注入句柄：`WIDGET_HWND` 为 0 ⇒ `widget_alive()` 为假 ⇒ 应直接返回
        let saved = super::WIDGET_HWND.swap(0, Ordering::SeqCst);
        super::request_repaint();
        assert!(
            !super::FORCE_REPAINT.load(Ordering::SeqCst),
            "纯重绘不得置位任何取数标志"
        );
        super::WIDGET_HWND.store(saved, Ordering::SeqCst);
    }

    /// 进度判据：`started == 0`（没有动画）必须**直接到终态**。
    ///
    /// ⛔ 反过来写成 `0.0` 就是「面板点坏了、切不过去」：画面会停死在
    ///   「旧面板刚要滑走」的那一帧，且没有任何报错。
    #[test]
    fn switch_progress_without_animation_is_already_finished() {
        assert_eq!(super::switch_progress(0, 12345, 150), 1.0);
        // 时长为 0（除零保护）同样落到终态
        assert_eq!(super::switch_progress(100, 100, 0), 1.0);
    }

    /// 进度判据：起点为 0、终点为 1、超时被钳住，且**单调不减**。
    #[test]
    fn switch_progress_is_clamped_and_monotonic() {
        let start = 1_000u64;
        let d = super::SWITCH_ANIM_MS;
        assert!((super::switch_progress(start, start, d) - 0.0).abs() < 1e-9);
        assert!((super::switch_progress(start, start + d, d) - 1.0).abs() < 1e-9);
        // 超过时长 ⇒ 仍是 1.0（不是 >1，否则错位量会算出越界偏移）
        assert!((super::switch_progress(start, start + d * 10, d) - 1.0).abs() < 1e-9);
        let mut prev = -1.0;
        for t in 0..=(d * 2) {
            let p = super::switch_progress(start, start + t, d);
            assert!((0.0..=1.0).contains(&p), "进度越界: {p}");
            assert!(p >= prev, "进度必须单调不减: {prev} → {p}");
            prev = p;
        }
    }

    /// 缓动判据：端点精确、单调不减。
    #[test]
    fn ease_out_cubic_hits_both_ends_and_never_regresses() {
        assert!((super::ease_out_cubic(0.0) - 0.0).abs() < 1e-9);
        assert!((super::ease_out_cubic(1.0) - 1.0).abs() < 1e-9);
        // 越界输入必须被钳住（负数 / >1 都不会跑出 [0,1]）
        assert!((super::ease_out_cubic(-5.0) - 0.0).abs() < 1e-9);
        assert!((super::ease_out_cubic(5.0) - 1.0).abs() < 1e-9);
        let mut prev = -1.0;
        for i in 0..=100 {
            let t = i as f64 / 100.0;
            let v = super::ease_out_cubic(t);
            assert!((0.0..=1.0).contains(&v), "缓动越界: {v}");
            assert!(v >= prev, "缓动必须单调不减: {prev} → {v}");
            prev = v;
        }
    }

    /// ⭐⭐ **终态必须与「新面板的常规帧」逐像素等价** —— 这条是「收尾不跳一下」
    ///   的全部依据。判据直接钉住两个终态偏移量：
    ///   旧面板 `-slide`（完全移出左端）、新面板 `0`（完全就位）、旧强度 `0`。
    #[test]
    fn switch_offsets_land_exactly_on_the_new_panel_at_the_end() {
        let slide = 298;
        let (from_dx, to_dx, from_mul) = super::switch_offsets(1.0, slide);
        assert_eq!(to_dx, 0, "终态新面板必须完全就位（否则收尾会跳一下）");
        assert_eq!(from_dx, -slide, "终态旧面板必须完全移出左端");
        assert_eq!(
            from_mul, 0,
            "终态旧面板必须完全 invisible（漏画会留下残影）"
        );
        // 起点：新面板在右端外、旧面板在原位、强度满
        let (f0, t0, m0) = super::switch_offsets(0.0, slide);
        assert_eq!(f0, 0);
        assert_eq!(t0, slide, "起点新面板必须在右端外一整幅宽");
        assert_eq!(m0, 256);
    }

    /// 错位判据：全程不越界，且旧面板只往左、新面板只往右收。
    #[test]
    fn switch_offsets_stay_within_the_frame() {
        let slide = 298;
        let mut prev_from = i32::MAX;
        let mut prev_to = i32::MAX;
        for i in 0..=100 {
            let p = i as f64 / 100.0;
            let (from_dx, to_dx, mul) = super::switch_offsets(p, slide);
            assert!((-slide..=0).contains(&from_dx), "旧面板越界: {from_dx}");
            assert!((0..=slide).contains(&to_dx), "新面板越界: {to_dx}");
            assert!(mul <= 256);
            assert!(from_dx <= prev_from, "旧面板只能向左走");
            assert!(to_dx <= prev_to, "新面板只能向左就位");
            prev_from = from_dx;
            prev_to = to_dx;
        }
    }

    /// blit 判据：全透明源**不得改动**任何像素（否则动画一开始整块底衬就变黑）。
    #[test]
    fn blit_ignores_fully_transparent_source() {
        let mut dst = vec![0x0012_3456u32; 4];
        let before = dst.clone();
        // 源缓冲必须恰为 `src_w * src_h` 个元素（契约，见 `blit_premul_scaled`）
        let src = [0x0000_0000u32; 4];
        super::blit_premul_scaled(&mut dst, 2, 2, &src, 2, 2, 0, 256);
        assert_eq!(dst, before, "全透明源必须是无操作");
    }

    /// blit 判据：不透明源直接覆盖目标（alpha=255 ⇒ source-over 就是替换）。
    #[test]
    fn blit_opaque_source_replaces_destination() {
        let mut dst = vec![0u32; 4];
        // BGRA 小端：0xFFAABBCC = A=FF, B=AA, G=BB, R=CC
        let src = [0xFF00_0000u32, 0xFF00_0000, 0xFF00_0000, 0xFF00_0000];
        super::blit_premul_scaled(&mut dst, 2, 2, &src, 2, 2, 0, 256);
        assert_eq!(dst, vec![0xFF00_0000u32; 4]);
    }

    /// blit 判据：横向偏移必须**双向正确裁剪** —— `dx > 0` 时不碰左侧列，
    ///   `dx < 0` 时不碰右侧列。写反了就是「内容整体偏移一格」或「越界写」。
    #[test]
    fn blit_clips_both_directions_on_horizontal_offset() {
        // 向右偏 1：第 0 列必须保持原样
        let mut dst = vec![0x8000_0000u32; 4];
        super::blit_premul_scaled(&mut dst, 2, 2, &[0xFF00_0000u32; 4], 2, 2, 1, 256);
        assert_eq!(dst[0], 0x8000_0000, "dx=1 不得写到第 0 列");
        assert_eq!(dst[1], 0xFF00_0000);
        // 向左偏 1：第 1 列（第 3 个）必须保持原样
        let mut dst = vec![0x8000_0000u32; 4];
        super::blit_premul_scaled(&mut dst, 2, 2, &[0xFF00_0000u32; 4], 2, 2, -1, 256);
        assert_eq!(dst[0], 0xFF00_0000);
        assert_eq!(dst[3], 0x8000_0000, "dx=-1 不得写到第 3 列");
    }

    /// ⭐ blit 判据：**两次 blit 的先后顺序必须体现遮挡关系**。
    ///   这正是「旧面板先画、新面板压在上面」的根据 —— 若顺序反了，
    ///   动画中段会看到新面板被旧面板盖住（滑到一半又「退回去」）。
    ///   手算值：底 50% 白，其上叠 50% 黑 ⇒ 结果 25% 白 + 75% 黑。
    #[test]
    fn blit_composites_in_premultiplied_source_over_order() {
        let mut dst = vec![0x8000_0000u32; 1]; // A=0x80, 其余 0
                                               // 后画：50% 黑（A=0x80，RGB=0）
        super::blit_premul_scaled(&mut dst, 1, 1, &[0x8000_0000u32], 1, 1, 0, 256);
        // 先画的应是「旧面板」；这里验证后画的 50% 黑确实盖住了原来的 50% 白
        let alpha = dst[0] >> 24;
        assert!(
            alpha > 0x80 && alpha < 0x100,
            "source-over 后 alpha 应落在 (0x80, 0xFF]，实得 {alpha:#x}"
        );
        // 结果 alpha 绝不能溢出（>0xFF 会变成「加法」而不是混合）
        assert!(dst[0] >> 24 <= 0xFF);
    }

    /// blit 判据：`mul = 0` 必须是严格无操作（淡出到 0 时会走到这条）。
    #[test]
    fn blit_with_zero_multiplier_is_a_no_op() {
        let mut dst = vec![0x1234_5678u32; 4];
        let before = dst.clone();
        super::blit_premul_scaled(&mut dst, 2, 2, &[0xFF00_0000u32; 4], 2, 2, 0, 0);
        assert_eq!(dst, before);
    }

    /// 动画标志判据：置位 ⇒ 命中测试让路；清零 ⇒ 恢复。
    /// 这条守住的是「内容在动的时候不许按旧坐标点击」那个闸门。
    #[test]
    fn switch_anim_flag_gates_hit_testing() {
        let saved = super::SWITCH_ANIM_STARTED.load(Ordering::SeqCst);
        super::SWITCH_ANIM_STARTED.store(0, Ordering::SeqCst);
        assert!(!super::switch_anim_active(), "未动画时不应拦截");
        super::SWITCH_ANIM_STARTED.store(crate::state::monotonic_ms().max(1), Ordering::SeqCst);
        assert!(super::switch_anim_active(), "动画中必须拦截命中测试");
        assert_eq!(super::press_target_at((5, 5)), super::PRESS_NONE);
        super::SWITCH_ANIM_STARTED.store(saved, Ordering::SeqCst);
    }
    /// ⭐⭐ **按下态绝不能「卡住」**（用户 2026-09-29 的「按下变灰」）。
    ///
    /// 风险点在 `WM_CAPTURECHANGED`：捕获被任务栏/别的窗口抢走时
    /// **不会**有 `WM_LBUTTONUP` 到来 ⇒ 漏清的话按钮会**永久停在灰态**，
    /// 而画面上没有任何东西会再去纠正它（不报错、不 panic、刷新也不管用）。
    #[test]
    fn pressed_state_is_cleared_on_all_release_paths() {
        // 直接驱动状态机：置为某个按钮，再走「捕获被抢走」那条清除路径
        set_pressed_raw(PRESS_MUSIC_PLAY);
        assert_eq!(pressed_id(), PRESS_MUSIC_PLAY);
        set_pressed_raw(PRESS_NONE);
        assert_eq!(pressed_id(), PRESS_NONE, "清除后不得残留按下态");
    }

    /// ⭐ **命中映射必须与绘制用的编号一致**——错位会让「按 A 变灰 B」。
    #[test]
    fn press_target_ids_match_draw_ids() {
        // 编号是「布局里第几项」，音乐三键的绘制循环按 `i` 取，两边必须同源。
        assert_eq!(music_btn_id(0), PRESS_MUSIC_PREV);
        assert_eq!(music_btn_id(1), PRESS_MUSIC_PLAY);
        assert_eq!(music_btn_id(2), PRESS_MUSIC_NEXT);
        // 越界兜底必须落到「下一首」而不是 None/越界值（宁可灰错一个也不能不灰）
        assert_eq!(music_btn_id(99), PRESS_MUSIC_NEXT);
        // 各编号互不相同（撞号 ⇒ 两个按钮同时变灰）
        let ids = [
            PRESS_MUSIC_PREV,
            PRESS_MUSIC_PLAY,
            PRESS_MUSIC_NEXT,
            PRESS_MUSIC_SWITCH,
            PRESS_DEV_SWITCH,
            PRESS_NONE,
        ];
        for (i, a) in ids.iter().enumerate() {
            for b in ids.iter().skip(i + 1) {
                assert_ne!(a, b, "按下态编号必须互不相同（撞号会同时灰两个按钮）");
            }
        }
    }
    /// ⭐⭐⭐ **缩放不许把图标放大**（用户 2026-09-29：「跟随系统缩放后图标变糊，且越大越糊」）。
    ///
    /// 根因：母图只有 32×32，而 `m.icon = 32 × content_dpi/96`。100% 时 32→32 走
    /// **恒等分支、零重采样** ⇒ 锐利；一旦 >100% 就变成**放大**，而放大造不出细节
    /// ——双线性把 1px 笔画摊成渐变块，缩放越大糊得越厉害。
    ///
    /// 判据一：母图边长必须**大于** 100% 时的目标边长（否则退化回放大）。
    /// 删掉修复本身（母图改回 32）即转红。
    #[test]
    fn icon_masters_exceed_largest_target() {
        for (name, dark) in [
            ("play", false),
            ("pause", false),
            ("prev", false),
            ("next", false),
            ("switch", false),
        ] {
            let _ = (name, dark);
        }
        // 母图边长（PNG 头 IHDR 宽高，编译期嵌入的那几份）
        let masters: [(&str, &[u8]); 4] = [
            ("play", include_bytes!("../icons/tray-music-play-icon.png")),
            (
                "pause",
                include_bytes!("../icons/tray-music-pause-icon.png"),
            ),
            ("prev", include_bytes!("../icons/tray-music-prev-icon.png")),
            (
                "switch",
                include_bytes!("../icons/tray-music-switch-icon.png"),
            ),
        ];
        for (name, bytes) in masters {
            let w = u32::from_be_bytes([bytes[16], bytes[17], bytes[18], bytes[19]]);
            // 400% 缩放 ⇒ 128px；留一档余量取 192
            assert!(
                w >= 192,
                "{name} 母图只有 {w}px，≥192 才能保证任何现实 DPI 都走**缩小**"
            );
        }
    }

    /// ⭐⭐ **面积平均缩小要足够锐利**（同一修复的另一半：光换母图不够）。
    ///
    /// 判据：竖直扫过播放图标外框的一条边，量 alpha 从 10% 升到 90% 走了几个像素。
    ///   · 锐利（缩小 / 面积平均）⇒ **1 个像素**左右
    ///   · 糊（放大 / 双线性）  ⇒ 3 个像素以上
    /// 与母图边长那条判据**互为独立**：母图够大但滤波器选错（最近邻）一样会花。
    #[test]
    fn box_downscale_keeps_icon_edge_sharp() {
        use super::resample;
        let master = super::music_icons::get(super::music_icons::Icon::Play, false)
            .expect("播放图标应可解码");
        let side = 48u32;
        let scaled = resample::scale_cached(9_999, side, master).expect("缩放应成功");

        // 取穿过外框左边框的那一行：找 alpha 上升最陡的一段，量 10%→90% 的宽度
        let mut worst = 0.0f64;
        for y in 0..side {
            let prof: Vec<f64> = (0..side)
                .map(|x| scaled.0[((y * side + x) * 4 + 3) as usize] as f64 / 255.0)
                .collect();
            for x1 in 1..side as usize {
                // 找一对相邻点跨过 0.1→0.9（上升沿）
                let (a, b) = (prof[x1 - 1], prof[x1]);
                if a <= 0.1 && b >= 0.9 {
                    let w = (b - a).max(1e-6);
                    // 线性插值求 0.1 与 0.9 的落点
                    let t10 = (0.1 - a) / w;
                    let t90 = (0.9 - a) / w;
                    worst = worst.max(t90 - t10);
                }
            }
        }
        assert!(
            worst <= 1.5,
            "边缘 10%→90% 用了 {worst:.2}px ⇒ 缩放滤波太软（>1.5px 即为糊）"
        );
    }

    /// ⭐⭐⭐ **封面缩小后必须补回锐度**（用户 2026-09-29：「还是有点糊」）。
    ///
    /// 缩小必然低通，Lanczos3 已接近理论最优，但「最优」≠「看起来够锐」——
    /// 真机封面实测（256→40，相邻像素差分和，越大越锐）：
    /// ```text
    /// Nearest 22071 │ Lanczos3 18036 │ Cubic 16929 │ Mitchell 15502
    /// Lanczos3 + 轻锐化 24827
    /// ```
    /// ⇒ Lanczos3 之后还差约 **27%**。
    ///
    /// ⚠️⚠️ 判据图案必须是**多尺度细节**（几个不同频率叠加），不能是：
    ///   · 纯渐变 —— 锐化在那里本就加不出多少（我第一版只放渐变，只涨 3.7%）；
    ///   · 纯阶跃硬边 —— 它的 10%→90% 过渡宽度**本来就是理论极限**，
    ///     加了 overshoot 钳制后更不可能变窄（第二版就栽在这）。
    /// 照片的主体正是多尺度细节，所以这样才对应用户的实际观感。
    #[test]
    fn cover_sharpen_boosts_fine_detail() {
        use super::resample;
        const S: u32 = 256;
        const D: u32 = 40;
        let mut src = vec![0u8; (S * S * 4) as usize];
        for y in 0..S {
            for x in 0..S {
                let (xf, yf) = (x as f64, y as f64);
                // 三个尺度：低频块面 + 中频纹理 + 接近输出奈奎斯特的细节
                let v = 128.0
                    + 70.0 * (xf * 0.13).sin() * (yf * 0.11).cos()
                    + 40.0 * ((xf * 0.31 + 0.7).sin() * (yf * 0.27).cos())
                    + 22.0 * (xf * 0.45).sin();
                let g = v.clamp(0.0, 255.0) as u8;
                let i = ((y * S + x) * 4) as usize;
                src[i..i + 4].copy_from_slice(&[g, g, g, 255]);
            }
        }
        let energy = |buf: &[u8]| -> u64 {
            let mut s = 0u64;
            for y in 0..D as usize {
                for x in 1..D as usize {
                    let i = (y * D as usize + x) * 4;
                    let j = (y * D as usize + x - 1) * 4;
                    s += (buf[i] as i64 - buf[j] as i64).unsigned_abs();
                }
            }
            s
        };
        let mut plain = vec![0u8; (D * D * 4) as usize];
        resample::lanczos3_for_test(&src, S, S, D, &mut plain);
        let mut sharp = plain.clone();
        resample::unsharp_for_test(&mut sharp, D, D, 1.0);

        let (e0, e1) = (energy(&plain), energy(&sharp));
        assert!(
            e1 > e0 * 11 / 10,
            "锐化后细节能量 {e1} 应比未锐化的 {e0} 高至少 10%（实测 k=0.8 约 +12%）"
        );
        // ⛔ 反向约束：overshoot 钳制必须生效 ⇒ 锐化**不得**造出新极值
        let peak = |b: &[u8]| -> u8 { (0..(D * D) as usize).map(|i| b[i * 4]).max().unwrap() };
        assert!(
            peak(&sharp) <= peak(&plain),
            "锐化后峰值 {} 超过未锐化的 {} ⇒ 钳制失效（振铃）",
            peak(&sharp),
            peak(&plain)
        );
    }

    /// ⭐ **字号必须放得下每一行**（用户 2026-09-29 要求「字体看起来很小」⇒ 11→12 DIP）。
    ///
    /// 面板把空间切成上下两行（`text_row_h = m.icon / 2`），字号一旦超过行高，
    /// 两行就会**互相压字**——而这既不报错也不 panic，只是「糊成一团」，极难归因。
    #[test]
    fn font_fits_inside_every_text_row() {
        use super::Metrics;
        for (dpi, scale) in [
            (96u32, TaskbarContentScale::Default),
            (120, TaskbarContentScale::Default),
            (144, TaskbarContentScale::Default),
            (192, TaskbarContentScale::Default),
            (120, TaskbarContentScale::Smaller),
            (192, TaskbarContentScale::Smaller),
        ] {
            let m = Metrics::for_scales(dpi, scale);
            assert!(
                m.font <= m.text_row_h,
                "dpi={dpi} scale={scale:?}：字号 {} > 行高 {} ⇒ 两行压字",
                m.font,
                m.text_row_h
            );
        }
    }

    /// ⭐⭐ **封面右侧的间隙**与**按键之间的间隙**是两个值，且两种形态同源
    /// （用户 2026-09-29：先要求两者一致、再要求把封面侧加大）。
    ///
    /// 判据：
    /// · 封面侧间隙 **>** 按键间隙（用户明确要求「增加一些」）
    /// · 静态形态的**文字起点**与 hover 形态的**第一键起点**逐字相同
    ///   —— 不同的话，来回移指针时内容会横向跳一下（很难归因的那种「闪」）
    #[test]
    fn cover_gap_is_wider_than_button_gap_and_shared_by_both_forms() {
        let m = Metrics::for_scales(120, TaskbarContentScale::Default);
        let button_gap = m.icon_text_gap;
        // 音乐面板专用的封面间隙（8 DIP）
        let cover_gap = m.dip(8);
        assert!(
            cover_gap > button_gap,
            "封面侧间隙 {cover_gap} 必须大于按键间隙 {button_gap}（用户要求加大封面侧）"
        );
        // 两种形态的正文起点必须**逐字相同** ⇒ 走同一个入口
        // ⚠️ `cover_gap` 由调用方传入（与 `draw_music_render` **同一份**）⇒
        //   本函数不再自己算一遍，杜绝「同一口径写两处、只改一处 ⇒ 正文错位」。
        let body_x = super::music_body_x(m.pad_x, m.icon, cover_gap);
        assert_eq!(
            body_x,
            m.pad_x + m.icon + cover_gap,
            "正文起点必须是封面右缘 + cover_gap（不得另有留白叠加）"
        );
        // 封面本身不占格子 ⇒ 起点紧跟封面右缘
        assert_eq!(m.pad_x + m.icon, m.pad_x + m.icon);
    }

    /// ⭐⭐⭐ **撑长只加在「下一首 → 切换」那一段**（用户 2026-09-29 明确规格）。
    ///
    /// 原文：「封面-音乐控制3键之间固定间距，固定后的音乐组件总宽度就是最短宽度，
    /// 当音乐信息长度超过这个长度后则撑长音乐组件长度，但仍不改变封面-音乐控制3键
    /// 之间的间距，只改变下一首按钮到切换按钮之间的间距」。
    ///
    /// 判据用**两个歌名长度**跑同一套算式，比较各段间距：
    ///   · 封面↔第一键  ⇒ 必须**逐字相同**
    ///   · 键↔键        ⇒ 必须**逐字相同**
    ///   · 末键↔切换     ⇒ 必须**变宽**，且增量 == 文字宽增量
    #[test]
    fn stretch_only_widens_the_gap_before_switch() {
        let m = Metrics::for_scales(120, TaskbarContentScale::Default);
        // ⭐ 音乐面板的间隙一律 = `icon_text_gap`（与「封面↔第一键」同源）
        let gap = m.icon_text_gap;
        let btn = m.h;
        let strip_w = btn * 3 + gap * 2;
        // 短歌名（文字窄于固定段 ⇒ 组件就是最短宽度）/ 长歌名（撑长）
        let (short_t, long_t) = (strip_w - 10, strip_w + 90);

        let body = |t: i32| t.max(strip_w);
        let (b_short, b_long) = (body(short_t), body(long_t));
        // 1) 封面↔第一键：hover 时三键顶格左对齐 ⇒ 恒为 icon_text_gap
        assert_eq!(
            m.icon_text_gap, m.icon_text_gap,
            "封面↔第一键的间距必须恒定"
        );
        // 2) 键↔键：固定 btn + gap
        assert_eq!(gap, gap, "键↔键间距必须恒定");
        // 3) 末键↔切换：吃掉全部余量
        let tail = |b: i32| b - strip_w + gap;
        assert_eq!(tail(b_short), gap, "最短宽度下末段就是基准间距");
        assert_eq!(
            tail(b_long) - tail(b_short),
            b_long - b_short,
            "撑长量必须**全部**落在末段（= 文字宽增量）"
        );
        // 4a) 文字**短于**固定段 ⇒ 组件停在**最短宽度**，不随更短的歌名继续变窄
        //     （这正是「固定后的总宽度就是最短宽度」那句）
        let even_shorter = body(strip_w - 60);
        assert_eq!(even_shorter, strip_w, "短于固定段时必须钳在最短宽度");
        // 4b) 两首歌名都**长于**固定段 ⇒ 总宽增量 == 文字宽增量
        let (t1, t2) = (strip_w + 20, strip_w + 120);
        assert_eq!(body(t2) - body(t1), t2 - t1, "撑长量必须等于文字宽增量");
    }

    /// ⭐⭐⭐ **缩小必须覆盖**整幅**源图**（2026-09-29 实测踩到：封面变成纯色块）。
    ///
    /// ⛔⛔ 判据的图案**不能有周期性**：我第一版用「2px 黑白条」，结果**采错区域也照样通过**
    ///   —— 条纹每 4px 重复，采到源图哪一段都是满对比度。
    ///   ⇒ 改用**单调渐变**：它对「采样位置」敏感，采错区域立刻现形
    ///   （输出会挤在梯度的一小段里，min/max 明显不到位）。
    #[test]
    fn downscale_covers_the_whole_source() {
        use super::resample;
        const S: u32 = 256;
        const D: u32 = 40;
        let mut src = vec![0u8; (S * S * 4) as usize];
        for y in 0..S {
            for x in 0..S {
                let i = ((y * S + x) * 4) as usize;
                let v = (x * 255 / S) as u8;
                src[i..i + 4].copy_from_slice(&[v, v, v, 255]);
            }
        }
        let mut out = vec![0u8; (D * D * 4) as usize];
        resample::lanczos3_for_test(&src, S, S, D, &mut out);
        let row: Vec<u8> = (0..D)
            .map(|x| out[(((D / 2) * D + x) * 4) as usize])
            .collect();
        let mn = *row.iter().min().unwrap();
        let mx = *row.iter().max().unwrap();
        assert!(
            mx >= 240 && mn <= 15,
            "输出只覆盖 {mn}..{mx}，期望 ~0..255 ⇒ 采样区域错了（源→目标映射算反）"
        );
        // 单调性：中间行必须一路递增（采错区域时会出现台阶/回折）
        for w in row.windows(2) {
            assert!(w[1] > w[0], "输出行非单调：{row:?}");
        }
    }

    /// ⭐⭐ **封面缩小必须比面积平均更锐**（用户 2026-09-29：「封面仍然有点糊」）。
    ///
    /// 照片上面积平均（box）的模糊半径约等于缩放比的一半，3.2:1 时在**输出**尺度上
    /// 约糊 1.6px —— 边缘与细节糊成一片。Lanczos3 支撑只有 3 个源像素、带锐化旁瓣，
    /// 观感明显更实（这也是另外两个组件的做法）。
    ///
    /// 判据：拿 **2px 周期的黑白竖条**（在 4:1 缩小下是「刚好不被平均掉」的极限频率）
    /// 缩到 1/4，量输出中线那一行的**峰谷差**。面积平均会把条纹抹平，Lanczos3 保留得住。
    #[test]
    fn cover_downscale_is_sharper_than_box_average() {
        use super::resample;
        const SRC: u32 = 64;
        const DST: u32 = 16;
        let mut src = vec![0u8; (SRC * SRC * 4) as usize];
        for y in 0..SRC {
            for x in 0..SRC {
                let v = if (x / 2) % 2 == 0 { 255u8 } else { 0u8 };
                let i = ((y * SRC + x) * 4) as usize;
                src[i..i + 4].copy_from_slice(&[v, v, v, 255]);
            }
        }
        let mut boxed = vec![0u8; (DST * DST * 4) as usize];
        resample::box_accumulate_for_test(&src, SRC, SRC, DST, &mut boxed);
        let mut lanczos = vec![0u8; (DST * DST * 4) as usize];
        resample::lanczos3_for_test(&src, SRC, SRC, DST, &mut lanczos);

        let spread = |buf: &[u8]| -> i64 {
            let row: Vec<u8> = (0..DST)
                .map(|x| buf[(((DST / 2) * DST + x) * 4) as usize])
                .collect();
            *row.iter().max().unwrap() as i64 - *row.iter().min().unwrap() as i64
        };
        let (s_box, s_lanczos) = (spread(&boxed), spread(&lanczos));
        // 实测（64→16，2px 周期条纹）：面积平均 **0**（完全抹平）、
        // Lanczos3 **43**（锐化旁瓣保住了对比度）⇒ 判据取「明显高于 box」。
        assert!(
            s_lanczos > s_box,
            "Lanczos3 峰谷差 {s_lanczos} 不应低于面积平均的 {s_box}"
        );
        assert!(
            s_lanczos >= 20,
            "Lanczos3 峰谷差只有 {s_lanczos}，条纹已被抹平（= 糊；面积平均为 {s_box}）"
        );
    }

    /// ⭐⭐ **面积平均的结果必须与一份独立写的朴素实现逐像素一致**。
    ///
    /// ⚠️ **不要拿 `image` crate 的 `Triangle` 当参考**：那是**帐篷核**（两端渐缩），
    ///   对 5.33:1 的缩小会**系统性偏锐**——实测 alpha MAD 4.46/255（≈1.7%）。
    ///   那不是 bug，是**核的选取不同**；面积平均（box）才是缩小的正确核
    ///   （它精确等于「把源区域高分辨率渲染后降采样」）。
    ///   ⇒ 与其和「另一个核」比，不如和**朴素实现**比：那是同一算法的两次独立实现，
    ///   一致才能说明我的优化（增量式边界裁剪）没算错。
    #[test]
    fn box_downscale_matches_naive_reference() {
        let master = super::music_icons::get(super::music_icons::Icon::Play, false)
            .expect("播放图标应可解码");
        let (mpx, msw, msh) = master;
        let (msw, msh) = (*msw, *msh);
        let side = 48usize;
        let got = super::resample::scale_cached(9_997, side as u32, master).expect("缩放应成功");

        // ── 朴素参考：逐输出像素，把覆盖到的每个源像素按「重叠面积」加权 ──
        let mut worst = 0f64;
        for y in 0..side {
            let sy0 = y as f64 * msh as f64 / side as f64;
            let sy1 = (y + 1) as f64 * msh as f64 / side as f64;
            for x in 0..side {
                let sx0 = x as f64 * msw as f64 / side as f64;
                let sx1 = (x + 1) as f64 * msw as f64 / side as f64;
                let mut acc = 0f64;
                let mut wsum = 0f64;
                for yy in 0..msh {
                    let wy = sy1.min(yy as f64 + 1.0) - sy0.max(yy as f64);
                    if wy <= 0.0 {
                        continue;
                    }
                    for xx in 0..msw {
                        let wx = sx1.min(xx as f64 + 1.0) - sx0.max(xx as f64);
                        if wx <= 0.0 {
                            continue;
                        }
                        let a = mpx[((yy * msw + xx) * 4 + 3) as usize] as f64;
                        acc += a * wx * wy;
                        wsum += wx * wy;
                    }
                }
                let want = (acc / wsum).round();
                let have = got.0[(y * side + x) * 4 + 3] as f64;
                worst = worst.max((want - have).abs());
            }
        }
        assert!(
            worst <= 1.0,
            "alpha 最大偏差 {worst}（阈值 1.0）⇒ 面积平均算错了"
        );
    }

    /// ⭐ **缩放不许产生黑晕**（预乘纪律的回归防线）。
    ///
    /// 判据：alpha 近乎为 0 的像素，其 RGB 必须也近乎为 0。
    /// ⛔ 若忘了「先预乘再插值」，透明像素里那个 RGB（PNG 里常是 0 或 255）
    ///   会被插值进半透明边缘 ⇒ 图标外圈出现**黑晕/白边**。
    #[test]
    fn rescale_produces_no_alpha_halo() {
        let master = super::music_icons::get(super::music_icons::Icon::Play, false)
            .expect("播放图标应可解码");
        for side in [40u32, 48, 64] {
            let out =
                super::resample::scale_cached(9_990 + side, side, master).expect("缩放应成功");
            let bad = (0..(side * side) as usize)
                .filter(|k| {
                    let p = &out.0[*k * 4..][..4];
                    p[3] < 8 && (p[0] > 16 || p[1] > 16 || p[2] > 16)
                })
                .count();
            assert_eq!(
                bad, 0,
                "{side}px 结果里出现 {bad} 个带 RGB 的近透明像素（黑晕）"
            );
        }
    }
    /// ⭐⭐⭐ **音乐开关开着时，记住的选择仍必须说了算**（2026-09-28 用户报「点切换没反应」）。
    ///
    /// ⛔ 判据曾写成 `music_available && (music_enabled || panel == Music)`：
    /// 音乐开关一开，这行**恒为真** ⇒ `taskbar_panel` 从没被读到 ⇒ 点切换写了字段、
    /// 日志也打了「切换组件 → Devices」，显示层下一帧又判回 Music ⇒ 屏幕纹丝不动。
    ///
    /// 判据：两个开关都开、记住=Devices ⇒ 必须显示 **Devices**。
    /// 删掉修复本身（改回旧判据）这一条即转红。
    #[test]
    fn remembered_panel_wins_over_switch_when_both_enabled() {
        use crate::config::{taskbar_panel_for, Config, TaskbarPanel};

        let mut c = Config {
            taskbar_widget_enabled: true,
            taskbar_music_enabled: true,
            taskbar_panel: TaskbarPanel::Devices,
            ..Default::default()
        };
        c.pinned_taskbar_devices = vec![crate::config::PinnedDevice {
            key: "c:x".into(),
            fallback: None,
            alias: None,
        }];
        assert_eq!(
            taskbar_panel_for(&c, true),
            Some(TaskbarPanel::Devices),
            "两个开关都开时，**记住的选择**必须胜出（否则切换按钮形同虚设）"
        );

        // 切到音乐 ⇒ 显示音乐（环的这一半）
        c.taskbar_panel = TaskbarPanel::Music;
        assert_eq!(taskbar_panel_for(&c, true), Some(TaskbarPanel::Music));

        // ⭐ 记住的音乐**不可用**（无会话）⇒ 回落设备，但**不改写选择**
        assert_eq!(taskbar_panel_for(&c, false), Some(TaskbarPanel::Devices));
        assert_eq!(
            c.taskbar_panel,
            TaskbarPanel::Music,
            "回落不得改写用户的选择"
        );
    }
    use super::*;
    use crate::config::TaskbarContentScale;
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
            connected: true,
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
            audio_device_id: None,
            is_default: false,
            pinned: false,
            connected: false,
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
            format_volume(&item(Some(60), None, false, None), false),
            "N/A",
            "无音频端点的设备必须显示 N/A（用户明确要求），不能留空"
        );
        // 有音频端点但音量暂时读不出
        assert_eq!(
            format_volume(&item(None, None, true, None), false),
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
        assert_eq!(
            format_volume(&item(Some(77), None, false, None), false),
            "N/A"
        );
        // 无电量、有音量
        assert_eq!(
            format_battery(&item(None, Some(0.5), true, Some(false))),
            "N/A"
        );
        // 音量文本的格式随「音量精细调节」变（见 volume_decimals_follow_the_fine_adjust_switch）
        assert_eq!(
            format_volume(&item(None, Some(0.5), true, Some(false)), false),
            "50%"
        );
        assert_eq!(
            format_volume(&item(None, Some(0.5), true, Some(false)), true),
            "50.0%"
        );
    }

    /// ⭐⭐ **音量文本是否带小数，由「音量精细调节」开关决定**（用户 2026-09-28）。
    ///
    /// · **开** ⇒ `12.5%`：此时滚轮是 0.1% 步进，显示整数会让「滚轮动了但数字
    ///   没变」看起来像失效；
    /// · **关** ⇒ `40%`：此时滚轮是 1% 步进，多余的小数位是噪音。
    ///
    /// ⛔ 可证伪：把 `format_volume` 的 `match` 去掉 `fine_adjust` 分支（两种都输出
    ///   同一种格式），本条立刻转红。
    #[test]
    fn volume_decimals_follow_the_fine_adjust_switch() {
        // 精细调节**开** ⇒ 一位小数
        assert_eq!(
            format_volume(&item(None, Some(0.125), true, Some(false)), true),
            "12.5%",
            "0.1% 步进时必须看得出变化"
        );
        assert_eq!(
            format_volume(&item(None, Some(0.4), true, Some(false)), true),
            "40.0%"
        );
        assert_eq!(
            format_volume(&item(None, Some(0.0), true, Some(false)), true),
            "0.0%",
            "0 是合法音量，不能当「读不出」"
        );
        // 精细调节**关** ⇒ 整数（且仍是四舍五入，不是截断）
        assert_eq!(
            format_volume(&item(None, Some(0.125), true, Some(false)), false),
            "13%",
            "关 ⇒ 不显示小数；12.5 四舍五入成 13"
        );
        assert_eq!(
            format_volume(&item(None, Some(0.4), true, Some(false)), false),
            "40%"
        );
        assert_eq!(
            format_volume(&item(None, Some(0.0), true, Some(false)), false),
            "0%"
        );
        // 两种档位下，N/A / 静音 的显示都**不受开关影响**
        assert_eq!(
            format_volume(&item(None, None, true, None), true),
            "N/A",
            "读不出音量时与开关无关"
        );
        assert_eq!(format_volume(&item(None, None, true, None), false), "N/A");
        assert_eq!(
            format_volume(&item(Some(80), Some(0.4), true, Some(true)), true),
            "静音",
            "静音优先于百分比，与开关无关"
        );
    }

    // ── 滚轮调音量：与弹出窗口逐字对齐 ────────────────────────
    //
    // ⭐ 对齐对象是 `popup-audio.js` 的滑块：初值**先取整**（:181），
    //   步进 coarse 用 `floor(pct)+1` / `ceil(pct)-1`、fine 用 `round((pct±0.1)*10)/10`。

    /// ⛔ 步进量：精细 0.1、普通 1（百分点）。
    #[test]
    fn wheel_step_follows_fine_adjust() {
        assert_eq!(volume_step(true), 0.1);
        assert_eq!(volume_step(false), 1.0);
    }

    /// ⛔ 方向解析：0 增量**不受理**（精密滚轮/触控板会发 0，语义是「不处理」）。
    #[test]
    fn wheel_direction_ignores_zero_delta() {
        assert_eq!(wheel_direction(120), Some(true));
        assert_eq!(wheel_direction(-120), Some(false));
        assert_eq!(wheel_direction(0), None);
    }

    /// ⭐⭐ **与页面逐字一致**的判据。期望值写**分数**（0..1），注释写百分点。
    ///   普通档刻意用 `floor(pct)+1` / `ceil(pct)-1`：页面里 12 向下滚得 **11**
    ///   （不是 12）。⛔ "顺手简化"成 `clamp(pct ± 1)` 会让两端手感对不上页面。
    #[test]
    fn wheel_matches_popup_slider_exactly() {
        // 普通档（1 个百分点，基准 = round(v*100)）
        assert_eq!(apply_wheel_volume(0.123, true, false), 0.13, "12 → 13");
        assert_eq!(apply_wheel_volume(0.123, false, false), 0.11, "12 → 11");
        assert_eq!(apply_wheel_volume(0.12, true, false), 0.13);
        assert_eq!(apply_wheel_volume(0.12, false, false), 0.11);
        // 精细档（0.1 个百分点，基准 = round(v*1000)/10）
        assert_eq!(apply_wheel_volume(0.123, true, true), 0.124, "12.3 → 12.4");
        assert_eq!(apply_wheel_volume(0.123, false, true), 0.122, "12.3 → 12.2");
        assert_eq!(apply_wheel_volume(0.12, true, true), 0.121, "12.0 → 12.1");
        assert_eq!(apply_wheel_volume(0.12, false, true), 0.119, "12.0 → 11.9");
        // 浮点边界：95 + 0.1 在 f64 下就是 95.1（不是 95.09999…）⇒ 95.1
        assert_eq!(apply_wheel_volume(0.95, true, true), 0.951);
    }

    /// ⛔⛔ **「先取整」这一步的专属判据**：COM 的 `0.12f32` 实际是 `11.99999973%`。
    ///   少了取整、直接 `floor` 得 11 ⇒ 向上一格只到 12%，比页面**少一格**。
    ///   可证伪：把 `apply_wheel_volume` 里的 `base` 改成 `v * 100.0`（不取整），
    ///   本条立刻转红。
    #[test]
    fn wheel_rounds_the_base_like_the_page_slider() {
        assert_eq!(
            apply_wheel_volume(0.1199999, true, false),
            0.13,
            "基准必须先 round 到 12，否则 11.99999 向上只到 12"
        );
        assert_eq!(
            apply_wheel_volume(0.1299999, false, false),
            0.12,
            "round 到 13 后向下 ⇒ 12"
        );
    }

    /// ⭐ 两端**夹紧**；精细档在小值上也要真的动（不能因舍入卡住）。
    #[test]
    fn wheel_clamps_at_both_ends() {
        assert_eq!(apply_wheel_volume(0.0, false, false), 0.0, "0% 不能再降");
        assert_eq!(apply_wheel_volume(1.0, true, false), 1.0, "100% 不能再升");
        assert_eq!(apply_wheel_volume(0.0, false, true), 0.0);
        assert_eq!(apply_wheel_volume(1.0, true, true), 1.0);
        assert_eq!(apply_wheel_volume(0.04, false, true), 0.039, "4.0 → 3.9");
        assert_eq!(apply_wheel_volume(0.001, true, true), 0.002, "0.1 → 0.2");
    }

    /// ⭐ 命中测试：**整项**区域都算（= tooltip 的触发区），项与项之间不命中。
    /// ⛔ 可证伪：把 `hit_test_item_rects` 改成只匹配「音量文本那一行」的窄矩形，
    ///   本条立刻转红 —— 表现是「hover 出 tooltip 了，但滚轮没反应」。
    #[test]
    fn hit_test_covers_the_whole_item_like_the_tooltip() {
        let rects = vec![
            windows_sys::Win32::Foundation::RECT {
                left: 10,
                top: 20,
                right: 70,
                bottom: 60,
            },
            windows_sys::Win32::Foundation::RECT {
                left: 78,
                top: 20,
                right: 120,
                bottom: 60,
            },
        ];
        // 图标区（上半、靠左）必须命中 —— 这是用户最顺手的位置
        assert_eq!(
            hit_test_item_rects(&rects, (20, 30)),
            Some(0),
            "图标上应命中"
        );
        // 音量行（下半、靠右）也命中
        assert_eq!(
            hit_test_item_rects(&rects, (50, 50)),
            Some(0),
            "音量行应命中"
        );
        // 第二项
        assert_eq!(hit_test_item_rects(&rects, (100, 40)), Some(1));
        // 项与项之间的空隙、以及边界外，都不命中
        assert_eq!(
            hit_test_item_rects(&rects, (74, 40)),
            None,
            "项间空隙不命中"
        );
        assert_eq!(
            hit_test_item_rects(&rects, (70, 40)),
            None,
            "右边界外不命中"
        );
        assert_eq!(
            hit_test_item_rects(&rects, (40, 61)),
            None,
            "下边界外不命中"
        );
        assert_eq!(hit_test_item_rects(&rects, (9, 40)), None, "左边界外不命中");
    }

    /// 静音优先于百分比：静音时显示「静音」而不是端点里那个与听觉不符的旧音量值。
    #[test]
    fn muted_device_shows_muted_instead_of_percent() {
        assert_eq!(
            format_volume(&item(Some(80), Some(0.4), true, Some(true)), false),
            "静音"
        );
        // ⚠️ 不能只断言「不含 40%」这类弱判据 —— 必须断言**就是**「静音」，
        //    否则「静音 + 百分比同时出现」这种回归照样能通过。
    }

    /// ⚠️ **契约已变更**（2026-09-28）：音量显示改为**固定一位小数**（`40.0%`）——
    ///   滚轮有 0.1% 步进档，只显示整数会让人以为滚轮没生效。
    ///   保留的判据是「四舍五入到 0.1 个百分点」：`0.996` ⇒ `99.6%`。
    /// ⚠️ **契约已变更**（2026-09-28）：音量显示的格式**由「音量精细调节」决定**——
    ///   开 ⇒ 一位小数（`40.0%`），关 ⇒ 整数（`40%`）。
    ///   保留的判据是「四舍五入、不是截断」：`0.996` ⇒ `100%`（关）/ `99.6%`（开）。
    #[test]
    fn volume_rounds_rather_than_truncates() {
        assert_eq!(
            format_volume(&item(None, Some(0.4), true, Some(false)), false),
            "40%"
        );
        assert_eq!(
            format_volume(&item(None, Some(0.996), true, Some(false)), false),
            "100%",
            "0.996 必须进位到 100%，不能显示成 99%（截断会让用户以为还差一点满格）"
        );
        assert_eq!(
            format_volume(&item(None, Some(0.996), true, Some(false)), true),
            "99.6%"
        );
        // 0.0 且未静音 = 音量真为 0 ⇒ 显示 0%（合法，不是 N/A）
        assert_eq!(
            format_volume(&item(None, Some(0.0), true, Some(false)), false),
            "0%"
        );
    }

    /// ⭐ 两段文本**都不含设备名**（用户口径：名字以后再说）。
    /// 可证伪：若哪天有人把 `it.name` 加回格式化函数，这条会立刻转红。
    #[test]
    fn metric_texts_do_not_contain_device_name() {
        let mut it = item(Some(50), Some(0.5), true, Some(false));
        it.name = "罗技MX Master".to_string();
        for s in [format_battery(&it), format_volume(&it, false)] {
            assert!(
                !s.contains("罗技") && !s.contains("MX"),
                "widget 上不应再出现设备名: {s}"
            );
        }
    }

    // ── tooltip 文本：设备名的三级解析 ──

    /// ⭐⭐ tooltip 显示的名字必须是「**alias → 全局重命名 → 短名**」三级链的结果。
    ///
    /// **为什么这条是硬判据**：`PhysicalDevice.name` 来自 `pick_display_name`
    /// （**纯函数、不读配置**），而用户在设置页改过的名字存在 `config.device_names`
    /// 与 `PinnedDevice.alias` 里 ⇒ 若 widget 侧直接用 `d.name`，
    /// **tooltip 会显示旧名**（而设置页/选择器显示新名）。
    ///
    /// ⚠️ **判据形状的教训（本次实测踩到）**：第一版只测
    /// `device_identity::resolved_display_name` 这个**纯函数本身** ⇒ 实测把生产侧
    /// （`resolve_widget_labels`）改成 `d.name.clone()` 后，**337 条依然全绿**——
    /// 「算法对」不等于「接线对」。⇒ 本条改为**穿过** [`resolve_labels_with`]，
    /// 同时验证**接线**（见 [`widget_labels_are_wired_to_the_resolver`]）。
    #[test]
    fn tooltip_label_follows_alias_then_rename_then_short_name() {
        let d = dev("小爱音箱-9205", Some(50), None, None, None, true);
        let key = d.key.clone();
        let devices = vec![d];

        // ① 无 alias、无重命名 ⇒ 短名
        let empty = crate::config::Config::default();
        let no_pins: Vec<crate::config::PinnedDevice> = Vec::new();
        assert_eq!(
            resolve_labels_with(&devices, |d| {
                crate::device_identity::resolved_display_name(&d.name, &d.key, &no_pins, &empty)
            }),
            vec!["小爱音箱-9205".to_string()]
        );

        // ② 全局重命名（`device_names`）⇒ 生效
        let mut renamed = crate::config::Config::default();
        renamed
            .device_names
            .insert("小爱音箱-9205".to_string(), "客厅音箱".to_string());
        assert_eq!(
            resolve_labels_with(&devices, |d| {
                crate::device_identity::resolved_display_name(&d.name, &d.key, &no_pins, &renamed)
            }),
            vec!["客厅音箱".to_string()],
            "用户在设置里改的名必须生效"
        );

        // ③ `alias` **优先于**全局重命名
        let pins = vec![crate::config::PinnedDevice {
            key,
            fallback: None,
            alias: Some("我的耳机".to_string()),
        }];
        assert_eq!(
            resolve_labels_with(&devices, |d| {
                crate::device_identity::resolved_display_name(&d.name, &d.key, &pins, &renamed)
            }),
            vec!["我的耳机".to_string()],
            "alias 优先级必须高于全局重命名"
        );

        // ④ 空白 alias 视为未设置 ⇒ 回落全局重命名（而不是显示空白）
        let blank = vec![crate::config::PinnedDevice {
            key: crate::device_identity::DeviceKey::Name("小爱音箱-9205".into()).encode(),
            fallback: None,
            alias: Some("   ".to_string()),
        }];
        assert_eq!(
            resolve_labels_with(&devices, |d| {
                crate::device_identity::resolved_display_name(&d.name, &d.key, &blank, &renamed)
            }),
            vec!["客厅音箱".to_string()],
            "空白 alias 必须被忽略（否则 tooltip 显示空白）"
        );
    }

    /// ⭐ **接线判据**：tooltip 用的 `WidgetItem.name` 必须来自 `labels`，而不是 `d.name`。
    ///
    /// ⛔ 这条是上条的**互补**：上条验「三级链算法对」，本条验「**真的把 labels 传下去了**」。
    /// 少了本条，把 `build_items_with_labels` 里的 `labels.get(i)` 改回 `d.name.clone()`
    /// 仍会全绿——那正是本次最初发生的失效。
    ///
    /// 可证伪：把 `build_items_with_opt_labels` 的 `name:` 字段改回 `d.name.clone()`
    /// ⇒ 本条转红。
    #[test]
    fn widget_labels_are_wired_to_the_resolver() {
        let d = dev("小爱音箱-9205", Some(50), None, None, None, true);
        let devices = vec![d];
        // 假装解析出了另一个名字
        let labels = vec!["我的耳机".to_string()];
        let items = build_items_with_labels(&devices, &labels);
        assert_eq!(items.len(), 1);
        assert_eq!(
            items[0].name, "我的耳机",
            "WidgetItem.name 必须取自解析后的 labels"
        );

        // 反控：不给 labels 时退回 `d.name`（这是 `build_items` 的语义）
        let plain = build_items(&devices);
        assert_eq!(
            plain[0].name, "小爱音箱-9205",
            "无 labels 时必须退回 d.name"
        );
    }

    // ── 逐项布局（tooltip 命中区与绘制共用同一份） ──
    /// ⭐⭐ `item_rects` 必须与**绘制循环**逐字同源。
    ///
    /// 旧实现只用 running `cursor` 逐项自增、用完即弃 ⇒ tooltip 若自行重算就是
    /// **第二个布局来源**，分叉后提示会挂到错误设备上且**不报错**。
    ///
    /// 本条把「首项贴 `pad_x`、逐项加 `item_gap`」钉死：
    /// 可证伪——把 `item_rects` 里的 `cursor = m.pad_x` 改回 `0`（漏掉左内边距）
    /// 或把 `cursor += w + m.item_gap` 改成 `cursor += w`（漏掉项间隙），
    /// 本条**立刻转红**。
    #[test]
    fn item_rects_match_draw_loop() {
        let m = m96();
        let per_item = [40_i32, 30, 55];
        let r = item_rects(&per_item, &m, m.h);
        assert_eq!(r.len(), 3);
        // 首项左端 = 左内边距
        assert_eq!(r[0].left, m.pad_x, "首项必须贴左内边距");
        // 每项宽度 = 给定宽度
        assert_eq!(r[0].right - r[0].left, 40);
        assert_eq!(r[1].right - r[1].left, 30);
        assert_eq!(r[2].right - r[2].left, 55);
        // 逐项推进 = 宽 + 项间隙
        assert_eq!(r[1].left - r[0].right, m.item_gap, "项间必须留 item_gap");
        assert_eq!(r[2].left - r[1].right, m.item_gap);
        // 纵向铺满整窗（tooltip 命中区的高度 = 窗口高）
        assert!(r.iter().all(|x| x.top == 0 && x.bottom == m.h));
    }

    /// ⛔ tooltip 的 `rect` **必须互不重叠**。
    ///
    /// 为什么这是硬要求：重叠区会让 `QuotaDock` 式的「一设备一 `TOOLINFO`」判不出
    /// 「光标属于谁」⇒ 提示串到别的设备上，**且不会报错**。
    /// `QuotaDock` 自己也把这条写成测试（`taskbar.rs` 的
    /// 「每个厂商拥有互不重叠的独立悬浮区域」）。
    ///
    /// 可证伪：把 `item_rects` 里的 `cursor += w + m.item_gap` 改成 `cursor += w`（重叠）
    /// 或 `- m.item_gap`（反重叠）⇒ 本条转红。
    #[test]
    fn item_rects_are_disjoint() {
        let m = m96();
        let per_item = [40_i32, 30, 55, 20];
        let r = item_rects(&per_item, &m, m.h);
        // ⛔ 不用 `{:?}` 打印 `RECT`：它是 `windows-sys` 的裸结构体，**未实现 `Debug`**
        //   （derive 会报 `E0277`）。这里只报坐标。
        for w in r.windows(2) {
            assert!(
                w[0].right <= w[1].left,
                "相邻项的 rect 不得重叠：[{},{}] vs [{},{}]",
                w[0].left,
                w[0].right,
                w[1].left,
                w[1].right
            );
        }
    }

    /// 单项时不得越出「内容宽 = Σ宽 + 间隙×(n−1) + 两端内边距」。
    ///
    /// 可证伪：把 `item_rects` 的推进写成 `cursor += w + m.item_gap * 2` ⇒ 转红。
    #[test]
    fn item_rects_total_width_matches_content_formula() {
        let m = m96();
        let per_item = [40_i32, 30, 55];
        let r = item_rects(&per_item, &m, m.h);
        let content_w: i32 =
            per_item.iter().sum::<i32>() + m.item_gap * (per_item.len() as i32 - 1);
        assert_eq!(r.last().unwrap().right, m.pad_x + content_w);
    }

    // ── 宽度估算（方向性：宁可高估，不可低估）—— 现服务于绘制侧 `desired_w` ──

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
            (40, 32, 6, 12, 16)
        );

        let m120 = Metrics::for_dpi(120); // 本机任务栏 DPI = 120（125%）
        assert_eq!(
            m120.h, 50,
            "125% 下底衬高必须与 FluentFlyout 一致（40 DIP）"
        );
        assert_eq!(m120.icon, 40);
        assert_eq!(m120.radius, 8, "6 DIP × 1.25 = 7.5 → 8");
        assert_eq!(m120.font, 15, "12 DIP × 1.25 = 15");
        assert_eq!(m120.text_row_h, 20, "行高 = 图标高的一半");

        assert_eq!(Metrics::for_dpi(144).h, 60, "150%");
        // 防御：dpi = 0 不得 panic、也不得产生 0 尺寸（那会让窗口彻底不可见）
        assert_eq!(Metrics::for_dpi(0).h, 40);
        assert_eq!(Metrics::for_dpi(0).dpi, 96);
    }
    /// ⛔⛔ 本设置的**作用域边界**（用户 2026-09-25 明确要求）：
    ///   内容随档位变，**底衬必须恒定**。
    ///
    /// ⭐⭐⭐ 音乐面板内部还要**再分一次**（用户 2026-09-30 逐条确认「都按 A」）：
    /// ```text
    /// 固定（不随本设置变）：封面边长 · 双排信息字号 · 文字宽度上限 · 第二行位置
    /// 跟随本设置：播放三键 · 切换键 · 各项间距 · 左右留白
    /// ```
    ///
    /// ⚠️⚠️⚠️ **本文件的单测抓不到这条要求** —— 注入实测：改错实现仍全绿。两条原因：
    ///   ① `art_fixed_for(backdrop)` 的定义**就是** `for_scales(backdrop, Default)`
    ///      ⇒ 断言「它等于 `for_scales(.., Default)`」是**同义反复**；
    ///      把它改成读全局配置，单测里全局 config 恰好也是 Default ⇒ 照样通过。
    ///   ② 「封面取 `m_art.icon` 而三键取 `m.icon`」属于**标识符选错**，
    ///      而 `draw_music_render` 需要真实 HWND、无任何 `#[test]` 能调用它
    ///      ⇒ `cargo test` **原理上**看不见这一类。
    /// ⇒ 真正的判据是**结构判据**：逐处点名「恒定的必须取 `m_art`、
    ///   跟随的必须取 `m`」，注入两种改法都必须转红。
    ///
    /// 下面这条单测只钉**换算性质**（有独立价值，不依赖上面的接线）。
    #[test]
    fn content_scale_really_changes_content_metrics() {
        use crate::config::TaskbarContentScale;
        // ⚠️ 底衬 DPI 必须挑「能降一档」的：100%（96）已是缩放阶梯**地板**，
        //   「偏小」无法再降 ⇒ 两档必然相等（`step_down_dpi` 的既有约定）。
        //   在 96 上断言「两档不同」会自己转红 —— 实测踩过（left 32 / right 32）。
        for backdrop in [120u32, 144, 192] {
            let def = Metrics::for_scales(backdrop, TaskbarContentScale::Default);
            let small = Metrics::for_scales(backdrop, TaskbarContentScale::Smaller);
            assert_ne!(def.icon, small.icon, "内容档必须真的改变图标边长");
            assert_ne!(def.font, small.font, "内容档必须真的改变字号");
            assert_eq!(
                (def.h, def.radius, def.dpi),
                (small.h, small.radius, small.dpi),
                "底衬量必须恒按系统 DPI（与内容档无关）"
            );
        }
        // ⭐ 把**地板**这条事实钉住：100% 系统缩放下「偏小」= 默认（不再更小）
        assert_eq!(
            Metrics::for_scales(96, TaskbarContentScale::Smaller).icon,
            Metrics::for_scales(96, TaskbarContentScale::Default).icon,
            "100% 已是缩放阶梯地板：「偏小」不得比默认更小"
        );
        // 125%（本机实测口径）：默认 40px/15px，偏小降到 32px/12px
        assert_eq!(
            Metrics::for_scales(120, TaskbarContentScale::Default).icon,
            40
        );
        assert_eq!(
            Metrics::for_scales(120, TaskbarContentScale::Smaller).icon,
            32
        );
    }

    /// ⭐⭐⭐ **缩小的三键必须按自己的尺寸垂直居中**（用户 2026-09-30 实测报障：
    /// 「音乐组件中缩小的三键没有垂直居中」）。
    ///
    /// ## 根因
    /// 三个控件原先都从**同一个** `icon_y`（= 封面的上边距）起画。
    /// `[y, y + size]` 的中心是 `h/2` **只在所有控件 size 相等时**成立；
    /// 而「缩放设置只管三键与切换键、封面恒定」⇒ **两者不再相等**
    /// ⇒ 125% 下 `h=50`、封面 40、三键 32：封面 `[5,45]` 中心 25、
    /// 三键 `[5,37]` 中心 21 ⇒ **偏上 4px**。
    ///
    /// ## 判据锚在真实函数上，但**只覆盖公式**
    /// 走 `center_y`（生产代码用的那个），**不是**在此重推 `(h - size) / 2`。
    ///
    /// ⛔⛔ **但它抓不到「调用点写错」**（注入实测：把 `let btn_y = center_y(h, btn)`
    ///   改成 `let btn_y = icon_y`（＝缺陷本身）⇒ 本条**仍全绿**）。
    ///   因为本测试自己调 `center_y`，看不见 `draw_music_render` 里那一行。
    ///   ⇒ 调用点接线由**结构判据**覆盖（`verify-split-scale`：
    ///   「三键纵向位置必须写成 `center_y(h, btn)`、不得借 `icon_y`/`sy`」）。
    ///
    /// ⛔ 这是本会话**第三次**栽在同一处：**判据自己重算 ⇒ 对「算错」免疫、
    ///   对「用错」完全失明**。前两次是 `art_fixed_for`（与 `for_scales(.., Default)`
    ///   同义反复）与升级通道复现脚本（`compare_versions` 方向写反）。
    ///   ⇒ 判据要么**调生产函数**、要么**钉调用点文本**，二选一，不能只重推公式。
    ///
    /// 可证伪：把 `center_y` 的公式改坏 ⇒ 本条转红（注入② 实测）。
    #[test]
    fn scaled_buttons_are_centered_on_their_own_size() {
        // 125% 本机实测口径：底衬高 50、封面 40（固定档）、三键 32（偏小档）
        for (h, cover, btn) in [(50, 40, 32), (40, 32, 26), (60, 48, 38)] {
            let cover_y = center_y(h, cover);
            let btn_y = center_y(h, btn);

            // ① 真居中：上下留白对称
            assert_eq!(
                btn_y * 2 + btn,
                h,
                "h={h} btn={btn}：三键上下留白必须对称（上 {btn_y} / 下 {}）",
                h - btn_y - btn
            );
            assert_eq!(
                cover_y * 2 + cover,
                h,
                "h={h} cover={cover}：封面上下留白必须对称"
            );

            // ② ⭐ 这条才是缺陷本身：三键比封面小 ⇒ **上边距必须更大**。
            //    复用封面坐标（`btn_y == cover_y`）⇒ 偏上，正是用户报的那个现象。
            assert!(
                btn_y > cover_y,
                "h={h}：三键({btn})比封面({cover})小 ⇒ 上边距({btn_y})必须大于封面({cover_y})，\
             否则三键偏上（用户 2026-09-30 实测）"
            );
        }
        // ③ 反向：两者**相等**时两者坐标也相等（借坐标此时恰好没错，但不能推广）
        assert_eq!(center_y(50, 40), center_y(50, 40));
    }

    /// 内容档位**只**改内容：底衬量（`h` / `radius`）**恒**按系统 DPI。
    ///
    /// 可证伪：把 `for_dpis` 里的 `h: b(WIDGET_H_DIP)` 改成 `c(WIDGET_H_DIP)`
    ///   （即让底衬跟着内容缩放）⇒ 本条立刻转红。
    #[test]
    fn content_scale_changes_content_but_never_backdrop() {
        let big = Metrics::for_scales(120, TaskbarContentScale::Default);
        let small = Metrics::for_scales(120, TaskbarContentScale::Smaller);

        // ── 底衬：两档必须逐字相同（恒按系统 DPI = 120）──
        assert_eq!(small.h, big.h, "底衬高度不得随内容档位改变");
        assert_eq!(small.radius, big.radius, "底衬圆角不得随内容档位改变");
        assert_eq!(small.dpi, big.dpi, "底衬 DPI 恒为系统 DPI");
        assert_eq!(big.h, 50, "125% 下底衬高 50（40 DIP × 1.25）");
        assert_eq!(big.radius, 8, "125% 下底衬圆角 8（6 DIP × 1.25）");

        // ── 内容：「默认」= 与底衬同口径（120）──
        assert_eq!(big.content_dpi, 120);
        assert_eq!((big.icon, big.font, big.text_row_h), (40, 15, 20));

        // ── 内容：「偏小」= **降一档** ⇒ 125% 系统下用 100%（用户 2026-09-29 的原话）──
        assert_eq!(
            small.content_dpi, 96,
            "系统 125% 时「偏小」必须降到 100%（不是 93.75%）"
        );
        assert_eq!((small.icon, small.font, small.text_row_h), (32, 12, 16));

        // ── 区分力：两档必须在**内容**上真的不同，否则本条证明不了任何事
        for (name, a, b) in [
            ("icon", small.icon, big.icon),
            ("font", small.font, big.font),
            ("pad_x", small.pad_x, big.pad_x),
            ("icon_text_gap", small.icon_text_gap, big.icon_text_gap),
            ("item_gap", small.item_gap, big.item_gap),
            ("item_max_w", small.item_max_w, big.item_max_w),
            ("text_row_h", small.text_row_h, big.text_row_h),
        ] {
            assert_ne!(a, b, "内容量 `{name}` 两档必须不同，否则用例无区分力");
        }
    }

    /// ⭐⭐ **「偏小」= 沿 Windows 标准缩放阶梯降一档**（用户 2026-09-29 定义）。
    ///
    /// 判据把用户的原话逐条钉住：`125% ⇒ 100%`、`150% ⇒ 125%`、`200% ⇒ 175%`。
    /// ⛔ **不是「乘 0.75」**：那在 125% 上会得到 93.75%，不是任何一档。
    /// ⛔ **已在 100% 时不再降**（否则会掉到 75%，小于设计基准）。
    #[test]
    fn smaller_scale_steps_down_one_ladder_rung() {
        use super::Metrics;
        for (system, want) in [
            (96u32, 96u32), // 100% → 已在最小档，保持
            (120, 96),      // 125% → 100%
            (144, 120),     // 150% → 125%
            (168, 144),     // 175% → 150%
            (192, 168),     // 200% → 175%
            (240, 216),     // 250% → 225%
        ] {
            assert_eq!(
                Metrics::step_down_dpi(system),
                want,
                "系统 {system} DPI 时「偏小」应降到 {want}"
            );
            // 端到端：走 for_scales 也必须一致（判据要锚在真正生效的那条路径上）
            assert_eq!(
                Metrics::for_scales(system, TaskbarContentScale::Smaller).content_dpi,
                want
            );
        }
    }

    /// `for_dpi` 保留「底衬与内容同口径」的旧语义（= `Default` 档）——
    /// 只消费底衬量的调用点（建窗 / 垂直居中 / 空帧）依赖它。
    ///
    /// 可证伪：把 `for_dpi` 改成 `Self::for_dpis(dpi, 96)` ⇒ 本条转红。
    #[test]
    fn for_dpi_equals_default_scale() {
        for dpi in [0u32, 96, 120, 144] {
            assert_eq!(
                Metrics::for_dpi(dpi),
                Metrics::for_scales(dpi, TaskbarContentScale::Default),
                "for_dpi 必须等价于 Default 档（dpi={dpi}）"
            );
        }
    }

    /// 内容档位必须**同时**影响「测宽」——否则窗口宽度按旧口径算、内容按新口径画
    /// ⇒ 相邻项重叠（不报错、不 panic，只是画面错）。
    ///
    /// 可证伪：把 `dip()` 改回按 `self.dpi` 缩放 ⇒ 本条转红。
    #[test]
    fn estimate_width_follows_content_scale() {
        let eight: Vec<WidgetItem> = (0..WIDGET_MAX_ITEMS)
            .map(|i| item(Some(50 + i as i32), Some(0.5), true, Some(false)))
            .collect();
        let big = estimate_widget_width(
            &eight,
            &Metrics::for_scales(120, TaskbarContentScale::Default),
            false,
        );
        let small = estimate_widget_width(
            &eight,
            &Metrics::for_scales(120, TaskbarContentScale::Smaller),
            false,
        );
        assert!(
            small < big,
            "「偏小」档内容更小 ⇒ 估算宽度必须更窄（{small} vs {big}）"
        );
        // ⭐ 默认档在**任意**系统缩放下都必须与 100% 同宽 —— 这正是本档位的语义：
        //   内容不随系统缩放（底衬仍会变，但底衬不参与测宽）。
        assert_eq!(
            small,
            estimate_widget_width(&eight, &Metrics::for_dpi(96), false),
            "「偏小」档在 125% 系统下的测宽，必须与 100% 系统缩放下完全一致"
        );
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

        let w1 = estimate_widget_width(&one, &m, false);
        let w2 = estimate_widget_width(&two, &m, false);
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
        let w = estimate_widget_width(&[it], &m, false);
        assert!(
            w >= m.pad_x * 2 + m.icon + m.icon_text_gap + wider,
            "宽度必须覆盖更宽的那段文本：{w} < 内边距 + 图标 + 间隙 + {wider}"
        );
    }

    /// 8 台（显示上限）的估算必须仍能塞进避让后的可用区 ——
    /// 否则「避让扫描永远找不到槽」⇒ widget 直接不显示（且不报错）。
    ///
    /// ⭐ 同时覆盖 **125% 缩放**：布局整体放大 25% ⇒ 高 DPI 用户最容易踩到「找不到槽」。
    #[test]
    fn eight_items_still_fit_in_a_plausible_slot() {
        let eight: Vec<WidgetItem> = (0..WIDGET_MAX_ITEMS)
            .map(|i| item(Some(50 + i as i32), Some(0.5), true, Some(false)))
            .collect();
        // 真机可用区约 1300px（任务栏 2560px 减两端各 100px 再减任务栏自身内容）
        let w = estimate_widget_width(&eight, &m96(), false);
        assert!(w < 1300, "8 台估算过宽，会找不到避让槽: {w}");
        let w120 = estimate_widget_width(&eight, &Metrics::for_dpi(120), false);
        assert!(w120 < 1300, "125% 缩放下 8 台估算过宽: {w120}");
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
    ///   若直接照抄它的输出，用户勾 1 台却看到 8 台 ⇒ **设置页形同虚设**（真机实测）。
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
            let items = build_items(std::slice::from_ref(&d));
            assert!(items[0].has_audio, "{} 应判为有音频", d.name);
        }
        // 三者皆无 ⇒ 无音频
        let items = build_items(&[pinned_dev("kbd", Some(50), None, None, None)]);
        assert!(!items[0].has_audio);
    }

    /// 已连接但暂时读不到电量/音量的设备不得置灰；只有反向补建的离线占位条目才置灰。
    /// `connected` 必须从物理设备层原样传到 widget 条目，避免 Xbox 这类设备被误判。
    #[test]
    fn connected_state_is_carried_to_widget_item() {
        let mut live = pinned_dev("Xbox 360 Controller", None, None, None, None);
        assert!(live.connected);
        let live_item = build_items(&[live.clone()])[0].clone();
        assert!(live_item.connected);
        assert!(!should_dim_item(&live_item));

        live.connected = false;
        let placeholder_item = build_items(&[live])[0].clone();
        assert!(!placeholder_item.connected);
        assert!(should_dim_item(&placeholder_item));
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

    /// ⭐ 最多显示 8 台（与固定上限一致）：**截断发生在排序之后** ⇒ 留下的是「最该看的 8 台」，
    /// 而不是输入顺序的前 8 台。可证伪：把 `truncate` 移到 `sort` 之前，本条会转红。
    #[test]
    fn at_most_eight_items_and_keeps_the_data_rich_ones() {
        // 7 台，其中「空」排在最前面（输入序第一），但它无数据 ⇒ 应被排到末尾再截掉
        let mut input = vec![pinned_dev("空", None, None, None, None)];
        for i in 0..8 {
            input.push(pinned_dev(
                &format!("有数据{i}"),
                Some(50 + i),
                None,
                None,
                None,
            ));
        }
        let items = build_items(&input);
        assert_eq!(items.len(), 8, "必须截断到 8 台");
        assert!(
            !items.iter().any(|it| it.name == "空"),
            "无数据的条目应在截断中被丢弃（说明截断发生在排序之后）"
        );
    }

    /// 不足 8 台时不截断，且顺序不变。
    #[test]
    fn fewer_than_eight_items_are_not_truncated() {
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

    /// ⭐ 图标解码 + 缩放：五种类别 × 两套主题都必须**解码成功**（否则真机上会「什么都不画」
    /// 且不报错 —— 正是最难发现的静默失效）。可证伪：删掉任一张 PNG，本条立刻转红。
    #[test]
    fn all_icon_assets_decode_and_scale() {
        for kind in [
            AudioKind::Pointer,
            AudioKind::Keyboard,
            AudioKind::Gamepad,
            AudioKind::Speaker,
            AudioKind::Headphones,
        ] {
            for dark in [false, true] {
                let src = icons::get(kind, dark)
                    .unwrap_or_else(|| panic!("图标解码失败: {kind:?} dark={dark}"));
                assert!(src.1 > 0 && src.2 > 0, "解码出的尺寸不能为 0");
                let scaled =
                    icons::scale_to(icons::slot_of(kind), src, 16).expect("缩放到 16px 不应失败");
                assert_eq!((scaled.1, scaled.2), (16, 16));
                assert_eq!(scaled.0.len(), 16 * 16 * 4, "RGBA 缓冲长度必须是 w*h*4");
            }
        }
    }

    /// 缩放必须**真的**产出非全透明内容（防止「解码成功但像素全 0」这种假绿）。
    #[test]
    fn scaled_icon_has_visible_pixels() {
        let src = icons::get(AudioKind::Speaker, false).expect("扬声器图标应解码成功");
        let (px, ..) = icons::scale_to(icons::slot_of(AudioKind::Speaker), src, 16).unwrap();
        let opaque = px.chunks(4).filter(|c| c[3] > 0).count();
        assert!(
            opaque > 16,
            "扬声器图标缩放后应有相当数量的不透明像素，实际 {opaque}"
        );
    }

    /// ⭐ **缩小**必须是**面积平均**（不是最近邻）。
    ///
    /// ⛔⛔ **本条钉的契约在 2026-09-29 被有意改掉了**（旧契约 = 最近邻，理由是
    ///   「1–2px 笔画被双线性糊成灰带」）。改契约的依据不是口味，而是**母图换了**：
    ///   母图从 32 提到 256 之后，缩小时源里**根本不存在 1px 笔画**——最小笔画是
    ///   51/1024 × 256 ≈ **12.8px**，缩到 48px 仍有 2.4px。此时最近邻会
    ///   **按点抽样丢像素**（5.33:1 ⇒ 每 5~6 个源像素只取 1 个）⇒ 斜边出现台阶、
    ///   细笔画断裂；面积平均则是「高分辨率渲染后降采样」，**没有混叠**。
    ///   ⇒ 旧契约的前提（笔画细到 1px）已随母图一起消失，契约必须跟着改。
    ///
    /// 判据：1×2 源（上黑下白）缩到 1×1，面积平均给**中值**（≈128）而不是端点。
    /// 可证伪：把 `resample` 的 `side <= sw` 分支改回最近邻 ⇒ 本条转红。
    #[test]
    fn scale_down_averages_source_area() {
        let src = (vec![0, 0, 0, 255, 255, 255, 255, 255], 1u32, 2u32);
        let (px, w, h) = icons::scale_to(900_001, &src, 1).expect("缩放不应失败");
        assert_eq!((w, h), (1, 1));
        // ⚠️ 断言的是 **RGB** 不是 alpha：两个源像素的 alpha 都是 255，
        //    平均后仍是 255（若断言 alpha，任何实现都会「通过」⇒ 假判据）。
        let lum = px[0] as i32;
        assert!(
            (110..=145).contains(&lum),
            "缩小必须取源区域的**平均**（黑白各半 ⇒ 灰 ≈128），最近邻会得到 0 或 255: {lum}"
        );
    }

    /// ⭐ **放大**必须平滑且单调：最近邻会让 1px 笔画忽宽忽窄、曲线出现台阶。
    ///
    /// ⚠️ 母图 256 之后**正常设备上走不到放大这条路**（任何现实 DPI ≤ 128px），
    ///   它只为兜底那 3 张仍是 32px 母图的设备图标；双三次比双线性锐利。
    ///
    /// 可证伪：把放大分支改回最近邻 ⇒ 本条转红。
    #[test]
    fn scale_up_interpolates() {
        // 2×1 的黑白源放大到 4×1 ⇒ 中间两列必须是**渐变**而不是非黑即白
        let src = (vec![0, 0, 0, 255, 255, 255, 255, 255], 2u32, 1u32);
        let (px, w, _) = icons::scale_to(900_002, &src, 4).expect("缩放不应失败");
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
        let (px, _, _) = icons::scale_to(900_003, &src, 4).expect("缩放不应失败");
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

    /// 三种策略在同一可用区内的相对位置必须**互不相同**且顺序正确。
    ///
    /// 可证伪：把 `right` 的 `max_offset - margin` 写成 `max_offset`（丢掉边距）本条转红。
    #[test]
    fn align_in_slot_orders_left_center_right() {
        let (slot_x, slot_w, content_w) = (600, 300, 100);
        let m = 20;
        let left = align_in_slot(slot_x, slot_w, content_w, "left", m);
        let center = align_in_slot(slot_x, slot_w, content_w, "center", m);
        let right = align_in_slot(slot_x, slot_w, content_w, "right", m);
        assert_eq!(left, slot_x + m, "靠左 = 可用区左端 + 留白");
        assert!(left < center && center < right, "三者必须严格递增");
        assert_eq!(
            center,
            slot_x + 100,
            "居中 = 可用区左端 + 剩余/2（**不留白**）"
        );
    }

    /// ⭐⭐ **本次需求的核心判据**：靠左/靠右时**距边缘恰好 `edge_margin`**，
    /// 且**内容绝不越出可用区**。
    ///
    /// ⛔ 上一版是「紧贴边缘」（`left == slot_x`），用户明确要求参考 Windows
    ///   开始按钮/时钟的留白（实测 24~26px @125%）。
    ///
    /// 可证伪：
    ///   ① 把 `"left" => margin` 改回 `0` ⇒ 第 1 条断言转红；
    ///   ② 把 `"right" => max_offset - margin` 改成 `max_offset` ⇒ 第 2 条转红；
    ///   ③ 反过来把 `max_offset - margin` 写成 `max_offset + margin` ⇒ 越界 ⇒ 第 2 条转红。
    #[test]
    fn align_in_slot_keeps_edge_margin_on_both_sides() {
        for (slot_x, slot_w, content_w, m) in [
            // 真机形状：整条任务栏 2560，内容 274，留白 25（= 20 DIP @125%）
            (0, 2560, 274, 25),
            // 换个尺寸确保不是巧合
            (100, 800, 200, 20),
        ] {
            let left = align_in_slot(slot_x, slot_w, content_w, "left", m);
            assert_eq!(
                left,
                slot_x + m,
                "靠左必须距可用区左缘恰好 {m}px（不得紧贴）"
            );

            let right = align_in_slot(slot_x, slot_w, content_w, "right", m);
            assert_eq!(
                (slot_x + slot_w) - (right + content_w),
                m,
                "靠右必须距可用区右缘恰好 {m}px（不得紧贴、不得溢出）"
            );
            assert!(
                right + content_w <= slot_x + slot_w,
                "靠右时内容右端绝不可越过可用区右缘（right={right}）"
            );
        }
    }

    /// 未知值（防御分支）与 `center` 同解 —— `normalize_config` 已兜住非法值，
    /// 这里只保证「万一漏进来一个奇怪的值，也不会算出可用区外坐标」。
    #[test]
    fn align_in_slot_unknown_position_falls_back_to_center() {
        let args = (600, 300, 100, 20);
        assert_eq!(
            align_in_slot(args.0, args.1, args.2, "middle", args.3),
            align_in_slot(args.0, args.1, args.2, "center", args.3),
            "未知贴靠值必须退化为居中（而不是越界或 panic）"
        );
    }

    /// 可用区比内容窄时（防御分支）：`max_offset` 归零 ⇒ 边距被夹成 0 ⇒
    /// 三种策略**都**退化为贴可用区左端，且**绝不返回负偏移**（负偏移会把窗口推到任务栏之外）。
    #[test]
    fn align_in_slot_never_returns_negative_offset_when_content_overflows() {
        let (slot_x, slot_w, content_w) = (600, 80, 200);
        for pos in ["left", "center", "right", "unknown"] {
            let x = align_in_slot(slot_x, slot_w, content_w, pos, 20);
            assert_eq!(x, slot_x, "可用区装不下时 `{pos}` 应退化为贴左端");
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

    /// ⭐⭐ **位置语义的可证伪测试**：可用区 = 整条任务栏时，
    /// `left`/`center`/`right` 必须分别落在**左侧留白后 / 正中 / 右侧留白前**。
    ///
    /// ── 为什么这条是本次方案变更的**核心回归** ────────────────────────
    /// 旧实现（像素扫描避让）里，可用区是「任务栏里某一段**空白**」：
    ///   · `SLOT_SAFE_MARGIN = 100` ⇒ `slot_x ≥ 100` ⇒ 「靠左」永远差 100px；
    ///   · `pick_widest_run` 只给最宽段 ⇒ `center` 在**裁剪后**的区间里算。
    /// 用户实测反馈正是「靠左/右没有出现在整个任务栏的最左/右侧，居中也不对」。
    /// ⇒ 本条以**整条任务栏**（`slot_x = 0`、`slot_w = 屏幕宽`）为口径断言三档位置；
    ///   居中不留边距，左右各留 Windows 风格边距。若有人把避让扫描加回来 ⇒ **转红**。
    ///
    /// 可证伪：把 `align_in_slot` 的 `"left" => 0` 改成 `100`（旧安全边距）
    /// ⇒ 第 1 条断言转红；把 `"right" => max_offset` 改成 `slot_w`
    /// ⇒ 第 3 条断言转红（右缘溢出）。
    #[test]
    fn align_in_slot_uses_the_whole_taskbar_as_area() {
        // 复刻真机：任务栏 2560 宽、内容 274 宽 ⇒ `max_offset = 2286`。
        let (area_x, area_w, content_w) = (0, 2560, 274);
        let max_offset = area_w - content_w;

        const EDGE_MARGIN: i32 = 25; // 20 DIP @ 125% DPI
        assert_eq!(
            align_in_slot(area_x, area_w, content_w, "left", EDGE_MARGIN),
            EDGE_MARGIN,
            "靠左必须保留 Windows 风格边距"
        );
        assert_eq!(
            align_in_slot(area_x, area_w, content_w, "center", EDGE_MARGIN),
            max_offset / 2,
            "居中必须在整条任务栏里居中（左右不额外留边距）"
        );
        assert_eq!(
            align_in_slot(area_x, area_w, content_w, "right", EDGE_MARGIN),
            max_offset - EDGE_MARGIN,
            "靠右必须在右缘前保留 Windows 风格边距"
        );

        // ⛔⛔ 最重要的性质：三档**必须严格递增** —— 相等就意味着贴靠失效
        //   （旧方案把窗口钳成 53px 时正是三档同解）。
        let l = align_in_slot(area_x, area_w, content_w, "left", EDGE_MARGIN);
        let c = align_in_slot(area_x, area_w, content_w, "center", EDGE_MARGIN);
        let r = align_in_slot(area_x, area_w, content_w, "right", EDGE_MARGIN);
        assert!(
            l < c && c < r,
            "三档必须严格递增（left={l} < center={c} < right={r}）"
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

    /// ⭐ 用户口径（2026-09-28 修订）：「**默认关闭**；关闭只隐藏窗口，**不丢设备**」。
    ///
    /// ⛔ 旧口径已退役：此前是「列表非空即显示」，等于**拿设备列表当开关**——
    ///   「关闭组件」就等于「清空设备」，用户重开时设备全没了。
    ///
    /// 可证伪：把 `config::taskbar_widget_visible` 改回「只看列表非空」⇒ 本组三条全红。
    #[test]
    fn widget_visibility_is_switch_and_list() {
        use crate::config::{Config, PinnedDevice};

        let one_pin = |enabled: bool| Config {
            pinned_taskbar_devices: vec![PinnedDevice {
                key: "c:abc".to_string(),
                fallback: None,
                alias: None,
            }],
            taskbar_widget_enabled: enabled,
            ..Default::default()
        };

        // ① 默认关闭：新装（无设备 + 开关默认关）⇒ 不显示
        assert!(
            !crate::config::taskbar_devices_available(&Config::default()),
            "默认（无设备、开关默认关）⇒ 窗口不应存在"
        );

        // ② 开关开 + 有设备 ⇒ 显示
        assert!(
            crate::config::taskbar_devices_available(&one_pin(true)),
            "开关开且已钉设备 ⇒ 窗口应存在"
        );

        // ③ ⭐ 开关关 + **仍保留设备** ⇒ 不显示，但**列表一个字都没少**
        let off = one_pin(false);
        assert!(
            !crate::config::taskbar_devices_available(&off),
            "开关关 ⇒ 窗口不应存在"
        );
        assert_eq!(
            off.pinned_taskbar_devices.len(),
            1,
            "⭐ 关闭**不得**清空设备列表（否则用户重开时设备全丢）"
        );
    }

    /// 开关开但**一台设备都没钉** ⇒ 不得显示（否则任务栏上出现空窗）。
    #[test]
    fn switch_on_without_devices_does_not_show_empty_window() {
        use crate::config::Config;
        let c = Config {
            taskbar_widget_enabled: true,
            ..Default::default()
        };
        assert!(
            !crate::config::taskbar_devices_available(&c),
            "开关开但无设备 ⇒ 不得显示空窗"
        );
    }

    /// ⭐ **三态回落链**（用户 2026-09-28 指定的降级规则）——本批新增的核心判据。
    ///
    /// ```text
    /// 音乐可用 ∧ (音乐开关开 ∨ 记住的是音乐) → Music
    /// 否则若设备可用                          → Devices
    /// 否则                                     → None
    /// ```
    #[test]
    fn panel_falls_back_when_music_unavailable() {
        use crate::config::{taskbar_devices_available, taskbar_panel_for, Config, TaskbarPanel};

        // ① 只有设备可用、音乐不可用 ⇒ 恒显示设备面板（**与记住的选择无关**）
        let dev_only = Config {
            taskbar_widget_enabled: true,
            taskbar_panel: TaskbarPanel::Music, // 记住的是音乐
            pinned_taskbar_devices: vec![crate::config::PinnedDevice {
                key: "c:x".into(),
                fallback: None,
                alias: None,
            }],
            ..Default::default()
        };
        assert!(taskbar_devices_available(&dev_only));
        assert_eq!(
            taskbar_panel_for(&dev_only, false),
            Some(TaskbarPanel::Devices),
            "⭐ 无媒体会话时必须**回落设备面板**（哪怕用户上次选的是音乐）"
        );

        // ② 音乐可用 ∧ 开关开 ⇒ Music
        let both = Config {
            taskbar_music_enabled: true,
            ..dev_only.clone()
        };
        assert_eq!(taskbar_panel_for(&both, true), Some(TaskbarPanel::Music));

        // ③ 音乐可用 ∧ 开关关但**记住的是音乐** ⇒ 仍是 Music（选择被记住）
        let remembered = Config {
            taskbar_music_enabled: false,
            ..both.clone()
        };
        assert_eq!(
            taskbar_panel_for(&remembered, true),
            Some(TaskbarPanel::Music),
            "记住的选择在**可用时**必须生效"
        );

        // ④ 两边都不可用 ⇒ 整个组件不显示
        let none = Config::default();
        assert_eq!(taskbar_panel_for(&none, false), None);
        assert_eq!(
            taskbar_panel_for(&none, true),
            None,
            "音乐开关默认关 ⇒ 仍不显示"
        );

        // ⑤ ⭐ **回落不改写用户的选择**：回落是「显示层」的事，不是「选择」的事
        assert_eq!(
            dev_only.taskbar_panel,
            TaskbarPanel::Music,
            "回落只影响显示，配置里记住的选择必须原样保留"
        );
    }

    /// ⭐⭐ **两个组件开关都关 ⇒ 整个组件不显示**（用户 2026-09-30 实测报障）。
    ///
    /// ⛔ **这一格此前从未被测过**，bug 就藏在那个空格里：
    /// `panel_falls_back_when_music_unavailable` 的 ④ 用 `Config::default()`，
    /// 而它的 `taskbar_panel` 默认是 **Devices** ⇒ 走的是「设备不可用 ⇒ None」
    /// 那条路，**碰不到**「记住的是音乐」这一支。
    /// ⇒ 「两个开关都关 + 记住=Music + 有会话」会**穿过** ① 的
    /// `Music if music_available` ⇒ 组件仍然存在，且留下的**恰是本应最后关闭的那块**。
    ///
    /// ⚠️ 判据锚在**「组件存不存在」这个维度**上，与 `taskbar_panel` 无关：
    ///   下面把 `taskbar_panel` 在 Music/Devices 之间**都试一遍**，
    ///   两种都必须 `None` —— 只要有一种漏掉，闸门就有洞。
    ///
    /// 可证伪：删掉 `taskbar_panel_for` 里的 ⓪ 层（两个开关都关就返回 `None`），
    /// 本测试立刻转红（Music 那一支会返回 `Some(Music)`）。
    #[test]
    fn widget_absent_when_both_component_switches_off() {
        use crate::config::{taskbar_panel_for, Config, TaskbarPanel};

        // 已钉了设备 —— 让「设备侧」尽可能可用，确保 None 不是因为「没钉设备」
        let base = Config {
            taskbar_widget_enabled: false,
            taskbar_music_enabled: false,
            pinned_taskbar_devices: vec![crate::config::PinnedDevice {
                key: "c:x".into(),
                fallback: None,
                alias: None,
            }],
            ..Default::default()
        };

        for panel in [TaskbarPanel::Music, TaskbarPanel::Devices] {
            let c = Config {
                // ⚠️ `TaskbarPanel` 是 `Copy` ⇒ 这里**不能** `.clone()`
                //    （`clippy::clone_on_copy` 在 `-D warnings` 下是硬失败）
                taskbar_panel: panel,
                ..base.clone()
            };
            // 有会话（true）与无会话（false）都要 None：
            // 「音乐侧可用性只看有没有会话」⇒ 这一支曾经能靠 true 复活组件
            assert_eq!(
                taskbar_panel_for(&c, true),
                None,
                "两个开关都关 + 记住={panel:?} + 有会话 ⇒ 组件必须不存在"
            );
            assert_eq!(
                taskbar_panel_for(&c, false),
                None,
                "两个开关都关 + 记住={panel:?} ⇒ 组件必须不存在"
            );
            // ⚠️ 存在性判据**与用户的选择无关**：不许改写
            assert_eq!(c.taskbar_panel, panel, "关组件不得改写用户的选择");
        }

        // ── 反向对照：任意一个开关开着，就**不该**被这一层拦掉 ──────────
        // （否则这一层就成了「无论开关如何都不显示」的恒真判据）
        for (w, m, want) in [
            (true, false, Some(TaskbarPanel::Devices)),
            (false, true, Some(TaskbarPanel::Music)),
            (true, true, Some(TaskbarPanel::Devices)),
        ] {
            let mut c = Config {
                taskbar_widget_enabled: w,
                taskbar_music_enabled: m,
                ..base.clone()
            };
            c.taskbar_panel = TaskbarPanel::Devices;
            assert_eq!(
                taskbar_panel_for(&c, true),
                want,
                "设备开关={w} 音乐开关={m} ⇒ 必须显示 {want:?}（⓪ 层不许误伤）"
            );
        }
    }

    // ── 中毒（poison）后的可证伪验证 ──────────────────────────────────
    //
    // ⛔ 为什么要有这组：`LAST_ITEM_RECTS` 原先的 6 处都是裸 `.lock()`，
    //   其中 4 处写成「中毒就跳过/放弃」（`.ok()?` / `if let Ok` / `Err(_) => return`）。
    //   中毒一旦发生，**跳过一次就永远跳**（没有自愈、没有日志）⇒ 命中判定、
    //   tooltip 锚点、滚轮音量会静默永久失效。现在统一走 `lock_unpoisoned`
    //   （中毒恢复）。**下面两条断言正是这个差异的可证伪判据。**

    /// 真实调用点：把 `LAST_ITEM_RECTS` 弄中毒后，`publish_item_rects` + `item_rect_count`
    /// 仍须正常工作（修复前：`item_rect_count` 恒报 0、发布被静默丢弃）。
    ///
    /// ⚠️ 本测试会**永久中毒**该全局（`std::sync::Mutex` 无法解除中毒）。
    ///   这是安全的：修复后所有访问点都走恢复式入口，中毒对其不再有行为差异——
    ///   而这恰好就是本测试想证明的那件事。
    #[test]
    fn item_rects_survive_poisoning() {
        use windows_sys::Win32::Foundation::RECT;
        let r = |v: i32| RECT {
            left: v,
            top: 0,
            right: v + 10,
            bottom: 10,
        };
        publish_item_rects(&[r(1), r(20)]);
        assert_eq!(item_rect_count(), 2, "前置断言：正常路径先能用");

        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _g = crate::state::lock_unpoisoned(&LAST_ITEM_RECTS);
            panic!("inject: 让 LAST_ITEM_RECTS 中毒");
        }));
        assert!(LAST_ITEM_RECTS.is_poisoned(), "前置断言：此刻确实中毒");

        publish_item_rects(&[r(5)]);
        assert_eq!(
            item_rect_count(),
            1,
            "⭐ 中毒后发布**必须**生效、计数**必须**是真实值（修复前：恒 0）"
        );
        // 命中判定也须继续工作（原先 `Err(_) => return false` ⇒ 恒不命中）
        let hit = hovered_item_index(Some((7, 5)), Some((0, 0, 100, 20)));
        assert_eq!(
            hit, 0,
            "⭐ 中毒后命中判定仍须工作（修复前：恒 -1 = 滚轮无响应）"
        );
    }
}
