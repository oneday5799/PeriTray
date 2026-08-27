// ── 模块职责 ─────────────────────────────────────────────
// AULA F99 Pro/X99 Pro 无线协议（Report ID 0x13 报文族，20 字节）。
// 协议事实转录自 aula-x99-pro-driver-linux 的 PROTOCOL.md；
// 仅支持 2.4G 无线模式（有线固件为另一套协议且电量 N/A）。
//
// 传输层：write()（WriteFile → 中断 OUT 端点）+ read_timeout（ReadFile → 中断 IN）。
// vendor 集合（Usage Page 0xFF02）的中断 OUT 不被 kbdclass 锁定，
// 故 write() 在 Windows 上可达。详见 Wiki「F99 Pro 排障过程」页。

use std::time::{Duration, Instant};

use crate::wireless_24g::hid_link::HidLink;

pub const VID: u16 = 0x3554;
pub const PID: u16 = 0xFA09;

/// 配置协议 Report ID
const REPORT_ID: u8 = 0x13;
/// 获取固件版本 & 电量命令
const CMD_GET_FIRMWARE: u8 = 0x0B;
/// 固件版本响应 cmd（注意：不是 0x0B，是 0x0A）
const RESPONSE_CMD: u8 = 0x0A;
/// 报文总长（Report ID + 19 字节 payload）
const PACKET_SIZE: usize = 20;
/// read_timeout 缓冲长
const READ_BUF_LEN: usize = 64;
/// 响应等待总超时
const RESPONSE_WAIT_MS: u64 = 500;
/// 单次 read_timeout 超时（循环 poll 间隔）
const POLL_INTERVAL_MS: i32 = 30;

// ── 入口 ─────────────────────────────────────────────────

/// 读取电量百分比。
/// - `Ok(Some(pct))`：无线连接态的有效电量
/// - `Ok(None)`：设备在线但处于非无线连接态（conn_type != 0x01）
/// - `Err`：全部候选均失败
pub fn read_battery_percent(link: &HidLink) -> Result<Option<i32>, String> {
    let paths = link.enumerate_paths(VID, PID)?;

    // 厂商集合优先（0xFF02/0xFF04 不被 kbdclass 锁）
    let mut vendor: Vec<_> = paths.iter().filter(|p| p.usage_page >= 0xFF00).collect();
    // 排序：0xFF02 > 0xFF04 > 其他 FFxx（protocol.md 明确两个均可用，0xFF02 优先）
    vendor.sort_by(|a, b| {
        let rank = |p: u16| match p {
            0xFF02 => 0,
            0xFF04 => 1,
            _ => 2,
        };
        rank(a.usage_page).cmp(&rank(b.usage_page))
    });
    let targets = if vendor.is_empty() {
        paths.iter().collect::<Vec<_>>()
    } else {
        vendor
    };

    crate::process::append_verbose_log(&format!(
        "[24g:dbg] AULA F99 Pro 枚举到 {} 个集合，厂商集合 {} 个，选中策略：vendor 优先",
        paths.len(),
        targets.len()
    ));
    for (i, p) in paths.iter().enumerate() {
        crate::process::append_verbose_log(&format!(
            "[24g:dbg]   候选 {}/{} page={:#06X} ifc={} {}{}",
            i + 1,
            paths.len(),
            p.usage_page,
            p.interface_number,
            p.path
                .chars()
                .rev()
                .take(30)
                .collect::<String>()
                .chars()
                .rev()
                .collect::<String>(),
            if targets.iter().any(|t| std::ptr::eq(*t, p)) {
                " ← 候选"
            } else {
                ""
            }
        ));
    }

    let mut last_err = String::from("无可用候选集合");

    for (i, p) in targets.iter().enumerate() {
        let dev = match link.open_path_handle(&p.path) {
            Ok(d) => d,
            Err(e) => {
                crate::process::append_verbose_log(&format!(
                    "[24g:dbg] 集合 {}/{} (page={:#06X},ifc={}) 打开失败: {}",
                    i + 1,
                    targets.len(),
                    p.usage_page,
                    p.interface_number,
                    e
                ));
                last_err = format!("集合 {} 打开失败: {}", i + 1, e);
                continue;
            }
        };

        match query_battery(&dev, i + 1, targets.len(), p.usage_page, p.interface_number) {
            Ok(Some(pct)) => return Ok(Some(pct)),
            Ok(None) => return Ok(None),
            Err(e) => {
                crate::process::append_verbose_log(&format!(
                    "[24g:dbg] 集合 {}/{} (page={:#06X},ifc={}) 失败: {}",
                    i + 1,
                    targets.len(),
                    p.usage_page,
                    p.interface_number,
                    e
                ));
                last_err = e;
            }
        }
    }
    Err(last_err)
}

// ── 协议细节 ────────────────────────────────────────────

/// 发送 CMD 0x0B → 循环 read_timeout 等待响应 → 解析电量。
///
/// 请求格式（20 字节）：
///   [0] 0x13 (Report ID)
///   [1] 0x0B (CMD)
///   [2..19] 0x00 (padding, 设备不校验请求校验和)
///
/// 响应格式（20 字节）：
///   [0] 0x13 (Report ID)
///   [1] 0x0A (响应 cmd, 非 0x0B)
///   [2] 总页数（单页=0x01）
///   [3] 页索引（0x00）
///   [4] 数据长度（0x04）
///   [5] 固件版本
///   [6] 电量百分比（1..=100 有效）
///   [7] 连接类型（0x01=无线）
///   [8..18] 保留
///   [19] 校验和 sum(buf[0..19]) & 0xFF
fn query_battery(
    dev: &hidapi::HidDevice,
    idx: usize,
    total: usize,
    usage_page: u16,
    ifc: i32,
) -> Result<Option<i32>, String> {
    let mut frame = [0u8; PACKET_SIZE];
    frame[0] = REPORT_ID;
    frame[1] = CMD_GET_FIRMWARE;

    dev.write(&frame).map_err(|e| format!("write 失败: {e}"))?;
    crate::process::append_verbose_log(&format!(
        "[24g:dbg] AULA F99 Pro 集合 {}/{} (page={:#06X},ifc={}) TX: {}",
        idx,
        total,
        usage_page,
        ifc,
        hex_bytes(&frame)
    ));

    // 循环 poll 响应（30ms 间隔，500ms 总超时）
    let mut buf = [0u8; READ_BUF_LEN];
    let deadline = Instant::now() + Duration::from_millis(RESPONSE_WAIT_MS);
    while Instant::now() < deadline {
        match dev.read_timeout(&mut buf, POLL_INTERVAL_MS) {
            Ok(n) if n >= 20 && buf[0] == REPORT_ID => {
                crate::process::append_verbose_log(&format!(
                    "[24g:dbg] AULA F99 Pro 集合 {}/{} RX: {}",
                    idx,
                    total,
                    hex_bytes(&buf[..20])
                ));
                return parse_battery_response(&buf[..20], idx, total, usage_page, ifc);
            }
            Ok(0) => {}
            Ok(n) => {
                crate::process::append_verbose_log(&format!(
                    "[24g:dbg] AULA F99 Pro 集合 {}/{} 短帧 {}字节: {}",
                    idx,
                    total,
                    n,
                    hex_bytes(&buf[..n])
                ));
            }
            Err(e) => return Err(format!("read 失败: {e}")),
        }
    }
    Err("响应超时（500ms）".to_string())
}

/// 解析电量响应（Linux 协议 CMD 0x0B 响应）。
fn parse_battery_response(
    buf: &[u8],
    idx: usize,
    total: usize,
    usage_page: u16,
    ifc: i32,
) -> Result<Option<i32>, String> {
    if buf[1] != RESPONSE_CMD {
        crate::process::append_verbose_log(&format!(
            "[24g:dbg] AULA F99 Pro 集合 {}/{} 非预期响应 cmd: {:#04X}（期望 {:#04X}）",
            idx, total, buf[1], RESPONSE_CMD
        ));
        return Err(format!(
            "非预期响应 cmd: {:#04X}（期望 {:#04X}）",
            buf[1], RESPONSE_CMD
        ));
    }

    let total_pages = buf[2];
    let page_idx = buf[3];
    let data_len = buf[4];
    let firmware = buf[5];
    let level = buf[6] as i32;
    let conn_type = buf[7];

    crate::process::append_verbose_log(&format!(
        "[24g:dbg] AULA F99 Pro 集合 {}/{} (page={:#06X},ifc={}) 解析: 固件={}, 数据{}字节, 页{}/{}, 连接类型={:#04X}, 电量={}",
        idx, total, usage_page, ifc, firmware, data_len, page_idx, total_pages, conn_type, level
    ));

    if conn_type != 0x01 {
        crate::process::append_verbose_log(&format!(
            "[24g:dbg] AULA F99 Pro 集合 {}/{} 非无线连接态: conn={:#04X}",
            idx, total, conn_type
        ));
        return Ok(None);
    }
    if !(1..=100).contains(&level) {
        return Err(format!("电量值超出有效范围: {}", level));
    }
    Ok(Some(level))
}

// ── 工具 ────────────────────────────────────────────────

fn hex_bytes(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|b| format!("{b:02X}"))
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vid_pid_match_known_device() {
        assert_eq!(VID, 0x3554);
        assert_eq!(PID, 0xFA09);
    }

    #[test]
    fn packet_size_is_20() {
        assert_eq!(PACKET_SIZE, 20);
    }
}
