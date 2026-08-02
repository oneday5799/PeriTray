use std::sync::atomic::Ordering;
use std::sync::{Mutex, OnceLock};
use tauri::{
    image::Image,
    menu::{Menu, MenuItem, PredefinedMenuItem, Submenu},
    tray::{TrayIcon, TrayIconBuilder},
    Emitter, Listener,
};

use crate::config;
use crate::popup;
use crate::windows;
use crate::state::{TRAY_POS, AUTO_START, AUTO_MENU_ITEM, get_devices_cache};
use crate::audio;

static TRAY_ICON: OnceLock<Mutex<Option<TrayIcon<tauri::Wry>>>> = OnceLock::new();
static AUDIO_DEVICES_SUBMENU: OnceLock<Mutex<Option<Submenu<tauri::Wry>>>> = OnceLock::new();

/// 刷新设备缓存，返回是否发生变化
fn refresh_devices_cache() -> bool {
    let new_devices = crate::wmi_query::query_devices();
    let cache = get_devices_cache();

    if let Ok(mut guard) = cache.lock() {
        if *guard != new_devices {
            *guard = new_devices;
            true
        } else {
            false
        }
    } else {
        false
    }
}

/// 根据缓存的设备信息构建 tooltip 文本
pub fn build_tooltip_text() -> String {
    let cache = get_devices_cache();
    let devices = cache.lock().unwrap_or_else(|e| e.into_inner());

    let mut lines = Vec::new();
    config::with_config(|c| {
        for tray_name in &c.tray_devices {
            if let Some(dev) = devices.iter().find(|d| &d.name == tray_name) {
                let display_name = c.device_names.get(&dev.name).unwrap_or(&dev.name);
                let dot = if dev.status == "已连接" { "🟢" } else { "⚪" };
                match dev.battery {
                    Some(battery) => lines.push(format!("{} {} - {}%", dot, display_name, battery)),
                    None => lines.push(format!("{} {}", dot, display_name)),
                }
            }
        }
    });

    if lines.is_empty() {
        "外设监控".to_string()
    } else {
        lines.join("\n")
    }
}

/// 更新托盘 tooltip
fn update_tooltip() {
    let tooltip = build_tooltip_text();

    if let Ok(guard) = TRAY_ICON.get_or_init(|| Mutex::new(None)).lock() {
        if let Some(ref tray) = *guard {
            let _ = tray.set_tooltip(Some(tooltip));
        }
    }
}

/// 后台刷新线程：定期查询设备并更新缓存，状态变化时自动更新 tooltip
fn start_device_watcher() {
    std::thread::spawn(move || {
        loop {
            std::thread::sleep(std::time::Duration::from_secs(10));

            if !config::with_config(|c| !c.tray_devices.is_empty()) {
                continue;
            }

            let changed = refresh_devices_cache();
            if changed {
                std::thread::spawn(move || update_tooltip());
            }
        }
    });
}

pub fn refresh_tray_tooltip(_app_handle: &tauri::AppHandle) {
    refresh_devices_cache();
    update_tooltip();
}

pub fn init_auto_start() {
    AUTO_START.store(config::with_config(|c| c.auto_start), Ordering::Relaxed);
}

/// 构建完整的顶层菜单
fn build_full_menu(
    app: &tauri::AppHandle,
    audio_devices_menu: &Submenu<tauri::Wry>,
) -> Result<Menu<tauri::Wry>, Box<dyn std::error::Error>> {
    let auto_text = if AUTO_START.load(Ordering::Relaxed) {
        "开机自启 ✓"
    } else {
        "开机自启"
    };
    let show_i = MenuItem::with_id(app, "show", "设备信息", true, None::<&str>)?;
    let volume_i = MenuItem::with_id(app, "volume", "音量控制", true, None::<&str>)?;
    let settings_i = MenuItem::with_id(app, "settings", "设置", true, None::<&str>)?;
    let about_i = MenuItem::with_id(app, "about", "关于", true, None::<&str>)?;
    let auto_i = MenuItem::with_id(app, "auto_start", auto_text, true, None::<&str>)?;
    let exit_i = MenuItem::with_id(app, "exit", "退出", true, None::<&str>)?;
    let win_sound_menu = build_windows_sound_settings_menu(app)?;
    let _ = AUTO_MENU_ITEM.get_or_init(|| Mutex::new(None));
    if let Ok(mut guard) = AUTO_MENU_ITEM.get().unwrap().lock() {
        *guard = Some(auto_i.clone());
    }

    let sep1 = PredefinedMenuItem::separator(app)?;
    let sep2 = PredefinedMenuItem::separator(app)?;
    let sep3 = PredefinedMenuItem::separator(app)?;

    let menu = Menu::with_items(
        app,
        &[
            &show_i,
            &volume_i,
            &sep1,
            audio_devices_menu,
            &win_sound_menu,
            &sep2,
            &auto_i,
            &sep3,
            &settings_i,
            &about_i,
            &exit_i,
        ],
    )?;
    Ok(menu)
}

pub fn setup_tray(app: &tauri::App) -> Result<(), Box<dyn std::error::Error>> {
    use tauri_plugin_autostart::ManagerExt;
    let autostart = app.autolaunch();
    let current = autostart.is_enabled().unwrap_or(false);
    let wanted = AUTO_START.load(Ordering::Relaxed);
    if wanted != current {
        let _ = if wanted { autostart.enable() } else { autostart.disable() };
    }

    // 构建音频设备切换子菜单
    let audio_devices_menu = build_audio_devices_menu(app.handle())?;
    let _ = AUDIO_DEVICES_SUBMENU.get_or_init(|| Mutex::new(Some(audio_devices_menu.clone())));

    let menu = build_full_menu(app.handle(), &audio_devices_menu)?;

    let _tray = TrayIconBuilder::with_id("main-tray")
        .icon(Image::from_bytes(include_bytes!("../icons/tray-icon.png"))
            .expect("Failed to load tray icon"))
        .tooltip("外设监控")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(move |app, event| {
            match event.id.as_ref() {
                "show" => { crate::popup::open_popup(app, "devices"); }
                "volume" => { crate::popup::open_popup(app, "volume"); }
                "settings" => { windows::open_settings(app); }
                "about" => { windows::open_about(app); }
                "auto_start" => {
                    let old = AUTO_START.load(Ordering::Relaxed);
                    let new_val = !old;
                    AUTO_START.store(new_val, Ordering::Relaxed);
                    config::with_config_mut(|c| c.auto_start = new_val);
                    let autostart = app.autolaunch();
                    let _ = if new_val { autostart.enable() } else { autostart.disable() };
                    update_auto_text();
                    crate::process::append_log(&format!("[tray] auto_start toggled: {}", new_val));
                    let _ = app.emit("auto-start-changed", new_val);
                }
                "exit" => { app.exit(0); }
                id if id.starts_with("audio_dev_") => {
                    let device_id = id[10..].to_owned();
                    if !device_id.is_empty() {
                        crate::process::append_log(&format!("[tray] set_default_device: {}", device_id));
                        std::thread::spawn(move || {
                            let _ = audio::set_default_device(&device_id);
                            update_audio_devices_menu();
                        });
                    }
                }
                "win_sound_volume_mixer" => {
                    let _ = crate::process::open_with_system("sndvol.exe");
                }
                "win_sound_playback" => {
                    crate::process::open_sound_panel("playback");
                }
                "win_sound_recording" => {
                    crate::process::open_sound_panel("recording");
                }
                "win_sound_sounds" => {
                    crate::process::open_sound_panel("sounds");
                }
                "win_sound_settings" => {
                    crate::process::open_settings_page("sound");
                }
                "win_sound_app_volume" => {
                    crate::process::open_settings_page("apps-volume");
                }
                _ => {}
            }
        })
        .on_tray_icon_event(|tray, event| {
            let app = tray.app_handle();
            if let tauri::tray::TrayIconEvent::Click {
                button, button_state, rect, ..
            } = event {
                if let Some(pos) = TRAY_POS.get() {
                    let sf = windows::scale_factor(app);
                    let (px, py) = match rect.position {
                        tauri::Position::Physical(p) => (p.x as f64 / sf, p.y as f64 / sf),
                        tauri::Position::Logical(p) => (p.x, p.y),
                    };
                    *pos.lock().unwrap() = (px, py);
                }

                if button_state != tauri::tray::MouseButtonState::Up {
                    return;
                }
                if button == tauri::tray::MouseButton::Left {
                    popup::toggle(app, "devices");
                }
            }
        })
        .build(app)?;

    if let Ok(mut guard) = TRAY_ICON.get_or_init(|| Mutex::new(None)).lock() {
        *guard = Some(_tray);
    }

    let _ = TRAY_POS.get_or_init(|| {
        let handle = app.handle();
        let sf = windows::scale_factor(handle);
        let screen_w = handle.primary_monitor()
            .ok()
            .flatten()
            .map(|m| m.size().width as f64 / sf)
            .unwrap_or(1920.0);
        let screen_h = handle.primary_monitor()
            .ok()
            .flatten()
            .map(|m| m.size().height as f64 / sf)
            .unwrap_or(1080.0);
        Mutex::new((screen_w - 300.0, screen_h - 50.0))
    });

    app.listen("config-changed", move |_| {
        let new_auto = config::with_config(|c| c.auto_start);
        AUTO_START.store(new_auto, Ordering::Relaxed);
        update_auto_text();
    });

    app.listen("tray-devices-changed", move |_| {
        std::thread::spawn(move || update_tooltip());
    });

    app.listen("audio-devices-changed", |_| {
        update_audio_devices_menu();
    });

    // 启动后台设备监控线程
    start_device_watcher();

    Ok(())
}

fn update_auto_text() {
    if let Some(item) = AUTO_MENU_ITEM.get() {
        if let Ok(guard) = item.lock() {
            if let Some(ref mi) = *guard {
                let text = if AUTO_START.load(Ordering::Relaxed) {
                    "开机自启 ✓"
                } else {
                    "开机自启"
                };
                let _ = mi.set_text(text);
            }
        }
    }
}

/// 构建音频设备切换子菜单
fn build_audio_devices_menu(app: &tauri::AppHandle) -> Result<Submenu<tauri::Wry>, Box<dyn std::error::Error>> {
    let submenu = Submenu::with_id(app, "audio_devices", "音频设备", true)?;
    let devices = audio::enumerate_output_devices().unwrap_or_default();
    if devices.is_empty() {
        let empty = MenuItem::with_id(app, "audio_dev_empty", "无音频设备", false, None::<&str>)?;
        submenu.append(&empty)?;
    } else {
        config::with_config(|c| {
            for device in &devices {
                if c.hidden_audio_devices.contains(&device.name) {
                    continue;
                }
                let check = if device.is_default { " ✓" } else { "" };
                let display = c.device_names.get(&device.name).unwrap_or(&device.name);
                let label = format!("{}{}", display, check);
                let item = MenuItem::with_id(app, format!("audio_dev_{}", device.id), label, true, None::<&str>);
                if let Ok(item) = item {
                    let _ = submenu.append(&item);
                }
            }
        });
    }
    Ok(submenu)
}

/// 更新音频设备切换子菜单（在设备列表变化时调用）
pub fn update_audio_devices_menu() {
    let Ok(tray_guard) = TRAY_ICON.get().unwrap().lock() else { return };
    let Some(ref tray) = *tray_guard else { return };
    let app = tray.app_handle().clone();

    let new_submenu = match build_audio_devices_menu(&app) {
        Ok(s) => s,
        Err(_) => return,
    };

    if let Ok(menu) = build_full_menu(&app, &new_submenu) {
        let _ = tray.set_menu(Some(menu));
    }

    drop(tray_guard);
    if let Some(submenu_lock) = AUDIO_DEVICES_SUBMENU.get() {
        if let Ok(mut guard) = submenu_lock.lock() {
            *guard = Some(new_submenu);
        }
    }
}

/// 构建 Windows 声音设置子菜单
fn build_windows_sound_settings_menu(app: &tauri::AppHandle) -> Result<Submenu<tauri::Wry>, Box<dyn std::error::Error>> {
    let submenu = Submenu::with_id(app, "win_sound", "声音设置", true)?;
    let items = [
        ("win_sound_volume_mixer", "音量合成器 (Classic)"),
        ("win_sound_playback", "播放设备 (Classic)"),
        ("win_sound_recording", "录制设备 (Classic)"),
        ("win_sound_sounds", "声音 (Classic)"),
        ("win_sound_settings", "声音设置"),
        ("win_sound_app_volume", "音量合成器"),
    ];
    for (id, label) in items {
        let item = MenuItem::with_id(app, id, label, true, None::<&str>)?;
        submenu.append(&item)?;
    }
    Ok(submenu)
}
