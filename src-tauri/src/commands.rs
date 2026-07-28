use crate::config::{self, Config};
use crate::device;
use crate::process;
use crate::wmi_query::query_devices;
use tauri::{Emitter, Manager};
use tauri_plugin_global_shortcut::GlobalShortcutExt;

/// 在 tokio blocking 线程中执行阻塞操作
async fn run_blocking<F, T>(f: F) -> Result<T, String>
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| e.to_string())
}

/// 切换 Vec 中某个元素的存在/不存在
fn toggle_vec_item(vec: &mut Vec<String>, item: &str) {
    if let Some(pos) = vec.iter().position(|v| v == item) {
        vec.remove(pos);
    } else {
        vec.push(item.to_string());
    }
}

#[tauri::command(async)]
pub async fn get_devices() -> Vec<device::Device> {
    let devices = run_blocking(query_devices).await.unwrap_or_default();
    device::store_device_ids(&devices);
    devices
}

#[tauri::command]
pub fn open_settings(app: tauri::AppHandle) {
    crate::windows::open_settings(&app);
}

#[tauri::command]
pub fn exit_app(app: tauri::AppHandle) {
    crate::process::append_log("[cmd] exit_app");
    app.exit(0);
}

#[tauri::command]
pub fn get_config() -> Config {
    config::with_config(|c| c.clone())
}

#[tauri::command]
pub fn update_config(app: tauri::AppHandle, new_config: Config) {
    config::with_config_mut(|c| {
        *c = new_config;
    });
    let _ = app.emit("config-changed", ());
}

#[tauri::command]
pub fn toggle_device_hidden(app: tauri::AppHandle, name: String) {
    crate::process::append_log(&format!("[cmd] toggle_device_hidden: {}", name));
    config::with_config_mut(|c| toggle_vec_item(&mut c.hidden_devices, &name));
    let _ = app.emit("config-changed", ());
}

#[tauri::command]
pub fn toggle_audio_device_hidden(app: tauri::AppHandle, name: String) {
    crate::process::append_log(&format!("[cmd] toggle_audio_device_hidden: {}", name));
    config::with_config_mut(|c| toggle_vec_item(&mut c.hidden_audio_devices, &name));
    let _ = app.emit("config-changed", ());
    let _ = app.emit("audio-devices-changed", ());
}

#[tauri::command]
pub fn open_bt_settings() -> Result<(), String> {
    process::open_with_system("ms-settings:bluetooth")
}

#[tauri::command]
pub fn open_url(url: String) -> Result<(), String> {
    process::open_with_system(&url)
}

#[tauri::command]
pub fn close_window(app: tauri::AppHandle, name: String) {
    if let Some(window) = app.get_webview_window(&name) {
        let _ = window.close();
    }
}

#[tauri::command]
pub fn rename_device(app: tauri::AppHandle, original: String, new_name: String) {
    crate::process::append_log(&format!("[cmd] rename_device: '{}' -> '{}'", original, new_name));
    config::with_config_mut(|c| {
        if new_name.is_empty() {
            c.device_names.remove(&original);
        } else {
            c.device_names.insert(original, new_name);
        }
    });
    let _ = app.emit("audio-devices-changed", ());
}

#[tauri::command]
pub fn change_device_group(app: tauri::AppHandle, name: String, group: String) {
    config::with_config_mut(|c| {
        if group.is_empty() {
            c.device_groups.remove(&name);
        } else {
            c.device_groups.insert(name, group);
        }
    });
    let _ = app.emit("config-changed", ());
}

#[tauri::command]
pub fn toggle_group_hidden(app: tauri::AppHandle, group: String) {
    config::with_config_mut(|c| toggle_vec_item(&mut c.hidden_groups, &group));
    let _ = app.emit("config-changed", ());
}

#[tauri::command(async)]
pub async fn disconnect_bluetooth_device(name: String) -> Result<String, String> {
    crate::process::append_log(&format!("[cmd] disconnect_bluetooth_device: {}", name));
    run_blocking(move || crate::bluetooth::bt_action(&name, "disconnect"))
        .await?
        .map_err(|e| e.to_string())
}

#[tauri::command(async)]
pub async fn connect_bluetooth_device(name: String) -> Result<String, String> {
    crate::process::append_log(&format!("[cmd] connect_bluetooth_device: {}", name));
    run_blocking(move || crate::bluetooth::bt_action(&name, "connect"))
        .await?
        .map_err(|e| e.to_string())
}

#[tauri::command(async)]
pub async fn check_bt_connection(name: String) -> Result<Option<bool>, String> {
    Ok(run_blocking(move || crate::bluetooth::check_device_connection(&name)).await?)
}

#[tauri::command]
pub fn open_24g_device_file() -> Result<(), String> {
    let path = crate::process::exe_dir().join("data").join("wireless_24g_devices_user.json");
    if !path.exists() {
        std::fs::write(&path, "{}").map_err(|e| e.to_string())?;
    }
    process::open_with_system(&path.to_string_lossy())
}

const TRAY_DEVICE_LIMIT: usize = 4;

#[tauri::command(async)]
pub async fn toggle_device_tray(app: tauri::AppHandle, name: String) -> Result<(), String> {
    let already_added = config::with_config(|c| c.tray_devices.contains(&name));
    if !already_added {
        let count = config::with_config(|c| c.tray_devices.len());
        if count >= TRAY_DEVICE_LIMIT {
            return Err(format!("托盘最多添加 {} 个设备", TRAY_DEVICE_LIMIT));
        }
    }
    run_blocking(move || {
        config::with_config_mut(|c| toggle_vec_item(&mut c.tray_devices, &name));
    })
    .await?;
    crate::tray::refresh_tray_tooltip(&app);
    let _ = app.emit("tray-devices-changed", ());
    Ok(())
}

#[tauri::command]
pub fn get_tray_tooltip() -> String {
    crate::tray::build_tooltip_text()
}

// Audio commands

#[tauri::command(async)]
pub async fn get_audio_devices() -> Result<Vec<crate::audio::AudioDevice>, String> {
    run_blocking(crate::audio::enumerate_output_devices)
        .await?
        .map_err(|e| e.to_string())
}

#[tauri::command(async)]
pub async fn set_device_volume(device_id: String, volume: f32) -> Result<(), String> {
    run_blocking(move || crate::audio::set_device_volume(&device_id, volume))
        .await?
        .map_err(|e| e.to_string())
}

#[tauri::command(async)]
pub async fn toggle_device_mute(device_id: String) -> Result<(), String> {
    run_blocking(move || crate::audio::toggle_device_mute(&device_id))
        .await?
        .map_err(|e| e.to_string())
}

#[tauri::command(async)]
pub async fn get_audio_sessions(device_id: String) -> Result<Vec<crate::audio::AudioSession>, String> {
    run_blocking(move || crate::audio::enumerate_audio_sessions(&device_id))
        .await?
        .map_err(|e| e.to_string())
}

#[tauri::command(async)]
pub async fn set_session_volume(session_id: String, volume: f32) -> Result<(), String> {
    run_blocking(move || crate::audio::set_session_volume(&session_id, volume))
        .await?
        .map_err(|e| e.to_string())
}

#[tauri::command(async)]
pub async fn toggle_session_mute(session_id: String) -> Result<(), String> {
    run_blocking(move || crate::audio::toggle_session_mute(&session_id))
        .await?
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn set_default_device(app: tauri::AppHandle, device_id: String) -> Result<(), String> {
    crate::process::append_log(&format!("[cmd] set_default_device: {}", device_id));
    crate::audio::set_default_device(&device_id).map_err(|e| e.to_string())?;
    let _ = app.emit("audio-devices-changed", ());
    Ok(())
}

#[tauri::command]
pub fn open_log_dir() -> Result<(), String> {
    let dir = crate::process::exe_dir();
    let _ = std::fs::create_dir_all(&dir);
    process::open_with_system(&dir.to_string_lossy())
}

#[tauri::command(async)]
pub async fn check_for_update(
    app: tauri::AppHandle,
    include_prerelease: bool,
) -> Result<crate::update::UpdateInfo, String> {
    let current_version = app.package_info().version.to_string();
    // WinHTTP is blocking but fast; spawn_blocking to avoid blocking Tauri's async runtime
    tokio::task::spawn_blocking(move || {
        crate::update::check_for_update(&current_version, include_prerelease)
    })
    .await
    .map_err(|e| format!("task error: {}", e))?
}

fn parse_shortcut(s: &str) -> Result<tauri_plugin_global_shortcut::Shortcut, String> {
    tauri_plugin_global_shortcut::Shortcut::try_from(s).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn set_hotkey_config(app: tauri::AppHandle, action: String, key: Option<String>) -> Result<(), String> {
    let prev_key = config::with_config(|c| {
        if action == "devices" { c.shortcut_devices.clone() }
        else { c.shortcut_volume.clone() }
    });
    if let Some(ref pk) = prev_key {
        if let Ok(sc) = parse_shortcut(pk) {
            let _ = app.global_shortcut().unregister(sc);
            crate::process::append_log(&format!("[hotkey] unregistered old key {} for {}", pk, action));
        }
    }
    if let Some(ref new_key_str) = key {
        let sc = parse_shortcut(new_key_str)?;
        if app.global_shortcut().is_registered(sc.clone()) {
            config::with_config_mut(|c| {
                if action == "devices" { c.shortcut_devices = None; }
                else { c.shortcut_volume = None; }
            });
            return Err("快捷键已被占用".to_string());
        }
        let action_clone = action.clone();
        let key_clone = new_key_str.clone();
        app.global_shortcut()
            .on_shortcut(sc, move |_app, _shortcut, _event| {
                if action_clone == "devices" {
                    crate::popup::open_popup(&_app, "devices");
                } else {
                    crate::popup::open_popup(&_app, "volume");
                }
            })
            .map_err(|e| e.to_string())?;
        crate::process::append_log(&format!("[hotkey] registered {} {}", key_clone, action));
    }
    config::with_config_mut(|c| {
        if action == "devices" { c.shortcut_devices = key.clone(); }
        else { c.shortcut_volume = key.clone(); }
    });
    Ok(())
}


