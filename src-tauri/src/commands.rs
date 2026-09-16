use crate::config::{self, Config};
use crate::device;
use crate::process;
use crate::standard_log;
use crate::wmi_query::query_devices;
use tauri::Emitter;
use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};

#[tauri::command]
pub fn set_shortcut_recording(recording: bool) {
    crate::state::SHORTCUT_RECORDING.store(recording, std::sync::atomic::Ordering::Relaxed);
    standard_log!("[hotkey] shortcut recording = {}", recording);
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
pub async fn get_devices() -> Result<Vec<device::Device>, String> {
    let devices = run_blocking(|| query_devices(false)).await??;
    Ok(devices)
}

/// 设备信息页手动刷新入口：强制现查 2.4G 接收器电量（绕过 TTL 缓存），
/// 其余流程与 get_devices 一致；鼠标休眠时现查耗时可达数秒
#[tauri::command(async)]
pub async fn get_devices_fresh() -> Result<Vec<device::Device>, String> {
    let devices = run_blocking(|| query_devices(true)).await??;
    Ok(devices)
}

/// 返回设备缓存（tray watcher 每轮更新），供设置页等场景避免重复 WMI 查询。
/// 缓存为空时返回空 vec，调用方应 fallback 到 get_devices。
#[tauri::command]
pub fn get_cached_devices() -> Vec<device::Device> {
    let cache = crate::state::get_devices_cache();
    crate::state::lock_unpoisoned(&cache).clone()
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
    // 保留时长变更时，落地后立即清理一次旧日志
    let retention_changed = config::with_config(|c| c.log_retention) != new_config.log_retention;
    config::with_config_mut(|c| {
        *c = new_config;
    });
    // 传递完整 config 快照，前端无需再调用 get_config
    let config_snapshot = config::with_config(|c| c.clone());
    let _ = app.emit("config-changed", config_snapshot);
    if retention_changed {
        process::clean_old_logs();
    }
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
    standard_log!("[cmd] toggle_device_hidden: {}", name);
    config::with_config_mut(|c| toggle_vec_item(&mut c.hidden_devices, &name));
    let config_snapshot = config::with_config(|c| c.clone());
    let _ = app.emit("config-changed", config_snapshot);
}

#[tauri::command]
pub fn toggle_audio_device_hidden(app: tauri::AppHandle, name: String) {
    standard_log!("[cmd] toggle_audio_device_hidden: {}", name);
    config::with_config_mut(|c| toggle_vec_item(&mut c.hidden_audio_devices, &name));
    let config_snapshot = config::with_config(|c| c.clone());
    let _ = app.emit("config-changed", config_snapshot);
    let _ = app.emit("audio-devices-changed", ());
}

#[tauri::command]
pub fn open_bt_settings() -> Result<(), String> {
    process::shell_open("ms-settings:bluetooth", None)
}

/// `open_url` 允许的目标协议白名单。
/// 覆盖前端全部实际调用面：GitHub / Release 页（https）、空间音效回退（ms-settings:）。
const OPEN_URL_ALLOWED: &[&str] = &["https://", "http://", "ms-windows-store://", "ms-settings:"];

/// 判定一个 URL 是否允许交给系统打开（纯函数，便于单测覆盖拒绝分支）。
fn is_allowed_open_url(url: &str) -> bool {
    OPEN_URL_ALLOWED.iter().any(|p| url.starts_with(p))
}

/// 打开外部链接。
/// **必须白名单**：`ShellExecuteW` 会执行任意已注册协议，不加限制时前端一旦被注入
/// （XSS / 恶意扩展）即可借 `open_url` 用 `file:`、自定义协议等做本地落地，
/// 而本命令是前端唯一能触达「系统执行」的入口（见代码审查报告 P1-4）。
#[tauri::command]
pub fn open_url(url: String) -> Result<(), String> {
    if !is_allowed_open_url(&url) {
        standard_log!("[cmd] open_url 拒绝非白名单协议: {}", url);
        return Err(format!("不允许的链接协议: {}", url));
    }
    process::shell_open(&url, None)
}

#[tauri::command]
pub fn rename_device(app: tauri::AppHandle, original: String, new_name: String) {
    standard_log!("[cmd] rename_device: '{}' -> '{}'", original, new_name);
    config::with_config_mut(|c| {
        if new_name.is_empty() || new_name == original {
            c.device_names.remove(&original);
        } else {
            c.device_names.insert(original, new_name);
        }
    });
    let config_snapshot = config::with_config(|c| c.clone());
    let _ = app.emit("config-changed", config_snapshot);
    let _ = app.emit("audio-devices-changed", ());
}

#[tauri::command]
pub fn change_device_group(app: tauri::AppHandle, name: String, group: String) {
    standard_log!("[cmd] change_device_group: {} -> {}", name, group);
    config::with_config_mut(|c| {
        if group.is_empty() {
            c.device_groups.remove(&name);
        } else {
            c.device_groups.insert(name, group);
        }
    });
    let config_snapshot = config::with_config(|c| c.clone());
    let _ = app.emit("config-changed", config_snapshot);
}

#[tauri::command]
pub fn toggle_group_hidden(app: tauri::AppHandle, group: String) {
    standard_log!("[cmd] toggle_group_hidden: {}", group);
    config::with_config_mut(|c| toggle_vec_item(&mut c.hidden_groups, &group));
    let config_snapshot = config::with_config(|c| c.clone());
    let _ = app.emit("config-changed", config_snapshot);
}

#[tauri::command(async)]
pub async fn disconnect_bluetooth_device(device_id: String) -> Result<String, String> {
    standard_log!("[cmd] disconnect_bluetooth_device: {}", device_id);
    run_blocking(move || crate::bluetooth::bt_action(&device_id, "disconnect", false))
        .await?
        .map_err(|e| e.to_string())
}

#[tauri::command(async)]
pub async fn connect_bluetooth_device(device_id: String, is_ble: bool) -> Result<String, String> {
    standard_log!("[cmd] connect_bluetooth_device: {}", device_id);
    run_blocking(move || crate::bluetooth::bt_action(&device_id, "connect", is_ble))
        .await?
        .map_err(|e| e.to_string())
}

#[tauri::command(async)]
pub async fn check_bt_connection(device_id: String) -> Result<Option<bool>, String> {
    Ok(run_blocking(move || crate::bluetooth::check_device_connection(&device_id)).await?)
}

/// 前端行为埋点：写入运行日志（受日志级别门控，标准级可见）
#[tauri::command]
pub fn frontend_log(tag: String, msg: String) {
    standard_log!("[{tag}] {msg}");
}

#[tauri::command]
pub fn open_24g_device_file() -> Result<(), String> {
    let path = crate::device_data::user_data_path();
    if !path.exists() {
        // 内置 2.4G 库已废除，全新安装环境可能没有 data 目录
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        std::fs::write(&path, "{}").map_err(|e| e.to_string())?;
    }
    process::shell_open(&path.to_string_lossy(), None)
}

const TRAY_DEVICE_LIMIT: usize = 4;

#[tauri::command(async)]
pub async fn toggle_device_tray(app: tauri::AppHandle, name: String) -> Result<(), String> {
    let (already_added, count) =
        config::with_config(|c| (c.tray_devices.contains(&name), c.tray_devices.len()));
    if !already_added && count >= TRAY_DEVICE_LIMIT {
        standard_log!(
            "[cmd] toggle_device_tray: {} 达上限({})拒绝",
            name,
            TRAY_DEVICE_LIMIT
        );
        return Err(format!("托盘最多添加 {} 个设备", TRAY_DEVICE_LIMIT));
    }
    standard_log!("[cmd] toggle_device_tray: {}", name);
    run_blocking(move || {
        config::with_config_mut(|c| toggle_vec_item(&mut c.tray_devices, &name));
    })
    .await?;
    crate::tray::refresh_tray_tooltip(&app);
    let config_snapshot = config::with_config(|c| c.clone());
    let _ = app.emit("config-changed", config_snapshot);
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
pub async fn get_audio_sessions(
    device_id: String,
) -> Result<Vec<crate::audio::AudioSession>, String> {
    standard_log!("[cmd] get_audio_sessions: device_id={}", device_id);
    run_blocking(move || {
        // 同步等待 STA 线程完成会话回调注册，确保后续枚举能获取实时音量变化
        crate::audio_notify::request_session_sync_blocking();
        crate::audio::enumerate_audio_sessions(&device_id)
    })
    .await?
    .map_err(|e| e.to_string())
}

#[tauri::command(async)]
pub async fn set_session_volume(
    session_id: String,
    device_id: String,
    volume: f32,
) -> Result<(), String> {
    run_blocking(move || crate::audio::set_session_volume(&session_id, &device_id, volume))
        .await?
        .map_err(|e| e.to_string())
}

#[tauri::command(async)]
pub async fn set_session_mute(
    session_id: String,
    device_id: String,
    muted: bool,
) -> Result<(), String> {
    run_blocking(move || crate::audio::set_session_mute(&session_id, &device_id, muted))
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
pub async fn set_session_device(
    pid: u32,
    direction: String,
    device_id: String,
) -> Result<(), String> {
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

#[tauri::command(async)]
pub async fn get_sessions_device_names(
    pids: Vec<u32>,
) -> Result<std::collections::HashMap<u32, crate::audio::SessionDeviceNames>, String> {
    run_blocking(move || crate::audio::resolve_session_device_names(&pids)).await
}

#[tauri::command]
pub fn set_default_device(app: tauri::AppHandle, device_id: String) -> Result<(), String> {
    standard_log!("[cmd] set_default_device: {}", device_id);
    crate::audio::set_default_device(&device_id).map_err(|e| e.to_string())?;
    let _ = app.emit("audio-devices-changed", ());
    Ok(())
}

#[tauri::command(async)]
pub async fn get_spatial_sound(
    device_id: String,
) -> Result<crate::audio_spatial::SpatialSoundState, String> {
    run_blocking(move || crate::audio_spatial::get_spatial_sound(&device_id)).await?
}

#[tauri::command(async)]
pub async fn set_spatial_sound(
    device_id: String,
    format_guid: Option<String>,
) -> Result<(), String> {
    run_blocking(move || {
        crate::audio_spatial::set_spatial_sound(&device_id, format_guid.as_deref())
    })
    .await?
}

#[tauri::command]
pub fn open_log_dir() -> Result<(), String> {
    let dir = crate::process::logs_dir();
    let _ = std::fs::create_dir_all(&dir);
    process::shell_open(&dir.to_string_lossy(), None)
}

#[tauri::command(async)]
pub async fn check_for_update(
    app: tauri::AppHandle,
    include_prerelease: bool,
) -> Result<crate::update::UpdateStatus, String> {
    let current_version = app.package_info().version.to_string();
    let _ = crate::update::check_and_store("", current_version, include_prerelease).await;
    crate::update::get_last_status().ok_or_else(|| "状态未存储".to_string())
}

#[tauri::command]
pub fn get_update_status() -> Option<crate::update::UpdateStatus> {
    crate::update::get_last_status()
}

fn parse_shortcut(s: &str) -> Result<tauri_plugin_global_shortcut::Shortcut, String> {
    tauri_plugin_global_shortcut::Shortcut::try_from(s).map_err(|e| e.to_string())
}

/// 为一个动作注册快捷键（含事件分发闭包）。**只碰注册表，不写配置**，
/// 便于提交阶段在注册失败时把旧键原样恢复回去。
fn register_shortcut(
    app: &tauri::AppHandle,
    action: &str,
    sc: tauri_plugin_global_shortcut::Shortcut,
    key: &str,
) -> Result<(), String> {
    let action_clone = action.to_string();
    let key_clone = key.to_string();
    app.global_shortcut()
        .on_shortcut(sc, move |_app, _shortcut, event| {
            if event.state != ShortcutState::Pressed {
                return;
            }
            crate::shortcut::dispatch_shortcut_action(_app, &action_clone, &key_clone);
        })
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn set_hotkey_config(
    app: tauri::AppHandle,
    action: String,
    key: Option<String>,
) -> Result<(), String> {
    let prev_key = config::with_config(|c| match action.as_str() {
        "devices" => c.shortcut_devices.clone(),
        "volume" => c.shortcut_volume.clone(),
        "volume_up" => c.shortcut_volume_up.clone(),
        "volume_down" => c.shortcut_volume_down.clone(),
        "volume_mute" => c.shortcut_volume_mute.clone(),
        _ => None,
    });
    let prev_sc = prev_key.as_deref().and_then(|pk| parse_shortcut(pk).ok());

    // ① 校验阶段：**不做任何副作用**。
    // 原实现先注销旧键再校验新键，于是「新键解析失败」这条分支会留下：
    // 注册表里旧键已没了、配置里旧键还在 —— 前端显示「已设置 XX」但按键无反应，
    // 且不广播 config-changed，用户完全无从察觉（见代码审查报告 P1-5）。
    let new_sc = match key.as_deref() {
        Some(k) => {
            let sc = parse_shortcut(k)?; // 解析失败：状态零变化
                                         // 同键重设必须放行：此刻旧键尚未注销（校验先于副作用），它必然处于已注册状态，
                                         // 若按「已占用」拒绝，用户重新选中同一个键就会失败。
            let same_as_prev = prev_sc.as_ref() == Some(&sc);
            if !same_as_prev && app.global_shortcut().is_registered(sc.clone()) {
                return Err("快捷键已被占用".to_string()); // 被占用：状态零变化
            }
            Some(sc)
        }
        None => None,
    };

    // ② 提交阶段：注销旧 → 注册新 → 落配置（顺序不变，只是整体挪到校验之后）
    if let Some(prev) = prev_sc {
        let _ = app.global_shortcut().unregister(prev);
        standard_log!(
            "[hotkey] unregistered old key {} for {}",
            prev_key.as_deref().unwrap_or(""),
            action
        );
    }
    if let Some(sc) = new_sc {
        let new_key_str = key.as_deref().unwrap_or_default();
        if let Err(e) = register_shortcut(&app, &action, sc, new_key_str) {
            // 注册失败：把旧键恢复回去，否则会重现本条要修的那种不一致
            //（注册表空着、配置里留着旧键）
            if let (Some(prev), Some(prev_key_str)) = (prev_sc, prev_key.as_deref()) {
                let _ = register_shortcut(&app, &action, prev, prev_key_str);
            }
            return Err(e);
        }
        // 成功后才记日志：原先在注册尝试之前打印，失败时日志与事实相反
        standard_log!("[hotkey] registered {} {}", new_key_str, action);
    }
    set_config_key(&action, key);
    let config_snapshot = config::with_config(|c| c.clone());
    let _ = app.emit("config-changed", config_snapshot);
    Ok(())
}

fn set_config_key(action: &str, key: Option<String>) {
    config::with_config_mut(|c| match action {
        "devices" => c.shortcut_devices = key,
        "volume" => c.shortcut_volume = key,
        "volume_up" => c.shortcut_volume_up = key,
        "volume_down" => c.shortcut_volume_down = key,
        "volume_mute" => c.shortcut_volume_mute = key,
        _ => {}
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
                let any = c
                    .device_shortcuts
                    .values()
                    .any(|d| d.shortcut.as_deref() == Some(new_key_str));
                let other = c
                    .device_shortcuts
                    .iter()
                    .any(|(id, d)| id != &device_id && d.shortcut.as_deref() == Some(new_key_str));
                (any, other)
            });
            // 关闭共享：被其他设备或非设备功能占用都拒绝（本设备自身占用允许）
            // 开启共享：仅拒绝非设备功能占用
            let conflict = if share_enabled {
                !used_by_any_device
            } else {
                used_by_other_device || !used_by_any_device
            };
            if conflict {
                return Err("快捷键已被占用".to_string());
            }
        }
    }
    standard_log!(
        "[hotkey] set_device_shortcut: {} key={}",
        name,
        key.as_deref().unwrap_or("None")
    );
    set_device_shortcut_key(&device_id, &name, key);
    crate::shortcut::sync_device_shortcuts(&app);
    let config_snapshot = config::with_config(|c| c.clone());
    let _ = app.emit("config-changed", config_snapshot);
    Ok(())
}

fn set_device_shortcut_key(device_id: &str, name: &str, key: Option<String>) {
    config::with_config_mut(|c| {
        if let Some(k) = key {
            c.device_shortcuts.insert(
                device_id.to_string(),
                crate::config::DeviceShortcut {
                    name: name.to_string(),
                    shortcut: Some(k),
                },
            );
        } else if let Some(entry) = c.device_shortcuts.get_mut(device_id) {
            entry.shortcut = None;
            entry.name = name.to_string();
        }
    });
}

#[tauri::command]
pub fn remove_device_shortcut(app: tauri::AppHandle, device_id: String) {
    standard_log!("[hotkey] remove_device_shortcut: {}", device_id);
    config::with_config_mut(|c| {
        c.device_shortcuts.remove(&device_id);
    });
    crate::shortcut::sync_device_shortcuts(&app);
    let config_snapshot = config::with_config(|c| c.clone());
    let _ = app.emit("config-changed", config_snapshot);
}

// ═══════════════════════════════════════════════════════════════
// 窗口材质
// ═══════════════════════════════════════════════════════════════

/// 设置窗口材质并应用到所有窗口
#[tauri::command]
pub fn set_window_material(app: tauri::AppHandle, material: String) -> Result<bool, String> {
    crate::window_material::set_window_material(&app, material)
}

/// 检查系统是否支持指定材质
#[tauri::command]
pub fn check_material_support(material: String) -> Result<bool, String> {
    Ok(crate::window_material::check_material_support(&material))
}

#[cfg(test)]
mod tests {
    use super::is_allowed_open_url;

    /// P1-4 的可证伪单测：`open_url` 只放行白名单协议。
    ///
    /// 修复前 `open_url` 直接把任意字符串交给 `ShellExecuteW`，没有这层判据；
    /// 本测试锁定「前端唯一触达系统执行的入口不得被用作本地落地原语」这一性质。
    #[test]
    fn open_url_rejects_non_whitelisted_schemes() {
        for ok in [
            "https://github.com/oneday5799/PeriTray",
            "http://127.0.0.1:8080/x",
            "ms-settings:sound-defaultoutputproperties",
            "ms-windows-store://pdp/?productid=X",
        ] {
            assert!(is_allowed_open_url(ok), "白名单协议应放行: {ok}");
        }

        for bad in [
            "file:///C:/Windows/System32/calc.exe",
            "javascript:alert(1)",
            "data:text/html,<script>alert(1)</script>",
            "vbscript:msgbox(1)",
            "ftp://example.com/x",
            r"C:/Windows/System32/calc.exe",
            r"\\server\share\payload.exe",
            "shell:startup",
            "",
        ] {
            assert!(!is_allowed_open_url(bad), "非白名单协议必须拒绝: {bad}");
        }
    }
}
