// ── 模块职责 ─────────────────────────────────────────────
// 雷蛇键盘域驱动：收录 OpenRazer 全系支持电量上报的雷蛇无线键盘
// （12 个 PID：BlackWidow V3/V4、DeathStalker V2 系列，含同款有线
// 形态）。协议原语与参数常量复用上级 mod.rs；除参数表外与鼠标域
// 结构一致。均未经实机逐一验证，依赖回显校验/负缓存兜底。

use super::super::{BatteryDriver, DeviceIdentity};
use super::{read_battery_level, TXID_KBD_WL, TXID_MID, TXID_NEW, WAIT_DEFAULT_MS, WAIT_KBD_WL_MS};
use crate::wireless_24g::hid_link::HidLink;

// ── 设备能力表 ───────────────────────────────────────────
// 时序与接口索引照抄 razerkbd_driver.c razer_get_report_params()；
// 接口索引差异（0x02/0x03）由 HID 集合逐一试探覆盖。

struct KbdDev {
    vid_pid: (u16, u16),
    /// 设备显示名（识别注册表与日志共用）
    name: &'static str,
    /// 事务 ID
    txid: u8,
    /// 发送后等待响应的时长
    wait_ms: u64,
}

macro_rules! dev {
    ($pid:expr, $name:expr, $txid:expr, $wait:expr) => {
        KbdDev {
            vid_pid: (0x1532, $pid),
            name: $name,
            txid: $txid,
            wait_ms: $wait,
        }
    };
}

static DEVICES: &[KbdDev] = &[
    // ── BlackWidow V3 Mini HyperSpeed · 有线 1ms / 无线 5ms ──
    dev!(
        0x0258,
        "Razer BlackWidow V3 Mini HyperSpeed",
        TXID_NEW,
        WAIT_DEFAULT_MS
    ), // 有线
    dev!(
        0x0271,
        "Razer BlackWidow V3 Mini HyperSpeed",
        TXID_KBD_WL,
        WAIT_KBD_WL_MS
    ), // 无线
    // ── BlackWidow V3 Pro · 有线 1ms / 无线 5ms ──
    dev!(0x025A, "Razer BlackWidow V3 Pro", TXID_MID, WAIT_DEFAULT_MS), // 有线
    dev!(
        0x025C,
        "Razer BlackWidow V3 Pro",
        TXID_KBD_WL,
        WAIT_KBD_WL_MS
    ), // 无线
    // ── BlackWidow V4 Mini HyperSpeed · 有线 1ms / 无线 5ms ──
    dev!(
        0x02B9,
        "Razer BlackWidow V4 Mini HyperSpeed",
        TXID_NEW,
        WAIT_DEFAULT_MS
    ), // 有线
    dev!(
        0x02BA,
        "Razer BlackWidow V4 Mini HyperSpeed",
        TXID_KBD_WL,
        WAIT_KBD_WL_MS
    ), // 无线
    // ── BlackWidow V4 Tenkeyless HyperSpeed · 有线 1ms / 无线 5ms ──
    dev!(
        0x02D7,
        "Razer BlackWidow V4 Tenkeyless HyperSpeed",
        TXID_NEW,
        WAIT_DEFAULT_MS
    ), // 有线
    dev!(
        0x02D5,
        "Razer BlackWidow V4 Tenkeyless HyperSpeed",
        TXID_KBD_WL,
        WAIT_KBD_WL_MS
    ), // 无线
    // ── DeathStalker V2 Pro · 有线 1ms / 无线 5ms ──
    dev!(
        0x0292,
        "Razer DeathStalker V2 Pro",
        TXID_NEW,
        WAIT_DEFAULT_MS
    ), // 有线
    dev!(
        0x0290,
        "Razer DeathStalker V2 Pro",
        TXID_KBD_WL,
        WAIT_KBD_WL_MS
    ), // 无线
    // ── DeathStalker V2 Pro Tenkeyless · 有线 1ms / 无线 5ms ──
    dev!(
        0x0298,
        "Razer DeathStalker V2 Pro Tenkeyless",
        TXID_NEW,
        WAIT_DEFAULT_MS
    ), // 有线
    dev!(
        0x0296,
        "Razer DeathStalker V2 Pro Tenkeyless",
        TXID_KBD_WL,
        WAIT_KBD_WL_MS
    ), // 无线
];

// ── 驱动实现 ────────────────────────────────────────────

pub(crate) struct RazerKeyboardDriver;

pub(crate) static RAZER_KEYBOARD: RazerKeyboardDriver = RazerKeyboardDriver;

impl BatteryDriver for RazerKeyboardDriver {
    fn matches(&self, vid: u16, pid: u16) -> bool {
        DEVICES.iter().any(|d| d.vid_pid == (vid, pid))
    }

    fn read_battery(&self, link: &HidLink, vid: u16, pid: u16) -> Result<i32, String> {
        let dev = DEVICES
            .iter()
            .find(|d| d.vid_pid == (vid, pid))
            .ok_or_else(|| format!("未收录的雷蛇设备 {:04X}:{:04X}", vid, pid))?;
        read_battery_level(link, vid, pid, dev.txid, dev.wait_ms)
    }

    fn identities(&self) -> Vec<DeviceIdentity> {
        DEVICES
            .iter()
            .map(|d| DeviceIdentity {
                vid: d.vid_pid.0,
                pid: d.vid_pid.1,
                name: d.name,
                dev_type: "keyboard",
            })
            .collect()
    }

    fn device_name(&self, vid: u16, pid: u16) -> Option<&'static str> {
        DEVICES
            .iter()
            .find(|d| d.vid_pid == (vid, pid))
            .map(|d| d.name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_table_has_no_duplicate_pids() {
        let mut pids: Vec<(u16, u16)> = DEVICES.iter().map(|d| d.vid_pid).collect();
        let total = pids.len();
        pids.sort_unstable();
        pids.dedup();
        assert_eq!(pids.len(), total, "设备表存在重复 VID:PID");
    }

    #[test]
    fn identities_derive_one_to_one_from_table() {
        let ids = RAZER_KEYBOARD.identities();
        assert_eq!(ids.len(), DEVICES.len());
        assert!(ids.iter().all(|i| !i.name.is_empty()), "存在空名称身份");
        assert!(ids.iter().all(|i| i.dev_type == "keyboard"));
        for i in ids.iter().take(4) {
            assert_eq!(RAZER_KEYBOARD.device_name(i.vid, i.pid), Some(i.name));
        }
    }
}
