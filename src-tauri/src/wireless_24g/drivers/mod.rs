// ── 模块职责 ─────────────────────────────────────────────
// 2.4G 电量驱动抽象。组织约定：每品牌一个子目录（drivers/<品牌>/），
// 目录内 <设备类型>.rs 为该品牌对应类型的驱动文件，品牌级共享的
// 协议原语与参数常量放子目录 mod.rs；新增「品牌+类型」在此注册即可。
// 驱动同时以 identities() 声明设备身份（名称/类型），作为
// 识别注册表（device_data）的编译期内置数据源。

pub(crate) mod logitech;
pub(crate) mod razer;

/// 驱动声明的设备身份：识别注册表的编译期内置数据源。
/// dev_type 与历史 JSON 口径一致："mouse"/"keyboard"/"audio"/"other"
pub struct DeviceIdentity {
    pub vid: u16,
    pub pid: u16,
    pub name: &'static str,
    pub dev_type: &'static str,
}

/// 单台 2.4G 设备电量查询能力的统一抽象
pub trait BatteryDriver: Sync {
    /// 是否支持该 VID/PID（实现应直接扫描自身设备表，保持热路径零分配）
    fn matches(&self, vid: u16, pid: u16) -> bool;
    /// 查询电量百分比（0-100）；设备休眠/离线/未收录时返回 Err，由上层走负缓存
    fn read_battery(&self, vid: u16, pid: u16) -> Result<i32, String>;
    /// 声明收录设备的身份列表（识别注册表构建期调用一次，非热路径）
    fn identities(&self) -> Vec<DeviceIdentity>;
    /// 设备显示名（日志用，零分配；动态名经 display_override 通道）
    fn device_name(&self, vid: u16, pid: u16) -> Option<&'static str>;
    /// 动态显示名覆盖：下游设备名等运行期才能确定的名称（默认无）。
    /// 管线层存在此覆盖时将替换卡片显示名
    fn display_override(&self, _vid: u16, _pid: u16) -> Option<String> {
        None
    }
}

/// 已注册的驱动清单（新增「品牌+类型」在此追加）
pub static DRIVERS: &[&dyn BatteryDriver] = &[
    &razer::mouse::RAZER_MOUSE,
    &razer::keyboard::RAZER_KEYBOARD,
    &logitech::LOGITECH,
];

/// 在注册表中查找支持该 VID/PID 的驱动
pub fn find_driver(vid: u16, pid: u16) -> Option<&'static dyn BatteryDriver> {
    DRIVERS.iter().copied().find(|d| d.matches(vid, pid))
}
