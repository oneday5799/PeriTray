// ── 模块职责 ─────────────────────────────────────────────
// 雷蛇 2.4G 接收器电量驱动。
// 协议来源：借鉴 OpenRazer（https://github.com/openrazer/openrazer）
// 逆向所得的协议事实（报文布局/命令字/CRC/时序），本文件为 Windows
// 用户态独立实现，运行时不依赖 OpenRazer。

use std::time::Duration;

use super::BatteryDriver;
use crate::wireless_24g::hid_link::{HidLink, REPORT_LEN};

// ── 协议常量 ────────────────────────────────────────────

/// 命令大类：电池
const CLASS_BATTERY: u8 = 0x07;
/// 命令字：获取电量（响应 arguments[1] 为百分比）
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

// ── 设备能力表 ───────────────────────────────────────────
// 拓展同协议设备（Pro Click、Viper V2 Pro 等新一代接收器）在此加行。

struct RazerDev {
    vid_pid: (u16, u16),
    /// 事务 ID：新一代接收器固定 0x1F
    txid: u8,
    /// 发送后等待响应的时长（Atheris/Orochi 类接收器需 400ms）
    wait_ms: u64,
}

static DEVICES: &[RazerDev] = &[
    // Razer Orochi V2 接收器
    RazerDev {
        vid_pid: (0x1532, 0x0094),
        txid: 0x1F,
        wait_ms: 400,
    },
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

/// 校验响应回显与状态位并解析电量百分比
fn parse_level(req: &[u8; REPORT_LEN], resp: &[u8; REPORT_LEN]) -> Result<i32, String> {
    // remaining_packets(BE16)/command_class/command_id 必须与请求回显一致
    if resp[2..4] != req[2..4] || resp[6] != req[6] || resp[7] != req[7] {
        return Err("响应回显不匹配".to_string());
    }
    if resp[0] != STATUS_SUCCESS {
        let reason = if resp[0] == STATUS_TIMEOUT {
            "设备未响应（可能休眠/离线）"
        } else {
            "状态码异常"
        };
        return Err(format!("{}: {:#04X}", reason, resp[0]));
    }
    // 电量位于 arguments[1]，即字节偏移 9；Orochi V2 直接返回 0-100 百分比
    let level = resp[9] as i32;
    if (0..=100).contains(&level) {
        Ok(level)
    } else {
        Err(format!("电量原始值越界: {}", level))
    }
}
