//! 全局状态与锁。
//!
//! # 锁序登记（P3-10）
//!
//! 本模块是**全局锁的登记处**。新增任何 `static … Mutex` / `RwLock` 之前，先在这里
//! 登记它的层级以及它与既有锁的获取关系。
//!
//! 为什么必须集中登记：**锁序缺陷（AB/BA 死锁）不产生编译错误，也不产生 panic 栈**。
//! 它的表现是整个进程静默僵死（窗口点不动、日志停在同一行），事后只能靠人把两条
//! 路径的加锁顺序摆在一起看。原先的锁序说明散落在 4 个文件的行内注释里
//! （`battery_notify.rs` / `config.rs` / `device_data.rs` / `tray.rs`），挡不住新调用点
//! ——新代码只要在闭包里顺手取一次另一把锁，死锁条件就齐备了，而没有任何一处会报错。
//!
//! 因此下面给出**白名单**：**不在白名单里的「持 A 取 B」一律按缺陷处理**。
//!
//! ## 一、允许的嵌套边（白名单，当前仅此三条）
//!
//! | # | 边（持 → 取） | 现存实例 | 为什么允许 |
//! |---|---|---|---|
//! | 1 | `DEVICES_CACHE` → `CONFIG` | `tray.rs::build_tooltip_text` | 两条临界区内都只剩纯内存操作（`format!` / 字段读）。落盘已被 P1-3 移出配置锁、B11 移出调用线程 |
//! | 2 | `PERSIST_LOCK` → `LAST_CONFIG_CONTENT` | `config.rs::persist_now` | 只在 `peritray-config` 写线程（或队列满时的同步兜底）上执行，不与 UI 线程争抢 |
//! | 3 | `BT_LOCK` → `BLE_CONN` | `bluetooth.rs::bt_action` → `bt_ble::ble_connect` / `ble_disconnect` | `BT_LOCK` 是蓝牙操作的串行锁，`BLE_CONN` 是连接表。⚠️ `BLE_CONN` 内目前有持锁 WinRT 调用，见「四」 |
//!
//! ## 二、禁止的反向边（一旦出现即构成 AB/BA 死锁条件）
//!
//! | 边 | 当前状态 | 为什么必须守住 |
//! |---|---|---|
//! | `CONFIG` → `DEVICES_CACHE` | **不存在** | 与白名单第 1 条反向。全仓 66 处 `with_config` / `with_config_mut` 调用点（2026-09-18 复核）目前都是纯字段读写；只要有一处在闭包里取设备缓存锁，死锁条件立刻齐备 |
//! | `LAST_CONFIG_CONTENT` → `PERSIST_LOCK` | **不存在** | 与白名单第 2 条反向 |
//! | `BLE_CONN` → `BT_LOCK` | **不存在** | 与白名单第 3 条反向 |
//!
//! ## 三、锁清单
//!
//! ### 顶层锁
//!
//! 可以被白名单中的下层锁依赖；自身**不得**在持有其他锁时获取。
//!
//! | 符号 | 定义处 | 说明 |
//! |---|---|---|
//! | `CONFIG` | `config.rs` | 配置。**临界区内只允许纯内存操作**（P1-3 / B11 已把落盘全部外移） |
//! | `DEVICES_CACHE` | 本文件 | 设备列表缓存（托盘 tooltip 与弹窗卡片的数据源） |
//! | `TRAY_ICON` | `tray.rs` | 托盘句柄。锁内只取句柄/换菜单，**调用 API 前必须先释放**（`set_icon` / `set_tooltip` / `set_menu` 会同步等主线程） |
//! | `AUTO_MENU_ITEM` | 本文件 | 自启菜单项，同 `TRAY_ICON`（`set_text` 同步等主线程） |
//! | `DEVICE_REGISTERED_KEYS` | `shortcut.rs` | 已注册快捷键集合。锁内只算差集，插件 API 调用在锁外 |
//! | `PERSIST_LOCK` | `config.rs` | 落盘串行锁，**故意跨 I/O 持有**（防两次落盘交错写坏文件）；只在写线程上执行 |
//! | `BT_LOCK` | `bluetooth.rs` | 蓝牙连接/断开的串行锁 |
//!
//! ### 叶子锁
//!
//! **不得在其中再获取任何锁。**
//!
//! | 符号 | 定义处 | 说明 |
//! |---|---|---|
//! | `LAST_CONFIG_CONTENT` | `config.rs` | 脏检查基准（上次成功写盘的内容） |
//! | `CONFIG_LOAD_ERROR` | `config.rs` | 启动期解析错误信息 |
//! | `TRAY_POS` / `POPUP_POS` / `TRAY_MONITOR` | 本文件 | 坐标与托盘所在显示器信息 |
//! | `LAST_MTIME` | `device_data.rs` | 用户数据文件 mtime（守卫必须收在块内，见该处注释） |
//! | `DEVICE_DATA` | `device_data.rs` | `RwLock`，设备自定义数据 |
//! | `BT_BATTERY` | `bluetooth.rs` | 蓝牙电量缓存 |
//! | `BLE_CONN` | `bt_ble.rs` | BLE 连接表 |
//! | `REGISTER_FAILED` | `shortcut.rs` | 注册失败的快捷键（供启动提示） |
//! | `ICON_CACHE` / `NAME_CACHE` | `app_icon.rs` | 图标 / 进程名 LRU（读命中与回填分两次加锁，避免持锁做取图） |
//! | `LAST_PROP_LOG` | `audio_notify.rs` | 属性日志去重时间戳 |
//! | `PACKAGE_CACHE` | `audio_spatial.rs` | 包族查询 TTL 缓存 |
//! | `NOTIFIED` | `battery_notify.rs` | 已提醒的低电量条目 |
//! | `PREV_TOAST` | `toast.rs` | 上一条通知句柄（`take()` 必须落到独立语句，见该处注释） |
//! | `LAST_STATUS` | `update.rs` | 更新状态 |
//! | `CACHED_REGEX` | `wmi_query.rs` | 过滤正则缓存 |
//! | `force_mute_prev_volume()` | `audio.rs` | 强制静音前的音量 |
//! | `ANIM_TEST_LOCK` | 本文件（`#[cfg(test)]`） | 串行化动画相关用例 |
//!
//! ## 四、持锁做 I/O：历史与现状
//!
//! 登记锁序时顺带扫出三处「持锁做 I/O」（同属 `AGENTS.md`「持锁区不得调用…」）。
//! 前两处已在紧随 P3-10 的提交中修复，留档于此以免回归：
//!
//! - ✅ `bt_ble.rs` 的 `ble_connect` / `ble_disconnect`：原先在持 `BLE_CONN` 时
//!   ① 调 WinRT（`session.Close()` / `device.Close()`）；② **写日志文件**
//!   （`verbose_log!` / `append_verbose_log` / `append_log`，`sync_all` 在杀软实时
//!   扫描下可达数十毫秒）。现改为锁内只「摘表/换表」，锁外再 Close 与记日志
//!   ——与 P2-7 的低电量通知同款：锁内取数据，锁外做 I/O。
//! - ✅ `audio.rs` 的 `toggle_device_mute`：原先在持 `force_mute_prev_volume()` 时
//!   调用 `SetMasterVolumeLevelScalar`（COM）。这与 `toast.rs` 已修过的是**完全同类**
//!   的 edition 2021 临时量陷阱（`if let Some(x) = lock().take()` 会让守卫活到整个
//!   `if` 块）；现已把 `remove` 落到独立语句。
//!
//! 仍待处理：
//!
//! - `bt_ble.rs` 用 `.lock().map_err(..)` 而非统一的 `state::lock_unpoisoned`，
//!   与 P2-11 确立的统一入口约定不一致（中毒时会让蓝牙操作报错而非继续）。
//!
//! ## 五、自查方法
//!
//! 改动涉及锁时，在仓库根执行：
//!
//! ```bash
//! # ① 列出全部加锁点，逐个人工确认没有白名单之外的新边
//! grep -rn "lock_unpoisoned(\|read_unpoisoned(\|write_unpoisoned(\|\.lock()" src-tauri/src/
//!
//! # ② 确认配置锁的闭包里没有取其他锁（有输出即缺陷）
//! grep -rn -A6 "with_config_mut(" src-tauri/src/ | grep -E "lock_unpoisoned|read_unpoisoned|\.lock\(\)"
//! ```
//!
//! ⚠️ 这两条命令只做**提示**：闭包可以跨很多行，也可能经由被调用函数间接取锁
//! （白名单第 1、3 条正是这种形态）。真正的把关仍是人读代码。

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard, OnceLock, RwLock, RwLockReadGuard, RwLockWriteGuard};
use tauri::menu::MenuItem;

use crate::device::Device;

/// 托盘图标位置
pub static TRAY_POS: OnceLock<Mutex<(f64, f64)>> = OnceLock::new();

/// 弹窗窗口位置
pub static POPUP_POS: OnceLock<Mutex<(f64, f64)>> = OnceLock::new();

/// 托盘图标所在显示器的信息（缩放因子 + 逻辑工作区），
/// 用于混合 DPI 下弹窗定位与高度 clamp。
#[derive(Debug, Clone, Copy)]
pub struct TrayMonitorInfo {
    pub scale_factor: f64,
    pub work_x: f64,
    pub work_y: f64,
    pub work_w: f64,
    pub work_h: f64,
}

/// 托盘所在显示器信息缓存（托盘点击时刷新，None 表示尚未确定）
pub static TRAY_MONITOR: OnceLock<Mutex<Option<TrayMonitorInfo>>> = OnceLock::new();

/// 获取托盘所在显示器信息缓存的引用
pub fn get_tray_monitor() -> &'static Mutex<Option<TrayMonitorInfo>> {
    TRAY_MONITOR.get_or_init(|| Mutex::new(None))
}

/// 弹窗动画状态。
///
/// **不要直接 `store` 它**（P1-8）：置位/复位一律经 [`try_begin_animation`] 取守卫，
/// 由 `SingleFlightGuard` 的 Drop 负责复位。手工 `store(false)` 一旦被漏写
/// （或动画线程 panic 未展开到复位点），弹窗会**永久打不开也关不掉**。
pub static ANIMATING: AtomicBool = AtomicBool::new(false);

/// 本次动画的起始时刻（[`monotonic_ms`] 刻度）。
///
/// 存在的唯一理由：`Cargo.toml` 的 `[profile.release] panic = "abort"` 让
/// **Drop 复位在 release 下根本不会执行**（abort 直接终止进程，不展开栈），
/// 因此不能只靠 RAII。这里存下起始时刻，配合 [`ANIMATION_TIMEOUT_MS`]
/// 把「永久卡死」降级为「最多卡 2 秒」。
static ANIMATION_STARTED: AtomicU64 = AtomicU64::new(0);

/// 动画超时上限（毫秒）。动画本体最长 250ms（`animate_open`），
/// 取 2 秒给足调度余量；超过即认定动画线程已死。
pub(crate) const ANIMATION_TIMEOUT_MS: u64 = 2000;

/// 单调毫秒时钟（自本进程首次调用起算）。
///
/// **不用 `SystemTime`**：系统时间会被 NTP 校正、用户改表、休眠唤醒调整，
/// 跳变会让「已过多少毫秒」算出负数或突增，超时判定随之失效
/// （看门狗的时间跳变误报就是同一类坑）。
pub fn monotonic_ms() -> u64 {
    static START: OnceLock<std::time::Instant> = OnceLock::new();
    START
        .get_or_init(std::time::Instant::now)
        .elapsed()
        .as_millis() as u64
}

/// 纯判据：`flag` 置位且未超时 ⇒ 动画仍在阻塞新操作。
/// 抽成纯函数以便对边界（恰好 `ANIMATION_TIMEOUT_MS`）做单测。
fn animation_blocks_at(flag: bool, elapsed_ms: u64) -> bool {
    flag && elapsed_ms < ANIMATION_TIMEOUT_MS
}

/// 动画是否正在阻塞新的开关操作（含超时自愈判定）。
///
/// 注意它**不**清除标志：超时后返回 `false` 只是让调用方继续往下走，
/// 真正的夺回发生在 [`try_begin_animation`] 里。
pub fn animation_blocks() -> bool {
    let flag = ANIMATING.load(Ordering::SeqCst);
    if !flag {
        return false;
    }
    let elapsed = monotonic_ms().saturating_sub(ANIMATION_STARTED.load(Ordering::SeqCst));
    animation_blocks_at(flag, elapsed)
}

/// 尝试开始一次弹窗动画：成功返回守卫（Drop 时复位 `ANIMATING`），
/// 失败返回 `None`（已有动画在跑且未超时）。
///
/// **超时自愈**：标志为 true 但已超过 [`ANIMATION_TIMEOUT_MS`] 时，认定上一轮
/// 动画线程已异常终止（release 下 `panic = "abort"` 使 Drop 复位不生效），
/// 强行夺回标志并放行——这是「永久卡死」与「最多卡 2 秒」的分界。
///
/// **已知残留**（超时夺回时才可能出现）：若那轮「疑似已死」的动画线程其实还活着，
/// 它随后 Drop 守卫时会把 `ANIMATING` 复位一次，可能误清掉新一轮的持有状态。
/// 后果是允许两个动画短暂重叠（画面抖动），而非卡死——方向上仍是净改善。
/// 彻底消除需要带代际号的守卫类型，而本项目的守卫是共用的 `SingleFlightGuard`。
pub(crate) fn try_begin_animation() -> Option<SingleFlightGuard<'static>> {
    try_begin_animation_with_timeout(ANIMATION_TIMEOUT_MS)
}

/// [`try_begin_animation`] 的实际实现。`timeout_ms` 作为参数是为了让「超时夺回」
/// 这条分支能被**确定性地**单测（不必真的等 2 秒）。
fn try_begin_animation_with_timeout(timeout_ms: u64) -> Option<SingleFlightGuard<'static>> {
    if let Some(guard) = SingleFlightGuard::new(&ANIMATING) {
        ANIMATION_STARTED.store(monotonic_ms(), Ordering::SeqCst);
        return Some(guard);
    }
    // 已被占用：仅当判定超时才夺回
    let elapsed = monotonic_ms().saturating_sub(ANIMATION_STARTED.load(Ordering::SeqCst));
    if !(elapsed < timeout_ms) {
        // 先复位再重新抢占：直接 CAS 抢占会失败（标志仍为 true）
        let _ = ANIMATING.compare_exchange(true, false, Ordering::SeqCst, Ordering::SeqCst);
        if let Some(guard) = SingleFlightGuard::new(&ANIMATING) {
            ANIMATION_STARTED.store(monotonic_ms(), Ordering::SeqCst);
            crate::standard_log!(
                "[popup] ANIMATING 超时自愈：动画线程疑似已死（已过 {}ms，上限 {}ms），强制放行",
                elapsed,
                timeout_ms
            );
            return Some(guard);
        }
    }
    None
}

/// 开机自启状态
pub static AUTO_START: AtomicBool = AtomicBool::new(false);

/// 快捷键录制期间置位，抑制全局快捷键触发，避免录制时误触发动作
pub static SHORTCUT_RECORDING: AtomicBool = AtomicBool::new(false);

/// 开机自启菜单项引用
pub static AUTO_MENU_ITEM: OnceLock<Mutex<Option<MenuItem<tauri::Wry>>>> = OnceLock::new();

/// **全仓唯一的 Mutex 加锁入口**（P2-11）。
///
/// 语义：**容忍中毒**——锁中毒（持锁线程 panic）时直接接管内部数据继续使用。
/// 依据：本项目各静态量在 panic 后仅需「可用」而非「严格一致」，
/// 统一在此表达该语义，避免每个调用点各自发明一套。
///
/// **禁止的写法及其后果**（评审时逐条对照；括号内为 2026-09-17 统一前的历史写法）：
/// - `Mutex::lock()` 之后直接 `unwrap()`（`audio_notify.rs`）——中毒即 **panic**。
///   最危险的是 COM 回调路径：panic 会穿过 FFI/COM 边界向外抛，属未定义行为，
///   且会把「一次 panic」放大成「此后每次回调都炸」。
/// - `Mutex::lock()` 之后接 `ok()` / `.ok().and_then(..)`（`update.rs`）——中毒即
///   **静默返回 `None`**，表现为「设置页永远显示不出更新状态」这类无日志的哑故障。
/// - `if let Ok(guard) = mutex.lock() { .. }` / `match mutex.lock() { .. Err(_) => .. }`
///   （`config.rs` / `tray.rs` / `device_data.rs` / `audio_spatial.rs`）——
///   中毒即**静默跳过整个临界区**。写入侧跳过尤其致命：缓存/句柄会永久停在旧值
///   （如 `TRAY_ICON` 永不被赋值 ⇒ tooltip 此后再也刷不动）。
///
/// **唯一例外**：需要把中毒**上报给调用方**的命令边界可用
/// `.lock().map_err(|e| e.to_string())?`（见 `bt_ble.rs`）——那里返回 `Result`，
/// 中毒属于可报告的错误，而非应当吞掉的内部状态。
///
/// > 本注释刻意不把被禁写法写成**连续字面量**（上一条已按该规则改写），
/// > 以便「全仓 grep 该字面量 = 0」这条验收可机械执行、无需先剥离注释。
///
/// RwLock 同族见 [`read_unpoisoned`] / [`write_unpoisoned`]。
pub fn lock_unpoisoned<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// 容忍中毒的 RwLock 读锁（语义与 [`lock_unpoisoned`] 一致）。
pub fn read_unpoisoned<T>(m: &RwLock<T>) -> RwLockReadGuard<'_, T> {
    m.read().unwrap_or_else(|e| e.into_inner())
}

/// 容忍中毒的 RwLock 写锁（语义与 [`lock_unpoisoned`] 一致）。
pub fn write_unpoisoned<T>(m: &RwLock<T>) -> RwLockWriteGuard<'_, T> {
    m.write().unwrap_or_else(|e| e.into_inner())
}

/// 单飞标志的 RAII 守卫：CAS 抢占，Drop 时复位。
/// 正常返回与 panic 展开均会复位，避免后台任务被永久锁死。
///
/// 用法要点（见 AGENTS.md「RAII 守卫的绑定命名」）：
/// - **外层绑定不得以 `_` 开头**——若守卫需要在线程闭包内被持有，必须以具名绑定
///   `let Some(guard) = ...` 取得，再在闭包体内显式引用（`let _guard = guard;`）触发捕获。
///   写成 `let Some(_guard)` 且闭包内不引用它时，`move` 闭包**不会捕获它**，
///   守卫会在函数返回时立即 Drop，单飞语义退化为无保护。
/// - 同步路径（在本函数内跑完工作再返回）用 `let Some(_guard)` 是正确的：Drop-only 绑定。
pub(crate) struct SingleFlightGuard<'a> {
    flag: &'a AtomicBool,
}

impl<'a> SingleFlightGuard<'a> {
    /// CAS 获取标志；成功返回 guard，失败返回 None（已有任务在跑）
    pub(crate) fn new(flag: &'a AtomicBool) -> Option<Self> {
        if flag
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
        {
            Some(Self { flag })
        } else {
            None
        }
    }
}

impl Drop for SingleFlightGuard<'_> {
    fn drop(&mut self) {
        self.flag.store(false, Ordering::SeqCst);
    }
}

/// 单飞 + 合并执行器：同一时刻只跑一轮 `job`，且**尽量不丢更新**。
///
/// 语义（B3 引入，用于托盘菜单重建）：
/// - 抢到单飞权者执行 `job`。每轮开始前先清掉积压的 `pending`（「认领」），
///   于是**启动前**积压的多个请求被合并成一轮；
/// - 执行**期间**到达的请求把 `pending` 置起并**立即返回**（不阻塞事件分发），
///   由持权者在收尾时补跑一轮，因此这类请求不会丢；
/// - 返回实际执行的轮数；`None` = 没抢到单飞权（已代为置 `pending`）。
///
/// 为什么需要它：改造前每个事件都 `spawn` 一个线程独立重建菜单，两个事件撞车时
/// 「后写覆盖」——较慢的那次会用**稍旧的一份**菜单盖掉较新的一份。且
/// `config-changed` 一次会连做图标 + 菜单 + 设备缓存 + tooltip，与
/// `audio-devices-changed` 撞车时整棵菜单（含 COM 枚举）会被重建两次。
///
/// **残留窗口（已知且接受）**：请求的 `pending` 写入若恰好落在持权者
/// 「最后一次 `swap` 返回 false」与「守卫 Drop」之间的纳秒级窗口内，该请求
/// 既不会被补跑、也不会有人再检查。后果与改造前一致（菜单滞后一拍，
/// 下次事件即纠正），但窗口已从「整轮重建耗时（含 COM 枚举，数十 ms）」
/// 缩到「两次原子操作之间」。
pub(crate) fn run_coalesced(
    running: &AtomicBool,
    pending: &AtomicBool,
    mut job: impl FnMut(),
) -> Option<usize> {
    let Some(_guard) = SingleFlightGuard::new(running) else {
        pending.store(true, Ordering::SeqCst);
        return None;
    };
    // 认领：清掉启动前积压的请求——本轮 `job` 读到的就是最新状态，
    // 故这些请求已被本轮满足，无需再补跑。
    pending.store(false, Ordering::SeqCst);
    let mut rounds = 0usize;
    loop {
        job();
        rounds += 1;
        // 收尾检查：本轮执行期间到达的请求 → 再跑一轮
        if !pending.swap(false, Ordering::SeqCst) {
            return Some(rounds);
        }
    }
}

/// 设备缓存，用于托盘 tooltip 显示，避免重复 WMI 查询。
///
/// 锁序：本锁是**顶层锁**，白名单允许它 → `CONFIG`（唯一实例在
/// `tray.rs::build_tooltip_text`）。完整登记见本文件头的模块文档。
static DEVICES_CACHE: OnceLock<Mutex<Vec<Device>>> = OnceLock::new();

/// 获取设备缓存的引用
pub fn get_devices_cache() -> &'static Mutex<Vec<Device>> {
    DEVICES_CACHE.get_or_init(|| Mutex::new(Vec::new()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    /// 动画相关用例会操作全局 `ANIMATING` / `ANIMATION_STARTED`，
    /// 而测试默认并行 ⇒ 用一把测试专用锁串行化，避免相互干扰。
    static ANIM_TEST_LOCK: Mutex<()> = Mutex::new(());

    // ── P1-8：弹窗动画守卫 ────────────────────────────────────────

    /// 超时判据的边界：**恰好等于上限**即视为超时（`<` 而非 `<=`）。
    /// 若把比较写反（`flag && elapsed > TIMEOUT`），正常动画在 2 秒内仍会被
    /// 判定为「未超时」而拒绝夺回——即超时自愈彻底失效，本用例会红。
    #[test]
    fn animation_timeout_boundary() {
        assert!(!animation_blocks_at(false, 0), "标志未置位时不应阻塞");
        assert!(
            !animation_blocks_at(false, u64::MAX),
            "标志未置位时与耗时无关"
        );
        assert!(animation_blocks_at(true, 0), "刚置位应阻塞");
        assert!(
            animation_blocks_at(true, ANIMATION_TIMEOUT_MS - 1),
            "未到上限应阻塞"
        );
        assert!(
            !animation_blocks_at(true, ANIMATION_TIMEOUT_MS),
            "到达上限即视为超时"
        );
        assert!(
            !animation_blocks_at(true, u64::MAX),
            "远超上限（时钟异常/守卫丢失）必须放行，否则永久卡死"
        );
    }

    /// 守卫的核心契约：互斥 + Drop 复位。
    /// 「Drop 复位」是本条修复的立足点——原先靠两处手工 `store(false)`，
    /// 漏一处弹窗就永久打不开也关不掉。
    #[test]
    fn animation_guard_is_exclusive_and_released_on_drop() {
        let _serial = lock_unpoisoned(&ANIM_TEST_LOCK);
        // 前置清理：避免其它用例残留状态影响本用例
        ANIMATING.store(false, Ordering::SeqCst);

        let g1 = try_begin_animation().expect("首次获取应成功");
        assert!(animation_blocks(), "持有期间应阻塞新操作");
        assert!(try_begin_animation().is_none(), "重复获取必须失败（互斥）");

        drop(g1);
        assert!(!animation_blocks(), "守卫 Drop 后不应再阻塞");
        let g2 = try_begin_animation().expect("释放后应能再次开始动画");
        drop(g2);
        assert!(!animation_blocks());
    }

    /// 动画线程 panic（debug 下 unwind）时守卫必须随栈展开释放。
    /// 这是「一次动画异常不能永久废掉弹窗」的直接断言。
    #[test]
    fn animation_guard_released_when_animation_panics() {
        let _serial = lock_unpoisoned(&ANIM_TEST_LOCK);
        ANIMATING.store(false, Ordering::SeqCst);

        let guard = try_begin_animation().expect("首次获取应成功");
        let h = std::thread::spawn(move || {
            let _guard = guard;
            panic!("模拟动画线程 panic");
        });
        assert!(h.join().is_err(), "线程应因 panic 结束");
        assert!(!animation_blocks(), "panic 展开后守卫应已释放");
        assert!(
            try_begin_animation().is_some(),
            "panic 之后必须还能重新开始动画"
        );
    }

    /// 超时自愈：模拟 release 下 `panic = "abort"` 的后果
    /// （abort 不展开栈 ⇒ 守卫 Drop 不会执行 ⇒ 标志永久停在 true）。
    /// 这条断言的是「永久卡死」已降级为「最多卡 2 秒」。
    #[test]
    fn animation_flag_is_reclaimed_after_timeout() {
        let _serial = lock_unpoisoned(&ANIM_TEST_LOCK);
        // 伪造「上一位持权者已死」的现场：标志置位 + 起始时刻记为当下
        ANIMATING.store(true, Ordering::SeqCst);
        ANIMATION_STARTED.store(monotonic_ms(), Ordering::SeqCst);

        // ① 未超时：不得夺回，否则会打断正在进行的正常动画
        assert!(animation_blocks(), "刚置位应处于阻塞状态");
        assert!(
            try_begin_animation_with_timeout(ANIMATION_TIMEOUT_MS).is_none(),
            "未超时时不得夺回"
        );

        // ② 超时（timeout=0 等价于「已过期」）：必须夺回
        let guard = try_begin_animation_with_timeout(0).expect("超时后必须夺回标志");
        drop(guard);
        assert!(!animation_blocks(), "夺回并释放后应恢复正常");
    }

    // ── B3：单飞 + 合并执行器 ────────────────────────────────────

    /// 无竞争：只跑一轮，且结束后单飞权必须已释放（否则后续所有重建都会被永久吞掉）
    #[test]
    fn coalesced_runs_once_when_uncontended() {
        let running = AtomicBool::new(false);
        let pending = AtomicBool::new(false);
        let calls = AtomicUsize::new(0);
        let rounds = run_coalesced(&running, &pending, || {
            calls.fetch_add(1, Ordering::SeqCst);
        });
        assert_eq!(rounds, Some(1));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert!(!running.load(Ordering::SeqCst), "守卫必须已释放单飞权");
        assert!(
            !pending.load(Ordering::SeqCst),
            "正常收尾后不应残留 PENDING"
        );
    }

    /// 已被占用：立即返回 `None`、不执行 job、**但必须置 PENDING**——
    /// 漏了这一步就是「丢更新」，是本条修复最容易写错的方向。
    #[test]
    fn coalesced_defers_when_busy() {
        let running = AtomicBool::new(true); // 模拟另一轮正在进行
        let pending = AtomicBool::new(false);
        let calls = AtomicUsize::new(0);
        let rounds = run_coalesced(&running, &pending, || {
            calls.fetch_add(1, Ordering::SeqCst);
        });
        assert_eq!(rounds, None, "抢不到单飞权应返回 None");
        assert_eq!(calls.load(Ordering::SeqCst), 0, "抢不到时不得执行 job");
        assert!(
            pending.load(Ordering::SeqCst),
            "抢不到时必须置 PENDING，否则本次请求丢失"
        );
    }

    /// 启动前积压的请求被「认领」合并进本轮：只跑一轮而不是两轮。
    /// 这是合并（coalescing）相对「排队」的价值所在——避免无谓的重复重建。
    #[test]
    fn coalesced_absorbs_backlog_before_start() {
        let running = AtomicBool::new(false);
        let pending = AtomicBool::new(true); // 上一轮遗留 / 抢权失败者置的
        let calls = AtomicUsize::new(0);
        let rounds = run_coalesced(&running, &pending, || {
            calls.fetch_add(1, Ordering::SeqCst);
        });
        assert_eq!(rounds, Some(1), "积压请求应被本轮合并，无需补跑");
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert!(!pending.load(Ordering::SeqCst));
    }

    /// 执行期间到达的请求必须触发**补跑一轮**（这是「不丢更新」的核心断言）
    #[test]
    fn coalesced_reruns_when_request_arrives_during_job() {
        let running = AtomicBool::new(false);
        let pending = AtomicBool::new(false);
        let calls = AtomicUsize::new(0);
        let rounds = run_coalesced(&running, &pending, || {
            // 模拟「job 执行期间另一个线程抢权失败、置了 PENDING」
            if calls.fetch_add(1, Ordering::SeqCst) == 0 {
                pending.store(true, Ordering::SeqCst);
            }
        });
        assert_eq!(rounds, Some(2), "执行期间到达的请求应触发补跑");
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    /// 并发场景：主线程先占住单飞权，8 个线程同时请求 —— 全部应立即返回 `None`
    /// 并只把 PENDING 置起（不各自开一轮），释放后由持权者合并成一轮。
    /// 这条钉的是「多个事件撞车时不再重建多次」这一 B3 的核心收益。
    #[test]
    fn concurrent_requests_do_not_each_rebuild() {
        let running = AtomicBool::new(true); // 主线程占位，等价于「一轮正在进行」
        let pending = AtomicBool::new(false);
        let calls = AtomicUsize::new(0);
        std::thread::scope(|s| {
            for _ in 0..8 {
                s.spawn(|| {
                    let r = run_coalesced(&running, &pending, || {
                        calls.fetch_add(1, Ordering::SeqCst);
                    });
                    assert_eq!(r, None, "单飞被占用时调用方应立即返回");
                });
            }
        });
        assert_eq!(calls.load(Ordering::SeqCst), 0, "并发请求期间不得执行 job");
        assert!(pending.load(Ordering::SeqCst));

        // 释放单飞权：由下一轮把 8 个请求合并成 1 轮
        running.store(false, Ordering::SeqCst);
        let rounds = run_coalesced(&running, &pending, || {
            calls.fetch_add(1, Ordering::SeqCst);
        });
        assert_eq!(rounds, Some(1), "8 个请求应合并为 1 轮重建");
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    // ── P2-2 / P2-11：中毒容忍的加锁入口 ──────────────────────────

    /// 让 `m` 处于中毒态：在持锁期间 panic，再用 `catch_unwind` 收住。
    ///
    /// 注：这会向 stderr 打印一行默认 hook 的 `thread panicked at ...`，
    /// 属预期噪声——不用临时 hook 覆盖它，因为那是进程级全局状态。
    fn poison_mutex<T>(m: &Mutex<T>) {
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = lock_unpoisoned(m);
            panic!("注入：持锁 panic，令 Mutex 中毒");
        }));
        assert!(m.lock().is_err(), "前提：注入后锁必须已中毒");
    }

    /// 可证伪性：若 `lock_unpoisoned` 退回 `Mutex::lock()` + `unwrap()`，本用例转红（panic）。
    #[test]
    fn lock_unpoisoned_takes_over_poisoned_mutex() {
        let m = Mutex::new(0u32);
        poison_mutex(&m);
        *lock_unpoisoned(&m) = 42;
        assert_eq!(*lock_unpoisoned(&m), 42, "中毒后仍应能读写内部数据");
    }

    /// 同 `poison_mutex`，作用于 RwLock 的写锁。
    fn poison_rwlock<T>(m: &RwLock<T>) {
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = write_unpoisoned(m);
            panic!("注入：持写锁 panic，令 RwLock 中毒");
        }));
        assert!(m.write().is_err(), "前提：注入后写锁必须已中毒");
    }

    #[test]
    fn read_unpoisoned_takes_over_poisoned_rwlock() {
        let m = RwLock::new(7u32);
        poison_rwlock(&m);
        assert_eq!(*read_unpoisoned(&m), 7, "中毒后读锁仍应可用");
    }

    #[test]
    fn write_unpoisoned_takes_over_poisoned_rwlock() {
        let m = RwLock::new(7u32);
        poison_rwlock(&m);
        *write_unpoisoned(&m) = 9;
        assert_eq!(*read_unpoisoned(&m), 9, "中毒后写锁仍应可用");
    }
}
