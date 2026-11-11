// ── 模块职责 ─────────────────────────────────────────────
// Flydigi Vader 5 Pro（飞智黑武士 5 Pro）电量查询：V2 协议 GET_INFO。
// VID:PID 37D7:2401 —— 有线与 2.4G dongle 共用手柄接口身份（连接字节区分
// Wired/Wireless），vendor 集合 usage page 0xFFA0 / usage 0x0001。
// 协议事实转录自 vermi5/VaderBatteryTray（实机 USBPcap + 真机验证的 C# 实现）：
// - 请求 33 字节：[Report ID 0x00] 5A A5 01 02 00 + 零填充
// - 回复帧在偏移 0/1 处含魔数 5A A5 01（取决于帧有无 Report ID 前缀）
// - byte[+5]=设备 ID（0x82=Vader 5 Pro），byte[+6]=连接方式（1 有线/2 无线）
// - byte[+15]=固件版本高字节、byte[+16]=低字节（大端序；低于 0x7141 的固件
//   不保证本协议语义）
// - byte[+11]：高 nibble=状态（0 放电/1 充电/2 已充满），低 nibble=电量档 0-6
//   转百分比 10/25/40/55/70/85/100 —— 固件只报 7 档序数，非真实百分比；
//   充电态（1）照带档位字节，本仓充电中也报百分比（与 V4 Pro 充电哨兵不同）
// 与 Vader 4 Pro 旧协议（04B4:2412、Report ID 0x05/0xEC、6 档）完全不同，勿合并。

use std::time::{Duration, Instant};

use hidapi::HidDevice;

use crate::wireless_24g::hid_link::HidLink;
use crate::{standard_log, verbose_log};

pub const VID: u16 = 0x37D7;
pub const PID: u16 = 0x2401;

/// 帧魔数第一字节
const MAGIC1: u8 = 0x5A;
/// 帧魔数第二字节
const MAGIC2: u8 = 0xA5;
/// 命令码：GET_INFO（设备信息查询，含电量）
const CMD_GET_INFO: u8 = 0x01;
/// GET_INFO 应答的设备 ID（0x82 = Vader 5 Pro）
const DEVICE_ID: u8 = 0x82;
/// 支持本协议的最低固件版本（低版本回复语义不保证，按不支持处理）
const MIN_FIRMWARE: u16 = 0x7141;
/// 电量档位 → 百分比映射（7 档，转录自参考实现的 BatteryDisplayScale）
const LEVEL_PERCENTS: [i32; 7] = [10, 25, 40, 55, 70, 85, 100];
/// 状态 nibble：放电
const STATUS_DISCHARGING: u8 = 0;
/// 状态 nibble：充电中（仍携带档位字节）
const STATUS_CHARGING: u8 = 1;
/// 状态 nibble：已充满（返回 100%，非充电中）
const STATUS_CHARGED: u8 = 2;
/// 回复帧负载最小长度（魔数后须覆盖到固件字段，转录自参考实现的偏移 +31 判据）
const REPLY_MIN: usize = 31;
/// 回复帧各字段相对魔数帧头的字节偏移（转录自参考实现 DecodeInfoResponse）
const OFF_DEVICE_ID: usize = 5;
/// 连接方式字段偏移（1=有线 / 2=无线）
const OFF_CONNECTION: usize = 6;
/// 状态+档位复合字节偏移（高 nibble=状态 / 低 nibble=档位）
const OFF_STATUS_LEVEL: usize = 11;
/// 固件版本高字节偏移（低字节在其后一字；**大端**，见 decode_reply 内依据）
const OFF_FIRMWARE: usize = 15;
/// 请求帧总长（Report ID 前缀 + 32 字节负载）
const REQUEST_LEN: usize = 33;
/// 单次 read_timeout 窗口（ms）
const READ_SLICE_MS: i32 = 100;
/// 单轮查询整体截止（ms）
const DEADLINE_MS: u64 = 1000;
/// 临界档复查延迟（ms）：给唤醒初始化值留出被真实遥测替换的时间
const CONFIRM_DELAY_MS: u64 = 1000;

// ── 对外入口 ────────────────────────────────────────────

/// 读取电量百分比。充电中照报档位（协议里状态与档位是独立字节）；
/// Err=通信失败/协议不符。
///
/// `scope` 是设备容器 GUID：同型号两台手柄的 HID 集合同 VID 同 PID，
/// 不分域会读到另一台的电量。`None` = 调用方拿不到容器（不做分域）。
pub fn read_battery_percent(link: &HidLink, scope: Option<&str>) -> Result<Option<i32>, String> {
    let paths = link.enumerate_paths(VID, PID, scope)?;
    let candidates = super::prefer_vendor_page(&paths);
    // 标准级枚举摘要：默认日志下排查「不出电量」的第一线索（与 razer 域同口径）
    standard_log!(
        "[24g] Flydigi V2 {:04X}:{:04X} 枚举到 {} 个候选集合",
        VID,
        PID,
        candidates.len()
    );

    if crate::config::verbose_log_enabled() {
        let detail = candidates
            .iter()
            .enumerate()
            .map(|(i, p)| {
                format!(
                    "#{}/page={:#06X}/usage={:#06X}/ifc={}",
                    i + 1,
                    p.usage_page,
                    p.usage,
                    p.interface_number
                )
            })
            .collect::<Vec<_>>()
            .join(" ");
        verbose_log!(
            "[24g:dbg] Flydigi V2 {:04X}:{:04X} 候选集合 {} 个（0xFFA0:0x0001 优先）: {}",
            VID,
            PID,
            candidates.len(),
            detail
        );
    }

    let mut last_err = String::new();
    for (i, p) in candidates.iter().enumerate() {
        let dev = match link.open_path_handle(&p.path) {
            Ok(d) => d,
            Err(e) => {
                // 打开失败同样计入 last_err：全部候选都打不开时，最终错误
                // 不能退化成无信息量的「均未应答」（排查要的是「为什么」）
                last_err = format!(
                    "集合 {}/{} (page={:#06X},ifc={}) 打开失败: {}",
                    i + 1,
                    candidates.len(),
                    p.usage_page,
                    p.interface_number,
                    e
                );
                if crate::config::verbose_log_enabled() {
                    verbose_log!("[24g:dbg] {}", last_err);
                }
                continue;
            }
        };

        // 不做 drain_input：vendor 集合上 read_timeout 不超时会无限循环
        // 直接写命令，在连续读取中按魔数过滤回复帧
        match try_query(&dev) {
            Ok(Some(pct)) => return Ok(Some(pct)),
            Ok(None) => {
                last_err = format!(
                    "集合 {}/{} 命中回复但无有效电量值（防御分支）",
                    i + 1,
                    candidates.len()
                );
                if crate::config::verbose_log_enabled() {
                    verbose_log!("[24g:dbg] {}", last_err);
                }
            }
            Err(e) => {
                last_err = format!(
                    "集合 {}/{} (page={:#06X},ifc={}) 查询失败: {}",
                    i + 1,
                    candidates.len(),
                    p.usage_page,
                    p.interface_number,
                    e
                );
                if crate::config::verbose_log_enabled() {
                    verbose_log!("[24g:dbg] {}", last_err);
                }
            }
        }
    }
    // ⭐ 最终 Err 必须携带 last_err（最后一害的真实原因）：query_and_cache 会把
    //    本返回值打进标准级日志（「查询失败: …」）⇒ 默认日志即可看到
    //    设备 ID 不符 / 固件过低 / 复查未确认等关键排查信息（razer 域同口径）
    Err(if last_err.is_empty() {
        "无候选集合可查询".to_string()
    } else {
        last_err
    })
}

// ── 协议细节 ────────────────────────────────────────────

/// 构造 GET_INFO 请求：[0x00, 0x5A, 0xA5, 0x01, 0x02, 0x00, 0x00 × 27]
fn build_command() -> [u8; REQUEST_LEN] {
    let mut cmd = [0u8; REQUEST_LEN];
    cmd[0] = 0x00; // Report ID（设备无编号报告）
    cmd[1] = MAGIC1;
    cmd[2] = MAGIC2;
    cmd[3] = CMD_GET_INFO;
    cmd[4] = 0x02;
    cmd[5] = 0x00;
    cmd
}

/// 在一帧输入报告里定位 GET_INFO 回复。
/// 帧首字节是魔数时回复从 0 起，否则视为带 1 字节 Report ID 前缀；
/// 魔数 + 命令码 + 长度三者齐备才算命中，其余帧（手柄按键上报等）继续读。
fn locate_reply(buf: &[u8]) -> Option<usize> {
    let offset = if buf.first() == Some(&MAGIC1) { 0 } else { 1 };
    if buf.len() >= offset + REPLY_MIN
        && buf.get(offset) == Some(&MAGIC1)
        && buf.get(offset + 1) == Some(&MAGIC2)
        && buf.get(offset + 2) == Some(&CMD_GET_INFO)
    {
        Some(offset)
    } else {
        None
    }
}

/// 单轮查询：写出命令 → 轮询回复 → 解析电量（临界档读数经一次性复查）
fn try_query(dev: &HidDevice) -> Result<Option<i32>, String> {
    let first = query_once(dev)?;
    if first == Some(LEVEL_PERCENTS[0]) {
        // 临界档须复查：唤醒初期的 GET_INFO 回复可能是控制器初始化值（档位 0
        // 闪现，参考实现 1.2.0-rc.1 的 wake filter）。本仓无展示层 ⇒ 无状态化——
        // 延迟复查一次，复查值优先（临界值复现则发布、更高值则替换闪现值）；
        // 复查失败/超时不发布 ⇒ 上层 SWR 保旧值，真临界由下轮 TTL 补上
        verbose_log!(
            "[24g:dbg] Flydigi V2 首读临界档 {}%（疑似唤醒初始化值），{}ms 后复查",
            LEVEL_PERCENTS[0],
            CONFIRM_DELAY_MS
        );
        std::thread::sleep(Duration::from_millis(CONFIRM_DELAY_MS));
        return match query_once(dev) {
            Ok(Some(confirmed)) => {
                verbose_log!(
                    "[24g:dbg] Flydigi V2 临界档复查: {}%（{}）",
                    confirmed,
                    if confirmed == LEVEL_PERCENTS[0] {
                        "复现，发布"
                    } else {
                        "首读为初始化值，已替换"
                    }
                );
                Ok(Some(confirmed))
            }
            _ => {
                verbose_log!("[24g:dbg] Flydigi V2 临界档复查未获回复，本轮不发布");
                Err("临界档读数复查未确认（可能为唤醒初始化值），本轮不发布".to_string())
            }
        };
    }
    Ok(first)
}

/// 一次写出命令并轮询回复
fn query_once(dev: &HidDevice) -> Result<Option<i32>, String> {
    let cmd = build_command();
    dev.write(&cmd)
        .map_err(|e| format!("写入命令失败: {}", e))?;

    let deadline = Instant::now() + Duration::from_millis(DEADLINE_MS);
    let mut read_count = 0u32;
    let mut sample_logged = false;
    loop {
        let remain = deadline.saturating_duration_since(Instant::now());
        if remain.is_zero() {
            // 0 帧 = 设备完全沉默（休眠/离线/非命令通道）；N>0 帧 = 有输入流
            // 但回帧不匹配——协议漂移或按键上报混流，排查方向相反
            return if read_count == 0 {
                Err("响应超时（未收到任何输入帧——设备可能休眠或该集合非命令通道）".to_string())
            } else {
                Err(format!(
                    "响应超时（共读 {} 帧，均非 GET_INFO 回复）",
                    read_count
                ))
            };
        }
        let mut buf = [0u8; 64];
        let n = dev
            .read_timeout(&mut buf, READ_SLICE_MS.min(remain.as_millis() as i32))
            .unwrap_or(0);
        if n == 0 {
            continue;
        }
        read_count += 1;
        if let Some(offset) = locate_reply(&buf[..n]) {
            if crate::config::verbose_log_enabled() {
                verbose_log!(
                    "[24g:dbg] Flydigi V2 命中回复（第 {} 帧，offset {}）: {}",
                    read_count,
                    offset,
                    hex_prefix(&buf[..n.min(33)])
                );
            }
            return decode_reply(&buf[..n], offset);
        }
        // 只留首个非匹配帧样本：协议漂移时看它就知道回的是什么格式；
        // 活跃游玩期按键上报会连绵不断，逐帧记会刷屏
        if !sample_logged && crate::config::verbose_log_enabled() {
            sample_logged = true;
            verbose_log!(
                "[24g:dbg] Flydigi V2 首个非匹配帧样本（len {}）: {}",
                n,
                hex_prefix(&buf[..n.min(33)])
            );
        }
    }
}

/// 从回复帧解析电量。恒返回 Some（充电中照报档位，见 read_battery_percent）；
/// Err=设备/固件/档位不符或帧损坏
fn decode_reply(buf: &[u8], offset: usize) -> Result<Option<i32>, String> {
    let device_id = buf[offset + OFF_DEVICE_ID];
    if device_id != DEVICE_ID {
        return Err(format!("非 Vader 5 Pro 设备 ID: {:#04X}", device_id));
    }
    // 大端：参考实现 DecodeInfoResponse 组装为 [+15]<<8 | [+16]（高字节在前）。
    // ⛔ 不得改回小端——固件 0x7141 线上字节为 [71 41]，小端读得 0x4171 <
    // 0x7141，恒判「固件版本过低」，真机永远不出电量（decode_firmware_is_big_endian 钉死）
    let firmware = u16::from_be_bytes([buf[offset + OFF_FIRMWARE], buf[offset + OFF_FIRMWARE + 1]]);
    if firmware < MIN_FIRMWARE {
        return Err(format!(
            "固件版本过低: {:#06X}（低于 {:#06X}）",
            firmware, MIN_FIRMWARE
        ));
    }
    let status_level = buf[offset + OFF_STATUS_LEVEL];
    if crate::config::verbose_log_enabled() {
        let conn = match buf[offset + OFF_CONNECTION] {
            1 => "有线",
            2 => "无线",
            _ => "未知",
        };
        // 状态字必须随详细日志输出：充电态的百分比是档位映射值而非实测值，
        // 状态字节是分辨两者的唯一依据
        let state = match status_level >> 4 {
            0 => "放电",
            1 => "充电中",
            2 => "已充满",
            _ => "未知",
        };
        verbose_log!(
            "[24g:dbg] Flydigi V2 连接方式: {}（{:#04X}），状态: {}，原始字节 {:#04X}，固件 {:#06X}",
            conn,
            buf[offset + OFF_CONNECTION],
            state,
            status_level,
            firmware
        );
    }

    let status = (status_level >> 4) & 0x0F;
    let level = (status_level & 0x0F) as usize;
    match status {
        // 放电与充电中同报档位百分比（协议里充电态照带档位字节，与参考实现一致）；
        // 本仓语义：充电中不再隐藏电量（Vader 4 Pro 的充电哨兵顶掉档位值才无法照报）
        STATUS_DISCHARGING | STATUS_CHARGING => LEVEL_PERCENTS
            .get(level)
            .map(|&pct| Some(pct))
            .ok_or_else(|| format!("无效电量档位: {}", level)),
        STATUS_CHARGED => Ok(Some(100)),
        _ => Err(format!("未知状态 nibble: {}", status)),
    }
}

/// 字节序列 → 空格分隔的大写十六进制（帧样本日志用，aula 域同款）
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

    /// 造一帧完整 GET_INFO 回复（测试专用）：前缀 + 魔数 + 可定制字段
    fn make_reply(
        report_id_prefix: bool,
        device_id: u8,
        firmware: u16,
        status_level: u8,
    ) -> Vec<u8> {
        let mut frame = vec![];
        if report_id_prefix {
            frame.push(0x00); // Report ID 前缀（无编号报告为 0）
        }
        let base = frame.len(); // 回复负载基址：带前缀 1 / 不带 0
        frame.resize(base + REPLY_MIN, 0); // 一次补足最小负载长度，字段按协议偏移填入
        frame[base..base + 3].copy_from_slice(&[MAGIC1, MAGIC2, CMD_GET_INFO]);
        frame[base + OFF_DEVICE_ID] = device_id;
        frame[base + OFF_CONNECTION] = 2; // 连接方式 = 无线
        frame[base + OFF_STATUS_LEVEL] = status_level;
        frame[base + OFF_FIRMWARE..base + OFF_FIRMWARE + 2]
            .copy_from_slice(&firmware.to_be_bytes()); // 大端：高字节在前
        frame
    }

    #[test]
    fn decode_firmware_is_big_endian() {
        // 大端钉死：固件 0x7141 线上字节序为 [71 41]（参考实现 DecodeInfoResponse
        // 组装为 [+15]<<8 | [+16]，高字节在前）。误读小端得 0x4171 < 0x7141，
        // 恒判「固件版本过低」⇒ 真机永远不出电量。可证伪：from_be_bytes 改回
        // from_le_bytes 本用例必转红
        let frame = make_reply(false, DEVICE_ID, MIN_FIRMWARE, 0x00);
        assert_eq!(frame[OFF_FIRMWARE], 0x71);
        assert_eq!(frame[OFF_FIRMWARE + 1], 0x41);
        assert_eq!(decode_reply(&frame, 0).unwrap(), Some(10));
        // 反例：字节交换后 0x4171 < 0x7141，必须按固件过低拒收
        let mut swapped = frame.clone();
        swapped.swap(OFF_FIRMWARE, OFF_FIRMWARE + 1);
        assert!(decode_reply(&swapped, 0).is_err());
    }

    #[test]
    fn build_command_format() {
        let cmd = build_command();
        assert_eq!(cmd.len(), 33);
        assert_eq!(&cmd[..6], &[0x00, 0x5A, 0xA5, 0x01, 0x02, 0x00]);
        assert!(cmd[6..].iter().all(|&b| b == 0));
    }

    #[test]
    fn locate_reply_with_report_id_prefix() {
        let frame = make_reply(true, DEVICE_ID, MIN_FIRMWARE, 0x00);
        assert_eq!(locate_reply(&frame), Some(1));
    }

    #[test]
    fn locate_reply_without_prefix() {
        let frame = make_reply(false, DEVICE_ID, MIN_FIRMWARE, 0x00);
        assert_eq!(locate_reply(&frame), Some(0));
    }

    #[test]
    fn locate_reply_rejects_non_reply_frames() {
        // 手柄按键上报等噪声帧不得命中
        let mut noise = vec![0x00, 0x01, 0x02, 0x03];
        noise.resize(40, 0x55);
        assert_eq!(locate_reply(&noise), None);
        // 魔数齐全但命令码不符
        let mut bad_cmd = vec![0x00, 0x5A, 0xA5, 0x02];
        bad_cmd.resize(40, 0);
        assert_eq!(locate_reply(&bad_cmd), None);
        // 魔数齐全但帧长不足
        let short = vec![MAGIC1, MAGIC2, CMD_GET_INFO, 0u8, 0, 0];
        assert_eq!(locate_reply(&short), None);
    }

    #[test]
    fn decode_seven_levels_discharging() {
        for (level, expect) in LEVEL_PERCENTS.iter().enumerate() {
            let frame = make_reply(false, DEVICE_ID, MIN_FIRMWARE, level as u8);
            assert_eq!(
                decode_reply(&frame, 0).unwrap(),
                Some(*expect),
                "档位 {} 应转 {}%",
                level,
                expect
            );
        }
    }

    #[test]
    fn decode_charging_reports_level() {
        // 高 nibble=1 充电中：低 nibble 档位照报百分比（本仓语义，不隐藏电量）
        let frame = make_reply(false, DEVICE_ID, MIN_FIRMWARE, (STATUS_CHARGING << 4) | 3);
        assert_eq!(decode_reply(&frame, 0).unwrap(), Some(55));
    }

    #[test]
    fn decode_charged_returns_100() {
        // 高 nibble=2 已充满：无论档位，恒 100%
        let frame = make_reply(false, DEVICE_ID, MIN_FIRMWARE, STATUS_CHARGED << 4);
        assert_eq!(decode_reply(&frame, 0).unwrap(), Some(100));
    }

    #[test]
    fn decode_wrong_device_id_rejected() {
        let frame = make_reply(false, 0x81, MIN_FIRMWARE, 0x00);
        assert!(decode_reply(&frame, 0).is_err());
    }

    #[test]
    fn decode_old_firmware_rejected() {
        let frame = make_reply(false, DEVICE_ID, MIN_FIRMWARE - 1, 0x00);
        assert!(decode_reply(&frame, 0).is_err());
    }

    #[test]
    fn decode_invalid_level_rejected() {
        // 低 nibble=7 超出 0-6 档位范围
        let frame = make_reply(false, DEVICE_ID, MIN_FIRMWARE, 0x07);
        assert!(decode_reply(&frame, 0).is_err());
    }

    #[test]
    fn decode_unknown_status_rejected() {
        let frame = make_reply(false, DEVICE_ID, MIN_FIRMWARE, (0x0F << 4) | 3);
        assert!(decode_reply(&frame, 0).is_err());
    }

    #[test]
    fn decode_with_report_id_prefix() {
        // 带前缀的帧按 offset=1 解码，结果与不带前缀一致
        let frame = make_reply(true, DEVICE_ID, MIN_FIRMWARE, 2);
        assert_eq!(decode_reply(&frame, 1).unwrap(), Some(40));
    }

    #[test]
    fn vid_pid_match_reference() {
        assert_eq!(VID, 0x37D7);
        assert_eq!(PID, 0x2401);
    }
}
