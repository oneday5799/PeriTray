// ── 模块职责 ─────────────────────────────────────────────
// 雷蛇域驱动：收录 OpenRazer 全系支持电量上报的雷蛇无线设备
// （mouse.rs 鼠标域 64 PID / keyboard.rs 键盘域 12 PID）。
// 本文件承载跨类型共享的私有协议编解码原语与参数常量；各设备
// 类型文件以数据表 + 迷你驱动实现接入顶层注册表。
//
// 协议来源：借鉴 OpenRazer（https://github.com/openrazer/openrazer）
// 逆向所得的协议事实，Windows 用户态独立实现，运行时不依赖。
// 参数取值严格照抄其开关表：
//   txid ← razermouse_driver.c / razerkbd_driver.c 的 charge_level()
//   wait ← razermouse_driver.c razer_get_report() /
//          razerkbd_driver.c razer_get_report_params()

pub(crate) mod keyboard;
pub(crate) mod mouse;

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
const RETRY_INTERVAL: std::time::Duration = std::time::Duration::from_millis(10);

/// 响应状态码：命令成功
const STATUS_SUCCESS: u8 = 0x02;
/// 响应状态码：命令无响应/超时（鼠标休眠或离线时常见）
const STATUS_TIMEOUT: u8 = 0x04;

// ── 设备参数常量 ─────────────────────────────────────────
// 品牌级参数词汇表：鼠标与键盘域的设备表按行引用。

/// 事务 ID：新一代接收器
const TXID_NEW: u8 = 0x1F;
/// 事务 ID：键盘无线形态（BlackWidow V3/V4、DeathStalker V2 系列）
const TXID_KBD_WL: u8 = 0x9F;
/// 事务 ID：中代（Lancehead/Mamba Wireless/DeathAdder V2 Pro/BlackWidow V3 Pro 有线）
const TXID_MID: u8 = 0x3F;
/// 事务 ID：远古（Mamba 2012/Ouroboros/Viper Ultimate 等）
const TXID_LEGACY: u8 = 0xFF;

/// OpenRazer 默认等待 600us，取整 1ms（含键盘 BlackWidow Chroma 档）
const WAIT_DEFAULT_MS: u64 = 1;
/// 新一代接收器常规等待（OpenRazer 31ms）
const WAIT_NEW_MS: u64 = 31;
/// VIPER 族接收器等待（OpenRazer 59.9ms）
const WAIT_VIPER_MS: u64 = 60;
/// Atheris/Orochi 类接收器等待（OpenRazer 400ms）
const WAIT_ATHERIS_MS: u64 = 400;
/// 键盘无线形态等待（OpenRazer 4900us，BlackWidow V3/V4 WL 与 DeathStalker V2 WL 共用）
const WAIT_KBD_WL_MS: u64 = 5;

// ── 查询流程 ─────────────────────────────────────────────

/// 完整电量查询流程：枚举 HID 集合 → 组包收发 → 校验解析，
/// 含重试（与 OpenRazer 一致）。鼠标/键盘域共用。
/// 标准级输出枚举摘要；详细级输出逐路径尝试、重试轮次与成功响应原文。
fn read_battery_level(vid: u16, pid: u16, txid: u8, wait_ms: u64) -> Result<i32, String> {
    let link = HidLink::new()?;
    let paths = link.enumerate_paths(vid, pid)?;
    crate::process::append_log(&format!(
        "[24g] {:04X}:{:04X} 枚举到 {} 个候选集合",
        vid,
        pid,
        paths.len()
    ));
    if crate::config::verbose_log_enabled() {
        let detail = paths
            .iter()
            .enumerate()
            .map(|(i, p)| format!("#{}/page={:#06X}", i + 1, p.usage_page))
            .collect::<Vec<_>>()
            .join(" ");
        crate::process::append_verbose_log(&format!(
            "[24g:dbg] {:04X}:{:04X} 候选清单(txid={:#04X}, wait={}ms): {}",
            vid, pid, txid, wait_ms, detail
        ));
    }

    let request = build_report(txid);
    let mut last_err = String::new();

    for round in 0..MAX_RETRIES {
        if round > 0 && crate::config::verbose_log_enabled() {
            crate::process::append_verbose_log(&format!(
                "[24g:dbg] {:04X}:{:04X} 进入第 {}/{} 轮重试",
                vid,
                pid,
                round + 1,
                MAX_RETRIES
            ));
        }
        for (i, p) in paths.iter().enumerate() {
            match link.exchange(&p.path, &request, wait_ms) {
                Ok(resp) => {
                    if crate::config::verbose_log_enabled() {
                        crate::process::append_verbose_log(&format!(
                            "[24g:dbg] 路径 {}/{} (page={:#06X}) 响应: {}",
                            i + 1,
                            paths.len(),
                            p.usage_page,
                            hex_prefix(&resp)
                        ));
                    }
                    match parse_level(&request, &resp) {
                        Ok(level) => return Ok(level),
                        Err(e) => {
                            last_err = e.clone();
                            if crate::config::verbose_log_enabled() {
                                crate::process::append_verbose_log(&format!(
                                    "[24g:dbg] 路径 {}/{} (page={:#06X}) 解析失败: {}",
                                    i + 1,
                                    paths.len(),
                                    p.usage_page,
                                    e
                                ));
                            }
                        }
                    }
                }
                Err(e) => {
                    last_err = e.clone();
                    if crate::config::verbose_log_enabled() {
                        crate::process::append_verbose_log(&format!(
                            "[24g:dbg] 路径 {}/{} (page={:#06X}) 收发失败: {}",
                            i + 1,
                            paths.len(),
                            p.usage_page,
                            e
                        ));
                    }
                }
            }
        }
        std::thread::sleep(RETRY_INTERVAL);
    }
    Err(last_err)
}

// ── 报文组包与解析原语 ───────────────────────────────────
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
}
