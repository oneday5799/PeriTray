// ── 模块职责 ─────────────────────────────────────────────
// 2.4G 电量驱动抽象：每协议族（约等于品牌）一个子模块文件，
// 设备在各自文件内以数据表维护；新增品牌在此注册即可接入。

pub mod razer;

/// 单台 2.4G 设备电量查询能力的统一抽象
pub trait BatteryDriver: Sync {
    /// 是否支持该 VID/PID
    fn matches(&self, vid: u16, pid: u16) -> bool;
    /// 查询电量百分比（0-100）；设备休眠/离线/未收录时返回 Err，由上层走负缓存
    fn read_battery(&self, vid: u16, pid: u16) -> Result<i32, String>;
}

/// 已注册的驱动清单（新增品牌在此追加）
pub static DRIVERS: &[&dyn BatteryDriver] = &[&razer::RAZER];

/// 在注册表中查找支持该 VID/PID 的驱动
pub fn find_driver(vid: u16, pid: u16) -> Option<&'static dyn BatteryDriver> {
    DRIVERS.iter().copied().find(|d| d.matches(vid, pid))
}
