use crate::standard_log;
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

/// 计算「需要注销」「需要注册」两个差集。
///
/// 抽成纯函数的两个理由：
/// ① 它决定了快捷键注册表与配置的一致性（漏注销会留下幽灵热键，漏注册会让快捷键失效），
///    而这些语义**可以在单测里钉死**，不必依赖真实插件；
/// ② 让 `sync_device_shortcuts` 的锁内区退化成「取快照 + 调本函数」，
///    锁的作用范围一眼可审（见该函数的锁纪律注释）。
///
/// 返回顺序不保证稳定（来自 `HashSet` 迭代），调用方不得依赖顺序。
fn diff_keys(
    registered: &HashSet<String>,
    desired: &HashSet<String>,
) -> (Vec<String>, Vec<String>) {
    (
        registered
            .iter()
            .filter(|k| !desired.contains(*k))
            .cloned()
            .collect(),
        desired
            .iter()
            .filter(|k| !registered.contains(*k))
            .cloned()
            .collect(),
    )
}

/// 根据配置中的设备快捷键集合同步全局快捷键注册。
/// 同一快捷键键仅注册一次，action 为 `device_shortcut_key:<key>`，
/// 多个设备可共用同一键（触发后在设备间循环切换）。
///
/// ⚠️ **锁纪律**：本函数分三段，`DEVICE_REGISTERED_KEYS` 只在第 1、3 段（纯内存差集
/// 计算与集合更新）持有，**所有 `global_shortcut()` 调用都必须在第 2 段（锁外）**。
/// 原因：该插件的 `register` / `unregister` / `on_shortcut` 内部经 `run_main_thread!`
/// 展开为 `run_on_main_thread(..) + rx.recv()`（**recv 无超时**，见
/// tauri-plugin-global-shortcut-2.3.2/src/lib.rs:75），即**同步等主线程**；
/// 持锁调用时，只要主线程此刻正等本锁（例如它正在 `with_config_mut` 里写配置），
/// 就是 AB/BA 死锁——与 P0-4（`tray.rs` 持配置锁调菜单 API）完全同型。
/// 当前调用点都在主线程（主线程内 `send_user_message` 走内联执行，暂不自锁），
/// 但一旦有子线程调用本函数即会复现，故此处按锁纪律写死。
pub fn sync_device_shortcuts(app: &tauri::AppHandle) {
    let desired: HashSet<String> = crate::config::with_config(|c| {
        c.device_shortcuts
            .values()
            .filter_map(|d| d.shortcut.clone())
            .collect()
    });

    // ── 第 1 段：锁内只算差集（纯内存），算完立即释放 ──────────────
    let (to_unregister, to_register): (Vec<String>, Vec<String>) = {
        let registered = crate::state::lock_unpoisoned(&DEVICE_REGISTERED_KEYS);
        diff_keys(&registered, &desired)
    }; // ← 锁在此释放，下面所有插件调用都不再持锁

    // ── 第 2 段：锁外调用插件 API ────────────────────────────────
    for key in &to_unregister {
        if let Ok(sc) = tauri_plugin_global_shortcut::Shortcut::try_from(key.as_str()) {
            let _ = app.global_shortcut().unregister(sc);
        }
    }

    let mut newly_registered: Vec<String> = Vec::with_capacity(to_register.len());
    for key in &to_register {
        let sc = match tauri_plugin_global_shortcut::Shortcut::try_from(key.as_str()) {
            Ok(sc) => sc,
            Err(_) => {
                standard_log!("[shortcut] invalid key: {}", key);
                continue;
            }
        };
        let action = format!("device_shortcut_key:{}", key);
        let key_str = key.clone();
        standard_log!("[shortcut] registered {} -> {}", key, action);
        let _ = app
            .global_shortcut()
            .on_shortcut(sc, move |_app, _shortcut, event| {
                if event.state != ShortcutState::Pressed {
                    return;
                }
                dispatch_shortcut_action(_app, &action, &key_str);
            });
        newly_registered.push(key.clone());
    }

    // ── 第 3 段：锁内只更新集合（纯内存）──────────────────────────
    {
        let mut registered = crate::state::lock_unpoisoned(&DEVICE_REGISTERED_KEYS);
        registered.retain(|k| desired.contains(k));
        for key in newly_registered {
            registered.insert(key);
        }
    }
}

fn register_single(app: &tauri::AppHandle, key: &str, action: &'static str) {
    let sc = match tauri_plugin_global_shortcut::Shortcut::try_from(key) {
        Ok(sc) => sc,
        Err(_) => {
            standard_log!("[shortcut] invalid key: {}", key);
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
    standard_log!("[shortcut] registered {} -> {}", key, action);
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
        standard_log!("[hotkey] no connected devices for shared key {}", key);
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

    standard_log!(
        "[hotkey] device shortcut '{}' -> switch default to {}",
        key,
        next.name
    );
    if let Err(e) = crate::audio::set_default_device(&next.id) {
        standard_log!("[hotkey] set default device failed: {}", e);
    } else {
        let notify = crate::config::with_config(|c| c.shortcut_switch_notify);
        if notify {
            let display = crate::config::with_config(|c| {
                c.device_names.get(&next.name).cloned().unwrap_or_else(|| {
                    if c.simplify_device_names {
                        crate::tray::simplify_device_name(&next.name).to_string()
                    } else {
                        next.name.clone()
                    }
                })
            });
            #[cfg(target_os = "windows")]
            {
                let icon = crate::windows::resolve_toast_icon();
                crate::toast::show_toast(
                    "音频设备切换提示",
                    &format!("音频设备已切换到「{}」", display),
                    icon.as_deref(),
                );
            }
        }
    }
    let _ = app.emit("audio-devices-changed", ());
}

pub(crate) fn dispatch_shortcut_action(app: &tauri::AppHandle, action: &str, key: &str) {
    if crate::state::SHORTCUT_RECORDING.load(Ordering::Relaxed) {
        // 录制期间：不执行动作，把按下的键上报给前端用于录制
        standard_log!("[hotkey] captured while recording: {}", key);
        let _ = app.emit("shortcut-recorded", key);
        return;
    }
    if let Some(key) = action.strip_prefix("device_shortcut_key:") {
        standard_log!("[hotkey] device shortcut key triggered: {}", key);
        cycle_device_shortcut(app, key);
        return;
    }
    match action {
        "devices" => crate::popup::open_popup(app, "devices"),
        "volume" => crate::popup::open_popup(app, "volume"),
        "volume_up" => {
            standard_log!("[hotkey] volume action: {} (key={})", action, key);
            crate::audio::adjust_default_volume_up()
        }
        "volume_down" => {
            standard_log!("[hotkey] volume action: {} (key={})", action, key);
            crate::audio::adjust_default_volume_down()
        }
        "volume_mute" => {
            standard_log!("[hotkey] volume action: {} (key={})", action, key);
            crate::audio::toggle_default_mute()
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `diff_keys` 返回的 Vec 来自 `HashSet` 迭代，顺序不稳定；
    /// 断言一律先归一成 `HashSet` 再比较，避免把实现细节写进测试。
    fn to_set(v: Vec<String>) -> HashSet<String> {
        v.into_iter().collect()
    }

    fn set(items: &[&str]) -> HashSet<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    /// 首次启动：注册表为空，配置里有一堆设备键 ⇒ 全部待注册、零待注销。
    /// 这条钉的是「漏注册」方向——若 `to_register` 被写成空，用户快捷键会静默失效。
    #[test]
    fn empty_registry_registers_everything() {
        let (unregister, register) =
            diff_keys(&HashSet::new(), &set(&["Ctrl+Alt+1", "Ctrl+Alt+2"]));
        assert!(unregister.is_empty(), "注册表为空时不应有需要注销的键");
        assert_eq!(to_set(register), set(&["Ctrl+Alt+1", "Ctrl+Alt+2"]));
    }

    /// 用户清空了所有设备快捷键 ⇒ 全部待注销、零待注册。
    /// 这条钉的是「漏注销」方向——漏了会在系统里留下幽灵热键，
    /// 用户按下去触发的是已被删除的设备切换动作。
    #[test]
    fn clearing_config_unregisters_everything() {
        let (unregister, register) =
            diff_keys(&set(&["Ctrl+Alt+1", "Ctrl+Alt+2"]), &HashSet::new());
        assert_eq!(to_set(unregister), set(&["Ctrl+Alt+1", "Ctrl+Alt+2"]));
        assert!(register.is_empty(), "配置为空时不应有需要注册的键");
    }

    /// 两个集合完全相同 ⇒ 双空，即「幂等」：重复调用不会反复注销/重注册
    /// （重注册会让 `on_shortcut` 的 handler 累积叠加）。
    #[test]
    fn identical_sets_are_noop() {
        let s = set(&["Ctrl+Alt+1", "Ctrl+Alt+2"]);
        let (unregister, register) = diff_keys(&s, &s);
        assert!(unregister.is_empty(), "集合相同时不应重复注销");
        assert!(register.is_empty(), "集合相同时不应重复注册");
    }

    /// 改键场景（最常见的增量路径）：只动差异部分，交集的键两边都不出现。
    /// 若实现误用 `union` / `symmetric_difference` 之外的写法，
    /// 交集中的键会被误注销（导致刚改好的键失效）或被误注册（handler 叠加）。
    #[test]
    fn intersection_is_never_touched() {
        let (unregister, register) = diff_keys(&set(&["A", "B"]), &set(&["B", "C"]));
        assert_eq!(to_set(unregister), set(&["A"]), "只有 A 应被注销");
        assert_eq!(to_set(register), set(&["C"]), "只有 C 应被注册");
    }

    /// 空集对空集（无设备快捷键且注册表为空）⇒ 双空，不应有任何副作用。
    #[test]
    fn both_empty_is_noop() {
        let (unregister, register) = diff_keys(&HashSet::new(), &HashSet::new());
        assert!(unregister.is_empty() && register.is_empty());
    }
}
