// ── 模块职责 ─────────────────────────────────────────────
// 罗技域驱动：经 Unifying/Lightspeed/Bolt 接收器读取下游设备的电量。
//
// 协议实现见 hidpp.rs（HID++ 最小自研，协议事实三方交叉验证自
// Solaar/Mouser/logiops）。
//
// 产品形态（单下游简化）：接收器仅配对一台设备时，卡片显示该设备
// 名称与电量；多设备接收器暂不支持（返回失败并记录日志）。
//
// 未经实机逐一验证：真实兼容性依赖详细档日志的社区反馈闭环。

pub(crate) mod hidpp;

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use super::{BatteryDriver, DeviceIdentity};

/// 已知罗技接收器身份（识别注册表数据源）。
/// 下游设备名称为动态枚举（HID++ 特性 0x0005），不经由此表。
const RECEIVERS: &[(&str, u16)] = &[
    ("Logitech Unifying Receiver", 0xC52B),
    ("Logitech Unifying Receiver", 0xC532),
    ("Logitech Lightspeed Receiver", 0xC539),
    ("Logitech Lightspeed Receiver", 0xC53A),
    ("Logitech Lightspeed Receiver", 0xC53F),
    ("Logitech Nano Receiver", 0xC540),
    ("Logi Bolt Receiver", 0xC548),
];

/// display_override 名称缓存：(vid,pid) → 下游设备名。
/// String 所有权存于映射，按需克隆返回。
fn override_names() -> &'static Mutex<HashMap<(u16, u16), String>> {
    static NAMES: OnceLock<Mutex<HashMap<(u16, u16), String>>> = OnceLock::new();
    NAMES.get_or_init(|| Mutex::new(HashMap::new()))
}

fn set_override_name(vid: u16, pid: u16, name: Option<String>) {
    let mut guard = crate::state::lock_unpoisoned(override_names());
    match name {
        Some(n) => {
            guard.insert((vid, pid), n);
        }
        None => {
            guard.remove(&(vid, pid));
        }
    }
}

// ── 驱动实现 ────────────────────────────────────────────

pub(crate) struct LogitechDriver;

pub(crate) static LOGITECH: LogitechDriver = LogitechDriver;

impl BatteryDriver for LogitechDriver {
    fn matches(&self, vid: u16, pid: u16) -> bool {
        vid == 0x046D && RECEIVERS.iter().any(|(_, p)| *p == pid)
    }

    fn read_battery(&self, vid: u16, pid: u16) -> Result<i32, String> {
        if !self.matches(vid, pid) {
            return Err(format!("非罗技接收器设备 {:04X}:{:04X}", vid, pid));
        }

        let link = crate::wireless_24g::hid_link::HidLink::new()?;
        let paths = link.enumerate_paths(vid, pid)?;
        let mut last_err = String::from("无可用候选集合");

        // 逐候选集合尝试完整流程（槽位扫描 → 单下游判定 → 电量读取）
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

            // 槽位扫描（兼唤醒）：统计在线下游设备
            let alive: Vec<u8> = (1..=6).filter(|s| hidpp::ping(&dev, *s)).collect();
            crate::process::append_verbose_log(&format!(
                "[24g:dbg] 集合 {}/{} 在线槽位: {:?}",
                i + 1,
                paths.len(),
                alive
            ));

            match alive.len() {
                0 => {
                    last_err = "无在线设备（可能休眠/离线）".to_string();
                    continue;
                }
                n if n > 1 => {
                    last_err = format!("多设备接收器（{} 台在线），暂不支持", n);
                    continue;
                }
                _ => {}
            }

            let slot = alive[0];
            // 读下游设备名（display_override 数据源），失败不阻断电量
            let name = hidpp::read_device_name(&dev, slot);
            set_override_name(vid, pid, name.clone());

            match hidpp::read_battery_level(&dev, slot) {
                Ok(r) => {
                    crate::process::append_verbose_log(&format!(
                        "[24g:dbg] 槽位 {} 设备 {:?} 电量 {}%（充电={}）",
                        slot, name, r.percent, r.charging
                    ));
                    return Ok(r.percent);
                }
                Err(e) => {
                    last_err = e;
                }
            }
        }
        Err(last_err)
    }

    fn identities(&self) -> Vec<DeviceIdentity> {
        RECEIVERS
            .iter()
            .map(|(name, pid)| DeviceIdentity {
                vid: 0x046D,
                pid: *pid,
                name,
                dev_type: "other",
            })
            .collect()
    }

    /// 显示名：返回接收器静态名；下游设备动态名经 display_override
    /// 通道由管线层替换卡片名（String 无法保证 'static）
    fn device_name(&self, _vid: u16, pid: u16) -> Option<&'static str> {
        RECEIVERS
            .iter()
            .find(|(_, p)| *p == pid)
            .map(|(name, _)| *name)
    }
}
