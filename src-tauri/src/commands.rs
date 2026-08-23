use crate::config::{self, Config};
use crate::device;
use crate::process;
use crate::wmi_query::query_devices;
use tauri::Emitter;
use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};

#[tauri::command]
pub fn set_shortcut_recording(recording: bool) {
    crate::state::SHORTCUT_RECORDING.store(recording, std::sync::atomic::Ordering::Relaxed);
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
pub fn get_config() -> Config {
    config::with_config(|c| c.clone())
}

/// 应用版本号（来自 tauri.conf.json 的真实包版本，供关于页动态显示，消除静态文案漂移）
#[tauri::command]
pub fn get_app_version(app: tauri::AppHandle) -> String {
    app.package_info().version.to_string()
}

/// 设置当前窗口的系统标题栏主题（仅影响系统窗口标题栏，不影响页面内容）
#[tauri::command]
pub fn set_window_theme(window: tauri::Window, theme: String) {
    let t = match theme.as_str() {
        "dark" => Some(tauri::Theme::Dark),
        "light" => Some(tauri::Theme::Light),
        _ => None,
    };
    let _ = window.set_theme(t);
}

#[tauri::command]
pub fn update_config(app: tauri::AppHandle, mut new_config: Config) {
    let cycle_was_enabled = config::with_config(|c| c.enable_device_shortcut_cycle);
    if cycle_was_enabled && !new_config.enable_device_shortcut_cycle {
        // 关闭共享开关：清除被多个设备共用的快捷键
        clear_shared_device_shortcuts(&mut new_config);
        crate::shortcut::sync_device_shortcuts(&app);
    }
    config::with_config_mut(|c| {
        *c = new_config;
    });
    let _ = app.emit("config-changed", ());
}

/// 清除被多个设备共用的快捷键（保留各设备的名称，仅清空 shortcut）
fn clear_shared_device_shortcuts(c: &mut Config) {
    use std::collections::HashMap;
    let mut key_count: HashMap<String, usize> = HashMap::new();
    for d in c.device_shortcuts.values() {
        if let Some(ref k) = d.shortcut {
            *key_count.entry(k.clone()).or_insert(0) += 1;
        }
    }
    let mut cleared = false;
    for d in c.device_shortcuts.values_mut() {
        if let Some(ref k) = d.shortcut {
            if key_count.get(k).copied().unwrap_or(0) > 1 {
                d.shortcut = None;
                cleared = true;
            }
        }
    }
    if cleared {
        process::append_log("[config] shared device shortcuts cleared (cycle toggle disabled)");
    }
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
pub fn rename_device(app: tauri::AppHandle, original: String, new_name: String) {
    crate::process::append_log(&format!("[cmd] rename_device: '{}' -> '{}'", original, new_name));
    config::with_config_mut(|c| {
        if new_name.is_empty() || new_name == original {
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

// ── 音频命令 ─────────────────────────────────────────────

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
pub async fn set_device_mute(device_id: String, muted: bool) -> Result<(), String> {
    run_blocking(move || crate::audio::set_device_mute(&device_id, muted))
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
pub async fn set_session_mute(session_id: String, muted: bool) -> Result<(), String> {
    run_blocking(move || crate::audio::set_session_mute(&session_id, muted))
        .await?
        .map_err(|e| e.to_string())
}

#[tauri::command(async)]
pub async fn get_input_devices() -> Result<Vec<crate::audio::AudioDevice>, String> {
    run_blocking(crate::audio::enumerate_input_devices)
        .await?
        .map_err(|e| e.to_string())
}

#[tauri::command(async)]
pub async fn set_session_device(pid: u32, direction: String, device_id: String) -> Result<(), String> {
    run_blocking(move || crate::audio::set_session_device(pid, &direction, &device_id))
        .await?
        .map_err(|e| e.to_string())
}

#[tauri::command(async)]
pub async fn get_session_device(pid: u32, direction: String) -> Result<Option<String>, String> {
    run_blocking(move || crate::audio::get_session_device(pid, &direction))
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

#[tauri::command(async)]
pub async fn get_spatial_sound(device_id: String) -> Result<crate::audio_spatial::SpatialSoundState, String> {
    run_blocking(move || crate::audio_spatial::get_spatial_sound(&device_id)).await?
}

#[tauri::command(async)]
pub async fn set_spatial_sound(device_id: String, format_guid: Option<String>) -> Result<(), String> {
    run_blocking(move || crate::audio_spatial::set_spatial_sound(&device_id, format_guid.as_deref())).await?
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
    let (result, _) =
        crate::update::check_and_store("", current_version, include_prerelease).await;
    result
}

#[tauri::command]
pub fn get_update_status() -> Option<crate::update::UpdateStatus> {
    crate::update::get_last_status()
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
                crate::shortcut::dispatch_shortcut_action(_app, &action_clone, &key_clone);
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
            let share_enabled = crate::config::with_config(|c| c.enable_device_shortcut_cycle);
            let (used_by_any_device, used_by_other_device) = crate::config::with_config(|c| {
                let any = c.device_shortcuts.values().any(|d| d.shortcut.as_deref() == Some(new_key_str));
                let other = c.device_shortcuts.iter().any(|(id, d)| id != &device_id && d.shortcut.as_deref() == Some(new_key_str));
                (any, other)
            });
            // 关闭共享：被其他设备或非设备功能占用都拒绝（本设备自身占用允许）
            // 开启共享：仅拒绝非设备功能占用
            let conflict = if share_enabled { !used_by_any_device } else { used_by_other_device || !used_by_any_device };
            if conflict {
                return Err("快捷键已被占用".to_string());
            }
        }
    }
    set_device_shortcut_key(&device_id, &name, key);
    crate::shortcut::sync_device_shortcuts(&app);
    let _ = app.emit("config-changed", ());
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
    let _ = app.emit("config-changed", ());
}

// ═══════════════════════════════════════════════════════════════
// 窗口材质
// ═══════════════════════════════════════════════════════════════

/// 设置窗口材质并应用到所有窗口
#[tauri::command]
pub fn set_window_material(app: tauri::AppHandle, material: String) -> Result<bool, String> {
    crate::windows::set_window_material(&app, material)
}

/// 检查系统是否支持指定材质
#[tauri::command]
pub fn check_material_support(material: String) -> Result<bool, String> {
    Ok(crate::windows::check_material_support(&material))
}


