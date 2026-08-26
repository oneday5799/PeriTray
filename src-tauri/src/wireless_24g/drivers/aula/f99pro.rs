// ── 模块职责 ─────────────────────────────────────────────
// AULA F99 Pro/X99 Pro 无线协议（Report ID 0x13 报文族，20 字节）。
// 协议事实转录自 aula-x99-pro-driver-linux 的 PROTOCOL.md 与参考实现；
// 仅支持 2.4G 无线模式（有线固件为另一套 feature report 协议且电量 N/A）。
//
// 传输层选择（Windows 实测迭代）：
// 1. write()/read_timeout()（中断端点）→ ACCESS_DENIED：kbdclass 独占中断 OUT
// 2. send_feature_report()/get_feature_report()（控制端点）→ INVALID_FUNCTION：
//    设备 HID 描述符不含 Feature Report 定义
// 3. send_output_report()/read_timeout()（当前）→ send_output_report 走控制端点
//    （HidD_SetOutputReport，绕过 kbdclass），read_timeout 走中断 IN（已验证可达）。
//
// 收发模式：
// Output Report 是单次命令（控制端点），无残留帧队列，无需 drain。
// 固定300ms sleep + 单次 read_timeout 读取（与 hid_link::exchange 同款策略）。

use std::time::Duration;

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
/// Output Report 缓冲长（Report ID 1 字节 + 有效载荷）
const OUTPUT_BUF_LEN: usize = 1 + PACKET_SIZE;
/// read_timeout 缓冲长（Windows hidapi 可能附加 0x0 前缀，多留余量）
const READ_BUF_LEN: usize = 64;
/// 发送后等待固件处理的固定时长（参考实现：write 后 300ms sleep）
const RESPONSE_WAIT_MS: u64 = 300;
/// read_timeout 单次超时
const READ_TIMEOUT_MS: i32 = 500;

/// 读取电量百分比。
/// - `Ok(Some(pct))`：无线连接态的有效电量
/// - `Ok(None)`：设备在线但处于非无线连接态（conn_type != 0x01）
/// - `Err`：全部候选均失败
pub fn read_battery_percent(link: &HidLink) -> Result<Option<i32>, String> {
    let paths = link.enumerate_paths(VID, PID)?;
    crate::process::append_verbose_log(&format!(
        "[24g:dbg] AULA {:04X}:{:04X} 枚举到 {} 个候选集合（Output Report 模式）",
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
                    "[24g:dbg] 集合 {}/{} (page={:#06X},ifc={}) 打开失败: {} | 路径 {}",
                    i + 1,
                    paths.len(),
                    p.usage_page,
                    p.interface_number,
                    e,
                    p.path
                ));
                last_err = format!("集合 {} 打开失败: {}", i + 1, e);
                continue;
            }
        };

        match query_battery(&dev) {
            Ok(Some(pct)) => return Ok(Some(pct)),
            Ok(None) => return Err("非无线连接态，电量不可用".to_string()),
            Err(e) => {
                crate::process::append_verbose_log(&format!(
                    "[24g:dbg] 集合 {}/{} 协议交互失败: {}",
                    i + 1,
                    paths.len(),
                    e
                ));
                last_err = e;
            }
        }
    }
    Err(last_err)
}

// ── 协议细节 ────────────────────────────────────────────
//
// Output Report 发送缓冲格式（Windows hidapi）：
//   send_output_report([REPORT_ID, payload...]) → HidD_SetOutputReport（控制端点）
//
// 响应读取：
//   read_timeout(buf, timeout) → ReadFile（中断 IN 端点）
//   Windows hidapi 对 buf[0]==0x0 自动剥除 Report ID 前缀；
//   AULA 用 Report ID 0x13（非零），故 read_timeout 返回 [0x13, payload...] 完整帧。
//
// 有效载荷布局（20 字节，偏移相对于 payload 起始，即 buf[1]）：
//   [0] resp_cmd      响应命令（与请求 CMD 对应）
//   [1] pages         总页数
//   [2] page_idx      当前页索引（首页 = 0x00）
//   [3] data_len      数据区长度
//   [4] firmware_ver  固件版本
//   [5] battery_level 电量百分比（1..=100 有效）
//   [6] conn_type     连接类型（0x01 = 无线）
//   [7..18] reserved  保留区
//   [19] checksum     sum(payload[0..19]) & 0xFF（文档口径存歧义，v1 仅信息性记录）

/// 发送 Output Report → 固定等待 → read_timeout 读取响应 → 提取电量。
fn query_battery(dev: &HidDevice) -> Result<Option<i32>, String> {
    // 构造请求：Report ID + cmd + 补零
    let mut frame = [0u8; OUTPUT_BUF_LEN];
    frame[0] = REPORT_ID;
    frame[1] = CMD_GET_VERSION;
    dev.send_output_report(&frame)
        .map_err(|e| format!("Output Report 写入失败: {e}"))?;
    crate::process::append_verbose_log(&format!(
        "[24g:dbg] Output Report 已发送: {}",
        hex_prefix(&frame)
    ));

    // 等待固件处理
    std::thread::sleep(Duration::from_millis(RESPONSE_WAIT_MS));

    // 读取响应（中断 IN 端点，带超时不阻塞）
    let mut buf = [0u8; READ_BUF_LEN];
    let n = dev
        .read_timeout(&mut buf, READ_TIMEOUT_MS)
        .map_err(|e| format!("读取失败: {e}"))?;
    if n == 0 {
        return Err("响应为空（read_timeout 返回0字节）".to_string());
    }
    crate::process::append_verbose_log(&format!(
        "[24g:dbg] 已接收: {}字节 {}",
        n,
        hex_prefix(&buf[..n.min(READ_BUF_LEN)])
    ));

    // 自适应定位 payload：AULA Report ID 0x13 非零，read_timeout 不剥除，
    // buf[0]=0x13, buf[1..]=payload；若 HID 栈行为异常（buf[0]=0x00），则 buf[0..]=payload
    let (offset, payload) = if n > 0 && buf[0] == REPORT_ID {
        (1, &buf[1..n])
    } else {
        (0, &buf[..n])
    };
    crate::process::append_verbose_log(&format!(
        "[24g:dbg] Report ID 偏移={}，有效载荷 {} 字节",
        offset,
        payload.len()
    ));

    if payload.len() < 8 {
        return Err(format!("有效载荷过短: {} 字节", payload.len()));
    }
    if payload[2] != PAGE_INDEX_FIRST {
        return Err(format!(
            "非首页响应: page_idx={}（期望 {}）",
            payload[2], PAGE_INDEX_FIRST
        ));
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
        "[24g:dbg] 校验和: 帧={} 含尾={} 不含尾={}（信息性，不拒收）",
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
    if !(1..=100).contains(&level) {
        return Err(format!("电量值超出有效范围: {}", level));
    }
    Ok(Some(level))
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
    fn output_buf_len_sufficient() {
        assert_eq!(OUTPUT_BUF_LEN, 1 + PACKET_SIZE);
        assert_eq!(OUTPUT_BUF_LEN, 21);
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
