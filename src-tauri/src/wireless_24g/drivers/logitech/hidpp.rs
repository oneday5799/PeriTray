// ── 模块职责 ─────────────────────────────────────────────
// 罗技 HID++ 协议最小实现（只读电量与设备名）：
// 长报文构造 / Root Ping（唤醒+活性+回显校验）/ GetFeature 特性
// 索引发现 / 三类电池特性解析（0x1000 BatteryStatus /
// 0x1001 Voltage+插值表 / 0x1004 UnifiedBattery）/ 错误应答识别 /
// 输入缓冲清空 / 设备名分块读取。
//
// 协议事实三方交叉验证自 Solaar（权威）、Mouser（Windows/BLE 细节）、
// logiops（C++ 对照）；传输基于 hidapi crate 的 output report 写 +
// input report 超时读。均未经实机逐一验证，依赖回显校验与负缓存兜底。

use std::time::{Duration, Instant};

use hidapi::HidDevice;

// ── 协议常量 ────────────────────────────────────────────

/// HID++ 长报文总长（含 Report ID）
pub const LONG_LEN: usize = 20;
/// HID++ 短报文总长（含 Report ID，1.0 错误回显帧）
pub const SHORT_LEN: usize = 7;
/// 长报文 Report ID
const RID_LONG: u8 = 0x11;
/// 短报文 Report ID（仅 1.0 错误回显出现）
const RID_SHORT: u8 = 0x10;

/// Root 特性（固定索引 0）
const ROOT_FEATURE: u8 = 0x00;
/// Root 函数：GetFeature（特性索引发现）
const ROOT_GET_FEATURE: u8 = 0x00;
/// Root 函数：Ping（唤醒/活性/版本探测）
const ROOT_PING: u8 = 0x01;

/// 电量特性：BatteryStatus（老办公设备主流，func0）
const FEAT_BATTERY_STATUS: u16 = 0x1000;
/// 电量特性：BatteryVoltage（游戏鼠标系主流，func0，需插值表换算）
const FEAT_BATTERY_VOLTAGE: u16 = 0x1001;
/// 电量特性：UnifiedBattery（新办公设备，func1）
const FEAT_UNIFIED_BATTERY: u16 = 0x1004;
/// 设备名特性（display_override 数据源）
const FEAT_DEVICE_NAME: u16 = 0x0005;

/// 本项目软件 ID（低半字节，生态登记：0x07 OpenRGB/0x0A LGSTrayEx/
/// 0x0D G HUB，避开已占用值；应答按此过滤，广播帧 sw=0）
const SW_ID: u8 = 0x0C;

/// 槽位 Ping 超时（活设备响应 <50ms）
pub const PING_TIMEOUT_MS: i32 = 400;
/// 特性调用超时（兼容休眠唤醒首轮）
pub const CALL_TIMEOUT_MS: u64 = 1000;
/// 请求前清空输入缓冲的单次读取窗口
const FLUSH_READ_MS: i32 = 30;

/// 电量读数：百分比 + 充电状态
#[derive(Debug, Clone, Copy)]
pub struct BatteryReading {
    pub percent: i32,
    /// 充电中（含接近充满/慢充等充电态；放电与异常态为 false）
    pub charging: bool,
}

// ── 报文构造与收发 ───────────────────────────────────────

/// 构造 HID++ 长报文：[rid=0x11][dev_idx][feat][(fn<<4)|sw][params…]
/// params 不足 16 字节右侧补零
fn build_long(dev_idx: u8, feature_idx: u8, func_sw: u8, params: &[u8]) -> [u8; LONG_LEN] {
    let mut f = [0u8; LONG_LEN];
    f[0] = RID_LONG;
    f[1] = dev_idx;
    f[2] = feature_idx;
    f[3] = func_sw;
    let n = params.len().min(16);
    f[4..4 + n].copy_from_slice(&params[..n]);
    f
}

/// 组装字节 3：函数号（高半字节）| 软件 ID（低半字节）
fn func_sw(func: u8) -> u8 {
    (func << 4) | SW_ID
}

/// 错误应答判定：HID++2.0 长报文 feature 字节置 0xFF；
/// 1.0 错误回显为短报文（7 字节）且 subId 置 0x8F，以长度区分代际
fn is_error_frame(frame: &[u8]) -> bool {
    if frame.len() == SHORT_LEN {
        frame[0] == RID_SHORT && frame[2] == 0x8F
    } else {
        frame[2] == 0xFF
    }
}

/// 清空输入缓冲：丢弃积压的陈旧通知，防止污染应答匹配
fn flush_input(dev: &HidDevice) {
    let mut buf = [0u8; LONG_LEN];
    while dev.read_timeout(&mut buf, FLUSH_READ_MS).unwrap_or(0) > 0 {}
}

/// 底层收发：写长报文 → 循环读 input，直到「同槽位 + 本项目 sw_id」
/// 的帧到达或超时。不匹配帧（广播/残留）忽略继续等。
/// 返回完整帧；错误应答（feat=0xFF）原样返回，由上层判错误码。
fn transact(
    dev: &HidDevice,
    slot: u8,
    frame: &[u8; LONG_LEN],
    deadline: Instant,
) -> Result<[u8; LONG_LEN], String> {
    dev.write(frame).map_err(|e| format!("写入失败: {e}"))?;
    loop {
        let remain = deadline.saturating_duration_since(Instant::now());
        if remain.is_zero() {
            return Err("响应超时".to_string());
        }
        let mut buf = [0u8; LONG_LEN];
        let n = dev
            .read_timeout(&mut buf, remain.as_millis().min(i32::MAX as u128) as i32)
            .map_err(|e| format!("读取失败: {e}"))?;
        if n == 0 {
            continue;
        }
        if buf[1] == slot && (buf[3] & 0x0F) == SW_ID {
            return Ok(buf);
        }
    }
}

// ── 高层操作 ────────────────────────────────────────────

/// 对指定槽位发 Root Ping（宽松匹配：任意长短帧/sw_id/含 1.0 错误
/// 回显均视为存活）。存活返回 true，超时/离线返回 false。
pub fn ping(dev: &HidDevice, slot: u8) -> bool {
    flush_input(dev);
    let mut frame = build_long(
        slot,
        ROOT_FEATURE,
        (ROOT_PING << 4) | SW_ID,
        &[0x00, 0x00, 0xA1],
    );
    frame[6] = 0xA1; // 回显标记字节
    let deadline = Instant::now() + Duration::from_millis(PING_TIMEOUT_MS as u64);

    if dev.write(&frame).is_err() {
        return false;
    }
    loop {
        let remain = deadline.saturating_duration_since(Instant::now());
        if remain.is_zero() {
            return false;
        }
        let mut buf = [0u8; LONG_LEN];
        let n = dev
            .read_timeout(&mut buf, remain.as_millis().min(i32::MAX as u128) as i32)
            .unwrap_or(0);
        // 存活判定放宽：同槽位的任意合法应答（含 1.0 错误回显帧）
        if n > 0 && buf[1] == slot && (buf[0] == RID_LONG || buf[0] == RID_SHORT) {
            return true;
        }
    }
}

/// Root GetFeature：查询特性在本设备的动态索引；不存在/错误返回 None
/// （对发现流程而言，超时与不存在等价——均跳过该路径）
pub fn get_feature_index(dev: &HidDevice, slot: u8, feature: u16) -> Option<u8> {
    flush_input(dev);
    let frame = build_long(
        slot,
        ROOT_FEATURE,
        func_sw(ROOT_GET_FEATURE),
        &[(feature >> 8) as u8, feature as u8, 0],
    );
    let deadline = Instant::now() + Duration::from_millis(CALL_TIMEOUT_MS);
    let resp = transact(dev, slot, &frame, deadline).ok()?;
    let idx = resp[4];
    (idx != 0).then_some(idx)
}

/// 调用特性的指定函数，返回参数区 16 字节
fn call_feature(
    dev: &HidDevice,
    slot: u8,
    feature_idx: u8,
    func: u8,
    params: &[u8],
) -> Result<[u8; 16], String> {
    flush_input(dev);
    let frame = build_long(slot, feature_idx, func_sw(func), params);
    let deadline = Instant::now() + Duration::from_millis(CALL_TIMEOUT_MS);
    let resp = transact(dev, slot, &frame, deadline)?;
    if is_error_frame(&resp) {
        return Err(format!("HID++ 错误码 {:#04X}", resp[5]));
    }
    let mut args = [0u8; 16];
    args.copy_from_slice(&resp[4..20]);
    Ok(args)
}

/// 充电状态判定：HID++ batteryStatus 枚举 1=充电中 2=接近充满 3=充满
/// 4=慢充 均视为充电；0=放电、5=电池无效、6=热错误为非充电
fn status_charging(status: u8) -> bool {
    matches!(status, 1..=4)
}

/// 电压→百分比插值表（转录自 Solaar hidpp20.py estimate_battery_level_percentage，
/// 13 标定点线性插值）
const VOLTAGE_TABLE: [(u16, u8); 13] = [
    (4186, 100),
    (4067, 90),
    (3989, 80),
    (3922, 70),
    (3859, 60),
    (3811, 50),
    (3778, 40),
    (3751, 30),
    (3717, 20),
    (3671, 10),
    (3646, 5),
    (3579, 2),
    (3500, 0),
];

/// 电压（mV）→ 百分比线性插值，区间外截断
fn voltage_to_percent(mv: u16) -> u8 {
    let (first_mv, first_pct) = VOLTAGE_TABLE[0];
    let (last_mv, last_pct) = VOLTAGE_TABLE[VOLTAGE_TABLE.len() - 1];
    if mv >= first_mv {
        return first_pct;
    }
    if mv <= last_mv {
        return last_pct;
    }
    for w in VOLTAGE_TABLE.windows(2) {
        let (v_high, p_high) = w[0];
        let (v_low, p_low) = w[1];
        if v_low <= mv && mv <= v_high {
            let span = (v_high - v_low) as u32;
            let num = ((p_high - p_low) as u32) * (mv - v_low) as u32 + span / 2;
            return p_low + (num / span) as u8;
        }
    }
    0
}

/// 读取指定槽位设备的电量：按 0x1000 → 0x1001 → 0x1004 顺序发现并尝试，
/// 任一路径给出有效百分比即返回。全部落空 → Err（判定无电量特性）
pub fn read_battery_level(dev: &HidDevice, slot: u8) -> Result<BatteryReading, String> {
    // ── 0x1000 BatteryStatus（func0）：args[0]=放电%（0 视为无效）、args[2]=充电态 ──
    if let Some(idx) = get_feature_index(dev, slot, FEAT_BATTERY_STATUS) {
        let args = call_feature(dev, slot, idx, 0x00, &[0])?;
        if args[0] != 0 {
            crate::process::append_verbose_log(&format!(
                "[24g:dbg] 0x1000 电量 {}%（状态 {:#04X}）",
                args[0], args[2]
            ));
            return Ok(BatteryReading {
                percent: args[0] as i32,
                charging: status_charging(args[2]),
            });
        }
        crate::process::append_verbose_log("[24g:dbg] 0x1000 百分比为 0（无效），尝试下一路径");
    }

    // ── 0x1001 BatteryVoltage（func0）：args[0..2]=mV(BE)、args[2]=flags（bit7=充电中）──
    if let Some(idx) = get_feature_index(dev, slot, FEAT_BATTERY_VOLTAGE) {
        let args = call_feature(dev, slot, idx, 0x00, &[0])?;
        let mv = u16::from_be_bytes([args[0], args[1]]);
        let percent = voltage_to_percent(mv);
        crate::process::append_verbose_log(&format!(
            "[24g:dbg] 0x1001 电压 {mv}mV → 插值 {percent}%"
        ));
        return Ok(BatteryReading {
            percent: percent as i32,
            charging: args[2] & 0x80 != 0,
        });
    }

    // ── 0x1004 UnifiedBattery（func1 getBatteryLevelStatus）：与 Mouser 实测口径一致 ──
    // （Solaar 以 function 0x10 调用，上线后低半字节被 sw_id 覆盖，实际 func=1，两者等价）
    if let Some(idx) = get_feature_index(dev, slot, FEAT_UNIFIED_BATTERY) {
        let args = call_feature(dev, slot, idx, 0x01, &[0])?;
        crate::process::append_verbose_log(&format!(
            "[24g:dbg] 0x1004 电量 {}%（状态 {:#04X}）",
            args[0], args[2]
        ));
        return Ok(BatteryReading {
            percent: args[0] as i32,
            charging: status_charging(args[2]),
        });
    }

    Err("未发现电量特性（0x1000/0x1001/0x1004 均不存在）".to_string())
}

/// 读取设备名（特性 0x0005：func0 取长度、func0x10 按 offset 分块读）。
/// 名称读取失败不影响电量结果，故返回 Option。
pub fn read_device_name(dev: &HidDevice, slot: u8) -> Option<String> {
    let idx = get_feature_index(dev, slot, FEAT_DEVICE_NAME)?;
    let len_args = call_feature(dev, slot, idx, 0x00, &[0]).ok()?;
    let len = (len_args[0] as usize).min(64);
    if len == 0 {
        return None;
    }

    let mut name = Vec::with_capacity(len);
    while name.len() < len {
        let args = call_feature(dev, slot, idx, 0x10, &[name.len() as u8]).ok()?;
        let take = (len - name.len()).min(16);
        name.extend_from_slice(&args[..take]);
    }
    let end = name.iter().position(|&b| b == 0).unwrap_or(name.len());
    String::from_utf8(name[..end].to_vec()).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_long_layout() {
        let f = build_long(0x03, 0x07, func_sw(0x01), &[]);
        assert_eq!(f[0], RID_LONG);
        assert_eq!(f[1], 0x03);
        assert_eq!(f[2], 0x07);
        assert_eq!(f[3], 0x1C); // (func 1 << 4) | sw_id 0x0C
    }

    #[test]
    fn func_sw_encodes_nibbles() {
        assert_eq!(func_sw(0x00), SW_ID);
        assert_eq!(func_sw(0x01), 0x1C);
        // 高半字节函数、低半字节软件 ID（v1 不使用 >15 的函数编号）
    }

    #[test]
    fn voltage_table_boundaries() {
        assert_eq!(voltage_to_percent(4186), 100);
        assert_eq!(voltage_to_percent(4200), 100); // 区间外上截断
        assert_eq!(voltage_to_percent(3500), 0);
        assert_eq!(voltage_to_percent(3400), 0); // 区间外下截断
    }

    #[test]
    fn voltage_interpolation_matches_solaar_samples() {
        // Solaar 文档样例：3989mV=80%、3717mV=20%
        assert_eq!(voltage_to_percent(3989), 80);
        assert_eq!(voltage_to_percent(3717), 20);
        // 其余标定点直查
        assert_eq!(voltage_to_percent(4067), 90);
        assert_eq!(voltage_to_percent(3671), 10);
        assert_eq!(voltage_to_percent(3579), 2);
    }

    #[test]
    fn voltage_interpolation_midpoint_rounds() {
        // 3859(60%) 与 3811(50%) 中点 3835：线性插值 + 半 span 进位 → 55%
        let mid = (3859 + 3811) / 2;
        assert_eq!(voltage_to_percent(mid as u16), 55);
    }

    #[test]
    fn status_charging_semantics() {
        assert!(!status_charging(0)); // 放电
        assert!(status_charging(1)); // 充电中
        assert!(status_charging(2)); // 接近充满
        assert!(status_charging(3)); // 充满
        assert!(status_charging(4)); // 慢充
        assert!(!status_charging(5)); // 电池无效
        assert!(!status_charging(6)); // 热错误
    }

    #[test]
    fn is_error_frame_recognizes_both_generations() {
        let mut f20 = [0u8; LONG_LEN];
        f20[2] = 0xFF;
        assert!(is_error_frame(&f20));
        let mut f10 = [0u8; SHORT_LEN];
        f10[0] = RID_SHORT;
        f10[2] = 0x8F;
        assert!(is_error_frame(&f10));
    }
}
