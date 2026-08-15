use crate::config::{self, Config};
use crate::device;
use crate::process;
use crate::wmi_query::query_devices;
use std::sync::atomic::{AtomicBool, Ordering};
use tauri::{Emitter, Manager};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};

/// 快捷键录制期间置位，抑制全局快捷键触发，避免录制时误触发动作
static SHORTCUT_RECORDING: AtomicBool = AtomicBool::new(false);

#[tauri::command]
pub fn set_shortcut_recording(recording: bool) {
    SHORTCUT_RECORDING.store(recording, Ordering::Relaxed);
    process::append_log(&format!("[hotkey] shortcut recording = {}", recording));
}

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
    let (already_added, count) = config::with_config(|c| {
        (c.tray_devices.contains(&name), c.tray_devices.len())
    });
    if !already_added && count >= TRAY_DEVICE_LIMIT {
        return Err(format!("托盘最多添加 {} 个设备", TRAY_DEVICE_LIMIT));
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
pub fn adjust_volume_up() {
    crate::audio::adjust_default_volume_up();
}

#[tauri::command]
pub fn adjust_volume_down() {
    crate::audio::adjust_default_volume_down();
}

#[tauri::command]
pub fn toggle_mute() {
    crate::audio::toggle_default_mute();
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
        match action.as_str() {
            "devices" => c.shortcut_devices.clone(),
            "volume" => c.shortcut_volume.clone(),
            "volume_up" => c.shortcut_volume_up.clone(),
            "volume_down" => c.shortcut_volume_down.clone(),
            "volume_mute" => c.shortcut_volume_mute.clone(),
            _ => None,
        }
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
            set_config_key(&action, None);
            return Err("快捷键已被占用".to_string());
        }
        let action_clone = action.clone();
        let key_clone = new_key_str.clone();
        crate::process::append_log(&format!("[hotkey] registered {} {}", new_key_str, action));
        app.global_shortcut()
            .on_shortcut(sc, move |_app, _shortcut, event| {
                if event.state != ShortcutState::Pressed {
                    return;
                }
                dispatch_shortcut_action(_app, &action_clone, &key_clone);
            })
            .map_err(|e| e.to_string())?;
    }
    set_config_key(&action, key);
    Ok(())
}

fn set_config_key(action: &str, key: Option<String>) {
    config::with_config_mut(|c| {
        match action {
            "devices" => c.shortcut_devices = key,
            "volume" => c.shortcut_volume = key,
            "volume_up" => c.shortcut_volume_up = key,
            "volume_down" => c.shortcut_volume_down = key,
            "volume_mute" => c.shortcut_volume_mute = key,
            _ => {}
        }
    });
}

#[tauri::command]
pub fn set_device_shortcut(
    app: tauri::AppHandle,
    device_id: String,
    name: String,
    key: Option<String>,
) -> Result<(), String> {
    if let Some(ref new_key_str) = key {
        let sc = parse_shortcut(new_key_str)?;
        // 若键已被注册且不是另一设备快捷键（不在当前设备快捷键集合中）→ 与非设备功能冲突
        if app.global_shortcut().is_registered(sc.clone()) {
            let used_by_device = crate::config::with_config(|c| {
                c.device_shortcuts.values().any(|d| d.shortcut.as_deref() == Some(new_key_str))
            });
            if !used_by_device {
                return Err("快捷键已被占用".to_string());
            }
        }
    }
    set_device_shortcut_key(&device_id, &name, key);
    crate::shortcut::sync_device_shortcuts(&app);
    Ok(())
}

fn set_device_shortcut_key(device_id: &str, name: &str, key: Option<String>) {
    config::with_config_mut(|c| {
        if let Some(k) = key {
            c.device_shortcuts.insert(
                device_id.to_string(),
                crate::config::DeviceShortcut { name: name.to_string(), shortcut: Some(k) },
            );
        } else if let Some(entry) = c.device_shortcuts.get_mut(device_id) {
            entry.shortcut = None;
            entry.name = name.to_string();
        }
    });
}

#[tauri::command]
pub fn remove_device_shortcut(app: tauri::AppHandle, device_id: String) {
    config::with_config_mut(|c| {
        c.device_shortcuts.remove(&device_id);
    });
    crate::shortcut::sync_device_shortcuts(&app);
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
    let connected: Vec<&crate::audio::AudioDevice> =
        devices.iter().filter(|d| group.iter().any(|id| id == &d.id)).collect();
    if connected.is_empty() {
        crate::process::append_log(&format!("[hotkey] no connected devices for shared key {}", key));
        return;
    }

    let current_default = devices.iter().find(|d| d.is_default);
    let next = if let Some(current) = current_default {
        if let Some(pos) = connected.iter().position(|d| d.id == current.id) {
            connected[(pos + 1) % connected.len()]
        } else {
            connected[0]
        }
    } else {
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
    if SHORTCUT_RECORDING.load(Ordering::Relaxed) {
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
        "volume_up" => crate::audio::adjust_default_volume_up(),
        "volume_down" => crate::audio::adjust_default_volume_down(),
        "volume_mute" => crate::audio::toggle_default_mute(),
        _ => {}
    }
}


