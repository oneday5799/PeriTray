// ── 模块职责 ─────────────────────────────────────────────
// Flydigi 域驱动：Vader 4 Pro（04B4:2412 旧协议）与 Vader 5 Pro
// （37D7:2401 V2 GET_INFO 协议，有线/2.4G 共用身份）的电量读取。
// 两种协议帧格式互不兼容，各占一个设备文件，勿合并。
// Dock 2 底座（37D7:6001）是独立外设，电量走被动 EF 状态机、需底座实机
// 验证，未收录（也见 tests::rejects_unknown_devices）。
// 旧协议事实转录自 Jackwmtr/flydigi-apex4-linux 的 PROTOCOL.md；
// V2 协议转录自 vermi5/VaderBatteryTray。

pub(crate) mod vader4pro;
pub(crate) mod vader5pro;

use super::{BatteryDriver, DeviceIdentity};
use crate::wireless_24g::hid_link::{HidLink, HidPath};

/// vendor 命令通道的 usage page（两代 Flydigi 手柄一致）
const VENDOR_USAGE_PAGE: u16 = 0xFFA0;
/// vendor 命令通道的 usage（参考实现 FindTargetInterface 对 page+usage 双校验）
const VENDOR_USAGE: u16 = 0x0001;

/// 候选挑选共享原语：优先 vendor 集合（page+usage 双校验），缺失时回退全候选
/// ——回退出的集合由调用方以各自协议的回显校验确认有效通道，不会误读。
/// 两代手柄的候选循环共用此入口，勿在各设备文件另写一份挑选逻辑。
pub(crate) fn prefer_vendor_page(paths: &[HidPath]) -> Vec<&HidPath> {
    let preferred: Vec<_> = paths
        .iter()
        .filter(|p| p.usage_page == VENDOR_USAGE_PAGE && p.usage == VENDOR_USAGE)
        .collect();
    if preferred.is_empty() {
        paths.iter().collect()
    } else {
        preferred
    }
}

/// 已知 Flydigi 无线设备身份（识别注册表数据源）
const DEVICES: &[(&str, u16, u16)] = &[
    ("Flydigi Vader 4 Pro", vader4pro::VID, vader4pro::PID),
    ("Flydigi Vader 5 Pro", vader5pro::VID, vader5pro::PID),
];

pub(crate) struct FlydigiDriver;

pub(crate) static FLYDIGI: FlydigiDriver = FlydigiDriver;

impl BatteryDriver for FlydigiDriver {
    fn matches(&self, vid: u16, pid: u16) -> bool {
        DEVICES.iter().any(|(_, v, p)| *v == vid && *p == pid)
    }

    fn read_battery(
        &self,
        link: &HidLink,
        vid: u16,
        pid: u16,
        scope: Option<&str>,
    ) -> Result<i32, String> {
        let percent = match (vid, pid) {
            (vader4pro::VID, vader4pro::PID) => vader4pro::read_battery_percent(link, scope)?,
            (vader5pro::VID, vader5pro::PID) => vader5pro::read_battery_percent(link, scope)?,
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
                dev_type: "gamepad",
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
        assert_eq!(ids.len(), 2);
        for i in ids {
            assert!(FLYDIGI.matches(i.vid, i.pid), "身份表与 matches 不一致");
            assert_eq!(FLYDIGI.device_name(i.vid, i.pid), Some(i.name));
        }
    }

    #[test]
    fn rejects_unknown_devices() {
        assert!(!FLYDIGI.matches(0x046D, 0xC52B)); // 罗技不归本域
        assert!(!FLYDIGI.matches(0x05AC, 0x024F)); // AULA 不归本域
        assert!(!FLYDIGI.matches(0x37D7, 0x6001)); // Dock 2 底座不归本域（独立外设，未收录）
    }
}
