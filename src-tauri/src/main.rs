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

/// 事件循环僵死自愈：spawn 自身新实例后立即退出当前进程。
/// --autostart 复用静默启动逻辑（重启不弹窗）；
/// 旧 pid 经参数传递，新实例启动时内核级等待其退出，规避 single-instance 转发竞态。
fn watchdog_self_restart() {
    process::append_log("[watchdog] EVENT LOOP STUCK — self-restarting");
    let exe = std::env::current_exe().unwrap_or_default();
    if exe.as_os_str().is_empty() {
        // 走 process::exit 不经过 RunEvent::Exit，故此处显式排空日志队列（B5）
        process::flush_log();
        std::process::exit(0);
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
    // 否则「EVENT LOOP STUCK」这段最关键的现场日志会随进程一起消失
    process::flush_log();
    std::process::exit(0);
}

/// 处理第二实例启动：聚焦既有弹窗，或经 toggle 重建。
/// 回调在事件线程上分发，窗口/配置操作全部移出线程。
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
/// 探针原理：is_visible 经 proxy 往返（排队+recv），事件循环僵死则永挂；
/// worker 结果经 channel 回传，主循环 recv_timeout 超时即计僵死。
/// 连续 2 次超时（最坏 ~40s）判定僵死，自动重启自身进程自愈。
/// 时间跳变检测：唤醒后主动 Resume WebView2（B 类僵死根治）。
fn spawn_watchdog(app: &tauri::AppHandle) {
    let app_handle = app.clone();
    std::thread::spawn(move || {
        use std::time::Instant;

        let mut stuck_streak = 0u32;
        let mut last_instant = Instant::now();

        loop {
            std::thread::sleep(std::time::Duration::from_secs(15));
            crate::process::append_log("[heartbeat]");

            // 时间跳变检测：期望 ~15s，>20s 说明系统经历过休眠/唤醒。
            // 唤醒后主动 Resume WebView2 渲染进程（Suspend 期间渲染暂停）。
            //
            // ⚠️ 基准点 `last_instant` 必须取在**本轮探活结束之后**（见循环末尾），
            // 即只度量 sleep 间隔。若按直觉取在本行（sleep 之后、探活之前），
            // 上一轮探活的超时耗时（最长 5s）会叠加进下一轮间隔，使 15s 变成
            // 20.008s > 20s —— **每次探活超时都必然误报**为「系统休眠唤醒」，
            // 进而执行一次不必要的 resume_webview（唤醒本应休眠的弹窗）。
            let elapsed = Instant::now().duration_since(last_instant);
            if elapsed > std::time::Duration::from_secs(20) {
                standard_log!(
                    "[watchdog] time jump: {:.1}s — resuming webview",
                    elapsed.as_secs_f64()
                );
                if let Some(popup_win) = app_handle.get_webview_window("popup") {
                    let wv: &tauri::Webview = popup_win.as_ref();
                    crate::webview::resume_webview(wv);
                }
            }

            let probe_app = app_handle.clone();
            let (tx, rx) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                let result = probe_app
                    .get_webview_window("popup")
                    .map(|w| w.is_visible().is_ok())
                    .unwrap_or(true);
                let _ = tx.send(result);
            });
            match rx.recv_timeout(std::time::Duration::from_secs(5)) {
                Ok(true) => stuck_streak = 0,
                _ => {
                    stuck_streak += 1;
                    standard_log!(
                        "[watchdog] event loop unresponsive, streak={}",
                        stuck_streak
                    );
                    if stuck_streak >= 2 {
                        watchdog_self_restart();
                    }
                }
            }

            // 下一轮的时间基准：置于探活之后，使 elapsed 只含 sleep 间隔（~15s）
            last_instant = Instant::now();
        }
    });
}

/// 处理窗口事件：弹窗失焦关闭、设置窗延迟销毁、弹窗关闭改为隐藏。
/// 事件线程只做轻量判断，阻塞操作移入子线程。
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
            standard_log!("[main] CoInitializeEx failed: 0x{:08X}", hr);
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
        .on_window_event(|window, event| handle_window_event(window, event));
    let app = match builder.build(tauri::generate_context!()) {
        Ok(app) => app,
        Err(e) => {
            show_error_box(&format!("应用初始化失败：\n{}", e));
            // 日志已改为独立写线程（B5），退出前须等队列排空，否则启动期日志会丢
            crate::process::flush_log();
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
        RunEvent::Exit => crate::process::flush_log(),
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

    fn recorder() -> (DialogLog, Box<dyn Fn(&str) + Send + Sync>) {
        let log: DialogLog = Arc::new(Mutex::new(Vec::new()));
        let handle = log.clone();
        let f: Box<dyn Fn(&str) + Send + Sync> =
            Box::new(move |msg: &str| handle.lock().unwrap().push(msg.to_string()));
        (log, f)
    }

    /// 取一个确定与当前线程不同的线程 id。
    fn other_thread_id() -> std::thread::ThreadId {
        std::thread::spawn(|| std::thread::current().id())
            .join()
            .expect("取线程 id 的线程不应 panic")
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
        let got = log.lock().unwrap();
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
            log.lock().unwrap().is_empty(),
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
        let _serial = HOOK_LOCK.lock().unwrap_or_else(|e| e.into_inner());

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
            log.lock().unwrap().is_empty(),
            "后台线程 panic 不得走弹框分支（走了会卡死该线程）"
        );
    }

    /// `#[track_caller]` 包装：让 `Location::caller()` 返回调用点的位置。
    #[track_caller]
    fn location_here() -> &'static panic::Location<'static> {
        panic::Location::caller()
    }
}
