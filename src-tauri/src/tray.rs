use crate::standard_log;
use std::sync::atomic::Ordering;
use std::sync::{Mutex, OnceLock};
use tauri::{
    image::Image,
    menu::{Menu, MenuItem, PredefinedMenuItem, Submenu},
    tray::{TrayIcon, TrayIconBuilder},
    Emitter, Listener,
};

use crate::audio;
use crate::config;
use crate::popup;
use crate::state::{get_devices_cache, AUTO_MENU_ITEM, AUTO_START, TRAY_POS};
use crate::windows;

static TRAY_ICON: OnceLock<Mutex<Option<TrayIcon<tauri::Wry>>>> = OnceLock::new();

/// 将查询结果写回设备缓存，返回是否发生变化（新旧列表比较）。
fn apply_devices_cache(new_devices: Vec<crate::device::Device>) -> bool {
    let cache = get_devices_cache();
    // P2-11：中毒时不再静默返回 false —— 那会让调用方认为「设备列表没变」，
    // 于是 tooltip 与弹窗卡片此后再也不刷新（无日志的哑故障）。
    let mut guard = crate::state::lock_unpoisoned(cache);
    if *guard != new_devices {
        *guard = new_devices;
        true
    } else {
        false
    }
}

/// 刷新设备缓存，返回是否发生变化。
/// 查询失败（WMI 不可信态）时跳过本轮，保留旧缓存避免 tooltip 抖动
fn refresh_devices_cache() -> bool {
    match crate::wmi_query::query_devices(false) {
        Ok(d) => apply_devices_cache(d),
        Err(e) => {
            standard_log!("[tray] skip cache refresh: {}", e);
            false
        }
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

    // 先取句柄再释放锁：`set_tooltip` 内部同步等主线程，
    // 持 TRAY_ICON 调用会与「主线程等 TRAY_ICON」构成 AB/BA 死锁（详见 build_audio_devices_menu 注释）
    let tray = {
        let guard = crate::state::lock_unpoisoned(TRAY_ICON.get_or_init(|| Mutex::new(None)));
        match *guard {
            Some(ref tray) => tray.clone(),
            None => return,
        }
    };
    let _ = tray.set_tooltip(Some(tooltip));
}

/// 后台刷新线程：定期查询设备并更新缓存，状态变化时自动更新 tooltip
/// 并向弹窗推送 devices-changed（卡片实时增删，无需等焦点）
fn start_device_watcher(app: &tauri::AppHandle) {
    use tauri::Emitter;

    let handle = app.clone();
    std::thread::spawn(move || loop {
        // 连接在线程内建立一次并复用（WMIConnection 为 !Send，须在此线程内持有）；
        // 建立失败退避重试，查询失败跳出重建连接以自愈
        let con = match wmi::WMIConnection::new() {
            Ok(c) => c,
            Err(e) => {
                standard_log!("[tray] WMIConnection::new failed: {}", e);
                std::thread::sleep(std::time::Duration::from_secs(10));
                continue;
            }
        };

        loop {
            // 每轮重新读取间隔（支持运行时修改）
            let secs = config::with_config(|c| c.low_battery_refresh_secs.max(10));
            std::thread::sleep(std::time::Duration::from_secs(secs as u64));

            let has_tray = config::with_config(|c| !c.tray_devices.is_empty());
            let has_battery_notify = config::with_config(|c| c.low_battery_notify);
            if !has_tray && !has_battery_notify {
                continue;
            }

            let changed = match crate::wmi_query::query_devices_with(&con, false) {
                Ok(d) => apply_devices_cache(d),
                Err(e) => {
                    standard_log!("[tray] skip cache refresh: {}", e);
                    break; // 连接可能失效，跳出重建
                }
            };
            if has_tray && changed {
                crate::process::append_verbose_log("[tray] 设备缓存变化，更新 tooltip");
                std::thread::spawn(move || update_tooltip());
                let _ = handle.emit("devices-changed", ());
            }

            // 低电量通知检查
            if has_battery_notify {
                let cache = get_devices_cache();
                crate::battery_notify::check_battery_notify(&crate::state::lock_unpoisoned(cache));
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
    let slot = AUTO_MENU_ITEM.get_or_init(|| Mutex::new(None));
    *crate::state::lock_unpoisoned(slot) = Some(auto_i.clone());

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
            standard_log!("[tray] menu: {}", event.id.as_ref());
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
                    let config_snapshot = config::with_config(|c| c.clone());
                    let _ = app.emit("config-changed", config_snapshot);
                    standard_log!("[tray] auto_start toggled: {}", new_val);
                }
                "exit" => {
                    app.exit(0);
                }
                id if id.starts_with("audio_dev_") => {
                    let device_id = id[10..].to_owned();
                    if !device_id.is_empty() {
                        standard_log!("[tray] set_default_device: {}", device_id);
                        std::thread::spawn(move || {
                            let _ = audio::set_default_device(&device_id);
                            update_audio_devices_menu();
                        });
                    }
                }
                "win_sound_volume_mixer" => {
                    let _ = crate::process::shell_open("sndvol.exe", None);
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

    let tray_slot = TRAY_ICON.get_or_init(|| Mutex::new(None));
    *crate::state::lock_unpoisoned(tray_slot) = Some(_tray);

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
        // 事件线程只做轻量分发（见 AGENTS.md「事件回调不得在事件线程做阻塞工作」）：
        // 读配置 + 更新原子标志 + 刷新勾选文案，三者都需即时反映，故留在本线程；
        // 其余（图标重建、菜单重建、设备缓存刷新）含 COM 枚举与全量菜单构造，全部下放子线程。
        let new_auto = config::with_config(|c| c.auto_start);
        AUTO_START.store(new_auto, Ordering::Relaxed);
        update_auto_text();
        std::thread::spawn(|| {
            update_tray_icon();
            update_audio_devices_menu();
            refresh_devices_cache();
            update_tooltip();
        });
    });

    app.listen("tray-devices-changed", move |_| {
        std::thread::spawn(move || update_tooltip());
    });

    app.listen("audio-devices-changed", |_| {
        // 同 config-changed：菜单重建内含 COM 枚举，不能在事件线程上同步跑
        std::thread::spawn(|| update_audio_devices_menu());
    });

    // 启动后台设备监控线程
    start_device_watcher(app.handle());
    // 启动系统深色模式监听线程（仅跟随系统）
    start_theme_watcher();

    Ok(())
}

fn update_auto_text() {
    // 先取句柄再释放锁：`set_text` 内部同步等主线程（同 build_audio_devices_menu 注释）
    let item = match AUTO_MENU_ITEM.get() {
        Some(slot) => {
            let guard = crate::state::lock_unpoisoned(slot);
            match *guard {
                Some(ref mi) => mi.clone(),
                None => return,
            }
        }
        None => return,
    };
    let text = if AUTO_START.load(Ordering::Relaxed) {
        "开机自启 ✓"
    } else {
        "开机自启"
    };
    let _ = item.set_text(text);
}

/// 根据默认打开页面与系统深色模式更新托盘图标
fn update_tray_icon() {
    let icon = pick_tray_icon();
    // 先取句柄再释放锁：`set_icon` 内部同步等主线程（同 build_audio_devices_menu 注释）
    let tray = {
        let guard = crate::state::lock_unpoisoned(TRAY_ICON.get().unwrap());
        match *guard {
            Some(ref tray) => tray.clone(),
            None => return,
        }
    };
    let _ = tray.set_icon(Some(icon));
}

/// 简化设备名称：仅保留括号内内容，如 "耳机 (小爱音箱-9205)" -> "小爱音箱-9205"
/// 注意：与 dedup::core_name 语义不同——本函数不剥协议后缀、返回 &str，两者勿互相替换。
pub(crate) fn simplify_device_name(name: &str) -> &str {
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
///
/// **锁纪律（P0 死锁防护，勿破坏）**：`config::with_config` 闭包内**只允许纯内存拷贝**。
///
/// Tauri 的菜单 API（`MenuItem::with_id` / `Submenu::append` / `set_menu` /
/// `set_icon` / `set_tooltip` / `set_text` …）内部一律经 `run_item_main_thread!` 展开为
/// `run_on_main_thread(..)` + `rx.recv()`（**无超时**），即同步等待主线程执行完闭包。
/// 而主线程自身会通过 `with_config(_mut)` 读配置（`set_window_material`、`get_config`、
/// 快捷键分发、tooltip 刷新…）。因此「持配置锁 → 调菜单 API」与「主线程 → 等配置锁」
/// 构成 AB/BA 死锁：子线程等主线程、主线程等配置锁，**永久冻结且看门狗也救不回**
/// （看门狗探活 `is_visible()` 同样要主线程）。
///
/// 故此处先在锁内取纯数据快照，菜单 API 全部移到锁外调用。
fn build_audio_devices_menu(
    app: &tauri::AppHandle,
) -> Result<Submenu<tauri::Wry>, Box<dyn std::error::Error>> {
    let submenu = Submenu::with_id(app, "audio_devices", "音频设备", true)?;
    let devices = audio::enumerate_output_devices().unwrap_or_default();
    if devices.is_empty() {
        let empty = MenuItem::with_id(app, "audio_dev_empty", "无音频设备", false, None::<&str>)?;
        submenu.append(&empty)?;
    } else {
        // 锁内：只做纯内存快照（菜单 id + 显示文案），不触碰任何 Tauri API
        let rows: Vec<(String, String)> = config::with_config(|c| {
            devices
                .iter()
                .filter(|device| !c.hidden_audio_devices.contains(&device.name))
                .map(|device| {
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
                    (
                        format!("audio_dev_{}", device.id),
                        format!("{}{}", display, check),
                    )
                })
                .collect()
        });
        // 锁外：菜单 API（内部同步等主线程，绝不可在持锁时调用）
        for (id, label) in rows {
            if let Ok(item) = MenuItem::with_id(app, id, label, true, None::<&str>) {
                let _ = submenu.append(&item);
            }
        }
    }
    Ok(submenu)
}

/// 音频菜单重建的单飞标志 / 合并标志（B3）。
///
/// 触发源有三个且都在**各自新开的线程**上：`config-changed`（一次连做
/// 图标 + 菜单 + 设备缓存 + tooltip）、`tray-devices-changed`、
/// `audio-devices-changed`。改造前它们各自独立重建菜单，撞车时「后写覆盖」——
/// 较慢的那次会用稍旧的一份菜单盖掉较新的一份，且整棵菜单（含 COM 枚举）
/// 会被重复构造。现由 [`crate::state::run_coalesced`] 收敛为「单飞 + 合并」。
static MENU_REBUILD_RUNNING: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);
static MENU_REBUILD_PENDING: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// 更新音频设备切换子菜单（在设备列表变化时调用）
///
/// 入口做「单飞 + 合并」：同一时刻只有一轮在重建，期间到达的请求被合并进
/// 收尾的补跑轮次（详见 `state::run_coalesced`）。
fn update_audio_devices_menu() {
    let rounds = crate::state::run_coalesced(
        &MENU_REBUILD_RUNNING,
        &MENU_REBUILD_PENDING,
        rebuild_audio_devices_menu_once,
    );
    if let Some(n) = rounds {
        if n > 1 {
            // 合并生效的证据：一次触发期间到达了额外请求，但没有重建 n 次菜单树
            standard_log!("[tray] audio menu rebuild coalesced: {} rounds", n);
        }
    }
}

/// 重建一次音频设备切换子菜单（原 `update_audio_devices_menu` 的函数体）
///
/// 锁纪律：`TRAY_ICON` 只用于「取句柄」和「换菜单」两个瞬时动作，
/// 中间的 COM 枚举（`build_audio_devices_menu`）与全量菜单构造（`build_full_menu`）
/// 一律在锁外完成——否则会与 `update_tray_icon` 等持锁路径互相排队，
/// 把一次设备热插拔放大成可见卡顿。
fn rebuild_audio_devices_menu_once() {
    // 第一段：仅取 app 句柄，取到即释放锁
    let app = {
        let tray_guard = crate::state::lock_unpoisoned(TRAY_ICON.get().unwrap());
        match *tray_guard {
            Some(ref tray) => tray.app_handle().clone(),
            None => return,
        }
    };

    // 第二段：锁外做耗时工作（COM 枚举 + 菜单树构造）
    let new_submenu = match build_audio_devices_menu(&app) {
        Ok(s) => s,
        Err(_) => return,
    };
    let Ok(menu) = build_full_menu(&app, &new_submenu) else {
        return;
    };

    // 第三段：先取句柄、释放托盘锁，再换菜单——`set_menu` 内部经
    // `run_item_main_thread!` 同步等主线程，持锁调用会与主线程的
    // `update_tooltip` / `update_tray_icon`（同样要 TRAY_ICON）构成 AB/BA 死锁。
    let tray = {
        let tray_guard = crate::state::lock_unpoisoned(TRAY_ICON.get().unwrap());
        match *tray_guard {
            Some(ref tray) => tray.clone(),
            None => return,
        }
    };
    let _ = tray.set_menu(Some(menu));
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
