use std::collections::HashMap;
use std::sync::{Mutex, OnceLock, RwLock};
use std::time::SystemTime;

use serde::Deserialize;

#[derive(Debug, Clone)]
struct DeviceInfo {
    pub name: String,
    pub device_type: String,
}

#[derive(Deserialize)]
struct RawDeviceEntry {
    name: String,
    #[serde(default)]
    r#type: String,
}

static DEVICE_DATA: OnceLock<RwLock<HashMap<String, HashMap<String, DeviceInfo>>>> =
    OnceLock::new();
static LAST_MTIME: OnceLock<Mutex<Option<(Option<SystemTime>, Option<SystemTime>)>>> =
    OnceLock::new();

fn default_data_path() -> std::path::PathBuf {
    crate::process::exe_dir()
        .join("data")
        .join("wireless_24g_devices.json")
}

fn user_data_path() -> std::path::PathBuf {
    crate::process::exe_dir()
        .join("data")
        .join("wireless_24g_devices_user.json")
}

fn load_data_from_path(path: &std::path::Path) -> HashMap<String, HashMap<String, DeviceInfo>> {
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
        Err(_) => HashMap::new(),
    }
}

fn load_all_data() -> HashMap<String, HashMap<String, DeviceInfo>> {
    let mut result = load_data_from_path(&default_data_path());
    let user = load_data_from_path(&user_data_path());
    let user_count = user.len();
    for (vid, pids) in user {
        let entry = result.entry(vid).or_insert_with(HashMap::new);
        for (pid, info) in pids {
            entry.insert(pid, info);
        }
    }
    crate::process::append_log(&format!(
        "[device_data] loaded {} VIDs ({} user)",
        result.len(),
        user_count
    ));
    result
}

pub fn init_device_data() {
    let data = load_all_data();
    DEVICE_DATA.set(RwLock::new(data)).ok();
}

pub fn reload_device_data() {
    let default_mtime = std::fs::metadata(default_data_path())
        .and_then(|m| m.modified())
        .ok();
    let user_mtime = std::fs::metadata(user_data_path())
        .and_then(|m| m.modified())
        .ok();

    let last = LAST_MTIME.get_or_init(|| Mutex::new(None));
    if let Ok(mut guard) = last.lock() {
        if *guard == Some((default_mtime, user_mtime)) {
            return;
        }
        *guard = Some((default_mtime, user_mtime));
    }

    if let Some(rw_lock) = DEVICE_DATA.get() {
        let new_data = load_all_data();
        if let Ok(mut data) = rw_lock.write() {
            *data = new_data;
        }
    }
}

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
