// ── 模块职责 ─────────────────────────────────────────────
// Flydigi Vader 4 Pro 旧协议（与 Apex 4 / Vader 3 Pro 共用）。
// VID:PID 04B4:2412（Cypress），4 个 HID 接口；电量查询走接口 2
// （usage page 0xFFA0），写 Report ID 0x05 的12字节命令，
// 回复嵌入 Report ID 0x04 输入流，通过 byte[15] 命令回显匹配。
//
// 协议事实转录自 Jackwmtr/flydigi-apex4-linux 的 PROTOCOL.md
// 与 dantmnf/Vader4ProReader；电量为等级 0-5（6=充电中），
// 转百分比: level * 100 / 5。
//
// Windows 实现策略：跳过 drain_input（vendor 集合上
// read_timeout 不超时会无限循环），直接写命令后在连续读取中
// 过滤回复帧（buf[15]==0xEC）。

use std::time::{Duration, Instant};

use hidapi::HidDevice;

use crate::wireless_24g::hid_link::HidLink;

pub const VID: u16 = 0x04B4;
pub const PID: u16 = 0x2412;

/// 命令码：设备信息查询（含电量）
const CMD_GET_DEVICE_INFO: u8 = 0xEC;
/// 回复中命令回显的字节偏移
const CMD_ECHO_OFFSET: usize = 15;
/// 回复中电量等级的字节偏移
const BATTERY_OFFSET: usize = 11;
/// 电量最大等级
const BATTERY_LEVEL_MAX: i32 = 5;
/// 电量充电中哨兵值
const BATTERY_CHARGING: u8 = 6;
/// 单次 read_timeout 窗口（ms）
const READ_SLICE_MS: i32 = 100;
/// 单轮查询整体截止（ms）
const DEADLINE_MS: u64 = 1000;

// ── 对外入口 ────────────────────────────────────────────

/// 读取电量百分比。Ok(None)=充电中/无效值，Err=通信失败
pub fn read_battery_percent(link: &HidLink) -> Result<Option<i32>, String> {
    let paths = link.enumerate_paths(VID, PID)?;
    // 优先 usage page 0xFFA0（vendor 命令通道）；缺失时全候选逐一试探
    let preferred: Vec<_> = paths.iter().filter(|p| p.usage_page == 0xFFA0).collect();
    let candidates: Vec<_> = if preferred.is_empty() {
        paths.iter().collect()
    } else {
        preferred
    };

    if crate::config::verbose_log_enabled() {
        let detail = candidates
            .iter()
            .enumerate()
            .map(|(i, p)| {
                format!(
                    "#{}/page={:#06X}/ifc={}",
                    i + 1,
                    p.usage_page,
                    p.interface_number
                )
            })
            .collect::<Vec<_>>()
            .join(" ");
        crate::process::append_verbose_log(&format!(
            "[24g:dbg] Flydigi {:04X}:{:04X} 候选集合 {} 个（0xFFA0 优先）: {}",
            VID,
            PID,
            candidates.len(),
            detail
        ));
    }

    for (i, p) in candidates.iter().enumerate() {
        let dev = match link.open_path_handle(&p.path) {
            Ok(d) => d,
            Err(e) => {
                if crate::config::verbose_log_enabled() {
                    crate::process::append_verbose_log(&format!(
                        "[24g:dbg] 集合 {}/{} (page={:#06X},ifc={}) 打开失败: {}",
                        i + 1,
                        candidates.len(),
                        p.usage_page,
                        p.interface_number,
                        e
                    ));
                }
                continue;
            }
        };

        // 不做 drain_input：vendor 集合上 read_timeout 不超时会无限循环
        // 直接写命令，在连续读取中过滤电池回复
        if crate::config::verbose_log_enabled() {
            crate::process::append_verbose_log(&format!(
                "[24g:dbg] 集合 {}/{} (page={:#06X},ifc={}) 写出命令 {:02X}",
                i + 1,
                candidates.len(),
                p.usage_page,
                p.interface_number,
                CMD_GET_DEVICE_INFO
            ));
        }

        match try_query(&dev) {
            Ok(Some(pct)) => {
                crate::process::append_log(&format!(
                    "[24g] Flydigi {:04X}:{:04X} 电量 {}%",
                    VID, PID, pct
                ));
                return Ok(Some(pct));
            }
            Ok(None) => {
                if crate::config::verbose_log_enabled() {
                    crate::process::append_verbose_log(&format!(
                        "[24g:dbg] 集合 {}/{} 命中回复但电量值异常（充电中或无效）",
                        i + 1,
                        candidates.len()
                    ));
                }
            }
            Err(e) => {
                if crate::config::verbose_log_enabled() {
                    crate::process::append_verbose_log(&format!(
                        "[24g:dbg] 集合 {}/{} 查询失败: {}",
                        i + 1,
                        candidates.len(),
                        e
                    ));
                }
            }
        }
    }
    Err("所有候选集合均未获得有效电量应答".to_string())
}

// ── 协议细节 ────────────────────────────────────────────

/// 构造设备信息查询命令：[0x05, 0xEC, 0x00 × 10]
fn build_command() -> [u8; 12] {
    let mut cmd = [0u8; 12];
    cmd[0] = 0x05; // Report ID
    cmd[1] = CMD_GET_DEVICE_INFO;
    cmd
}

/// 单轮查询：写出命令 → 轮询回复 → 解析电量
fn try_query(dev: &HidDevice) -> Result<Option<i32>, String> {
    let cmd = build_command();
    dev.write(&cmd)
        .map_err(|e| format!("写入命令失败: {}", e))?;

    let deadline = Instant::now() + Duration::from_millis(DEADLINE_MS);
    let mut read_count = 0u32;
    loop {
        let remain = deadline.saturating_duration_since(Instant::now());
        if remain.is_zero() {
            return Err(format!("响应超时（共读 {} 帧无匹配）", read_count));
        }
        let mut buf = [0u8; 64];
        let n = dev
            .read_timeout(&mut buf, READ_SLICE_MS.min(remain.as_millis() as i32))
            .unwrap_or(0);
        if n == 0 {
            continue;
        }
        read_count += 1;
        // 匹配命令回显：buf[15] == 0xEC
        if n > CMD_ECHO_OFFSET && buf[CMD_ECHO_OFFSET] == CMD_GET_DEVICE_INFO {
            if crate::config::verbose_log_enabled() {
                crate::process::append_verbose_log(&format!(
                    "[24g:dbg] Flydigi 命中回复（第 {} 帧）: {:02X?}",
                    read_count,
                    &buf[..n.min(20)]
                ));
            }
            return parse_battery_level(&buf[..n]);
        }
    }
}

/// 从回复帧解析电量等级并转百分比。
/// Ok(None) = 充电中或无效值（不报百分比），Err = 帧太短
fn parse_battery_level(buf: &[u8]) -> Result<Option<i32>, String> {
    if buf.len() <= BATTERY_OFFSET {
        return Err(format!("回复长度不足: {} ≤ {}", buf.len(), BATTERY_OFFSET));
    }
    let level = buf[BATTERY_OFFSET];
    if level == BATTERY_CHARGING {
        return Ok(None);
    }
    if level as i32 > BATTERY_LEVEL_MAX {
        return Err(format!("无效电量等级: {}", level));
    }
    let pct = level as i32 * 100 / BATTERY_LEVEL_MAX;
    Ok(Some(pct))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_command_format() {
        let cmd = build_command();
        assert_eq!(cmd[0], 0x05);
        assert_eq!(cmd[1], 0xEC);
        assert_eq!(cmd.len(), 12);
        assert!(cmd[2..].iter().all(|&b| b == 0));
    }

    #[test]
    fn parse_battery_level_valid_range() {
        for level in 0..=5 {
            let mut buf = [0u8; 32];
            buf[BATTERY_OFFSET] = level;
            buf[CMD_ECHO_OFFSET] = CMD_GET_DEVICE_INFO;
            let pct = parse_battery_level(&buf).unwrap();
            assert_eq!(pct, Some(level as i32 * 20));
        }
    }

    #[test]
    fn parse_battery_level_charging() {
        let mut buf = [0u8; 32];
        buf[BATTERY_OFFSET] = BATTERY_CHARGING;
        buf[CMD_ECHO_OFFSET] = CMD_GET_DEVICE_INFO;
        assert_eq!(parse_battery_level(&buf).unwrap(), None);
    }

    #[test]
    fn parse_battery_level_invalid() {
        let mut buf = [0u8; 32];
        buf[BATTERY_OFFSET] = 7;
        buf[CMD_ECHO_OFFSET] = CMD_GET_DEVICE_INFO;
        assert!(parse_battery_level(&buf).is_err());
    }

    #[test]
    fn parse_battery_level_too_short() {
        let buf = [0u8; 10];
        assert!(parse_battery_level(&buf).is_err());
    }

    #[test]
    fn vid_pid_match_known_device() {
        assert_eq!(VID, 0x04B4);
        assert_eq!(PID, 0x2412);
    }
}
