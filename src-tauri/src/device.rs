use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum DevType {
    Audio,
    Battery,
    Monitor,
    Other,
    Usb,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Device {
    pub name: String,
    pub dt: DevType,
    pub status: String,
    pub battery: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device_id: Option<String>,
    #[serde(default)]
    pub is_bluetooth: bool,
    #[serde(default)]
    pub is_wireless_24g: bool,
    #[serde(default)]
    pub is_ble: bool,
}

// 设备ID存储 — 用于蓝牙连接/断开操作
static DEVICE_IDS: OnceLock<Mutex<HashMap<String, (String, bool)>>> = OnceLock::new();

fn get_device_ids() -> &'static Mutex<HashMap<String, (String, bool)>> {
    DEVICE_IDS.get_or_init(|| Mutex::new(HashMap::new()))
}

pub fn store_device_ids(devices: &[Device]) {
    if let Ok(mut ids) = get_device_ids().lock() {
        ids.clear();
        for dev in devices {
            if let Some(ref device_id) = dev.device_id {
                ids.insert(dev.name.clone(), (device_id.clone(), dev.is_ble));
            }
        }
        crate::process::append_log(&format!("[device] 更新设备 ID 映射: {} 条", ids.len()));
    }
}

pub fn get_device_id_by_name(name: &str) -> Option<String> {
    get_device_ids()
        .lock()
        .ok()?
        .get(name)
        .map(|(id, _)| id.clone())
}

pub fn is_ble_device(name: &str) -> bool {
    get_device_ids()
        .lock()
        .ok()
        .and_then(|ids| ids.get(name).map(|(_, ble)| *ble))
        .unwrap_or(false)
}
