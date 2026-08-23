use std::collections::HashSet;
use std::sync::{LazyLock, Mutex};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};

static DEVICE_REGISTERED_KEYS: LazyLock<Mutex<HashSet<String>>> =
    LazyLock::new(|| Mutex::new(HashSet::new()));

pub fn register_shortcuts(app: &tauri::AppHandle) {
    let app = app.clone();
    let (device_key, volume_key, vol_up_key, vol_down_key, vol_mute_key) = crate::config::with_config(|c| {
        (c.shortcut_devices.clone(), c.shortcut_volume.clone(), c.shortcut_volume_up.clone(), c.shortcut_volume_down.clone(), c.shortcut_volume_mute.clone())
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

    let mut registered = DEVICE_REGISTERED_KEYS
        .lock()
        .unwrap_or_else(|e| e.into_inner());

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
        let action: &'static str = Box::leak(action.into_boxed_str());
        let key_str: &'static str = Box::leak(key.clone().into_boxed_str());
        crate::process::append_log(&format!("[shortcut] registered {} -> {}", key, action));
        let _ = app.global_shortcut().on_shortcut(sc, move |_app, _shortcut, event| {
            if event.state != ShortcutState::Pressed {
                return;
            }
            crate::commands::dispatch_shortcut_action(_app, action, key_str);
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
    let _ = app.global_shortcut().on_shortcut(sc, move |_app, _shortcut, event| {
        if event.state != ShortcutState::Pressed {
            return;
        }
        crate::commands::dispatch_shortcut_action(_app, &action_str, &key_str);
    });
    crate::process::append_log(&format!("[shortcut] registered {} -> {}", key, action));
}