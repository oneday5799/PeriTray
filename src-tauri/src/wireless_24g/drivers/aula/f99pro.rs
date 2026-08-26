// ── 模块职责 ─────────────────────────────────────────────
// AULA F99 Pro/X99 Pro 无线协议（Report ID 0x13 报文族，20 字节）。
// 协议事实转录自 aula-x99-pro-driver-linux 的 PROTOCOL.md 与参考实现；
// 仅支持 2.4G 无线模式（有线固件为另一套 feature report 协议且电量 N/A）。
//
// 未经实机验证：详细档日志为唯一远程排障来源，关键中间值全部留痕。

use std::time::{Duration, Instant};

use hidapi::HidDevice;

use crate::wireless_24g::hid_link::HidLink;

pub const VID: u16 = 0x3554;
pub const PID: u16 = 0xFA09;

/// 配置协议 Report ID
const REPORT_ID: u8 = 0x13;
/// 报文总长
const PACKET_SIZE: usize = 20;
/// 固件版本与电量查询命令
const CMD_GET_VERSION: u8 = 0x0B;
/// 应答首页页索引
const PAGE_INDEX_FIRST: u8 = 0x00;
/// 请求前清空输入缓冲的单次读取窗口
const FLUSH_READ_MS: i32 = 30;
/// 单次读取分片窗口
/// 整体截止
const DEADLINE_MS: u64 = 2000;

/// 读取电量百分比。
/// - `Ok(Some(pct))`：无线连接态的有效电量
/// - `Ok(None)`：设备在线但处于非无线连接态（byte[7]!=0x01）
/// - `Err`：枚举/打开/收发失败等原因
pub fn read_battery_percent(link: &HidLink) -> Result<Option<i32>, String> {
    let paths = link.enumerate_paths(VID, PID)?;
    crate::process::append_verbose_log(&format!(
        "[24g:dbg] AULA {:04X}:{:04X} 枚举到 {} 个候选集合",
        VID,
        PID,
        paths.len()
    ));
    let mut last_err = String::from("无可用候选集合");

    for (i, p) in paths.iter().enumerate() {
        let dev = match link.open_path_handle(&p.path) {
            Ok(d) => d,
            Err(e) => {
                crate::process::append_verbose_log(&format!(
                    "[24g:dbg] 集合 {}/{} (page={:#06X},ifc={}) 打开失败: {}",
                    i + 1,
                    paths.len(),
                    p.usage_page,
                    p.interface_number,
                    e
                ));
                last_err = format!("集合 {} 打开失败: {}", i + 1, e);
                continue;
            }
        };

        match query_battery(&dev) {
            Ok(Some(pct)) => return Ok(Some(pct)),
            Ok(None) => return Err("非无线连接态，电量不可用".to_string()),
            Err(e) => {
                last_err = e;
            }
        }
    }
    Err(last_err)
}

// ── 协议细节 ────────────────────────────────────────────

/// 发送 CMD 并收集响应，定位首页后提取电量。
///
/// 请求：`[0x13][cmd][参数区补零]`（请求不带校验和）
/// 响应：`[0x13][resp_cmd][pages][page_idx][len][data…][checksum]`
/// 校验和 = sum(bytes[0..19])，文档口径存在歧义——v1 信息性日志
/// 记录计算值不强校验，避免歧义解读误杀可用设备
fn query_battery(dev: &HidDevice) -> Result<Option<i32>, String> {
    drain_input(dev);

    let mut frame = [0u8; PACKET_SIZE];
    frame[0] = REPORT_ID;
    frame[1] = CMD_GET_VERSION;
    dev.write(&frame).map_err(|e| format!("写入失败: {e}"))?;

    let deadline = Instant::now() + Duration::from_millis(DEADLINE_MS);
    loop {
        let remain = deadline.saturating_duration_since(Instant::now());
        if remain.is_zero() {
            return Err("响应超时".to_string());
        }
        let mut buf = [0u8; 64];
        let n = dev
            .read_timeout(&mut buf, remain.as_millis().min(i32::MAX as u128) as i32)
            .map_err(|e| format!("读取失败: {e}"))?;
        if n == 0 {
            continue;
        }

        // 自适应定位 Report ID（个别 HID 栈会剥除/保留 report id 字节）
        let Some(off) = locate_report_id(&buf[..n]) else {
            continue;
        };
        if n < off + 8 {
            continue;
        }
        let body = &buf[off..off + PACKET_SIZE.min(n - off)];
        if body[3] != PAGE_INDEX_FIRST {
            continue; // 非首页跳过
        }

        // 信息性校验和：文档口径 bytes[0..18] 存歧义，两种算法均入日志
        let sum_incl = body[..19].iter().fold(0u8, |a, b| a.wrapping_add(*b));
        let sum_excl = body[..18].iter().fold(0u8, |a, b| a.wrapping_add(*b));
        crate::process::append_verbose_log(&format!(
            "[24g:dbg] 响应校验和: 帧={} 含尾={} 不含尾={}（信息性，不拒收）",
            hex_prefix(&body[..19]),
            sum_incl,
            sum_excl
        ));

        // byte[7]=连接类型（0x01=无线），byte[6]=电量百分比
        let conn = body[7];
        let level = body[6] as i32;
        crate::process::append_verbose_log(&format!(
            "[24g:dbg] 固件 v{}，连接类型 {:#04X}，原始电量字节 {}",
            body[5], conn, body[6]
        ));
        if conn != 0x01 {
            crate::process::append_verbose_log("[24g:dbg] 非无线连接态，电量不可用");
            return Ok(None);
        }
        return Ok(Some(level));
    }
}

/// 自适应定位 Report ID 字节位置（部分 HID 栈剥除首字节）
fn locate_report_id(buf: &[u8]) -> Option<usize> {
    if !buf.is_empty() && buf[0] == REPORT_ID {
        Some(0)
    } else if buf.len() > 1 && buf[1] == REPORT_ID {
        Some(1)
    } else {
        None
    }
}

/// 清空输入缓冲
fn drain_input(dev: &HidDevice) {
    let mut buf = [0u8; 64];
    while dev.read_timeout(&mut buf, FLUSH_READ_MS).unwrap_or(0) > 0 {}
}

/// 前 N 字节十六进制摘要（诊断日志用）
fn hex_prefix(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|b| format!("{b:02X}"))
        .collect::<Vec<_>>()
        .join(" ")
}
