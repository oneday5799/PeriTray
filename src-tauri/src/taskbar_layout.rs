//! 任务栏「占用区探针」——供**自动**位置（`taskbar_position = "auto"`）使用。
//!
//! 加载序 N/N · 提供：`ensure_probe()` / `auto_slot()` / `find_free_interval()`
//! 依赖：`crate::audio::ensure_com_initialized`（全仓 COM 初始化唯一入口）
//!
//! ⭐⭐ **它问的是「控件自己的矩形」，不是「哪里看起来空」**——这是它与本仓
//!   已退役的「避让」方案的**根本区别**（退役机理 → Wiki 15 §8）：
//!   旧方案逐列扫描像素**推断**空白，得到的坐标系与用户所见无关（裁边距 ⇒
//!   靠左/右各差 100px）；本模块改为向 **UI Automation** 要交互元素的
//!   **包围盒**（开始按钮 / 已固定应用 / 运行中任务按钮 / 托盘 / 时钟），
//!   坐标来源就是用户看到的那些按钮本身 ⇒ 不存在「推断」这一步。
//! ⚠️ 思路与判据参照 `EchoMusic` 的 `native/taskbar-layout/TaskbarLayout.cs`
//!   （同一套做法：UIA 交互元素包围盒 + 经典子窗类名兜底 + `reliable` 闸门）。
//!
//! ⛔ **探针绝不在主线程跑**：UIA 是跨进程调用，提供方卡死时**不会自己返回**
//!   ——`EchoMusic` 为此把探针放进独立 `.exe` 并加 2.5s 超时杀掉。本模块改用
//!   「独立后台线程 + 结果缓存」达到同样的隔离效果：主线程只做一次纯内存读
//!   （`auto_slot`），读不到就退回整条任务栏（= 今天的行为）。
//! ⛔ **任何不确定都退回整条任务栏**，绝不猜：探针失败 / 超时 / 元素数为 0
//!   / 枚举被截断（>512）⇒ `reliable = false` ⇒ `auto_slot` 返回 `None`。
//!   依据：UIA 返回 0 个元素**不能**证明任务栏是空的（提供方缺失同样返回 0）。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use windows::core::implement;
use windows::Win32::Foundation::HWND;
use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER};
use windows::Win32::System::Variant::{
    VARIANT, VARIANT_0, VARIANT_0_0, VARIANT_0_0_0, VT_BOOL, VT_I4,
};
use windows::Win32::UI::Accessibility::{
    CUIAutomation, IUIAutomation, IUIAutomationCondition, IUIAutomationElement,
    IUIAutomationStructureChangedEventHandler, IUIAutomationStructureChangedEventHandler_Impl,
    StructureChangeType_ChildAdded, StructureChangeType_ChildRemoved,
    StructureChangeType_ChildrenBulkAdded, StructureChangeType_ChildrenBulkRemoved,
    StructureChangeType_ChildrenReordered, TreeScope_Descendants, UIA_ButtonControlTypeId,
    UIA_ControlTypePropertyId, UIA_IsKeyboardFocusablePropertyId, UIA_ListItemControlTypeId,
    UIA_TabItemControlTypeId,
};

type UIAEl = windows::Win32::UI::Accessibility::IUIAutomationElement;

/// ⭐ 一块占用：**矩形 + 它的 UIA 元素**。`None` = 无 UIA 节点（经典子窗那三块）。
type Block = (Rect, Option<UIAEl>);

/// 屏幕坐标下的一个矩形（物理像素，与 `taskbar_rect()` 同口径）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

/// 占用区里的一块。
///
/// ⭐ `element` 是**判定「真没了」的唯一凭据**：本次枚举里没出现时，拿它单独问
///   一次（`CurrentBoundingRectangle`）—— 返回有效矩形 = 只是这次没枚举到（真还在），
///   报错/空矩形 = 真的消失了。⇒ **不再需要靠时间猜**。
/// ⚠️ `None` = 这块来自 `collect_classic_children`（无 UIA 节点）⇒ 只能退回
///   按时间兜（见 `MERGE_FALLBACK_GRACE_MS`）。
#[derive(Clone)]
pub struct Entry {
    pub rect: Rect,
    pub element: Option<UIAEl>,
    /// 仅在 `element == None` 时有意义：上次真正看见它的时刻。
    pub seen: Instant,
}

/// 探针结果。
struct Probe {
    /// ⭐ **只有矩形、没有 COM 元素** —— 这样 `CACHE` 才是 `Send`，主线程才能
    ///   无锁读它（⛔ `IUIAutomationElement` 非 `Send`，放进 `static Mutex`
    ///   直接编译不过；元素只活在探针线程的局部 `tracked` 里）。
    /// ⚠️ 语义 = 「经 [`merge_observed`] 判活后的占用集合」（判据见其文档）。
    rects: Vec<Rect>,
    /// ⭐ **只有它为 `true` 才允许用这份数据避让**（判据见模块文档）。
    reliable: bool,
    stamp: Instant,
}

static CACHE: Mutex<Option<Probe>> = Mutex::new(None);
static STARTED: AtomicBool = AtomicBool::new(false);
/// 自动开着时的探针间隔（毫秒）。
///
/// ⚠️ 2s 与 `EchoMusic` 的轮询节奏同量级，且与本模块窗口的 2s 维护 tick 对齐
///   ——再快只是白烧 UIA 的跨进程调用。
const PROBE_INTERVAL_MS: u64 = 2000;
/// 自动关着时的空转间隔（毫秒）。只为「打开开关后 1 秒内就能生效」。
const IDLE_INTERVAL_MS: u64 = 1000;
/// 结果保质期（毫秒）。⛔ 过期一律当不可用：探针线程若因 UIA 卡死而停摆，
/// 指望旧数据会让窗口**卡在一个早已不存在的空隙里**（而任务栏可能已排满）。
const TTL_MS: u64 = 6000;
/// 无 UIA 元素可问时（经典子窗类名那三块）按时间兜住的时长（毫秒）。
///
/// ⛔ **只对「没有 UIA 节点」的块生效**，⛔ 不再是全局迟滞：绝大多数块都有
///   `IUIAutomationElement`，走的是**按证据判定**（见 [`Entry`]）⇒ 关闭图标后
///   **一个探针周期内就回位**（实测 ~0.4s），不再是 6s。
/// ⚠️ 取 2s = 兜底周期本身：连一个完整兜底周期都没出现，才认定它真没了。
const MERGE_FALLBACK_GRACE_MS: u64 = 2000;

/// 判「同一块」的容差（像素）。
///
/// ⚠️ 2px：任务栏按钮的矩形是像素对齐的，稳态下**不抖**；而 UIA 在 DPI 换算
///   边界上可能差 1px。⚠️ 这个值**必须远小于一个按钮的宽度**（实测 55px），
///   否则相邻两块会被认成同一块、其中一块就此永久丢失。
const RECT_MATCH_TOL_PX: i32 = 2;
/// 元素数上限。⛔ 超过即判不可靠：真实任务栏的交互元素是几十个量级，
///   达到数百说明枚举失控（或走进了某个子树的深层），此时结果不可信。
const MAX_ELEMENTS: i32 = 512;

/// 把本次采样并进历史占用区，得到**迟滞并集**。**纯函数**，可单测。
///
/// ⛔⛔ **核心判据：多一块立即生效，少一块要等宽限期。**
///   · **多一块**（新图标出现）⇒ 立刻并入 ⇒ 组件**马上**让开。这是用户要的。
///   · **少一块** ⇒ **保留**到宽限期结束才真正移除。依据：UIA 会间歇性漏报
///     单个元素（实测丢失率 ~40%，见 [`OCCUPIED_GRACE_MS`]），若「漏一次就当
///     它没了」，组件就会在「让开 / 压上去」之间来回抖 —— 那正是用户报的
///     「有新图标时不会自动避让」。
/// ⛔ 方向**不可反过来**：把宽限期用在「新增」上会让避让变慢（用户更在意这个），
///   用在「移除」上只是多避让几秒（安全方向，且无人抱怨）。
///
/// ⚠️ 已知代价：任务栏重排（开关应用）会让所有按钮平移约一个按钮宽（实测 55px）
///   ⇒ 旧位置的矩形在宽限期内仍在并集里 ⇒ 组件**晚 6s** 才移到新位置。
///   只影响观感，不产生重叠，故接受。
pub fn merge_observed<F>(
    prev: &[Entry],
    fresh: &[Block],
    now: Instant,
    mut recheck: F,
) -> Vec<Entry>
where
    F: FnMut(&Entry) -> Option<Rect>,
{
    let mut out: Vec<Entry> = Vec::with_capacity(fresh.len() + prev.len());
    // ① 本次出现的：立刻采用（刷新元素引用；`seen` 沿用旧值以便无元素时窗口继续计）
    for (rect, element) in fresh {
        let seen = prev
            .iter()
            .find(|e| same_rect(e.rect, *rect))
            .map(|e| e.seen)
            .unwrap_or(now);
        out.push(Entry {
            rect: *rect,
            element: element.clone(),
            seen,
        });
    }
    // ② 本次没出现的：**单独问一次**它还在不在（判据见文档）
    for e in prev {
        if fresh.iter().any(|(r, _)| same_rect(*r, e.rect)) {
            continue;
        }
        if let Some(rect) = recheck(e) {
            out.push(Entry {
                rect,
                element: e.element.clone(),
                seen: e.seen,
            });
        }
    }
    out
}
/// 判两块是不是同一块占用（容差比较，见 [`RECT_MATCH_TOL_PX`]）。
fn same_rect(a: Rect, b: Rect) -> bool {
    (a.left - b.left).abs() <= RECT_MATCH_TOL_PX
        && (a.right - b.right).abs() <= RECT_MATCH_TOL_PX
        && (a.top - b.top).abs() <= RECT_MATCH_TOL_PX
        && (a.bottom - b.bottom).abs() <= RECT_MATCH_TOL_PX
}

/// 懒启动探针线程（幂等）。由 `apply_from_config` 调用。
pub fn ensure_probe() {
    if STARTED.swap(true, Ordering::AcqRel) {
        return;
    }
    let spawned = std::thread::Builder::new()
        .name("pm-taskbar-probe".to_string())
        .spawn(probe_loop);
    if let Err(e) = spawned {
        STARTED.store(false, Ordering::Release);
        crate::process::append_log(&format!("[layout] 探针线程启动失败: {e}"));
    }
}

/// 探针线程是否已起（只读观察口，供验收脚本直接问这一句——「线程没起」与
/// 「起了但探针失败」在快照上都表现为「没有空隙」，光看结果分不出来）。
pub fn probe_started() -> bool {
    STARTED.load(Ordering::Acquire)
}

fn probe_loop() {
    // ⭐ UIA 会话**跨探针复用**：`CoCreateInstance` + 构造 Or 条件（4 个属性条件）
    //   每次重做一遍是纯浪费 —— 它们与任务栏内容无关，只有 COM 失联才需要重建。
    // ⭐ 上一轮**被事件唤醒**的时刻（`None` = 走兜底周期唤醒）。
    let mut woken_by_event: Option<Instant> = None;
    let mut session: Option<UiaSession> = None;
    // ⛔ `tracked` 是**线程局部**的：它带 COM 元素（非 `Send`），不进 `CACHE`。
    let mut tracked: Vec<Entry> = Vec::new();
    loop {
        let wanted = crate::config::with_config(|c| c.taskbar_position == "auto");
        if wanted {
            if session.is_none() {
                let raw = crate::taskbar_widget::taskbar_hwnd();
                if raw != 0 {
                    session = build_session(HWND(raw as *mut core::ffi::c_void));
                }
            }
            let started = Instant::now();
            let outcome = probe_once(session.as_ref());
            let cost = started.elapsed();
            // ⭐ 「事件→探针完成」= 用户真正感知的避让延迟（不含应用自身启动耗时）
            let latency = match woken_by_event {
                Some(at) => format!("{}ms", at.elapsed().as_millis()),
                None => "无(走兜底周期)".to_string(),
            };
            let mut guard = crate::state::lock_unpoisoned(&CACHE);
            match outcome {
                Some((fresh, reliable)) => {
                    let now = Instant::now();
                    let before = guard.as_ref().map(|p| p.rects.clone());
                    tracked = merge_observed(&tracked, &fresh, now, |e| recheck_entry(e, now));
                    let merged: Vec<Rect> = tracked.iter().map(|e| e.rect).collect();
                    let changed = before.as_deref() != Some(merged.as_slice());
                    // ⭐ **每次探针**都记一行 verbose：这是「自动开销」唯一可归因的
                    //   数据源（standard 那行只在集合**变化**时记，数不出探了几次）。
                    crate::process::append_verbose_log(&format!(
                        "[layout] 探针: 全量={}ms 事件累计={} 事件→探针完成={}",
                        cost.as_millis(),
                        event_hits(),
                        latency
                    ));
                    if changed {
                        verbose_log_counted(&guard, &merged, reliable, fresh.len(), cost);
                        crate::taskbar_widget::mark_slot_stale();
                    }
                    *guard = Some(Probe {
                        rects: merged,
                        reliable,
                        stamp: now,
                    });
                }
                None => {
                    // ⛔ 探针失败**必须清空**（而不是留着旧值）：调用方据此退回
                    //    整条任务栏，与「探针从来没成功过」走同一条降级路径。
                    *guard = None;
                    tracked.clear();
                    // ⛔ 会话一并丢弃：探针失败多半就是 COM 失联，下次重建。
                    session = None;
                }
            }
            drop(guard);
        }
        if !wanted {
            std::thread::sleep(Duration::from_millis(IDLE_INTERVAL_MS));
            continue;
        }
        // ⭐⭐ 等「事件」或「兜底周期」，二者谁先到算谁。
        //   事件到位 ⇒ 立刻探针（实测延迟 ~10ms + 探针 80ms）；
        //   周期到   ⇒ 兜底全量（覆盖事件覆盖不到的变化，见 `wait_for_change_or_timeout`）。
        let timeout = if session.as_ref().is_some_and(|s| s.watched) {
            Duration::from_millis(PROBE_INTERVAL_MS)
        } else {
            Duration::from_millis(PROBE_INTERVAL_MS / 4)
        };
        woken_by_event = wait_for_change_or_timeout(timeout);
    }
}

/// 占用区有变化时才打日志（逐次打会把日志刷爆——这是每 2s 一次的轮询）。
///
/// ⭐ `fresh_len` 与 `merged.len` 的差 = **「本次没枚举到、但单独问过、确认还活着」**
///   的块数（判据见 [`merge_observed`]）。它长期 > 0 说明 UIA 的枚举确实会漏，
///   此时正是「按证据判活」在替我们兜底 —— ⛔ 若去掉判活直接信枚举，这些块
///   就会让出空间、组件压上去。
fn verbose_log_counted(
    prev: &Option<Probe>,
    merged: &[Rect],
    reliable: bool,
    fresh_len: usize,
    cost: Duration,
) {
    let changed = match prev {
        Some(p) => p.reliable != reliable || p.rects != merged,
        None => true,
    };
    if changed {
        crate::process::append_log(&format!(
            "[layout] 占用区更新: 判活后={} 本次采到={} 问出还活着={} reliable={reliable} 耗时={}ms",
            merged.len(),
            fresh_len,
            merged.len().saturating_sub(fresh_len),
            cost.as_millis()
        ));
        // ⚠️ 起止 x 也进 verbose：排查「空隙放不下」时必须能看到邻居在哪 ——
        //   只报个数的话，「为什么没避让」永远只能靠猜（实测踩到）。
        let spans: Vec<String> = merged
            .iter()
            .map(|r| format!("{}..{}", r.left, r.right))
            .collect();
        crate::process::append_verbose_log(&format!("[layout] 占用区明细: [{}]", spans.join(", ")));
    }
}

/// 查找「自动」可用区的唯一入口。
///
/// 返回 `(可用区左端屏幕 x, 可用区宽)`；`None` = 调用方应退回整条任务栏。
/// ⛔ 只做纯内存读：**主线程会调它**，任何跨进程调用都不许出现在这里。
pub fn auto_slot(bar_left: i32, bar_w: i32, want_w: i32, margin: i32) -> Option<(i32, i32)> {
    let guard = crate::state::lock_unpoisoned(&CACHE);
    let p = guard.as_ref()?;
    if !p.reliable || p.stamp.elapsed() > Duration::from_millis(TTL_MS) {
        return None;
    }
    find_free_interval(bar_left, bar_w, &p.rects, want_w, margin)
}

/// 在任务栏上找一个「放得下 `want_w` 的空隙」。**纯函数**，可单测。
///
/// ⭐ 判据（与 `EchoMusic` 的 `findHorizontalSpace` 同源）：
///   ① 每个占用矩形**向两侧各扩 `margin`** ⇒ 与邻居之间留出视觉间距；
///   ② 扫掠合并重叠区间，收集空隙；
///   ③ 取**最左**的、**放得下完整 `want_w`** 的那个。
/// ⛔ **不缩进窄空隙**（`EchoMusic` 那一版允许缩到 `minimumWidth`）：
///   本仓曾因「53px 窄槽」把内容截断而被用户否掉（见 `draw_items` 的宽度注释）
///   ⇒ 放不下就**不避让**，退回整条任务栏，由日志说明原因。
/// ⛔ 取最左而不是最宽：让落点**稳定**（任务栏上开关应用不会让它左右跳），
///   也更可预测 —— 与 `EchoMusic` 的 `gaps.find(..)` 同款。
pub fn find_free_interval(
    bar_left: i32,
    bar_w: i32,
    occupied: &[Rect],
    want_w: i32,
    margin: i32,
) -> Option<(i32, i32)> {
    let start = bar_left + margin;
    let end = bar_left + bar_w - margin;
    if end <= start || want_w <= 0 {
        return None;
    }
    let mut intervals: Vec<(i32, i32)> = occupied
        .iter()
        .map(|r| ((r.left - margin).max(start), (r.right + margin).min(end)))
        .filter(|(a, b)| b > a)
        .collect();
    intervals.sort_unstable();
    let mut cursor = start;
    for (a, b) in intervals {
        if a > cursor && a - cursor >= want_w {
            return Some((cursor, a - cursor));
        }
        cursor = cursor.max(b);
    }
    if end - cursor >= want_w {
        return Some((cursor, end - cursor));
    }
    None
}

// ── 探针本体 ──────────────────────────────────────────────

/// ⭐ 可跨探针复用的 UIA 会话（自动化对象 + 那条 Or 条件）。
///
/// ⚠️ 二者都**与任务栏内容无关**（一个是 coclass 实例，一个是四个属性条件的组合），
///   所以反复重建是纯浪费。⚠️ 生命周期**只属于探针线程**（COM 接口非 `Send`），
///   ⛔ 不得挪进 `static`。
struct UiaSession {
    uia: IUIAutomation,
    cond: IUIAutomationCondition,
    /// ⭐ 监听是否挂上了。`false` ⇒ **只能靠 `PROBE_INTERVAL_MS` 兜底**
    ///   （事件驱动这条路在本机不可用时不会静默失效，只表现为延迟）。
    watched: bool,
}

/// 建一次 UIA 会话。`None` = COM 不可用（调用方按探针失败处理）。
fn build_session(bar: HWND) -> Option<UiaSession> {
    unsafe {
        // ⚠️ 复用全仓 COM 初始化唯一入口（幂等，且已处理 MTA 冲突的日志）。
        crate::audio::ensure_com_initialized();
        let uia = CoCreateInstance::<_, IUIAutomation>(&CUIAutomation, None, CLSCTX_INPROC_SERVER)
            .ok()?;
        let cond = interactive_condition(&uia)?;
        let watched = watch_structure_changes(
            &UiaSession {
                uia: uia.clone(),
                cond: cond.clone(),
                watched: false,
            },
            bar,
        )
        .is_ok();
        if !watched {
            crate::process::append_log(
                "[layout] 结构变更监听挂不上 ⇒ 只剩兜底轮询（避让延迟会变大）",
            );
        }
        Some(UiaSession { uia, cond, watched })
    }
}

/// 占用区「可能变了」标志（由事件回调置位，由探针循环消费）。
static DIRTY: AtomicBool = AtomicBool::new(false);

thread_local! {
    /// 上一轮**被事件唤醒**的时刻（`None` = 走兜底周期唤醒）。
    /// ⛔ 只在本探针线程读写：`thread_local!` 天然免掉 `static` 的 `Sync` 约束。
    static EVENT_WAITED_AT: std::cell::RefCell<Option<Instant>> = const { std::cell::RefCell::new(None) };
}

/// 收到过几次结构变更事件（**只读观察口**）。
///
/// ⭐ 存在的理由：`AddStructureChangedEventHandler` **在 XAML 任务栏上到底会不会
///   触发，事先无法确定**（应用按钮不是 HWND，是 `MSTaskListWClass` 里 XAML 画的）。
///   ⇒ 必须留一个能直接回答「事件到底有没有在响」的计数，否则线上只能靠
///   「用户说没及时避让」反推。
static EVENT_HITS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// 事件到底有没有在响（只读观察口）。
pub fn event_hits() -> u64 {
    EVENT_HITS.load(Ordering::Acquire)
}

/// ⭐ UIA「结构变更」监听器：任务栏上**增删/重排子元素**时立刻唤醒探针。
///
/// ⛔⛔ **为什么必须走事件、不能靠轮询**（本机实测，两组数）：
///   · 一次**全量**探针 **79~161ms**（23 元素 × 3 属性 = 69 次跨进程调用）；
///   · 连「只枚举 + 取长度」也要 **34ms**（占全量 43%）—— 枚举本身就贵。
///   ⇒ 想更快就只能更频繁地烧 CPU：500ms 一次 = 12~34% 的一个核。
///   而事件是**提供方主动推**，空闲时零成本。
#[implement(IUIAutomationStructureChangedEventHandler)]
struct StructureChangeListener;

impl IUIAutomationStructureChangedEventHandler_Impl for StructureChangeListener_Impl {
    fn HandleStructureChangedEvent(
        &self,
        _sender: windows::core::Ref<IUIAutomationElement>,
        change_type: windows::Win32::UI::Accessibility::StructureChangeType,
        _runtime_id: *const windows::Win32::System::Com::SAFEARRAY,
    ) -> windows::core::Result<()> {
        // ⛔ **只认「子元素增删 / 重排」**：`ChildrenInvalidated`（= 2）在 XAML 上
        //   触发极频繁且几乎不代表布局变化，认它等于把探针变成全程满负荷。
        if is_layout_change(change_type) {
            DIRTY.store(true, Ordering::Release);
            EVENT_HITS.fetch_add(1, Ordering::AcqRel);
        }
        Ok(())
    }
}

/// ⭐ 这个结构变更**值不值得唤醒探针**（纯函数，可单测）。
///
/// ⛔⛔ **`ChildrenInvalidated`（= 2）必须排除** —— 这是本函数唯一承重的判断：
///   本机实测任务栏 XAML 在**空闲时也持续**发事件（15s 内 +59 次），而其中
///   绝大多数是它。认它 ⇒ 探针被事件灌满（本机实测 0.8 次/秒、每次 30~95ms）
///   ⇒ 纯烧 CPU，且**不带来任何更快的避让**。
///   依据：真正对应「图标增删」的是 `ChildAdded` / `ChildRemoved` /
///   `ChildrenBulkAdded` / `ChildrenBulkRemoved`，以及布局重排
///   `ChildrenReordered`（开关应用会把后面的按钮整体推右，必须算）。
///
/// ⚠️ 用 `.0` 比较而不是 `matches!`：这些是 windows crate 生成的
///   `StructureChangeType_*`（**小写名**的常量），写在**模式位置**会触发
///   `non_upper_case_globals`（仓库按零警告要求拒绝）。
fn is_layout_change(change_type: windows::Win32::UI::Accessibility::StructureChangeType) -> bool {
    let c = change_type.0;
    c == StructureChangeType_ChildAdded.0
        || c == StructureChangeType_ChildRemoved.0
        || c == StructureChangeType_ChildrenBulkAdded.0
        || c == StructureChangeType_ChildrenBulkRemoved.0
        || c == StructureChangeType_ChildrenReordered.0
}

/// 在任务栏根元素上挂结构变更监听。返回 `Err` = 挂不上（调用方靠轮询兜底）。
fn watch_structure_changes(sess: &UiaSession, bar: HWND) -> windows::core::Result<()> {
    unsafe {
        let root = sess.uia.ElementFromHandle(bar)?;
        let handler: IUIAutomationStructureChangedEventHandler = StructureChangeListener.into();
        sess.uia
            .AddStructureChangedEventHandler(&root, TreeScope_Descendants, None, &handler)
    }
}

/// ⭐ 抽干消息队列（**事件回调靠它送达**）。
///
/// ⚠️ ⛔ **不抽 = 事件永远不回调**：UIA 的跨线程事件是通过消息队列投递的，
///   没有消息泵就等于没挂监听。⚠️ 因此本函数取代了原来的 `thread::sleep`
///   ——`thread::sleep` 期间消息不会被处理。
/// 返回是否收到了 `WM_QUIT`（收到就该退出线程）。
fn pump_messages() -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        DispatchMessageW, PeekMessageW, TranslateMessage, PM_REMOVE, WM_QUIT,
    };
    let mut msg =
        std::mem::MaybeUninit::<windows_sys::Win32::UI::WindowsAndMessaging::MSG>::uninit();
    loop {
        let got = unsafe { PeekMessageW(msg.as_mut_ptr(), std::ptr::null_mut(), 0, 0, PM_REMOVE) };
        if got == 0 {
            return false;
        }
        let msg = unsafe { msg.assume_init() };
        if msg.message == WM_QUIT {
            return true;
        }
        unsafe {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

/// ⭐ 等到「有事件」或「到兜底周期」。返回是否应立刻探针。
///
/// ⛔ **必须有兜底周期**：结构变更事件覆盖不到「元素数量没变、只是整体平移」
///   这类变化（实测开关应用会把它后面的按钮整体推右）⇒ 没有兜底就会漏避让。
const PUMP_SLICE_MS: u64 = 10;

fn wait_for_change_or_timeout(timeout: Duration) -> Option<Instant> {
    let deadline = Instant::now() + timeout;
    loop {
        if pump_messages() {
            // WM_QUIT：仍按「该探针了」处理，让上层走正常退出路径
            return None;
        }
        if DIRTY.swap(false, Ordering::AcqRel) {
            // ⭐ 记下**被事件唤醒**的时刻：与探针完成时刻一减，就得到
            //   「事件驱动本身的响应」—— 这段才是用户感知的避让延迟。
            return Some(Instant::now());
        }
        if Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(Duration::from_millis(PUMP_SLICE_MS));
    }
}

/// ⭐ 问一条「本次没枚举到」的条目：它**到底还在不在**？返回它**现在**的矩形。
///
/// ⛔⛔ 这是把「关闭方向 6s」压到一个探针周期的全部依据 —— **按证据判活，
///   不按时间猜**。返回 `None` = 真没了。
///
/// ⭐ 顺带治好了「按钮动画平移留一堆中间位置」：返回的是它**当前**的矩形，
///   于是调用方直接更新位置，而不是把每个中间位置都当成新块攒起来
///   （实测那些「漏报」多半是动画中间态：`336..376 → 336..377`、`365..405`）。
///
/// ⚠️ 空矩形（宽或高 ≤ 0）判为「真没了」：UIA 对已移除的元素会返回空矩形或报错，
///   两者都视为不在。⚠️ 无 UIA 元素可问时（经典子窗那三块）退回按时间兜。
fn recheck_entry(e: &Entry, now: Instant) -> Option<Rect> {
    if let Some(el) = &e.element {
        let r = unsafe { el.CurrentBoundingRectangle() }.ok()?;
        if r.right <= r.left || r.bottom <= r.top {
            return None;
        }
        return Some(Rect {
            left: r.left,
            top: r.top,
            right: r.right,
            bottom: r.bottom,
        });
    }
    (now.saturating_duration_since(e.seen) <= Duration::from_millis(MERGE_FALLBACK_GRACE_MS))
        .then_some(e.rect)
}

/// 采一次占用区。返回 `None` = 连任务栏都没拿到（调用方清缓存）。
fn probe_once(session: Option<&UiaSession>) -> Option<(Vec<Block>, bool)> {
    let raw = crate::taskbar_widget::taskbar_hwnd();
    if raw == 0 {
        return None;
    }
    let bar = HWND(raw as *mut core::ffi::c_void);
    let (bar_left, bar_top, bar_w, bar_h) = crate::taskbar_widget::taskbar_rect_tuple()?;
    let bar_box = Rect {
        left: bar_left,
        top: bar_top,
        right: bar_left + bar_w,
        bottom: bar_top + bar_h,
    };
    let mut rects: Vec<Block> = Vec::new();
    collect_classic_children(bar.0, bar_box, &mut rects);
    let (uia_rects, uia_complete) = collect_uia(session?, bar, bar_box);
    rects.extend(uia_rects);
    rects.retain(|(r, _)| r.right > r.left && r.bottom > r.top);
    // ⛔ 必须在算空隙**之前**做（判据见 `keep_innermost`）。
    keep_innermost(&mut rects);
    rects.sort_unstable_by_key(|(r, _)| (r.left, r.right));
    // ⭐ 判据：**既要有结果、又要枚举完整**。UIA 返回 0 个元素不能证明任务栏
    //   是空的（提供方缺失同样返回 0）⇒ 此时宁可不用避让。
    let reliable = uia_complete && !rects.is_empty();
    Some((rects, reliable))
}

/// 经典托盘区：这些区域在 Win10 上**不暴露单个 UIA 节点**（`EchoMusic` 实测
/// 同样结论），必须按子窗类名单独取。
fn collect_classic_children(
    bar: windows_sys::Win32::Foundation::HWND,
    bar_box: Rect,
    out: &mut Vec<Block>,
) {
    use windows_sys::Win32::Foundation::{HWND as SysHwnd, LPARAM, RECT};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        EnumChildWindows, GetClassNameW, GetWindowRect, GetWindowThreadProcessId, IsWindowVisible,
    };
    // 三个类名与判据来源：`EchoMusic` 的 `ReadBar`（同一份清单）。
    const CLASSES: [&str; 3] = ["TrayNotifyWnd", "TrayClockWClass", "ShowDesktopButton"];
    struct Ctx<'a> {
        bar: Rect,
        our_pid: u32,
        out: &'a mut Vec<Block>,
    }
    unsafe extern "system" fn cb(child: SysHwnd, lparam: LPARAM) -> i32 {
        let ctx = &mut *(lparam as *mut Ctx);
        let mut buf = [0u16; 256];
        let n = GetClassNameW(child, buf.as_mut_ptr(), buf.len() as i32);
        if n <= 0 {
            return 1;
        }
        let name = String::from_utf16_lossy(&buf[..n as usize]);
        if !CLASSES.contains(&name.as_str()) {
            return 1;
        }
        // ⛔ **排除自己**：widget 是任务栏的**子窗口**，不排除就会被算成「已占用」
        //   ⇒ 窗口会围着上次的位置躲自己（`EchoMusic` 无此问题：它的窗是顶层窗）。
        let mut pid = 0u32;
        GetWindowThreadProcessId(child, &mut pid);
        if pid == ctx.our_pid {
            return 1;
        }
        if IsWindowVisible(child) == 0 {
            return 1;
        }
        let mut r: RECT = std::mem::zeroed();
        if GetWindowRect(child, &mut r) == 0 {
            return 1;
        }
        let rect = Rect {
            left: r.left,
            top: r.top,
            right: r.right,
            bottom: r.bottom,
        };
        if intersects(&ctx.bar, &rect) {
            ctx.out.push((rect, None));
        }
        1
    }
    let mut ctx = Ctx {
        bar: bar_box,
        our_pid: std::process::id(),
        out,
    };
    unsafe {
        EnumChildWindows(bar, Some(cb), &mut ctx as *mut Ctx as LPARAM);
    }
}

/// UIA：拿交互元素的**包围盒**。返回 `(矩形, 枚举是否完整)`。
fn collect_uia(sess: &UiaSession, bar: HWND, bar_box: Rect) -> (Vec<Block>, bool) {
    let mut out: Vec<Block> = Vec::new();
    let our_pid = std::process::id() as i32;
    unsafe {
        // ⚠️ 复用全仓 COM 初始化唯一入口（它幂等，且已处理 MTA 冲突的日志）。
        //   ⛔ 不新开一条初始化路径：两处各初始化一次会在同一线程上互相干扰。
        let uia = &sess.uia;
        let Ok(root) = uia.ElementFromHandle(bar) else {
            return (out, false);
        };
        let Ok(found) = root.FindAll(TreeScope_Descendants, &sess.cond) else {
            return (out, false);
        };
        let Ok(count) = found.Length() else {
            return (out, false);
        };
        // ⛔ 见 `MAX_ELEMENTS`：截断 ⇒ 判不可靠（宁可不用避让，也不按残缺数据避让）。
        let complete = count <= MAX_ELEMENTS;
        let mut seen = 0i32;
        while seen < count.min(MAX_ELEMENTS) {
            let Ok(el) = found.GetElement(seen) else {
                return (out, false);
            };
            seen += 1;
            // ⛔ 排除自己（同上；UIA 侧按 pid 判，比类名可靠）。
            if el.CurrentProcessId().map(|p| p == our_pid).unwrap_or(true) {
                continue;
            }
            if el.CurrentIsOffscreen().map(|b| b.as_bool()).unwrap_or(true) {
                continue;
            }
            let Ok(r) = el.CurrentBoundingRectangle() else {
                continue;
            };
            let rect = Rect {
                left: r.left,
                top: r.top,
                right: r.right,
                bottom: r.bottom,
            };
            if rect.right > rect.left && rect.bottom > rect.top && intersects(&bar_box, &rect) {
                out.push((rect, Some(el.clone())));
            }
        }
        (out, complete)
    }
}

/// `EchoMusic` 的四条 Or 条件（可聚焦 ∨ 按钮 ∨ 列表项 ∨ 选项卡项）。
/// ⛔ 少了任何一条都会漏掉一类任务栏控件（例如开始按钮是 Button、
///   Win11 应用按钮是 ListItem、小组件是 TabItem）。
unsafe fn interactive_condition(uia: &IUIAutomation) -> Option<IUIAutomationCondition> {
    let focusable = uia
        .CreatePropertyCondition(UIA_IsKeyboardFocusablePropertyId, &var_bool(true))
        .ok()?;
    let ctrl = |id: windows::Win32::UI::Accessibility::UIA_CONTROLTYPE_ID| {
        uia.CreatePropertyCondition(UIA_ControlTypePropertyId, &var_i32(id.0))
            .ok()
    };
    let button = ctrl(UIA_ButtonControlTypeId)?;
    let listitem = ctrl(UIA_ListItemControlTypeId)?;
    let tabitem = ctrl(UIA_TabItemControlTypeId)?;
    uia.CreateOrConditionFromNativeArray(&[
        Some(focusable),
        Some(button),
        Some(listitem),
        Some(tabitem),
    ])
    .ok()
}

/// ⚠️ `windows` crate 0.62 **没有** `VARIANT` 的 `From<bool>` / `From<i32>`
///   （实测：`variant.rs` 里零 `impl From`）⇒ 只能按 `VT_*` 手工构造。
/// ⛔ `VARIANT_0.Anonymous` 是 `ManuallyDrop<VARIANT_0_0>` ⇒ 必须用
///   `ManuallyDrop::new(..)` 赋值（Rust 不会对 union 里的 `ManuallyDrop`
///   自动 `DerefMut`，直接改字段是编译错误）。
unsafe fn var_bool(v: bool) -> VARIANT {
    VARIANT {
        Anonymous: VARIANT_0 {
            Anonymous: core::mem::ManuallyDrop::new(VARIANT_0_0 {
                vt: VT_BOOL,
                wReserved1: 0,
                wReserved2: 0,
                wReserved3: 0,
                Anonymous: VARIANT_0_0_0 {
                    boolVal: windows::Win32::Foundation::VARIANT_BOOL(if v { -1 } else { 0 }),
                },
            }),
        },
    }
}

unsafe fn var_i32(v: i32) -> VARIANT {
    VARIANT {
        Anonymous: VARIANT_0 {
            Anonymous: core::mem::ManuallyDrop::new(VARIANT_0_0 {
                vt: VT_I4,
                wReserved1: 0,
                wReserved2: 0,
                wReserved3: 0,
                Anonymous: VARIANT_0_0_0 { lVal: v },
            }),
        },
    }
}

fn intersects(a: &Rect, b: &Rect) -> bool {
    a.left < b.right && a.right > b.left && a.top < b.bottom && a.bottom > b.top
}

/// ⛔⛔ **只留最内层矩形**（容器过滤）—— 少了它，避让**永远不会生效**。
///
/// 依据（本机实测）：UIA 的平铺 `Descendants` 查询除了真按钮，还会返回**容器**
///   —— 实测抓到 `789..2124`（任务栏中段容器）与 `2085..2560`（托盘容器）两块
///   大矩形。容器一进占用列表，`find_free_interval` 就认为「到处都被占」⇒
///   每次都退回整条任务栏，而**日志里只说「空隙放不下」**，看不出真因。
/// ⚠️ `EchoMusic` 用的是同一套条件（可聚焦 ∨ Button ∨ ListItem ∨ TabItem），
///   因此它同样会抓到容器；本仓按下面的判据多走一步。
///
/// 判据：一个矩形若**包含**了另一个矩形，它就是外层容器 ⇒ 删掉它，
///   保留被它包含的真元素。
/// ⛔ 必须用「完整包含」而不是「相交」：相邻按钮在边界上会相交 1~2px
///   （实测 `404..459` 与 `459..514` 相接），按相交删会把真按钮删光。
/// ⚠️ 相等的两个矩形**不算包含**（`o != inner` 拦住）：否则互为外层、
///   成对消失（重复元素应当保留，最多让空隙略窄）。
fn keep_innermost(rects: &mut Vec<Block>) {
    let all = rects.clone();
    rects.retain(|o| {
        !all.iter().any(|(inner, _)| {
            inner != &o.0
                && o.0.left <= inner.left
                && o.0.right >= inner.right
                && o.0.top <= inner.top
                && o.0.bottom >= inner.bottom
        })
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(left: i32, right: i32) -> Rect {
        Rect {
            left,
            top: 0,
            right,
            bottom: 40,
        }
    }

    /// ⭐ 基本盘：任务栏左右两端各自留出 `margin`，占用块向两侧各扩 `margin`。
    ///
    /// 依据：`EchoMusic` 的 `findHorizontalSpace` 同款口径 —— 区间是
    /// `[r.x - margin, right(r) + margin]`，故 100..200 的按钮在 `margin=8`
    /// 下占住 92..208。
    #[test]
    fn free_interval_respects_margins_on_both_sides() {
        // 任务栏 0..1000，占用 100..200；margin=8
        // ⇒ 空隙：8..92（84 宽）、208..992（784 宽）
        let occupied = [r(100, 200)];
        // 放得下 84 ⇒ 取**最左**那个
        assert_eq!(
            find_free_interval(0, 1000, &occupied, 84, 8),
            Some((8, 84)),
            "应取最左的、放得下的空隙"
        );
        // 放不下 84（要 85）⇒ 跳到右边那个大的
        assert_eq!(
            find_free_interval(0, 1000, &occupied, 85, 8),
            Some((208, 784)),
            "左侧放不下时应落到下一个能放下的空隙"
        );
    }

    /// ⛔⛔ **放不下就不避让**（返回 `None`）—— 不许缩进窄空隙。
    ///
    /// 依据：本仓曾因「避让后只剩 53px 的窄槽」把内容截断而被用户否掉
    /// （见 `fit_width` 的注释）。`EchoMusic` 那一版允许缩到 `minimumWidth`，
    /// 本仓**刻意不抄**这一条。
    #[test]
    fn free_interval_refuses_to_shrink_into_a_narrow_gap() {
        // 整条被占满（只留 8px 两端边距）
        assert_eq!(find_free_interval(0, 1000, &[r(0, 1000)], 84, 8), None);
        // 唯一空隙 40 宽 < 需要的 84
        assert_eq!(
            find_free_interval(0, 1000, &[r(0, 500), r(540, 1000)], 84, 8),
            None,
            "空隙不够就必须返回 None（交给调用方退回整条任务栏），不得缩窄"
        );
    }

    /// ⭐ 相邻/重叠的占用块必须先合并再算空隙，否则会把「块与块之间的细缝」
    /// 误当成可用空隙（真机表现：窗口插进两个按钮中间、把两个都压住）。
    #[test]
    fn free_interval_merges_overlapping_occupied_blocks() {
        // 100..200 与 190..300 重叠 ⇒ 合并成 100..300
        // margin=8 ⇒ 占住 92..308；1000 宽栏的空隙只有 308..992（684）
        assert_eq!(
            find_free_interval(0, 1000, &[r(100, 200), r(190, 300)], 400, 8),
            Some((308, 684)),
            "重叠块之间的细缝不得被当成空隙"
        );
        // 若不合并会得到 (208, 84) 这种「细缝」，放得下 400 才怪 ⇒ 用 400 当需求
        // 就能把「没合并」的实现筛出来。
    }

    /// ⛔⛔ **容器过滤**：少了它避让永远不生效（真机实测的真因）。
    ///
    /// 样本直接取本机实测的占用列表（`789..2124` 是中段容器、`2085..2560` 是
    /// 托盘容器，两者都完整包含别的矩形）。
    /// 判别力：去掉过滤 ⇒ `789..2124` 会占满中段 ⇒ 后面那条
    /// ⛔⛔ **容器过滤**：少了它避让永远不生效（真机实测的真因）。
    ///
    /// 样本直接取本机实测的占用列表（`789..2124` 是中段容器、`2085..2560` 是
    /// 托盘容器，两者都完整包含别的矩形）。
    /// 判别力：去掉过滤 ⇒ `789..2124` 会占满中段 ⇒ 后面那条
    /// 「过滤后能找到 1110 宽空隙」的断言立刻转红。
    #[test]
    fn keep_innermost_drops_containers_but_keeps_adjacent_buttons() {
        let mut rects: Vec<(Rect, Option<UIAEl>)> = vec![
            (r(789, 2124), None),  // 中段容器（包含下面几条）
            (r(1899, 1939), None), // 容器内的真元素
            (r(1939, 1979), None),
            (r(2085, 2560), None), // 托盘容器（包含下面几条）
            (r(2124, 2164), None), // 容器内的真元素
            (r(2545, 2560), None),
            (r(0, 69), None),    // 开始按钮（谁都不包含）
            (r(404, 459), None), // 相邻按钮：边界相接，⛔ 不得被当成容器
            (r(459, 514), None),
        ];
        keep_innermost(&mut rects);
        let spans: Vec<(i32, i32)> = rects.iter().map(|(x, _)| (x.left, x.right)).collect();
        assert!(
            !spans.contains(&(789, 2124)),
            "完整包含别人的中段容器必须删掉"
        );
        assert!(!spans.contains(&(2085, 2560)), "托盘容器必须删掉");
        for keep in [(1899, 1939), (2124, 2164), (0, 69), (404, 459), (459, 514)] {
            assert!(spans.contains(&keep), "{keep:?} 是真元素，不得被删");
        }
    }

    /// ⭐ 容器过滤后，真机上那段「中段 789..1899」必须能被认成空隙。
    /// ⭐ 容器过滤后，真机上那段「中段 789..1899」必须能被认成空隙。
    /// （过滤前它被容器 `789..2124` 盖住 ⇒ 永远找不到空隙。）
    #[test]
    fn container_filter_reveals_the_real_gap_sample() {
        let mut rects: Vec<(Rect, Option<UIAEl>)> = vec![
            (r(789, 2124), None),
            (r(1899, 1939), None),
            (r(2085, 2560), None),
            (r(2124, 2164), None),
            (r(0, 69), None),
        ];
        keep_innermost(&mut rects);
        let rects: Vec<Rect> = rects.into_iter().map(|(r, _)| r).collect();
        // 真机 music 面板的自然宽度是 241；空隙 = 69+8 .. 1899-8 = 77..1891
        let got = find_free_interval(0, 2560, &rects, 241, 8);
        assert_eq!(
            got,
            Some((77, 1814)),
            "过滤容器后应找到 77 起的空隙（到 1891，两端各留 margin）"
        );
    }

    /// ⭐⭐ 间距必须让组件底衬到邻居**图标**的距离 = 原生**图标↔图标**的距离。
    ///
    /// 依据（本机 125% DPI 逐像素实测，可复核）：槽位节距 **55px**（= 44 DIP）、
    ///   图标宽度 **30px**（= 24 DIP）⇒ 图标每侧内缩 **13px**、图标间距 **25px**。
    ///   调用点传的 `margin` = `20 DIP − 10 DIP` ⇒ 12.5px ⇒ 组件离邻居图标
    ///   `12.5 + 13 = 25.5px` ≈ 原生 25px。
    ///
    /// ⛔⛔ **两个值被用户当场判过，⛔ 不得回退**：
    ///   · `margin = 0` ⇒ 组件离邻居图标只有 **13px** —— 用户原话「间距不可能为0」；
    ///   · `margin = 20` ⇒ **33px** —— 用户原话「间距还是有点大」。
    /// ⇒ 改这个值之前先看这三个数，别再拿「槽位相接所以贴齐」推理：
    ///   **槽位相接 ≠ 底衬相接**（原生底衬画在槽位**内部**），这是踩过的坑。
    #[test]
    fn free_interval_margin_lands_the_backdrop_at_the_native_icon_gap() {
        // 槽位 734..789（节距 55）、其图标 747..776（每侧内缩 13）
        let occupied = [r(0, 69), r(72, 347), r(349, 789)];
        // ① 原生间距 = 25 ⇒ 组件左缘应落在 776 + 25 = 801 ⇒ margin = 801 - 789 = 12
        assert_eq!(
            // 宽度 1747：右边距也吃 12（end = 2560 - 12）
            find_free_interval(0, 2560, &occupied, 200, 12),
            Some((801, 1747)),
            "间距口径 = 原生图标↔图标 25px（用户判过 0 和 20 都错）"
        );
        // ② 被用户否掉的两种取值，留作回归护栏
        assert_eq!(
            find_free_interval(0, 2560, &occupied, 200, 0).map(|x| x.0),
            Some(789),
            "margin 0 ⇒ 组件离邻居图标只有 13px，用户明确说过不可能"
        );
        assert_eq!(
            find_free_interval(0, 2560, &occupied, 200, 20).map(|x| x.0),
            Some(809),
            "margin 20 ⇒ 离邻居图标 33px，用户说过还是有点大"
        );
    }

    /// ⛔⛔ `ChildrenInvalidated` 必须被排除 —— 判别力全在这一条上。
    ///
    /// 依据（本机实测）：空闲 15s 内任务栏 XAML 发了 **59 次**结构变更事件，
    ///   绝大多数是它。认它 ⇒ 探针被灌满（实测 0.8 次/秒 × 30~95ms，纯烧 CPU）
    ///   且**避让一点不变快** —— 真正的图标增删本来就会被 `ChildAdded` /
    ///   `ChildRemoved` 覆盖。
    #[test]
    fn is_layout_change_excludes_invalidated_but_keeps_every_real_change() {
        use windows::Win32::UI::Accessibility::{
            StructureChangeType, StructureChangeType_ChildAdded, StructureChangeType_ChildRemoved,
            StructureChangeType_ChildrenBulkAdded, StructureChangeType_ChildrenBulkRemoved,
            StructureChangeType_ChildrenInvalidated, StructureChangeType_ChildrenReordered,
        };
        // 认「图标增删」与「布局重排」
        for t in [
            StructureChangeType_ChildAdded,
            StructureChangeType_ChildRemoved,
            StructureChangeType_ChildrenBulkAdded,
            StructureChangeType_ChildrenBulkRemoved,
            StructureChangeType_ChildrenReordered,
        ] {
            assert!(is_layout_change(t), "{:?} 必须唤醒探针", t);
        }
        // ⛔ 排除「整棵子树失效」：XAML 空闲时也刷这个
        assert!(
            !is_layout_change(StructureChangeType_ChildrenInvalidated),
            "ChildrenInvalidated 必须排除，否则空闲时被事件灌满（实测 15s 内 59 次）"
        );
        // 未知值也不认（防御：宁可漏一次、走兜底周期，也不被噪声灌满）
        assert!(
            !is_layout_change(StructureChangeType(99)),
            "未知变更类型应保守忽略，靠兜底周期兜住"
        );
    }

    /// ⛔ 退化输入不得 panic、不得返回负数宽度。
    #[test]
    fn free_interval_handles_degenerate_inputs() {
        assert_eq!(
            find_free_interval(0, 10, &[], 84, 8),
            None,
            "栏比两边距还窄"
        );
        assert_eq!(find_free_interval(0, 1000, &[], 0, 8), None, "需求宽度为 0");
        assert_eq!(
            find_free_interval(0, 1000, &[], -5, 8),
            None,
            "需求宽度为负"
        );
        // 空占用 + 够宽 ⇒ 整条（扣掉两端边距）
        assert_eq!(find_free_interval(0, 1000, &[], 100, 8), Some((8, 984)));
    }

    /// ⭐ 屏幕坐标：任务栏不在 0 起点（副屏/负坐标）时同样成立。
    /// 依据：`taskbar_rect()` 给的是屏幕物理坐标，副屏可能为负。
    #[test]
    fn free_interval_works_in_negative_screen_coordinates() {
        // 副屏任务栏 -1000..0，占用 -900..-800，margin=8 ⇒ 占住 -908..-792
        // 左空隙 -992..-908（84 宽）⇒ 要 84 就命中它
        assert_eq!(
            find_free_interval(-1000, 1000, &[r(-900, -800)], 84, 8),
            Some((-992, 84)),
            "负坐标下同样按「最左且放得下」取"
        );
        // ⚠️ 要 100（左空隙放不下）⇒ 应跳到右侧 784 宽那个，而不是返回左空隙
        assert_eq!(
            find_free_interval(-1000, 1000, &[r(-900, -800)], 100, 8),
            Some((-792, 784)),
            "左空隙不够时应落到右空隙（负坐标下不得算错边界）"
        );
    }

    /// ⛔ `reliable` 闸门：探针没数据 / 不可信 / 过期时**一律退回整条任务栏**。
    ///
    /// 判别力：`auto_slot` 是唯一入口，这三条任一成立都必须返回 `None`
    /// （调用方据此退回）。⛔ 若把它写成「忽略 reliable 直接用」，
    /// 「占用区为空 ⇒ 整条任务栏都算空隙」就会让窗口压住所有图标。
    #[test]
    fn auto_slot_requires_a_reliable_fresh_probe() {
        // 清空缓存 ⇒ 无数据
        *crate::state::lock_unpoisoned(&CACHE) = None;
        assert_eq!(auto_slot(0, 1000, 100, 8), None, "无数据必须退回");

        // 有数据但不可信
        *crate::state::lock_unpoisoned(&CACHE) = Some(Probe {
            rects: vec![r(100, 200)],
            reliable: false,
            stamp: Instant::now(),
        });
        assert_eq!(auto_slot(0, 1000, 100, 8), None, "不可信必须退回");

        // 可信但已过期
        *crate::state::lock_unpoisoned(&CACHE) = Some(Probe {
            rects: vec![r(100, 200)],
            reliable: true,
            stamp: Instant::now() - Duration::from_millis(TTL_MS + 1),
        });
        assert_eq!(auto_slot(0, 1000, 100, 8), None, "过期必须退回");

        // 可信且新鲜 ⇒ 正常给空隙
        *crate::state::lock_unpoisoned(&CACHE) = Some(Probe {
            rects: vec![r(100, 200)],
            reliable: true,
            stamp: Instant::now(),
        });
        assert_eq!(auto_slot(0, 1000, 100, 8), Some((208, 784)));
        // ⛔ 收尾：不留状态给别的用例（用例间共享同一份静态缓存）
        *crate::state::lock_unpoisoned(&CACHE) = None;
    }

    // ── 迟滞并集（`merge_observed`）────────────────────────────────────
    // ⛔⛔ 这组是「组件不会自动避让」的**判别力所在**：本机实测 UIA 对同一块
    //   `789..844` **10 次采样丢 4 次**。若「漏一次就当它没了」，组件就在
    //   「让开 / 压上去」之间抖动 —— 用户报的正是这个。

    /// ⛔⛔ **方向不对称，但不再靠时间**：多一块立即生效；少一块**问过它**再弃。
    ///
    /// 依据（本机实测，两组）：
    ///   · 「本次没枚举到」的块**多数不是丢了，而是按钮在动画里平移**：明细里能看到
    ///     `336..376 → 336..377`、`365..405` 这类**中间位置**（后者根本不是任何
    ///     静止按钮的位置）。
    ///   · 旧的 6s 全局迟滞就是为这些兜底 ⇒ 副作用是**关掉应用后 6s 才回位**。
    ///
    /// ⇒ 改成**按证据判活**：`recheck` 单独问那个元素一次。
    ///   · 答「还在」⇒ 保留，并**采用它现在挪到哪儿了**（顺带治好中间位置堆积）；
    ///   · 答「没了 / 空矩形 / 报错」⇒ **本轮就弃**（实测回位 ~0.4s）。
    #[test]
    fn merge_observed_drops_only_what_recheck_confirms_gone() {
        let t0 = Instant::now();
        let grace = Duration::from_millis(MERGE_FALLBACK_GRACE_MS);
        // prev 有两块；本次只采到其中一块
        let prev = vec![
            Entry {
                rect: r(0, 69),
                element: None,
                seen: t0,
            },
            Entry {
                rect: r(789, 844),
                element: None,
                seen: t0,
            },
        ];
        let fresh = vec![(r(0, 69), None)];

        // ① `recheck` 说「都没了」⇒ 只剩本次采到的那块
        let out = merge_observed(&prev, &fresh, t0 + Duration::from_secs(1), |_| None);
        assert_eq!(out.len(), 1, "问出「真没了」就必须本轮弃掉，不许等宽限期");
        assert_eq!(out[0].rect, r(0, 69));

        // ② `recheck` 说「还在、而且挪到 800..855 了」⇒ 保留并**采用新位置**
        let out = merge_observed(&prev, &fresh, t0 + Duration::from_secs(1), |e| {
            (e.rect == r(789, 844)).then(|| r(800, 855))
        });
        assert_eq!(out.len(), 2, "问出「还在」就必须保留");
        assert!(
            out.iter().any(|e| e.rect == r(800, 855)),
            "必须采用它**现在**的位置 —— 否则动画中间位置会越积越多导致过度避让"
        );

        // ③ 无 UIA 元素可问时（经典子窗那三块）才按时间兜，且**只兜一个周期**
        let out = merge_observed(&prev, &fresh, t0, |e| {
            (t0.saturating_duration_since(e.seen) <= grace).then_some(e.rect)
        });
        assert_eq!(
            out.len(),
            2,
            "宽限期内仍保留（无元素可问时的唯一按时间判定）"
        );
        let later = t0 + grace + Duration::from_millis(1);
        let out = merge_observed(&prev, &fresh, later, |e| {
            (later.saturating_duration_since(e.seen) <= grace).then_some(e.rect)
        });
        assert_eq!(
            out.len(),
            1,
            "超过兜底周期才弃，且**仅限无元素可问的那几块**"
        );
    }

    /// ⛔ 新出现的块必须**立即**生效（不许因为在核对旧块而延后一轮）。
    #[test]
    fn merge_observed_takes_new_blocks_immediately() {
        let t0 = Instant::now();
        let prev = vec![Entry {
            rect: r(0, 69),
            element: None,
            seen: t0,
        }];
        let fresh = vec![(r(0, 69), None), (r(789, 844), None)];
        let out = merge_observed(&prev, &fresh, t0, |_| None);
        assert_eq!(out.len(), 2, "新图标必须本轮就进去 —— 避让延迟全靠这一条");
        assert!(out.iter().any(|e| e.rect == r(789, 844)));
    }

    /// ⭐ 判「同一块」按**容差**配对：1px 抖动必须刷新原条目、不得新增。
    /// （否则每次抖动都在占用集里堆一份新块 ⇒ 空隙越来越窄 ⇒ 过度避让。）
    #[test]
    fn merge_observed_refreshes_matched_rect_instead_of_duplicating() {
        let t0 = Instant::now();
        let prev = vec![Entry {
            rect: r(404, 459),
            element: None,
            seen: t0,
        }];
        // 1px 抖动（UIA 在 DPI 换算边界上会差 1px）
        let out = merge_observed(&prev, &[(r(405, 460), None)], t0, |_| None);
        assert_eq!(out.len(), 1, "容差内的位移应配到同一条，不得新增");
        assert_eq!(out[0].rect, r(405, 460), "并采用本次的位置");
    }

    /// ⛔ 容差**必须远小于一个按钮宽**，否则相邻两块会被认成同一块、其中一块
    /// 永久丢失。依据：本机实测应用按钮宽 **55px**（`404..459`、`459..514`…）。
    #[test]
    fn merge_observed_tolerance_never_merges_adjacent_buttons() {
        let t0 = Instant::now();
        let prev = vec![Entry {
            rect: r(404, 459),
            element: None,
            seen: t0,
        }];
        let out = merge_observed(
            &prev,
            &[(r(404, 459), None), (r(459, 514), None)],
            t0,
            |_| None,
        );
        assert_eq!(out.len(), 2, "相邻两块（实测宽 55px）不得被认成同一块");
    }
}
