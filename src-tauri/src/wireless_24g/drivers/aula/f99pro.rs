// ── 模块职责 ─────────────────────────────────────────────
// AULA F99 Pro/X99 Pro 无线协议（Report ID 0x13 报文族，20 字节）。
// 协议事实转录自 aula-x99-pro-driver-linux 的 PROTOCOL.md 与参考实现；
// 仅支持 2.4G 无线模式（有线固件为另一套 feature report 协议且电量 N/A）。
//
// 传输层选择：
// 参考实现用 write()/read()（中断端点），但在 Windows 上 AULA 接收器的
// 键盘 HID 接口中断 OUT 端点被系统 kbdclass 驱动独占，导致
// WriteFile 拒绝访问（ERROR_ACCESS_DENIED 0x00000005）。
// 改用 send_feature_report()/get_feature_report()（控制端点）绕过独占，
// 与罗技驱动对齐——控制端点不走中断端点，不受 kbdclass 独占影响。

use std::time::{Duration, Instant};

use hidapi::HidDevice;

use crate::wireless_24g::hid_link::HidLink;

pub const VID: u16 = 0x3554;
pub const PID: u16 = 0xFA09;

/// 配置协议 Report ID
const REPORT_ID: u8 = 0x13;
/// 有效载荷长（不含 Report ID 前缀）
const PACKET_SIZE: usize = 20;
/// 固件版本与电量查询命令
const CMD_GET_VERSION: u8 = 0x0B;
/// 应答首页页索引
const PAGE_INDEX_FIRST: u8 = 0x00;
/// Feature Report 缓冲长（Report ID 1 字节 + 有效载荷）
const FEATURE_BUF_LEN: usize = 1 + PACKET_SIZE;
/// 请求前清空残留 Feature Report 的尝试次数
const FLUSH_ATTEMPTS: usize = 10;
/// 整体截止
const DEADLINE_MS: u64 = 2000;

/// 读取电量百分比。
/// - `Ok(Some(pct))`：无线连接态的有效电量
/// - `Ok(None)`：设备在线但处于非无线连接态（conn_type != 0x01）
/// - `Err`：枚举/打开/收发失败等原因
pub fn read_battery_percent(link: &HidLink) -> Result<Option<i32>, String> {
    let paths = link.enumerate_paths(VID, PID)?;
    crate::process::append_verbose_log(&format!(
        "[24g:dbg] AULA {:04X}:{:04X} 枚举到 {} 个候选集合（Feature Report 模式）",
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
//
// Feature Report 收发缓冲格式（Windows hidapi）：
//   发送：send_feature_report([REPORT_ID, payload...])
//   接收：get_feature_report(buf) → buf[0]=REPORT_ID（回写），buf[1..]=payload
//
// 有效载荷布局（20 字节，偏移相对于 payload 起始）：
//   [0] resp_cmd      响应命令（与请求 CMD 对应）
//   [1] pages         总页数
//   [2] page_idx      当前页索引（首页 = 0x00）
//   [3] data_len      数据区长度
//   [4] firmware_ver  固件版本
//   [5] battery_level 电量百分比（1..=100 有效）
//   [6] conn_type     连接类型（0x01 = 无线）
//   [7..18] reserved  保留区
//   [19] checksum     sum(payload[0..19]) & 0xFF（文档口径存歧义，v1 仅信息性记录）

/// 发送 CMD 并收集响应，定位首页后提取电量。
fn query_battery(dev: &HidDevice) -> Result<Option<i32>, String> {
    drain_feature_queue(dev);

    // 构造请求：Report ID + cmd + 补零
    let mut frame = [0u8; FEATURE_BUF_LEN];
    frame[0] = REPORT_ID;
    frame[1] = CMD_GET_VERSION;
    dev.send_feature_report(&frame)
        .map_err(|e| format!("Feature Report 写入失败: {e}"))?;

    let deadline = Instant::now() + Duration::from_millis(DEADLINE_MS);
    loop {
        let remain = deadline.saturating_duration_since(Instant::now());
        if remain.is_zero() {
            return Err("Feature Report 响应超时".to_string());
        }
        let mut buf = [0u8; FEATURE_BUF_LEN];
        buf[0] = REPORT_ID;
        let n = dev
            .get_feature_report(&mut buf)
            .map_err(|e| format!("Feature Report 读取失败: {e}"))?;
        if n == 0 {
            std::thread::sleep(remain.min(Duration::from_millis(200)));
            continue;
        }

        // buf[1..] = 有效载荷（buf[0] 为 Report ID 回写）
        let payload = &buf[1..n.min(FEATURE_BUF_LEN)];
        if payload.len() < 8 {
            continue;
        }
        if payload[2] != PAGE_INDEX_FIRST {
            continue; // 非首页跳过
        }

        // 信息性校验和：文档口径 payload[0..18] 存歧义，两种算法均入日志
        let body_len = payload.len();
        let sum_all = payload[..body_len.min(19)]
            .iter()
            .fold(0u8, |a, b| a.wrapping_add(*b));
        let sum_no_tail = payload[..body_len.min(18)]
            .iter()
            .fold(0u8, |a, b| a.wrapping_add(*b));
        crate::process::append_verbose_log(&format!(
            "[24g:dbg] Feature Report 响应: 帧={} 含尾={} 不含尾={}（信息性，不拒收）",
            hex_prefix(&payload[..body_len.min(19)]),
            sum_all,
            sum_no_tail
        ));

        let firmware = payload[4];
        let level = payload[5] as i32;
        let conn = payload[6];
        crate::process::append_verbose_log(&format!(
            "[24g:dbg] 固件 v{}，连接类型 {:#04X}，原始电量字节 {}",
            firmware, conn, payload[5]
        ));
        if conn != 0x01 {
            crate::process::append_verbose_log("[24g:dbg] 非无线连接态，电量不可用");
            return Ok(None);
        }
        return Ok(Some(level));
    }
}

/// 清空 Feature Report 队列（部分 HID 栈残留旧帧）
fn drain_feature_queue(dev: &HidDevice) {
    for _ in 0..FLUSH_ATTEMPTS {
        let mut buf = [0u8; FEATURE_BUF_LEN];
        buf[0] = REPORT_ID;
        if dev.get_feature_report(&mut buf).unwrap_or(0) == 0 {
            break;
        }
    }
}

/// 前 N 字节十六进制摘要（诊断日志用）
fn hex_prefix(bytes: &[u8]) -> String {
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
    fn feature_buf_len_sufficient() {
        assert_eq!(FEATURE_BUF_LEN, 1 + PACKET_SIZE);
        assert_eq!(FEATURE_BUF_LEN, 21);
    }

    #[test]
    fn report_id_is_0x13() {
        assert_eq!(REPORT_ID, 0x13);
    }

    #[test]
    fn cmd_get_version_is_0x0B() {
        assert_eq!(CMD_GET_VERSION, 0x0B);
    }
}
