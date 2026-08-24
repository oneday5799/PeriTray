// ── 模块职责 ─────────────────────────────────────────────
// 2.4G 设备识别注册表：驱动编译期内置身份 ⊕ 用户自定义文件。
// 对外提供 VID/PID → 是否 2.4G / 显示名 / 类型的查询；
// 内置 wireless_24g_devices.json 已废除，识别数据由
// wireless_24g::drivers 各驱动以 identities() 权威声明，
// 用户文件（data/wireless_24g_devices_user.json）同键覆盖、
// 支持免编译扩展与 mtime 热重载。

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock, RwLock};
use std::time::SystemTime;

use serde::Deserialize;

#[derive(Debug, Clone)]
struct DeviceInfo {
    name: String,
    device_type: String,
}

#[derive(Deserialize)]
struct RawDeviceEntry {
    name: String,
    #[serde(default)]
    r#type: String,
}

static DEVICE_DATA: OnceLock<RwLock<HashMap<String, HashMap<String, DeviceInfo>>>> =
    OnceLock::new();
static LAST_MTIME: OnceLock<Mutex<Option<Option<SystemTime>>>> = OnceLock::new();

fn user_data_path() -> std::path::PathBuf {
    crate::process::exe_dir()
        .join("data")
        .join("wireless_24g_devices_user.json")
}

// ── 数据源：用户文件 ─────────────────────────────────────

fn load_user_data(path: &std::path::Path) -> HashMap<String, HashMap<String, DeviceInfo>> {
    match std::fs::read_to_string(path) {
        Ok(content) => {
            match serde_json::from_str::<HashMap<String, HashMap<String, RawDeviceEntry>>>(&content)
            {
                Ok(raw) => {
                    let mut result = HashMap::new();
                    for (vid, pids) in raw {
                        let mut pids_map = HashMap::new();
                        for (pid, entry) in pids {
                            pids_map.insert(
                                pid,
                                DeviceInfo {
                                    name: entry.name,
                                    device_type: if entry.r#type.is_empty() {
                                        "other".to_string()
                                    } else {
                                        entry.r#type
                                    },
                                },
                            );
                        }
                        result.insert(vid, pids_map);
                    }
                    result
                }
                Err(e) => {
                    crate::process::append_log(&format!(
                        "[device_data] JSON parse error ({}): {}",
                        path.display(),
                        e
                    ));
                    HashMap::new()
                }
            }
        }
        // 文件不存在属正常态（用户从未自定义过）
        Err(_) => HashMap::new(),
    }
}

// ── 数据源：驱动内置身份 ─────────────────────────────────

/// 从已注册驱动派生内置身份表，键格式与历史 JSON 一致（大写十六进制）
fn builtin_identities() -> HashMap<String, HashMap<String, DeviceInfo>> {
    let mut result = HashMap::new();
    for driver in crate::wireless_24g::DRIVERS {
        for identity in driver.identities() {
            let vid = format!("{:04X}", identity.vid);
            let pid = format!("{:04X}", identity.pid);
            result.entry(vid).or_insert_with(HashMap::new).insert(
                pid,
                DeviceInfo {
                    name: identity.name.to_string(),
                    device_type: identity.dev_type.to_string(),
                },
            );
        }
    }
    result
}

// ── 合并与加载 ───────────────────────────────────────────

/// 纯合并逻辑：overlay 同键覆盖 base（供单测直接验证优先级）
fn merge_maps(
    base: HashMap<String, HashMap<String, DeviceInfo>>,
    overlay: HashMap<String, HashMap<String, DeviceInfo>>,
) -> HashMap<String, HashMap<String, DeviceInfo>> {
    let mut result = base;
    for (vid, pids) in overlay {
        let entry = result.entry(vid).or_insert_with(HashMap::new);
        for (pid, info) in pids {
            entry.insert(pid, info);
        }
    }
    result
}

fn build_registry(
    user: HashMap<String, HashMap<String, DeviceInfo>>,
) -> HashMap<String, HashMap<String, DeviceInfo>> {
    merge_maps(builtin_identities(), user)
}

pub fn init_device_data() {
    let user = load_user_data(&user_data_path());
    let user_count = count_user_entries(&user);
    let data = build_registry(user);
    crate::process::append_log(&format!(
        "[device_data] registry: {} VIDs (driver builtin, {} user entries)",
        data.len(),
        user_count
    ));
    DEVICE_DATA.set(RwLock::new(data)).ok();
}

/// 用户条目数（仅用于日志观测）
fn count_user_entries(user: &HashMap<String, HashMap<String, DeviceInfo>>) -> usize {
    user.values().map(|pids| pids.len()).sum()
}

pub fn reload_device_data() {
    let user_mtime = std::fs::metadata(user_data_path())
        .and_then(|m| m.modified())
        .ok();

    let last = LAST_MTIME.get_or_init(|| Mutex::new(None));
    if let Ok(mut guard) = last.lock() {
        if *guard == Some(user_mtime) {
            return;
        }
        *guard = Some(user_mtime);
    }

    if let Some(rw_lock) = DEVICE_DATA.get() {
        let new_data = build_registry(load_user_data(&user_data_path()));
        if let Ok(mut data) = rw_lock.write() {
            *data = new_data;
        }
    }
}

// ── 查询接口 ────────────────────────────────────────────

pub fn is_wireless_24g(vid: &str, pid: &str) -> bool {
    let data = DEVICE_DATA.get().and_then(|rw_lock| rw_lock.read().ok());
    data.as_ref()
        .and_then(|d| d.get(vid))
        .map(|pids| pids.contains_key(pid))
        .unwrap_or(false)
}

pub(crate) fn get_device_name(vid: &str, pid: &str) -> Option<String> {
    let data = DEVICE_DATA.get().and_then(|rw_lock| rw_lock.read().ok());
    data.as_ref()
        .and_then(|d| d.get(vid))
        .and_then(|pids| pids.get(pid))
        .map(|info| info.name.clone())
}

pub(crate) fn get_device_type(vid: &str, pid: &str) -> String {
    let data = DEVICE_DATA.get().and_then(|rw_lock| rw_lock.read().ok());
    data.as_ref()
        .and_then(|d| d.get(vid))
        .and_then(|pids| pids.get(pid))
        .map(|info| info.device_type.clone())
        .unwrap_or_else(|| "other".to_string())
}

pub fn extract_vid_pid(pnp_id: &str) -> Option<(String, String)> {
    // 大小写不敏感查找 VID_ 和 PID_，避免整串 to_uppercase()
    let bytes = pnp_id.as_bytes();
    let vid = find_field(bytes, b"VID_")?;
    let pid = find_field(bytes, b"PID_")?;
    Some((vid, pid))
}

fn find_field(bytes: &[u8], marker: &[u8]) -> Option<String> {
    // 大小写不敏感搜索 marker
    let mut i = 0;
    'outer: while i + marker.len() <= bytes.len() {
        for (j, &m) in marker.iter().enumerate() {
            let b = bytes[i + j];
            if b != m && b.to_ascii_uppercase() != m {
                i += 1;
                continue 'outer;
            }
        }
        // 找到 marker，提取后续 4 个字符
        let start = i + marker.len();
        if start + 4 > bytes.len() {
            return None;
        }
        return Some(String::from_utf8_lossy(&bytes[start..start + 4]).to_uppercase());
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(name: &str, t: &str) -> DeviceInfo {
        DeviceInfo {
            name: name.to_string(),
            device_type: t.to_string(),
        }
    }

    fn sample_map(
        entries: &[(&str, &str, &str, &str)],
    ) -> HashMap<String, HashMap<String, DeviceInfo>> {
        let mut map: HashMap<String, HashMap<String, DeviceInfo>> = HashMap::new();
        for (vid, pid, name, t) in entries {
            map.entry(vid.to_string())
                .or_insert_with(HashMap::new)
                .insert(pid.to_string(), info(name, t));
        }
        map
    }

    #[test]
    fn user_entries_override_builtin_on_same_key() {
        let builtin = sample_map(&[("1532", "0094", "Razer Orochi V2", "mouse")]);
        let user = sample_map(&[("1532", "0094", "我的 Orochi", "mouse")]);
        let merged = merge_maps(builtin, user);
        assert_eq!(merged["1532"]["0094"].name, "我的 Orochi");
    }

    #[test]
    fn disjoint_keys_are_unioned() {
        let builtin = sample_map(&[
            ("1532", "0094", "Razer Orochi V2", "mouse"),
            ("046D", "C52B", "Logitech Unifying Receiver", "other"),
        ]);
        let user = sample_map(&[("25A7", "A101", "自定义键盘", "keyboard")]);
        let merged = merge_maps(builtin, user);
        assert!(merged.contains_key("1532"));
        assert!(merged.contains_key("046D"));
        assert!(merged.contains_key("25A7"));
    }

    #[test]
    fn builtin_identities_cover_all_drivers() {
        // 注册表必须完整包含所有驱动声明的身份（结构性防死行）
        let builtin = builtin_identities();
        for driver in crate::wireless_24g::DRIVERS {
            for identity in driver.identities() {
                let vid = format!("{:04X}", identity.vid);
                let pid = format!("{:04X}", identity.pid);
                let entry = builtin
                    .get(&vid)
                    .and_then(|pids| pids.get(&pid))
                    .expect("驱动身份未进入注册表");
                assert_eq!(entry.name, identity.name);
            }
        }
    }
}
