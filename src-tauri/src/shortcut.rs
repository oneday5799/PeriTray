use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};

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

    let device_shortcuts = crate::config::with_config(|c| c.device_shortcuts.clone());
    for (device_id, entry) in device_shortcuts {
        if let Some(ref key) = entry.shortcut {
            let action = format!("device_shortcut:{}", device_id);
            let action = Box::leak(action.into_boxed_str());
            register_single(&app, key, action);
        }
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
    let app = app.clone();
    let _ = app.global_shortcut().on_shortcut(sc, move |_app, _shortcut, event| {
        if event.state != ShortcutState::Pressed {
            return;
        }
        crate::commands::dispatch_shortcut_action(_app, &action_str);
    });
    crate::process::append_log(&format!("[shortcut] registered {} -> {}", key, action));
}