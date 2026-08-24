// ── 模块职责 ─────────────────────────────────────────────
// 雷蛇 2.4G 接收器电量驱动。
// 协议来源：借鉴 OpenRazer（https://github.com/openrazer/openrazer）
// 逆向所得的协议事实（报文布局/命令字/CRC/时序），本文件为 Windows
// 用户态独立实现，运行时不依赖 OpenRazer。
//
// 设备差异收敛为两个参数（事务 ID 与响应等待时长），取值严格照抄
// OpenRazer 的两张开关表：
//   txid ← razermouse_driver.c razer_attr_read_charge_level()
//   wait ← razermouse_driver.c razer_get_report()
// 除 Orochi V2（0x0094）经实机验证外，其余型号为同族协议移植，
// 依赖回显校验/负缓存兜底；查询失败时日志附带响应原文供远程定位。

use std::time::Duration;

use super::BatteryDriver;
use crate::wireless_24g::hid_link::{HidLink, REPORT_LEN};

// ── 协议常量 ────────────────────────────────────────────

/// 命令大类：电池
const CLASS_BATTERY: u8 = 0x07;
/// 命令字：获取电量（响应 arguments[1] 为 0-255 原始刻度，需换算百分比）
const CMD_GET_BATTERY: u8 = 0x80;
/// 载荷长度
const DATA_SIZE: u8 = 0x02;
/// 整轮查询最大重试次数（与 OpenRazer 一致）
const MAX_RETRIES: usize = 5;
/// 重试间隔
const RETRY_INTERVAL: Duration = Duration::from_millis(10);

/// 响应状态码：命令成功
const STATUS_SUCCESS: u8 = 0x02;
/// 响应状态码：命令无响应/超时（鼠标休眠或离线时常见）
const STATUS_TIMEOUT: u8 = 0x04;

// ── 设备参数常量 ─────────────────────────────────────────

/// 事务 ID：新一代接收器
const TXID_NEW: u8 = 0x1F;
/// 事务 ID：中代（Lancehead/Mamba Wireless/DeathAdder V2 Pro）
const TXID_MID: u8 = 0x3F;
/// 事务 ID：远古（Mamba 2012/Ouroboros/Viper Ultimate 等）
const TXID_LEGACY: u8 = 0xFF;

/// OpenRazer 默认等待 600us，取整 1ms
const WAIT_DEFAULT_MS: u64 = 1;
/// 新一代接收器常规等待（OpenRazer 31ms）
const WAIT_NEW_MS: u64 = 31;
/// VIPER 族接收器等待（OpenRazer 59.9ms）
const WAIT_VIPER_MS: u64 = 60;
/// Atheris/Orochi 类接收器等待（OpenRazer 400ms）
const WAIT_ATHERIS_MS: u64 = 400;

// ── 设备能力表 ───────────────────────────────────────────
// 蓝牙形态 PID 不收录（电量归系统蓝牙栈）；
// 同款鼠标的有线/无线形态均收录（有线连接也可读电量）。

struct RazerDev {
    vid_pid: (u16, u16),
    /// 事务 ID
    txid: u8,
    /// 发送后等待响应的时长
    wait_ms: u64,
}

macro_rules! dev {
    ($pid:expr, $txid:expr, $wait:expr) => {
        RazerDev {
            vid_pid: (0x1532, $pid),
            txid: $txid,
            wait_ms: $wait,
        }
    };
}

static DEVICES: &[RazerDev] = &[
    // ── 新代协议 txid=0x1F · 默认 31ms ──
    dev!(0x008F, TXID_NEW, WAIT_NEW_MS), // Naga Pro 有线
    dev!(0x0090, TXID_NEW, WAIT_NEW_MS), // Naga Pro 无线
    dev!(0x0088, TXID_NEW, WAIT_NEW_MS), // Basilisk Ultimate 接收器
    dev!(0x0086, TXID_NEW, WAIT_NEW_MS), // Basilisk Ultimate 有线
    dev!(0x006F, TXID_NEW, WAIT_NEW_MS), // Lancehead Wireless 接收器
    dev!(0x0070, TXID_NEW, WAIT_NEW_MS), // Lancehead Wireless 有线
    dev!(0x0077, TXID_NEW, WAIT_NEW_MS), // Pro Click 接收器
    dev!(0x0080, TXID_NEW, WAIT_NEW_MS), // Pro Click 有线
    dev!(0x009C, TXID_NEW, WAIT_NEW_MS), // DeathAdder V2 X HyperSpeed
    dev!(0x00A6, TXID_NEW, WAIT_NEW_MS), // Viper V2 Pro 无线
    dev!(0x00A5, TXID_NEW, WAIT_NEW_MS), // Viper V2 Pro 有线
    dev!(0x00B0, TXID_NEW, WAIT_NEW_MS), // Cobra Pro 无线
    dev!(0x00AF, TXID_NEW, WAIT_NEW_MS), // Cobra Pro 有线
    dev!(0x00B7, TXID_NEW, WAIT_NEW_MS), // DeathAdder V3 Pro 无线
    dev!(0x00B6, TXID_NEW, WAIT_NEW_MS), // DeathAdder V3 Pro 有线
    dev!(0x00C3, TXID_NEW, WAIT_NEW_MS), // DeathAdder V3 Pro 无线（新固件 PID）
    dev!(0x00C2, TXID_NEW, WAIT_NEW_MS), // DeathAdder V3 Pro 有线（新固件 PID）
    dev!(0x00C5, TXID_NEW, WAIT_NEW_MS), // DeathAdder V3 HyperSpeed 无线
    dev!(0x00C4, TXID_NEW, WAIT_NEW_MS), // DeathAdder V3 HyperSpeed 有线
    dev!(0x00AB, TXID_NEW, WAIT_NEW_MS), // Basilisk V3 Pro 无线
    dev!(0x00AA, TXID_NEW, WAIT_NEW_MS), // Basilisk V3 Pro 有线
    dev!(0x00CD, TXID_NEW, WAIT_NEW_MS), // Basilisk V3 Pro 35K 无线
    dev!(0x00CC, TXID_NEW, WAIT_NEW_MS), // Basilisk V3 Pro 35K 有线
    dev!(0x00D7, TXID_NEW, WAIT_NEW_MS), // Basilisk V3 Pro 35K 幻彩绿 无线
    dev!(0x00D6, TXID_NEW, WAIT_NEW_MS), // Basilisk V3 Pro 35K 幻彩绿 有线
    dev!(0x009A, TXID_NEW, WAIT_NEW_MS), // Pro Click Mini 接收器
    dev!(0x00A8, TXID_NEW, WAIT_NEW_MS), // Naga V2 Pro 无线
    dev!(0x00A7, TXID_NEW, WAIT_NEW_MS), // Naga V2 Pro 有线
    dev!(0x00B4, TXID_NEW, WAIT_NEW_MS), // Naga V2 HyperSpeed 接收器
    dev!(0x00B9, TXID_NEW, WAIT_NEW_MS), // Basilisk V3 X HyperSpeed
    dev!(0x00D4, TXID_NEW, WAIT_NEW_MS), // Basilisk Mobile 接收器
    dev!(0x00D3, TXID_NEW, WAIT_NEW_MS), // Basilisk Mobile 有线
    dev!(0x00BF, TXID_NEW, WAIT_NEW_MS), // DeathAdder V4 Pro 无线
    dev!(0x00BE, TXID_NEW, WAIT_NEW_MS), // DeathAdder V4 Pro 有线
    dev!(0x00C0, TXID_NEW, WAIT_NEW_MS), // Viper V3 Pro 有线（注意：无线为 60ms）
    dev!(0x00C8, TXID_NEW, WAIT_NEW_MS), // Pro Click V2 垂直版 无线
    dev!(0x00C7, TXID_NEW, WAIT_NEW_MS), // Pro Click V2 垂直版 有线
    dev!(0x00D1, TXID_NEW, WAIT_NEW_MS), // Pro Click V2 无线
    dev!(0x00D0, TXID_NEW, WAIT_NEW_MS), // Pro Click V2 有线
    // ── 新代协议 · Atheris/Orochi 类 400ms ──
    dev!(0x0062, TXID_NEW, WAIT_ATHERIS_MS), // Atheris 接收器
    dev!(0x0094, TXID_NEW, WAIT_ATHERIS_MS), // Orochi V2 接收器（已真机验证）
    // ── 新代协议 · VIPER 族 60ms ──
    dev!(0x009F, TXID_NEW, WAIT_VIPER_MS), // Viper Mini SE 无线
    dev!(0x009E, TXID_NEW, WAIT_VIPER_MS), // Viper Mini SE 有线
    dev!(0x00B3, TXID_NEW, WAIT_VIPER_MS), // HyperPolling Wireless Dongle
    dev!(0x00B8, TXID_NEW, WAIT_VIPER_MS), // Viper V3 HyperSpeed
    dev!(0x00C1, TXID_NEW, WAIT_VIPER_MS), // Viper V3 Pro 无线（注意：有线为 31ms）
    // ── 新代协议 · 35K 特例 1ms（走 interface 3，由集合试探覆盖）──
    dev!(0x00CB, TXID_NEW, WAIT_DEFAULT_MS), // Basilisk V3 35K
    // ── 中代协议 txid=0x3F ──
    dev!(0x0072, TXID_MID, WAIT_NEW_MS), // Mamba Wireless 接收器
    dev!(0x0073, TXID_MID, WAIT_NEW_MS), // Mamba Wireless 有线
    dev!(0x007D, TXID_MID, WAIT_VIPER_MS), // DeathAdder V2 Pro 无线
    dev!(0x007C, TXID_MID, WAIT_VIPER_MS), // DeathAdder V2 Pro 有线
    dev!(0x005A, TXID_MID, WAIT_DEFAULT_MS), // Lancehead Wireless
    dev!(0x0059, TXID_MID, WAIT_DEFAULT_MS), // Lancehead 有线
    // ── 远古协议 txid=0xFF ──
    dev!(0x0083, TXID_LEGACY, WAIT_NEW_MS), // Basilisk X HyperSpeed
    dev!(0x007B, TXID_LEGACY, WAIT_VIPER_MS), // Viper Ultimate 无线
    dev!(0x007A, TXID_LEGACY, WAIT_VIPER_MS), // Viper Ultimate 有线
    dev!(0x001F, TXID_LEGACY, WAIT_DEFAULT_MS), // Naga Epic
    dev!(0x0025, TXID_LEGACY, WAIT_DEFAULT_MS), // Mamba 2012 无线
    dev!(0x0024, TXID_LEGACY, WAIT_DEFAULT_MS), // Mamba 2012 有线
    dev!(0x0032, TXID_LEGACY, WAIT_DEFAULT_MS), // Ouroboros
    dev!(0x003E, TXID_LEGACY, WAIT_DEFAULT_MS), // Naga Epic Chroma
    dev!(0x003F, TXID_LEGACY, WAIT_DEFAULT_MS), // Naga Epic Chroma Dock
    dev!(0x0045, TXID_LEGACY, WAIT_DEFAULT_MS), // Mamba 无线
    dev!(0x0044, TXID_LEGACY, WAIT_DEFAULT_MS), // Mamba 有线
];

// ── 驱动实现 ────────────────────────────────────────────

pub struct RazerDriver;

pub static RAZER: RazerDriver = RazerDriver;

impl BatteryDriver for RazerDriver {
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
}

// ── 报文组包与解析 ───────────────────────────────────────
// 90 字节布局：status(0) txid(1) remaining(2-3,BE16) proto(4)
//              data_size(5) class(6) id(7) args[80](8-87) crc(88) reserved(89)

/// 构造「获取电量」请求报文并填入 CRC
fn build_report(txid: u8) -> [u8; REPORT_LEN] {
    let mut report = [0u8; REPORT_LEN];
    report[1] = txid;
    report[5] = DATA_SIZE;
    report[6] = CLASS_BATTERY;
    report[7] = CMD_GET_BATTERY;
    report[88] = crc(&report);
    report
}

/// CRC：字节 [2..88) 区间逐个 XOR
fn crc(report: &[u8; REPORT_LEN]) -> u8 {
    report[2..88].iter().fold(0u8, |acc, b| acc ^ b)
}

/// 响应前 16 字节十六进制摘要（失败诊断用，随日志输出便于远程定位）
fn hex_prefix(report: &[u8; REPORT_LEN]) -> String {
    report[..16]
        .iter()
        .map(|b| format!("{:02X}", b))
        .collect::<Vec<_>>()
        .join(" ")
}

/// 校验响应回显与状态位并解析电量百分比
fn parse_level(req: &[u8; REPORT_LEN], resp: &[u8; REPORT_LEN]) -> Result<i32, String> {
    // remaining_packets(BE16)/command_class/command_id 必须与请求回显一致
    if resp[2..4] != req[2..4] || resp[6] != req[6] || resp[7] != req[7] {
        return Err(format!("响应回显不匹配: resp=[{}]", hex_prefix(resp)));
    }
    if resp[0] != STATUS_SUCCESS {
        let reason = if resp[0] == STATUS_TIMEOUT {
            "设备未响应（可能休眠/离线）"
        } else {
            "状态码异常"
        };
        return Err(format!(
            "{}: {:#04X}, resp=[{}]",
            reason,
            resp[0],
            hex_prefix(resp)
        ));
    }
    // 电量位于 arguments[1]，即字节偏移 9。
    // 该值为 0-255 原始刻度而非百分比（Orochi V2 真机实测 35 ↔ 实际 13%），
    // 按 OpenRazer daemon 同款公式 (raw/255)*100 换算，截断取整与雷蛇自家显示一致
    let raw = resp[9] as u32;
    Ok(((raw * 100) / 255) as i32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc_self_consistent_with_xor_definition() {
        let report = build_report(TXID_NEW);
        let manual = report[2..88].iter().fold(0u8, |acc, b| acc ^ b);
        assert_eq!(crc(&report), manual);
    }

    #[test]
    fn report_layout_fields() {
        let report = build_report(TXID_MID);
        assert_eq!(report[0], 0x00); // status：新命令
        assert_eq!(report[1], TXID_MID);
        assert_eq!(&report[2..4], &[0, 0]); // remaining_packets(BE16)=0
        assert_eq!(report[4], 0x00); // protocol_type
        assert_eq!(report[5], DATA_SIZE);
        assert_eq!(report[6], CLASS_BATTERY);
        assert_eq!(report[7], CMD_GET_BATTERY);
        assert_eq!(report[89], 0x00); // reserved
    }

    #[test]
    fn parse_scales_raw_0_255_to_percent() {
        let req = build_report(TXID_NEW);
        let mut resp = [0u8; REPORT_LEN];
        resp[0] = STATUS_SUCCESS;
        resp[2..4].copy_from_slice(&req[2..4]);
        resp[6] = CLASS_BATTERY;
        resp[7] = CMD_GET_BATTERY;
        // 真机标定样本：原始值 35 ↔ 实际 13%（截断取整）
        resp[9] = 35;
        assert_eq!(parse_level(&req, &resp).unwrap(), 13);
        resp[9] = 255;
        assert_eq!(parse_level(&req, &resp).unwrap(), 100);
        resp[9] = 0;
        assert_eq!(parse_level(&req, &resp).unwrap(), 0);
    }

    #[test]
    fn parse_rejects_echo_mismatch_and_non_success_status() {
        let req = build_report(TXID_NEW);

        // 命令字不回显（Synapse 并发干扰帧的典型形态）
        let mut resp = [0u8; REPORT_LEN];
        resp[0] = STATUS_SUCCESS;
        resp[7] = 0x03;
        assert!(parse_level(&req, &resp).is_err());

        // busy 态应重试而非采信
        let mut resp = [0u8; REPORT_LEN];
        resp[0] = 0x01;
        resp[2..4].copy_from_slice(&req[2..4]);
        resp[6] = CLASS_BATTERY;
        resp[7] = CMD_GET_BATTERY;
        assert!(parse_level(&req, &resp).is_err());
    }

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
}
