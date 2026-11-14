//! A2DP Sink（蓝牙音频接收）：把本机当作蓝牙音箱，接收手机等音频源设备的播放。
//!
//! ## 平台事实（实现前必读，全部有出处）
//!
//! 1. Win10 2004（build 19041）起系统原生支持 A2DP Sink，管理入口是
//!    `Windows.Media.Audio.AudioPlaybackConnection`（WinRT）。依据：
//!    AudioPlaybackConnector（ysc3839）/ BlueAudioSink（liubaiqing）两个开源项目
//!    均走此 API 并经真机验证。
//! 2. **一个进程同一时刻只能存在一条连接对象**：其 `Start` 会注册模块级
//!    TraceLogging provider，二次注册直接 `__fastfail`（AudioPlaybackConnector
//!    实测）。因此本模块为**单连接模型**——同一时刻至多持有一条连接，
//!    换设备先释放旧对象。进程内「释放→重建」序列经 BlueAudioSink 生产验证安全。
//! 3. ⛔ **连接对象复用纪律**（BlueAudioSink 实测结论，违反会形成 2~8s 一次的
//!    「拆-建」循环，持续搞乱手机侧 A2DP 链路导致每次都立刻失败）：
//!    - OpenAsync 返回非 Success、稳定观察未达 Opened ⇒ **复用**原对象退避重试；
//!    - OpenAsync 超时被 Cancel、武装（StartAsync）失败、StateChanged=Closed
//!      ⇒ 对象已污染/终结，重试必须重建。
//! 4. 控制层成功（OpenAsync=Success）≠ 链路层就绪：蓝牙被系统中断后可能
//!    「连上了却没声音」。链路层判活 = 转发会话峰值电表，唯一实现点
//!    [`crate::audio::relay_session_flowing`]（A2DP 捕获端点连推流中也不进
//!    ACTIVE 枚举，SetupDi 接口类方案已被真机证伪，见 step 注释）。
//! 5. 武装（StartAsync）只在对象首次创建时执行一次，重试不重复武装——
//!    避免与手机端协商互相打断（BlueAudioSink 实测结论）。
//!
//! ## 线程模型
//!
//! 单控制器线程（MTA）拥有连接对象与全部状态机变量；Tauri 命令只经
//! `A2DP_TX` 投递消息（同步命令零阻塞）；UI 通过 `SNAPSHOT` 快照 +
//! `a2dp-changed` 事件取状态。WinRT `StateChanged` 回调在任意线程触发 ⇒
//! 只投递消息，不碰共享状态（对齐 taskbar_music「回调只单向投递」纪律）。
//! 连接流程的 arm/open 等待以 100ms 步进轮询辅助结果，间隙查打断——
//! `Disconnect` 即时生效，不等流程自然走完（「正在连接…」窗口最长 ~25s，
//! 断开按钮不能在这窗口里是死的）。

#![cfg(target_os = "windows")]

use crate::state::lock_unpoisoned;
use crate::{standard_log, verbose_log};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};
use tauri::Emitter;
use windows::Devices::Enumeration::DeviceInformation;
use windows::Foundation::Metadata::ApiInformation;
use windows::Media::Audio::{
    AudioPlaybackConnection, AudioPlaybackConnectionOpenResultStatus, AudioPlaybackConnectionState,
};
use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};
use windows_core::HSTRING;

// ── 可调参数（全部为外部项目实测调参值，勿凭直觉改）─────────────────────

/// 武装（StartAsync）超时。依据：BlueAudioSink `Policy.ArmTimeout`。
const ARM_TIMEOUT: Duration = Duration::from_secs(8);
/// OpenAsync 超时。依据：BlueAudioSink `Policy.OpenTimeout`。
const OPEN_TIMEOUT: Duration = Duration::from_secs(15);
/// OpenAsync 成功后的稳定观察窗口，把「稍慢」与「秒断」区分开。
/// 依据：BlueAudioSink `OpenStabilityDelay`(400ms) + 1.2s 轮询余量。
const STABILITY_WAIT: Duration = Duration::from_millis(1600);
/// 单次连接流程内最多自动重试次数，超过转待机、把发起权交还手机。
/// 依据：BlueAudioSink `MaxRetries = 3`。
const MAX_AUTO_RETRIES: u32 = 3;
/// 控制器 tick。依据：BlueAudioSink 采样间隔 1s；逐 tick 的判活探测为
/// 全端点会话枚举（实测 13~15ms 量级，见 audio::relay_session_flowing），
/// 逐秒成本可忽略。
const TICK: Duration = Duration::from_secs(1);
/// 武装成功后长时间未见流 ⇒ 提示检查手机侧。
/// 依据：BlueAudioSink `LinkWaitHintDelay = 15s`。
const LINK_HINT_AFTER: Duration = Duration::from_secs(15);
/// 释放连接对象的限时等待。依据：BlueAudioSink `ReleaseTimeout = 5s`
/// （Dispose 可能卡，不能无限等）。
const RELEASE_TIMEOUT: Duration = Duration::from_secs(5);

// ── 对外数据结构 ────────────────────────────────────────────────────────

/// 连接状态机对外状态（快照下发前端，serde 名即 JS 判据）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum A2dpState {
    /// 未连接（初始/用户断开后）
    #[default]
    Idle,
    /// 连接流程进行中
    Connecting,
    /// 失败退避中，到期自动重试
    Backoff,
    /// 控制层已打开，等待手机播放音频
    Armed,
    /// 转发会话见流（音频链路已建立；见流即闩锁、drop_conn 才清，见 step 注释）
    LinkReady,
    /// 系统不支持 A2DP Sink（防御路径：init 探测不支持时仍到达的连接请求落此态）
    Unsupported,
}

/// 状态快照：`a2dp_get_status` 命令与 `a2dp-changed` 事件的唯一载荷。
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct A2dpStatus {
    /// 系统是否支持 A2DP Sink（Win10 2004+ 才有 AudioPlaybackConnection 类型）
    pub supported: bool,
    pub state: A2dpState,
    /// 当前目标设备（A2DP 选择器空间 id）
    pub device_id: String,
    pub device_name: String,
    /// 当前目标设备的 12 位十六进制小写 MAC（id 提取失败为空串）
    pub device_mac: String,
    /// 一句话状态（直接展示）
    pub detail: String,
    /// 可操作提示（可为空）
    pub hint: String,
    /// 当前是第几次自动重试（非退避态为 0）
    pub attempt: u32,
}

/// A2DP 可接收设备（`a2dp_get_devices` 的条目）。
#[derive(Debug, Clone, serde::Serialize)]
pub struct A2dpDevice {
    /// A2DP 选择器空间 id（`AudioPlaybackConnection::TryCreateFromId` 只认它）
    pub id: String,
    pub name: String,
    /// 12 位十六进制小写 MAC（取自 id 的 `_C00000000` 前缀）；提取失败为空串
    pub mac: String,
}

// ── 模块级共享状态 ──────────────────────────────────────────────────────

/// 命令通道（同步命令 → 控制器线程的唯一入口）
static A2DP_TX: OnceLock<Sender<Msg>> = OnceLock::new();
/// 事件推送句柄（main setup 注入，对齐 bluetooth.rs 的 BT_EVENT_HANDLE 模式）
static A2DP_HANDLE: OnceLock<tauri::AppHandle> = OnceLock::new();
/// 系统支持性（控制器启动时经 ApiInformation 探测一次）
static SUPPORTED: OnceLock<bool> = OnceLock::new();
/// 状态快照（`a2dp-changed` 事件与 `a2dp_get_status` 命令的唯一读出口）
static SNAPSHOT: Mutex<A2dpStatus> = Mutex::new(A2dpStatus {
    supported: false,
    state: A2dpState::Idle,
    device_id: String::new(),
    device_name: String::new(),
    device_mac: String::new(),
    detail: String::new(),
    hint: String::new(),
    attempt: 0,
});

/// 控制器命令。`LinkClosed` 是 WinRT 回调侧唯一会投递的消息。
/// `Connect.name` = 前端现成显示名（None ⇒ 控制器回查枚举，仅启动恢复路径）。
enum Msg {
    Connect { id: String, name: Option<String> },
    Disconnect,
    LinkClosed,
    Shutdown,
}

// ── 快照与事件 ──────────────────────────────────────────────────────────

fn publish() {
    let snap = lock_unpoisoned(&SNAPSHOT).clone();
    if let Some(app) = A2DP_HANDLE.get() {
        let _ = app.emit("a2dp-changed", snap);
    }
}

/// 更新快照并推送事件（持锁区只做纯内存写，emit 在锁外）。
fn set_snapshot(mutate: impl FnOnce(&mut A2dpStatus)) {
    mutate(&mut lock_unpoisoned(&SNAPSHOT));
    publish();
}

/// 语义化设置状态行；内容与快照完全一致时**不**重复推送（step 每秒采样，
/// 不去重会让弹窗/设置页每秒无意义重渲染一次）。
/// ⛔ 判重必须挡在 emit **之前**：`set_snapshot` 是「写后必推」语义，把判重
/// 塞进 mutate 闭包只会跳过字段写入、拦不住 publish。
fn set_detail(state: A2dpState, detail: &str, hint: &str) {
    {
        let mut s = lock_unpoisoned(&SNAPSHOT);
        if s.state == state && s.detail == detail && s.hint == hint {
            return;
        }
        s.state = state;
        s.detail = detail.to_string();
        s.hint = hint.to_string();
    }
    publish();
}

fn supported() -> bool {
    SUPPORTED.get().copied().unwrap_or(false)
}

// ── 控制器状态机 ────────────────────────────────────────────────────────

/// 控制器线程本有的状态机变量（⛔ 跨线程共享一律经 `SNAPSHOT`，不许加第二份）。
#[derive(Default)]
struct Machine {
    /// 用户意图：连接中/保持连接。false 时 fail/断链一律落 Idle 不再重试
    want_connected: bool,
    /// 当前连接对象（单连接模型：同一时刻至多一条）
    conn: Option<AudioPlaybackConnection>,
    /// 当前目标设备 (id, name)。conn 复用与否的判据之一
    armed_device: Option<(String, String)>,
    retry_count: u32,
    retry_at: Option<Instant>,
    armed_at: Option<Instant>,
    link_since: Option<Instant>,
    link_hinted: bool,
    /// 连接对象是否可复用（false ⇒ 重试时重建；判据见模块头注释第 3 条）
    reuse_conn: bool,
    /// 本条链路上**见过转发会话流量**（电表判活，见 step）。闩锁语义：
    /// 见流即置真、`drop_conn` 才清 —— 换曲间隙 / 静音段电表归零不得闪断。
    flow_seen: bool,
    /// 连接流程内打断检查抽走、暂不能立即处理的消息（主循环按原序回灌）
    pending_msgs: Vec<Msg>,
}

impl Machine {
    /// 释放并清空连接对象。
    fn drop_conn(&mut self, reason: &str) {
        if let Some(conn) = self.conn.take() {
            release_conn(conn, reason);
        }
        self.reuse_conn = false;
        // 链路没了，流量闩锁必须清：否则旧链路的 flow_seen 会把重连后的
        // 等待期误报成 LinkReady（唯一清点：所有断链路径都经此收口）
        self.flow_seen = false;
    }
}

/// 释放连接对象。对象析构可能阻塞（BlueAudioSink 实测 Dispose 会卡）⇒
/// 移交一次性线程并限时等待；超时后控制器继续推进，析构由该线程自行收尾。
fn release_conn(conn: AudioPlaybackConnection, reason: &str) {
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        drop(conn);
        let _ = tx.send(());
    });
    if rx.recv_timeout(RELEASE_TIMEOUT).is_err() {
        standard_log!("[a2dp] 连接对象释放超时（{}）", reason);
    }
}

/// 等待类辅助的轮询步进：打断请求的生效延迟上限 ≈ 一步（100ms），
/// 相对 arm 8s / open 15s 的等待窗口可忽略。
const WAIT_POLL: Duration = Duration::from_millis(100);

/// 等待类结果。`Aborted` = 等待间隙查到用户断开，流程必须静默返回
/// （状态已由 [`abort_if_disconnected`] 落 Idle，调用方不得再走 [`fail`]）。
enum OpOutcome {
    Done,
    Timeout,
    Aborted,
    Failed(String),
}

/// 等待武装动作完成：以 [`WAIT_POLL`] 步进轮询 helper 结果，间隙查打断——
/// ⛔ 用户断开必须即时生效，不能等 arm 自然超时（最长 8s 内 UI 已确认断开、
/// 后端却还在武装）。超时必须 Cancel：不取消的话后台 join 线程会一直
/// 持有操作对象，且下一次 Start 会与系统侧未完成的协商相撞。
/// ⚠️ WinRT 异步类型在 `windows_future` crate，`windows` crate 不 re-export
/// （taskbar_music 同款口径）。
fn wait_arm(
    m: &mut Machine,
    rx: &Receiver<Msg>,
    action: windows_future::IAsyncAction,
    timeout: Duration,
) -> OpOutcome {
    let (tx, rx_op) = channel();
    let cloned = action.clone();
    std::thread::spawn(move || {
        let _ = tx.send(cloned.join());
    });
    let deadline = Instant::now() + timeout;
    loop {
        match rx_op.recv_timeout(WAIT_POLL) {
            Ok(res) => {
                return match res {
                    Ok(()) => OpOutcome::Done,
                    Err(e) => OpOutcome::Failed(format!("{}", e)),
                };
            }
            Err(RecvTimeoutError::Disconnected) => {
                return OpOutcome::Failed("等待线程异常退出".to_string());
            }
            Err(RecvTimeoutError::Timeout) => {
                if abort_if_disconnected(rx, m) {
                    let _ = action.Cancel();
                    return OpOutcome::Aborted;
                }
                if Instant::now() >= deadline {
                    let _ = action.Cancel();
                    return OpOutcome::Timeout;
                }
            }
        }
    }
}

fn wait_open(
    m: &mut Machine,
    rx: &Receiver<Msg>,
    op: windows_future::IAsyncOperation<windows::Media::Audio::AudioPlaybackConnectionOpenResult>,
    timeout: Duration,
) -> (
    OpOutcome,
    Option<windows::Media::Audio::AudioPlaybackConnectionOpenResult>,
) {
    let (tx, rx_op) = channel();
    let cloned = op.clone();
    std::thread::spawn(move || {
        let _ = tx.send(cloned.join());
    });
    let deadline = Instant::now() + timeout;
    loop {
        match rx_op.recv_timeout(WAIT_POLL) {
            Ok(res) => {
                return match res {
                    Ok(result) => (OpOutcome::Done, Some(result)),
                    Err(e) => (OpOutcome::Failed(format!("{}", e)), None),
                };
            }
            Err(RecvTimeoutError::Disconnected) => {
                return (OpOutcome::Failed("等待线程异常退出".to_string()), None);
            }
            Err(RecvTimeoutError::Timeout) => {
                if abort_if_disconnected(rx, m) {
                    let _ = op.Cancel();
                    return (OpOutcome::Aborted, None);
                }
                if Instant::now() >= deadline {
                    let _ = op.Cancel();
                    return (OpOutcome::Timeout, None);
                }
            }
        }
    }
}

/// 稳定观察：轮询 State==Opened（[`WAIT_POLL`] 步进，间隙查打断）。
/// State 查询出错时**放行**——验证是防御性的，不能因验证机制本身故障让
/// 所有连接失败（对齐 AudioPlaybackConnector 端点验证口径）。
/// 返回 None = 已打断（状态落 Idle），Some(true)=稳定打开，Some(false)=未稳定。
fn wait_stable_opened(
    m: &mut Machine,
    rx: &Receiver<Msg>,
    conn: &AudioPlaybackConnection,
) -> Option<bool> {
    let deadline = Instant::now() + STABILITY_WAIT;
    loop {
        match conn.State() {
            Ok(s) => {
                if s == AudioPlaybackConnectionState::Opened {
                    return Some(true);
                }
            }
            Err(_) => return Some(true),
        }
        if Instant::now() >= deadline {
            return Some(false);
        }
        if abort_if_disconnected(rx, m) {
            return None;
        }
        std::thread::sleep(WAIT_POLL);
    }
}

/// 第 retry_index 次（0 基）重试的退避时长：[2,4,8,16,30]s，越界封顶。
/// 依据：BlueAudioSink `Policy.BackoffSteps`（实测调参值）。
fn backoff_delay(retry_index: usize) -> Duration {
    const STEPS_SECS: [u64; 5] = [2, 4, 8, 16, 30];
    Duration::from_secs(STEPS_SECS[retry_index.min(STEPS_SECS.len() - 1)])
}

/// 把失败码翻成「用户下一步该做什么」。口径取自 BlueAudioSink
/// （实测：OpenStatus 类失败数毫秒内本地返回 ⇒ 电脑侧正常，问题在手机未接受连接）。
fn build_hint(code: &str) -> String {
    match code {
        "OpenStatus" => "电脑侧正常，是手机没有接受音频连接。请在手机上开关蓝牙，或在已配对列表中点一下本电脑重新连接".to_string(),
        "OpenTimeout" => "手机可能未响应。请把手机靠近电脑、唤醒屏幕后重试".to_string(),
        "ArmTimeout" => "蓝牙栈无响应。可尝试重新开关电脑蓝牙后重试".to_string(),
        // TryCreateFromId 返回空无法区分「设备真不支持」与「蓝牙链路未激活」（手机不在
        // 范围/已关蓝牙同样返回空），提示按两种可能都成立来写
        "NotSupported" => "设备未连接或未提供音频接收服务：请确认手机蓝牙已开启且在附近，并与本机保持配对".to_string(),
        "NotOpened" => "音频通道未建立。请确认手机正在播放，并在手机上把音频输出指向本电脑".to_string(),
        "RemoteClosed" => "连接已断开，可点击「连接」重新接收".to_string(),
        _ => format!("连接失败（{}）。请确认手机在附近且蓝牙已开启", code),
    }
}

fn open_status_text(s: AudioPlaybackConnectionOpenResultStatus) -> &'static str {
    if s == AudioPlaybackConnectionOpenResultStatus::RequestTimedOut {
        "手机未响应（RequestTimedOut）"
    } else if s == AudioPlaybackConnectionOpenResultStatus::DeniedBySystem {
        "被系统拒绝（DeniedBySystem）"
    } else {
        "未知失败"
    }
}

/// 统一失败处理：按可重试性分类、推进退避或转待机。
///
/// `keep_conn` = true 表示对象未被污染、保留复用（判据见模块头注释第 3 条）；
/// `fatal` = true 表示重试无意义（设备不支持），直接转待机。
fn fail(m: &mut Machine, code: &str, msg: &str, keep_conn: bool, fatal: bool) {
    if !keep_conn {
        m.drop_conn(code);
    }
    m.retry_count += 1;
    verbose_log!(
        "[a2dp] 连接失败 code={} 第{}次：{}",
        code,
        m.retry_count,
        msg
    );

    let auto_retry = m.want_connected
        && !fatal
        && crate::config::with_config(|c| c.a2dp_reconnect_on_drop)
        && m.retry_count <= MAX_AUTO_RETRIES;
    if auto_retry {
        m.retry_at = Some(Instant::now() + backoff_delay((m.retry_count - 1) as usize));
        set_snapshot(|s| {
            s.state = A2dpState::Backoff;
            s.detail = format!("连接失败，正在重试（第 {} 次）", m.retry_count);
            s.hint = msg.to_string();
            s.attempt = m.retry_count;
        });
        return;
    }

    // 重试耗尽 / 用户已取消意图 / 关了自动重试 / 致命失败：转待机，
    // 把发起权交还手机与 Windows（⛔ 不死循环重连，避免留下僵死链路）
    if keep_conn {
        m.drop_conn(code);
    }
    // 用户已主动断开（want_connected=false）时失败不吓唬用户「已停止重试」
    let was_wanting = m.want_connected;
    m.want_connected = false;
    m.retry_at = None;
    m.armed_at = None;
    m.link_since = None;
    let detail = if was_wanting {
        "连接失败，已停止重试"
    } else {
        "未连接"
    };
    set_snapshot(|s| {
        s.state = A2dpState::Idle;
        s.detail = detail.to_string();
        s.hint = build_hint(code);
        s.attempt = 0;
    });
    standard_log!(
        "[a2dp] 连接失败转待机 code={} attempts={}",
        code,
        m.retry_count
    );
}

// ── 连接流程 ────────────────────────────────────────────────────────────

/// 连接流程等待间隙的打断检查，返回「是否已打断」。打断路径经 on_disconnect
/// 落 Idle，调用方静默返回（状态已落，不得再走 [`fail`]）。
fn abort_if_disconnected(rx: &Receiver<Msg>, m: &mut Machine) -> bool {
    // 抽干通道——⛔ 只有 `Disconnect` 立即生效（落 Idle 并拆连接）；其余消息
    // 暂存 `pending_msgs` 由主循环按原序回灌，不能丢（连接中点另一台设备 =
    // 换目标，吞掉就是吞请求）。已打断时 pending 里的积压请求一并作废：
    // 用户最后的动作是断开，回灌更早的 Connect 会覆盖最新意图；打断后的
    // 新请求留在通道，主循环正常处理。
    let mut requested = false;
    while let Ok(msg) = rx.try_recv() {
        match msg {
            Msg::Disconnect => requested = true,
            other => m.pending_msgs.push(other),
        }
    }
    if requested {
        m.pending_msgs.clear();
        on_disconnect(m);
        standard_log!("[a2dp] 连接流程被用户断开打断");
    }
    requested
}

/// 完整连接流程：获取或复用对象 → 武装（仅首次）→ 打开 → 稳定观察。
/// 失败路径全部经 [`fail`] 收口，成功路径在此落 `Armed` 并持久化 last_device；
/// 各等待环节可被用户断开打断（[`abort_if_disconnected`]），打断路径静默返回。
fn connect_internal(m: &mut Machine, rx: &Receiver<Msg>) {
    let Some((id, name)) = m.armed_device.clone() else {
        return;
    };
    set_snapshot(|s| {
        s.state = A2dpState::Connecting;
        s.detail = "正在连接…".to_string();
        s.hint = String::new();
        s.attempt = 0;
    });

    // 1) 获取或复用连接对象：进程生命周期内每个目标只建一个（复用纪律）
    let mut fresh = false;
    let conn = if m.reuse_conn {
        m.conn.clone()
    } else {
        m.conn = None;
        match AudioPlaybackConnection::TryCreateFromId(&HSTRING::from(id.as_str())) {
            Ok(c) => {
                fresh = true;
                Some(c)
            }
            Err(e) => {
                fail(
                    m,
                    "NotSupported",
                    &format!("设备不支持蓝牙音频接收（{}）", e),
                    false,
                    true,
                );
                return;
            }
        }
    };
    let Some(conn) = conn else {
        fail(m, "NotSupported", "设备不支持蓝牙音频接收", false, true);
        return;
    };
    m.conn = Some(conn.clone());
    m.reuse_conn = true;

    // StateChanged → 只投递消息（回调在任意线程触发，不碰共享状态）。
    // ⛔ 仅 fresh 时挂一次：复用对象已带订阅，重复挂会导致 LinkClosed 多投。
    if fresh {
        if let Some(tx) = A2DP_TX.get().cloned() {
            let handler = windows::Foundation::TypedEventHandler::<
                AudioPlaybackConnection,
                windows_core::IInspectable,
            >::new(move |sender, _args| {
                // sender 经 windows_core::Ref 可空包装；None 视为非 Closed 不投递
                let closed = sender
                    .as_ref()
                    .is_some_and(|s| s.State() == Ok(AudioPlaybackConnectionState::Closed));
                if closed {
                    let _ = tx.send(Msg::LinkClosed);
                }
                Ok(())
            });
            if let Err(e) = conn.StateChanged(&handler) {
                verbose_log!("[a2dp] StateChanged 订阅失败：{}", e);
            }
        }
    }

    // ⛔ 先查打断再武装：武装一旦开始，Cancel 只能在 wait 循环内生效，
    // 这里有 Disconnect 就别开 Start（免得武装备好后马上拆）
    if abort_if_disconnected(rx, m) {
        return;
    }

    // 2) 武装：⛔ 只在首次创建时 Start，重试不重复武装（模块头注释第 5 条）
    if fresh {
        match conn.StartAsync() {
            Err(e) => {
                fail(
                    m,
                    "ArmException",
                    &format!("武装接收通道失败（{}）", e),
                    false,
                    false,
                );
                return;
            }
            Ok(action) => match wait_arm(m, rx, action, ARM_TIMEOUT) {
                OpOutcome::Done => {}
                OpOutcome::Aborted => return,
                OpOutcome::Timeout => {
                    // 已 Cancel ⇒ 对象污染，必须重建
                    fail(m, "ArmTimeout", "武装接收通道超时", false, false);
                    return;
                }
                OpOutcome::Failed(e) => {
                    fail(
                        m,
                        "ArmException",
                        &format!("武装接收通道失败（{}）", e),
                        false,
                        false,
                    );
                    return;
                }
            },
        }
    }

    // 3) 打开连接：失败按可重试性分类
    match conn.OpenAsync() {
        Err(e) => {
            fail(
                m,
                "OpenException",
                &format!("打开连接失败（{}）", e),
                false,
                false,
            );
            return;
        }
        Ok(op) => {
            let (outcome, result) = wait_open(m, rx, op, OPEN_TIMEOUT);
            match outcome {
                OpOutcome::Aborted => return,
                OpOutcome::Timeout => {
                    // ⛔ 被 Cancel 的 OpenAsync 会让连接对象进入不可恢复状态，
                    // 复用必然一直失败 ⇒ 丢弃，让下次重试重建并重新武装
                    fail(
                        m,
                        "OpenTimeout",
                        "打开连接超时（手机可能未响应）",
                        false,
                        false,
                    );
                    return;
                }
                OpOutcome::Failed(e) => {
                    fail(
                        m,
                        "OpenException",
                        &format!("打开连接失败（{}）", e),
                        false,
                        false,
                    );
                    return;
                }
                OpOutcome::Done => {
                    let Some(result) = result else {
                        fail(m, "OpenException", "打开连接结果缺失", false, false);
                        return;
                    };
                    match result.Status() {
                        Ok(AudioPlaybackConnectionOpenResultStatus::Success) => {}
                        Ok(other) => {
                            // ⭐ 这类失败不是超时，对象未被 Cancel 污染 ⇒ 保留复用
                            fail(m, "OpenStatus", open_status_text(other), true, false);
                            return;
                        }
                        Err(e) => {
                            fail(
                                m,
                                "OpenException",
                                &format!("读取打开结果失败（{}）", e),
                                false,
                                false,
                            );
                            return;
                        }
                    }
                }
            }
        }
    }

    // 4) 稳定观察：状态迁移到位才算成功（过滤「秒断」）
    match wait_stable_opened(m, rx, &conn) {
        None => return, // 已打断落 Idle
        Some(false) => {
            fail(m, "NotOpened", "连接未能保持打开状态", true, false);
            return;
        }
        Some(true) => {}
    }

    // 5) 成功：落 Armed，持久化 last_device（供「启动时恢复连接」）
    m.retry_count = 0;
    m.retry_at = None;
    m.armed_at = Some(Instant::now());
    m.link_since = None;
    m.link_hinted = false;
    set_snapshot(|s| {
        s.state = A2dpState::Armed;
        s.detail = "已就绪，等待手机播放".to_string();
        s.hint = "请在手机上播放音乐以开始传输".to_string();
        s.attempt = 0;
    });
    crate::config::with_config_mut(|c| c.a2dp_last_device = id.clone());
    standard_log!("[a2dp] 已连接 {}，等待手机播放", name);
}

/// 消息处理：连接请求。换设备先释放旧对象（单连接模型）。
/// `name_hint` = 前端现成显示名。
fn on_connect(m: &mut Machine, rx: &Receiver<Msg>, id: String, name_hint: Option<String>) {
    if !supported() {
        set_detail(
            A2dpState::Unsupported,
            "此系统版本不支持蓝牙音频接收（需 Win10 2004+）",
            "",
        );
        return;
    }
    // ⛔ 连接路径不得回查枚举取名（控制器线程会多一次整表 COM 枚举）：
    // 名字用前端现成 hint，仅启动恢复路径无 hint 时才回查。
    let name = name_hint
        .filter(|s| !s.is_empty())
        .or_else(|| device_name_by_id(&id))
        .unwrap_or_else(|| "设备".to_string());
    if let Some((armed_id, _)) = &m.armed_device {
        if *armed_id != id {
            m.drop_conn("switch device");
            m.armed_device = None;
        }
    }
    m.armed_device = Some((id.clone(), name.clone()));
    m.want_connected = true;
    m.retry_count = 0;
    m.retry_at = None;
    let mac = a2dp_id_mac(&id).unwrap_or_default();
    set_snapshot(|s| {
        s.device_id = id;
        s.device_name = name.clone();
        s.device_mac = mac;
    });
    connect_internal(m, rx);
}

/// 消息处理：用户主动断开。
fn on_disconnect(m: &mut Machine) {
    m.want_connected = false;
    m.retry_at = None;
    m.retry_count = 0;
    m.armed_at = None;
    m.link_since = None;
    m.drop_conn("user disconnect");
    set_detail(A2dpState::Idle, "未连接", "");
    verbose_log!("[a2dp] 用户断开");
}

/// 消息处理：StateChanged=Closed（手机走远、系统回收链路、或用户断开的余波）。
/// 连接对象在 Closed 后按终结处理（AudioPlaybackConnector 对 Closed 只走退出、
/// 不复用），重建对象再走退避。
fn on_link_closed(m: &mut Machine) {
    if m.conn.is_none() && !m.want_connected {
        // 用户断开路径的余波：已落 Idle，无事可做（防重复事件刷状态）
        return;
    }
    m.drop_conn("remote closed");
    fail(m, "RemoteClosed", "连接被手机或系统断开", false, false);
}

/// 每 tick 执行：退避到期重试优先，其次链路层采样推进 Armed/LinkReady。
fn step(m: &mut Machine, rx: &Receiver<Msg>) {
    // ⛔ 退避检查必须在 `conn.is_none()` 判空**之前**：失败路径已把 conn 清空，
    // 判空在前会让重试永不执行、卡死在失败态（BlueAudioSink 同款教训）。
    if !m.want_connected {
        return;
    }
    if let Some(at) = m.retry_at {
        if Instant::now() >= at {
            m.retry_at = None;
            verbose_log!("[a2dp] 退避到期，重试连接");
            connect_internal(m, rx);
        }
        return;
    }
    if m.conn.is_none() {
        return;
    }

    // 链路层判活：转发会话峰值电表（唯一流量信号，判据见
    // audio::relay_session_flowing）按 tick 逐秒执行。⭐ 见流即闩锁（flow_seen），
    // drop_conn 才清 —— 换曲间隙 / 静音段电表归零不得闪断。
    let Some((_, armed_name)) = m.armed_device.clone() else {
        return;
    };
    if !m.flow_seen {
        m.flow_seen = crate::audio::relay_session_flowing(&armed_name);
        if m.flow_seen {
            standard_log!("[a2dp] 转发会话见流（音频链路建立）");
        }
    }
    let now_state = lock_unpoisoned(&SNAPSHOT).state;

    if m.flow_seen {
        if m.link_since.is_none() {
            m.link_since = Some(Instant::now());
            m.link_hinted = false;
        }
        set_detail(A2dpState::LinkReady, "音频链路已建立", "");
    } else {
        if m.link_since.is_some() {
            m.link_since = None;
            verbose_log!("[a2dp] 转发会话无流量");
        }
        // 武装成功后长时间未见流 ⇒ 提示检查手机侧（一次）
        let waiting_long = m
            .armed_at
            .map(|at| Instant::now().duration_since(at) > LINK_HINT_AFTER)
            .unwrap_or(false);
        if waiting_long && !m.link_hinted && now_state == A2dpState::Armed {
            m.link_hinted = true;
        }
        // ⛔ 提示一经触发就持续保留（link_hinted 只在下次武装复位）：判重真正
        // 生效后，else 支路若回空 hint，下一 tick 就会把上一 tick 的提示清掉
        let hint = if m.link_hinted && now_state == A2dpState::Armed {
            "若手机正在播放却听不到声音，请检查手机蓝牙输出是否指向本电脑"
        } else {
            ""
        };
        set_detail(A2dpState::Armed, "等待手机播放", hint);
    }
}

/// 单条消息分发。返回 true = 请求退出控制器。
fn dispatch(m: &mut Machine, rx: &Receiver<Msg>, msg: Msg) -> bool {
    match msg {
        Msg::Connect { id, name } => {
            on_connect(m, rx, id, name);
            false
        }
        Msg::Disconnect => {
            on_disconnect(m);
            false
        }
        Msg::LinkClosed => {
            on_link_closed(m);
            false
        }
        Msg::Shutdown => true,
    }
}

fn controller_main(rx: Receiver<Msg>) {
    ensure_mta();
    // 支持性已在 init() 同步探测（判据见 init 注释），此处只读
    let supported = supported();
    // 初始 idle 落「未连接」（空 detail 无意义）
    set_detail(A2dpState::Idle, "未连接", "");
    standard_log!("[a2dp] 控制器启动 supported={}", supported);

    let mut m = Machine::default();

    // 启动恢复上次连接（配置开关 + last_device 双判据）；无前端 hint 可用 ⇒ 回查枚举
    if supported && crate::config::with_config(|c| c.a2dp_reconnect_on_start) {
        let last = crate::config::with_config(|c| c.a2dp_last_device.clone());
        if !last.is_empty() {
            standard_log!("[a2dp] 启动恢复上次连接");
            on_connect(&mut m, &rx, last, None);
        }
    }

    let mut shutdown = false;
    while !shutdown {
        while let Ok(msg) = rx.try_recv() {
            if dispatch(&mut m, &rx, msg) {
                shutdown = true;
                break;
            }
        }
        if shutdown {
            break;
        }
        // 连接流程内打断检查抽走的消息按原序回灌（打断时已被清空，见
        // abort_if_disconnected）；回灌期间新抽走的由下一轮继续
        let pending = std::mem::take(&mut m.pending_msgs);
        for msg in pending {
            if dispatch(&mut m, &rx, msg) {
                shutdown = true;
                break;
            }
        }
        if shutdown {
            break;
        }
        step(&mut m, &rx);
        // 真线程循环，允许 sleep（AGENTS「异步上下文禁止 sleep」不适用）
        std::thread::sleep(TICK);
    }

    m.drop_conn("shutdown");
    set_detail(A2dpState::Idle, "未连接", "");
    standard_log!("[a2dp] 控制器退出");
}

// ── 设备枚举与端点验证 ──────────────────────────────────────────────────

/// 调用线程初始化 COM MTA（S_FALSE=已初始化、RPC_E_CHANGED_MODE 均无害，忽略）。
/// 依据：taskbar_music——WinRT 异步操作需 MTA 线程才能无消息泵阻塞等待。
fn ensure_mta() {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }
}

/// 枚举当前系统内「可被本机接收音频」的设备（手机等音频源）。
/// 命令层经 run_blocking 调用（枚举 + join 阻塞数百毫秒，禁入主线程）。
pub fn enumerate_devices() -> Result<Vec<A2dpDevice>, String> {
    // ⛔ 不得在调用线程直接 ensure_mta：命令层经 run_blocking（tokio
    //   spawn_blocking 共享池）调用，MTA 会**永久污染**池线程，此后复用该
    //   线程的 STA 命令全部 RPC_E_CHANGED_MODE（audio.rs「进程内统一 STA」
    //   契约被破坏，STA-only 组件在该线程上必败）。故收进一次性专用线程，
    //   线程随 join 结束、污染不外溢（taskbar_music 的 MTA 同样限定专用线程）。
    std::thread::spawn(enumerate_devices_mta)
        .join()
        // ⚠️ panic→Err 映射仅 dev/test 可达：release 为 panic="abort"，
        //    工作线程 panic 直接终止进程（与既有全局语义一致，非回归）
        .unwrap_or_else(|e| Err(format!("枚举线程异常退出：{e:?}")))
}

/// `enumerate_devices` 本体：**只能在专用线程调用**（WinRT 异步操作需 MTA
/// 线程才能无消息泵阻塞等待，依据见 `ensure_mta` 注释）。
fn enumerate_devices_mta() -> Result<Vec<A2dpDevice>, String> {
    ensure_mta();
    let selector = AudioPlaybackConnection::GetDeviceSelector()
        .map_err(|e| format!("获取选择器失败：{}", e))?;
    let coll = DeviceInformation::FindAllAsyncAqsFilter(&selector)
        .map_err(|e| format!("设备枚举请求失败：{}", e))?
        .join()
        .map_err(|e| format!("设备枚举失败：{}", e))?;
    let size = coll
        .Size()
        .map_err(|e| format!("读取设备数量失败：{}", e))?;
    let mut out = Vec::with_capacity(size as usize);
    for i in 0..size {
        let d = coll.GetAt(i).map_err(|e| format!("读取设备失败：{}", e))?;
        let id = d
            .Id()
            .map_err(|e| format!("读取设备 id 失败：{}", e))?
            .to_string();
        let name = d
            .Name()
            .map_err(|e| format!("读取设备名失败：{}", e))?
            .to_string();
        let mac = a2dp_id_mac(&id).unwrap_or_default();
        out.push(A2dpDevice { id, name, mac });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name).then(a.id.cmp(&b.id)));
    Ok(out)
}

fn device_name_by_id(id: &str) -> Option<String> {
    enumerate_devices()
        .ok()?
        .iter()
        .find(|d| d.id == id)
        .map(|d| d.name.clone())
}

/// 从 A2DP 选择器返回的设备 id 提取 12 位十六进制 MAC（小写）。
/// 两种 BTHENUM 形态按序尝试：
/// ① `_C00000000` 之前 12 字符（依据：AudioPlaybackConnector 的提取口径）；
/// ② `DEV_` 之后 12 位 hex——覆盖 `DEV_<MAC>.<hash>_C00000000` 形态，此时
///    形态①夹着 `.` 与 hash 段、必然校验失败，不做兜底就会整链匹配不上。
/// 提取失败返回 None（端点验证随之跳过，不据此判失败）。
fn a2dp_id_mac(id: &str) -> Option<String> {
    let lower = id.to_lowercase();
    let is_12_hex = |s: &str| s.len() == 12 && s.bytes().all(|b| b.is_ascii_hexdigit());
    if let Some(idx) = lower.find("_c00000000") {
        if let Some(mac) = lower.get(idx.saturating_sub(12)..idx) {
            if is_12_hex(mac) {
                return Some(mac.to_string());
            }
        }
    }
    let mut scan_from = 0;
    while let Some(pos) = lower[scan_from..].find("dev_") {
        let abs = scan_from + pos + "dev_".len();
        if let Some(mac) = lower.get(abs..abs + 12) {
            if is_12_hex(mac) {
                return Some(mac.to_string());
            }
        }
        scan_from = abs;
    }
    None
}

// ── 对外命令入口 ────────────────────────────────────────────────────────

/// 当前状态快照（纯内存读，同步命令直呼）。
pub fn get_status() -> A2dpStatus {
    lock_unpoisoned(&SNAPSHOT).clone()
}

/// 请求连接指定设备（同步命令直呼：仅通道投递，零阻塞）。
/// `name_hint` = 前端现成显示名，None 时控制器回查枚举（仅启动恢复路径）。
pub fn request_connect(id: &str, name: Option<&str>) -> Result<(), String> {
    if !supported() {
        return Err("此系统版本不支持蓝牙音频接收（需 Win10 2004+）".to_string());
    }
    let Some(tx) = A2DP_TX.get() else {
        return Err("蓝牙音频接收尚未就绪".to_string());
    };
    verbose_log!("[a2dp] 请求连接 {}", id);
    tx.send(Msg::Connect {
        id: id.to_string(),
        name: name.filter(|s| !s.is_empty()).map(str::to_string),
    })
    .map_err(|_| "蓝牙音频接收控制器已退出".to_string())
}

/// 请求断开当前连接（同步命令直呼）。
pub fn request_disconnect() -> Result<(), String> {
    let Some(tx) = A2DP_TX.get() else {
        return Err("蓝牙音频接收尚未就绪".to_string());
    };
    tx.send(Msg::Disconnect)
        .map_err(|_| "蓝牙音频接收控制器已退出".to_string())
}

/// 请求控制器退出（应用退出路径调用；连接对象由控制器收尾）。
pub fn request_shutdown() {
    if let Some(tx) = A2DP_TX.get() {
        let _ = tx.send(Msg::Shutdown);
    }
}

/// popup「通过本机播放」：把 popup 设备卡的 WinRT 设备 id 折算成 MAC
/// （复用 `bluetooth::normalize_mac` 口径），在可接收列表里按 MAC 匹配后
/// 发起连接。匹配不上 ⇒ 该设备没有 A2DP Sink 能力。
pub fn connect_from_popup_device(popup_device_id: &str, popup_name: &str) -> Result<(), String> {
    if !supported() {
        return Err("此系统版本不支持蓝牙音频接收（需 Win10 2004+）".to_string());
    }
    let Some(mac) = crate::bluetooth::normalize_mac(popup_device_id) else {
        return Err("无法解析该设备的蓝牙地址".to_string());
    };
    let devices = enumerate_devices()?;
    let Some(target) = devices.iter().find(|d| d.mac.eq_ignore_ascii_case(&mac)) else {
        return Err("该设备不支持音频接收（手机需与本机配对且支持蓝牙音频）".to_string());
    };
    request_connect(&target.id, Some(popup_name))
}

/// main setup 注入：装配事件句柄并启动控制器线程（对齐 bluetooth.rs 注入模式）。
pub fn init(app: &tauri::AppHandle) {
    let _ = A2DP_HANDLE.set(app.clone());
    // ⛔ 支持性探测必须在本函数同步完成、不能等控制器线程：request_connect 与
    // on_connect 都以 `supported()` 判定，若等控制器异步探测，启动约 1s 内到达的
    // 连接请求会被误报「此系统版本不支持」。IsTypePresent 是纯元数据查询，
    // 任意线程可调、主线程直呼无阻塞。
    let probe = ApiInformation::IsTypePresent(&HSTRING::from(
        "Windows.Media.Audio.AudioPlaybackConnection",
    ))
    .unwrap_or(false);
    let _ = SUPPORTED.set(probe);
    set_snapshot(|s| s.supported = probe);
    let (tx, rx) = channel();
    let _ = A2DP_TX.set(tx);
    let spawned = std::thread::Builder::new()
        .name("a2dp-sink".to_string())
        .spawn(move || controller_main(rx));
    if let Err(e) = spawned {
        crate::process::append_log(&format!("[a2dp] 控制器线程启动失败：{}", e));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a2dp_id_mac_extracts_12_hex_before_c00000000() {
        // 形态①：`_C00000000` 紧跟 MAC
        assert_eq!(
            a2dp_id_mac("BTHENUM\\DEV_AABBCCDDEEFF_C00000000_CYCLONE").as_deref(),
            Some("aabbccddeeff")
        );
        // 形态②：`DEV_<MAC>.<hash>_C00000000`——形态①夹着 `.` 与 hash 段，走 DEV_ 兜底
        assert_eq!(
            a2dp_id_mac("BTHENUM\\DEV_a888ce26e2be.8bfa9a68_c00000000_cyclone_localname")
                .as_deref(),
            Some("a888ce26e2be")
        );
        // 只有 `DEV_` 前缀、无 `_C00000000` 的实例 id
        assert_eq!(
            a2dp_id_mac("BTHENUM\\DEV_AABBCCDDEEFF").as_deref(),
            Some("aabbccddeeff")
        );
    }

    #[test]
    #[ignore = "真机探针：cargo test a2dp_probe -- --ignored --nocapture；依赖本机蓝牙栈状态"]
    fn a2dp_probe_enumerate() {
        match enumerate_devices() {
            Ok(devices) => {
                println!("枚举到 {} 台：", devices.len());
                for d in &devices {
                    println!("  name={:?} mac={} id={}", d.name, d.mac, d.id);
                }
            }
            Err(e) => println!("枚举失败：{e}"),
        }
    }

    #[test]
    fn a2dp_id_mac_rejects_missing_or_short_prefix() {
        assert_eq!(a2dp_id_mac(""), None);
        // `_C00000000` 之前不足 12 位，且 DEV_ 后也只有 3 位
        assert_eq!(a2dp_id_mac("BTHENUM\\DEV_ABC_C00000000"), None);
        // MAC 段不是十六进制
        assert_eq!(a2dp_id_mac("BTHENUM\\DEV_NOT-HEX-KEYS_C00000000"), None);
    }

    #[test]
    fn backoff_delay_follows_steps_and_caps() {
        assert_eq!(backoff_delay(0), Duration::from_secs(2));
        assert_eq!(backoff_delay(1), Duration::from_secs(4));
        assert_eq!(backoff_delay(4), Duration::from_secs(30));
        assert_eq!(backoff_delay(9), Duration::from_secs(30));
    }

    #[test]
    fn build_hint_always_actionable() {
        for code in [
            "OpenStatus",
            "OpenTimeout",
            "ArmTimeout",
            "NotSupported",
            "NotOpened",
            "RemoteClosed",
            "Weird",
        ] {
            assert!(!build_hint(code).is_empty(), "code={} 提示为空", code);
        }
    }

    #[test]
    fn abort_lands_idle_only_on_disconnect_and_drops_pending() {
        // 打断路径：Disconnect 立即生效（落 Idle、意图清除），积压的旧 Connect 一并作废
        let (tx, rx) = channel();
        tx.send(Msg::Connect {
            id: "old".into(),
            name: None,
        })
        .unwrap();
        tx.send(Msg::Disconnect).unwrap();
        let mut m = Machine {
            want_connected: true,
            ..Default::default()
        };
        assert!(abort_if_disconnected(&rx, &mut m));
        assert!(!m.want_connected);
        assert!(m.pending_msgs.is_empty());
        assert_eq!(lock_unpoisoned(&SNAPSHOT).state, A2dpState::Idle);
    }

    #[test]
    fn abort_without_disconnect_keeps_messages_in_pending() {
        // 无打断：非 Disconnect 消息不能丢（连接中点另一台设备 = 换目标），暂存回灌
        let (tx, rx) = channel();
        tx.send(Msg::Connect {
            id: "new".into(),
            name: Some("手机".into()),
        })
        .unwrap();
        let mut m = Machine::default();
        assert!(!abort_if_disconnected(&rx, &mut m));
        assert!(
            matches!(m.pending_msgs.as_slice(), [Msg::Connect { id, name: Some(_) }] if id == "new")
        );
    }
}
