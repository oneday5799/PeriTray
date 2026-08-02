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
        match action_str.as_str() {
            "devices" => crate::popup::open_popup(_app, "devices"),
            "volume" => crate::popup::open_popup(_app, "volume"),
            "volume_up" => crate::audio::adjust_default_volume_up(),
            "volume_down" => crate::audio::adjust_default_volume_down(),
            "volume_mute" => crate::audio::toggle_default_mute(),
            _ => {}
        }
    });
    crate::process::append_log(&format!("[shortcut] registered {} -> {}", key, action));
}