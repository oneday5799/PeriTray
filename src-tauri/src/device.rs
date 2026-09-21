use serde::{Deserialize, Serialize};

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
    /// 物理设备身份键（`c:` 容器 / `i:` 实例 / `n:` 名称，见 `device_identity`）。
    /// 与 `device_id` 不同：`device_id` 只承载蓝牙 WinRT ID，本字段对**所有**设备都尽力填充。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device_key: Option<String>,
    #[serde(default)]
    pub is_bluetooth: bool,
    #[serde(default)]
    pub is_wireless_24g: bool,
    #[serde(default)]
    pub is_ble: bool,
}
