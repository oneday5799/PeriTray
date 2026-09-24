use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum DevType {
    Audio,
    Battery,
    Monitor,
    Other,
    Usb,
}

/// 2.4G 设备的具体外设类型，供任务栏无音频端点时选择图标。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Wireless24gKind {
    Mouse,
    Keyboard,
    Gamepad,
}

impl Wireless24gKind {
    /// 把 2.4G 驱动注册表的类型字面归一为任务栏所需的三种外设。
    pub fn from_device_type(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "mouse" => Some(Self::Mouse),
            "keyboard" => Some(Self::Keyboard),
            "gamepad" | "controller" => Some(Self::Gamepad),
            _ => None,
        }
    }
}

/// 统一设备列表中的连接状态判据。
///
/// WMI/PnP 使用 `OK`，蓝牙查询使用 `已连接` / `已配对`；只有明确的未连接/配对状态
/// 才应让任务栏置灰。未知但非空状态按已连接处理，避免 Xbox 这类非蓝牙设备因状态文案
/// 不同而被误判为占位条目。
pub fn status_is_connected(status: &str) -> bool {
    let s = status.trim();
    if s.is_empty() {
        return false;
    }
    !matches!(
        s.to_ascii_lowercase().as_str(),
        "已配对" | "paired" | "disconnected" | "未连接" | "offline"
    )
}

#[cfg(test)]
mod tests {
    use super::{status_is_connected, Wireless24gKind};

    #[test]
    fn connected_status_accepts_pnp_and_rejects_paired_or_offline() {
        assert!(status_is_connected("OK"));
        assert!(status_is_connected("已连接"));
        assert!(status_is_connected("Connected"));
        assert!(!status_is_connected("已配对"));
        assert!(!status_is_connected("paired"));
        assert!(!status_is_connected("offline"));
        assert!(!status_is_connected(""));
    }

    #[test]
    fn wireless_24g_kind_accepts_driver_type_literals() {
        assert_eq!(
            Wireless24gKind::from_device_type("mouse"),
            Some(Wireless24gKind::Mouse)
        );
        assert_eq!(
            Wireless24gKind::from_device_type("keyboard"),
            Some(Wireless24gKind::Keyboard)
        );
        assert_eq!(
            Wireless24gKind::from_device_type("gamepad"),
            Some(Wireless24gKind::Gamepad)
        );
        assert_eq!(
            Wireless24gKind::from_device_type("controller"),
            Some(Wireless24gKind::Gamepad)
        );
        assert_eq!(Wireless24gKind::from_device_type("other"), None);
    }
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
    /// 2.4G 设备的具体类型；非 2.4G 或未识别设备为 `None`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wireless_24g_kind: Option<Wireless24gKind>,
    /// 该设备节点是否处于连接/在线状态；占位设备不会进入 `Device`。
    #[serde(default)]
    pub is_connected: bool,
    #[serde(default)]
    pub is_ble: bool,
}
