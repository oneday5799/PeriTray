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
    #[serde(default)]
    pub is_bluetooth: bool,
    #[serde(default)]
    pub is_wireless_24g: bool,
    #[serde(default)]
    pub is_ble: bool,
}
