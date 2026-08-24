// ── 模块职责 ─────────────────────────────────────────────
// 雷蛇鼠标域驱动：收录 OpenRazer 全系支持电量上报的雷蛇无线鼠标
// （64 个 PID，含同款有线形态；蓝牙 PID 不收）。
// 协议编解码原语与参数常量见上级 mod.rs；本文件只维护设备表与
// 驱动实现。除 Orochi V2（0x0094）经实机验证外，其余型号为同族
// 协议移植，依赖回显校验/负缓存兜底。

use super::super::{BatteryDriver, DeviceIdentity};
use super::{
    build_report, parse_level, MAX_RETRIES, RETRY_INTERVAL, TXID_LEGACY, TXID_MID, TXID_NEW,
    WAIT_ATHERIS_MS, WAIT_DEFAULT_MS, WAIT_NEW_MS, WAIT_VIPER_MS,
};
use crate::wireless_24g::hid_link::HidLink;

// ── 设备能力表 ───────────────────────────────────────────
// 蓝牙形态 PID 不收录（电量归系统蓝牙栈）；
// 同款鼠标的有线/无线形态均收录（有线连接也可读电量）。

struct RazerDev {
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
        RazerDev {
            vid_pid: (0x1532, $pid),
            name: $name,
            txid: $txid,
            wait_ms: $wait,
        }
    };
}

static DEVICES: &[RazerDev] = &[
    // ── 新代协议 txid=0x1F · 默认 31ms ──
    dev!(0x008F, "Razer Naga Pro", TXID_NEW, WAIT_NEW_MS), // 有线
    dev!(0x0090, "Razer Naga Pro", TXID_NEW, WAIT_NEW_MS), // 无线
    dev!(0x0088, "Razer Basilisk Ultimate", TXID_NEW, WAIT_NEW_MS), // 接收器
    dev!(0x0086, "Razer Basilisk Ultimate", TXID_NEW, WAIT_NEW_MS), // 有线
    dev!(0x006F, "Razer Lancehead Wireless", TXID_NEW, WAIT_NEW_MS), // 接收器
    dev!(0x0070, "Razer Lancehead Wireless", TXID_NEW, WAIT_NEW_MS), // 有线
    dev!(0x0077, "Razer Pro Click", TXID_NEW, WAIT_NEW_MS), // 接收器
    dev!(0x0080, "Razer Pro Click", TXID_NEW, WAIT_NEW_MS), // 有线
    dev!(
        0x009C,
        "Razer DeathAdder V2 X HyperSpeed",
        TXID_NEW,
        WAIT_NEW_MS
    ),
    dev!(0x00A6, "Razer Viper V2 Pro", TXID_NEW, WAIT_NEW_MS), // 无线
    dev!(0x00A5, "Razer Viper V2 Pro", TXID_NEW, WAIT_NEW_MS), // 有线
    dev!(0x00B0, "Razer Cobra Pro", TXID_NEW, WAIT_NEW_MS),    // 无线
    dev!(0x00AF, "Razer Cobra Pro", TXID_NEW, WAIT_NEW_MS),    // 有线
    dev!(0x00B7, "Razer DeathAdder V3 Pro", TXID_NEW, WAIT_NEW_MS), // 无线
    dev!(0x00B6, "Razer DeathAdder V3 Pro", TXID_NEW, WAIT_NEW_MS), // 有线
    dev!(0x00C3, "Razer DeathAdder V3 Pro", TXID_NEW, WAIT_NEW_MS), // 无线（新固件 PID）
    dev!(0x00C2, "Razer DeathAdder V3 Pro", TXID_NEW, WAIT_NEW_MS), // 有线（新固件 PID）
    dev!(
        0x00C5,
        "Razer DeathAdder V3 HyperSpeed",
        TXID_NEW,
        WAIT_NEW_MS
    ), // 无线
    dev!(
        0x00C4,
        "Razer DeathAdder V3 HyperSpeed",
        TXID_NEW,
        WAIT_NEW_MS
    ), // 有线
    dev!(0x00AB, "Razer Basilisk V3 Pro", TXID_NEW, WAIT_NEW_MS), // 无线
    dev!(0x00AA, "Razer Basilisk V3 Pro", TXID_NEW, WAIT_NEW_MS), // 有线
    dev!(0x00CD, "Razer Basilisk V3 Pro 35K", TXID_NEW, WAIT_NEW_MS), // 无线
    dev!(0x00CC, "Razer Basilisk V3 Pro 35K", TXID_NEW, WAIT_NEW_MS), // 有线
    dev!(
        0x00D7,
        "Razer Basilisk V3 Pro 35K Phantom Green Edition",
        TXID_NEW,
        WAIT_NEW_MS
    ), // 无线
    dev!(
        0x00D6,
        "Razer Basilisk V3 Pro 35K Phantom Green Edition",
        TXID_NEW,
        WAIT_NEW_MS
    ), // 有线
    dev!(0x009A, "Razer Pro Click Mini", TXID_NEW, WAIT_NEW_MS), // 接收器
    dev!(0x00A8, "Razer Naga V2 Pro", TXID_NEW, WAIT_NEW_MS),  // 无线
    dev!(0x00A7, "Razer Naga V2 Pro", TXID_NEW, WAIT_NEW_MS),  // 有线
    dev!(0x00B4, "Razer Naga V2 HyperSpeed", TXID_NEW, WAIT_NEW_MS), // 接收器
    dev!(
        0x00B9,
        "Razer Basilisk V3 X HyperSpeed",
        TXID_NEW,
        WAIT_NEW_MS
    ),
    dev!(0x00D4, "Razer Basilisk Mobile", TXID_NEW, WAIT_NEW_MS), // 接收器
    dev!(0x00D3, "Razer Basilisk Mobile", TXID_NEW, WAIT_NEW_MS), // 有线
    dev!(0x00BF, "Razer DeathAdder V4 Pro", TXID_NEW, WAIT_NEW_MS), // 无线
    dev!(0x00BE, "Razer DeathAdder V4 Pro", TXID_NEW, WAIT_NEW_MS), // 有线
    dev!(
        0x00C8,
        "Razer Pro Click V2 Vertical Edition",
        TXID_NEW,
        WAIT_NEW_MS
    ), // 无线
    dev!(
        0x00C7,
        "Razer Pro Click V2 Vertical Edition",
        TXID_NEW,
        WAIT_NEW_MS
    ), // 有线
    dev!(0x00D1, "Razer Pro Click V2", TXID_NEW, WAIT_NEW_MS),    // 无线
    dev!(0x00D0, "Razer Pro Click V2", TXID_NEW, WAIT_NEW_MS),    // 有线
    // ── 新代协议 · Atheris/Orochi 类 400ms ──
    dev!(0x0062, "Razer Atheris", TXID_NEW, WAIT_ATHERIS_MS), // 接收器
    dev!(0x0094, "Razer Orochi V2", TXID_NEW, WAIT_ATHERIS_MS), // 接收器（已真机验证）
    // ── 新代协议 · VIPER 族 60ms ──
    dev!(0x009F, "Razer Viper Mini SE", TXID_NEW, WAIT_VIPER_MS), // 无线
    dev!(0x009E, "Razer Viper Mini SE", TXID_NEW, WAIT_VIPER_MS), // 有线
    dev!(
        0x00B3,
        "Razer HyperPolling Wireless Dongle",
        TXID_NEW,
        WAIT_VIPER_MS
    ),
    dev!(0x00B8, "Razer Viper V3 HyperSpeed", TXID_NEW, WAIT_VIPER_MS),
    dev!(0x00C1, "Razer Viper V3 Pro", TXID_NEW, WAIT_VIPER_MS), // 无线（注意：有线为 31ms）
    // ── 新代协议 · 35K 特例 1ms（走 interface 3，由集合试探覆盖）──
    dev!(0x00CB, "Razer Basilisk V3 35K", TXID_NEW, WAIT_DEFAULT_MS),
    // ── 中代协议 txid=0x3F ──
    dev!(0x0072, "Razer Mamba Wireless", TXID_MID, WAIT_NEW_MS), // 接收器
    dev!(0x0073, "Razer Mamba Wireless", TXID_MID, WAIT_NEW_MS), // 有线
    dev!(0x007D, "Razer DeathAdder V2 Pro", TXID_MID, WAIT_VIPER_MS), // 无线
    dev!(0x007C, "Razer DeathAdder V2 Pro", TXID_MID, WAIT_VIPER_MS), // 有线
    dev!(
        0x005A,
        "Razer Lancehead Wireless",
        TXID_MID,
        WAIT_DEFAULT_MS
    ),
    dev!(0x0059, "Razer Lancehead", TXID_MID, WAIT_DEFAULT_MS), // 有线
    // ── 远古协议 txid=0xFF ──
    dev!(
        0x0083,
        "Razer Basilisk X HyperSpeed",
        TXID_LEGACY,
        WAIT_NEW_MS
    ),
    dev!(0x007B, "Razer Viper Ultimate", TXID_LEGACY, WAIT_VIPER_MS), // 无线
    dev!(0x007A, "Razer Viper Ultimate", TXID_LEGACY, WAIT_VIPER_MS), // 有线
    dev!(0x001F, "Razer Naga Epic", TXID_LEGACY, WAIT_DEFAULT_MS),
    dev!(0x0025, "Razer Mamba 2012", TXID_LEGACY, WAIT_DEFAULT_MS), // 无线
    dev!(0x0024, "Razer Mamba 2012", TXID_LEGACY, WAIT_DEFAULT_MS), // 有线
    dev!(0x0032, "Razer Ouroboros", TXID_LEGACY, WAIT_DEFAULT_MS),
    dev!(
        0x003E,
        "Razer Naga Epic Chroma",
        TXID_LEGACY,
        WAIT_DEFAULT_MS
    ),
    dev!(
        0x003F,
        "Razer Naga Epic Chroma Dock",
        TXID_LEGACY,
        WAIT_DEFAULT_MS
    ),
    dev!(0x0045, "Razer Mamba", TXID_LEGACY, WAIT_DEFAULT_MS), // 无线
    dev!(0x0044, "Razer Mamba", TXID_LEGACY, WAIT_DEFAULT_MS), // 有线
];

// ── 驱动实现 ────────────────────────────────────────────

pub(crate) struct RazerMouseDriver;

pub(crate) static RAZER_MOUSE: RazerMouseDriver = RazerMouseDriver;

impl BatteryDriver for RazerMouseDriver {
    fn matches(&self, vid: u16, pid: u16) -> bool {
        DEVICES.iter().any(|d| d.vid_pid == (vid, pid))
    }

    fn read_battery(&self, vid: u16, pid: u16) -> Result<i32, String> {
        let dev = DEVICES
            .iter()
            .find(|d| d.vid_pid == (vid, pid))
            .ok_or_else(|| format!("未收录的雷蛇设备 {:04X}:{:04X}", vid, pid))?;

        let link = HidLink::new()?;
        let paths = link.enumerate_paths(vid, pid)?;
        let request = build_report(dev.txid);
        let mut last_err = String::new();

        for _ in 0..MAX_RETRIES {
            for path in &paths {
                match link.exchange(path, &request, dev.wait_ms) {
                    Ok(resp) => match parse_level(&request, &resp) {
                        Ok(level) => return Ok(level),
                        Err(e) => last_err = e,
                    },
                    Err(e) => last_err = e,
                }
            }
            std::thread::sleep(RETRY_INTERVAL);
        }
        Err(last_err)
    }

    fn identities(&self) -> Vec<DeviceIdentity> {
        DEVICES
            .iter()
            .map(|d| DeviceIdentity {
                vid: d.vid_pid.0,
                pid: d.vid_pid.1,
                name: d.name,
                dev_type: "mouse",
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
    fn orochi_v2_reference_row_stays_verified() {
        // 已真机验证的基准行防回归：改动参数须同步真机复测
        let dev = DEVICES
            .iter()
            .find(|d| d.vid_pid == (0x1532, 0x0094))
            .expect("Orochi V2 参考行缺失");
        assert_eq!((dev.txid, dev.wait_ms), (TXID_NEW, WAIT_ATHERIS_MS));
    }

    #[test]
    fn identities_derive_one_to_one_from_table() {
        let ids = RAZER_MOUSE.identities();
        assert_eq!(ids.len(), DEVICES.len());
        assert!(ids.iter().all(|i| !i.name.is_empty()), "存在空名称身份");
        assert!(ids.iter().all(|i| i.dev_type == "mouse"));
        // 与 device_name 抽查一致性
        for i in ids.iter().take(5) {
            assert_eq!(RAZER_MOUSE.device_name(i.vid, i.pid), Some(i.name));
        }
    }
}
