use std::sync::atomic::Ordering;
use std::sync::{Mutex, OnceLock};
use tauri::{
    image::Image,
    menu::{Menu, MenuItem, PredefinedMenuItem, Submenu},
    tray::{TrayIcon, TrayIconBuilder},
    Listener,
};

use crate::audio;
use crate::config;
use crate::popup;
use crate::state::{get_devices_cache, AUTO_MENU_ITEM, AUTO_START, TRAY_POS};
use crate::windows;

static TRAY_ICON: OnceLock<Mutex<Option<TrayIcon<tauri::Wry>>>> = OnceLock::new();

/// 刷新设备缓存，返回是否发生变化。
/// 查询失败（WMI 不可信态）时跳过本轮，保留旧缓存避免 tooltip 抖动
fn refresh_devices_cache() -> bool {
    let new_devices = match crate::wmi_query::query_devices(false) {
        Ok(d) => d,
        Err(e) => {
            crate::process::append_log(&format!("[tray] skip cache refresh: {}", e));
            return false;
        }
    };
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
fn build_tooltip_text() -> String {
    let cache = get_devices_cache();
    let devices = crate::state::lock_unpoisoned(cache);

    let mut lines = Vec::new();
    config::with_config(|c| {
        for tray_name in &c.tray_devices {
            if let Some(dev) = devices.iter().find(|d| &d.name == tray_name) {
                let display_name = c.device_names.get(&dev.name).unwrap_or(&dev.name);
                let dot = if dev.status == crate::wmi_query::BT_STATUS_CONNECTED {
                    "🟢"
                } else {
                    "⚪"
                };
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
/// 并向弹窗推送 devices-changed（卡片实时增删，无需等焦点）
fn start_device_watcher(app: &tauri::AppHandle) {
    use tauri::Emitter;

    let handle = app.clone();
    std::thread::spawn(move || loop {
        // 每轮重新读取间隔（支持运行时修改）
        let secs = config::with_config(|c| c.low_battery_refresh_secs.max(10));
        std::thread::sleep(std::time::Duration::from_secs(secs as u64));

        let has_tray = config::with_config(|c| !c.tray_devices.is_empty());
        let has_battery_notify = config::with_config(|c| c.low_battery_notify);
        if !has_tray && !has_battery_notify {
            continue;
        }

        let changed = refresh_devices_cache();
        if has_tray && changed {
            std::thread::spawn(move || update_tooltip());
            let _ = handle.emit("devices-changed", ());
        }

        // 低电量通知检查
        if has_battery_notify {
            let cache = get_devices_cache();
            if let Ok(guard) = cache.lock() {
                crate::battery_notify::check_battery_notify(&guard);
            }
        }
    });
}

/// 根据默认打开页面与系统深色模式选择托盘图标
fn pick_tray_icon() -> Image<'static> {
    let is_volume = config::with_config(|c| c.default_popup_tab == "volume");
    let dark = crate::windows::system_dark_mode();
    if is_volume {
        let bytes = if dark {
            include_bytes!("../icons/tray-volume-icon-dark.png").to_vec()
        } else {
            include_bytes!("../icons/tray-volume-icon.png").to_vec()
        };
        Image::from_bytes(&bytes).expect("Failed to load tray volume icon")
    } else {
        let bytes = if dark {
            include_bytes!("../icons/tray-icon-dark.png").to_vec()
        } else {
            include_bytes!("../icons/tray-icon.png").to_vec()
        };
        Image::from_bytes(&bytes).expect("Failed to load tray icon")
    }
}

/// 后台线程：监听系统深色模式变化并刷新托盘图标（仅跟随系统）
/// 通过 RegNotifyChangeKeyValue 注册表变更通知实现事件驱动，无轮询
fn start_theme_watcher() {
    use windows_sys::Win32::System::Registry::{
        RegCloseKey, RegNotifyChangeKeyValue, RegOpenKeyExW, HKEY_CURRENT_USER, KEY_READ,
        REG_NOTIFY_CHANGE_LAST_SET,
    };
    use windows_sys::Win32::System::Threading::{CreateEventW, WaitForSingleObject, INFINITE};

    std::thread::spawn(move || {
        unsafe {
            let mut hkey = std::ptr::null_mut();
            let status = RegOpenKeyExW(
                HKEY_CURRENT_USER,
                windows_sys::core::w!(
                    "Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize"
                ),
                0,
                KEY_READ,
                &mut hkey,
            );
            if status != 0 {
                return;
            }

            let event = CreateEventW(std::ptr::null(), 1, 0, std::ptr::null());
            if event.is_null() {
                RegCloseKey(hkey);
                return;
            }

            let mut last = crate::windows::system_dark_mode();
            loop {
                let status = RegNotifyChangeKeyValue(hkey, 0, REG_NOTIFY_CHANGE_LAST_SET, event, 1);
                if status != 0 {
                    // 通知注册失败时退避重试，避免线程空转
                    std::thread::sleep(std::time::Duration::from_secs(3));
                    continue;
                }
                WaitForSingleObject(event, INFINITE);
                let current = crate::windows::system_dark_mode();
                if current != last {
                    last = current;
                    std::thread::spawn(move || update_tray_icon());
                }
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
        let _ = if wanted {
            autostart.enable()
        } else {
            autostart.disable()
        };
    }

    // 构建音频设备切换子菜单
    let audio_devices_menu = build_audio_devices_menu(app.handle())?;

    let menu = build_full_menu(app.handle(), &audio_devices_menu)?;

    let tray_icon = pick_tray_icon();

    let _tray = TrayIconBuilder::with_id("main-tray")
        .icon(tray_icon)
        .tooltip("外设监控")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(move |app, event| {
            crate::process::append_log(&format!("[tray] menu: {}", event.id.as_ref()));
            // 菜单事件在事件线程上分发：重操作（窗口/材质/DWM）一律 spawn 移出，
            // 比照 audio_dev_ 分支的既有模式
            match event.id.as_ref() {
                "show" => {
                    let app = app.clone();
                    std::thread::spawn(move || crate::popup::open_popup(&app, "devices"));
                }
                "volume" => {
                    let app = app.clone();
                    std::thread::spawn(move || crate::popup::open_popup(&app, "volume"));
                }
                "settings" => {
                    let app = app.clone();
                    std::thread::spawn(move || windows::open_settings(&app));
                }
                "about" => {
                    let app = app.clone();
                    std::thread::spawn(move || windows::open_settings_tab(&app, "about"));
                }
                "auto_start" => {
                    let old = AUTO_START.load(Ordering::Relaxed);
                    let new_val = !old;
                    AUTO_START.store(new_val, Ordering::Relaxed);
                    config::with_config_mut(|c| c.auto_start = new_val);
                    let autostart = app.autolaunch();
                    let _ = if new_val {
                        autostart.enable()
                    } else {
                        autostart.disable()
                    };
                    update_auto_text();
                    crate::process::append_log(&format!("[tray] auto_start toggled: {}", new_val));
                }
                "exit" => {
                    app.exit(0);
                }
                id if id.starts_with("audio_dev_") => {
                    let device_id = id[10..].to_owned();
                    if !device_id.is_empty() {
                        crate::process::append_log(&format!(
                            "[tray] set_default_device: {}",
                            device_id
                        ));
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
                button,
                button_state,
                rect,
                ..
            } = event
            {
                if button_state != tauri::tray::MouseButtonState::Up {
                    return;
                }
                if button == tauri::tray::MouseButton::Left {
                    crate::process::append_log("[tray] click → spawn");
                    // 事件线程仅做分发：显示器枚举/配置读取/窗口操作全部移出，
                    // 防止唤醒后子窗口消息队列卡死拖垮整个事件循环
                    let app = app.clone();
                    let rect = rect;
                    std::thread::spawn(move || {
                        if let Some(pos) = TRAY_POS.get() {
                            // 物理坐标需整体转逻辑：仅除 x 会让 y 携带物理值，
                            // 在缩放屏上把弹出窗底边推出屏幕外。
                            // 混合 DPI 时须按托盘所在屏的 SF 换算（主屏 SF 会定位偏移）。
                            let (px, py) = match rect.position {
                                tauri::Position::Physical(p) => {
                                    let (x, y) = (p.x as f64, p.y as f64);
                                    let info = windows::monitor_info_at(&app, x, y);
                                    let sf = info
                                        .as_ref()
                                        .map(|i| i.scale_factor)
                                        .unwrap_or_else(|| windows::scale_factor(&app));
                                    if let Some(i) = info {
                                        *crate::state::lock_unpoisoned(
                                            crate::state::get_tray_monitor(),
                                        ) = Some(i);
                                    }
                                    (x / sf, y / sf)
                                }
                                tauri::Position::Logical(p) => (p.x, p.y),
                            };
                            *crate::state::lock_unpoisoned(pos) = (px, py);
                        }
                        let tab = config::with_config(|c| c.default_popup_tab.clone());
                        popup::toggle(&app, &tab);
                    });
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
        let screen_w = handle
            .primary_monitor()
            .ok()
            .flatten()
            .map(|m| m.size().width as f64 / sf)
            .unwrap_or(1920.0);
        let screen_h = handle
            .primary_monitor()
            .ok()
            .flatten()
            .map(|m| m.size().height as f64 / sf)
            .unwrap_or(1080.0);
        Mutex::new((screen_w - 300.0, screen_h - 50.0))
    });

    // 首启即定位任务栏所在屏与通知区锚点：托盘尚未被点击前，弹窗据此确定所在屏、
    // 工作区与落点，避免 fallback 主屏/假锚点导致副屏/混合 DPI 首启错位（托盘点击后再精确纠偏）。
    if let Some((info, anchor_x, anchor_y)) = windows::monitor_info_of_taskbar(app.handle()) {
        *crate::state::lock_unpoisoned(crate::state::get_tray_monitor()) = Some(info);
        if let Some(pos) = TRAY_POS.get() {
            *crate::state::lock_unpoisoned(pos) = (anchor_x, anchor_y);
        }
    }

    app.listen("config-changed", move |_| {
        let new_auto = config::with_config(|c| c.auto_start);
        AUTO_START.store(new_auto, Ordering::Relaxed);
        update_auto_text();
        update_tray_icon();
        update_audio_devices_menu();
        // config 变更时立即刷新设备缓存和 tooltip（异步避免阻塞主线程）
        std::thread::spawn(|| {
            refresh_devices_cache();
            update_tooltip();
        });
    });

    app.listen("tray-devices-changed", move |_| {
        std::thread::spawn(move || update_tooltip());
    });

    app.listen("audio-devices-changed", |_| {
        update_audio_devices_menu();
    });

    // 启动后台设备监控线程
    start_device_watcher(app.handle());
    // 启动系统深色模式监听线程（仅跟随系统）
    start_theme_watcher();

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

/// 根据默认打开页面与系统深色模式更新托盘图标
fn update_tray_icon() {
    let icon = pick_tray_icon();
    let guard = crate::state::lock_unpoisoned(TRAY_ICON.get().unwrap());
    if let Some(ref tray) = *guard {
        let _ = tray.set_icon(Some(icon));
    }
}

/// 简化设备名称：仅保留括号内内容，如 "耳机 (小爱音箱-9205)" -> "小爱音箱-9205"
/// 注意：与 dedup::core_name 语义不同——本函数不剥协议后缀、返回 &str，两者勿互相替换。
fn simplify_device_name(name: &str) -> &str {
    if let Some(open) = name.find('(') {
        if let Some(close) = name.rfind(')') {
            if close > open {
                let inner = name[open + 1..close].trim();
                if !inner.is_empty() {
                    return inner;
                }
            }
        }
    }
    name
}

/// 构建音频设备切换子菜单
fn build_audio_devices_menu(
    app: &tauri::AppHandle,
) -> Result<Submenu<tauri::Wry>, Box<dyn std::error::Error>> {
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
                let display = c
                    .device_names
                    .get(&device.name)
                    .cloned()
                    .unwrap_or_else(|| {
                        if c.simplify_device_names {
                            simplify_device_name(&device.name).to_string()
                        } else {
                            device.name.clone()
                        }
                    });
                let label = format!("{}{}", display, check);
                let item = MenuItem::with_id(
                    app,
                    format!("audio_dev_{}", device.id),
                    label,
                    true,
                    None::<&str>,
                );
                if let Ok(item) = item {
                    let _ = submenu.append(&item);
                }
            }
        });
    }
    Ok(submenu)
}

/// 更新音频设备切换子菜单（在设备列表变化时调用）
fn update_audio_devices_menu() {
    let tray_guard = crate::state::lock_unpoisoned(TRAY_ICON.get().unwrap());
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
}

/// 构建 Windows 声音设置子菜单
fn build_windows_sound_settings_menu(
    app: &tauri::AppHandle,
) -> Result<Submenu<tauri::Wry>, Box<dyn std::error::Error>> {
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
