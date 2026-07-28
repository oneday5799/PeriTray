use tauri_plugin_global_shortcut::GlobalShortcutExt;

pub fn register_shortcuts(app: &tauri::AppHandle) {
    let app = app.clone();
    let (device_key, volume_key) = crate::config::with_config(|c| {
        (c.shortcut_devices.clone(), c.shortcut_volume.clone())
    });

    if let Some(ref key) = device_key {
        register_single(&app, key, "devices");
    }
    if let Some(ref key) = volume_key {
        register_single(&app, key, "volume");
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
    let _ = app.global_shortcut().on_shortcut(sc, move |_app, _shortcut, _event| {
        if action_str == "devices" {
            crate::popup::open_popup(&_app, "devices");
        } else {
            crate::popup::open_popup(&_app, "volume");
        }
    });
    crate::process::append_log(&format!("[shortcut] registered {} -> {}", key, action));
}