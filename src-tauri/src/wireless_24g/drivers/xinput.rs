// ── 模块职责 ─────────────────────────────────────────────
// XInput 设备身份声明：仅注册 Xbox 360 Controller 的 VID/PID
// 到识别注册表，使其通过 is_generic_hid 过滤进入设备列表。
// 实际电量读取由 wireless_24g::query_and_cache 中的 XInput 兜底完成，
// 此驱动的 read_battery 始终返回 Err（不走 HID 通道）。

use super::{BatteryDriver, DeviceIdentity};
use crate::wireless_24g::hid_link::HidLink;

const DEVICES: &[(&str, u16, u16)] = &[
    ("Xbox 360 Controller", 0x045E, 0x028E),
    ("Xbox One Controller", 0x045E, 0x02D1),
];

pub(crate) struct XInputDriver;

pub(crate) static XINPUT: XInputDriver = XInputDriver;

impl BatteryDriver for XInputDriver {
    fn matches(&self, vid: u16, pid: u16) -> bool {
        DEVICES.iter().any(|(_, v, p)| *v == vid && *p == pid)
    }

    fn read_battery(&self, _link: &HidLink, _vid: u16, _pid: u16) -> Result<i32, String> {
        // XInput 电量由 wireless_24g::query_and_cache 中的 XInput 兜底读取，
        // 不走 HID 通道；此处仅声明设备身份
        Err("XInput 电量由上层兜底读取".into())
    }

    fn identities(&self) -> Vec<DeviceIdentity> {
        DEVICES
            .iter()
            .map(|(name, vid, pid)| DeviceIdentity {
                vid: *vid,
                pid: *pid,
                name,
                dev_type: "other",
            })
            .collect()
    }

    fn device_name(&self, vid: u16, pid: u16) -> Option<&'static str> {
        DEVICES
            .iter()
            .find(|(_, v, p)| *v == vid && *p == pid)
            .map(|(n, _, _)| *n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identities_match_dispatch_table() {
        let ids = XINPUT.identities();
        assert_eq!(ids.len(), 2);
        for i in ids {
            assert!(XINPUT.matches(i.vid, i.pid), "身份表与 matches 不一致");
            assert_eq!(XINPUT.device_name(i.vid, i.pid), Some(i.name));
        }
    }

    #[test]
    fn rejects_unknown_devices() {
        assert!(!XINPUT.matches(0x046D, 0xC52B));
        assert!(!XINPUT.matches(0x04B4, 0x2412));
    }
}
