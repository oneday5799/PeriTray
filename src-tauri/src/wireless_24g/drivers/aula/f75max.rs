// ── 模块职责 ─────────────────────────────────────────────
// AULA F75 Max 无线协议（0x20/0x01 查询帧族）。
// dongle 枚举为 05AC:024F（VID 为 Apple 系保留值，设备 quirk），
// 控制端点按 usage page 0xFF60 raw 页选择（不看接口号）。
//
// 协议事实转录自 Aula-F75-Max-Driver/OSX/Web 三参考项目
// （互相印证）；Windows 下 hidapi write 需含 report ID 字节（0x00），
// 故仅实现 include-rid 形态，长度 64/33 两档降级尝试。
//
// 未经实机验证：详细档日志为唯一远程排障来源。

use std::time::{Duration, Instant};

use hidapi::HidDevice;

use crate::wireless_24g::hid_link::HidLink;

pub const VID: u16 = 0x05AC;
pub const PID: u16 = 0x024F;

/// 电量查询命令头
const CMD_HEADER: [u8; 2] = [0x20, 0x01];
/// 形态长度降级链（Windows 报告描述符长度不确定，参考实现同款策略）
const FORM_LENGTHS: &[usize] = &[64, 33];
/// 单次读取分片窗口（与参考实现一致）
const READ_SLICE_MS: i32 = 250;
/// 单形态整体截止
const FORM_DEADLINE_MS: u64 = 1250;
/// 请求前清空输入缓冲的单次读取窗口
const FLUSH_READ_MS: i32 = 30;

/// 读取电量百分比。Err 表示全部形态均未获得有效应答
pub fn read_battery_percent(link: &HidLink) -> Result<Option<i32>, String> {
    let paths = link.enumerate_paths(VID, PID)?;
    // F75 Max 电量走 usage page 0xFF60 raw 页；缺失时退回全部候选逐一试探
    let preferred: Vec<_> = paths.iter().filter(|p| p.usage_page == 0xFF60).collect();
    let candidates: Vec<_> = if preferred.is_empty() {
        paths.iter().collect()
    } else {
        preferred
    };
    crate::process::append_verbose_log(&format!(
        "[24g:dbg] AULA {:04X}:{:04X} 候选集合 {} 个（raw 页优先）",
        VID,
        PID,
        candidates.len()
    ));

    for (i, p) in candidates.iter().enumerate() {
        let dev = match link.open_path_handle(&p.path) {
            Ok(d) => d,
            Err(e) => {
                crate::process::append_verbose_log(&format!(
                    "[24g:dbg] 集合 {}/{} 打开失败: {} | 路径 {}",
                    i + 1,
                    candidates.len(),
                    e,
                    p.path
                ));
                continue;
            }
        };

        // 多长度降级链：任一形态命中有效百分比即返回
        for &len in FORM_LENGTHS {
            let Some(frame) = build_query(len, true) else {
                continue;
            };
            crate::process::append_verbose_log(&format!(
                "[24g:dbg] 形态 len={}: 写出帧 {}",
                len,
                hex_prefix(&frame)
            ));
            drain_input(&dev);
            if let Some(pct) = try_form(&dev, &frame, FORM_DEADLINE_MS as i64) {
                return Ok(Some(pct));
            }
        }
    }
    Err("所有形态均未获得有效电量应答".to_string())
}

// ── 协议细节 ────────────────────────────────────────────

/// 构造电量查询帧：头 `20 01` + 补零，校验位=全字节累加 &0xFF
/// （校验位自身先清零再参与求和）。include_rid=true 时首字节补
/// Report ID 占位 0x00，校验位索引右移至 32
fn build_query(len: usize, include_rid: bool) -> Option<Vec<u8>> {
    if len < 4 || len > 255 {
        return None;
    }
    let mut buf = vec![0u8; len];
    if include_rid {
        buf[1] = CMD_HEADER[0];
        buf[2] = CMD_HEADER[1];
        apply_checksum(&mut buf, 32);
    } else {
        buf[0] = CMD_HEADER[0];
        buf[1] = CMD_HEADER[1];
        apply_checksum(&mut buf, 31);
    }
    Some(buf)
}

/// 校验和：先清零校验位，累加其余全部字节后 &0xFF 写回
fn apply_checksum(bytes: &mut [u8], checksum_index: usize) {
    if checksum_index >= bytes.len() {
        return;
    }
    bytes[checksum_index] = 0;
    let sum: u8 = bytes.iter().fold(0u8, |a, b| a.wrapping_add(*b));
    bytes[checksum_index] = sum;
}

/// 自适应解析电量百分比：
/// - mac 形态 `[0]=0x20 [1]=0x01` → 百分比在 byte[3]
/// - win 形态 `[1]=0x20 [2]=0x01` → 百分比在 byte[4]
/// 有效范围 1..=100，越界或长度不足返回 None
fn parse_battery_percent(buf: &[u8]) -> Option<i32> {
    let pct = if buf.first() == Some(&0x20) && buf.get(1) == Some(&0x01) {
        *buf.get(3)?
    } else if buf.get(1) == Some(&0x20) && buf.get(2) == Some(&0x01) {
        *buf.get(4)?
    } else {
        return None;
    } as i32;
    (1..=100).contains(&pct).then_some(pct)
}

/// 单形态尝试：写入查询帧后在截止时间内轮询解析应答
fn try_form(dev: &HidDevice, frame: &[u8], deadline_ms: i64) -> Option<i32> {
    dev.write(frame).ok()?;
    let deadline = Instant::now() + Duration::from_millis(deadline_ms as u64);
    loop {
        let remain = deadline.saturating_duration_since(Instant::now());
        if remain.is_zero() {
            return None;
        }
        let mut buf = [0u8; 64];
        let n = dev
            .read_timeout(&mut buf, READ_SLICE_MS.min(remain.as_millis() as i32))
            .unwrap_or(0);
        if n == 0 {
            continue;
        }
        if let Some(pct) = parse_battery_percent(&buf[..n]) {
            crate::process::append_verbose_log(&format!(
                "[24g:dbg] 命中应答: 帧前 8 字节 {} → 电量 {}%",
                hex_prefix(&buf[..8.min(n)]),
                pct
            ));
            return Some(pct);
        }
        crate::process::append_verbose_log(&format!(
            "[24g:dbg] 收到非匹配帧（已忽略）: {}",
            hex_prefix(&buf[..n.min(16)])
        ));
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_frame_33b_with_report_id() {
        let f = build_query(33, true).unwrap();
        assert_eq!(f[0], 0x00); // report ID 占位
        assert_eq!(f[1], 0x20);
        assert_eq!(f[2], 0x01);
        assert_eq!(f[32], sum_without(&f, 32));
    }

    #[test]
    fn query_frame_64b_checksum_at_32() {
        let f = build_query(64, true).unwrap();
        assert_eq!(f.len(), 64);
        assert_eq!(f[32], sum_without(&f, 32));
    }

    #[test]
    fn query_frame_32b_without_report_id() {
        let f = build_query(32, false).unwrap();
        assert_eq!(f[0], 0x20);
        assert_eq!(f[1], 0x01);
        assert_eq!(f[31], sum_without(&f, 31));
    }

    #[test]
    fn reject_unsupported_lengths() {
        assert!(build_query(3, true).is_none()); // 校验位放不下
        assert!(build_query(300, true).is_none()); // 超 u8 索引域
    }

    #[test]
    fn parse_accepts_both_forms_in_range() {
        let mac = [0x20, 0x01, 0x00, 42];
        assert_eq!(parse_battery_percent(&mac), Some(42));
        let win = [0x00, 0x20, 0x01, 0x00, 77];
        assert_eq!(parse_battery_percent(&win), Some(77));
    }

    #[test]
    fn parse_rejects_out_of_range_percent() {
        let mac_zero = [0x20, 0x01, 0x00, 0];
        assert_eq!(parse_battery_percent(&mac_zero), None);
        let mac_over = [0x20, 0x01, 0x00, 101];
        assert_eq!(parse_battery_percent(&mac_over), None);
    }

    #[test]
    fn parse_rejects_unknown_headers() {
        assert_eq!(parse_battery_percent(&[0x13, 0x01, 0x00, 50]), None);
        assert_eq!(parse_battery_percent(&[0u8; 4]), None);
    }

    /// 独立复算：跳过校验位后的全字节累加
    fn sum_without(buf: &[u8], idx: usize) -> u8 {
        buf.iter().enumerate().fold(
            0u8,
            |a, (i, b)| if i == idx { a } else { a.wrapping_add(*b) },
        )
    }
}
