#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
#![warn(unused_imports, dead_code)]

mod app_icon;
mod audio;
mod audio_notify;
mod audio_policy;
mod audio_spatial;
mod battery_notify;
mod bluetooth;
mod bt_ble;
mod classify;
mod commands;
mod config;
mod dedup;
mod device;
mod device_data;
mod device_identity;
mod popup;
mod process;
mod shortcut;
mod state;
#[cfg(target_os = "windows")]
mod toast;
mod tray;
mod update;
mod webview;
mod window_material;
mod windows;
mod wireless_24g;
mod wmi_query;
mod xinput;

use std::panic;

use tauri::Emitter;
use tauri::Manager;
use tauri::RunEvent;

/// panic 发生的位置（相对主线程）——决定「要不要弹模态框」（B12）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PanicSite {
    /// panic 发生在主线程：应用本就要终止，弹框是合理的最后一手告知
    Main,
    /// panic 发生在后台线程：只落日志，把栈展开还给 panic 运行时
    Background,
}

impl PanicSite {
    /// 依据「panic 所在线程 id」与「主线程 id」判定。
    fn of(panicking: std::thread::ThreadId, main: std::thread::ThreadId) -> Self {
        if panicking == main {
            Self::Main
        } else {
            Self::Background
        }
    }

    /// 是否应当弹模态框。**只有主线程可以**（根因见 `install_panic_hook` 文档）。
    fn shows_dialog(self) -> bool {
        matches!(self, Self::Main)
    }
}

/// 从 panic payload 提取可读消息；无法识别的 payload 退回固定文案。
fn panic_payload_message(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "Unknown panic".to_string()
    }
}

/// 把 panic 位置格式化成 `file:line:col`；拿不到位置时退回固定文案。
fn format_location(location: Option<&panic::Location<'_>>) -> String {
    location
        .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
        .unwrap_or_else(|| "unknown location".to_string())
}

/// 拼出完整的 panic 报告（消息 + 位置），用于弹框正文与 stderr 兜底。
fn format_panic_report(msg: &str, location: &str) -> String {
    format!("{}\n\nLocation: {}", msg, location)
}

/// 统一的 panic 处置（B12）：主线程弹框、后台线程只写 stderr；返回本次处置结果。
///
/// 之所以把「弹框动作」交由调用方注入（`on_main_panic`），是为了让单测能传入
/// **记录器**替代 `show_error_box`：模态框会把测试挂死，而替换后即可在真实安装
/// hook 的前提下断言「后台线程 panic 不走弹框分支、且栈展开照常完成」。
fn handle_panic(
    msg: &str,
    location: &str,
    panicking_thread: std::thread::ThreadId,
    main_thread_id: std::thread::ThreadId,
    on_main_panic: &(dyn Fn(&str) + Send + Sync),
) -> PanicSite {
    let full = format_panic_report(msg, location);
    let site = PanicSite::of(panicking_thread, main_thread_id);
    if site.shows_dialog() {
        on_main_panic(&full);
    } else {
        // 后台线程不弹框（见 `install_panic_hook`），改用 stderr 兜底——
        // 安装版无控制台，但开发/调试与重定向到文件时可见
        eprintln!("[panic] 后台线程 panic（不弹框以免阻塞栈展开）：{}", full);
    }
    site
}

/// 组装 panic hook 本体（**不含**对默认 hook 的委托），抽出以便单测安装同一份逻辑。
fn make_panic_hook(
    main_thread_id: std::thread::ThreadId,
    on_main_panic: Box<dyn Fn(&str) + Send + Sync + 'static>,
) -> Box<dyn Fn(&panic::PanicHookInfo<'_>) + Send + Sync + 'static> {
    Box::new(move |info| {
        let msg = panic_payload_message(info.payload());
        let location = format_location(info.location());
        standard_log!("[panic] {} @ {}", msg.replace('\n', " | "), location);
        handle_panic(
            &msg,
            &location,
            std::thread::current().id(),
            main_thread_id,
            &*on_main_panic,
        );
    })
}

/// 安装 panic hook：主线程 panic 弹 MessageBox 告知用户，后台线程 panic 只写日志。
///
/// ⚠️ **绝不能对后台线程弹框**（B12）：`show_error_box` 用的是 `MessageBoxW`，**模态**
/// ——它会一直阻塞到用户点掉。而 panic hook 是在**panic 的那个线程**上同步执行的，
/// 于是「后台线程 panic」的真实后果不是「死一个线程」，而是：
///   ① 该线程**永久卡在弹框上，栈展开根本不发生** ⇒ 它持有的锁、RAII 守卫
///      （`SingleFlightGuard` 等）永不释放，把「局部故障」放大成「整机卡死」。
///      P1-8 的注入验证实测到了这条链路：panic 后子进程 **stderr 为 0 字节**
///      （证明 `default_hook` 从未执行），`ANIMATING` 一直为 `true`，
///      只能靠超时自愈救回。
///   ② release 下 `panic = "abort"` 本想「快速失败」，但 hook 卡住 ⇒ **abort 永远不会发生**，
///      进程带着一个僵死线程继续跑，`panic = "abort"` 的语义被静默抵消。
///
/// 故此处按线程区分（`PanicSite`）：主线程 panic 时应用本就要终止，弹框是合理的最后一手
/// 告知；后台线程只落日志（`standard_log!` + stderr 双通道），把栈展开还给 panic 运行时。
///
/// 副作用（知情）：release 下后台线程 panic 现在会**真的**走到 `abort()`，
/// 即整个进程终止（此前是「弹框 + 线程僵死 + 进程存活」）。这是 `panic = "abort"`
/// 的既定语义，也是「快速失败优于带病运行」的选择；若不希望如此，
/// 应改为 `panic = "unwind"` 并让后台任务自行兜底，而不是靠 hook 卡住进程。
fn install_panic_hook() {
    let default_hook = panic::take_hook();
    // 本函数在 `main` 里调用 ⇒ 此处就是主线程
    let main_thread_id = std::thread::current().id();
    let inner = make_panic_hook(main_thread_id, Box::new(show_error_box));
    panic::set_hook(Box::new(move |info| {
        inner(info);
        default_hook(info);
    }));
}

fn show_error_box(msg: &str) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONERROR, MB_OK};
    let title = crate::process::to_wide("外设监控 - 启动失败");
    let message = crate::process::to_wide(msg);
    unsafe {
        MessageBoxW(
            core::ptr::null_mut(),
            message.as_ptr(),
            title.as_ptr(),
            MB_OK | MB_ICONERROR,
        );
    }
}

/// 等待指定 pid 的进程退出，最多 timeout_ms 毫秒。
/// 进程不存在（已退出/从未存在）时立即返回。用于看门狗重启路径。
#[cfg(target_os = "windows")]
fn wait_process_exit(pid: u32, timeout_ms: u32) {
    use windows_sys::Win32::System::Threading::{
        OpenProcess, WaitForSingleObject, PROCESS_QUERY_LIMITED_INFORMATION,
    };
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if !handle.is_null() {
            WaitForSingleObject(handle, timeout_ms);
            windows_sys::Win32::Foundation::CloseHandle(handle);
        }
    }
}

#[cfg(not(target_os = "windows"))]
fn wait_process_exit(_pid: u32, _timeout_ms: u32) {}

/// 探活连续超时几次判定「事件循环僵死」⇒ 自愈重启（P2-4）。
///
/// 单次超时（5s）可能只是偶发瞬时卡顿（GC、磁盘抖动、杀软扫描），
/// 连续两次（最坏 ~40s）才足以判定僵死。抽成纯函数以便边界单测。
fn should_restart(stuck_streak: u32) -> bool {
    stuck_streak >= 2
}

/// 看门狗每轮的 sleep 间隔（B7 由 15s 收紧到 3s）。
///
/// 收紧的理由：探活的判据是「主线程能否在 [`PROBE_REPLY_TIMEOUT`] 内执行一个纯内存
/// 的排队任务」，而连续 2 次超时才判定僵死 ⇒ 自愈延迟 ≈ 2×3s + 2×2s = 10s，
/// 原先（15s + 5s，同步等待）最坏要 ~40s 才重启。
const WATCHDOG_ROUND_INTERVAL: std::time::Duration = std::time::Duration::from_secs(3);

/// 探活等待上限：主线程超过这个时间仍未执行排队的闭包即视为无响应。
///
/// 取 2s 而非更长：正常路径上主线程执行一次原子自增是**微秒级**的，2s 已经是
/// 三个数量级的余量；再放长只会拖慢自愈。
const PROBE_REPLY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);

/// 事件循环探活计数（B7）：每次探活闭包在主线程上执行即 +1。
///
/// 为什么保留一个计数而不只用 channel 回执：回执只能回答「这一次有没有被处理」，
/// 累计值能区分「事件循环完全没跑」与「跑得很慢但一直在推进」——僵死日志带上它，
/// 事后归因时不必再猜。
static EVENT_LOOP_TICK: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// 探活等待侧（B7）：`timeout` 内收到回执 ⇒ 事件循环有响应。
///
/// **必须用 `recv_timeout` 而不是 `recv`**：后者在事件循环僵死时会永久挂住调用线程
/// ——原实现正是栽在这里（详见 [`spawn_watchdog`] 的注释）。抽成独立函数是为了让
/// 这条契约有单测锚点：把它改回 `recv`，`probe_wait_returns_false_instead_of_hanging`
/// 会直接挂死（即失败）。
fn await_event_loop_reply(
    rx: &std::sync::mpsc::Receiver<()>,
    timeout: std::time::Duration,
) -> bool {
    rx.recv_timeout(timeout).is_ok()
}

/// 心跳日志节流：每 [`HEARTBEAT_EVERY_ROUNDS`] 轮记一次 `[heartbeat]`。
///
/// 探活节奏从 15s 收紧到 3s 后，若心跳跟着每轮记一次，日志量会涨 5 倍
/// （每小时 1200 条），把真正有用的行淹掉。心跳的用途只是「证明进程还活着」，
/// 15s 一次足够，故与探活节奏解耦。
const HEARTBEAT_EVERY_ROUNDS: u64 = 5;

/// 本轮是否应记录心跳日志。抽成纯函数以便边界单测。
fn should_log_heartbeat(round: u64) -> bool {
    round.is_multiple_of(HEARTBEAT_EVERY_ROUNDS)
}

/// 本轮 sleep 间隔是否长到可判定「系统经历过休眠 / 唤醒」（P2-4）。
///
/// 期望 ~3s（[`WATCHDOG_ROUND_INTERVAL`]）；超过 8s 说明中间发生了挂起 ——
/// Suspend 期间渲染进程被暂停，唤醒后需主动 Resume WebView2。
///
/// ⚠️ 阈值与节奏**必须同步**：B7 把节奏从 15s 改成 3s 时，这里的 20s 若不同步下调，
/// 阈值就永远够不到，休眠唤醒检测会彻底失效。抽成纯函数以便边界单测。
fn is_time_jump(elapsed: std::time::Duration) -> bool {
    elapsed > std::time::Duration::from_secs(8)
}

/// 看门狗自愈重启的退出码。
///
/// 取非 0（BSD sysexits 的 `EX_SOFTWARE`）：这条路径是「事件循环僵死后的自愈」，
/// 不是正常关闭 —— 非 0 才能让任务管理器 / 事件日志 / 外部守护脚本把它与用户
/// 主动退出区分开。原先一律用 0，异常退出在外部看来与正常关闭无异（P2-4）。
const EXIT_CODE_WATCHDOG_RESTART: i32 = 70;

/// 事件循环僵死自愈：spawn 自身新实例后立即退出当前进程。
/// --autostart 复用静默启动逻辑（重启不弹窗）；
/// 旧 pid 经参数传递，新实例启动时内核级等待其退出，规避 single-instance 转发竞态。
fn watchdog_self_restart() {
    process::append_log("[watchdog] EVENT LOOP STUCK — self-restarting");
    // 退出前投递一次 WM_CLOSE，让音频 STA 线程反注册 IMMNotificationClient 与
    // 各设备/会话回调（P2-4）。本路径直接 process::exit，不经 RunEvent::Exit，
    // 不显式投递就会带着已注册的回调被进程直接丢弃。
    // `request_shutdown` 内部是 **PostMessageW 异步投递、不等待**，
    // 故在「主线程已僵死」的本场景下也不会卡住看门狗线程。
    crate::audio_notify::request_shutdown();
    let exe = std::env::current_exe().unwrap_or_default();
    if exe.as_os_str().is_empty() {
        // 走 process::exit 不经过 RunEvent::Exit，故此处显式排空日志与配置落盘队列
        // （B5 / B11）
        process::flush_log();
        crate::config::flush_persist();
        std::process::exit(EXIT_CODE_WATCHDOG_RESTART);
    }
    let arg = format!("--watchdog-restart={}", std::process::id());
    let spawn_ok = std::process::Command::new(&exe)
        .args(["--autostart", &arg])
        .spawn()
        .is_ok();
    if !spawn_ok {
        process::append_log("[watchdog] respawn FAILED, exiting anyway");
    }
    std::thread::sleep(std::time::Duration::from_millis(300));
    // 同上：这条路径同样绕过 RunEvent::Exit，必须显式 flush，
    // 否则「EVENT LOOP STUCK」这段最关键的现场日志会随进程一起消失。
    // （这 300ms 同时留给音频 STA 线程处理上面投递的 WM_CLOSE）
    process::flush_log();
    crate::config::flush_persist();
    std::process::exit(EXIT_CODE_WATCHDOG_RESTART);
}

/// 处理第二实例启动：聚焦既有弹窗，或经 toggle 重建。
/// 回调在 emit 的调用线程上同步执行（Tauri 无独立「事件线程」，见 AGENTS.md），
/// 窗口/配置操作全部移出线程。
fn forward_second_instance(app: &tauri::AppHandle) {
    process::append_log("[single-instance] second instance forwarded");
    let app = app.clone();
    std::thread::spawn(move || {
        let tab = config::with_config(|c| c.default_popup_tab.clone());
        if app.get_webview_window("popup").is_some() {
            standard_log!("[single-instance] popup exists, open tab={}", tab);
            popup::open_popup(&app, &tab);
        } else {
            process::append_log("[single-instance] no popup, create via toggle");
            popup::toggle(&app, &tab);
        }
    });
}

/// 开发调试：设置环境变量 PM_DEV_OPEN_SETTINGS 时延迟自动打开设置窗口（自动化检测用）
fn spawn_dev_open_settings(app: &tauri::AppHandle) {
    if std::env::var("PM_DEV_OPEN_SETTINGS").is_ok() {
        let handle = app.clone();
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
            crate::windows::open_settings(&handle);
        });
    }
}

/// 启动时检测更新：延迟 3s 后查询并广播状态，有更新时弹 Windows 原生通知。
fn spawn_startup_update_check(app: &tauri::AppHandle) {
    let app_handle = app.clone();
    tauri::async_runtime::spawn(async move {
        crate::process::append_verbose_log("[update] startup check starting");
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
        let include = config::with_config(|c| c.include_prerelease);
        let current_version = app_handle.package_info().version.to_string();
        let (result, stored) =
            crate::update::check_and_store("startup", current_version, include).await;
        match result {
            Ok(info) => {
                let status = if info.has_update {
                    if crate::windows::is_msix_context() {
                        "storeUpdate"
                    } else {
                        "update"
                    }
                } else {
                    "latest"
                };
                let payload = crate::update::UpdateStatus::from_info(&info, status);
                let _ = app_handle.emit("update-status", payload);
                if info.has_update {
                    let _ = app_handle.emit("update-available", info.clone());
                    // Windows 原生通知（带图标 + 点击跳转关于页）
                    #[cfg(target_os = "windows")]
                    {
                        let ico_path = crate::windows::resolve_toast_icon();
                        let app = app_handle.clone();
                        let toast_text = if status == "storeUpdate" {
                            "Microsoft Store 有新版本可用".to_string()
                        } else {
                            format!("发现新版本 v{}，点击查看详情", info.latest_version)
                        };
                        let toast = crate::windows::build_toast(
                            "发现新版本",
                            &toast_text,
                            ico_path.as_deref(),
                        )
                        .on_activated(move |_args| {
                            crate::process::append_log("[update] toast clicked → open about");
                            let app = app.clone();
                            std::thread::spawn(move || {
                                crate::windows::open_settings_tab(&app, "about");
                            });
                            Ok(())
                        });
                        if let Err(e) = toast.show() {
                            standard_log!("[update] toast failed: {:?}", e);
                        }
                    }
                }
            }
            Err(_) => {
                // 检查失败：广播已存储的错误状态；任务级失败（未存储）不广播
                if stored {
                    if let Some(payload) = crate::update::get_last_status() {
                        let _ = app_handle.emit("update-status", payload);
                    }
                }
            }
        }
    });
}

/// 看门狗线程：心跳 + 事件循环探活 + 唤醒恢复。
///
/// 探针原理（B7 改写）：主线程能否在 [`PROBE_REPLY_TIMEOUT`] 内执行一个**纯内存**
/// 的排队任务。连续 2 次超时（≈10s）判定僵死，自动重启自身进程自愈。
/// 时间跳变检测：唤醒后主动 Resume WebView2（仅针对**休眠唤醒**这一类，见下方范围限定）。
///
/// ⚠️ **范围限定（2026-09-18）**：**「B 类僵死」不是「运行期窗口冻结」的解释。**
/// 实测（`AppHangTransient` / 退出码 `0xcfffffff`）证明那次冻结的根因是**锁序死锁（P0-4）**：
/// 子线程持配置锁调菜单 API（`run_item_main_thread!` = 无超时 `rx.recv()`）⇄ 主线程等同一把
/// 配置锁 ⇒ 永久互等。**看门狗救不回这一类**——本函数的探活同样依赖主线程。
/// 遇到「窗口完全无响应」请**先查锁序**（登记表见 `state.rs` 模块文档）。
///
/// ⚠️ **不要改回「调 Tauri API + 同步等待」的写法**。原实现是
/// `spawn` 一个线程调 `w.is_visible()`——它内部是 `run_on_main_thread(..)` +
/// `rx.recv()`，**无超时地同步等待主线程**。事件循环真僵死时它永不返回，于是：
/// ① **每轮泄漏一个永久挂住的探针线程**（本循环 3s 一轮）；
/// ② 这些线程各自已在主线程队列里压了一个任务，主线程一旦恢复就会一次性全部执行；
/// ③ 泄漏速度与「连续 2 次超时」的判定窗口叠加，僵死期间线程数持续增长。
///
/// 现在**不再 spawn 线程**：`run_on_main_thread` 只负责**排队**（立即返回，不等待），
/// 闭包体内只有一次原子自增与一次 channel 通知（都不碰任何 Tauri API），
/// 由看门狗线程自己 [`await_event_loop_reply`] 限时等待。主线程僵死 ⇒ 队列不被消费
/// ⇒ 超时；无论成败都不留线程，堆积问题从结构上消失。
fn spawn_watchdog(app: &tauri::AppHandle) {
    let app_handle = app.clone();
    std::thread::spawn(move || {
        use std::time::Instant;

        let mut stuck_streak = 0u32;
        let mut last_instant = Instant::now();
        let mut round = 0u64;

        loop {
            std::thread::sleep(WATCHDOG_ROUND_INTERVAL);
            round = round.wrapping_add(1);
            if should_log_heartbeat(round) {
                crate::process::append_log("[heartbeat]");
            }

            // 时间跳变检测：期望 ~3s，>8s 说明系统经历过休眠/唤醒。
            // 唤醒后主动 Resume WebView2 渲染进程（Suspend 期间渲染暂停）。
            //
            // ⚠️ 基准点 `last_instant` 必须取在**本轮探活结束之后**（见循环末尾），
            // 即只度量 sleep 间隔。若按直觉取在本行（sleep 之后、探活之前），
            // 上一轮探活的超时耗时（最长 2s）会叠加进下一轮间隔，使 3s 变成
            // 5.008s —— 虽然仍够不到 8s 阈值，但这个「基准点位置」的约定必须守住：
            // 阈值一旦收紧（或超时上限放大），同样的错位就会变成每次超时都误报
            // 「系统休眠唤醒」，进而执行一次不必要的 resume_webview。
            let elapsed = Instant::now().duration_since(last_instant);
            if is_time_jump(elapsed) {
                standard_log!(
                    "[watchdog] time jump: {:.1}s — resuming webview",
                    elapsed.as_secs_f64()
                );
                if let Some(popup_win) = app_handle.get_webview_window("popup") {
                    let wv: &tauri::Webview = popup_win.as_ref();
                    crate::webview::resume_webview(wv);
                }
            }

            // ── 探活（B7）───────────────────────────────────────────────
            let (tx, rx) = std::sync::mpsc::channel();
            let queued = app_handle.run_on_main_thread(move || {
                EVENT_LOOP_TICK.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let _ = tx.send(());
            });
            // 排队失败（事件循环已退出）时 `tx` 随闭包一起被丢弃，`recv_timeout`
            // 会立即返回 `Disconnected` ⇒ 同样判为无响应，无需单独分支。
            let responded = queued.is_ok() && await_event_loop_reply(&rx, PROBE_REPLY_TIMEOUT);
            if responded {
                stuck_streak = 0;
            } else {
                stuck_streak += 1;
                standard_log!(
                    "[watchdog] event loop unresponsive, streak={} (tick={})",
                    stuck_streak,
                    EVENT_LOOP_TICK.load(std::sync::atomic::Ordering::SeqCst)
                );
                if should_restart(stuck_streak) {
                    watchdog_self_restart();
                }
            }

            // 下一轮的时间基准：置于探活之后，使 elapsed 只含 sleep 间隔（~3s）
            last_instant = Instant::now();
        }
    });
}

/// 处理窗口事件：弹窗失焦关闭、设置窗延迟销毁、弹窗关闭改为隐藏。
/// 回调只做轻量判断，阻塞操作移入子线程（窗口事件由 tao 主循环派发 ⇒ 本回调即主线程）。
fn handle_window_event(window: &tauri::Window, event: &tauri::WindowEvent) {
    match event {
        tauri::WindowEvent::Focused(focused) => {
            if window.label() == "popup" && !focused {
                // close_popup 内部自带 ANIMATING/is_visible 防护，
                // 重复分发安全，且 compute_position/Win32 调用不阻塞事件循环
                let app = window.app_handle().clone();
                std::thread::spawn(move || {
                    popup::close_popup(&app);
                    // 隐藏后降内存档位：Chromium 主动收缩 browser/GPU 缓存（与 TrySuspend 互补）
                    if let Some(w) = app.get_webview_window("popup") {
                        let wv: &tauri::Webview = w.as_ref();
                        crate::webview::set_memory_usage_target(wv, true);
                    }
                });
            }
        }
        tauri::WindowEvent::CloseRequested { api, .. } => {
            let label = window.label();
            if label == "settings" {
                // hide 先行：点击瞬间响应。销毁延后 3s 在子线程触发——
                // wry drop 链的 controller.Close() 会在事件循环线程同步执行，
                // 浏览器进程繁忙时曾三次卡死整个事件循环（假死）。
                // dispatched/destroyed 成对日志用于定罪卡点。
                api.prevent_close();
                let _ = window.hide();
                let win = window.clone();
                std::thread::spawn(move || {
                    std::thread::sleep(std::time::Duration::from_secs(3));
                    if win.is_visible().unwrap_or(true) {
                        return; // 延迟期内被重新打开，放弃本次销毁
                    }
                    crate::process::append_log("[window] settings destroy dispatched");
                    let _ = win.destroy();
                    crate::process::append_log("[window] settings destroyed");
                });
            } else if label == "popup" {
                api.prevent_close();
                let _ = window.hide();
                if let Some(popup_wv) = window.app_handle().get_webview_window("popup") {
                    let wv: &tauri::Webview = popup_wv.as_ref();
                    crate::webview::suspend_webview(wv);
                    crate::webview::set_memory_usage_target(wv, true);
                }
            }
        }
        _ => {}
    }
}

fn main() {
    // 看门狗自重启路径：等待旧实例内核级死亡后再初始化。
    // 必须先于一切（含 single-instance 插件）：插件第二实例会 SendMessageW(WM_COPYDATA)
    // 同步转发给旧窗口，若旧实例僵死未退则新实例将永久阻塞在转发这一步
    let watchdog_restart = std::env::args().find_map(|a| {
        a.strip_prefix("--watchdog-restart=")
            .and_then(|p| p.parse::<u32>().ok())
    });
    if let Some(old_pid) = watchdog_restart {
        // 此行先于 init_config 会被 log_enabled 吞掉，仅保留语义占位；
        // 确认日志在配置初始化后补写（见下）
        wait_process_exit(old_pid, 3000);
    }

    // 先初始化配置（panic hook 和日志都依赖配置）
    config::init_config();

    if let Some(old_pid) = watchdog_restart {
        standard_log!("[watchdog] restart mode, waited old pid={} exit", old_pid);
    }

    install_panic_hook();

    // Init COM with apartment-threaded mode (same as Tauri) BEFORE Tauri starts.
    // 供 bluetooth/audio 等 WinRT 消费方使用；wmi 0.18 已在查询侧经 CoIncrementMTAUsage 自行初始化
    unsafe {
        let hr = windows_sys::Win32::System::Com::CoInitializeEx(
            std::ptr::null(),
            0x2, // COINIT_APARTMENTTHREADED
        );
        if hr < 0 {
            // 区分「公寓模型冲突」与真失败（P2-5）：主线程本该是最先初始化 COM 的，
            // 出现 RPC_E_CHANGED_MODE 说明有别的东西抢先把主线程定成了 MTA
            // ⇒ 与「进程内统一 STA」的约定冲突，值得单独告警。
            if crate::audio::is_apartment_mode_conflict(hr) {
                standard_log!(
                    "[main] COM 公寓模型冲突（RPC_E_CHANGED_MODE）：主线程已被初始化为 MTA，\
                     与「进程内统一 STA」的约定不符"
                );
            } else {
                standard_log!("[main] CoInitializeEx failed: 0x{:08X}", hr);
            }
        }
    }

    device_data::init_device_data();
    tray::init_auto_start();

    // 根据日志保留策略清理旧日志
    process::clean_old_logs();

    let is_autostart = std::env::args().any(|a| a == "--autostart");
    if is_autostart {
        process::append_log("[main] autostart mode");
    }

    let mut builder = tauri::Builder::default().plugin(tauri_plugin_autostart::init(
        tauri_plugin_autostart::MacosLauncher::LaunchAgent,
        Some(vec!["--autostart"]),
    ));
    // 看门狗自重启实例跳过单实例保护：旧进程在数百毫秒内退出、新实例必然独占，
    // 若仍注册，启动竞态窗口内会因旧实例互斥量/窗口尚未完成内核清理而被
    // 误判为第二实例，走转发路径 exit(0) 自杀（beta.1 自愈实测踩中）
    if watchdog_restart.is_none() {
        builder = builder.plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            forward_second_instance(app);
        }));
    }
    let builder = builder
        .plugin(
            tauri_plugin_window_state::Builder::default()
                // popup 每次点击都重定位，持久化无意义且会以旧物理坐标覆盖新算位置
                .with_denylist(&["popup"])
                .build(),
        )
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .invoke_handler(tauri::generate_handler![
            // 设备列表与配置
            commands::get_devices,
            commands::get_devices_fresh,
            commands::get_cached_devices,
            commands::get_config,
            commands::get_config_load_error,
            commands::get_shortcut_register_failed,
            commands::get_app_version,
            commands::set_window_theme,
            commands::update_config,
            commands::toggle_device_hidden,
            commands::toggle_audio_device_hidden,
            commands::open_settings,
            commands::rename_device,
            commands::change_device_group,
            commands::toggle_group_hidden,
            // 蓝牙
            commands::connect_bluetooth_device,
            commands::disconnect_bluetooth_device,
            commands::check_bt_connection,
            commands::open_bt_settings,
            // 杂项入口
            commands::open_url,
            commands::frontend_log,
            commands::open_24g_device_file,
            commands::toggle_device_tray,
            // 音频设备/会话
            commands::get_audio_devices,
            commands::set_device_volume,
            commands::toggle_device_mute,
            commands::set_device_mute,
            commands::get_audio_sessions,
            commands::set_session_volume,
            commands::set_session_mute,
            commands::get_input_devices,
            commands::set_session_device,
            commands::get_session_device,
            commands::get_sessions_device_names,
            commands::set_default_device,
            commands::get_spatial_sound,
            commands::set_spatial_sound,
            // 日志与更新
            commands::open_log_dir,
            commands::check_for_update,
            commands::get_update_status,
            // 快捷键
            commands::set_hotkey_config,
            commands::set_device_shortcut,
            commands::remove_device_shortcut,
            commands::set_shortcut_recording,
            // 窗口材质
            commands::set_window_material,
            commands::check_material_support,
        ])
        .setup(move |app| {
            // 注册 AUMID，使 Windows 通知显示应用图标。
            // 首次启动需同步等待一次 PowerShell（冷启动 300ms~1.5s），故下放子线程，
            // 不叠加到应用可见时间上；函数自身幂等（快捷方式已最新时立即返回）。
            #[cfg(target_os = "windows")]
            std::thread::spawn(crate::windows::register_aumid);

            if let Err(e) = tray::setup_tray(app) {
                standard_log!("[main] setup_tray failed: {}", e);
            }
            // 初始化音频通知回调（替代轮询）
            crate::audio_notify::init_audio_notify(app.handle().clone());
            process::append_log("[main] audio_notify initialized");
            // 2.4G 电量变更事件推送句柄
            crate::wireless_24g::init_event_handle(app.handle());
            // 蓝牙电量变更事件推送句柄
            crate::bluetooth::init_bt_event_handle(app.handle());
            // 蓝牙适配器状态监听（开关蓝牙立即刷新设备列表）
            crate::bluetooth::init_radio_watcher(app.handle());
            process::append_log("[main] bt_event_handle initialized");
            process::append_log("[main] radio_watcher initialized");
            crate::shortcut::register_shortcuts(app.handle());
            if !is_autostart {
                popup::open_popup(app.handle(), "devices");
            }

            // 开发调试：设置此环境变量时自动打开设置窗口（用于自动化检测）
            spawn_dev_open_settings(app.handle());

            // 启动时检测更新（仅非 autostart 模式）
            if !is_autostart && config::with_config(|c| c.check_updates) {
                spawn_startup_update_check(app.handle());
            }

            // 看门狗线程：心跳 + 事件循环探活 + 唤醒恢复（详注见 spawn_watchdog）
            spawn_watchdog(app.handle());

            process::append_log("[main] startup complete");
            Ok(())
        })
        .on_window_event(handle_window_event);
    let app = match builder.build(tauri::generate_context!()) {
        Ok(app) => app,
        Err(e) => {
            show_error_box(&format!("应用初始化失败：\n{}", e));
            // 日志已改为独立写线程（B5）、配置落盘已改为写线程（B11），
            // 退出前须等两个队列都排空，否则启动期日志与刚写入的设置会丢
            crate::process::flush_log();
            crate::config::flush_persist();
            std::process::exit(1);
        }
    };
    app.run(|_app_handle, event| match event {
        RunEvent::ExitRequested { .. } => {
            crate::audio_notify::request_shutdown();
        }
        // 兜底 flush：`RunEvent::Exit` 是所有正常退出路径（含托盘菜单 `app.exit`）
        // 的必经点，在此等日志队列排空，避免丢掉关停路径的最后几行——
        // 那恰是排查时最想看的部分。flush 幂等（队列已空则立即返回）。
        //
        // 配置落盘同理（B11）：`with_config_mut` 只把快照交给写线程就返回，
        // 不在此排空的话，「改完设置立刻退出」会丢掉最后一次改动。
        // 这里也是 B11「异常终止可能丢最后一次设置」那个代价的**收窄点**——
        // 只有崩溃 / 被强杀才落在窗口内。
        RunEvent::Exit => {
            crate::config::flush_persist();
            crate::process::flush_log();
        }
        _ => {}
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};

    /// 「弹框」的替身：把弹框正文记进 Vec，避免测试真的弹模态框。
    type DialogLog = Arc<Mutex<Vec<String>>>;
    /// 注入给被测代码的「弹框函数」。
    type DialogFn = Box<dyn Fn(&str) + Send + Sync>;

    fn recorder() -> (DialogLog, DialogFn) {
        let log: DialogLog = Arc::new(Mutex::new(Vec::new()));
        let handle = log.clone();
        let f: DialogFn =
            Box::new(move |msg: &str| crate::state::lock_unpoisoned(&handle).push(msg.to_string()));
        (log, f)
    }

    /// 取一个确定与当前线程不同的线程 id。
    fn other_thread_id() -> std::thread::ThreadId {
        std::thread::spawn(|| std::thread::current().id())
            .join()
            .expect("取线程 id 的线程不应 panic")
    }

    // ── P2-4：看门狗判据的边界 ─────────────────────────

    #[test]
    fn should_restart_only_after_two_consecutive_timeouts() {
        assert!(!should_restart(0), "0 次超时不应重启");
        assert!(!should_restart(1), "单次超时可能只是瞬时卡顿，不应重启");
        assert!(should_restart(2), "连续两次超时判定僵死");
        assert!(should_restart(3), "持续僵死应继续重启");
    }

    /// B7 把看门狗节奏从 15s 收紧到 3s，阈值必须同步下调（20s → 8s）——
    /// 否则阈值永远够不到，休眠唤醒检测会**彻底失效**（且没有任何报错）。
    #[test]
    fn is_time_jump_is_strictly_greater_than_8s() {
        assert!(!is_time_jump(std::time::Duration::from_secs(7)));
        // 期望间隔 ~3s；8s 整仍视为正常抖动（判据是严格大于）
        assert!(!is_time_jump(std::time::Duration::from_secs(8)));
        assert!(is_time_jump(std::time::Duration::from_secs(9)));
        // 真实休眠唤醒场景：几十分钟
        assert!(is_time_jump(std::time::Duration::from_secs(3600)));
        // 与节奏常量的关系：阈值必须明显大于一轮间隔，否则正常轮次就会误报
        assert!(
            !is_time_jump(WATCHDOG_ROUND_INTERVAL),
            "一轮正常的 sleep 间隔绝不能被判为时间跳变"
        );
    }

    // ── B7：探活不再泄漏线程 ───────────────────────────

    /// B7 的核心契约：事件循环僵死时，等待侧必须**限时返回 false**，不得挂住。
    ///
    /// 可证伪性：把 `await_event_loop_reply` 里的 `recv_timeout` 改回 `recv`，
    /// 本用例会直接**挂死**（= 失败），而不是侥幸通过。原实现的
    /// `is_visible()` 正是这种「无超时同步等待」，才导致每轮泄漏一个探针线程。
    #[test]
    fn probe_wait_returns_false_instead_of_hanging_when_no_reply() {
        // 发送端保持存活但永不发送 = 模拟「闭包已排队、主线程僵死不消费队列」
        let (_tx, rx) = std::sync::mpsc::channel::<()>();
        let timeout = std::time::Duration::from_millis(120);

        let started = std::time::Instant::now();
        let responded = await_event_loop_reply(&rx, timeout);
        let elapsed = started.elapsed();

        assert!(!responded, "队列未被消费时必须判为「无响应」");
        assert!(elapsed >= timeout, "必须等满超时才返回（实测 {elapsed:?}）");
        assert!(
            elapsed < timeout * 5,
            "必须在超时后立即返回，不得挂住（实测 {elapsed:?}）"
        );
    }

    /// 回执到达时应立即返回 true，不必等满超时——否则每轮都会白等 2s，
    /// 正常路径下的节奏会从 3s 退化成 5s。
    #[test]
    fn probe_wait_returns_true_immediately_when_reply_arrives() {
        let (tx, rx) = std::sync::mpsc::channel::<()>();
        tx.send(()).expect("发送端存活，应能发送");

        let started = std::time::Instant::now();
        let responded = await_event_loop_reply(&rx, PROBE_REPLY_TIMEOUT);

        assert!(responded, "已有回执时必须判为「有响应」");
        assert!(
            started.elapsed() < PROBE_REPLY_TIMEOUT,
            "有回执时不应等满超时（实测 {:?}）",
            started.elapsed()
        );
    }

    /// 心跳日志节流：节奏收紧到 3s 后不能每轮都记，否则日志量涨 5 倍。
    #[test]
    fn heartbeat_is_throttled_to_every_fifth_round() {
        assert_eq!(HEARTBEAT_EVERY_ROUNDS, 5, "节流周期即「原 15s / 新 3s」");
        let logged: Vec<u64> = (1..=15).filter(|r| should_log_heartbeat(*r)).collect();
        assert_eq!(logged, vec![5, 10, 15], "应每 5 轮记一次，且第 5 轮即首记");
        assert!(
            !should_log_heartbeat(1),
            "第 1 轮不记，避免刚启动就多一条日志"
        );
    }

    #[test]
    fn panic_site_of_distinguishes_main_from_background() {
        let main = std::thread::current().id();
        let other = other_thread_id();
        assert_ne!(main, other, "前提：另起线程的 id 必须与当前线程不同");
        assert_eq!(PanicSite::of(main, main), PanicSite::Main);
        assert_eq!(PanicSite::of(other, main), PanicSite::Background);
    }

    #[test]
    fn only_main_thread_shows_dialog() {
        assert!(PanicSite::Main.shows_dialog());
        assert!(
            !PanicSite::Background.shows_dialog(),
            "后台线程弹模态框会永久阻塞栈展开（B12 根因）"
        );
    }

    #[test]
    fn main_thread_panic_goes_through_dialog_hook() {
        let (log, rec) = recorder();
        let me = std::thread::current().id();
        let site = handle_panic("boom", "src/x.rs:1:2", me, me, &*rec);

        assert_eq!(site, PanicSite::Main);
        let got = crate::state::lock_unpoisoned(&log);
        assert_eq!(got.len(), 1, "主线程 panic 应恰好走一次弹框分支");
        assert!(
            got[0].contains("boom"),
            "弹框正文应含 panic 消息：{:?}",
            got[0]
        );
        assert!(
            got[0].contains("src/x.rs:1:2"),
            "弹框正文应含位置：{:?}",
            got[0]
        );
    }

    #[test]
    fn background_thread_panic_skips_dialog_hook() {
        let (log, rec) = recorder();
        let main = std::thread::current().id();
        let site = handle_panic("boom", "src/x.rs:1:2", other_thread_id(), main, &*rec);

        assert_eq!(site, PanicSite::Background);
        assert!(
            crate::state::lock_unpoisoned(&log).is_empty(),
            "后台线程 panic 不得走弹框分支"
        );
    }

    #[test]
    fn payload_message_extracts_str_literal() {
        assert_eq!(panic_payload_message(&"静态字面量"), "静态字面量");
    }

    #[test]
    fn payload_message_extracts_owned_string() {
        assert_eq!(
            panic_payload_message(&String::from("格式化消息")),
            "格式化消息"
        );
    }

    #[test]
    fn payload_message_falls_back_for_opaque_payload() {
        assert_eq!(panic_payload_message(&42u32), "Unknown panic");
    }

    #[test]
    fn location_is_formatted_as_file_line_col() {
        let loc = location_here();
        let formatted = format_location(Some(loc));
        assert_eq!(
            formatted,
            format!("{}:{}:{}", loc.file(), loc.line(), loc.column())
        );
        assert!(formatted.contains("main.rs"), "应含文件名：{formatted}");
    }

    #[test]
    fn missing_location_falls_back_to_placeholder() {
        assert_eq!(format_location(None), "unknown location");
    }

    #[test]
    fn panic_report_contains_message_then_location() {
        assert_eq!(
            format_panic_report("boom", "src/x.rs:1:2"),
            "boom\n\nLocation: src/x.rs:1:2"
        );
    }

    /// 真装 hook 的集成验证（B12 验收要求）：后台线程 panic 必须
    /// ① 完成栈展开（RAII 守卫被释放）、② 不走弹框分支。
    ///
    /// 可证伪性：把 `PanicSite::shows_dialog` 改成恒 `true`，第三条断言转红。
    #[test]
    fn installed_hook_lets_background_panic_unwind_without_dialog() {
        struct DropFlag(Arc<AtomicBool>);
        impl Drop for DropFlag {
            fn drop(&mut self) {
                self.0.store(true, Ordering::SeqCst);
            }
        }

        // panic hook 是进程级全局状态 ⇒ 与其它安装 hook 的用例串行
        static HOOK_LOCK: Mutex<()> = Mutex::new(());
        let _serial = crate::state::lock_unpoisoned(&HOOK_LOCK);

        let (log, rec) = recorder();
        let hook = make_panic_hook(std::thread::current().id(), rec);
        let prev = panic::take_hook();
        panic::set_hook(hook);

        let unwound = Arc::new(AtomicBool::new(false));
        let flag = unwound.clone();
        let joined = std::thread::spawn(move || {
            let _guard = DropFlag(flag);
            panic!("B12 注入：后台线程 panic");
        })
        .join();
        panic::set_hook(prev);

        assert!(joined.is_err(), "后台线程应以 panic 收尾（Err）");
        assert!(
            unwound.load(Ordering::SeqCst),
            "栈展开必须真的发生 ⇒ RAII 守卫已释放"
        );
        assert!(
            crate::state::lock_unpoisoned(&log).is_empty(),
            "后台线程 panic 不得走弹框分支（走了会卡死该线程）"
        );
    }

    /// `#[track_caller]` 包装：让 `Location::caller()` 返回调用点的位置。
    #[track_caller]
    fn location_here() -> &'static panic::Location<'static> {
        panic::Location::caller()
    }
}
