//! 任务栏「音乐控制组件」的数据层（SMTC / `Windows::Media::Control`）。
//!
//! ## 职责边界
//!
//! 本模块**只管取数与控制**，不碰绘制；绘制在 `taskbar_widget::draw_music` 那边，
//! 本模块通过 [`snapshot`] 把「一个音乐面板的静态快照」交出去。
//!
//! ## 为什么全部放后台线程
//!
//! ⛔ SMTC 的取数**全是阻塞等待**：`RequestAsync()` / `TryGetMediaPropertiesAsync()`
//!   返回的 `IAsyncOperation` 必须 `.join()`（内部 = `Waiter` +
//!   `WaitForSingleObject(INFINITE)` + `get_results`）。
//!   主线程同时持有 Tauri 事件循环与任务栏 widget 窗口 ⇒ 在其上 `join()`
//!   就是**界面冻结**，且违反 AGENTS.md「主线程不得阻塞等自己」。
//! ⇒ 单后台线程 + **MTA**（`CoInitializeEx(COINIT_MULTITHREADED)`）：
//!   WinRT 异步在线程池回调，MTA 线程不需要自己的消息泵就能等到完成。
//!
//! ⛔ **回调只单向投递**。SMTC 事件（`Revents`）在**任意线程**触发，一律
//!   `post_message` 给 widget 窗口线程，**绝不**反过去等它处理完。
//!   （用 `Dispatcher.Invoke` 同步等 UI 线程，那是本仓明令禁用的反模式。）
//!
//! ## 会话增删的订阅维护
//!
//! `SessionsChanged` 到来时 diff `GetSessions()`：
//! · 新增会话 → 给它挂 4 个事件回调（媒体属性 / 播放信息 / 播放状态 / 时间线）
//! · 消失会话 → 退订并 **drop 闭包**
//! ⛔ 闭包持有 `Session` 强引用 ⇒ 不 drop 就等于**会话永不释放**（泄漏）。
//!   这一层第三方库会替调用方做，直连 WinRT 时**得自己做**。
//!
//! ## SMTC API 的几个坑（全部由探针实测确认，勿凭直觉改）
//!
//! 1. 方法名是 **PascalCase**（`RequestAsync` / `GetSessions` / `TryPlayAsync`），
//!    不是 snake_case。
//! 2. `IAsyncOperation::join()` 是**固有方法**（`windows-future` 的 `Async` trait
//!    默认方法），**不需要** `use ...::Async`（那个 trait 是私有的，import 会编译失败）。
//! 3. `Thumbnail()` 返回 `Result<IRandomAccessStreamReference>`，**不是 Option**。
//! 4. **不能把流 `cast` 成 `DataReader`**（DataReader 是独立对象、不是流的接口）
//!    ⇒ 直接 `E_NOINTERFACE(0x80004002)`。正解 `DataReader::CreateDataReader(stream)`。
//! 5. `DataReaderLoadOperation::join()` 返回**已读字节数 `u32`**，不是操作对象。
//! 6. `IVectorView` 的入口是 `Size()` / `GetAt(u32)`（同样 PascalCase）。
//! 7. `windows` crate **不 re-export** `windows_future` / `windows_collections`，
//!    `Cargo.toml` 里必须显式声明这两个依赖。

#![cfg(target_os = "windows")]

use crate::state::lock_unpoisoned;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;
use windows::Media::Control::{
    GlobalSystemMediaTransportControlsSession, GlobalSystemMediaTransportControlsSessionManager,
    GlobalSystemMediaTransportControlsSessionPlaybackStatus,
};
use windows::Win32::System::Com::CoInitializeEx;
use windows::Win32::System::Com::COINIT_MULTITHREADED;

/// 单个媒体会话的展示信息（**不含封面字节**，封面单独走 LRU）。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SessionInfo {
    /// 会话 id（= 源应用的 AUMID），用作**稳定身份**。
    pub id: String,
    pub title: String,
    pub artist: String,
    pub playing: bool,
    pub can_prev: bool,
    pub can_play_pause: bool,
    pub can_next: bool,
}

/// 音乐面板的完整快照（绘制层唯一输入）。
#[derive(Debug, Clone, Default)]
pub struct MusicSnapshot {
    pub sessions: Vec<SessionInfo>,
    /// 当前选中的会话下标。`sessions` 为空时恒为 0。
    pub current: usize,
    /// 封面缓存键（封面字节的 FNV-1a hash）。0 = 无封面。
    pub cover_hash: u64,
    pub cover_size: u32,
    /// ⭐⭐ **用户显式选中的会话 id**（`None` = 跟随系统）。
    ///
    /// ⛔⛔ **为什么必须记 id 而不是记下标**（修「切不过面板」）：
    ///   `select_session` 改的是 `current` 下标，而下一次 `refresh_snapshot` 会用
    ///   **系统**的 `GetCurrentSession()` **重算** `current`
    ///   ⇒ 用户的选���下一轮就被抹掉。
    ///   后果不是「选中错会话」，而是**「切换」按钮永远走不到切面板那一步**：
    ///   `advance_switch_target` 的第①步判「还有下一个会话」在 `current` 恒为 0 时
    ///   **永远成立** ⇒ 从设备面板**永远切不到音乐面板**（真机实测连点 13 次，
    ///   面板纹丝不动、且**没有任何日志**）。
    ///
    ///   记 **id**（而非下标）是因为会话列表会因应用启停而**重排**，
    ///   下标随时可能指向另一个应用。
    ///   ⚠️ 该会话消失时（应用退出）清空 ⇒ 回落跟随系统，这是期望行为。
    pub pinned_session_id: Option<String>,
    /// ⭐⭐ **当前会话所属应用的会话音量**（分数 `0.0..1.0`；`None` =
    ///   没有会话、或该应用在默认输出设备上匹配不到音频会话）。
    ///
    /// ⚠️ **它来自音频会话侧、不是 SMTC 侧**：SMTC 不含音量（方法面里
    ///   没有任何音量接口），音量只能经 `IAudioSessionManager` 读
    ///   `ISimpleAudioVolume` ⇒ 必须先按 §8.5.3c 的 AUMID↔exe 名口径把
    ///   会话匹配上才读得到。
    /// ⛔ **存快照而不是现读**：tooltip 与绘制都在绘制线程上，而枚举会话 /
    ///   读音量是 COM 阻塞调用 ⇒ 现读会把阻塞带进绘制路径。
    /// 由音乐后台线程按轮询刷新（≤1.5s），滚轮改完由 worker 立即乐观更新。
    pub session_volume: Option<f32>,
}

impl MusicSnapshot {
    pub fn current_session(&self) -> Option<&SessionInfo> {
        self.sessions.get(self.current)
    }
    /// 是否有任何媒体会话（决定「音乐面板可不可用」）。
    pub fn available(&self) -> bool {
        !self.sessions.is_empty()
    }
}

/// 封面缓存：hash → 已解码缩放到 `COVER_PX` 的 RGBA。
///
/// ⭐ 容量小是有意的：任务栏只显示一枚小封面，解一张 400×400 JPEG 要几毫秒，
///   没必要留一堆。淘汰用最简单的「插队式」——满了就丢掉最早插入的那个。
static COVER_CACHE: OnceLock<Mutex<CoverCache>> = OnceLock::new();

/// 封面在缓存里的**画布边长**（不是显示边长！显示边长是 `taskbar_widget` 的
/// `m.icon`，随 DPI/缩放档位变）。
///
/// ⛔⛔ **缓存画布不得小于任何目标尺寸**：32px 的画布在「跟随系统缩放」下要被
///   **放大**到 `m.icon`（125% ⇒ 40），而放大造不出细节——双三次/双线性只能把
///   每个源像素摊成渐变块，照片类内容尤其明显（大片柔和色带，实测「封面非常模糊」）。
///   ⇒ 与图标的修法一致：**缓存画布必须大于任何目标尺寸**，让运行时永远走**缩小**
///   （面积平均 = 超采样渲染，锐利）。
///   128 覆盖到 200% 缩放（`m.icon` 64）仍有 2:1 余量。
///   代价：单张 128×128×4 = 64KB，缓存 4 张共 256KB —— 可接受。
///
/// ⭐ 提到 **256** 是因为「两次重采样」本身也是损失来源。实测（400×400 源 → 40px，
///   对照「单步 lanczos3」）：
///   ```text
///   128→40 lanczos3  MAD 1.80   400→40 单步 cubic MAD 0.65
///   ```
///   400→128 是 3.1:1、128→40 又是 3.2:1，**两遍滤波各糊一次**；
///   缓存留 256 只剩一次温和的 1.56:1 预处理，接近单步效果。
///   代价：单张 256×256×4 = 256KB，缓存 4 张共 1MB —— 可接受。
pub const COVER_PX: u32 = 256;
const COVER_CACHE_CAP: usize = 4;

/// 已解码的封面：预乘 alpha 的 RGBA8（画布 `COVER_PX × COVER_PX`）。
pub struct CoverImage {
    pub px: u32,
    /// 预乘后的 RGBA，逐像素 4 字节。
    pub data: Vec<u8>,
}

struct CoverCache {
    entries: Vec<(u64, CoverImage)>,
}

impl CoverCache {
    fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }
    fn get(&self, hash: u64) -> Option<&CoverImage> {
        self.entries
            .iter()
            .find(|(h, _)| *h == hash)
            .map(|(_, v)| v)
    }
    fn put(&mut self, hash: u64, img: CoverImage) {
        if self.entries.iter().any(|(h, _)| *h == hash) {
            return;
        }
        if self.entries.len() >= COVER_CACHE_CAP {
            self.entries.remove(0);
        }
        self.entries.push((hash, img));
    }
}

fn cover_cache() -> &'static Mutex<CoverCache> {
    COVER_CACHE.get_or_init(|| Mutex::new(CoverCache::new()))
}

/// 读当前会话的封面（已解码、已缩放、已预乘）。
pub fn cover(hash: u64) -> Option<CoverImage> {
    if hash == 0 {
        return None;
    }
    lock_unpoisoned(cover_cache())
        .get(hash)
        .map(|i| CoverImage {
            px: i.px,
            data: i.data.clone(),
        })
}

// ═══════════════════════════════════════════════════════════════════
// 快照 + 唤醒通道
// ═══════════════════════════════════════════════════════════════════

static SNAPSHOT: Mutex<MusicSnapshot> = Mutex::new(MusicSnapshot {
    sessions: Vec::new(),
    current: 0,
    cover_hash: 0,
    cover_size: 0,
    pinned_session_id: None,
    session_volume: None,
});

/// 后台线程是否已起来（避免重复起线程）。
static WORKER_STARTED: AtomicBool = AtomicBool::new(false);

/// 后台线程是否**已经起来过**（只读观察口，供按需启动的验收脚本与判据使用）。
///
/// ⭐ 为什么需要这个观察口：按需启动的判据是「该起的时候起了、没有的时候确实没起」，
/// 而「没起」这件事**从外部看不出来**——快照为空既可能是「没起」，也可能是
/// 「起了但机器上一个会话都没有」。⚠️ 所以验收必须能直接问这一句。
pub fn worker_started() -> bool {
    WORKER_STARTED.load(Ordering::Acquire)
}
/// 会话集合的版本号：每读到一次新的会话集合就 +1（去重判据之一）。
static SESSION_EPOCH: AtomicUsize = AtomicUsize::new(0);
/// 读当前快照（绘制层入口）。
pub fn snapshot() -> MusicSnapshot {
    lock_unpoisoned(&SNAPSHOT).clone()
}

/// 请求后台线程重取一次（SMTC 事件、会话切换后调用）。
pub fn request_refresh() {
    send_cmd(Cmd::Refresh);
}

/// 刷新快照里的**会话音量**（tooltip 第三行显示它）。
///
/// ⛔ **只在后台线程调**：内部要枚举音频会话 + 读音量（COM 阻塞），
///   而 `SNAPSHOT` 是绘制线程每帧都要读的锁 ⇒ 绝不能在持锁状态下做。
/// ⛔ **只在有会话时才做**：没有会话就没有「当前应用的音量」，白花一次
///   枚举（每次要把全部会话的音量/静音都读一遍）。
/// ⚠️ 会话音量变化**不触发任何 SMTC 事件** ⇒ 它只能靠这里的轮询
///   （跟随 worker 的 ≤1.5s 节奏）发现；滚轮造成的变更走
///   `set_session_volume_hint` 立即生效，不等轮询。
pub fn refresh_session_volume() {
    let (aumid, has) = {
        let s = lock_unpoisoned(&SNAPSHOT);
        let a = s
            .sessions
            .get(s.current)
            .map(|x| x.id.clone())
            .unwrap_or_default();
        (a, !s.sessions.is_empty())
    };
    if !has {
        // ⛔ 无会话时必须显式写 None，不能保留上一轮的值：面板已切走却
        //   还显示旧音量，比不显示更坏（用户会以为那是当前应用的音量）。
        let mut s = lock_unpoisoned(&SNAPSHOT);
        if s.session_volume.is_some() {
            s.session_volume = None;
        }
        return;
    }
    let vol = crate::audio::media_session_volume(&aumid);
    let mut s = lock_unpoisoned(&SNAPSHOT);
    // ⚠️ 只有**值真的变了**才记日志：这是每 1.5s 一次的轮询，
    //   逐次打日志会把日志刷爆。
    if let (Some(a), Some(b)) = (s.session_volume, vol) {
        if (a - b).abs() < f32::EPSILON {
            return;
        }
    } else if s.session_volume.is_none() && vol.is_none() {
        return;
    }
    crate::process::append_log(&format!(
        "[music] 会话音量刷新: aumid={aumid} {:?} -> {:?}",
        s.session_volume, vol
    ));
    s.session_volume = vol;
}

/// 滚轮改完会话音量后**立即**把新值落到快照（乐观更新）。
///
/// ⭐ 与设备侧「乐观更新快照 + 立刻 `post_refresh`」是同一招：等下一轮
///   轮询（≤1.5s）会让 tooltip 里的数字在用户滚完时**还是旧值**，
///   看起来就是「滚了但没反应」。
pub fn set_session_volume_hint(volume: f32) {
    let mut s = lock_unpoisoned(&SNAPSHOT);
    s.session_volume = Some(volume);
}

// ═══════════════════════════════════════════════════════════════════
// 后台线程
// ═══════════════════════════════════════════════════════════════════

/// `AppHandle` 句柄：后台线程要靠它把「该重估挂载了」投递回主线程。
static APP: OnceLock<tauri::AppHandle> = OnceLock::new();

/// 取 `AppHandle`（未启动时为 `None`）。
pub fn app() -> Option<tauri::AppHandle> {
    APP.get().cloned()
}

/// 切到第 `idx` 个会话（用户点「切换」时调用；只改内存选中态，**不改配置**）。
pub fn select_session(idx: usize) {
    let mut s = lock_unpoisoned(&SNAPSHOT);
    if idx < s.sessions.len() {
        s.current = idx;
        // ⭐ 记 **id**：下标会随会话列表重排而失效，id 不会
        //   （会话 id = 源应用 AUMID，见 `SessionInfo::id`）
        s.pinned_session_id = Some(s.sessions[idx].id.clone());
        // ⛔⛔ **换会话必须同时清封面**（修「闪现另一个会话的封面」）。
        //   `cover_hash` 是**由 worker 异步解码后回填**的，而 `current` 是这里
        //   **同步**改的 ⇒ 不清就会出现一段「`current` 已是新会话、`cover_hash`
        //   还是旧会话」的**内部不自洽**快照 ⇒ 面板先画出**旧封面**，
        //   等 worker 回来再重画 ⇒ 闪一下（真机实测）。
        //   清成 0 ⇒ 期间**不画封面**（`cover(0)` 返回 `None`），
        //   是「空缺」而不是「错误」——观感上明显更好，且绝不该改成
        //   「在新会话的封面到位前先沿用旧封面」。
        s.cover_hash = 0;
        s.cover_size = 0;
    }
    drop(s);
    request_refresh();
}

/// 启动后台线程（幂等）。在**主线程**调用，但立刻返回——不在其上等任何东西。
pub fn start(app: tauri::AppHandle) {
    if WORKER_STARTED.swap(true, Ordering::AcqRel) {
        return;
    }
    let _ = APP.set(app);
    let (tx, rx) = channel::<Cmd>();
    let _ = CMD_TX.set(tx);
    // ⭐ 命令通道**兼作唤醒源**（`recv_timeout` 本身就是「有活干或超时」的唯一信号），
    //   故不再单开一条唤醒通道 —— 多一条通道就多一处「发了但没人收」的失联可能。
    let spawned = std::thread::Builder::new()
        .name("pm-music-smtc".to_string())
        .spawn(move || music_worker_loop(rx));
    match spawned {
        Ok(_) => crate::process::append_log("[music] SMTC 后台线程已启动"),
        Err(e) => {
            // ⚠️ 启动失败必须复位，否则 `request_refresh` 会静默无动作
            WORKER_STARTED.store(false, Ordering::Release);
            crate::process::append_log(&format!("[music] SMTC 后台线程启动失败: {e}"));
        }
    }
}

fn music_worker_loop(rx: Receiver<Cmd>) {
    // ⛔ MTA：WinRT 异步在线程池完成，MTA 线程无需消息泵即可 join。
    //    失败（RPC_E_CHANGED_MODE）不致命——已有 apartment 也能用，只是不能改。
    let co = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
    if co.is_err() {
        crate::process::append_verbose_log(&format!(
            "[music] CoInitializeEx(MTA) 返回 {co:?}（继续，可能已有 apartment）"
        ));
    }

    let mgr = match request_manager() {
        Ok(m) => m,
        Err(e) => {
            crate::process::append_log(&format!("[music] 取 SMTC 会话管理器失败: {e}"));
            return; // 线程退出：音乐面板永远不可用，但设备面板不受影响
        }
    };

    // ⭐ **manager 层的两个事件必须订**（恰好漏了这俩 ⇒ 换 app 播放时
    //   要等下一次媒体属性变化才反应过来，慢一拍）：
    //   · `SessionsChanged`      → 有会话出现/消失（决定「音乐面板可不可用」）
    //   · `CurrentSessionChanged`→ 播放焦点转移（决定「当前是哪个会话」）
    subscribe_manager(&mgr);

    // 会话集合变化 ⇒ 需要重新订阅
    let mut watched: Vec<(String, Subscription)> = Vec::new();

    // ⛔⛔ 循环退出条件只能是「通道断开」，**绝不是「收到了一条消息」**。
    //   ⛔ 不得用 `pending = rx.recv_timeout(..).is_err()`：超时(is_err=true)会继续、
    //   **收到消息(is_err=false)反而退出** ⇒
    //   线程在**第一条消息**上死掉。而 `request_refresh()` 会被**每个 SMTC 事件**调用
    //   ⇒ 实测启动约 90 秒后后台线程就没了：
    //   订阅被 drop、快照被清空（会话数=0）⇒ 面板按回落链退成设备组件。
    //   ⭐ 这类「跑一会儿就消失」的 bug，日志里**看不出异常**（线程正常返回），
    //   唯一线索是快照被清空 —— 这类 bug **日志里看不出异常**。
    loop {
        // 先把积压的命令**全部**执行掉（它们比取数更即时）
        while let Ok(cmd) = rx.try_recv() {
            run_cmd(&mgr, cmd);
        }
        // 阻塞等待下一条：收到就处理、**继续循环**；超时则退化成 1.5s 一次的兜底取数；
        // **只有通道断开（所有 Sender 都drop 了）才退出**。
        match rx.recv_timeout(Duration::from_millis(1500)) {
            Ok(cmd) => run_cmd(&mgr, cmd),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                crate::process::append_log("[music] 命令通道断开，后台线程退出");
                break;
            }
        }
        // 重取会话集合并 diff
        match list_sessions(&mgr) {
            Ok(list) => {
                let ids: Vec<String> = list.iter().map(|(_, i)| i.id.clone()).collect();
                let changed = {
                    let cur: Vec<&String> = watched.iter().map(|(id, _)| id).collect();
                    cur != ids.iter().collect::<Vec<&String>>()
                };
                if changed {
                    // 退订消失的（drop 闭包 = 解引用 = 不泄漏）
                    // ⛔ `retain` 给的是 `&T` 拿不走所有权 ⇒ 先 `std::mem::take`
                    //   把要退订的那些**移出来**，再统一退订（退订会 drop 闭包与 session）。
                    let gone: Vec<Subscription> = std::mem::take(&mut watched)
                        .into_iter()
                        .filter(|(id, _)| !ids.contains(id))
                        .map(|(_, sub)| sub)
                        .collect();
                    watched.retain(|(id, _)| ids.contains(id));
                    for sub in gone {
                        sub.unsubscribe();
                    }
                    // 订阅新增的
                    for (session, info) in &list {
                        if watched.iter().any(|(id, _)| id == &info.id) {
                            continue;
                        }
                        if let Some(sub) = subscribe(session) {
                            watched.push((info.id.clone(), sub));
                        }
                    }
                    SESSION_EPOCH.fetch_add(1, Ordering::AcqRel);
                }
            }
            Err(e) => crate::process::append_verbose_log(&format!("[music] GetSessions 失败: {e}")),
        }
        refresh_snapshot(&mgr);
        refresh_session_volume();
    }
    // 线程退出前把快照清空：否则音乐面板会永远停在最后一帧的旧数�
    {
        let mut s = lock_unpoisoned(&SNAPSHOT);
        s.sessions.clear();
        s.cover_hash = 0;
        s.session_volume = None;
    }
    notify_widget();
}

/// 取会话管理器（**阻塞等待**，只能在后台线程调）。
fn request_manager() -> windows::core::Result<GlobalSystemMediaTransportControlsSessionManager> {
    let op = GlobalSystemMediaTransportControlsSessionManager::RequestAsync()?;
    op.join()
}

/// 列出全部会话的基础信息（不含封面 —— 封面只在选中那个会话上取）。
fn list_sessions(
    mgr: &GlobalSystemMediaTransportControlsSessionManager,
) -> windows::core::Result<Vec<(GlobalSystemMediaTransportControlsSession, SessionInfo)>> {
    let view = mgr.GetSessions()?;
    let n = view.Size().unwrap_or(0);
    let mut out = Vec::new();
    for i in 0..n {
        let Ok(session) = view.GetAt(i) else { continue };
        if let Some(info) = read_session_info(&session) {
            out.push((session, info));
        }
    }
    Ok(out)
}

fn read_session_info(s: &GlobalSystemMediaTransportControlsSession) -> Option<SessionInfo> {
    let id = s.SourceAppUserModelId().ok()?.to_string();
    // ⚠️ 逐会话 id 走 verbose：SMTC 只给 AUMID，**不给 PID**，而音量要落到
    //   音频会话（唯一带 PID 的一侧）⇒ 两边的匹配依据全在这串文本的形态上，
    //   换播放器 / 换商店版包形态就可能变，排障必须看得到原值。
    crate::process::append_verbose_log(&format!("[music] SMTC 会话: id={}", id));
    let (playing, can_prev, can_play_pause, can_next) = match s.GetPlaybackInfo() {
        Ok(info) => {
            let playing = matches!(
                info.PlaybackStatus(),
                Ok(GlobalSystemMediaTransportControlsSessionPlaybackStatus::Playing)
            );
            let (p, n) = match info.Controls() {
                Ok(c) => (
                    c.IsPreviousEnabled().unwrap_or(false),
                    c.IsNextEnabled().unwrap_or(false),
                ),
                Err(_) => (false, false),
            };
            let (play, pause) = match info.Controls() {
                Ok(c) => (
                    c.IsPlayEnabled().unwrap_or(false),
                    c.IsPauseEnabled().unwrap_or(false),
                ),
                Err(_) => (false, false),
            };
            (playing, p, play || pause, n)
        }
        Err(_) => (false, false, false, false),
    };
    Some(SessionInfo {
        id,
        title: String::new(),
        artist: String::new(),
        playing,
        can_prev,
        can_play_pause,
        can_next,
    })
}

// ═══════════════════════════════════════════════════════════════════
// 会话订阅
// ═══════════════════════════════════════════════════════════════════

/// 一个会话上的 4 个事件订阅句柄。
///
/// ⛔ `Drop` 时必须退订：闭包持有 `Session` 强引用，不退订 = 闭包永远不释放
///   = 会话对象泄漏。这正是「失败/退出路径必须清理」那条纪律的适用场景。
struct Subscription {
    media_props: i64,
    playback_info: i64,
    timeline: i64,
    session: GlobalSystemMediaTransportControlsSession,
}

impl Subscription {
    fn unsubscribe(self) {
        let s = &self.session;
        let _ = s.RemoveMediaPropertiesChanged(self.media_props);
        let _ = s.RemovePlaybackInfoChanged(self.playback_info);
        let _ = s.RemoveTimelinePropertiesChanged(self.timeline);
        crate::process::append_verbose_log("[music] 已退订会话事件");
    }
}

fn subscribe(session: &GlobalSystemMediaTransportControlsSession) -> Option<Subscription> {
    use windows::Foundation::TypedEventHandler;
    use windows::Media::Control::{
        MediaPropertiesChangedEventArgs, PlaybackInfoChangedEventArgs,
        TimelinePropertiesChangedEventArgs,
    };
    type Handler<A> = TypedEventHandler<GlobalSystemMediaTransportControlsSession, A>;

    // ⭐ **闭包不捕获 `session`**：`TypedEventHandler` 的回调签名本身就带
    //   `sender: Option<&Session>` ⇒ 不需要 clone 进去。
    //   这不是风格问题：闭包若捕获 `Session` 就是一条**强引用**，
    //   而 WinRT 事件是**长生命**的（退订前一直挂着）⇒ 会话对象永远不释放 = 泄漏。
    //   会话由 `Subscription.session` 字段持有，`unsubscribe` 时随 self 一起 drop。
    let fire = |tag: &str| {
        SESSION_EPOCH.fetch_add(1, Ordering::AcqRel);
        if crate::config::verbose_log_enabled() {
            crate::process::append_verbose_log(&format!("[music] 事件 {tag}"));
        }
        request_refresh();
    };

    let media_props = session
        .MediaPropertiesChanged(&Handler::<MediaPropertiesChangedEventArgs>::new(
            move |_, _| {
                fire("media_props");
                // ⚠️ `TypedEventHandler` 的闭包返回 `Result<()>`，不是 `()`
                Ok(())
            },
        ))
        .ok()?;

    // ⚠️ **平台没有独立的 `PlaybackStateChanged` 事件**（实测：该 crate 的 session
    //   只有 Timeline/PlaybackInfo/MediaProperties 三个）⇒ 播放状态变化由
    //   `PlaybackInfoChanged` 一并覆盖。**别照抄第三方库那份四事件列表**
    //   （它有独立事件，本仓直连 WinRT 没有）。
    let playback_info = session
        .PlaybackInfoChanged(&Handler::<PlaybackInfoChangedEventArgs>::new(
            move |_, _| {
                fire("playback_info");
                Ok(())
            },
        ))
        .ok()?;

    let timeline = session
        .TimelinePropertiesChanged(&Handler::<TimelinePropertiesChangedEventArgs>::new(
            move |_, _| {
                fire("timeline");
                Ok(())
            },
        ))
        .ok()?;

    Some(Subscription {
        media_props,
        playback_info,
        timeline,
        session: session.clone(),
    })
}

// ═══════════════════════════════════════════════════════════════════
// 快照刷新
// ═══════════════════════════════════════════════════════════════════

/// 开发门控：把**艺人名强制清空**（`PM_DEV_EMPTY_ARTIST=1`）。
///
/// ⭐ **为什么需要它**：空 artist 是 那次**访问违例闪退**
///   的触发条件，而它的复现**不可控** —— 得恰好有个播放器不上报 artist。
///   ⛔ **它不是那道闪退的兜底**：闪退已由机械判据
///   `measure_text_on_empty_slice_returns_zero_and_never_touches_gdi` 覆盖
///   （拆掉空串闸 → 测试进程 `0xc0000005`，CI 随时能跑）。
///   ⇒ 本门控只补**判据覆盖不到的那一格**：空 artist 下面板的**实际观感**
///   （宽度算窄、文本是否被裁切）—— 那只能看像素。
///
/// 📌 与 `PM_DEV_TOOLTIP_SHOW` 同款理由（见其注释：「本机无法注入鼠标，
///   自然 hover 无法自动验收」）：**把不可注入的条件变成可判定的条件**。
///
/// ⚠️ 只清**艺人**、不动标题 —— 标题本就有「未在播放」兜底，
///   两处都清会把变量混在一起。
fn dev_empty_artist_enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| {
        let on = std::env::var("PM_DEV_EMPTY_ARTIST").is_ok();
        if on {
            crate::standard_log!(
                "[music] ⚠️ 开发门控 PM_DEV_EMPTY_ARTIST 已开启：艺人名将被强制清空"
            );
        }
        on
    })
}

/// 门控的**纯**部分：抽出成函数才能单测（环境变量本身不好测，效果好测）。
fn apply_dev_artist_gate(artist: String, on: bool) -> String {
    if on {
        String::new()
    } else {
        artist
    }
}

/// ⭐⭐ 「当前该显示哪个会话」的**唯一判据**（纯函数，可单测）。
///
/// 返回 `(下标, 钉子是否命中)`。
///
/// 判据顺序**不可调换**：
/// ① 用户钉住的 id 仍存在 ⇒ 用它（**用户的显式选择必须赢过系统的自动判定**）
/// ② 否则系统的 `GetCurrentSession` 对应的会话
/// ③ 否则 0
///
/// ⛔⛔ 反序的后果（真机实测）：`select_session` 改的下标每轮都被
///   冲回系统当前 ⇒ 「切换」按钮第①步「还有下一个会话」**恒成立**
///   ⇒ 从设备面板**永远切不到音乐面板**，且**零日志**。
fn resolve_current_index(
    pinned: Option<&str>,
    sessions: &[SessionInfo],
    system_id: Option<&str>,
) -> (usize, bool) {
    if let Some(pid) = pinned {
        if let Some(pos) = sessions.iter().position(|s| s.id == pid) {
            return (pos, true);
        }
        // 钉住的会话已消失 ⇒ 落回系统当前（`false` = 钉子失效，调用方据此清空）
    }
    if let Some(sid) = system_id {
        if let Some(pos) = sessions.iter().position(|s| s.id == sid) {
            return (pos, false);
        }
    }
    (0, false)
}

fn refresh_snapshot(mgr: &GlobalSystemMediaTransportControlsSessionManager) {
    let Ok(list) = list_sessions(mgr) else { return };
    let mut sessions: Vec<SessionInfo> = list.iter().map(|(_, i)| i.clone()).collect();

    // ⭐⭐ 当前会话：**用户钉住的优先**，否则用系统的 `GetCurrentSession`，再否则第一个。
    //   ⚠️ 顺序不能反：反了就是「每轮刷新都把用户的选择抹回系统当前」，
    //   而「切换」按钮的第①步（还有下一个会话）因此永远成立
    //   ⇒ **永远切不到另一个面板**（见 `pinned_session_id` 的注释）。
    let pinned = lock_unpoisoned(&SNAPSHOT).pinned_session_id.clone();
    let system_id = mgr
        .GetCurrentSession()
        .ok()
        .and_then(|cur| cur.SourceAppUserModelId().ok().map(|h| h.to_string()));
    let (current, pinned_hit) =
        resolve_current_index(pinned.as_deref(), &sessions, system_id.as_deref());
    // ⛔ 钉子失效（应用退出）必须**在这里**清掉，不能放进下面的「内容有变化才写」里：
    //   否则快照恰好没变化时钉子会**永久留着**，之后每轮都白跑一次「钉子查找」。
    if pinned.is_some() && !pinned_hit {
        let mut g = lock_unpoisoned(&SNAPSHOT);
        g.pinned_session_id = None;
    }

    // ⭐ 标题/艺人/封面只对**当前会话**取（每次都取所有会话的封面纯属浪费）
    let mut cover_hash = 0u64;
    let mut cover_size = 0u32;
    if let Some((session, info)) = list.get(current) {
        if let Ok(props) = session
            .TryGetMediaPropertiesAsync()
            .and_then(|op| op.join())
        {
            if let Ok(t) = props.Title() {
                sessions[current].title = t.to_string();
            }
            if let Ok(a) = props.Artist() {
                sessions[current].artist = a.to_string();
            }
            sessions[current].artist = apply_dev_artist_gate(
                std::mem::take(&mut sessions[current].artist),
                dev_empty_artist_enabled(),
            );
            if let Ok(Some(bytes)) = read_thumbnail(&props) {
                cover_hash = fnv1a(&bytes);
                cover_size = COVER_PX;
                if cover(cover_hash).is_none() {
                    if let Some(img) = decode_cover(&bytes) {
                        lock_unpoisoned(cover_cache()).put(cover_hash, img);
                    }
                }
            }
        }
        let _ = info;
    }

    // ⭐ 去重：内容与下标都没变就不发通知（SMTC 事件会**重复**触发，
    //   不去重会让封面被反复重解码 —— 这是 踩过并修掉的）。
    {
        let mut snap = lock_unpoisoned(&SNAPSHOT);
        let same =
            snap.sessions == sessions && snap.current == current && snap.cover_hash == cover_hash;
        if same {
            return;
        }
        snap.sessions = sessions;
        snap.current = current;
        snap.cover_hash = cover_hash;
        snap.cover_size = cover_size;
    }
    notify_widget();
}

/// 单向通知 widget 重绘 / 重估挂载。
pub fn notify_widget() {
    crate::taskbar_widget::on_music_changed();
}

// ═══════════════════════════════════════════════════════════════════
// 封面读取与解码
// ═══════════════════════════════════════════════════════════════════

/// 读封面的原始字节。
///
/// ⛔ 四个坑见模块文档：不能 cast 成 DataReader、要 `CreateDataReader`、
///    `LoadAsync().join()` 返回 u32、`Thumbnail()` 不是 Option。
fn read_thumbnail(
    props: &windows::Media::Control::GlobalSystemMediaTransportControlsSessionMediaProperties,
) -> windows::core::Result<Option<Vec<u8>>> {
    use windows::core::Interface;
    use windows::Storage::Streams::{DataReader, IInputStream};

    let thumb = props.Thumbnail()?;
    let stream = thumb.OpenReadAsync()?.join()?;
    let size = stream.Size().unwrap_or(0);
    if size == 0 {
        return Ok(None);
    }
    let input: IInputStream = stream.cast()?;
    let reader = DataReader::CreateDataReader(&input)?;
    let loaded = reader.LoadAsync(size as u32)?.join()?;
    let mut buf = vec![0u8; loaded as usize];
    reader.ReadBytes(&mut buf)?;
    Ok(Some(buf))
}

fn fnv1a(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// 解码 + 缩放到 `COVER_PX` + **预乘 alpha**。
///
/// ⭐ 缩放滤波显式指定（放大双线性 / 缩小最近邻），理由与设备图标完全同源：
///   本仓纪律「放大双线性 / 缩小最近邻，且**必须**在预乘空间插值」——
///   直接在直通 alpha 上插值会让透明像素的 RGB 混进边缘 ⇒ 黑晕。
fn decode_cover(bytes: &[u8]) -> Option<CoverImage> {
    let img = image::load_from_memory(bytes).ok()?.to_rgba8();
    let side = COVER_PX;
    let (w, h) = (img.width(), img.height());
    // ⛔⛔ **不得**写成 `w.min(h).min(COVER_PX * 4)`：那个 `.min(128)` 看似「限制
    //   处理量」，实则**把图裁成了中心 128×128**（400×400 封面的正中 32%）
    //   ⇒ 屏幕上表现为「封面只显示了一部分」。
    //   ⭐ 正确做法：**取整张图的正方形部分**再缩放。`MAX_DECODE_PX` 只用来
    //   防止超大图吃掉内存，命中它时**先缩后裁**而不是**先裁后缩**。
    const MAX_DECODE_PX: u32 = 512;
    // 详细级记**源图分辨率**：封面糊有两个完全不同的成因（源图小 / 重采样差），
    // 不量出来就没法区分——而两者的修法互不相干。
    if crate::config::verbose_log_enabled() {
        crate::process::append_log(&format!(
            "[music] 封面源图 {w}x{h} → 缓存 {COVER_PX}x{COVER_PX}"
        ));
    }
    let (img, w, h) = if w.max(h) > MAX_DECODE_PX {
        // 超大图：先整体缩到上限内（**保持完整画面**），再取正方形
        let k = MAX_DECODE_PX as f32 / w.max(h) as f32;
        let nw = ((w as f32 * k).round() as u32).max(1);
        let nh = ((h as f32 * k).round() as u32).max(1);
        let small = image::imageops::resize(&img, nw, nh, image::imageops::FilterType::Triangle);
        (small, nw, nh)
    } else {
        (img, w, h)
    };
    let side_px = w.min(h);
    let src_x = (w - side_px) / 2;
    let src_y = (h - side_px) / 2;

    // 先做「正方形裁剪 + 预乘」，再缩放
    let crop = image::imageops::crop_imm(&img, src_x, src_y, side_px, side_px).to_image();
    let mut pre = image::RgbaImage::new(side_px, side_px);
    for y in 0..side_px {
        for x in 0..side_px {
            let p = crop.get_pixel(x, y).0;
            let a = p[3] as u32;
            pre.put_pixel(
                x,
                y,
                image::Rgba([
                    ((p[0] as u32 * a) / 255) as u8,
                    ((p[1] as u32 * a) / 255) as u8,
                    ((p[2] as u32 * a) / 255) as u8,
                    p[3],
                ]),
            );
        }
    }
    // ⛔ 缩小**不用 Nearest**：照片类内容缩小时按点抽样会把细密纹理抽成噪点。
    //   两侧都用 Triangle（= 面积平均的近似），只有放大才另说。
    let filter = image::imageops::FilterType::Triangle;
    let small = image::imageops::resize(&pre, side, side, filter);
    Some(CoverImage {
        px: side,
        data: small.into_raw(),
    })
}

// ═══════════════════════════════════════════════════════════════════
// 控制命令（由 UI 线程调用 ⇒ 立刻投递给后台线程，绝不在调用方阻塞）
// ═══════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {

    /// 开发门控 `PM_DEV_EMPTY_ARTIST` 的**纯**判据。
    ///
    /// ⭐ 为什么要单测它：门控本身「有没有生效」很好测（「值变空」），
    ///   但**它会不会误伤**（关着时必须原样透传）才是容易写错的地方。
    #[test]
    fn dev_artist_gate_blanks_only_when_enabled() {
        assert_eq!(apply_dev_artist_gate("Aimer".into(), true), "");
        assert_eq!(
            apply_dev_artist_gate("Aimer".into(), false),
            "Aimer",
            "门控关着时必须原样透传（误伤会让所有会话都没艺人名）"
        );
        // 本来就是空串时，两种情况都应为空（幂等）
        assert_eq!(apply_dev_artist_gate(String::new(), false), "");
        assert_eq!(apply_dev_artist_gate(String::new(), true), "");
    }

    /// ⭐⭐ **用户的显式选择必须赢过系统的自动判定**。
    ///
    /// 可证伪：把 `resolve_current_index` 的判据顺序反过来（本用例立刻转红，
    /// 且症状是「切换按钮永远切不到另一个面板」——**零日志**的那种）。
    #[test]
    fn pinned_session_wins_over_system_current() {
        let sessions = vec![
            SessionInfo {
                id: "App.A".into(),
                title: "A".into(),
                artist: String::new(),
                playing: true,
                can_prev: false,
                can_play_pause: true,
                can_next: false,
            },
            SessionInfo {
                id: "App.B".into(),
                title: "B".into(),
                artist: String::new(),
                playing: false,
                can_prev: false,
                can_play_pause: true,
                can_next: false,
            },
        ];
        // 用户钉住 B，系统说当前是 A ⇒ 必须显示 B
        let (idx, hit) = resolve_current_index(Some("App.B"), &sessions, Some("App.A"));
        assert_eq!(idx, 1, "钉住的下标必须胜出（否则用户的选择每轮被抹掉）");
        assert!(hit, "钉子命中");

        // 没钉子 ⇒ 跟系统
        let (idx, hit) = resolve_current_index(None, &sessions, Some("App.A"));
        assert_eq!(idx, 0);
        assert!(!hit, "未钉住时不应报告命中（否则会误清钉子）");

        // 钉子指向已消失的会话 ⇒ 落回系统，且**报告未命中**以便清钉子
        let (idx, hit) = resolve_current_index(Some("App.GONE"), &sessions, Some("App.A"));
        assert_eq!(idx, 0, "钉子失效必须落回系统当前");
        assert!(!hit, "失效的钉子必须报告未命中（调用方据此清空）");

        // 都没有 ⇒ 0
        assert_eq!(resolve_current_index(None, &sessions, None), (0, false));
    }

    /// 会话列表**重排**时下标会漂移 ⇒ 记 id 才安全。
    /// 判据：同 id 在不同下标上都必须解析到**正确的那一个**。
    #[test]
    fn resolution_follows_the_id_not_the_index() {
        let mk = |id: &str| SessionInfo {
            id: id.into(),
            title: id.into(),
            artist: String::new(),
            playing: false,
            can_prev: false,
            can_play_pause: true,
            can_next: false,
        };
        let before = vec![mk("App.A"), mk("App.B")];
        let after = vec![mk("App.B"), mk("App.C"), mk("App.A")]; // B/C 插入，A 挪到末尾
        assert_eq!(resolve_current_index(Some("App.A"), &before, None).0, 0);
        assert_eq!(
            resolve_current_index(Some("App.A"), &after, None).0,
            2,
            "列表重排后仍须按 id 找到同一个会话（记下标就会指错应用）"
        );
    }
    use super::*;

    /// ⭐⭐⭐ **后台线程不能在「收到消息」时退出**。
    ///
    /// 根因：循环退出条件写成 `pending = rx.recv_timeout(..).is_err()`
    /// ⇒ 超时(is_err=true)继续、**收到消息(is_err=false)反而退出**。
    /// 而 `request_refresh()` 会被**每个 SMTC 事件**调用 ⇒ 线程在第一条事件上死掉
    /// （实测约 90 秒后），订阅被 drop、快照被清空 ⇒ 面板按回落链退成设备组件。
    ///
    /// ⚠️ 这个 bug 特别贵的地方在于**日志里看不出异常**（线程正常返回、无 panic）。
    ///
    /// 判据：向通道发一条消息，接收端**必须继续等**（只有 `Disconnected` 才退出）。
    /// ⛔ 用 `recv`（而非 `try_recv` 轮询）：后者会让第二次等待直接返回 `Disconnected`。
    #[test]
    fn worker_loop_survives_incoming_message() {
        let (tx, rx) = std::sync::mpsc::channel::<Cmd>();
        // 模拟后台循环的等待逻辑
        // 只走**前 2 轮**：第 3 轮会阻塞在 `recv_timeout` 上，而 sender 仍活着
        // ⇒ 那里拿不到消息，断言不到任何东西（实测白等 150ms）。
        for _ in 0..2 {
            while let Ok(cmd) = rx.try_recv() {
                let _ = cmd;
            }
            match rx.recv_timeout(Duration::from_millis(50)) {
                Ok(cmd) => {
                    let _ = cmd;
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
        // 发 3 条，每条都应该被处理
        for _ in 0..3 {
            tx.send(Cmd::Refresh).unwrap();
        }
        // 关键：sender 还在（未 drop）⇒ 不该收到 Disconnected
        let mut processed = 0usize;
        for _ in 0..3 {
            while let Ok(cmd) = rx.try_recv() {
                let _ = cmd;
                processed += 1;
            }
            if processed >= 3 {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(processed, 3, "三条消息都应被处理（循环不能提前退出）");
        drop(tx);
        assert!(
            rx.recv_timeout(Duration::from_millis(50)).is_err(),
            "sender 全 drop 后才该收到 Disconnected"
        );
    }

    /// ⭐⭐⭐ **封面缓存画布必须大于任何目标尺寸**。
    ///
    /// 根因与设备图标当初同源：`COVER_PX` 曾是 **32**，而绘制时目标边长是
    /// `taskbar_widget` 的 `m.icon`（随 DPI 与缩放档位变，125% ⇒ 40）。
    /// 32 → 40 是**放大** ⇒ 照片类内容被摊成柔和色带。
    ///
    /// 判据一（尺寸）：`COVER_PX` 至少要能覆盖 200% 缩放的 `m.icon`（64px）。
    /// 可证伪：把 `COVER_PX` 改回 32 ⇒ 转红。
    #[test]
    fn cover_cache_outlives_every_display_size() {
        // 200% 系统缩放 ⇒ m.icon = 32 × 200/96 ≈ 67，取 64 作上界
        let max_display = 64u32;
        assert!(
            COVER_PX >= max_display * 2,
            "COVER_PX={COVER_PX} 太小：目标最大 {max_display}px 时只剩 {:.1}x 余量，             缩放会退化成**放大**（= 糊）",
            COVER_PX as f32 / max_display as f32
        );
    }

    /// ⭐⭐ **封面必须取整张图，不是中心裁剪**。
    ///
    /// 根因：解码时写了 `w.min(h).min(COVER_PX * 4)`，那个 `.min(128)` 本意是
    /// 「限制处理量」，实际把 400×400 的封面**裁成了中心 128×128**（正中 32%）。
    ///
    /// 判据构造：中心 40×40 纯红、其余纯蓝的 400×400 图
    /// → 若仍裁剪，32×32 输出**几乎全红**
    /// → 若取整张，输出**以蓝为主**（红只占 (40/400)² ≈ 1%）。
    #[test]
    fn cover_decodes_whole_image_not_center_crop() {
        const N: u32 = 400;
        let mut img = image::RgbaImage::new(N, N);
        for y in 0..N {
            for x in 0..N {
                let center = (180..220).contains(&x) && (180..220).contains(&y);
                img.put_pixel(
                    x,
                    y,
                    if center {
                        image::Rgba([255, 0, 0, 255])
                    } else {
                        image::Rgba([0, 0, 255, 255])
                    },
                );
            }
        }
        let mut cur = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(img)
            .write_to(&mut cur, image::ImageFormat::Png)
            .expect("encode");
        let bytes = cur.into_inner();

        let Some(out) = decode_cover(&bytes) else {
            panic!("构造的 PNG 应当能解码");
        };
        let side = COVER_PX as usize;
        let mut red = 0usize;
        for i in 0..(side * side) {
            let px = &out.data[i * 4..i * 4 + 4];
            if px[0] > 150 && px[2] < 100 {
                red += 1;
            }
        }
        let total = side * side;
        assert!(
            red < total / 20,
            "红色占 {:.1}% ⇒ 仍在做**中心裁剪**（不是整张图）",
            red as f64 * 100.0 / total as f64
        );
    }
}

// ═══════════════════════════════════════════════════════════════════
// 控制命令
// ═══════════════════════════════════════════════════════════════════

/// 控制命令（作用于「快照里的当前会话」）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cmd {
    Prev,
    PlayPause,
    Next,
    /// 纯唤醒：不执行任何 SMTC 调用，只让 worker 立刻重取一次快照。
    ///
    /// ⭐ 单列一个变体而不是「发个空命令」：`send_cmd` 的语义是**执行**，
    ///   拿它当唤醒会在 worker 里走进 `TryXxxAsync` —— 那是真的会去按播放键。
    Refresh,
}

/// 命令通道：`UI 线程 → 后台线程`。⛔ **只入队不阻塞**（UI 线程绝不能等 SMTC）。
static CMD_TX: OnceLock<Sender<Cmd>> = OnceLock::new();

/// 投递一条控制命令（**fire-and-forget**，UI 线程零等待）。
pub fn send_cmd(cmd: Cmd) {
    if let Some(tx) = CMD_TX.get() {
        if tx.send(cmd).is_err() {
            crate::process::append_log("[music] 命令通道已断开（后台线程已退出？）");
        }
    }
}

pub fn cmd_previous() {
    send_cmd(Cmd::Prev);
}
pub fn cmd_play_pause() {
    send_cmd(Cmd::PlayPause);
}
pub fn cmd_next() {
    send_cmd(Cmd::Next);
}

/// 在后台线程上执行一条命令。
///
/// ⛔ `TryXxxAsync()` 也要 `join()`（同 `RequestAsync`）⇒ 只能在这条后台线程上跑。
fn run_cmd(mgr: &GlobalSystemMediaTransportControlsSessionManager, cmd: Cmd) {
    if cmd == Cmd::Refresh {
        return; // 纯唤醒：worker 循环接着就会重取快照
    }
    let Ok(view) = mgr.GetSessions() else { return };
    let n = view.Size().unwrap_or(0);
    let cur = lock_unpoisoned(&SNAPSHOT).current as u32;
    if n == 0 || cur >= n {
        return;
    }
    let Ok(session) = view.GetAt(cur) else { return };
    let (label, res) = match cmd {
        // 纯唤醒：直接返回，不碰 SMTC 控制接口
        Cmd::Refresh => return,
        Cmd::Prev => ("上一首", session.TrySkipPreviousAsync()),
        Cmd::PlayPause => ("播放/暂停", session.TryTogglePlayPauseAsync()),
        Cmd::Next => ("下一首", session.TrySkipNextAsync()),
    };
    match res {
        Ok(op) => match op.join() {
            Ok(ok) => {
                if crate::config::verbose_log_enabled() {
                    crate::process::append_verbose_log(&format!("[music] {label} -> {ok}"));
                }
            }
            Err(e) => crate::process::append_log(&format!("[music] {label} 失败: {e}")),
        },
        Err(e) => crate::process::append_log(&format!("[music] {label} 调用失败: {e}")),
    }
}

/// 订阅 manager 层的两个事件（**只做一次**，与具体会话无关）。
///
/// ⛔ 闭包不能捕获 `mgr` 的引用（要 `'static`）⇒ 这里不捕获任何东西，
///    只发唤醒信号；真正的重取在 worker 循环里做。
fn subscribe_manager(mgr: &GlobalSystemMediaTransportControlsSessionManager) {
    use windows::Foundation::TypedEventHandler;
    use windows::Media::Control::{CurrentSessionChangedEventArgs, SessionsChangedEventArgs};

    let ok = mgr.SessionsChanged(&TypedEventHandler::<
        GlobalSystemMediaTransportControlsSessionManager,
        SessionsChangedEventArgs,
    >::new(|_, _| {
        SESSION_EPOCH.fetch_add(1, Ordering::AcqRel);
        request_refresh();
        Ok(())
    }));
    match ok {
        Ok(_) => crate::process::append_verbose_log("[music] 已订阅 SessionsChanged"),
        Err(e) => crate::process::append_log(&format!("[music] 订阅 SessionsChanged 失败: {e}")),
    }

    let ok = mgr.CurrentSessionChanged(&TypedEventHandler::<
        GlobalSystemMediaTransportControlsSessionManager,
        CurrentSessionChangedEventArgs,
    >::new(|_, _| {
        SESSION_EPOCH.fetch_add(1, Ordering::AcqRel);
        request_refresh();
        Ok(())
    }));
    match ok {
        Ok(_) => crate::process::append_verbose_log("[music] 已订阅 CurrentSessionChanged"),
        Err(e) => {
            crate::process::append_log(&format!("[music] 订阅 CurrentSessionChanged 失败: {e}"))
        }
    }
}
