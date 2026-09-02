use std::collections::HashSet;
use std::sync::atomic::Ordering;
use std::sync::{LazyLock, Mutex};
use tauri::Emitter;
use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};

static DEVICE_REGISTERED_KEYS: LazyLock<Mutex<HashSet<String>>> =
    LazyLock::new(|| Mutex::new(HashSet::new()));

pub fn register_shortcuts(app: &tauri::AppHandle) {
    let app = app.clone();
    let (device_key, volume_key, vol_up_key, vol_down_key, vol_mute_key) =
        crate::config::with_config(|c| {
            (
                c.shortcut_devices.clone(),
                c.shortcut_volume.clone(),
                c.shortcut_volume_up.clone(),
                c.shortcut_volume_down.clone(),
                c.shortcut_volume_mute.clone(),
            )
        });

    if let Some(ref key) = device_key {
        register_single(&app, key, "devices");
    }
    if let Some(ref key) = volume_key {
        register_single(&app, key, "volume");
    }
    if let Some(ref key) = vol_up_key {
        register_single(&app, key, "volume_up");
    }
    if let Some(ref key) = vol_down_key {
        register_single(&app, key, "volume_down");
    }
    if let Some(ref key) = vol_mute_key {
        register_single(&app, key, "volume_mute");
    }

    sync_device_shortcuts(&app);
}

/// 根据配置中的设备快捷键集合同步全局快捷键注册。
/// 同一快捷键键仅注册一次，action 为 `device_shortcut_key:<key>`，
/// 多个设备可共用同一键（触发后在设备间循环切换）。
pub fn sync_device_shortcuts(app: &tauri::AppHandle) {
    let desired: HashSet<String> = crate::config::with_config(|c| {
        c.device_shortcuts
            .values()
            .filter_map(|d| d.shortcut.clone())
            .collect()
    });

    let mut registered = crate::state::lock_unpoisoned(&DEVICE_REGISTERED_KEYS);

    // 注销已注册但不再使用的键
    for key in registered.iter() {
        if !desired.contains(key) {
            if let Ok(sc) = tauri_plugin_global_shortcut::Shortcut::try_from(key.as_str()) {
                let _ = app.global_shortcut().unregister(sc);
            }
        }
    }
    registered.retain(|k| desired.contains(k));

    // 注册期望但尚未注册的键
    for key in desired.iter() {
        if registered.contains(key) {
            continue;
        }
        let sc = match tauri_plugin_global_shortcut::Shortcut::try_from(key.as_str()) {
            Ok(sc) => sc,
            Err(_) => {
                crate::process::append_log(&format!("[shortcut] invalid key: {}", key));
                continue;
            }
        };
        let action = format!("device_shortcut_key:{}", key);
        let key_str = key.clone();
        crate::process::append_log(&format!("[shortcut] registered {} -> {}", key, action));
        let _ = app
            .global_shortcut()
            .on_shortcut(sc, move |_app, _shortcut, event| {
                if event.state != ShortcutState::Pressed {
                    return;
                }
                dispatch_shortcut_action(_app, &action, &key_str);
            });
        registered.insert(key.clone());
    }
}

fn register_single(app: &tauri::AppHandle, key: &str, action: &'static str) {
    let sc = match tauri_plugin_global_shortcut::Shortcut::try_from(key) {
        Ok(sc) => sc,
        Err(_) => {
            crate::process::append_log(&format!("[shortcut] invalid key: {}", key));
            return;
        }
    };
    let action_str = action.to_string();
    let key_str = key.to_string();
    let app = app.clone();
    let _ = app
        .global_shortcut()
        .on_shortcut(sc, move |_app, _shortcut, event| {
            if event.state != ShortcutState::Pressed {
                return;
            }
            dispatch_shortcut_action(_app, &action_str, &key_str);
        });
    crate::process::append_log(&format!("[shortcut] registered {} -> {}", key, action));
}

/// 在共用同一快捷键的设备间循环切换默认输出设备（按设备列表自然顺序）
fn cycle_device_shortcut(app: &tauri::AppHandle, key: &str) {
    let group: Vec<String> = crate::config::with_config(|c| {
        c.device_shortcuts
            .iter()
            .filter(|(_, d)| d.shortcut.as_deref() == Some(key))
            .map(|(id, _)| id.clone())
            .collect()
    });
    if group.is_empty() {
        return;
    }

    let devices = crate::audio::enumerate_output_devices().unwrap_or_default();
    let connected: Vec<&crate::audio::AudioDevice> = devices
        .iter()
        .filter(|d| group.iter().any(|id| id == &d.id))
        .collect();
    if connected.is_empty() {
        crate::process::append_log(&format!(
            "[hotkey] no connected devices for shared key {}",
            key
        ));
        return;
    }

    let current_default = devices.iter().find(|d| d.is_default);
    let share_enabled = crate::config::with_config(|c| c.enable_device_shortcut_cycle);
    let next = if share_enabled {
        if let Some(current) = current_default {
            if let Some(pos) = connected.iter().position(|d| d.id == current.id) {
                connected[(pos + 1) % connected.len()]
            } else {
                connected[0]
            }
        } else {
            connected[0]
        }
    } else {
        // 未开启共享：切换到该键关联的第一个已连接设备
        connected[0]
    };

    crate::process::append_log(&format!(
        "[hotkey] device shortcut '{}' -> switch default to {}",
        key, next.name
    ));
    if let Err(e) = crate::audio::set_default_device(&next.id) {
        crate::process::append_log(&format!("[hotkey] set default device failed: {}", e));
    }
    let _ = app.emit("audio-devices-changed", ());
}

pub(crate) fn dispatch_shortcut_action(app: &tauri::AppHandle, action: &str, key: &str) {
    if crate::state::SHORTCUT_RECORDING.load(Ordering::Relaxed) {
        // 录制期间：不执行动作，把按下的键上报给前端用于录制
        crate::process::append_log(&format!("[hotkey] captured while recording: {}", key));
        let _ = app.emit("shortcut-recorded", key);
        return;
    }
    if let Some(key) = action.strip_prefix("device_shortcut_key:") {
        crate::process::append_log(&format!("[hotkey] device shortcut key triggered: {}", key));
        cycle_device_shortcut(app, key);
        return;
    }
    match action {
        "devices" => crate::popup::open_popup(app, "devices"),
        "volume" => crate::popup::open_popup(app, "volume"),
        "volume_up" => {
            crate::process::append_log(&format!(
                "[hotkey] volume action: {} (key={})",
                action, key
            ));
            crate::audio::adjust_default_volume_up()
        }
        "volume_down" => {
            crate::process::append_log(&format!(
                "[hotkey] volume action: {} (key={})",
                action, key
            ));
            crate::audio::adjust_default_volume_down()
        }
        "volume_mute" => {
            crate::process::append_log(&format!(
                "[hotkey] volume action: {} (key={})",
                action, key
            ));
            crate::audio::toggle_default_mute()
        }
        _ => {}
    }
}
