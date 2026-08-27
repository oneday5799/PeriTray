// ── 模块职责 ─────────────────────────────────────────────
// Flydigi 域驱动：Vader 4 Pro（04B4:2412 接收器）的电量读取。
// Flydigi 旧协议（与 Apex 4 / Vader 3 Pro 共用）走 vendor 接口
// （usage page 0xFFA0），写命令 + 轮询回复流。
// 协议事实转录自 Jackwmtr/flydigi-apex4-linux 的 PROTOCOL.md。

pub(crate) mod vader4pro;

use super::{BatteryDriver, DeviceIdentity};

/// 已知 Flydigi 无线设备身份（识别注册表数据源）
const DEVICES: &[(&str, u16, u16)] = &[("Flydigi Vader 4 Pro", vader4pro::VID, vader4pro::PID)];

pub(crate) struct FlydigiDriver;

pub(crate) static FLYDIGI: FlydigiDriver = FlydigiDriver;

impl BatteryDriver for FlydigiDriver {
    fn matches(&self, vid: u16, pid: u16) -> bool {
        DEVICES.iter().any(|(_, v, p)| *v == vid && *p == pid)
    }

    fn read_battery(&self, vid: u16, pid: u16) -> Result<i32, String> {
        let link = crate::wireless_24g::hid_link::HidLink::new()?;
        let percent = match (vid, pid) {
            (vader4pro::VID, vader4pro::PID) => vader4pro::read_battery_percent(&link)?,
            _ => return Err(format!("未收录的 Flydigi 设备 {:04X}:{:04X}", vid, pid)),
        };
        percent.ok_or_else(|| "充电中或无效电量值".to_string())
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
        let ids = FLYDIGI.identities();
        assert_eq!(ids.len(), 1);
        for i in ids {
            assert!(FLYDIGI.matches(i.vid, i.pid), "身份表与 matches 不一致");
            assert_eq!(FLYDIGI.device_name(i.vid, i.pid), Some(i.name));
        }
    }

    #[test]
    fn rejects_unknown_devices() {
        assert!(!FLYDIGI.matches(0x046D, 0xC52B)); // 罗技不归本域
        assert!(!FLYDIGI.matches(0x05AC, 0x024F)); // AULA 不归本域
    }
}
