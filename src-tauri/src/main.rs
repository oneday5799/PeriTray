#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
#![warn(unused_imports, dead_code)]

mod app_icon;
mod audio;
mod audio_notify;
mod audio_policy;
mod audio_spatial;
mod battery_notify;
mod bluetooth;
mod bt_audio;
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

/// 安装 panic hook：捕获 panic 后弹 MessageBox 再退出（避免 release 模式静默闪退）
fn install_panic_hook() {
    let default_hook = panic::take_hook();
    panic::set_hook(Box::new(move |info| {
        let payload = info.payload();
        let msg = if let Some(s) = payload.downcast_ref::<&str>() {
            s.to_string()
        } else if let Some(s) = payload.downcast_ref::<String>() {
            s.clone()
        } else {
            "Unknown panic".to_string()
        };
        let location = info
            .location()
            .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
            .unwrap_or_else(|| "unknown location".to_string());
        let full = format!("{}\n\nLocation: {}", msg, location);
        process::append_log(&format!(
            "[panic] {} @ {}",
            msg.replace('\n', " | "),
            location
        ));
        show_error_box(&full);
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
            process::append_log(&format!("[single-instance] popup exists, open tab={}", tab));
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
                let status = if info.has_update { "update" } else { "latest" };
                let payload = crate::update::UpdateStatus::from_info(&info, status);
                let _ = app_handle.emit("update-status", payload);
                if info.has_update {
                    let _ = app_handle.emit("update-available", info.clone());
                    // Windows 原生通知（带图标 + 点击跳转关于页）
                    #[cfg(target_os = "windows")]
                    {
                        let ico_path = crate::windows::resolve_toast_icon();
                        let app = app_handle.clone();
                        let toast = crate::windows::build_toast(
                            "发现新版本",
                            &format!("发现新版本 v{}，点击查看详情", info.latest_version),
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
                            crate::process::append_log(&format!("[update] toast failed: {:?}", e));
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
            let now = Instant::now();
            let elapsed = now.duration_since(last_instant);
            last_instant = now;
            if elapsed > std::time::Duration::from_secs(20) {
                crate::process::append_log(&format!(
                    "[watchdog] time jump: {:.1}s — resuming webview",
                    elapsed.as_secs_f64()
                ));
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
                    crate::process::append_log(&format!(
                        "[watchdog] event loop unresponsive, streak={}",
                        stuck_streak
                    ));
                    if stuck_streak >= 2 {
                        watchdog_self_restart();
                    }
                }
            }
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
                std::thread::spawn(move || popup::close_popup(&app));
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
        process::append_log(&format!(
            "[watchdog] restart mode, waited old pid={} exit",
            old_pid
        ));
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
            process::append_log(&format!("[main] CoInitializeEx failed: 0x{:08X}", hr));
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
            // 注册 AUMID，使 Windows 通知显示应用图标
            #[cfg(target_os = "windows")]
            crate::windows::register_aumid();

            if let Err(e) = tray::setup_tray(app) {
                process::append_log(&format!("[main] setup_tray failed: {}", e));
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
            std::process::exit(1);
        }
    };
    app.run(|_app_handle, event| {
        if let RunEvent::ExitRequested { .. } = event {
            crate::audio_notify::request_shutdown();
        }
    });
}
