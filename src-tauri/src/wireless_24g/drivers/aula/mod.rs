// ── 模块职责 ─────────────────────────────────────────────
// AULA 域驱动：F75 Max（05AC:024F dongle）与 F99 Pro/X99 Pro
// （3554:FA09 无线接收器）的电量读取，按 VID 分发至各自协议模块。
//
// 仅支持无线接收器模式（有线固件为另一套协议且无电量能力）。
// AULA 接收器为专用单设备——无需槽位扫描/单下游判定。
//
// 未经实机验证：详细档日志（原始帧 hex/形态尝试/校验计算）为
// 唯一远程排障来源。

pub(crate) mod f75max;
pub(crate) mod f99pro;

use super::{BatteryDriver, DeviceIdentity};

/// 已知 AULA 无线设备身份（识别注册表数据源）
const DEVICES: &[(&str, u16, u16)] = &[
    ("AULA F75 Max", f75max::VID, f75max::PID),
    ("AULA Wireless Keyboard", f99pro::VID, f99pro::PID),
];

pub(crate) struct AulaDriver;

pub(crate) static AULA: AulaDriver = AulaDriver;

impl BatteryDriver for AulaDriver {
    fn matches(&self, vid: u16, pid: u16) -> bool {
        DEVICES.iter().any(|(_, v, p)| *v == vid && *p == pid)
    }

    fn read_battery(&self, vid: u16, pid: u16) -> Result<i32, String> {
        let link = crate::wireless_24g::hid_link::HidLink::new()?;
        // 按 VID 分发至对应协议实现；None=非无线连接态，统一转 Err 走负缓存
        let percent = match (vid, pid) {
            (f99pro::VID, f99pro::PID) => f99pro::read_battery_percent(&link)?,
            (f75max::VID, f75max::PID) => f75max::read_battery_percent(&link)?,
            _ => return Err(format!("未收录的 AULA 设备 {:04X}:{:04X}", vid, pid)),
        };
        percent.ok_or_else(|| "非无线连接态，电量不可用".to_string())
    }

    fn identities(&self) -> Vec<DeviceIdentity> {
        DEVICES
            .iter()
            .map(|(name, vid, pid)| DeviceIdentity {
                vid: *vid,
                pid: *pid,
                name,
                dev_type: "keyboard",
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
        let ids = AULA.identities();
        assert_eq!(ids.len(), 2);
        for i in ids {
            assert!(AULA.matches(i.vid, i.pid), "身份表与 matches 不一致");
            assert_eq!(AULA.device_name(i.vid, i.pid), Some(i.name));
        }
    }

    #[test]
    fn rejects_unknown_devices() {
        assert!(!AULA.matches(0x046D, 0xC52B)); // 罗技不归本域
        assert!(!AULA.matches(0x3554, 0xFA08));
    }
}
