#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
#![warn(unused_imports, dead_code)]

mod audio;
mod audio_notify;
mod app_icon;
mod bluetooth;
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
mod windows;
mod wmi_query;

use std::panic;

use tauri::Manager;
use tauri::Emitter;

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
        show_error_box(&full);
        default_hook(info);
    }));
}

fn show_error_box(msg: &str) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_OK, MB_ICONERROR};
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

fn main() {
    // 先初始化配置（panic hook 和日志都依赖配置）
    config::init_config();

    install_panic_hook();

    // Init COM with apartment-threaded mode (same as Tauri) BEFORE Tauri starts.
    // This lets wmi use COMLibrary::assume_initialized() instead of re-initializing.
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

    tauri::Builder::default()
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            Some(vec!["--autostart"]),
        ))
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            if app.get_webview_window("popup").is_some() {
                popup::open_popup(app, "devices");
            }
        }))
        .plugin(tauri_plugin_window_state::Builder::default().build())
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .invoke_handler(tauri::generate_handler![
            commands::get_devices,
            commands::get_config,
            commands::update_config,
            commands::toggle_device_hidden,
            commands::toggle_audio_device_hidden,
            commands::open_settings,
            commands::exit_app,
            commands::close_window,
            commands::rename_device,
            commands::change_device_group,
            commands::toggle_group_hidden,
            commands::connect_bluetooth_device,
            commands::disconnect_bluetooth_device,
            commands::check_bt_connection,
            commands::open_bt_settings,
            commands::open_url,
            commands::open_24g_device_file,
            commands::toggle_device_tray,
            commands::get_tray_tooltip,
            commands::get_audio_devices,
            commands::set_device_volume,
            commands::toggle_device_mute,
            commands::get_audio_sessions,
            commands::set_session_volume,
            commands::toggle_session_mute,
            commands::adjust_volume_up,
            commands::adjust_volume_down,
            commands::toggle_mute,
            commands::set_default_device,
            commands::open_log_dir,
            commands::check_for_update,
            commands::set_hotkey_config,
            commands::set_device_shortcut,
            commands::remove_device_shortcut,
        ])
        .setup(move |app| {
            if let Err(e) = tray::setup_tray(app) {
                process::append_log(&format!("[main] setup_tray failed: {}", e));
            }
            // 初始化音频通知回调（替代轮询）
            crate::audio_notify::init_audio_notify(app.handle().clone());
            process::append_log("[main] audio_notify initialized");
            crate::shortcut::register_shortcuts(app.handle());
            if !is_autostart {
                popup::open_popup(app.handle(), "devices");
            }

            // 启动时检测更新（仅非 autostart 模式）
            if !is_autostart && config::with_config(|c| c.check_updates) {
                let app_handle = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    tokio::time::sleep(std::time::Duration::from_secs(3)).await;
                    let include = config::with_config(|c| c.include_prerelease);
                    let current_version = app_handle.package_info().version.to_string();
                    let result = tokio::task::spawn_blocking(move || {
                        crate::update::check_for_update(&current_version, include)
                    })
                    .await;
                    match result {
                        Ok(Ok(info)) if info.has_update => {
                            let _ = app_handle.emit("update-available", info);
                        }
                        Ok(Err(e)) => {
                            process::append_log(&format!("[update] startup check failed: {}", e));
                        }
                        Err(e) => {
                            process::append_log(&format!("[update] task failed: {}", e));
                        }
                        _ => {}
                    }
                });
            }

            process::append_log("[main] startup complete");
            Ok(())
        })
        .on_window_event(|window, event| {
            match event {
                tauri::WindowEvent::Focused(false) => {
                    if window.label() == "popup"
                        && !state::ANIMATING.load(std::sync::atomic::Ordering::Relaxed)
                        && window.is_visible().unwrap_or(false)
                    {
                        let app = window.app_handle();
                        popup::close_popup(app);
                    }
                }
                tauri::WindowEvent::CloseRequested { api, .. } => {
                    let label = window.label();
                    if label == "settings" || label == "about" {
                        api.prevent_close();
                        let _ = window.destroy();
                    } else if label == "popup" {
                        api.prevent_close();
                        let _ = window.hide();
                    }
                }
                _ => {}
            }
        })
        .build(tauri::generate_context!())
        .unwrap_or_else(|e| {
            show_error_box(&format!("应用初始化失败：\n{}", e));
            std::process::exit(1);
        })
        .run(|_app_handle, _event| {});
}
